//! The background plugin-run registry.
//!
//! A `Driver::run` is blocking and long, so the host runs it on a background
//! thread and tracks it here. The registry is long-lived shared state
//! (`Arc<Mutex<RunRegistry>>` owned by `serve`), NOT owned by the
//! `PluginsExternal` — that External is a throwaway projection rebuilt per
//! request (R969), so an owned registry would be lost each request.
//!
//! Each run carries its own `Arc<Mutex<RunState>>` cell; the worker thread
//! holds only that cell (never the registry), so reading the registry never
//! blocks behind a running plugin.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::Value;
use sprag_plugin::{Outcome, Progress, ProgressCell};
use sprag_terminal::PaneId;

use crate::external::lock;

/// A stable, monotonic identifier for a background plugin run.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RunId(pub u64);

/// The lifecycle of one background plugin run.
#[derive(Clone, Debug)]
pub enum RunState {
    /// The worker thread is still driving the plugin.
    Running,
    /// The run finished with this outcome, plus any content the plugin captured
    /// (an AI adapter's reply); `output` is `None` for control plugins.
    ///
    /// ⚠ **BOXED, and the reason is that this variant grows and its siblings do not.** An
    /// [`Outcome`] carries a failure, what became of the work, and — since R366 — what the peer was
    /// asking and why nothing was answered; it passed 256 bytes when the last of those landed,
    /// while `Running` and `Interrupted` carry nothing at all. Unboxed, every live run's state cell
    /// pays the terminal record's size for its whole life, which is backwards: the payload matters
    /// once, at the end, and is read rarely after that.
    Done {
        outcome: Box<Outcome>,
        output: Option<String>,
        /// ⛔⛔⛔⛔⛔ **WHAT THE TREE THIS RUN WORKED IN WAS HOLDING WHEN IT ENDED**, in bytes of
        /// `git diff HEAD` — register item 682's commit-contamination clause.
        ///
        /// # ⚠⚠⚠⚠⚠ The cost this exists to stop, measured before it was written
        ///
        /// A run died mid-edit and left its agent's half-applied mutation in the shared tree: one
        /// deleted line in `deliver.rs`, which the NEXT person's commit would have shipped —
        /// re-introducing the defect register item 669 had just repaired. The only reason it was
        /// caught is that somebody ran the whole suite before committing, and a round that skips
        /// the sweep does not see it. **The dead writer cannot put its own work back**, so the
        /// question has to be asked by something that outlives it.
        ///
        /// ⚠⚠ **IT DOES NOT SAY THIS RUN LEFT IT, and the wording matters because a tree has more
        /// than one writer** (item 196). A person and another run can both be editing, so
        /// attribution is not available to anybody here — what IS available, and is what the reader
        /// needs before committing, is *this tree is holding something no commit does*.
        ///
        /// ⚠ [`None`] is **cannot say** — a run whose pane named no directory, a directory that is
        /// no repository, a `git` that is not installed — never *clean*, which is `Some(0)`. Item
        /// 709's discipline: a fabricated zero here would be the sentence *nothing was left behind*
        /// on no evidence, which is the accident this field exists to prevent, arriving by the
        /// other road.
        uncommitted: Option<usize>,
    },
    /// **THE RUN FINISHED IN ANOTHER PROCESS AND THIS IS WHAT IT REPORTED** — register items 650
    /// and 544.
    ///
    /// # ⚠⚠⚠⚠⚠ Why this is a fifth state and not a [`Done`](Self::Done) with a rebuilt `Outcome`
    ///
    /// An [`Outcome`] has no way back from the wire. `crate::plugins::outcome_to_json` is a one-way
    /// RENDER: it drops `screened`, `deliveries`, `checks` and `banked`, so a daemon that
    /// "reconstructed" one would be asserting four facts it was never told. A `Done` built that way
    /// would be **indistinguishable from a real one and quietly wrong** — an out-of-process run
    /// losing what an in-process one keeps, which is the invisible divergence
    /// [`crate::options::RUN_DRIVER_PROCESS`] promises cannot happen.
    ///
    /// So the honest shape is a state that says *this ending was reported, not computed here*, and
    /// carries exactly what arrived. Every reader that weighs an outcome then has to say what it
    /// does with a reported one — and the compiler makes it, which is the whole reason this is a
    /// variant rather than a field.
    ///
    /// ⚠⚠ **THE WIRE DOES NOT LEARN A FIFTH WORD.** The row publishes `done` with the reported
    /// object spliced in, because a client asked *what became of this run* and *whose process
    /// computed the answer* is not part of that question. Item 342's rule — an added `status` word
    /// is a break no address pin can see — is exactly why this stays on this side.
    ///
    /// ⚠ Boxed for [`Done`](Self::Done)'s reason: the payload matters once, at the end.
    Reported(Box<Value>),
    /// The worker thread panicked (defensive — a plugin step should not).
    Panicked(String),
    /// ⚠⚠ THE DAEMON THAT WAS DRIVING THIS RUN DIED. It was `Running` when its process ended, and
    /// nothing resumed it: a run is a thread over live panes, and neither survives a restart.
    ///
    /// # Why a fourth state and not silence
    ///
    /// Before it, a restart left `runs` answering *"no runs"* — the same answer as a daemon nobody
    /// has ever asked for a loop. A person who started a bounded loop, walked away, and came back
    /// to a restarted daemon could not tell *it finished and the record is gone* from *it never
    /// ran*. The counters it reached are kept, so what it managed before it died is still readable.
    ///
    /// ⚠ It is NOT resumable and does not pretend to be: the thread that was driving it died with
    /// its daemon, and nothing here re-enters a statechart from a summary.
    ///
    /// # ⚠⚠⚠⚠⚠ THIS ENTRY USED TO GIVE A REASON THAT IS FALSE, AND IT CITED ITS OWN REFUTATION
    ///
    /// It said *"The pane it drove came back as a plain shell **(see the restore allowlist)** and
    /// the agent that asked for it is gone with its process."* Measured 2026-08-18 on this
    /// repository's own state file: the allowlist it points at
    /// (`durability`'s default restore allowlist) **contains `claude`**, and
    /// [`crate::durability::restore_command`] appends `--resume <uuid>` from the pane's recorded
    /// conversation. A daemon restarted at 08:29 brought pane 91 back as
    /// `claude --resume 13cac637-…`, holding the same conversation — not a shell.
    ///
    /// ⚠⚠⚠ **So what makes a run unresumable is the DRIVER, not the peer.** That is a much smaller
    /// claim than the one this doc was making, and it is the one worth writing down: a run's
    /// statechart state was never persisted, so there is nothing to re-enter. The peer being gone
    /// was doing none of the work in that argument, and while it stood it also justified
    /// [`RunRegistry::restore`]'s two authority decisions — see the ⚠ paragraph there for what is
    /// now owed.
    Interrupted,
}

/// **WHAT A PERSON CAN SAY TO A RUN** — the three orders as MESSAGES, rather than as flags a caller
/// reaches in and sets.
///
/// # ⚠⚠⚠⚠⚠ Why this is a type and not three method names — register item 544
///
/// The registry's orders were three `Arc<AtomicBool>` stores, which is only expressible when the
/// thing being ordered is A THREAD IN THIS PROCESS. Item 544's direction is that a run's driver
/// stops living inside the terminal multiplexer, and the moment it does, *"set this bool"* has no
/// meaning — the order has to TRAVEL. Naming the orders makes the set of them closed and makes each
/// one a value that can be carried somewhere, which is the whole difference between a registry that
/// is a container of threads and one that is a DIRECTORY.
///
/// ⚠⚠⚠ THE THREE ARE DELIBERATELY NOT COLLAPSIBLE, and `RunRecord`'s own fields already argued it:
/// [`Cancel`](Self::Cancel) loses the turn in flight, [`StandDown`](Self::StandDown) banks the
/// milestone and then stops, and those are exactly the two outcomes a person raising one is choosing
/// between. [`Hold`](Self::Hold) is a third because it is the only TWO-WAY one — a level somebody
/// raises and lowers — where the other two are latches that must be, since an un-ordering racing a
/// milestone would make a run's ending depend on which message arrived first.
/// ⚠ NO LONGER `Copy` — register item 835. [`StandDown`](Self::StandDown) carries the conversation
/// this daemon read off the ordering pane, and a name is not a bit. Nothing here is on a hot path:
/// an order is a message a person sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunOrder {
    /// Stop now and lose the turn in flight — `RunRegistry::cancel`, carrying WHO said so.
    ///
    /// ⚠⚠ The word rides on the ORDER rather than being a second call, because the two arrive at
    /// the same flag and a caller that had to set a reason separately could set one and not the
    /// other. See [`Canceller`].
    Cancel(Canceller),
    /// Finish what you are doing and then stop — `RunRegistry::stand_down`. One-way.
    ///
    /// ⛔⛔⛔ **CARRYING WHERE THE ORDER CAME FROM** — register item 835, on the arm above's
    /// argument exactly: the two arrive at the same flag, so a caller that had to record the
    /// orderer separately could raise one and not the other. [`None`] is *nobody wrote it down*
    /// and never *a person*; see [`StoodDownBy`].
    StandDown(Option<StoodDownBy>),
    /// Halt between turns (`true`), or let go again (`false`) — `RunRegistry::hold`.
    Hold(bool),
}

impl RunOrder {
    /// **WHICH STANDING ORDER THIS IS**, or [`None`] for one the plugin never has to read.
    ///
    /// ⚠⚠⚠ Register items 539 and 597. A cancel is acted on by the DRIVER, so every run honours one
    /// and there is nobody to ask; the other two are carried into the plugin's own document and
    /// take effect at a moment only that document can name, so a plugin with no such moment cannot
    /// obey them at all. That difference is what this method exists to state once.
    ///
    /// ⚠ No `_` arm: a fourth [`RunOrder`] has to be classified here rather than defaulting into
    /// *nobody needs to read it*, which is the answer that made this defect invisible.
    #[must_use]
    pub const fn standing(&self) -> Option<sprag_plugin::StandingOrder> {
        match self {
            Self::Cancel(_) => None,
            Self::StandDown(_) => Some(sprag_plugin::StandingOrder::StandDown),
            Self::Hold(_) => Some(sprag_plugin::StandingOrder::Hold),
        }
    }
}

/// ⛔⛔⛔ **WHO STOPPED THIS RUN** — register item 596, and the fact a `cancelled` outcome could not
/// carry.
///
/// # The two cancels that were one word
///
/// [`RunRegistry::cancel`] is a PERSON saying stop. [`RunRegistry::cancel_all`] is the DAEMON
/// shutting down, raising every run's flag so nothing is waited out and detached. Both arrived at
/// one `AtomicBool`, so the driver raised one `OrchestrationEvent::Cancel` and every run reported
/// the same `cancelled` — and **the remedies are opposite**: a run the daemon stopped wants
/// *bring the daemon back and start it again*, and a run a person stopped wants *ask them why*.
///
/// ⚠⚠⚠⚠⚠ **IT IS WHY REGISTER ITEM 594's «WHY» COULD NOT BE SETTLED.** That round measured a run
/// reported `cancelled after 56 iterations` under a standing stand-down order and could not tell
/// whether a person had cancelled it or the promotion's `kill-server` had — the product does not
/// distinguish them, so no amount of reading could. This is that half.
///
/// ⚠⚠ **THE DECISION IS UNCHANGED.** Both still cancel, immediately, losing the turn in flight;
/// the flag and every wait that reads it are untouched. What changed is that the run can say which
/// happened — `sprag_plugin::judge::Unheard`'s shape one crate over, and register item 593's rule:
/// **the answer stays, the REPORT gains a reason.**
/// **WHY A STANDING ORDER WAS NOT DELIVERED** — register items 539 and 597.
///
/// ⚠⚠⚠ Each arm is a DIFFERENT thing for the caller to do, which is why it is a type and not a
/// `false`: a wrong id is retyped, an order no plugin of that kind reads is abandoned or the run is
/// cancelled instead, and the third arm cannot happen from either public door.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unordered {
    /// No run of that id is in this directory — the answer both doors already gave.
    NoSuchRun,
    /// **THE RUN EXISTS AND ITS PLUGIN HAS NO READER FOR THE ORDER**, so delivering it would change
    /// nothing while telling the caller it had. This is the whole of items 539 and 597.
    Unread {
        /// The plugin, as the TYPE the request named — never re-derived from a run's label, which
        /// is prose a reader composed and register item 587's finding.
        plugin: crate::plugins::PluginName,
        /// Which order went unread, so the sentence can name what was actually asked for.
        order: sprag_plugin::StandingOrder,
    },
    /// **NOTHING IS DRIVING THIS RUN** — it was restored from a daemon that is gone, so there is no
    /// worker to carry the order anywhere.
    ///
    /// ⚠⚠ The same lie as [`Unread`](Self::Unread) wearing a different cause, and it was answered
    /// `true` before this type existed: a person could stand down a run whose driver died with its
    /// daemon and be told it landed.
    NoDriver,
    /// A [`RunOrder`] that is not something a person raises over a running run — unreachable from
    /// either public door, and answered rather than panicked for the reason given at the call site.
    NotAStandingOrder,
}

impl Unordered {
    /// **WHAT HAPPENED AND WHAT TO DO ABOUT IT** — prose, and never the arm's own name, the rule
    /// every describing vocabulary in this workspace follows.
    #[must_use]
    pub fn describe(&self, id: RunId) -> String {
        match self {
            Self::NoSuchRun => format!("no run {} is in flight", id.0),
            // ⚠⚠⚠ IT NAMES THE PLUGIN, and that is the load-bearing half. *Refused* alone sends a
            // person to check whether they typed the wrong id; what they need is that THIS KIND OF
            // RUN has no reader for the order, which tells them to cancel it instead.
            Self::Unread { plugin, order } => {
                let plugin = plugin.wire_str();
                // ⚠ NO ARTICLE BEFORE THE PLUGIN'S NAME, and that is deliberate rather than terse:
                // `a`/`an` depends on the word, and a mutation of this round printed *"a
                // `ai_loop`"*. A sentence built by a machine should not have to know English
                // orthography, so the shape avoids needing to.
                format!(
                    "run {}'s plugin is `{plugin}`, and `{plugin}` cannot {} — that order is only \
                     read by a plugin built to act on it, and this one drives straight on. Nothing \
                     was changed. `sprag cancel-run {}` is the ending that works on any run.",
                    id.0,
                    order.describe(),
                    id.0,
                )
            }
            Self::NoDriver => format!(
                "run {} came back from a daemon that is gone, so nothing is driving it and there \
                 is nothing to order. Its record is here to be read, not steered.",
                id.0,
            ),
            Self::NotAStandingOrder => format!(
                "run {} was sent an order that is not one a person raises over a running run",
                id.0,
            ),
        }
    }
}

/// **WHAT A HOLD ORDER FOUND, AND WHAT IT LEFT** — register item 694, and the fact `resume-run`
/// answered about without ever asking for.
///
/// # ⛔⛔⛔⛔⛔ One sentence was printed over two different worlds
///
/// `resume-run` printed *"run N let go; it takes a fresh turn at its next pass"* unconditionally,
/// and [`RunOrder::Hold`] is the one order that is a LEVEL rather than a latch — so *the order was
/// delivered* and *the order changed something* are different facts about it, and that sentence
/// claimed the second while the door could only answer the first.
///
/// Measured 2026-08-25 in a sibling repository, twice: two runs nobody was holding — one standing
/// down, one waiting on a silent peer — were each told they had been let go, and neither moved a
/// step in the twenty-three minutes after. **The product was right and only the sentence was
/// false**: `resume` is a transition of `awaiting_human`, three OTHER doors lead into that state,
/// and `sprag_plugin`'s own gate drives one of them and asserts the loop stays put. The cost is
/// recorded with a name on it — the person watching believed the `rc=0` and reported to their own
/// user that a stand-down had been lifted, then corrected themselves.
///
/// # ⚠⚠⚠⚠ Four arms rather than a boolean
///
/// Register item 594's rule at a second door: a bare *did it move* hands every mouth the job of
/// pairing it back up with the direction that was asked for, and the two pairings that changed
/// nothing are exactly the two a person needs told apart from the two that did.
///
/// ⚠⚠ **IT IS A FACT ABOUT THE LEVEL AND NEVER ABOUT THE RUN'S BEHAVIOUR**, which is
/// [`RunHandle::held`]'s own warning one seam out. A run this order took the hold of parks at its
/// NEXT pass; it has not parked yet, and nothing here says it has.
///
/// ⚠ **AND IT CANNOT SAY *never*.** The level is the current value and no history is kept, so a run
/// held and let go an hour ago is [`NothingHeld`](Self::NothingHeld) today — which is what a person
/// resuming it needs to know, while *this run was never held* would be a claim nothing measured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Holding {
    /// It was running free and this order holds it.
    Took,
    /// It was already held and still is — a person said it twice.
    Already,
    /// A person was holding it and this order lets it go.
    LetGo,
    /// Nobody was holding it, so this order let nothing go.
    NothingHeld,
}

impl Holding {
    /// **EVERY ARM, BUILT FROM THE DOMAIN ITSELF** so the list cannot go stale.
    ///
    /// ⚠⚠⚠ The four `(before, after)` pairs ARE the four arms, so a fifth one would need a fifth
    /// pair of booleans — where a hand-written list is free to forget a variant, which is the
    /// defect `crate::plugins::outcome_word`'s own doc records paying for.
    pub const ALL: [Self; 4] = [
        Self::of(false, true),
        Self::of(true, true),
        Self::of(true, false),
        Self::of(false, false),
    ];

    /// Pair the level as the order FOUND it with the level it left.
    ///
    /// ⚠ No `_` arm, this workspace's rule for a classifier: an unclassified pairing must not fall
    /// through into whichever answer was written last.
    #[must_use]
    pub const fn of(before: bool, after: bool) -> Self {
        match (before, after) {
            (false, true) => Self::Took,
            (true, true) => Self::Already,
            (true, false) => Self::LetGo,
            (false, false) => Self::NothingHeld,
        }
    }

    /// The word this fact crosses a socket as.
    ///
    /// ⚠ There is deliberately no `moved()` beside this. *Did the level move* is the arithmetic
    /// this type exists to stop a mouth doing for itself, and a predicate nobody reads is register
    /// item 492's shape — an answer authored and never asked for — in the very feature whose
    /// defect was a fact that reached the wire and died at the mouth.
    #[must_use]
    pub const fn wire_str(self) -> &'static str {
        match self {
            Self::Took => "took",
            Self::Already => "already",
            Self::LetGo => "let_go",
            Self::NothingHeld => "nothing_held",
        }
    }

    /// Read it back, or [`None`] for a word this build has no arm for — which is what a daemon
    /// older than this answer sends, and a caller must be able to tell from an answer it knows.
    #[must_use]
    pub fn from_wire(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|it| it.wire_str() == word)
    }
}

/// **WHY A PROGRESS REPORT WAS NOT TAKEN** — register item 764, and [`Unordered`]'s shape one door
/// over.
///
/// # ⚠⚠⚠⚠⚠ The door answered *received* for a run it had decided nothing would ever drive
///
/// [`RunRegistry::report`] was a `find` and nothing else, while the door above it
/// (`crate::plugins::PluginsExternal::report_progress`) wrote this in its own doc: *"a run this
/// daemon does not hold is REFUSED rather than ignored: a driver reporting for an id nobody has is
/// a driver that has outlived its run, and telling it so is what lets it stop."* A successor daemon
/// **holds the id** of every run it inherited — including the ones register item 737 withheld and
/// the ones item 771 could not stand a driver up for — so at exactly the promotion that sentence
/// was written for, the answer was *received*.
///
/// # ⚠⚠⚠⚠ Where that costs something is the LOG, not the row
///
/// `crate::plugins::run_to_json`'s `interrupted` arm reads no report at all, so nothing a person
/// opens moves — measured, and it is why this is not filed as a lie on a screen.
/// [`RunRegistry::persistable`] is the reader that matters: it merges a record's report into the
/// durable log for a `Running` **and an `Interrupted`** run alike — `place`, the `driving` pane,
/// the counters — and stamps [`sprag_plugin::STATECHARTS_FINGERPRINT`] beside the words. So a
/// report taken for a set-aside run **rewrites where the next boot would put that run back**,
/// vouched for by a daemon that never drove it.
///
/// ⚠ Each arm is a different thing for the reporter to do, which is why this is a type and not a
/// `false`: an unknown id is a driver that has outlived its run, a set-aside one is a run somebody
/// has to start again, and an ended one already has its answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unreported {
    /// No run of that id is in this directory — the answer the `false` already gave.
    NoSuchRun,
    /// **NO SUCCESSOR WILL EVER PUT THIS RUN BACK**, in the words register item 737 recorded at the
    /// boot that read the log — the promotion case, and the one this item was filed about.
    Withheld(Withheld),
    /// **THIS BOOT TRIED AND COULD NOT**, in the words register item 771 recorded — a different
    /// fact with a different remedy, kept apart here for the reason those two items kept them apart
    /// on the row.
    NotResumed(NotResumed),
    /// **THE RUN'S ENDING IS ALREADY RECORDED HERE.** A driver still reporting into one is a driver
    /// whose own process was collected — its stdout read to EOF — so there is nothing left for it
    /// to say and nothing here that would keep it.
    Ended,
}

impl Unreported {
    /// The words every one of these clauses opens with, and **the ONE thing a driver matches on**.
    ///
    /// # ⚠⚠⚠⚠⚠ Why the sentence is the channel, and why that is not the two-mouths defect
    ///
    /// `pinion_core::external::InvokeError::rejected` carries a STRING and nothing else, so a
    /// refusal's reason reaches the far side of a socket as prose or not at all. What makes prose
    /// safe to match on is that both ends read one const: [`describe`](Self::describe) composes
    /// with it and [`spoken_in`](Self::spoken_in) recognises with it, they sit here together, and a
    /// gate drives the one into the other. `crate::wire::unknown_slot` is the same arrangement
    /// against a word pinion authored.
    ///
    /// ⚠ It names the RUN and not the reason, deliberately: the reason differs per arm and a reader
    /// that had to know all four would go stale the day a fifth is added, while *this daemon is not
    /// driving that run* is the whole of what a driver has to act on.
    pub const NOT_DRIVING: &'static str = "this daemon is not driving run";

    /// **WHAT HAPPENED AND WHAT TO DO ABOUT IT** — prose, and never the arm's own name, the rule
    /// every describing vocabulary in this workspace follows. [`Unordered::describe`]'s twin.
    ///
    /// ⚠⚠ The two set-aside arms carry the boot's OWN sentence rather than re-authoring it
    /// (`crate::plugins::withheld_sentence` and `crate::plugins::not_resumed_sentence`), which is
    /// [`NotResumed::Refused`]'s rule: the party that decided is the party that says why, and a
    /// second wording here would be free to drift from the one the row and the operator's log
    /// already carry.
    ///
    /// ⚠⚠⚠ **AND NO REMEDY IS APPENDED TO THEM**, which is the same rule and was measured rather
    /// than reasoned: a first draft added *"Start a new run"* to both, and against the arm a
    /// promotion actually causes the clause came out ending *"…Start it again. Start a new run"* —
    /// the carried sentence already says it. A wrapper that re-instructs a reader the wrapped
    /// sentence has already instructed is a second mouth wearing a helpful coat.
    #[must_use]
    pub fn describe(&self, id: RunId) -> String {
        let subject = format!("{} {}", Self::NOT_DRIVING, id.0);
        match self {
            Self::NoSuchRun => format!(
                "{subject}: it holds no such run. A driver reporting for an id nobody has has \
                 outlived its run and should stop"
            ),
            Self::Withheld(why) => {
                format!("{subject}: {}", crate::plugins::withheld_sentence(why))
            }
            Self::NotResumed(why) => {
                format!("{subject}: {}", crate::plugins::not_resumed_sentence(why))
            }
            Self::Ended => format!(
                "{subject}: its ending is already recorded here, so there is nothing left for a \
                 driver of it to report"
            ),
        }
    }

    /// **IS THIS REFUSAL CLAUSE ONE OF THESE?** — the far side of [`describe`](Self::describe), and
    /// the whole of what `crate::drive` needs in order to know it has been abandoned.
    ///
    /// ⚠ A PREFIX and not a `contains`: a clause that merely mentions the words — one refusal
    /// quoting another, a plugin's own prose — is not this daemon speaking about the reporter's run.
    #[must_use]
    pub fn spoken_in(clause: &str) -> bool {
        clause.starts_with(Self::NOT_DRIVING)
    }
}

/// ⚠⚠ `Serialize`/`Deserialize` because it is written into the durable run log, and
/// `rename_all = "snake_case"` so the log holds `"person"` and not `"Person"` — the shape every
/// other word in [`PersistedRun`] already takes. An arm added later must keep the old spellings
/// readable: a log written by yesterday's daemon is read by today's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Canceller {
    /// **A PERSON SAID STOP** — `sprag cancel-run`, or an agent's `cancel_run` on its own run.
    Person,
    /// **THE DAEMON IS SHUTTING DOWN** and raised every run's flag so none is waited out and
    /// detached — [`RunRegistry::cancel_all`].
    ///
    /// ⚠ Nobody decided anything about THIS run. That is the whole difference: the remedy is to
    /// bring the daemon back, and asking a person why they stopped it would be asking about a
    /// decision nobody took.
    Shutdown,
}

impl Canceller {
    /// **WHAT A READER OF THE RUN SHOULD DO ABOUT IT** — prose, and deliberately not the arm's own
    /// name, the rule every describing vocabulary in this workspace follows.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::Person => {
                "a person cancelled this run, so the turn it was in the middle of was thrown away \
                 — whoever asked for that is the one who knows why"
            }
            Self::Shutdown => {
                "the daemon this run was in shut down and stopped it on the way out, so NOBODY \
                 decided anything about this run — bring the daemon back and start it again"
            }
        }
    }

    /// **WHO RAISED IT, WITHOUT CLAIMING THE RUN IS OVER** — the phrase for every reader whose run
    /// did NOT end on this cancel.
    ///
    /// # ⚠⚠⚠⚠⚠ Why [`describe`](Self::describe) cannot be used there, measured rather than reasoned
    ///
    /// `describe` is written about a run the cancel FINISHED, so it contains the word **cancelled**
    /// — and this repository's own suites read that word as *the run is over*: two integration
    /// tests waited for it and were satisfied by a run that was **still running**, because a clause
    /// naming the canceller had put the ending word on a live run's line. A person scanning
    /// `sprag runs` for the same word would have been misled the same way.
    ///
    /// ⚠⚠ **THIS IS THE DEFECT REGISTER ITEM 596 EXISTS TO FIX, ONE TURN LATER**: a word that
    /// carries a conclusion has to appear only where the conclusion holds. So the ending word lives
    /// in exactly one arm of [`crate::plugins::cancel_sentence`], and every other arm says who
    /// raised the cancel in words that claim nothing about how the run finished.
    ///
    /// ⚠ It still names the REMEDY, because that is the half a reader acts on and it is true
    /// whatever the ending was.
    #[must_use]
    pub const fn raiser(self) -> &'static str {
        match self {
            Self::Person => "a person, and they are the one who knows why",
            Self::Shutdown => {
                "a daemon on its way out, so NOBODY decided anything about this run — bring the \
                 daemon back and start it again"
            }
        }
    }
}

/// ⛔⛔⛔⛔⛔ **WHERE A STAND-DOWN ORDER CAME FROM** — register item 835, and the fact a
/// `stood_down` ending could not carry.
///
/// # ⛔⛔⛔⛔ Two supervisors, one word, and a stopped run restarted twice
///
/// Measured 2026-09-02: this repository's watcher stood five runs down on the owner's instruction.
/// Another repository's watcher (`scxml-core-engine-e9`) saw one of them end and re-launched it —
/// **twice** — and said exactly why:
///
/// > I never saw the stand-down. What I saw was the run's closing line *"a person asked this run to
/// > stand down"*, **and I had no way to know who that person was.** I had not asked, so I
/// > suspected a false claim, and I was in the middle of asking the owner whether they had pressed
/// > something in the GUI.
///
/// ⇒ **A closing word that says *a person* and not *which* is read by the next supervisor as a
/// normal handover it should pick up.** In this system several watchers share one daemon, so *stand
/// everything down* was not one act: the population kept refilling and it took three passes.
///
/// # ⚠⚠⚠⚠⚠ It POINTS, and the daemon reads the name — never the caller's word for itself
///
/// [`Canceller`] beside it is a closed vocabulary because the two cancels are raised inside this
/// binary. A stand-down comes over the wire, so the honest shape is the one
/// `PluginGrammar::OPENED_BY` already uses and its doc already argues: **the caller sends a PANE,
/// and this daemon reads the conversation off that pane itself.** The worst a forged pane can do is
/// attribute an order to a real pane of this daemon — it can never invent a name.
///
/// ⚠⚠ **AND ABSENCE IS NOT *A PERSON*.** A caller that sent no pane is one nobody wrote down, which
/// is precisely the distinction `Canceller`'s own doc demands — *a reader must be able to tell
/// **nobody decided this** from **nobody wrote it down***. Answering *a person* for an unrecorded
/// order would be deducing, and the field's whole discipline is REPEATED, NEVER DEDUCED.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoodDownBy {
    /// The pane the order was sent from, as the caller pointed at it.
    pub pane: u64,
    /// **WHICH CONVERSATION WAS IN THAT PANE**, read by this daemon at the moment the order landed
    /// — [`PersistedRun::opened_by_session`]'s rule one order over: resolved while the pane is
    /// still there to answer, because that is what survives the pane.
    ///
    /// [`None`] when the pane held nothing agent-shaped, which is a person's own terminal and is a
    /// different fact from an order with no pane at all.
    pub session: Option<String>,
}

impl StoodDownBy {
    /// **WHO TO GO AND ASK**, in words that claim nothing about how the run finished.
    ///
    /// ⚠ [`Canceller::raiser`]'s rule verbatim, and for the reason that method was measured into
    /// existence: a clause naming the orderer must not contain a word this repository's own suites
    /// read as *the run is over*, because a stand-down is an ORDER and the run may still be
    /// working through the turn it was in.
    #[must_use]
    pub fn raiser(&self) -> String {
        match &self.session {
            Some(session) => format!("the conversation {session}, in pane {}", self.pane),
            None => format!(
                "somebody in pane {} that this daemon could put no conversation to",
                self.pane,
            ),
        }
    }

    /// **THE SUBJECT OF A STAND-DOWN SENTENCE**, for an order this daemon has no record of.
    ///
    /// ⛔⛔⛔⛔⛔ **IT IS NOT *A PERSON*, AND THAT IS THE WHOLE OF REGISTER ITEM 835.** The word
    /// *person* is what another supervisor read as *the owner, or somebody whose decision I can
    /// treat as a normal handover* — and it re-launched the run twice. An unrecorded order is one
    /// **nobody wrote down**, which is a different fact and the one [`Canceller`]'s own doc already
    /// demanded be tellable apart.
    ///
    /// ⚠ Spelled here rather than at the renderer so the two subjects — recorded and not — are
    /// composed in one place and cannot drift into two ideas of what an absence means.
    pub const UNRECORDED: &'static str =
        "somebody this daemon did not write down (an older client, or a caller that named no pane)";
}

/// **A RUN AS THE REGISTRY KNOWS IT** — the seam that lets [`RunRegistry`] be a DIRECTORY of runs
/// instead of a container of threads.
///
/// # ⚠⚠⚠⚠⚠ The fusion this exists to unpick — register item 544
///
/// A run is a SUPERVISOR: it holds a statechart and drives a pane, and its natural lifetime is the
/// work. The daemon is a terminal multiplexer: it owns PTYs and panes, and its natural lifetime is
/// weeks. They share one process today, and the consequence is a sentence nobody would design on
/// purpose — **changing how an AI loop reflects requires restarting the thing that holds your
/// PTYs.** Moving the driver out needs the registry to stop knowing HOW a run is driven, and this
/// trait is that boundary: everything the registry does to a run it does through these four
/// questions, none of which mentions a thread.
///
/// ⚠⚠⚠ **IT HAS TWO IMPLEMENTATIONS IN THIS FILE ALREADY, AND THAT IS THE POINT.** [`ThreadRun`] is
/// today's in-process worker; [`EndedRun`] is a run restored from a dead daemon's log, which has no
/// driver at all. The second one is not a test fixture — it is what `RunRegistry::restore` builds,
/// and it replaces three fresh `AtomicBool`s whose own doc admitted there was *"nothing on the other
/// end of them"*. A seam exercised only by tests is a seam nothing keeps honest.
///
/// ⚠⚠ REAPING IS PART OF IT because a directory that could deliver orders but still had to reach
/// for a `JoinHandle` would be a container of threads wearing a trait. `reapable` and `reap` are how
/// a run's driver is found to have stopped and collected, whatever kind of driver it was.
pub trait RunHandle: Send + Sync {
    /// Deliver `order` to this run. Delivery is best-effort and says nothing about when the run acts
    /// — the registry's callers are told only whether the run EXISTS, which is a fact about the
    /// directory rather than about the driver.
    fn deliver(&self, order: RunOrder);

    /// **WHETHER A PERSON HAS ASKED THIS RUN TO FINISH UP AND STAND DOWN** — register item 594.
    ///
    /// # ⚠⚠⚠⚠⚠ Why the ORDER is asked of the handle and not remembered by the directory
    ///
    /// [`deliver`](Self::deliver)'s doc says the registry's callers are told only whether a run
    /// EXISTS, *"which is a fact about the directory rather than about the driver"*. This is the
    /// other side of that sentence: the directory forwards an order and does not know what became
    /// of it, so a second copy kept here would be a fact about a delivery rather than about the run
    /// — and it would answer `true` for an [`EndedRun`], which accepts every order and delivers
    /// none.
    ///
    /// ⚠⚠ **IT IS NOT AN OUTCOME AND MUST NEVER BE READ AS ONE.** A standing order says a person
    /// spoke, not that the run obeyed: `stand-down` is honoured at the loop document's next
    /// milestone and a run cut short before one reaches it having banked nothing. What the two facts
    /// MEAN together is `crate::plugins::stand_down_sentence`'s to say, and it is the only reader
    /// allowed to put them side by side.
    fn stood_down(&self) -> bool;

    /// ⛔⛔⛔ **WHERE THE STAND-DOWN ORDER CAME FROM**, or [`None`] if nobody wrote it down —
    /// register item 835, and [`stood_down`](Self::stood_down)'s missing half.
    ///
    /// ⚠⚠ [`cancelled_by`](Self::cancelled_by)'s argument word for word: it is a FACT ABOUT THE
    /// ORDER and never about the ending, and the handle is what remembers because the directory
    /// forwards an order and does not know what became of it.
    ///
    /// ⚠ [`None`] means **nobody wrote it down**, which is not *a person* — see [`StoodDownBy`].
    fn stood_down_by(&self) -> Option<StoodDownBy>;

    /// **WHO CANCELLED THIS RUN**, or [`None`] if nobody has — register item 596.
    ///
    /// ⚠⚠ [`stood_down`](Self::stood_down)'s argument verbatim: the directory forwards an order and
    /// does not know what became of it, so the handle is what remembers. And like that one it is a
    /// FACT ABOUT THE ORDER and never about the ending — a run whose flag was raised at the same
    /// instant it converged still converged, and `crate::plugins::cancel_sentence` is the only
    /// reader allowed to weigh the two together.
    fn cancelled_by(&self) -> Option<Canceller>;

    /// **WHETHER A PERSON IS HOLDING THIS RUN RIGHT NOW** — register item 699.
    ///
    /// ⚠⚠⚠⚠⚠ **THE ORDER BESIDE IT HAD A READER WITH THE WRONG TYPE; THIS ONE HAD NO READER AT
    /// ALL.** `deliver` stored `RunOrder::Hold` and nothing ever loaded it, so `hold-run` said
    /// *"it parks at its next pass"* and parked nothing. Measured 2026-08-26 across four
    /// repositories: neither of the two orders a person can give a WORKING run had ever landed.
    ///
    /// ⚠⚠ [`stood_down`](Self::stood_down)'s two warnings hold here word for word — it is a fact
    /// about the ORDER and never about the ending, and an [`EndedRun`] answers `false` because
    /// nothing can be holding a run that is over.
    ///
    /// ⚠ Unlike its neighbours it is a LEVEL: `resume-run` delivers `Hold(false)`, so a reader
    /// wants the current value rather than *did this ever happen*.
    fn held(&self) -> bool;

    /// **DOES THIS RUN'S PLUGIN READ `order`?** — register items 539 and 597.
    ///
    /// ⚠⚠⚠ Forwarded from the plugin's own [`sprag_plugin::Plugin::honours`] rather than decided
    /// here, so the day a second plugin grows a reader its answer changes and NOTHING in this
    /// directory has to be remembered. A handle that answered from a table of plugin names would be
    /// the list this design exists to avoid.
    ///
    /// ⚠ `false` is the honest default for a run with no driver left: nothing can act on an order
    /// given to a run that is over, so refusing it is the truth rather than a limitation.
    fn honours(&self, order: sprag_plugin::StandingOrder) -> bool;

    /// Whether a driver has stopped and is waiting to be collected — non-blocking. `false` once
    /// [`reap`](Self::reap) has taken it, and `false` for a run that never had one.
    fn reapable(&self) -> bool;

    /// Collect the stopped driver, reporting why it died badly if it did. Called only when
    /// [`reapable`](Self::reapable) says there is one, and at most once.
    fn reap(&mut self) -> Option<String>;

    /// Whether a driver of this run is still uncollected — what a shutdown's bounded join waits on.
    fn outstanding(&self) -> bool;

    /// **THE PROCESS DRIVING THIS RUN, WHEN THE DRIVER IS ONE** — register item 526, and the field
    /// that makes [`RunRegistry`] a directory of PROCESSES rather than a directory that says it is.
    ///
    /// # ⚠⚠⚠⚠⚠ Why a successor daemon cannot do without it
    ///
    /// Since register item 544's default moved, a driver is a process of its own and does not die
    /// with the daemon that started it — item 544's stage 1 built that on purpose, so a promotion
    /// does not stop other people's work mid-step. But the boot ALSO reads the run log and puts an
    /// unfinished run back on a driver (item 543), and those two together were **measured on
    /// 2026-08-25 to leave two processes typing at one agent**: five `sprag-term` against one
    /// socket where three was right. No ROW can show that — a row deliberately cannot say which
    /// kind of driver filled it in — so without this the successor has no way to ask.
    ///
    /// ⚠⚠ [`None`] for a run driven on a thread, for a run that is over, and for a log written
    /// before this field existed. All three mean *there is no process to ask about*, and a boot
    /// reads them the same way.
    fn driver_pid(&self) -> Option<u32> {
        None
    }
}

/// **WHAT A PERSON HAS SAID TO ONE RUN** — the daemon's record of it, and the half of a
/// [`RunHandle`] that is the same whatever kind of driver the run has.
///
/// # ⚠⚠⚠⚠⚠ Why this is a type and not a second copy of four fields
///
/// Every implementation of [`RunHandle`] with a live driver answers `deliver` / `cancelled_by` /
/// `stood_down` / `honours` **identically** — an order is written down here, and how the driver
/// LEARNS of it is the part that differs. [`ThreadRun`] shares the flags with a worker in this
/// process; [`ProcessRun`] publishes them in the run's row and the driver is woken to re-read it
/// (`Event::RunOrdered`, register item 648). Neither of those differences reaches this record.
///
/// Written twice, the two `deliver`s would be free to drift — and `deliver`'s body carries the
/// write ORDER that register item 596 paid for (who first, flag second). A rule that has to be
/// remembered at two sites is the shape this repository keeps finding defects in.
///
/// ⚠⚠ **THE THREE FLAGS ARE HANDED IN, NEVER MADE HERE.** For a thread-driven run the same `Arc`s
/// go to the worker's `RunContext`, so an order delivered here is seen at its next loop top or wait
/// poll. For a process-driven run nothing in this image reads them — they are pure record, and the
/// row is what the driver reads. Same type, and the difference is stated rather than hidden.
///
/// # ⛔⛔⛔⛔⛔ Why the ANNOUNCE is in here too — register item 664
///
/// It was not. The three doors of `crate::plugins::PluginsExternal` each called an `ordered` hook
/// on their own accepted arm, which is three sites remembering one rule — and the daemon's SHUTDOWN
/// SWEEP is a fourth caller that never went through them: it asks the registry directly
/// ([`RunRegistry::cancel_all`]), so **nothing was published, and a driver in another process was
/// never woken to re-read the row its order had just been written into.** Measured: such a daemon
/// takes [`RunRegistry::JOIN_DEADLINE`] to answer a signal — 5.03 s — because its collector thread
/// waits out a child nobody told.
///
/// So the announcement moved to the one place every order already passes through. *An order that
/// was accepted is announced* is now true by construction rather than by four callers each
/// remembering, and an order that was NOT accepted still announces nothing:
/// [`EndedRun`] holds no `Orders` at all, so a cancel aimed at a run with no driver left changes
/// nothing and says nothing — which is this journal's own rule about never waking a reader to
/// re-read a row that did not move.
pub struct Orders {
    cancel: Arc<AtomicBool>,
    stand_down: Arc<AtomicBool>,
    hold: Arc<AtomicBool>,
    /// ⚠⚠⚠⚠⚠ **WHO RAISED THE CANCEL FLAG** — register item 596, and it is HERE rather than shared
    /// with the worker on purpose.
    ///
    /// The three flags above are the worker's business: it reads them to decide what to do. This is
    /// nobody's business but a READER's — the driver does the same thing either way, and handing it
    /// down would invite a decision to be taken on it. So it stays on this side of the seam, where
    /// the run's ANSWER is assembled.
    ///
    /// ⚠ `Mutex` and not an atomic, because the value is an enum rather than a bit and because
    /// nothing reads it on a hot path: it is asked once, when a run's answer is projected.
    cancelled_by: Mutex<Option<Canceller>>,
    /// ⛔⛔⛔⛔⛔ **WHERE THE STAND-DOWN ORDER CAME FROM** — register item 835, and here rather than
    /// shared with the worker for [`cancelled_by`](Self::cancelled_by)'s reason exactly: the driver
    /// stands down the same way whoever asked, and handing this down would invite a decision to be
    /// taken on it. It is a READER's fact, so it lives where the run's answer is assembled.
    stood_down_by: Mutex<Option<StoodDownBy>>,
    /// **WHICH STANDING ORDERS THIS RUN'S PLUGIN ANSWERED THAT IT READS** — register items 539
    /// and 597, captured at submit because the plugin itself moves into the worker thread and is
    /// unreachable from here afterwards.
    ///
    /// ⚠⚠⚠ A LIST THE PLUGIN PRODUCED, not one anybody here composed: the caller walks
    /// [`sprag_plugin::StandingOrder::ALL`] and keeps what
    /// [`sprag_plugin::Plugin::honours`] said yes to, so an order added to that set is asked about
    /// with nothing here to update.
    honoured: Vec<sprag_plugin::StandingOrder>,
    /// **WHICH RUN THIS IS**, so [`deliver`](Self::deliver) can name it to the announcer below.
    ///
    /// ⚠ Carried rather than passed in at `deliver`: [`RunHandle::deliver`] takes an order and
    /// nothing else, and widening that signature so every caller could re-state a fact the record
    /// already knows is how two answers to *which run is this* get created.
    id: RunId,
    /// **WHERE THE NEWS OF AN ORDER GOES** — `crate::run_announcers`' second half, or [`None`] for a
    /// run with nowhere to announce (a registry off a daemon, and every fixture in this file).
    ///
    /// ⚠⚠⚠ **AN OPAQUE `Fn`, exactly as `on_run_end` is.** A journal is per SESSION and a run is not
    /// in one — the registry is the daemon's, the pane pool is a window's — so what crosses this
    /// boundary is a call with a run id in it, and which channel that reaches is the caller's
    /// business. That is the discipline every hook on the plugin surface follows and it is what
    /// lets this directory stay session-tree-free while still publishing.
    announce: Option<crate::RunAnnounce>,
}

impl Orders {
    /// Take the three flags a driver is already sharing (or, for an out-of-process one, the three
    /// this daemon will publish), the plugin's own list of what it reads, and where an order to
    /// this run is announced.
    #[must_use]
    pub fn new(
        cancel: Arc<AtomicBool>,
        stand_down: Arc<AtomicBool>,
        hold: Arc<AtomicBool>,
        honoured: Vec<sprag_plugin::StandingOrder>,
        id: RunId,
        announce: Option<crate::RunAnnounce>,
    ) -> Self {
        Self {
            cancel,
            stand_down,
            hold,
            cancelled_by: Mutex::new(None),
            stood_down_by: Mutex::new(None),
            honoured,
            id,
            announce,
        }
    }

    /// Write `order` down and say so — [`RunHandle::deliver`]'s whole body, for every driver kind.
    fn deliver(&self, order: RunOrder) {
        // ⚠ ONE `match`, so a fourth order cannot be added without this arm being written. That is
        // the ratchet a trio of `store` calls at three call sites did not have.
        match order {
            RunOrder::Cancel(who) => {
                // ⚠⚠⚠ WHO FIRST, FLAG SECOND, and the order matters — register item 596. The
                // worker can observe the flag on its very next poll, so a reason written afterwards
                // could be read as absent by a projection racing the run's own ending. Written
                // first, a reader that sees the cancel has already been able to see the reason.
                //
                // ⚠ THE FIRST WORD WINS: a person's cancel that a shutdown then repeats is still a
                // person's decision, and `cancel_all` sweeps every run on the way out — including
                // ones somebody had already stopped.
                let mut said = lock(&self.cancelled_by);
                said.get_or_insert(who);
                drop(said);
                self.cancel.store(true, Ordering::Release);
            }
            RunOrder::StandDown(who) => {
                // ⚠⚠⚠ WHO FIRST, FLAG SECOND — the arm above's rule and its reason, one order
                // over: the worker can observe the flag on its very next poll, so an orderer
                // written afterwards could be read as absent by a projection racing the run's own
                // ending. That race is not hypothetical here — register item 835 is a run whose
                // ending was read by another supervisor the moment it appeared.
                //
                // ⚠ THE FIRST WORD WINS, for `Cancel`'s reason: a stand-down is idempotent and
                // one-way (`RunRegistry::stand_down`), so a second order changes nothing — and the
                // ORDERER a reader needs is the one whose decision this was, not whoever repeated
                // it. ⚠⚠ An order that names NOBODY must not erase a name already written, which
                // `get_or_insert` gives for free and an assignment would have taken away.
                let mut said = lock(&self.stood_down_by);
                if let Some(who) = who {
                    said.get_or_insert(who);
                }
                drop(said);
                self.stand_down.store(true, Ordering::Release);
            }
            RunOrder::Hold(held) => self.hold.store(held, Ordering::Release),
        }
        // ⚠⚠⚠ AFTER THE RECORD IS WRITTEN, NEVER BEFORE — the rule a run's ENDING already follows
        // one seam over: a reader woken by this asks for the row immediately, and an announcement
        // that raced the write would answer about a run that had not moved yet, leaving the reader
        // parked on an event that has already fired. Here it is sharper still, because the reader
        // being woken may be the run's own DRIVER, and what it comes back to read is this order.
        //
        // ⚠⚠ A REPEATED ORDER ANNOUNCES TOO. The event says *a person spoke*, not *the level
        // moved*, so re-asserting a hold is a thing that happened; suppressing it would need this
        // to compare levels, and a `hold(false)` that changed nothing is still an answer somebody
        // is waiting for.
        if let Some(announce) = &self.announce {
            announce(self.id);
        }
    }

    fn cancelled_by(&self) -> Option<Canceller> {
        *lock(&self.cancelled_by)
    }

    fn stood_down_by(&self) -> Option<StoodDownBy> {
        lock(&self.stood_down_by).clone()
    }

    /// ⚠ THE PLUGIN'S OWN ANSWER, replayed. Nothing here decides it: the list was taken from
    /// `sprag_plugin::Plugin::honours` at submit, before the plugin left this image.
    fn honours(&self, order: sprag_plugin::StandingOrder) -> bool {
        self.honoured.contains(&order)
    }

    fn stood_down(&self) -> bool {
        // ⚠ THE SAME FLAG THE WORKER'S `RunContext` IS SHARING, read rather than copied — see this
        // type's own note on why the three `Arc`s are handed in. A second bool set beside the
        // `store` above could disagree with what the driver is reading.
        self.stand_down.load(Ordering::Acquire)
    }

    /// **WHETHER A PERSON IS HOLDING THIS RUN RIGHT NOW** — register item 699.
    ///
    /// ⚠⚠⚠⚠⚠ **THIS READER DID NOT EXIST, AND THE FLAG BESIDE IT HAD NO OTHER.** `deliver` has
    /// stored `RunOrder::Hold` into `self.hold` since the order was built, and NOTHING in this
    /// process or any other ever loaded it: no method here, none on [`RunHandle`], no field on
    /// [`RunSummary`], nothing in the row. `hold-run` was write-only from end to end — it answered
    /// *"it parks at its next pass"* and parked nothing.
    ///
    /// ⚠⚠ **A LEVEL, NOT A LATCH**, which is what makes it different from its two neighbours and
    /// is why it is read rather than remembered: `hold` is the one order a person can take back
    /// (`resume-run` delivers `Hold(false)`), so what a reader wants is the CURRENT value and never
    /// *did this ever happen*.
    fn held(&self) -> bool {
        self.hold.load(Ordering::Acquire)
    }
}

/// A run driven by **A THREAD IN THIS PROCESS** — the kind that was the only one, and the one
/// register item 544 is about moving out. Its out-of-process sibling is [`ProcessRun`].
pub struct ThreadRun {
    /// What a person said to it — see [`Orders`], and note that the flags in there ARE the
    /// worker's, shared with its `RunContext`.
    orders: Orders,
    handle: Option<JoinHandle<()>>,
}

impl ThreadRun {
    /// Take the worker and the [`Orders`] it shares with it.
    ///
    /// ⚠ The record is BUILT BY THE CALLER rather than assembled from its parts here, and that is
    /// register item 664's arity showing: `Orders` carries six things now, and a constructor taking
    /// all of them plus a join handle would be seven positional arguments — three of which are
    /// `Arc<AtomicBool>` and freely transposable. One named type at the call site says which flag
    /// is which exactly once.
    #[must_use]
    pub fn new(orders: Orders, handle: JoinHandle<()>) -> Self {
        Self {
            orders,
            handle: Some(handle),
        }
    }
}

impl RunHandle for ThreadRun {
    fn deliver(&self, order: RunOrder) {
        self.orders.deliver(order);
    }

    fn cancelled_by(&self) -> Option<Canceller> {
        self.orders.cancelled_by()
    }

    fn honours(&self, order: sprag_plugin::StandingOrder) -> bool {
        self.orders.honours(order)
    }

    fn stood_down(&self) -> bool {
        self.orders.stood_down()
    }

    fn stood_down_by(&self) -> Option<StoodDownBy> {
        self.orders.stood_down_by()
    }

    fn held(&self) -> bool {
        self.orders.held()
    }

    fn reapable(&self) -> bool {
        self.handle.as_ref().is_some_and(JoinHandle::is_finished)
    }

    fn reap(&mut self) -> Option<String> {
        let handle = self.handle.take()?;
        handle
            .join()
            .err()
            .map(|_| "plugin run panicked".to_string())
    }

    fn outstanding(&self) -> bool {
        self.handle.is_some()
    }
}

/// A run driven by **ANOTHER PROCESS** — register item 544's whole point, and the third
/// [`RunHandle`].
///
/// # ⚠⚠⚠⚠⚠ How an order reaches a driver that shares no memory with this daemon
///
/// [`deliver`](RunHandle::deliver) writes the order into [`Orders`] exactly as [`ThreadRun`] does,
/// and the run's ROW publishes it (`stood_down`, `cancelled_by`). The driver is then WOKEN to
/// re-read that row by `Event::RunOrdered` — register item 648, built for this — which [`Orders`]
/// raises as the second half of the same delivery.
///
/// ⚠⚠ So the three flags here are **pure record**: nothing in this image reads them. That is the
/// one difference from [`ThreadRun`], where the same `Arc`s are the worker's own, and [`Orders`]
/// says so rather than leaving it to be inferred.
///
/// ⛔⛔⛔ **AND THE WAKE USED TO BE SOMEBODY ELSE'S TO REMEMBER** — register item 664. It was raised
/// by the three doors of `crate::plugins::PluginsExternal`, so a caller that reached the registry
/// directly published nothing and this driver never heard: the daemon's own shutdown sweep is
/// exactly such a caller, and it cost [`RunRegistry::JOIN_DEADLINE`] on every signalled daemon
/// holding a driven run. Delivering and announcing are one act now, inside [`Orders`]'s own
/// `deliver`.
///
/// # ⚠⚠⚠ The collector thread is not a driver
///
/// A thread per run in this daemon looks like it gives back what moving out was meant to win. It
/// does not: what item 544 is about is the PLUGIN LOGIC — an AI loop's turn model, a sentinel
/// rule — living somewhere it can be replaced without restarting the thing that holds the PTYs.
/// This thread holds none of that. It blocks on the child, and when the child ends it writes the
/// outcome and announces, which is byte-for-byte what a thread-driven run's worker does at ITS end.
///
/// ⚠ And it blocks rather than samples. `wait` IS the wake — no clock, which is what items
/// 629/630/631/640 spent four rounds establishing on the pane axis and what this must not undo on
/// the run axis.
pub struct ProcessRun {
    /// What a person said to it, published in the row for a reader that is not in this process.
    orders: Orders,
    /// The thread collecting the driver — see this type's note on why it is not a driver.
    handle: Option<JoinHandle<()>>,
    /// **WHERE THE DRIVER IS** — see [`RunHandle::driver_pid`], which is where the reason lives.
    pid: u32,
}

impl ProcessRun {
    /// Take the [`Orders`] this daemon will publish, the thread collecting the driver, and the pid
    /// of the driver itself.
    ///
    /// ⚠ [`ThreadRun::new`]'s argument for taking the record whole rather than its parts, and it
    /// bites harder here: the announcer inside it is the ONLY way an order reaches this run's
    /// driver at all.
    ///
    /// ⚠⚠ The PID is taken beside the collector rather than read off it, because a `JoinHandle`
    /// knows nothing about the child it is waiting on — the only moment both facts are in one hand
    /// is the spawn, and this constructor is the shape that says so.
    #[must_use]
    pub fn new(orders: Orders, handle: JoinHandle<()>, pid: u32) -> Self {
        Self {
            orders,
            handle: Some(handle),
            pid,
        }
    }
}

impl RunHandle for ProcessRun {
    fn deliver(&self, order: RunOrder) {
        self.orders.deliver(order);
    }

    fn cancelled_by(&self) -> Option<Canceller> {
        self.orders.cancelled_by()
    }

    fn honours(&self, order: sprag_plugin::StandingOrder) -> bool {
        self.orders.honours(order)
    }

    fn stood_down(&self) -> bool {
        self.orders.stood_down()
    }

    fn stood_down_by(&self) -> Option<StoodDownBy> {
        self.orders.stood_down_by()
    }

    fn held(&self) -> bool {
        self.orders.held()
    }

    fn reapable(&self) -> bool {
        self.handle.as_ref().is_some_and(JoinHandle::is_finished)
    }

    /// ⚠ THE COLLECTOR is what is joined, not the child — it has already reaped the child and
    /// written the outcome by the time it finishes. A panic here is this daemon's own failure to
    /// collect, which is a different sentence from a driver that died badly: that one reaches the
    /// row as the run's OUTCOME, where a reader is looking for it.
    fn reap(&mut self) -> Option<String> {
        let handle = self.handle.take()?;
        handle
            .join()
            .err()
            .map(|_| "collecting a run's driver process panicked".to_string())
    }

    fn outstanding(&self) -> bool {
        self.handle.is_some()
    }

    fn driver_pid(&self) -> Option<u32> {
        Some(self.pid)
    }
}

/// A run **WITH NO DRIVER LEFT** — what a predecessor daemon's log restores to.
///
/// # ⚠⚠⚠ It replaces three flags whose own doc said nothing read them
///
/// `RunRegistry::restore` used to mint a fresh `AtomicBool` for each order, and each carried a
/// comment explaining that setting it did nothing: *"the worker that would have read it died with
/// its daemon"*. Three write-only flags are a lie the type system was not being asked to catch, and
/// the registry's `cancel` had to document that it *"finds it and returns true having done
/// nothing"*. This says the same thing as a TYPE: the run is in the directory, orders to it are
/// accepted and go nowhere, and there is no driver to reap.
///
/// ⚠ The registry still answers `true` for it, which is unchanged and correct — the caller asked
/// whether the run exists, and it does.
pub struct EndedRun {
    /// **WHETHER A PERSON HAD STOOD THIS RUN DOWN BEFORE ITS DAEMON DIED** — register item 594, and
    /// the one thing in here that is a MEMORY rather than a capability.
    ///
    /// # ⚠⚠⚠⚠⚠ Why an order survives here when [`RunRegistry::restore`] refuses to resurrect one
    ///
    /// That function's own note says persisting an order *"would let a restart resurrect an
    /// instruction nobody could act on"*, and it is right about `hold` and `cancel`: a hold is a
    /// level somebody is CURRENTLY holding, and nothing can be holding a run that is not moving.
    ///
    /// A stand-down on a run that is over is not an instruction any more. It is the only thing that
    /// explains the ENDING — *a person said stop* — and dropping it made a restart erase the
    /// question a reader is actually asking: **did my order land, or did the work go?** So it comes
    /// back as a fact and is read as one. [`deliver`](RunHandle::deliver) here still accepts every
    /// order and delivers none, so nothing can be written into this after the fact and nobody can
    /// mistake the memory for a live order.
    stood_down: bool,
    /// WHO RAISED THE CANCEL, as the log recorded it — register item 596, and [`stood_down`]'s
    /// argument reaching one field further.
    ///
    /// [`stood_down`]: Self::stood_down
    ///
    /// ⚠⚠⚠ **REPEATED, NEVER DEDUCED.** This daemon did not end the run and cannot tell why one it
    /// found on disk stopped; what it may do is carry forward what the daemon that DID end it wrote
    /// down. The distinction matters because the most common recorded reason is
    /// [`Canceller::Shutdown`] — a daemon sweeping runs on its way out — and a reader must be able
    /// to tell *nobody decided this* from *nobody wrote it down*.
    cancelled_by: Option<Canceller>,
    /// ⛔⛔⛔ **WHERE THE STAND-DOWN ORDER CAME FROM**, as the log recorded it — register item 835,
    /// and the field above's *REPEATED, NEVER DEDUCED* applied to the order this type was already
    /// remembering half of.
    ///
    /// ⚠⚠ It is the pair with [`stood_down`](Self::stood_down) and never a substitute for it: the
    /// flag says an order was given, and this says who by. A restored run whose flag is `true` and
    /// whose orderer is [`None`] is one the recording daemon did not write down — which is the
    /// state register item 835 was filed on and must stay distinguishable from *nobody ordered it*.
    stood_down_by: Option<StoodDownBy>,
    /// **THE PROCESS THE DEAD DAEMON HAD DRIVING IT**, as the log recorded it — register item 526,
    /// and the one field in here that may describe something still ALIVE.
    ///
    /// ⚠⚠⚠ That is not a contradiction with this type's name. *No driver LEFT* is a statement about
    /// what THIS daemon holds: it inherited a row and holds no channel to anything. A driver
    /// process outliving the daemon that spawned it is exactly what register item 544's stage 1
    /// built — and it is why a successor has to be able to ask, because that process is still
    /// typing at somebody's agent and its ending can no longer be read by anyone (its outcome
    /// travels on the pipe of a parent that is gone).
    driver: Option<u32>,
}

impl EndedRun {
    /// A run with no driver left, carrying what the log said became of it.
    ///
    /// ⚠ Named rather than a struct literal at the call site: `EndedRun { stood_down: false }`
    /// reads as a decision somebody took, and these are the values a restore may not guess at.
    #[must_use]
    pub const fn restored(
        stood_down: bool,
        cancelled_by: Option<Canceller>,
        driver: Option<u32>,
    ) -> Self {
        Self {
            stood_down,
            cancelled_by,
            stood_down_by: None,
            driver,
        }
    }

    /// **AND WHERE THAT STAND-DOWN CAME FROM** — register item 835.
    ///
    /// ⚠ A separate builder rather than a fifth parameter, and that is a decision rather than
    /// convenience: `restored` is called from fixtures all over this workspace that have no orderer
    /// to give, and widening it would put `None` at every one of them — the shape where a caller
    /// who HAD the fact forgets to pass it reads exactly like a caller who never had one. Here the
    /// only callers are the two that actually read a log.
    #[must_use]
    pub fn ordered_by(mut self, who: Option<StoodDownBy>) -> Self {
        self.stood_down_by = who;
        self
    }
}

impl RunHandle for EndedRun {
    fn deliver(&self, _order: RunOrder) {}

    fn cancelled_by(&self) -> Option<Canceller> {
        self.cancelled_by
    }

    /// ⚠⚠ **NEVER, AND THAT IS THE FACT RATHER THAN A LIMITATION** — register items 539 and 597.
    /// This run's driver died with its daemon, so no order given now reaches anything at all. It
    /// used to be told `true`: a person could stand down a run that had been over for hours and be
    /// answered as though it had landed.
    fn honours(&self, _order: sprag_plugin::StandingOrder) -> bool {
        false
    }

    fn stood_down(&self) -> bool {
        self.stood_down
    }

    /// ⛔⛔⛔ **RESTORED, for the flag above's reason exactly** — register item 835. A run read out
    /// of a dead daemon's log is precisely the run another supervisor meets: it is over, its line
    /// says a person asked it to stand down, and *who* is the whole of what item 835 measured being
    /// missing. Answering [`None`] here would put every restored run back in the state that had one
    /// stopped run re-launched twice.
    fn stood_down_by(&self) -> Option<StoodDownBy> {
        self.stood_down_by.clone()
    }

    /// ⚠ ALWAYS `false`, and it is not the same shape as the order above it. A stand-down is
    /// remembered here because a run that ENDED can still have been asked to finish up, and a
    /// reader wants to weigh the two. A hold is a LEVEL on a run somebody might still let go, so
    /// there is nothing to remember about one that is over: nobody is holding this.
    fn held(&self) -> bool {
        false
    }

    fn reapable(&self) -> bool {
        false
    }

    fn reap(&mut self) -> Option<String> {
        None
    }

    fn outstanding(&self) -> bool {
        false
    }

    fn driver_pid(&self) -> Option<u32> {
        self.driver
    }
}

/// ⛔⛔⛔⛔⛔ **WHICH RUN THIS IS, WHEN THE NUMBER CANNOT SAY** — register item 887.
///
/// # ⛔⛔⛔⛔⛔ `RunId` is an ADDRESS, and this repository had it written down as a name
///
/// [`RunRegistry::reserve`]'s own doc said *"ids are monotonic and never reused"*, and on
/// 2026-09-04 that sentence was measured false in this daemon's own state. `next_id` is raised by
/// [`restore`](RunRegistry::restore) to `max(saved.id) + 1`, so a successor that restores a log
/// **missing some rows** starts issuing numbers a predecessor already spent. Measured, three of
/// them at once:
///
/// | | the persisted row | `/run/user/1000/loop/run<N>.log` |
/// | --- | --- | --- |
/// | 199 | 3 iterations, `cancelled`, ended 09:01:10 | starts `04:29:51`, 19,073 B, last written 08:38 |
/// | 200 | 3 iterations, `cancelled`, ended 08:58:09 | starts `04:53:27`, 4,858 B, last written 08:43 |
/// | 202 | 3 iterations, `cancelled`, ended 08:59:14 | starts `08:31:33`, 259 B |
///
/// Each log was finished BEFORE the row that now bears its number began, and the runs the ledger
/// had measured under 199 and 200 (`made: 24` and `made: 9`) have no row left at all.
///
/// # ⚠⚠⚠⚠⚠ Why the pair `(build, id)` is not the answer, measured rather than argued
///
/// The obvious cheap fix is to qualify the number with something the row already carries. All three
/// reused rows above are build `cb991990bcbf` — **the same build**, from one daemon, and the
/// predecessor that wrote two of those logs was `a7eaa889b195`. So `(build, id)` separates the log
/// from the row in two of the three cases and NOT in the third, and a discriminator that works
/// sometimes is worse than none: it reads as a check.
///
/// ⇒ So a value is MINTED, and its uniqueness is by construction rather than by hope: the process
/// id (two daemons alive at once), the nanosecond the registry was made (one pid reused after the
/// first process exited), a process-wide counter (two registries inside one process — every test
/// file makes several), and the run's own number, which never repeats inside one registry.
///
/// ⚠ The minting type is described rather than LINKED: it is crate-private and this one is public,
/// so an intra-doc link to it is `private_intra_doc_links` under `-D warnings` — item 365, met
/// again. ⚠⚠ **The residue, stated rather than hidden**: a clock that went BACKWARDS across a
/// reboot, onto a reused pid, at the same nanosecond, would collide and nothing here would detect
/// it. What is claimed is *this does not repeat on a machine whose clock does not go backwards*,
/// which is strictly stronger than the number's claim — and the number's was false in ordinary
/// operation rather than in a corner.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct WhichRun(String);

impl WhichRun {
    /// The stamp as a program compares it — the whole of it, never a prefix.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// A stamp read back out of a row, a log or a durable record.
    ///
    /// ⚠ It is not parsed and not validated, deliberately: this build is not the authority on what
    /// a stamp a DIFFERENT build minted looks like, and a reader that refused an unfamiliar shape
    /// would answer *not the same run* about a run it simply could not read. The one thing done
    /// with a stamp is comparing it with another for equality.
    #[must_use]
    pub fn said(stamp: impl Into<String>) -> Self {
        Self(stamp.into())
    }
}

impl std::fmt::Display for WhichRun {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str(&self.0)
    }
}

/// ⛔⛔⛔⛔⛔ **WHAT ONE REGISTRY STAMPS ITS RUNS WITH** — register item 887, and the part of a
/// [`WhichRun`] that is not the run's number.
///
/// # ⚠⚠⚠ The three parts, and what each one rules out
///
/// | part | what it separates |
/// | --- | --- |
/// | the process id | two daemons alive at once on this machine |
/// | the instant this registry was made, in nanoseconds | one pid reused after the first process exited |
/// | a counter, process-wide | two registries made inside one process in the same nanosecond (every test file does this) |
///
/// The run's own number completes it, and within ONE registry that number never repeats:
/// `next_id` only ever increases, and [`RunRegistry::restore`] only raises it. **The reuse this
/// exists for happens BETWEEN registries**, which is exactly what the three parts above tell apart.
///
/// # ⚠⚠ The residue, stated rather than hidden
///
/// A pid can be reused after its process exits, so the nanosecond is what separates those two
/// registries — and a clock that went BACKWARDS across a reboot onto the same pid at the same
/// nanosecond would collide. That is not impossible and nothing here detects it. What is claimed is
/// *this value does not repeat by construction on a machine whose clock does not go backwards*,
/// which is a strictly stronger claim than the number's, and the number's claim was false in
/// ordinary operation rather than in a corner.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Minting(String);

impl Default for Minting {
    fn default() -> Self {
        /// Two registries in one process, in the same nanosecond. Every test file makes several.
        static MADE: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        Self(format!(
            "{:x}-{nanos:x}-{:x}",
            std::process::id(),
            MADE.fetch_add(1, Ordering::Relaxed),
        ))
    }
}

impl Minting {
    /// The stamp this registry gives the run numbered `id`.
    fn stamping(&self, id: RunId) -> WhichRun {
        WhichRun(format!("{}.{:x}", self.0, id.0))
    }
}

struct RunRecord {
    id: RunId,
    label: String,
    /// **WHICH PLUGIN THIS RUN IS**, as the request named it — register items 539 and 597.
    ///
    /// ⚠⚠⚠ A TYPE and not a word cut out of [`label`](Self::label). That label is prose composed
    /// for a reader (`"orchestrator pane=3"`), and register item 587's finding is that identity
    /// re-derived from prose is identity that drifts the day somebody rewords the prose. This is
    /// carried from the one place that decided it.
    ///
    /// ⚠ [`None`] for a run RESTORED from disk: the log records the run, not the request, so a
    /// successor daemon does not know and does not guess. Such a run has no driver either, so the
    /// order would be refused regardless — see [`RunRegistry::orderable`].
    plugin: Option<crate::plugins::PluginName>,
    /// **THE REQUEST THIS RUN WAS ASKED WITH**, or [`None`] for a run nothing could rebuild —
    /// register item 543's sixth brick. See [`PersistedRun::request`], which is where it goes.
    ///
    /// ⚠⚠ **KEPT SO A SUCCESSOR CAN BUILD THE SAME PLUGIN**, and for nothing else in this process:
    /// the plugin it describes is already built and already running here. It is the one thing a
    /// restart cannot re-derive — [`plugin`](Self::plugin) is a word, and a word does not carry a
    /// brief, a pane or a set of guardrails.
    ///
    /// ⚠ [`None`] on a run RESTORED from a log that carried no request, and on one whose place this
    /// build cannot read: `PersistedRun::resumable_request` is the only door it comes back through,
    /// so a record that could not be acted on does not hold a person's prose for a second daemon's
    /// lifetime either.
    request: Option<serde_json::Map<String, serde_json::Value>>,
    /// WHO ASKED for this run — the pane whose occupant wanted it, or [`None`] for a run nobody
    /// claims (what a person starting one from a shell is).
    ///
    /// [`sprag_terminal::Pane::opened_by`]'s field, one level up, and carried for its reason: the
    /// agent-facing mouth keeps an agent to its own runs, and it can only do that if the daemon
    /// remembers whose a run was. The daemon itself enforces nothing with it — see
    /// [`crate::wire::PluginGrammar`] on why this is provenance and not authorisation.
    ///
    /// ⚠⚠ **A PANE ID IS THIS DAEMON'S ANSWER AND NOT A DURABLE ONE** — see
    /// [`opened_by_session`](Self::opened_by_session), which is what survives a restart, and
    /// [`RunRegistry::restore`] for why the two are different questions.
    opened_by: Option<u64>,
    /// **WHICH CONVERSATION ASKED** — the [`sprag_terminal::Pane::agent_session`] of the pane named
    /// by [`opened_by`](Self::opened_by) at the moment it asked, or [`None`] when that pane held no
    /// agent (a person at a shell).
    ///
    /// # ⚠⚠⚠⚠⚠ Identity is the conversation, and the pane is only where it is sitting
    ///
    /// A pane id names a SEAT. It comes back exactly across a restart (`spawn_restored` is given
    /// `pane.id`), so it is stable — but stability is not identity: the same seat can hold a
    /// different agent, or a plain shell, and a run handed to whoever sits there next would be
    /// answered to a stranger. That risk is the whole reason [`RunRegistry::restore`] used to drop
    /// the provenance outright.
    ///
    /// A conversation is the thing that is actually the same thing. An allowlisted agent pane is
    /// restored as `claude --resume <uuid>` ([`crate::durability::restore_command`]), so the agent
    /// that asked comes back holding this exact string — which is what lets a successor daemon
    /// answer *"this run is yours"* without ever having to guess.
    ///
    /// ⚠⚠⚠ **AND IT IS DELIBERATELY NOT WHAT A REPLACEMENT CARRIES.** `ai_loop`'s `restarting`
    /// replaces a session precisely to throw the accumulated context away, so a replaced pane holds
    /// a FRESH conversation and this string stops matching — which is the right answer, arrived at
    /// by construction rather than by a rule. Restoring and replacing want opposite answers here,
    /// exactly as they do one layer down where the host chooses between `argv` and `agent_session`.
    opened_by_session: Option<String>,
    /// ⛔⛔⛔⛔⛔ **WHICH WORKING TREE THIS RUN IS FOR** — register item 890, and the fact this
    /// daemon resolved at the door and then threw away.
    ///
    /// # ⛔⛔⛔⛔⛔ One daemon drives several repositories and nothing said which was which
    ///
    /// Measured 2026-09-04 on this daemon's own store: **209 rows, and 3 carry a `request`** — the
    /// three that had not finished. Everything a reader could use to name a repository lived in
    /// that map, so a finished run named none, and the map is dropped on purpose
    /// ([`PersistedRun::request`] holds the argument: a brief is a person's prose and keeping it
    /// for the life of a log that can never use it is the wrong trade).
    ///
    /// ⇒ ⛔ **Even the live rows named no repository as a FIELD.** Their `request` carried
    /// `plugin`, `pane`, `loop_kind`, `north_star`, `milestone`, `reference`, `agent` — and the
    /// repository only inside the PROSE of `north_star`. Two of the three live runs were
    /// `loop_kind: unclaimed`, so not even the kind told them apart.
    ///
    /// # ⚠⚠⚠ Why this is small and the request is not
    ///
    /// One path. It is the fact `crate::plugins` already resolves at the door to refuse a pane
    /// standing outside the tree its kind works in (register item 738, layer 4) and again to read
    /// what the tree holds at the end (item 682) — computed twice, used twice, recorded never. So
    /// this keeps the ONE thing a later reader needs and none of the prose.
    ///
    /// ⚠⚠ [`None`] is *nobody recorded which tree*, never *this run had none*. A run restored from
    /// a log written before this field existed answers [`None`] and must read as the first —
    /// register item 891's lesson one key over, and this workspace's rule 6.
    tree: Option<String>,
    state: Arc<Mutex<RunState>>,
    /// **THE RUN ITSELF, AS THIS DIRECTORY KNOWS IT** — a [`RunHandle`], which is deliberately not a
    /// thread. See that trait for the fusion it exists to unpick (register item 544); the three
    /// order flags and the `JoinHandle` that used to sit here are [`ThreadRun`]'s private business
    /// now, and a restored run is an [`EndedRun`] rather than three flags nothing reads.
    run: Box<dyn RunHandle>,
    /// WHAT THE RUN HAS SPENT SO FAR, shared with the `Driver` that is spending it.
    ///
    /// The counters were readable only in the terminal `Outcome`, so a client watching a long run
    /// could not tell progress from stuck and could not see spend until it was spent — see
    /// [`sprag_plugin::Progress`].
    progress: ProgressCell,
    /// **WHAT A DRIVER IN ANOTHER PROCESS LAST REPORTED**, or [`None`] for a run whose driver
    /// shares the cell above — register item 650.
    ///
    /// # ⚠⚠⚠⚠⚠ Why a second place and not the cell beside it
    ///
    /// [`sprag_plugin::Progress`] is built out of `&'static str` — its `at`, and three per journal
    /// `Edge` — because it was only ever read in this process. Filling the cell from a wire message
    /// means either interning a statechart vocabulary that is UPSTREAM's to publish, or quietly
    /// dropping the fields that are words. Both are worse than holding what arrived.
    ///
    /// ⚠⚠ So this holds [`crate::plugins::progress_to_json`]'s own output, **verbatim and never
    /// read apart**. A key that renderer grows reaches the row with nothing here to update; a
    /// daemon that unpacked it would need a line per key, and the day one was forgotten it would go
    /// missing for out-of-process runs alone — the invisible divergence
    /// [`crate::options::RUN_DRIVER_PROCESS`] promises cannot happen.
    ///
    /// ⚠ A LEVEL, like the cell: each report REPLACES the last, because what a reader wants is what
    /// the run has done so far and a missed report costs nothing once the next one lands.
    reported: Arc<Mutex<Option<serde_json::Value>>>,
    /// WHICH BUILD DROVE THIS RUN — [`crate::wire::BUILD`] for a run this daemon started, the dead
    /// daemon's for one taken from a predecessor's log, and [`None`] for a log written before this
    /// field existed.
    ///
    /// # ⚠⚠⚠⚠⚠ It is not the caller's to say, which is why it is absent from [`NewRun`]
    ///
    /// Everything else a run brings arrives through that struct because a caller chose it. This one
    /// is a fact ABOUT THE IMAGE the worker will run inside, so letting it travel with the request
    /// would let a caller name a build that did not drive anything — and the whole point (register
    /// item 438) is that the value comes off the running image rather than off anybody's account of
    /// it. [`RunRegistry::submit`] stamps it; only [`RunRegistry::restore`] sets a different one,
    /// and what it sets is what a previous image already said about itself.
    ///
    /// ⚠⚠ **[`None`] means *"nothing recorded which build this was"*, never *"this build"*.** A run
    /// log from an older daemon has no such field, and reading its absence as the reader's own
    /// build would date every restored run to whoever happened to read it — the exact wrong answer
    /// that decodes cleanly which this field exists to prevent.
    build: Option<String>,
    /// ⛔⛔⛔⛔⛔ **WHICH RUN THIS IS** — register item 887, and the answer [`id`](Self::id) was
    /// being read as and cannot give. See [`WhichRun`] for the measurement.
    ///
    /// [`RunRegistry::submit`] stamps it, from this registry's own [`Minting`] and the run's
    /// number; [`RunRegistry::restore`] carries a predecessor's verbatim and **never mints a new
    /// one** — a restored run that was re-stamped here would be a different run every time the
    /// daemon booted, which is the failure this exists to name, inverted.
    ///
    /// ⚠⚠ **[`None`] means *nothing recorded which run this was*, never *the same run*.** A log
    /// written before this field existed carries no stamp, and a reader that filled one in would be
    /// asserting an identity nobody minted — see [`crate::plugins::the_same_run`], which answers a
    /// third word for it rather than guessing either of the other two.
    ///
    /// ⚠ It is not the caller's to say, [`build`](Self::build)'s argument verbatim: it is a fact
    /// about the REGISTRY that admitted this run, so it is absent from [`NewRun`].
    which_run: Option<WhichRun>,
    /// **HOW MANY PROGRESS REPORTS THIS RUN HAS TAKEN** — register item 671's watermark, and the
    /// only monotonic thing a driver in another process gives this daemon.
    ///
    /// # ⚠⚠⚠⚠⚠ Why the count and not what the reports SAY
    ///
    /// The obvious watermark is the run's own `iterations`, and it is wrong: a driver put back at a
    /// saved place counts ITS OWN steps from one (see [`InheritedRun::progress`], which says so as
    /// a property rather than a bug), so a replacement that had worked for minutes would still read
    /// as *behind where the last one got to*. This counter belongs to the RECORD, is never reported
    /// by anybody, and only ever goes up — so *did the driver I started say anything at all* has an
    /// answer that no driver's own bookkeeping can confuse.
    ///
    /// ⚠ [`AtomicU64`] because [`RunRegistry::report`] takes `&self`: a report is a message from
    /// another process and the directory it lands in is shared.
    reports: AtomicU64,
    /// **WHAT [`reports`](Self::reports) STOOD AT WHEN THIS RUN WAS LAST PUT BACK AFTER LOSING ITS
    /// DRIVER** — register item 671, and the whole of the bound on reviving.
    ///
    /// [`None`] for a run that has never lost one, which is why a FIRST death is always answered:
    /// a run that can be put back at all has a place, and a place is written by a machine that took
    /// a step. What this stops is the second death of a driver that never said anything — a broken
    /// image, a request its own door refuses — where respawning is a spin nobody asked for and the
    /// honest answer is to leave the run failed and say so.
    revived_at: Option<u64>,
    /// ⚠⚠⚠⚠⚠ **WHY THE PREDECESSOR'S RECORD OF THIS RUN DID NOT COME BACK WHOLE** — register item
    /// 737, read once by [`RunRegistry::restore`] and kept.
    ///
    /// [`None`] for every run this daemon started itself (nothing was withheld from a run nobody
    /// inherited) and for a restored one whose place and request both crossed the file.
    ///
    /// # ⚠⚠⚠ Why it is stored rather than answered on demand
    ///
    /// Because the evidence is deliberately dropped one line later: a restored record carries
    /// neither the foreign place nor the fingerprint that refused it (see [`RunRegistry::restore`],
    /// which is where item 544's *a changed document makes a new run* is taken by construction), so
    /// nothing downstream could re-derive this. The log is read once, at boot, and this is what is
    /// kept out of that reading. It is [`PersistedRun::withheld`]'s answer and nobody else's.
    withheld: Option<Withheld>,
    /// 🎯🎯🎯🎯🎯 **WHICH BOUNDS THIS RUN'S CALLER TOOK FROM ITS OWN DOCUMENT** — register item
    /// 853. Decided once, at submit, by the layer that held both authors.
    ///
    /// # ⚠⚠ The three claims, on [`crate::plugins::RUN_UNCHECKED_KEY`]'s exact rule
    ///
    /// [`None`] is *nobody said* — a plugin whose document authors no bound, and every run RESTORED
    /// from a predecessor's log, which records the run rather than the answer this was. An EMPTY
    /// [`Overridden`](crate::plugins::Overridden) is the affirmative *this run's document set every
    /// bound it has*, which is the healthy launch and the one a reader needs to be able to get
    /// without inferring it from a missing key. A non-empty one names what the caller took.
    ///
    /// ⚠ **A LEVEL THAT NEVER MOVES**, like [`withheld`](Self::withheld) above it: it is a fact
    /// about the request this run was submitted with, and a row that showed it changing would be
    /// reporting on the reading rather than on the run.
    overridden: Option<crate::plugins::Overridden>,
    /// ⛔⛔⛔⛔⛔ **THE PROCESS THIS BOOT ENDED BECAUSE IT WAS STILL DRIVING A RUN NOBODY IS PUTTING
    /// BACK** — register item 740, written once by `put_back_inherited_runs` through
    /// [`RunRegistry::ended_leftover_driver`].
    ///
    /// [`None`] for every run this daemon started, for one it inherited whole, and for a withheld
    /// one whose predecessor left no live driver behind — an absence said rather than filled in
    /// (register item 709's discipline), because *there was nothing still typing* and *something
    /// was, and this boot ended it* are different facts about somebody's pane.
    ///
    /// # ⚠⚠⚠ Why the ROW needs it, when the boot has already written it to the operator's log
    ///
    /// Because without it the run's ending WORD is settled by an accident nobody wrote down. A
    /// leftover driver ended by a PERSON before the promotion leaves its run [`RunState::Panicked`]
    /// — a live daemon watches its driver die and says so — and the very same driver ended HERE
    /// leaves it [`RunState::Interrupted`], because what happened is that the daemon went away.
    /// **Measured 2026-08-29 across four promotions on this machine**: another repository's watcher
    /// read `panicked` on its run 73 as *my loop hit a bug* and went looking through its own code,
    /// when what had happened was that this machine's promotion had `kill -9`ed the driver first.
    ///
    /// So the row says which of the two this was, in [`crate::plugins::leftover_sentence`]'s words
    /// — the same spelling the boot's log line carries, on [`crate::plugins::withheld_sentence`]'s
    /// argument: a promotion's whole point is that the person who reads it need not be the person
    /// who ran it.
    ended_driver: Option<u32>,
    /// ⛔⛔⛔⛔⛔ **THE PANE A PREDECESSOR'S DRIVER LAST REPORTED THIS RUN WAS ON** — register item
    /// 771, read once by [`RunRegistry::restore`] out of [`PersistedRun::driving`] and kept for the
    /// boot.
    ///
    /// [`None`] for every run this daemon started itself (a live run's pane is
    /// `Progress::driving`, which moves) and for a restored one whose log recorded no pane.
    ///
    /// # ⚠⚠⚠⚠⚠ Why it is not `Progress::driving`, which is the cell it came out of
    ///
    /// Because that cell is a LEVEL — *something is driving that pane right now* (register item
    /// 595) — and after a restart nothing is. `restore` says so at that field in as many words, and
    /// putting a stale pane there would make the row assert the opposite of what item 595 exists to
    /// make visible. This is the same number as a RECORD, held where only the boot reads it:
    /// [`InheritedRun::pane`] is its one consumer, and no projection publishes it.
    drove: Option<PaneId>,
    /// ⛔⛔⛔⛔⛔ **WHY THIS BOOT DID NOT PUT AN INHERITED RUN BACK, THOUGH ITS LOG SAID IT COULD** —
    /// register item 771, written once by `put_back_inherited_runs` through
    /// [`RunRegistry::not_resumed`].
    ///
    /// [`None`] for a run this daemon started, for one that came back, and for one
    /// [`withheld`](Self::withheld) already explains.
    ///
    /// # ⛔⛔⛔⛔⛔ It is the OTHER half of `withheld`, and `interrupted` covered both
    ///
    /// Item 737 split *waiting to be put back* from *no successor ever will, because the documents
    /// moved*. What it could not say is the third thing, which is what a loop actually hit: the
    /// documents were THIS build's, the log's place was resumable, item 737's gate passed — and the
    /// boot still could not stand a driver up, because the pane the run was on is gone.
    /// **Measured 2026-08-30 across one promotion**: four loops, one identical fingerprint, three
    /// back and one not, and the row of the one said `interrupted` and nothing else. The only way
    /// anybody learned why was comparing four log records by hand.
    ///
    /// ⚠⚠ A REASON PER RUN AND NEVER A COUNT, on [`Withheld`]'s argument: *one run stayed behind*
    /// cannot be acted on, and the remedies differ — a pane that did not come back wants a new run,
    /// and a request naming no pane at all is a predecessor that wrote an incomplete record.
    not_resumed: Option<NotResumed>,
    /// ⛔⛔⛔⛔⛔ **THAT A BOOT PUT THIS RUN BACK** — register item 774, and the fact every reader of
    /// a resumed row was missing.
    ///
    /// # ⚠⚠⚠⚠⚠ Why `interrupted` → `running` was not enough to see it
    ///
    /// [`not_resumed`](Self::not_resumed) says why a run did NOT come back. Nothing said that one
    /// DID — a rescued run's row goes back to `running` and is byte-identical to a run somebody
    /// started a second ago. **Measured 2026-08-30 across one promotion**: three loops came back
    /// and, two hours later, had made **zero deliveries** between them while four runs started
    /// fresh in the same window had made one each. The rows said `running` for all seven.
    ///
    /// ⚠⚠ IT IS THE PREMISE OF A SENTENCE AND NOT THE SENTENCE. *Came back* on its own is not worth
    /// a line; what a reader needs is *came back AND has not typed anything yet*, and the second
    /// half is the driver's own counters. This is the half only the registry knows.
    ///
    /// ⚠ [`false`] for a run this daemon started and for one that is still `interrupted` — a row
    /// that nothing has rescued must not claim it was.
    resumed: bool,
}

/// **WHAT A DAEMON SHOULD DO ABOUT A RUN WHOSE DRIVER PROCESS DIED WITHOUT AN OUTCOME** — register
/// item 671, and [`RunRegistry::revival`]'s answer.
///
/// # ⚠⚠⚠⚠ Four words and not a `bool`, because each names a different thing to tell a person
///
/// A daemon that could only say *yes* or *no* here would write one log line for four situations a
/// reader has to tell apart: the run is coming back; nobody knows the run; it never recorded a
/// place to come back to; and its replacement died without saying anything, so this daemon has
/// stopped trying. Collapsing the last three is the shape register item 641 names — *an absence
/// written as one word is a trap the next round walks into*.
pub enum Revival {
    /// **PUT IT BACK**, on the run as this daemon now knows it. The record has already been moved
    /// to [`RunState::Interrupted`], which is the one state
    /// [`put_back`](RunRegistry::put_back) accepts a driver for.
    PutBack(Box<InheritedRun>),
    /// This daemon holds no run with that id.
    NoSuchRun,
    /// It recorded no place, so there is nowhere to put it back — a run whose plugin walks no
    /// statechart, or one whose driver died before it took a step.
    NoPlace,
    /// It recorded no request, so nothing here knows what to build — a run restored from a log
    /// whose brief this image could not read.
    NoRequest,
    /// Its last driver was started by this same door and said NOTHING before dying, so starting a
    /// third would be a spin. See `RunRecord::revived_at`, which holds the watermark.
    NoProgress,
}

impl Revival {
    /// **WHY THE RUN IS NOT COMING BACK**, as one clause, or [`None`] when it is.
    ///
    /// ⚠⚠ THE ONE SPELLING OF EACH REASON, read twice on purpose: [`RunRegistry::revival`] writes
    /// it into the ROW so the person watching a stopped run learns that nothing will pick it up,
    /// and `crate::plugins::PluginsExternal::put_back_a_lost_driver` writes it to the operator's
    /// log beside the run id. Two sentences here would be free to drift into disagreeing about the
    /// same run.
    #[must_use]
    pub const fn not_put_back(&self) -> Option<&'static str> {
        match self {
            Self::PutBack(_) => None,
            Self::NoSuchRun => Some("this daemon holds no run with that id"),
            Self::NoPlace => Some("it recorded no place to be put back at"),
            Self::NoRequest => Some("nothing recorded what it was asked with"),
            Self::NoProgress => Some(
                "the driver this daemon started for it died without reporting a single step, so \
                 starting a third would be a spin",
            ),
        }
    }
}

/// ONE RUN as the `runs` slot reports it.
///
/// A named struct rather than the tuple this was: the opener is a fourth column and a reader has no
/// way to know from its position that it is a PANE and not a run id — the exact argument
/// [`crate::wire::WireSurface`] records against the four-tuple it used to be.
#[derive(Clone, Debug)]
pub struct RunSummary {
    /// The run's id, as `cancel` takes it.
    pub id: RunId,
    /// What the run is, in a reader's terms (`"agent pane=3"`).
    pub label: String,
    /// The pane whose occupant asked for it, or [`None`].
    pub opened_by: Option<u64>,
    /// **WHICH CONVERSATION ASKED**, or [`None`] — `RunRecord::opened_by_session`, republished so a
    /// reader can re-derive the seat when this daemon did not issue the run itself.
    pub opened_by_session: Option<String>,
    /// Where it has got to.
    pub state: RunState,
    /// What it has spent so far — meaningful while [`state`](Self::state) is
    /// [`RunState::Running`], and the last reading the driver took once it is not.
    pub progress: Progress,
    /// **WHAT A DRIVER IN ANOTHER PROCESS LAST REPORTED** — `RunRecord::reported`, republished so
    /// the row can show a run this daemon is not spending for. [`None`] means the driver shares
    /// [`progress`](Self::progress) above, which is every run today.
    ///
    /// ⚠ A reader takes this AHEAD of `progress` when it is here, and that ordering is the whole
    /// point: for such a run the cell beside it never moves, so preferring the cell would publish
    /// zeros over a report that had arrived.
    pub reported: Option<serde_json::Value>,
    /// WHICH BUILD DROVE IT, or [`None`] when nothing recorded one — see `RunRecord::build` for
    /// why those are different answers and why a reader must not fill the second one in.
    pub build: Option<String>,
    /// ⛔⛔⛔⛔⛔ **WHICH RUN THIS IS** — register item 887, and the answer [`id`](Self::id) cannot
    /// give because a successor daemon reissues numbers a predecessor already spent. See
    /// [`WhichRun`] for the measurement, and [`crate::plugins::the_same_run`] for the one thing a
    /// reader may do with it.
    ///
    /// [`None`] is *nothing recorded which run this was* and never *the same run*.
    pub which_run: Option<WhichRun>,
    /// ⛔⛔⛔⛔⛔ **WHICH WORKING TREE THIS RUN WAS FOR** — `RunRecord::tree`, register item 890.
    ///
    /// The field above says which RUN this is and [`build`](Self::build) says which CODE drove it.
    /// This says WHERE, which one daemon driving three repositories had recorded nowhere: measured
    /// 2026-09-04, 3 of 209 rows could name a repository and all three were unfinished.
    ///
    /// ⚠ [`None`] is *nobody recorded which tree* and never *this run had none*.
    pub tree: Option<String>,
    /// **WHETHER A PERSON ASKED THIS RUN TO STAND DOWN** — [`RunHandle::stood_down`], republished so
    /// a mouth can say what became of the ORDER and not only what became of the run.
    ///
    /// ⚠⚠ **IT IS THE ORDER AND NOT THE ENDING.** `true` beside a run that ended `cancelled` means
    /// the order was given and NOT honoured, which is register item 594's whole finding.
    /// [`crate::plugins::stand_down_sentence`] is the one reader allowed to weigh the two together.
    pub stood_down: bool,
    /// ⛔⛔⛔ **WHERE THAT ORDER CAME FROM** — [`RunHandle::stood_down_by`], register item 835, and
    /// the field above's missing half.
    ///
    /// ⚠⚠ It is the ORDER's provenance and never the ending's, on [`stood_down`](Self::stood_down)'s
    /// terms exactly. [`None`] beside a `true` flag means **nobody wrote it down**, and
    /// [`crate::plugins::stand_down_sentence`] is the one reader allowed to say so — in words that
    /// do not name a person nobody recorded.
    pub stood_down_by: Option<StoodDownBy>,
    /// **WHETHER A PERSON IS HOLDING THIS RUN RIGHT NOW** — [`RunHandle::held`], register item 699.
    ///
    /// ⚠⚠⚠⚠⚠ **THE ROW COULD NOT CARRY THIS BECAUSE NOTHING HERE HELD IT**, and the driver that
    /// needs it was reading `row["held"]` — a key no projection ever wrote. That is the whole of
    /// why `hold-run` never parked anything.
    ///
    /// ⚠⚠ A LEVEL, not a latch, which is what separates it from the order above: `false` here means
    /// *nobody is holding it now*, never *nobody ever did*. `resume-run` really does take it back.
    pub held: bool,
    /// ⛔⛔⛔⛔⛔ **WHOSE DECISIONS THIS RUN IS BEING JUDGED BY** — the `loop_kind` its caller named
    /// (register item 848), or [`None`] for a run of any other plugin. Register item 870.
    ///
    /// # ⚠⚠⚠⚠⚠ Item 848 made the caller choose and then nothing said what they chose
    ///
    /// A kind is not a smaller run, it is a run **judged by another document**: `debt` resolves to
    /// `debt_loop.scxml`, whose `successor_check` names
    /// `/home/coin/.claude/projects/-home-coin-sprag/memory/debt-open.md` **by absolute path**, so
    /// every checkpoint a run under it proposes is admitted or refused against THIS repository's
    /// register — whatever tree the run is actually working in. The document says so itself: *"a
    /// classifier that silently answered about whatever tree it happened to land in would be worse
    /// than one that failed to start."*
    ///
    /// **Measured on the live daemon**: of five recent runs, one was `debt` while driving a pane in
    /// another repository's tree, one was `unclaimed`, one was `debt` correctly — and all five rows
    /// printed identically apart from the pane. `wire.rs`' own pin records why: *"`loop_kind` is a
    /// REQUEST word — a caller says it and nothing answers with it."* The key was required at the
    /// door and dropped on the way out, so the one thing 848 made a caller decide was the one thing
    /// no reader could check. ⚠ Do not carry that count — re-derive it:
    /// `jq '[.runs[]|{id,kind:.request.loop_kind}]'` over a daemon's `*.runs.json`.
    ///
    /// ⚠⚠ **READ OFF THE RECORDED REQUEST rather than stored a second time.** The request map is
    /// what a successor puts a run back from, so it is already the one authority on what this run
    /// was asked with; a parallel field would be a second copy of a fact that has to survive a
    /// restart, and the two would drift exactly across the restart that matters.
    ///
    /// ⚠ [`None`] for a plugin that takes no kind, which is most of them — and for a run restored
    /// from a log written before its caller had to name one.
    pub loop_kind: Option<String>,
    /// **WHO RAISED THE CANCEL** — [`RunHandle::cancelled_by`], or [`None`] when none was raised
    /// (and also when the run was restored from disk, where nobody in this process knows).
    ///
    /// ⚠⚠⚠ **A WORD BESIDE `cancelled`, NEVER A WIDER `cancelled`** — register item 596. A person
    /// stopping one run and a daemon sweeping every run on its way out both end a run `cancelled`,
    /// and the remedies are opposite: the first is a decision to respect, the second is a run that
    /// nobody decided anything about and that a person would want back. Splitting the STATE into
    /// two words would have made every existing reader of `cancelled` wrong; a second key leaves
    /// them all correct and merely less informed, which is this repository's shape at
    /// `RUN_CEILING_KEY` and `RUN_STOPPED_KEY` already.
    pub cancelled_by: Option<Canceller>,
    /// ⚠⚠⚠⚠⚠ **WHY THIS RUN DID NOT COME BACK WHOLE FROM A PREDECESSOR'S LOG** — register item
    /// 737, and [`None`] both for a run this daemon started and for one it inherited whole.
    ///
    /// ⚠⚠ **IT IS ON THE ROW BECAUSE THE ROW IS WHERE A PERSON MEETS THE RUN.** The boot writes the
    /// same fact to the operator's log, which is read by whoever was watching the terminal the
    /// daemon was restarted in — and a promotion's whole point is that nobody has to be. The person
    /// who comes back to `sprag runs` sees `interrupted`, and without this there is nothing to
    /// distinguish *your loop is waiting for a daemon to pick it up* from *no daemon ever will*.
    /// `Revival::not_put_back` is the same shape one door over, for a run whose driver died.
    pub withheld: Option<Withheld>,
    /// 🎯🎯🎯🎯🎯 **WHICH BOUNDS THIS RUN'S CALLER TOOK FROM ITS OWN DOCUMENT** — `RunRecord::
    /// overridden`, republished because the row is where a person meets the run, and the row is
    /// what said nothing on 2026-09-03 while a loop spent against ceilings 47 times its document's.
    pub overridden: Option<crate::plugins::Overridden>,
    /// ⛔⛔⛔⛔⛔ **THE PROCESS A BOOT ENDED BECAUSE IT WAS STILL DRIVING THIS WITHHELD RUN** —
    /// register item 740, and [`None`] wherever there was nothing left typing.
    ///
    /// ⚠⚠ It travels beside [`withheld`](Self::withheld) because it is the OTHER half of what a
    /// promotion did to this run: that field says nobody is bringing it back, and this one says
    /// nobody is still working on it either — and until item 740 the second half was decided by
    /// whichever processes a person happened to `kill` by hand first.
    pub ended_driver: Option<u32>,
    /// ⛔⛔⛔⛔⛔ **WHY A BOOT COULD NOT PUT THIS RUN BACK, THOUGH ITS LOG SAID IT COULD** — register
    /// item 771, and [`None`] both for a run nothing tried to resume and for one that came back.
    ///
    /// ⚠⚠ **IT IS EXCLUSIVE WITH [`withheld`](Self::withheld) BY CONSTRUCTION**, not by a reader's
    /// discipline: `RunRegistry::not_resumed` refuses a record that already carries a withheld
    /// reason. The two are the two halves of *why is this row still `interrupted`* — one decided
    /// while reading the log, the other while acting on it — and item 737 could only ever see the
    /// first.
    pub not_resumed: Option<NotResumed>,
    /// ⛔⛔⛔⛔⛔ **THAT A BOOT PUT THIS RUN BACK** — register item 774, and
    /// [`not_resumed`](Self::not_resumed)'s twin: that field says why one stayed behind, this says
    /// one came back.
    ///
    /// ⚠⚠ **IT IS A PREMISE AND NOT A SENTENCE.** *Came back* alone is not worth a reader's line;
    /// what item 774 is about is *came back and has not typed anything since*, and the second half
    /// lives in the driver's own counters. This is the half only the registry can answer, and
    /// without it a rescued row and a row started a second ago are byte-identical.
    pub resumed: bool,
}

/// EVERYTHING A RUN BRINGS WITH IT — the argument list of [`RunRegistry::submit`], as a struct.
///
/// A named struct rather than seven positional parameters, and the argument is [`RunSummary`]'s one
/// level up: a reader at the call site has no way to know from POSITION that the fifth thing is the
/// worker's join handle and the sixth is where it writes its counters. (Clippy said the same thing
/// about the arity, which is the cheap version of the same point.)
pub struct NewRun {
    /// The id [`RunRegistry::reserve`] gave, and which the worker announces under.
    pub id: RunId,
    /// What the run is, in a reader's terms.
    pub label: String,
    /// **WHICH PLUGIN THIS RUN IS** — see `RunRecord::plugin` for why it is a type rather than a
    /// word parsed back out of [`label`](Self::label).
    pub plugin: crate::plugins::PluginName,
    /// **THE REQUEST IT WAS ASKED WITH** — see `RunRecord::request`. A caller that holds no map
    /// hands [`None`] and gets a run its successor cannot put back, which is what every run in
    /// this daemon was before register item 543's sixth brick.
    pub request: Option<serde_json::Map<String, serde_json::Value>>,
    /// The pane whose occupant asked for it, or [`None`] for a run nobody claims.
    pub opened_by: Option<u64>,
    /// **WHICH CONVERSATION ASKED** — the asking pane's `agent_session`, resolved by the caller
    /// (which is the layer holding the workspace) rather than looked up here. See
    /// `RunRecord::opened_by_session`.
    pub opened_by_session: Option<String>,
    /// ⛔⛔⛔⛔⛔ **WHICH WORKING TREE THIS RUN IS FOR** — register item 890, resolved by the caller
    /// because that is the layer holding the workspace. See `RunRecord::tree`.
    ///
    /// ⚠ [`None`] for a run whose pane names no directory this daemon can read, and for a plugin
    /// that drives no pane of its own. It must never be filled in with a guess: *nobody recorded
    /// which tree* and *this run had none* are different facts and only the first one is true here.
    pub tree: Option<String>,
    /// Where the worker writes its terminal state.
    pub state: Arc<Mutex<RunState>>,
    /// **THE RUN ITSELF** — a [`RunHandle`], and deliberately not a thread plus three flags. A
    /// caller spawning an in-process worker hands a [`ThreadRun`]; see that trait for why the
    /// registry is not allowed to know which kind it got (register item 544).
    pub run: Box<dyn RunHandle>,
    /// Where the driver writes what it has spent so far.
    pub progress: ProgressCell,
    /// **WHICH BOUNDS THIS RUN'S CALLER TOOK FROM THE DOCUMENT THAT AUTHORED THEM** — register item
    /// 853, answered at the door by `crate::plugins::parse_guardrails` and [`None`] when the run's
    /// plugin has no document that authors any bound.
    pub overridden: Option<crate::plugins::Overridden>,
}

/// **A RUN A PREDECESSOR DAEMON LEFT BEHIND THAT THIS ONE COULD PICK UP** — register item 543's
/// sixth brick, and everything [`crate::plugins::PluginsExternal::put_back`] needs to do it.
///
/// # ⚠⚠⚠⚠⚠ Why the registry hands this out instead of resuming anything itself
///
/// Putting a run back means building a plugin, and a plugin needs a pane, a script engine and a
/// world to validate against — none of which this directory has or should have. What it does have
/// is the only copy of what a predecessor wrote down. So the split is the one this file already
/// makes for orders: **finding the run is the directory's job, and knowing what to do with it is
/// somebody else's.**
///
/// ⚠⚠ **AND IT IS HANDED OUT AFTER THE ROW ALREADY EXISTS**, never instead of one. A restored run
/// is listed the moment [`RunRegistry::restore`] reads it, `interrupted` and honest; a boot that
/// then puts it back replaces the driver ([`RunRegistry::put_back`]). Holding these back out of the
/// list until a boot decided would make a daemon that crashed mid-boot answer *there is no such
/// run* about a run its own log carries.
#[derive(Clone)]
pub struct InheritedRun {
    /// The id it had, and keeps. A resumed run is the SAME run — a new id would make a reader who
    /// is watching one row watch a row that has stopped moving.
    pub id: RunId,
    /// What the run is, in a reader's terms — the predecessor's own label, kept so a resumed row
    /// does not rename itself under whoever was reading it.
    pub label: String,
    /// **WHERE ITS MACHINE WAS**, as `sprag_plugin::Plugin::place`'s words — already checked against
    /// this image's documents by `PersistedRun::resumable_place`.
    pub place: Vec<String>,
    /// **WHAT IT WAS ASKED WITH** — `crate::plugins::plugin_from_request`'s map. Already checked
    /// against the place by `PersistedRun::resumable_request`.
    pub request: serde_json::Map<String, serde_json::Value>,
    /// The cell the restored row already publishes, so a new driver writes where the row reads.
    ///
    /// ⚠⚠ **HANDED OVER RATHER THAN REPLACED, AND WHAT THAT BUYS IS EXACTLY ONE THING**: the row
    /// goes on showing what the predecessor recorded until the new driver's FIRST STEP, instead of
    /// dropping to zero the moment the run comes back. A fresh cell would make a person watching
    /// the row see the run's history vanish at the instant it was rescued.
    ///
    /// ⚠ From that first step the counters are the NEW driver's own and the inherited ones are
    /// gone — written down because it is a real loss and not an oversight. It is also the right
    /// answer: the guardrails this driver runs under bound THIS driver's work, so a total carried
    /// over would be measured against a ceiling nobody set. What survives the restart whole is the
    /// run's place, which is the thing the work is actually made of.
    pub progress: ProgressCell,
    /// **THE PROCESS THE DEAD DAEMON HAD DRIVING IT, IF THE LOG RECORDED ONE** — register item 526.
    ///
    /// ⚠⚠⚠ The boot has to END this before it starts a driver of its own, and the reason is not
    /// tidiness: a driver that outlived its daemon (item 544's stage 1) goes on typing at the
    /// agent, while its OUTCOME travels on the stdout pipe of a parent that is gone — so it can
    /// finish work nobody will ever be able to read. The run log is the channel that survives a
    /// restart, which is why the run comes back through the log and the leftover process does not.
    pub driver: Option<u32>,
    /// ⛔⛔⛔⛔⛔ **THE PANE ITS OWN DRIVER LAST SAID IT WAS ON** — register item 771,
    /// [`PersistedRun::driving`] carried through `RunRecord::drove`, and [`None`] when nothing ever
    /// reported one.
    ///
    /// ⚠⚠ **READ THROUGH [`pane`](Self::pane), NEVER DIRECTLY.** A caller that took this field on
    /// its own would put a loop back on a pane while rebuilding its plugin from a request that
    /// names a different one — two answers to *where does this run type*, which is the failure this
    /// field was added to end rather than to re-create one layer down.
    pub drove: Option<PaneId>,
}

impl InheritedRun {
    /// ⛔⛔⛔⛔⛔ **WHERE THIS RUN ACTUALLY IS** — register item 771, and the ONE answer both the
    /// boot and [`crate::plugins::PluginsExternal::put_back`] take.
    ///
    /// The live pane its driver last reported, and the pane its REQUEST names only when nothing
    /// reported one. [`None`] when neither does.
    ///
    /// # ⚠⚠⚠⚠⚠ Why the report wins over the request, and it is not a preference
    ///
    /// The request's pane is a birth certificate. `sprag_plugin::OuterLoop` replaces its inner
    /// session as it works — that is what `restarting` IS — and every replacement is a new pane, so
    /// a loop that has reflected even once is not on the pane it was asked over. The same
    /// distinction is already drawn for the PERSON reading the row: `crate::plugins::RUN_DRIVING_KEY`
    /// exists because a watcher followed a run's label to a pane that no longer existed (register
    /// item 726). This is that repair, for the reader that is a boot.
    ///
    /// ⚠⚠ A run whose driver reported nothing falls back rather than refusing, and that is the
    /// honest direction: a plugin that never moves (`agent`, `dialogue`) reports the pane it was
    /// asked over anyway, and one that took no step at all has nothing better to offer than what it
    /// was asked with.
    #[must_use]
    pub fn pane(&self) -> Option<PaneId> {
        self.drove
            .or_else(|| crate::plugins::pane_named(&self.request))
    }

    /// **THE REQUEST TO REBUILD THIS RUN FROM**, with [`pane`](Self::pane)'s answer written into it.
    ///
    /// # ⚠⚠⚠⚠⚠ Because putting it back over the right pane is not enough on its own
    ///
    /// The boot resolves a pool from [`pane`](Self::pane), and `put_back` then builds the plugin
    /// from a MAP — `crate::plugins::plugin_from_request`, which reads the pane out of the request
    /// and validates it exists. Left alone, those two are different numbers for a run that moved:
    /// the boot would find the live pane, and the plugin it stood up would type into the dead one
    /// (or, more usually, be refused because that pane is gone). One question, one answer.
    ///
    /// ⚠⚠ **A DAEMON MAY SAY THIS AND A CLIENT MAY NOT**, which is `put_back`'s own argument for
    /// being the one writer of `crate::plugins::RUN_PLACE_KEY`: this number did not come from
    /// anybody's request, it came out of this daemon's predecessor's log, where the run's OWN
    /// driver put it.
    ///
    /// ⚠ Unchanged when nothing was reported — the map then already carries whatever the log had,
    /// including nothing.
    #[must_use]
    pub fn asked_here(&self) -> serde_json::Map<String, serde_json::Value> {
        let mut asked = self.request.clone();
        if let Some(pane) = self.drove {
            asked.insert(
                crate::plugins::RUN_PANE_KEY.to_owned(),
                serde_json::json!(pane.0),
            );
        }
        asked
    }
}

/// ⚠⚠⚠⚠⚠ **WHY A RUN A SUCCESSOR FOUND IN ITS PREDECESSOR'S LOG IS NOT COMING BACK** — register
/// item 737, and the half of [`RunRegistry::inheritance`] that used to be an empty list.
///
/// # ⛔⛔⛔⛔⛔ *Nothing was withheld* and *everything was* were the same answer
///
/// A promotion swaps the binary, and **the reason to promote is usually a changed document** —
/// `sprag-plugin`'s own `build.rs` says so in as many words: *"the restart that motivates
/// persisting a run at all is a document change, so the dangerous case is the common one rather
/// than the rare one."* A changed `.scxml` changes [`sprag_plugin::STATECHARTS_FINGERPRINT`], and
/// [`PersistedRun::resumable_place`] compares that fingerprint for EQUALITY — so every run in the
/// log is withheld at once and [`RunRegistry::inheritance`] used to answer `[]`, which is precisely
/// what it answers for a daemon that had no runs to leave.
///
/// **Measured on this machine 2026-08-28**, before any of this existed: the loop daemon's log held
/// six unfinished runs, two of them carrying a place recorded against `091c26165f46a34d`, and the
/// tree they would have been promoted into fingerprints `3eabd86deafd4848`. The next promotion puts
/// back none of them and **nothing anywhere said so** — not the boot, not the run's row, not the
/// person who ran it.
///
/// # ⚠⚠ Withholding them is the DECISION, and this type is not an apology for it
///
/// Item 544 chose it: a configuration read against a document it did not come from decodes cleanly
/// and is WRONG, so a changed document makes a NEW run deliberately. What was missing is that the
/// decision was taken in silence — so a reader took item 526's *a promotion does not kill somebody
/// else's loop* for an unconditional promise, which it never was.
///
/// ⚠ Every arm is a REASON a caller can act on, and there is no catch-all: a fifth way of failing
/// to come back gets an arm on the day it exists rather than being folded into an existing one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Withheld {
    /// **ITS PLACE WAS SPELLED IN DOCUMENTS THIS IMAGE DID NOT COMPILE** — what a promotion causes,
    /// and the only arm whose cause is somebody else's act rather than an absence.
    ForeignDocuments {
        /// The fingerprint the log recorded, which is the predecessor's own — compare it against
        /// [`sprag_plugin::STATECHARTS_FINGERPRINT`], which is this image's.
        theirs: String,
    },
    /// The log recorded no position at all: a run that had completed no step, or a plugin that
    /// walks no statechart. There is nothing to put back rather than something being refused.
    NoPlace,
    /// A place with no fingerprint beside it — a log older than the pair. Nothing can say which
    /// documents those words came from, so they are words from an unknown vocabulary.
    NoDocument,
    /// A place this image CAN read, and nothing recorded what the run was asked with, so no plugin
    /// could be rebuilt to enter at it.
    NoRequest,
}

/// ⛔⛔⛔⛔⛔ **WHY A BOOT COULD NOT STAND A DRIVER UP FOR A RUN ITS LOG SAID WAS RESUMABLE** —
/// register item 771, and the half of a promotion [`Withheld`] cannot reach.
///
/// # ⛔⛔⛔⛔⛔ *Withheld* and *tried and could not* were the same word, and it was `interrupted`
///
/// [`Withheld`] answers a question asked while READING the log: are these words this build's? Item
/// 737 made that answer visible on the row, and it is the common case. It is not the only case. A
/// run whose place, request and fingerprint all crossed intact is handed to the boot as resumable —
/// and the boot then has to find the pane, build the plugin and place the machine, any of which can
/// refuse. Before this type every one of those refusals reached the operator's log and NOTHING
/// else: the row said `interrupted`, exactly as it says for a run waiting to be picked up.
///
/// **Measured 2026-08-30, one promotion, four loops.** All four carried fingerprint
/// `b92e993a99bd7d46` — this build's — so item 737 withheld none of them. Three came back. The
/// fourth had replaced its inner session twice while it worked (369 → 389 → 394), its log recorded
/// the pane it was BORN on, that pane was gone, and it stayed behind. Its row said `interrupted`,
/// its `withheld` clause was empty because nothing was withheld, and the daemon's boot log said
/// nothing a person went looking for. **The only way anybody found out was reading four log records
/// side by side.**
///
/// # ⚠⚠⚠⚠⚠ Every exit from that loop has an arm, and there is deliberately no catch-all
///
/// A boot that could not put a run back and did not say which of these it was would be the silence
/// this type is for, one level in. So `put_back_inherited_runs` has no `continue` this does not
/// name — the workspace's own rule that *unclassified is RED and not a pass* — and a fourth way of
/// failing gets a fourth arm on the day it exists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotResumed {
    /// ⛔ **THE PANE IT WAS ON DID NOT COME BACK.** Carries the pane this boot looked for, which is
    /// the one [`InheritedRun::pane`] chose — the live one its driver last reported when there is
    /// one, and the request's otherwise.
    PaneGone {
        /// The pane that was looked for and is not held by any pool.
        pane: u64,
        /// Whether that number came from the run's own driver ([`InheritedRun::drove`]) rather than
        /// from the request it was opened with.
        ///
        /// ⚠⚠ It is the difference between *your loop's current pane is gone* and *the pane this
        /// run was opened over is gone, and nothing ever reported a newer one* — the second is also
        /// what a run that never took a step looks like, and a person deciding whether to start the
        /// loop again reads them differently.
        reported: bool,
    },
    /// Nothing named a pane at all: the request crossed the log without the key that says which
    /// pane the run works in, so nothing could say which pane pool to put it back over.
    NoPane,
    /// ⛔ **THE PUT-BACK ITSELF REFUSED**, in the words `crate::plugins::PluginsExternal::put_back`
    /// used — a plugin word this build no longer spells, a guardrail it cannot parse, or a machine
    /// that will not be placed where the log said.
    ///
    /// ⚠ The sentence is carried rather than re-authored, on `Revival::not_put_back`'s rule: the
    /// door that refused is the one that knows why, and a second wording here would be free to
    /// drift from it.
    Refused(String),
}

/// **ONE RUN A SUCCESSOR IS NOT PUTTING BACK, AND WHY** — register item 737, the members of
/// [`Inheritance::withheld`].
///
/// ⚠ It carries the id and the label because a reason nobody can attach to a run is a reason nobody
/// can act on: *two runs stayed behind* is a different message from *run 46, the loop on pane 219,
/// stayed behind*.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WithheldRun {
    /// The id it had, which is the id its row still shows.
    pub id: RunId,
    /// What it was, in a reader's terms — the predecessor's own label.
    pub label: String,
    /// Why it is not coming back.
    pub why: Withheld,
    /// ⛔⛔⛔ **THE PROCESS THE DEAD DAEMON HAD DRIVING IT, IF THE LOG RECORDED ONE** — the same
    /// field [`InheritedRun::driver`] carries, and it matters MORE here than there.
    ///
    /// A run that comes back has its leftover driver ended by the boot (see
    /// `put_back_inherited_runs`, register item 526), because two processes over one pane is the
    /// failure that gate was built for. A run that does NOT come back is never reached by that
    /// loop, so its leftover — a process of its own since item 544's stage 1 — is still there,
    /// still typing into somebody's pane, and reporting its outcome down the stdout pipe of a
    /// daemon that no longer exists. **Nobody will ever read what it does.**
    ///
    /// ⚠⚠ Whether a boot should END such a process is a decision this type does not take: stopping
    /// it stops a loop that is still working, and leaving it costs an unreadable answer. What is
    /// NOT defensible is the third option this repository was in — not deciding, and not saying.
    pub driver: Option<u32>,
}

/// **WHAT A BOOT INHERITED FROM ITS PREDECESSOR, AND WHAT IT DID NOT** — register item 737.
///
/// # ⚠⚠⚠⚠⚠ Why one door answers both halves
///
/// Because a caller that could ask only the first cannot tell *there was nothing there* from
/// *everything was refused*, and those are opposite facts about a promotion. `inherited()` returned
/// a `Vec` and every caller that read it — the boot included — treated an empty one as *this
/// predecessor left nothing*. A second, separate reader for the refusals would be worse: it would
/// be optional, and the reader who most needs it is the one who did not think to ask.
///
/// ⚠ So the two travel together and neither can be read without the other being in reach.
/// ⚠ No `Debug`: [`InheritedRun`] holds a live progress cell and has none, deliberately — a run's
/// counters are read through the lock that owns them.
#[derive(Clone, Default)]
pub struct Inheritance {
    /// Every run this daemon CAN pick up, in submit order — both halves of a resume survived.
    pub resumed: Vec<InheritedRun>,
    /// Every unfinished run that stayed behind, in submit order, each with its reason.
    pub withheld: Vec<WithheldRun>,
}

/// ONE RUN AS IT SURVIVES ITS DAEMON — the durable mirror of a live run record.
///
/// # ⚠⚠ Why the host defines this instead of deriving serde on the plugin types
///
/// `sprag-plugin` is deliberately serde-free (*"serialization is a host concern, so the
/// pinion-free substrate stays serde-free"* — [`crate::plugins`]'s own rule for the wire). The same
/// rule applies to a FILE: a durable format is a host concern, and deriving it upstream would let a
/// refactor in the substrate silently change what is on somebody's disk.
///
/// It carries what a reader needs to see what the run managed, and nothing that could not survive:
/// no thread, no cancel flag, no panes.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PersistedRun {
    /// The id it had, so restored ids are never reissued.
    pub id: u64,
    /// What the run was, in a reader's terms.
    pub label: String,
    /// How many steps it had completed.
    pub iterations: u32,
    /// What it had spent, and in what unit — `None` for a run that took no measured step.
    pub cost: Option<u64>,
    /// The unit of [`cost`](Self::cost).
    pub unit: Option<String>,
    /// ⛔⛔⛔⛔⛔ **WHEN THIS RECORD LAST DIFFERED** — unix seconds, register item 801, part ⑵.
    ///
    /// # Why a run log with no clock cannot say *stopped*
    ///
    /// Every other field here says WHAT a run is; none said WHEN. Measured 2026-09-01 over the
    /// live loop's 145 records: not one field carried a time — `at` is a state NAME
    /// (`working`, `judging`) — so *finished* was answerable and *has not moved in three hours*
    /// was not, at any distance. Item 798 widened its own done-when to cover a run that STOPS, and
    /// this absence is where that widening ran out of road.
    ///
    /// ⚠⚠ IT IS STAMPED WHERE THE DIFFERENCE IS ALREADY KNOWN — `crate::durability`'s
    /// `save_runs_if_changed`, which holds the previous log in order to decide whether to write at
    /// all. Nothing in this module has to grow a clock, and the answer covers BOTH driver kinds:
    /// for a run driven in another process the progress cell never moves (item 662), so a stamp
    /// taken from the cell would be a lie about exactly the runs anybody reads.
    ///
    /// ⚠ [`None`] is *this build never recorded it* — a log written before this field, or a clock
    /// that would not answer — and never *it has not moved*. The two are the fold this register's
    /// 776 family keeps paying for, and a reader that cannot tell them apart is back where item
    /// 801 started.
    #[serde(default)]
    pub moved_at: Option<u64>,
    /// ⛔⛔⛔⛔⛔ **WHEN A LOG FIRST SAID IT HAD FINISHED** — unix seconds, register items 801 ⑴
    /// and **888**, and the headline is the item: this said *WHEN IT ENDED* for three days while
    /// meaning the sentence below it.
    ///
    /// Stamped once, on the first log in which [`finished`](Self::finished) is true, and carried
    /// unchanged after that: a recording is a moment, so a value that moved would be a second one.
    ///
    /// # ⛔⛔⛔⛔⛔ Why that is not the run's ending, measured
    ///
    /// ```text
    /// python3 -c "
    /// import json,collections,datetime
    /// rows=json.load(open('~/.local/share/sprag-loop/state/sprag/sprag-loop.runs.json'))['runs']
    /// c=collections.Counter(r.get('ended_at') for r in rows)
    /// print(len(rows), [(datetime.datetime.fromtimestamp(t).strftime('%m-%d %H:%M:%S') if t else None, n)
    ///                   for t,n in c.most_common(2)])"
    /// ```
    /// ⇒ **220 rows, 154 of them dated `09-02 21:43:55`** — one second. Their outcomes are real and
    /// varied (61 `failed`, 36 `converged`, 27 `cancelled`, 17 `exhausted`, 13 `blocked`) and their
    /// builds are a dozen different ones, so they are not one event: **they are every run that had
    /// already finished when the first daemon carrying this column wrote its first log.** The
    /// column shipped in `8bbedf1` at 09-01 16:51 and the daemon holding it started a day later,
    /// and `stamp_run_times`'s `(true, None) => now` arm cannot tell *it just ended* from *it ended
    /// before anybody was writing this down*.
    ///
    /// ⚠ Register item 888 guessed the cause was a boot marking unfinished runs finished all at
    /// once, and said in the same breath that it had not measured it. It is the opposite: those 154
    /// runs were finished long before, by daemons that watched them end and had nowhere to say so.
    ///
    /// ⚠⚠ **THE 154 CANNOT BE REPAIRED** — this stamp never moves once set, so their wrong second
    /// is permanent. Register item 891's rule: a column's SHAPE is retroactive and its VALUES are
    /// not. What the pair below can do is stop the next 154 from being invented.
    ///
    /// ⚠ [`None`] carries [`moved_at`](Self::moved_at)'s meaning exactly — *nobody recorded it*.
    #[serde(default)]
    pub ended_at: Option<u64>,
    /// 🎯🎯🎯🎯🎯 **WHEN A DAEMON WATCHED THIS RUN BEGIN** — unix seconds, register item 888, and
    /// the left end of the only interval anybody may subtract.
    ///
    /// Stamped on the first log a daemon writes that carries this run **and whose predecessor did
    /// not**, which is exactly *this daemon created it*, and carried unchanged after that. A run
    /// INHERITED from a predecessor's log is in that predecessor's log by construction, so it gets
    /// [`None`] — its beginning happened where nothing was watching.
    ///
    /// # ⛔⛔⛔ Why the field exists at all
    ///
    /// Register item 872 ⑶ is *measure the delay between a run ending and the next one being
    /// launched*, and three re-judgements in a row could not: `awk '/pub struct PersistedRun/,/^}/'
    /// … | grep -cE 'pub (started_at|start_at|began_at|birth)'` answered **0**, so the register
    /// recorded the clause as blocked behind this item four times. There was no left end.
    ///
    /// ⚠⚠ **THE RESIDUE, STATED**: this is the first LOG a daemon wrote carrying the run, not the
    /// instant it was submitted, so it is late by up to one save tick. That is `moved_at`'s own
    /// bargain (item 801: nothing in this module holds a clock) and it is why the name says
    /// *watched* rather than *started*. Every run already in the store gets [`None`] for ever.
    #[serde(default)]
    pub ran_from: Option<u64>,
    /// 🎯🎯🎯🎯🎯 **WHEN A DAEMON WATCHED THIS RUN STOP** — unix seconds, register item 888, and
    /// the right end of that interval.
    ///
    /// Stamped when a log finds this run [`finished`](Self::finished) and the log before it did
    /// NOT — the transition happened inside one tick, so a daemon was watching. Carried unchanged
    /// after that.
    ///
    /// ⚠⚠⚠ **THIS IS THE DISCRIMINATOR [`ended_at`](Self::ended_at) HAS NO ROOM FOR.** That field
    /// reads `(finished, before.ended_at)` and never asks whether `before` was ALREADY finished, so
    /// a run whose ending predates the column and a run that just ended reach the same arm. This
    /// one asks, and answers [`None`] for the first — which is why the 154 rows in that field's
    /// measurement get nothing here rather than a second that is off by two days.
    ///
    /// ⚠ [`None`] therefore means *nobody was watching when this stopped*, which covers a run out
    /// of a log older than this column, one inherited already finished, and one whose daemon died
    /// with it. Never a zero: `now_unix_secs` answers [`None`] for a clock that will not speak.
    #[serde(default)]
    pub ran_to: Option<u64>,
    /// Whether it had already finished. A run still `Running` when the daemon died comes back
    /// [`RunState::Interrupted`]; one that had finished keeps having finished.
    pub finished: bool,
    /// Its rendered terminal state (`"converged"`, `"exhausted"`, …) when `finished`.
    pub outcome: Option<String>,
    /// Which ceiling stopped it, when one did.
    pub ceiling: Option<String>,
    /// What it captured, when it captured anything.
    pub output: Option<String>,
    /// WHICH BUILD DROVE IT — see `RunRecord::build`. `#[serde(default)]`, so a log written before
    /// this field loads as [`None`] rather than being refused.
    ///
    /// ⚠⚠⚠ **AND THAT IS WHY [`RUN_LOG_VERSION`] DOES NOT MOVE.** That constant refuses a format
    /// this build cannot READ, and an optional field with a default is readable in both directions:
    /// an older daemon ignores a key it does not know (serde's default for an unknown field), and
    /// this one fills the absence with the honest answer. Bumping it would throw away every run
    /// record the running daemon holds in exchange for a field nothing needs to be told twice —
    /// the same trade the wire's own *added answer key* rule declines.
    #[serde(default)]
    pub build: Option<String>,
    /// ⛔⛔⛔⛔⛔ **WHICH RUN THIS IS, WHEN THE NUMBER CANNOT SAY** — register item 887, and the
    /// field whose absence let three of this daemon's own numbers name two runs each. See
    /// [`WhichRun`] for the measurement.
    ///
    /// # ⛔⛔⛔⛔⛔ This file is where the reuse comes FROM, which is why the stamp has to be in it
    ///
    /// A successor sets `next_id` to `max(saved.id) + 1` **over the rows it finds here**, so a log
    /// that is missing rows is a log that hands out numbers a predecessor already spent. The
    /// numbers in this file are therefore the exact numbers that repeat, and a stamp that lived
    /// only in memory would be gone on precisely the boot that needed it.
    ///
    /// ⚠⚠ [`None`] is *nobody wrote down which run this was* and never *the same run*. A log
    /// written before this field existed carries none, and [`RUN_LOG_VERSION`] does not move for it
    /// — [`build`](Self::build)'s argument, and the call items 606, 616, 762 and 856 each made.
    #[serde(default)]
    pub which_run: Option<String>,
    /// **WHICH PROCESS WAS DRIVING IT** — register item 526, [`RunHandle::driver_pid`]'s value put
    /// where a successor daemon can read it.
    ///
    /// ⚠⚠⚠⚠⚠ **A SUCCESSOR MUST NOT START A SECOND DRIVER OVER A PANE THAT ALREADY HAS ONE**, and
    /// before this field it had no way to tell. Measured 2026-08-25: a daemon replaced under two
    /// live loops left **five** `sprag-term` against one socket where three was right — the two
    /// drivers that outlived it (item 544's stage 1, on purpose) plus two the boot spawned to put
    /// the same two runs back (item 543). Two processes typing at one agent, invisible to every row.
    ///
    /// ⚠⚠ [`None`] for a thread-driven run and for a log written before this field existed —
    /// [`RUN_LOG_VERSION`] does not move for it, on [`build`](Self::build)'s argument.
    #[serde(default)]
    pub driver: Option<u32>,
    /// ⛔⛔⛔⛔⛔ **WHICH PANE THE RUN WAS ACTUALLY ON WHEN THE DAEMON DIED** — register item 771,
    /// `Progress::driving` as its own driver last reported it, and the answer that is NOT in
    /// [`request`](Self::request).
    ///
    /// # ⛔⛔⛔⛔⛔ A run that replaced its session came back to a pane that was gone
    ///
    /// The request names the pane the run was ASKED over, and for most plugins that is where it
    /// stays. A loop is not most plugins: `sprag_plugin::OuterLoop` REPLACES its inner session as it
    /// goes, and each replacement is a new pane — so the request's number is a birth certificate
    /// and the run is somewhere else. `crate::plugins::RUN_DRIVING_KEY` exists because a person
    /// reading the row had the same problem (register items 540 and 726), and until this field the
    /// BOOT did not have the answer that mouth did.
    ///
    /// **Measured 2026-08-30 across one promotion, four loops, 4/4 clean**: run 101 had replaced
    /// its session twice (369 → 389 → 394), its log recorded 369, and pane 369 was gone — so
    /// `put_back_inherited_runs` found no pool holding it and the run stayed `interrupted`. The
    /// three that had replaced nothing came back. All four carried the SAME fingerprint
    /// (`b92e993a99bd7d46`), so item 737's gate passed on all four and only the pane told them
    /// apart. **Measured in the same reading**: `driving` was absent from all 111 records in that
    /// log, because nothing wrote it — and re-measured that afternoon at 113 records, still zero.
    /// ⚠ Do not carry the count forward; re-derive the PREDICATE, which is what does not age:
    /// `jq '[.runs[] | select(has("driving"))] | length'` over the live daemon's `*.runs.json`.
    ///
    /// ⚠⚠⚠ **THIS IS NOT THE PANE ID [`opened_by_session`](Self::opened_by_session) REFUSES TO
    /// CARRY.** That one is a SEAT — *who asked* — and the objection to persisting it is that a
    /// successor cannot know whether the occupant of pane 3 is still the asker. This is a
    /// WORKPLACE — *where the run types* — and it crosses already, in
    /// [`request`](Self::request)'s own `pane` key. What this adds is the CURRENT one beside the
    /// original, which is the whole of what a run that moved was unable to say.
    ///
    /// ⚠⚠ **AND IT IS A RECORD RATHER THAN A LIVE READING**, which is why
    /// [`RunRegistry::restore`] does not put it back into `Progress::driving`: that cell means
    /// *something is driving that pane right now* (register item 595) and after a restart nothing
    /// is. It goes to `RunRecord::drove`, which the boot reads and no row publishes as live.
    ///
    /// ⚠ [`None`] for a run that never reported a pane and for a log written before this field
    /// existed — [`RUN_LOG_VERSION`] does not move for it, on [`build`](Self::build)'s argument.
    #[serde(default)]
    pub driving: Option<u64>,
    /// **WHICH CONVERSATION ASKED FOR IT** — `RunRecord::opened_by_session`, and the ONE piece of
    /// provenance that means anything to a successor daemon.
    ///
    /// ⚠⚠⚠⚠ **THE PANE ID IS DELIBERATELY NOT HERE.** It would decode cleanly and answer wrongly:
    /// pane 3 comes back as pane 3, but the successor has no way to know whether the thing sitting
    /// in it is the asker or a stranger who booted into the same seat. The conversation carries the
    /// identity, so a successor can answer that question instead of guessing at it —
    /// [`RunRegistry::restore`] states the decision this field re-takes.
    ///
    /// ⚠⚠⚠ **AND [`RUN_LOG_VERSION`] DOES NOT MOVE FOR IT**, on [`build`](Self::build)'s argument
    /// verbatim: an optional field with a default is readable in both directions, and bumping would
    /// throw away every run record the running daemon holds to gain a key nothing needs told twice.
    #[serde(default)]
    pub opened_by_session: Option<String>,
    /// ⚠⚠⚠⚠⚠ **WHERE THE RUN HAD GOT TO** — `Progress::at`, the plugin's own machine position, in
    /// the document's own word. [`None`] for a run that never completed a step, one whose plugin
    /// walks no statechart, or a log written before this field existed.
    ///
    /// # The one thing an interrupted run could never say — register item 543
    ///
    /// A run that outlives its daemon comes back [`RunState::Interrupted`] carrying its counters,
    /// so a reader learns HOW FAR it got and never WHERE it stopped. The position did exist, as a
    /// sentence inside a step note — in a journal bounded to sixty-four steps and **deliberately
    /// not persisted**. So the question a person actually asks of a killed run (*was it mid-turn,
    /// or waiting on me?*) had no answer at all, and `awaiting_human` and `working` were the same
    /// record.
    ///
    /// ⚠⚠⚠ **IT IS MEANINGLESS WITHOUT [`document`](Self::document) AND MUST NEVER BE READ ALONE.**
    /// A state name's meaning lives in a `.scxml`, and the restart this record exists for is
    /// usually a document change — see that field.
    ///
    /// ⚠ [`RUN_LOG_VERSION`] does not move: `build`'s argument verbatim, an optional field with a
    /// default reads in both directions.
    #[serde(default)]
    pub at: Option<String>,
    /// ⚠⚠⚠⚠⚠ **WHICH STATECHART DOCUMENTS [`at`](Self::at)'s WORD CAME FROM** —
    /// `sprag_plugin::STATECHARTS_FINGERPRINT` as the writing daemon knew it. [`None`] for a run
    /// with no recorded position, or a log written before this field existed.
    ///
    /// # ⚠⚠⚠⚠ Why the pair travels together — register item 544
    ///
    /// Item 544 says the version skew must be **structurally impossible**, and this is that
    /// sentence as data. `reflecting` is not a fact; it is a fact *about a document*. Restart into a
    /// build whose `ai_loop.scxml` changed and the same word can name a different state or none —
    /// and because the restart that motivates persisting a run is usually a document change, the
    /// dangerous reading is the COMMON one. A reader that compares this against its own build's
    /// fingerprint can tell *this position is in my document* from *this position belongs to a
    /// document I do not have*, which is the difference between resuming a run and inventing one.
    ///
    /// ⚠⚠ **NOT [`build`](Self::build), which is present and cannot do this job.** A build stamp
    /// moves when any file in the tree does, so it would call every promotion a document change —
    /// and removing exactly that cost is why item 543 was filed.
    ///
    /// ⚠ Compared for EQUALITY only; it is an identity and never an ordering.
    #[serde(default)]
    pub document: Option<String>,
    /// ⛔⛔⛔⛔⛔ **WHICH CONTEXT CEILING THE RUN RAN UNDER** — register item 856(1b), and the field
    /// that makes that item's fold rate a comparison rather than a number.
    ///
    /// # ⛔ Why it is persisted and not left on the request
    ///
    /// It WAS on the request, and that is the defect: measured 2026-09-04, 0 of 214 finished rows
    /// still carried one, because the restore path drops it. So the ceiling a run obeyed was
    /// knowable exactly while the run was alive and never afterwards — and item 856's measurement
    /// is computed over runs that have ENDED. A moved ceiling and the eighty runs of the baseline
    /// published the same row.
    ///
    /// ⚠⚠ [`None`] is *nobody wrote it down* — a log from before this field, or a plugin with no
    /// such ceiling. **Never a zero**, which is a value the loop's own guards read as unbounded.
    #[serde(default)]
    pub context_ceiling: Option<i64>,
    /// ⛔⛔⛔⛔⛔ **AND HOW FULL ITS SESSION EVER GOT** — register item 894, the LEFT-hand side of
    /// the comparison the field above is the right-hand side of, and the number without which that
    /// one is half a measurement.
    ///
    /// # ⛔ Both sides, or the row says which bound and never which reading
    ///
    /// `ai_loop.scxml` restarts on `context >= context_ceiling`. Item 856's axis is that distance,
    /// and its two experiment arms had to MOVE the ceiling only because the reading was
    /// unobservable — measured 2026-09-05, of 49 answer keys exactly one mentioned context and it
    /// was the bound. With this field the covariate rides every ordinary run and the fold rate is
    /// read without pushing anything.
    ///
    /// ⚠⚠ [`None`] is *nobody wrote it down* — a log from before this field, a plugin with no
    /// session, or a run no pass of which read a positive one. **Never a zero**: the document
    /// seeds `context` at `0` and sends `0` for a record it could not read, so a zero here would
    /// claim a session had read nothing on behalf of a run nobody measured.
    #[serde(default)]
    pub context_high_water: Option<i64>,
    /// 🎯🎯🎯🎯🎯 **AND WHICH OF ITS NUMBERS WERE NOT ITS DOCUMENT'S** — register item 859, and the
    /// word that says an EXPERIMENT is an experiment.
    ///
    /// # ⛔⛔⛔⛔⛔ Why a log without it makes item 856 unmeasurable
    ///
    /// Item 856's measurement is a fold RATE over a population of runs, and its two experiment arms
    /// were launched by moving `context_ceiling` off the document's number. Its own entry states
    /// the consequence: *an experiment nobody is told about does not go unnoticed, it contaminates
    /// the denominator it was run in.* [`crate::plugins::Overridden`] was built so a moved run
    /// would say so — and measured 2026-09-05, **220 of 220 stored rows carry no such word**,
    /// because the answer lived only on the live record and this struct is what reaches the disk.
    /// Arms 214, 215 and 216 had to be told apart from the ordinary runs by a human note.
    ///
    /// ⚠⚠ The WORDS and not the type, for [`PersistedRun`]'s own reason one field over: a durable
    /// format is a host concern, and `Overridden` holds `&'static str`s that only a build which
    /// compiled them can name. [`crate::plugins::Overridden::restored`] resolves them back against
    /// the three authorities that publish them, and refuses a word this build cannot spell.
    ///
    /// ⚠ [`None`] is *nobody answered* — a log from before this field, a run whose plugin has no
    /// document that authors any number, or one restored from a log this build cannot read.
    /// `Some([])` is the affirmative and the healthy launch: **its document set every number it
    /// authored.** The two must not be folded together, which is `Overridden::joined`'s whole rule.
    #[serde(default)]
    pub overridden: Option<Vec<String>>,
    /// ⚠⚠⚠⚠⚠ **THE WHOLE PLACE THE MACHINE WAS IN** — `sprag_plugin::LoopPlace::in_words`, the
    /// active configuration led by the current state, in the document's own names. [`None`] for a
    /// run whose plugin walks no statechart, one that never took a step, or a log written before
    /// this field existed.
    ///
    /// # ⚠⚠⚠⚠ Why [`at`](Self::at) exists beside it and cannot replace it — register item 543
    ///
    /// `at` is ONE word and it is for a PERSON: *was my run mid-turn, or waiting on me?* A machine
    /// cannot be put back with it. `Engine::enter_at` takes the whole active set **and** the
    /// current state, and refuses a current that is not a member of that set — so a record holding
    /// a single word is, structurally, a run that can be reported and never resumed. The two are
    /// not redundant: one answers a question, the other re-enters a document.
    ///
    /// ⚠⚠ **READ ONLY THROUGH [`resumable_place`](Self::resumable_place)**, which compares
    /// [`document`](Self::document) first. A configuration is even more a fact about a document
    /// than a word is: rename one state and the set still decodes, still looks well-formed, and
    /// names a place that is gone.
    ///
    /// ⚠ [`RUN_LOG_VERSION`] does not move — `build`'s argument verbatim, an optional field with a
    /// default reads in both directions.
    #[serde(default)]
    pub place: Option<Vec<String>>,
    /// ⚠⚠⚠⚠ **WHETHER A PERSON HAD STOOD THIS RUN DOWN** — register item 594. [`None`] for a log
    /// written before this field existed, which is NOT the same answer as `Some(false)`: one is
    /// *nobody recorded whether an order was given*, the other is *no order was given*.
    ///
    /// # Why an ORDER is persisted here when [`RunRegistry::restore`] refuses to resurrect one
    ///
    /// See [`EndedRun::stood_down`], which holds the argument: a stand-down on a run that is over
    /// has stopped being an instruction and become the only thing that explains the ending. What
    /// this repository measured on 2026-08-22 is the cost of dropping it — a run ordered to stand
    /// down came back from a daemon restart as a plain `cancelled`, and *"its work is kept"* had no
    /// trace left anywhere to be weighed against.
    ///
    /// ⚠ [`RUN_LOG_VERSION`] does not move: [`build`](Self::build)'s argument verbatim, an optional
    /// field with a default reads in both directions.
    #[serde(default)]
    pub stood_down: Option<bool>,
    /// ⛔⛔⛔⛔⛔ **AND WHERE THAT ORDER CAME FROM** — register item 835, the field above's missing
    /// half, and the one a restore needs MOST rather than least.
    ///
    /// A run is read after it ends, and the daemon that drove it is restarted between rounds
    /// (item 606 measured thirteen live runs, every one restored). **The run another supervisor
    /// meets is therefore always a restored one** — which is exactly the reading item 835 was filed
    /// on: a closing line saying *a person* with no way to learn which person, and a stopped run
    /// re-launched twice.
    ///
    /// ⚠⚠ [`None`] both for a log written before this field existed and for an order nobody wrote
    /// down. The two read alike here and that is honest — neither is a claim about who — and
    /// `crate::plugins::stand_down_sentence` renders both as **nobody wrote it down**, never as *a
    /// person*.
    ///
    /// ⚠ [`RUN_LOG_VERSION`] does not move: an optional field with a default reads both ways.
    #[serde(default)]
    pub stood_down_by: Option<StoodDownBy>,
    /// ⚠⚠⚠⚠⚠ **WHO RAISED THE CANCEL THAT ENDED THIS RUN** — register item 596. [`None`] both for
    /// a log written before this field existed and for a run no cancel touched.
    ///
    /// # Why persisting this is not optional, the way [`stood_down`](Self::stood_down)'s was
    ///
    /// [`Canceller::Shutdown`] is raised by [`RunRegistry::cancel_all`], and every caller of that
    /// is a daemon on its way out — so the process that learns the reason stops existing moments
    /// later. Without this field the value could be *produced* and never *read* by anybody, which
    /// is a value space widened for nobody: the whole distinction item 596 exists to draw would
    /// have been observable only in the seconds between the sweep and the exit.
    ///
    /// ⚠⚠ A restore reads it and hands it to [`EndedRun::restored`], which is the ONLY way this
    /// daemon may answer `Shutdown` about a run it did not end — it is repeating a record, not
    /// deducing a cause.
    ///
    /// ⚠ [`RUN_LOG_VERSION`] does not move: an optional field with a default reads both ways.
    #[serde(default)]
    pub cancelled_by: Option<Canceller>,
    /// ⚠⚠⚠⚠⚠ **WHAT THE RUN PUT INTO ITS PANE, AND HOW MUCH OF IT NOBODY CAN SEE** — register item
    /// 606. [`None`] only for a log written before this field existed.
    ///
    /// # Why this is persisted where a HOLD is refused
    ///
    /// [`RunRegistry::restore`]'s rule turns away an ORDER, because resurrecting an instruction
    /// nobody can act on is a promise to a person that nothing will keep. This is not an
    /// instruction. It is a RECORD of what already happened, and it is the only thing that explains
    /// a pane that looks empty after a run spent thousands of bytes on it.
    ///
    /// ⚠⚠⚠ **MEASURED BEFORE IT WAS ADDED.** Asked of two live daemons on 2026-08-22, thirteen runs
    /// answered and none carried the pair — every one restored, one of them 90 iterations and
    /// 17203 bytes deep. A run is read AFTER it ends, and the daemon that drove it is restarted
    /// between rounds, so the instrument register item 591 built was unreadable on exactly the runs
    /// anybody looks at.
    ///
    /// ⚠ The PAIR or neither, which is why it is one value rather than two numbers: `folded: 3` is
    /// meaningless without the denominator, and two optional fields is a pair somebody writes half
    /// of. [`RUN_LOG_VERSION`] does not move — [`build`](Self::build)'s argument.
    #[serde(default)]
    pub deliveries: Option<PersistedDeliveries>,
    /// ⛔⛔⛔⛔⛔ **THOSE SAME FOLDS, SPLIT BY WHY THE LOOP WAS REFLECTING** — register item 856(1).
    /// See [`PersistedFoldsByReason`] for why a split that died with its daemon would be readable
    /// only while nobody was reading.
    ///
    /// [`None`] for a log written before this field existed, and it must NOT read as *no reflection
    /// of this run folded*: an absent split is one nobody wrote down. What the reader does with
    /// that is refuse the sentence rather than print a clean bill — the split's own `is_empty`, and
    /// register item 762's rule on the field above.
    ///
    /// ⚠ [`RUN_LOG_VERSION`] does not move — [`build`](Self::build)'s argument, and the same call
    /// items 606, 616 and 762 each made: a bump would refuse every run this machine already has, to
    /// gain a value no reader would treat differently from the absence.
    #[serde(default)]
    pub folds_by_reason: Option<PersistedFoldsByReason>,
    /// ⛔⛔⛔⛔⛔ **WHICH ROAD EACH OF THIS RUN'S DELIVERIES ARRIVED ON** — register item 856, and
    /// the only stored value from which a LANDING count can be read. See
    /// [`PersistedDeliveredByRoad`].
    ///
    /// [`None`] for a log written before this field existed, and it must NOT read as *this run
    /// landed nothing*: an absent table is one nobody wrote down. The distinction the field above
    /// makes, one axis over.
    ///
    /// ⚠ [`RUN_LOG_VERSION`] does not move — [`build`](Self::build)'s argument, and the same call
    /// items 606, 616, 762 and 856(1) each made.
    #[serde(default)]
    pub delivered_by_road: Option<PersistedDeliveredByRoad>,
    /// ⛔⛔⛔⛔⛔ **WHICH SENTENCE EACH OF THIS RUN'S PROMPTS WAS, AND HOW MANY OF EACH NEVER BECAME
    /// A QUESTION** — register item 889, and the only stored value from which *which prompt gets
    /// stuck* can be read. See [`PersistedSaidBySentence`].
    ///
    /// [`None`] for a log written before this field existed, and it must NOT read as *every one of
    /// this run's prompts was asked*: an absent table is one nobody wrote down. The distinction the
    /// two fields above make, one axis over.
    ///
    /// ⚠ [`RUN_LOG_VERSION`] does not move — [`build`](Self::build)'s argument, and the same call
    /// items 606, 616, 762, 856(1) and 856 each made.
    #[serde(default)]
    pub said_by_sentence: Option<PersistedSaidBySentence>,
    /// ⛔⛔⛔⛔⛔ **WHAT THE PANE'S WIDTH WOULD HAVE WITHHELD FROM THIS RUN'S REFLECTION ANSWERS** —
    /// register item 866(2). See [`PersistedWidthWithheld`].
    ///
    /// ⚠⚠ **IT IS STORED BECAUSE A LIVE ROW IS NOT A READING** — register item 859, measured on
    /// this store: a value with no column here answers `null` for ever no matter what the running
    /// row said, and item 606 measured that every run anybody reads has been through a restore. A
    /// tally of what the width would have taken is only worth anything read ACROSS runs, so a
    /// column that stopped at the daemon boundary would be the instrument item 866 already has —
    /// none.
    ///
    /// [`None`] for a log written before this field existed, and it must NOT read as *this run's
    /// answers all fitted on one row*: that is the reading a build which went back to the rendered
    /// row would produce, and it is the exact fact this column exists to tell apart from an absent
    /// one.
    ///
    /// ⚠ [`RUN_LOG_VERSION`] does not move — [`build`](Self::build)'s argument, and the same call
    /// items 606, 616, 762, 856(1), 856 and 889 each made.
    #[serde(default)]
    pub width_withheld: Option<PersistedWidthWithheld>,
    /// ⚠⚠⚠⚠⚠ **HOW MUCH OF ITS WORK WAS COMPLETE AND KEPT** — register item 616, the residue item
    /// 604 left. See [`PersistedBanked`] for why this crosses a restart where
    /// [`at`](Self::at) may not.
    ///
    /// [`None`] for a plugin that counts no completed work, and for a log written before this
    /// field existed — the two read alike here and that is correct: neither is a claim that
    /// nothing was banked. [`RUN_LOG_VERSION`] does not move, [`build`](Self::build)'s argument.
    #[serde(default)]
    pub banked: Option<PersistedBanked>,
    /// ⛔⛔⛔⛔⛔ **WHICH ENDING THE RUN CLOSED UNDER** — register item 706's third requirement, and
    /// it has to cross a restart for the reason that item's third cost measured.
    ///
    /// A run's WALK does not survive the daemon that recorded it: read across one restart,
    /// **every run before the boundary held zero walk lines and every run after it kept them**. So
    /// the sentence the word used to live inside is exactly what a restore cannot get back — *how
    /// it ended* survives and *how it got there* does not. A word left out here would therefore be
    /// a word gone for good the moment the daemon is replaced, and item 606 measured how ordinary
    /// that is: thirteen live runs, **every one restored**.
    ///
    /// ⚠⚠ [`None`] is *nobody wrote that down* — a run that named no ending, and a log written
    /// before this field existed — and never *it ended for no reason*. Same distinction the live
    /// [`sprag_plugin::Outcome::done_reason`] keeps, arriving here by the other road.
    ///
    /// ⚠ [`RUN_LOG_VERSION`] does not move, on [`build`](Self::build)'s argument: an optional field
    /// with a default is readable in both directions, and bumping it would throw away every run
    /// record the running daemon holds.
    #[serde(default)]
    pub done_reason: Option<String>,
    /// ⛔⛔⛔⛔⛔ **WHY IT FAILED, AS THE SENTENCE THE DRIVER THAT MET THE FAILURE WROTE** — register
    /// item 903, and the column whose absence made this loop's post-mortems impossible in principle.
    ///
    /// # ⛔⛔⛔⛔⛔ The diagnosis lived in daemon memory and a promotion is a daemon restart
    ///
    /// [`sprag_plugin::Outcome::failure`] is a typed [`sprag_plugin::PaneError`] composed by the
    /// process that met the failure, and [`crate::plugins::outcome_to_json`] publishes its sentence
    /// under `failure`. Nothing carried it here, so it survived exactly as long as the daemon —
    /// **and the loop restarts its daemon to promote a build, which is the moment somebody most
    /// wants to know why the last run died.**
    ///
    /// **Measured 2026-09-05T04:59:20Z over the loop's own store** (a live store: re-running the
    /// count takes a NEW reading rather than checking this one): 228 runs, **78 `failed`**, and of
    /// those `done_reason` 0, `output` 0, `request` 0, `ceiling` 0. What a failed run still carried
    /// was its ending WORD — *that* it failed, never *why*.
    ///
    /// ⚠⚠⚠ **IT IS THE SENTENCE AND NOT THE VARIANT**, and the type says so on the way back:
    /// a restore rebuilds [`sprag_plugin::PaneError::Recorded`], whose whole doc is that this
    /// daemon did not observe what it holds. Parsing a sentence back into a typed cause would be
    /// this process inventing structure the record never had.
    ///
    /// ⚠⚠ **IT IS NOT [`done_reason`](Self::done_reason)** and must not be folded into it. That one
    /// is *the plugin named this ending, out of a closed vocabulary it holds* — its own doc reserves
    /// [`None`] for every run that did NOT end on its own terms, which is every `failed` run there
    /// has ever been. A `failed` run did not close itself; the machine stopped being drivable. Two
    /// facts, two columns.
    ///
    /// ⚠ [`None`] is *no sentence was written down* — a run that did not fail, and a log written
    /// before this column existed. Never *it failed for no reason*.
    ///
    /// ⚠ [`RUN_LOG_VERSION`] does not move, on [`build`](Self::build)'s argument and the same call
    /// items 606, 616, 762, 856, 889 and 866(2) each made.
    #[serde(default)]
    pub failure: Option<String>,
    /// ⛔⛔⛔⛔⛔ **WHY A BLOCKED RUN WAS NEVER ANSWERED, AS THE REFUSAL'S OWN WORD** — register item
    /// 903, and [`failure`](Self::failure)'s sibling for the other ending that survives a restart
    /// saying nothing.
    ///
    /// **Measured 2026-09-05T05:05:23Z over the loop's own store** (a live store: counting again
    /// takes a NEW reading rather than checking this one). Every ending's *why* has a column, and
    /// two of them were empty:
    ///
    /// | ending | runs | its column | carried |
    /// | --- | ---: | --- | ---: |
    /// | `cancelled` | 36 | `cancelled_by` | **36** |
    /// | `exhausted` | 26 | `ceiling` | **26** |
    /// | `converged` | 54 | `done_reason` | 35 |
    /// | `failed` | 78 | *none* | **0** |
    /// | `blocked` | 14 | *none* | **0** |
    ///
    /// ⇒ ⭐ **The debt was never *`done_reason` is attached to one ending*.** Two endings already
    /// answered fully, in their own vocabularies; the hole was exactly these two.
    ///
    /// ⚠⚠⚠ **THE REFUSAL WORD AND NOT THE QUESTION.** `sprag_plugin::consent::Unanswered` carries
    /// both, and only one of them may cross: the question was READ OFF A PANE, and
    /// [`crate::plugins::outcome_from_words`] already refuses to republish it because *a question
    /// re-published from a durable record would be a claim about a screen nobody has looked at
    /// since*. **That argument is kept, and it does not reach the refusal** — `why` comes from a
    /// closed set of eleven this build holds, it is a statement about what THIS host could not do
    /// with what it saw, and it stays true however long ago it was. Same split
    /// [`failure`](Self::failure) draws.
    ///
    /// ⚠ [`None`] is *nobody wrote that down* — a run that did not block, and a log written before
    /// this column existed. Never *it blocked for no reason*.
    ///
    /// ⚠ [`RUN_LOG_VERSION`] does not move, on [`build`](Self::build)'s argument.
    #[serde(default)]
    pub blocked_by: Option<String>,
    /// ⚠⚠⚠⚠⚠ **HOW BIG THE BRIEF IT WAS STARTED WITH IS** — register item 719's second direction,
    /// and it crosses a restart on [`banked`](Self::banked)'s argument, which applies here harder
    /// than anywhere.
    ///
    /// The question this answers — *what was that run handed?* — is asked almost exclusively about
    /// a run that has already ended, and item 719's own diagnosis had to be reconstructed by hand
    /// because nothing on the row said. Item 606's measurement is the general form: thirteen live
    /// runs, **every one restored**. A level that does not survive the daemon is a level nobody
    /// reads.
    ///
    /// ⚠ Three byte counts and nothing else — no document vocabulary, so there is nothing for a
    /// fingerprint to disagree about ([`PersistedBanked`]'s line). [`None`] for a plugin nobody
    /// briefs and for a log written before this field existed; the two read alike and neither is a
    /// claim that a brief was empty. [`RUN_LOG_VERSION`] does not move.
    #[serde(default)]
    pub briefed: Option<PersistedBriefing>,
    /// ⚠⚠⚠⚠⚠ **THE REQUEST THIS RUN WAS ASKED WITH** — the map `crate::plugins::drive_request`
    /// builds a plugin from, so a successor daemon can build the SAME plugin and put it back at
    /// [`place`](Self::place). Register item 543's sixth brick.
    ///
    /// # ⚠⚠⚠⚠⚠ Why a place alone was a record nobody could act on
    ///
    /// Everything else in this file describes what BECAME of a run, and that is all a reader needs
    /// to be told what happened. Resuming is a different question: a machine cannot be put back
    /// into a plugin that does not exist, and this daemon's whole way of making a plugin is
    /// `plugin_from_request` — ONE builder, over a map. Without the map a successor holds a
    /// configuration and nothing to enter it.
    ///
    /// ⚠⚠ **WRITTEN ONLY FOR A RUN THAT COULD ACTUALLY BE PUT BACK**, which is the pair rule
    /// [`resumable_request`](Self::resumable_request) reads at the other end: unfinished, and with
    /// a place recorded beside it. A brief is prose somebody wrote, and keeping it on every
    /// finished `agent` run would put a person's words on disk for the life of a log that could
    /// never use them.
    ///
    /// ⚠ [`RUN_LOG_VERSION`] does not move — [`build`](Self::build)'s argument verbatim, an
    /// optional field with a default reads in both directions, and a log written before this
    /// existed simply describes runs nobody can resume.
    #[serde(default)]
    pub request: Option<serde_json::Map<String, serde_json::Value>>,
    /// ⛔⛔⛔⛔⛔ **WHICH WORKING TREE THIS RUN WAS FOR** — register item 890, and the one column
    /// here that a FINISHED run keeps.
    ///
    /// # ⛔⛔⛔⛔⛔ Written for every run, which is the whole difference from [`request`](Self::request)
    ///
    /// That field is written only for a run that could be put back, and its own doc gives the
    /// reason: a brief is a person's prose and keeping it for the life of a log that can never use
    /// it is the wrong trade. **That argument is right and this field exists because of it** — the
    /// repository is one path, it is the fact every later reading needs, and it is the only part
    /// of the request anybody was ever mining. Measured 2026-09-04: 209 rows, 3 with a request,
    /// **206 that could not say which of this daemon's three repositories they belonged to**.
    ///
    /// ⚠⚠ So the pair rule [`resumable_request`](Self::resumable_request) reads does NOT apply
    /// here: no `!finished` guard, no `place` guard. A run that ended an hour ago is exactly the
    /// run a reader is trying to attribute.
    ///
    /// ⚠ [`RUN_LOG_VERSION`] does not move — [`build`](Self::build)'s argument, and the same call
    /// items 606, 616, 762, 856 and 889 each made. A log written before this existed answers
    /// [`None`], which reads as *nobody recorded which tree* and never as *this run had none*.
    #[serde(default)]
    pub tree: Option<String>,
}

/// ⛔⛔⛔⛔⛔ **WHAT A COUNTER COLUMN IS WORTH WHEN NOBODY WAS COUNTING** — register item 891, and
/// the ONE decision every counter on [`PersistedRun`] is written through.
///
/// # ⛔⛔⛔⛔⛔ The absence this laundered, measured
///
/// Four of this struct's columns are TALLIES — [`deliveries`](PersistedRun::deliveries) and the
/// three splits beside it — and each was written `Some(report.unwrap_or(cell))`, on the argument
/// that *this image looked, so a zero is a claim it may make*. That argument is true of a flag this
/// image can read now ([`stood_down`](PersistedRun::stood_down) is written that way still) and
/// false of a tally, because a tally had to be INCREMENTED WHILE THE RUN RAN. A run this daemon
/// inherited already finished was never incremented by anything here, and its cell holds the zeros
/// `RunRegistry::restore` puts there for a log that had no such column — so the round trip
/// `None` → `…::NONE` → `Some(zeros)` **signed a predecessor's silence as this image's count**.
///
/// Measured 2026-09-05 over the live loop's store, and the store re-serialises every row through
/// the current struct on every save, so the laundering reaches rows from builds that had no such
/// concept at all:
///
/// ```text
/// python3 -c "
/// import json
/// rows=json.load(open('~/.local/share/sprag-loop/state/sprag/sprag-loop.runs.json'))['runs']
/// f=lambda t: sum(x for v in t.values() if isinstance(v,dict) for x in v.values())
/// print(len(rows), sum(1 for r in rows if r.get('folds_by_reason') is not None),
///       sum(1 for r in rows if f(r['folds_by_reason'])>0))"
/// ```
/// ⇒ **220 rows, 220 with a table, 11 with a number** — and row `id 0`, build `52459b9ebf78` from
/// 2026-08-26, carries six rows of four zeros for a concept its build did not have. So *nobody
/// counted* and *counted nothing* were the same shape on 209 rows, and the number register item
/// 856's done-when is judged by — **how many runs have a sample** — read 220 instead of 11.
///
/// # ⚠⚠ Why this is a function and not four `.map(Into::into)` calls
///
/// Item 891's third clause is *every answer key at once, because fixing one key leaves the next one
/// to land in the same place* — and it had already come true once, half a day after it was written,
/// when item 856 added an `ordinary` row that restored as a zero. So the four are written through
/// ONE named decision, and `a_tally_nobody_kept_is_not_a_tally_of_none` reads this module's own
/// source to hold that a fifth tally cannot reach [`PersistedRun`] any other way.
///
/// ⚠ The report is preferred over the cell — register item 663, for the reason each field states:
/// an out-of-process run's cell never moves, so reading the cell first would record `0 of 0` about
/// a run that had filled somebody's pane.
///
/// ⚠⚠ **THE RESIDUE, STATED**: this repairs nothing already written. A row whose stored table is
/// zeros restores as `Some(zeros)` and is re-written as `Some(zeros)` for ever — item 891's own
/// rule that a column's SHAPE is retroactive and its VALUES are not, which
/// [`ended_at`](PersistedRun::ended_at) pays in the same coin for its 154 rows. What this stops is
/// the next 209.
fn counted<Live, Stored>(reported: Option<Live>, cell: Option<Live>) -> Option<Stored>
where
    Stored: From<Live>,
{
    reported.or(cell).map(Into::into)
}

/// ⛔⛔⛔⛔⛔ **A COUNTER A STORED RUN CARRIES** — register item 895, the closed set `counted`
/// writes and [`PersistedRun::sampled`] answers about.
///
/// ⚠ `counted` is SPELLED rather than linked: it is crate-private and this type is public, so an
/// intra-doc link to it is `private_intra_doc_links` under `-D warnings` — register item 365, met
/// again here and refused by the commit hook before this sentence existed.
///
/// ⚠ A closed enum with an [`ALL`](Self::ALL) rather than a list at each reader, this workspace's
/// rule for a vocabulary: a fifth counter added to [`PersistedRun`] and forgotten here is caught by
/// `every_tally_this_record_carries_is_one_a_population_can_be_asked_about`, which derives the
/// record's real counter columns from the TYPE and compares them with this array.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tally {
    /// [`PersistedRun::deliveries`].
    Deliveries,
    /// [`PersistedRun::folds_by_reason`].
    FoldsByReason,
    /// [`PersistedRun::delivered_by_road`].
    DeliveredByRoad,
    /// [`PersistedRun::said_by_sentence`].
    SaidBySentence,
    /// [`PersistedRun::width_withheld`].
    WidthWithheld,
}

impl Tally {
    /// Every counter, in the order [`PersistedRun`] declares them.
    pub const ALL: [Self; 5] = [
        Self::Deliveries,
        Self::FoldsByReason,
        Self::DeliveredByRoad,
        Self::SaidBySentence,
        Self::WidthWithheld,
    ];

    /// **THE KEY IT IS STORED UNDER** — the serde field name, so a reader of the file and a reader
    /// of this type name the same column.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Deliveries => "deliveries",
            Self::FoldsByReason => "folds_by_reason",
            Self::DeliveredByRoad => "delivered_by_road",
            Self::SaidBySentence => "said_by_sentence",
            Self::WidthWithheld => "width_withheld",
        }
    }
}

/// ⛔⛔⛔⛔⛔ **WHETHER A STORED RUN IS IN A POPULATION** — register item 895, and the answer has
/// THREE arms because two of them were one for as long as nobody wrote this down.
///
/// # ⛔⛔⛔⛔⛔ The disease: the predicate was retyped, per reader and per round
///
/// Nothing in this product answered *which runs may a rate be taken over*, so every asker wrote
/// their own filter over the store file. Measured 2026-09-05, and each of these is a real reader:
///
/// * `crate::plugins::run_to_json` tested `made > 0 || unsubmitted > 0 || unreported > 0`, which is
///   [`sprag_plugin::Deliveries::attempted`] re-spelled — the one thing that method's doc forbids.
/// * Register item 856's baseline summed `made + folded`, which adds a sub-count to its container
///   and misses both refusals.
/// * Item 895's own first measurement summed every field.
/// * Item 856's three rounds each picked a DIFFERENT commit to compare a row's `build` against
///   (`4537385`, `21c7811`, `49f8333`) to decide whether the instrument existed yet.
///
/// ⇒ Two counts of the same population differed **8 against 10** across two askers, and both were
/// right about their own predicate. **A number nobody can attach a predicate to is not a
/// measurement**, and that is what this type ends.
///
/// # ⛔⛔⛔ Why the middle arm exists rather than being assigned to one of the other two
///
/// Register item 891 made *nobody counted* say so — as [`None`] on the column — but its own rule is
/// that **a column's SHAPE is retroactive and its VALUES are not**: every row the store already
/// held had been re-serialised with a zeroed table, so for those rows a zero means *counted and
/// found none* AND *never counted* and the row cannot say which. Measured over the live loop's 220
/// rows on 2026-09-05, after item 891 shipped:
///
/// ```text
/// deliveries         counted 205  zeroed  15  unsaid 0
/// folds_by_reason    counted  11  zeroed 209  unsaid 0
/// delivered_by_road  counted  11  zeroed 209  unsaid 0
/// said_by_sentence   counted  11  zeroed 209  unsaid 0
/// ```
///
/// ⇒ **[`Unsaid`](Self::Unsaid) is zero everywhere and will stay zero for every row already
/// written.** Folding [`Zeroed`](Self::Zeroed) into either neighbour would therefore decide 209 of
/// 220 rows by fiat: into `Counted` it claims a sample from a build that may never have had the
/// instrument, and into `Unsaid` it throws away every genuine zero. So it is its own answer, and
/// this workspace's rule 6 applies — an unclassified row is stated, never quietly passed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sampled {
    /// **A NUMBER**, so something was counting and this run is in the population.
    Counted,
    /// **PRESENT AND ALL ZERO.** *Counted nothing* for a row written by a build that had the
    /// counter, and *never counted* for one written before item 891 — and the row cannot say
    /// which. Never pooled with either neighbour; see the type.
    Zeroed,
    /// **ABSENT.** Nobody was counting, said out loud — register item 891, and available only from
    /// that fix onward.
    Unsaid,
}

impl Sampled {
    /// Every answer, so a reader printing a partition cannot leave one out — this workspace's
    /// rule 6, and [`Unsaid`](Self::Unsaid) is the arm that is zero for every row already stored
    /// and would therefore be the one to go missing.
    pub const ALL: [Self; 3] = [Self::Counted, Self::Zeroed, Self::Unsaid];

    /// **THE WORD IT IS REPORTED UNDER**, so a quoted number carries the predicate that produced
    /// it — the whole of register item 895.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Counted => "counted",
            Self::Zeroed => "zeroed",
            Self::Unsaid => "unsaid",
        }
    }
}

/// **THE STORED SHAPE OF [`sprag_plugin::Deliveries`]** — register item 606.
///
/// # ⚠⚠⚠ Why this is not that type with derives on it
///
/// `sprag-plugin` states a **serde-free contract** in its own manifest: it perceives a foreign
/// tool's JSON, and *"the host still owns the RPC-wire mapping"*. A derive over there would move
/// the decision about how sprag's own types are stored into the crate that must not make it — and
/// the crates that copy `ai_loop.scxml` would inherit a dependency for a file they never write.
///
/// ⚠⚠ So the pair crosses as ONE value here too. Two `Option<u32>` would be a pair a later writer
/// can fill half of, which is exactly what [`sprag_plugin::Deliveries`] exists to prevent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PersistedDeliveries {
    /// [`sprag_plugin::Deliveries::made`].
    pub made: u32,
    /// [`sprag_plugin::Deliveries::folded`].
    pub folded: u32,
    /// [`sprag_plugin::Deliveries::unsubmitted`] — register item 617.
    ///
    /// ⚠ `#[serde(default)]` so a run log written before this field READS: an older record simply
    /// has no wedged prompts recorded, which is the honest reading of a file whose writer could not
    /// count them. It is `RUN_LOG_VERSION`'s own rule for an added field, and the same call
    /// register item 616 made one field over — a version bump would refuse every run this machine
    /// already has, to gain nothing.
    #[serde(default)]
    pub unsubmitted: u32,
    /// [`sprag_plugin::Deliveries::unreported`] — register item 762.
    ///
    /// ⚠⚠ `#[serde(default)]` for its neighbour's reason, and the residue is NOT the same one. An
    /// older log's missing `unsubmitted` reads as *no wedged prompts*, which is nearly always true;
    /// a missing `unreported` reads as *no prompt of this run was swallowed*, and the runs whose
    /// records predate this field are exactly the runs that were dying of it. The alternative — a
    /// version bump — would refuse every run this machine already has to gain a `0` nobody would
    /// read differently, so the default stays and the caveat is written down instead of implied.
    #[serde(default)]
    pub unreported: u32,
    /// [`sprag_plugin::Deliveries::released`] — register item 669, and the sub-count that says
    /// WHICH witness closed this run's folds.
    ///
    /// ⚠⚠ `#[serde(default)]` for its neighbours' reason, and the residue is the SHARPEST of the
    /// three: a missing `released` reads as *no fold of this run was settled by its composer*, and
    /// that is precisely what every stored run says — the counter did not exist, so the runs whose
    /// records predate it are the entire population item 669 wanted to measure. The alternative is
    /// still worse (a version bump refuses every run this machine already has), so the default
    /// stays and the ROW's mouth is what keeps the two apart: `crate::plugins::delivery_sentence`
    /// reads the key's PRESENCE, not this field, and says nothing about a row that cannot say.
    #[serde(default)]
    pub released: u32,
}

impl From<sprag_plugin::Deliveries> for PersistedDeliveries {
    fn from(live: sprag_plugin::Deliveries) -> Self {
        Self {
            made: live.made,
            folded: live.folded,
            unsubmitted: live.unsubmitted,
            unreported: live.unreported,
            released: live.released,
        }
    }
}

impl From<PersistedDeliveries> for sprag_plugin::Deliveries {
    fn from(stored: PersistedDeliveries) -> Self {
        Self {
            made: stored.made,
            folded: stored.folded,
            unsubmitted: stored.unsubmitted,
            unreported: stored.unreported,
            released: stored.released,
        }
    }
}

/// **THE STORED SHAPE OF [`sprag_plugin::FoldsByReason`]** — register item 856(1), on
/// [`PersistedDeliveries`]' terms one type up: `sprag-plugin` states a serde-free contract, so the
/// host owns every mapping to a stored shape.
///
/// # ⛔⛔⛔⛔⛔ IT HAS TO CROSS A RESTART OR IT MEASURES NOTHING, AND THAT IS MEASURED
///
/// Item 606 asked two live daemons for their runs' delivery pairs and **thirteen answered with
/// none** — every one restored. Its conclusion is this field's whole reason: *a run is read AFTER
/// it ends, and the daemon that drove it is restarted between rounds.* Item 856's split is read on
/// exactly those runs and by exactly that road, so a split that died with its daemon would be an
/// instrument whose readings are only available while nobody is reading.
///
/// # ⚠⚠⚠ Keyed by the reason's WORD, and why that is safe here where a state name is not
///
/// [`PersistedRun::at`] is never restored into a live cell, because a state name means what a
/// `.scxml` says it means and a foreign document's word is not this build's. A
/// [`sprag_plugin::ReflectReason`] word is the same kind of thing — and it crosses anyway, for
/// [`PersistedBanked`]'s stated reason: the value is a COUNT, and a word this build cannot spell is
/// simply dropped rather than mis-restored. What comes back is *the rows this build has words for*,
/// which is honest in both directions — a row that arrives unspelled was written by a build that
/// knew a reason this one does not, and inventing a home for it would put a count under the wrong
/// axis, which is worse than losing it.
///
/// ⚠ A map rather than an array, because an array is a promise about ORDER that two builds could
/// disagree about silently — the identical failure a positional wire shape has, one surface over.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PersistedFoldsByReason {
    /// One entry per reflect reason WORD, each `[delivered, folded]`.
    ///
    /// ⚠ Rows with no deliveries are written too: *this reason never fired* and *this build had no
    /// word for it* must not read alike, and the only thing that tells them apart is the row being
    /// present and zero.
    #[serde(flatten)]
    pub under: std::collections::BTreeMap<String, PersistedFoldsUnder>,
}

/// One row of [`PersistedFoldsByReason`] — [`sprag_plugin::FoldsUnder`] as the log carries it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PersistedFoldsUnder {
    /// [`sprag_plugin::FoldsUnder::delivered`].
    pub delivered: u32,
    /// [`sprag_plugin::FoldsUnder::folded`].
    pub folded: u32,
    /// ⛔⛔⛔ [`sprag_plugin::Unasked::after_a_fold`] — register item 856(3).
    ///
    /// ⚠ `#[serde(default)]` and that is the OPPOSITE call from the live wire's
    /// (`crate::plugins::folds_by_reason_in` refuses a report missing this key). A log written
    /// before the field existed is a fact from another build and nobody can act on its absence; a
    /// LIVE driver that omits it is a build skew, and answering zeros for that would publish a
    /// comparison over a population this image never saw.
    #[serde(default)]
    pub unasked_after_a_fold: u32,
    /// ⛔⛔⛔ [`sprag_plugin::Unasked::on_the_pane`] — register item 856(3), and the number the
    /// whole item is about: a run that hardened WITHOUT folding is invisible without it, and runs
    /// 194 and 197 are two such runs measured in this repository's own log.
    #[serde(default)]
    pub unasked_on_the_pane: u32,
}

impl From<sprag_plugin::FoldsByReason> for PersistedFoldsByReason {
    fn from(live: sprag_plugin::FoldsByReason) -> Self {
        Self {
            under: live
                .rows()
                .map(|(reason, row)| {
                    (
                        reason.word().to_owned(),
                        PersistedFoldsUnder {
                            delivered: row.delivered,
                            folded: row.folded,
                            unasked_after_a_fold: row.unasked.after_a_fold,
                            unasked_on_the_pane: row.unasked.on_the_pane,
                        },
                    )
                })
                .collect(),
        }
    }
}

impl From<PersistedFoldsByReason> for sprag_plugin::FoldsByReason {
    fn from(stored: PersistedFoldsByReason) -> Self {
        let mut live = Self::NONE;
        for (word, row) in stored.under {
            // ⚠ A word this build has no arm for is DROPPED, which is this type's stated decision:
            // a count restored under the wrong axis is worse than a count lost, and `Occasion` is
            // the only authority on which words there are.
            //
            // ⚠⚠ `Occasion` and not `ReflectReason` since register item 856's widening — the key
            // space is every reason PLUS the ordinary traffic. A log written by a build that has
            // the wider space and restored by one that does not drops the new row, which is this
            // rule working: the identity that row exists for is a claim only a build that counts
            // it can make.
            let Some(occasion) = sprag_plugin::Occasion::named(&word) else {
                continue;
            };
            live.restore(
                occasion,
                sprag_plugin::FoldsUnder {
                    delivered: row.delivered,
                    folded: row.folded,
                    // ⛔⛔⛔ REGISTER ITEM 856(3). This is the crossing that decides whether the
                    // item is paid at all: item 606 measured thirteen live runs and every one was
                    // RESTORED, so the split a person reads is always one that came out of this
                    // file. A hardening that died with its daemon would leave the instrument
                    // exactly as blind as it was.
                    unasked: sprag_plugin::Unasked {
                        after_a_fold: row.unasked_after_a_fold,
                        on_the_pane: row.unasked_on_the_pane,
                    },
                },
            );
        }
        live
    }
}

/// **THE STORED SHAPE OF [`sprag_plugin::DeliveredByRoad`]** — register item 856, on
/// [`PersistedFoldsByReason`]'s terms exactly: `sprag-plugin` states a serde-free contract, so the
/// host owns every mapping to a stored shape.
///
/// # ⛔⛔⛔⛔⛔ IT HAS TO CROSS A RESTART OR IT MEASURES NOTHING, AND THAT IS MEASURED
///
/// Item 606 asked two live daemons for their runs' delivery pairs and **thirteen answered with
/// none** — every one restored. A landing count is read AFTER a run ends, off a daemon that has
/// been restarted since, so a table that died with its daemon would be an instrument whose readings
/// are only available while nobody is reading. That is the same sentence item 856(1) wrote for the
/// reason split, and it binds harder here: the reason split has a live rival in the walk, and the
/// landing count has none at all — the walk publishes a delivery's road as a CHANGE, so a run that
/// lands thirty prompts in a row says so once.
///
/// ⚠ A map keyed by the road's WORD rather than an array, [`PersistedFoldsByReason`]'s call: an
/// array is a promise about ORDER that two builds could disagree about in silence. A word this
/// build cannot spell is DROPPED — a count restored under the wrong road is worse than one lost.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PersistedDeliveredByRoad {
    /// One entry per road WORD, each a count.
    ///
    /// ⚠ Roads with no deliveries are written too: *nothing arrived this way* and *this build had
    /// no word for it* must not read alike, and the only thing that tells them apart is the row
    /// being present and zero. Two of the seven roads had no observed member at all when this was
    /// written, and they are the ones a surprise arrives on.
    #[serde(flatten)]
    pub on: std::collections::BTreeMap<String, u32>,
}

impl From<sprag_plugin::DeliveredByRoad> for PersistedDeliveredByRoad {
    fn from(live: sprag_plugin::DeliveredByRoad) -> Self {
        Self {
            on: live
                .rows()
                .map(|(road, count)| (road.word().to_owned(), count))
                .collect(),
        }
    }
}

impl From<PersistedDeliveredByRoad> for sprag_plugin::DeliveredByRoad {
    fn from(stored: PersistedDeliveredByRoad) -> Self {
        let mut live = Self::NONE;
        for (word, count) in stored.on {
            // ⚠ A word this build has no arm for is DROPPED, this type's stated decision:
            // `Witnessed::ALL` is the only authority on which roads there are, and a count restored
            // under the wrong road is worse than a count lost.
            let Some(road) = sprag_plugin::Witnessed::named(&word) else {
                continue;
            };
            live.restore(road, count);
        }
        live
    }
}

/// **THE STORED SHAPE OF [`sprag_plugin::SaidBySentence`]** — register item 889, on
/// [`PersistedFoldsByReason`]'s terms exactly: `sprag-plugin` states a serde-free contract, so the
/// host owns every mapping to a stored shape.
///
/// # ⛔⛔⛔⛔⛔ IT HAS TO CROSS A RESTART OR IT MEASURES NOTHING, AND THAT IS MEASURED
///
/// Item 606 asked two live daemons for their runs' delivery pairs and **thirteen answered with
/// none** — every one restored. This table's whole purpose is a rate compared ACROSS RUNS, which is
/// a reading taken off finished runs off a daemon that has been restarted since, so a split that
/// died with its daemon would be an instrument whose readings are only available while nobody is
/// reading. The 15× item 889 is about was measured over 197 run logs, not one.
///
/// ⚠ A map keyed by the sentence's WORD rather than an array, [`PersistedFoldsByReason`]'s call: an
/// array is a promise about ORDER that two builds could disagree about in silence. A word this
/// build cannot spell is DROPPED — a count restored under the wrong sentence is worse than one
/// lost, and it would be worse here than anywhere, because this table's readings ARE the
/// comparison between its rows.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PersistedSaidBySentence {
    /// One entry per sentence WORD.
    ///
    /// ⚠ Sentences with no prompts are written too: *this run never said one* and *this build had
    /// no word for it* must not read alike, and the only thing that tells them apart is the row
    /// being present and zero. Two of the eleven — `handover` and `rule` — are reached only by a
    /// run that spends a ceiling or meets a dialog, so most runs write them as exactly that.
    #[serde(flatten)]
    pub of: std::collections::BTreeMap<String, PersistedSaidUnder>,
}

/// ⛔⛔⛔⛔⛔ **WHAT THE WIDTH WOULD HAVE WITHHELD, AS THE LOG CARRIES IT** — register item 866(2),
/// [`sprag_plugin::WidthWithheld`] in stored form.
///
/// ⚠ THREE COUNTS AND NEVER ONE, this type's whole shape: `wider` without `adopted` is a numerator
/// over a population nobody stored, and `withheld` without either is a size nobody can place. A
/// reader restoring half of it would be publishing a rate this run never reported.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PersistedWidthWithheld {
    /// [`sprag_plugin::WidthWithheld::adopted`] — the denominator.
    pub adopted: u64,
    /// [`sprag_plugin::WidthWithheld::wider`] — how many ran past the first rendered row.
    pub wider: u64,
    /// [`sprag_plugin::WidthWithheld::withheld`] — the cells a width-read would have thrown away.
    pub withheld: u64,
}

impl From<sprag_plugin::WidthWithheld> for PersistedWidthWithheld {
    fn from(withheld: sprag_plugin::WidthWithheld) -> Self {
        Self {
            adopted: withheld.adopted,
            wider: withheld.wider,
            withheld: withheld.withheld,
        }
    }
}

impl From<PersistedWidthWithheld> for sprag_plugin::WidthWithheld {
    fn from(withheld: PersistedWidthWithheld) -> Self {
        Self {
            adopted: withheld.adopted,
            wider: withheld.wider,
            withheld: withheld.withheld,
        }
    }
}

/// One row of [`PersistedSaidBySentence`] — [`sprag_plugin::SaidUnder`] as the log carries it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PersistedSaidUnder {
    /// [`sprag_plugin::SaidUnder::sent`] — the denominator, and never absent from a row that has a
    /// numerator: see that field, and `sprag_plugin::Deliveries::attempted` for why a refusal is
    /// inside it.
    pub sent: u32,
    /// [`sprag_plugin::Unasked::after_a_fold`] — the composer swallowed the paste and the peer
    /// never named the question.
    ///
    /// ⚠ `#[serde(default)]` on [`PersistedFoldsUnder`]'s call, and the OPPOSITE of the live wire's
    /// (`crate::plugins::said_by_sentence_in` refuses a report missing a key). A log written before
    /// the field existed is a fact from another build; a LIVE driver that omits it is a build skew.
    #[serde(default)]
    pub unasked_after_a_fold: u32,
    /// [`sprag_plugin::Unasked::on_the_pane`] — it hardened with no fold at all, which is the road
    /// every observed `prompt.unasked` in this repository's log has taken.
    #[serde(default)]
    pub unasked_on_the_pane: u32,
}

impl From<sprag_plugin::SaidBySentence> for PersistedSaidBySentence {
    fn from(live: sprag_plugin::SaidBySentence) -> Self {
        Self {
            of: live
                .rows()
                .map(|(sentence, row)| {
                    (
                        sentence.named().to_owned(),
                        PersistedSaidUnder {
                            sent: row.sent,
                            unasked_after_a_fold: row.unasked.after_a_fold,
                            unasked_on_the_pane: row.unasked.on_the_pane,
                        },
                    )
                })
                .collect(),
        }
    }
}

impl From<PersistedSaidBySentence> for sprag_plugin::SaidBySentence {
    fn from(stored: PersistedSaidBySentence) -> Self {
        let mut live = Self::NONE;
        for (word, row) in stored.of {
            // ⚠ A word this build has no arm for is DROPPED, this type's stated decision:
            // `Sentence::ALL` is the only authority on which sentences there are, and a count
            // restored under the wrong sentence is worse than a count lost.
            let Some(sentence) = sprag_plugin::Sentence::of(&word) else {
                continue;
            };
            live.restore(
                sentence,
                sprag_plugin::SaidUnder {
                    sent: row.sent,
                    unasked: sprag_plugin::Unasked {
                        after_a_fold: row.unasked_after_a_fold,
                        on_the_pane: row.unasked_on_the_pane,
                    },
                },
            );
        }
        live
    }
}

/// [`sprag_plugin::Banked`] as the run log carries it — register item 616.
///
/// # ⚠⚠⚠⚠⚠ Why the answer travels where a POSITION does not
///
/// [`PersistedRun::at`] is a state name, and its meaning lives in a `.scxml`: the saved word and
/// this build's vocabulary are only the same fact when the fingerprints agree, which is why that
/// one is never restored into a live cell. **This is a count and a plain noun.** Three completed
/// turns are three completed turns whatever the document said, so it crosses a restart with
/// nothing to check it against — the two decisions look alike and are not.
///
/// ⚠⚠ **THE PAIR CROSSES AS ONE VALUE**, [`PersistedDeliveries`]' argument verbatim: two options a
/// later writer can fill half of would give a reader a count with no noun, which is the shape
/// `Banked` exists to prevent.
///
/// ⚠ Its own type rather than `sprag_plugin::Banked` directly, for that neighbour's reason: the
/// plugin crate is `serde`-free, and the crates that copy `ai_loop.scxml` would inherit a
/// dependency for a file they never write.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PersistedBanked {
    /// [`sprag_plugin::Banked::completed`].
    pub completed: u32,
    /// [`sprag_plugin::Banked::unit`], owned — the log has no `'static` to borrow from.
    pub unit: String,
}

impl From<sprag_plugin::Banked> for PersistedBanked {
    fn from(live: sprag_plugin::Banked) -> Self {
        Self {
            completed: live.completed,
            unit: live.unit.into_owned(),
        }
    }
}

impl From<PersistedBanked> for sprag_plugin::Banked {
    fn from(stored: PersistedBanked) -> Self {
        Self {
            completed: stored.completed,
            unit: std::borrow::Cow::Owned(stored.unit),
        }
    }
}

/// [`sprag_plugin::Briefing`] as the run log carries it — register item 719's second direction.
///
/// ⚠⚠ **THE FOUR CROSS AS ONE VALUE**, [`PersistedBanked`]'s argument verbatim: the sentence a
/// reader is shown names the LARGEST part, which is a comparison — so a later writer that filled in
/// three of four would produce a sentence pointing at the wrong one, confidently. ⛔ That is not a
/// hypothetical here: for a whole round this struct carried THREE while the prompt composed four,
/// and the missing one was the loop kind's 1,195-byte rules block (register item 762).
///
/// ⚠ Its own type rather than `sprag_plugin::Briefing` directly, for that neighbour's reason: the
/// plugin crate is `serde`-free.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PersistedBriefing {
    /// [`sprag_plugin::Briefing::north_star`], in bytes.
    pub north_star: usize,
    /// [`sprag_plugin::Briefing::milestone`], in bytes.
    pub milestone: usize,
    /// [`sprag_plugin::Briefing::reference`], in bytes.
    pub reference: usize,
    /// [`sprag_plugin::Briefing::working_rules`], in bytes.
    ///
    /// ⚠ `serde(default)` for the same reason every other later field here has one: a run log
    /// written before register item 762 carries no such key, and refusing to read it would make an
    /// old record unreadable rather than incomplete. **The caveat that buys**: those rows read `0`,
    /// which is *this build cannot say what its kind held* rather than *the kind held nothing* —
    /// and they are exactly the rows whose brief was under-reported when they were written.
    #[serde(default)]
    pub working_rules: usize,
}

impl From<sprag_plugin::Briefing> for PersistedBriefing {
    fn from(live: sprag_plugin::Briefing) -> Self {
        Self {
            north_star: live.north_star,
            milestone: live.milestone,
            reference: live.reference,
            working_rules: live.working_rules,
        }
    }
}

impl From<PersistedBriefing> for sprag_plugin::Briefing {
    fn from(stored: PersistedBriefing) -> Self {
        Self {
            north_star: stored.north_star,
            milestone: stored.milestone,
            reference: stored.reference,
            working_rules: stored.working_rules,
        }
    }
}

impl PersistedRun {
    /// ⛔⛔⛔⛔⛔ **WHETHER THIS ROW IS IN `tally`'S POPULATION** — register item 895, and the ONE
    /// place that question is answered.
    ///
    /// See [`Sampled`] for the three answers and for the four disagreeing predicates this
    /// replaces. Asked of the STORED row rather than of a live cell on purpose: a rate over a
    /// tally is taken across runs that have ENDED, which is the population item 606 measured is
    /// always read out of a file after the daemon that drove it is gone.
    ///
    /// ⚠ Every arm goes through the tables' own `is_empty`, never through a sum written here —
    /// [`sprag_plugin::Deliveries::attempted`]'s rule, and item 895's headline is what happens
    /// when a reader re-spells it.
    #[must_use]
    pub fn sampled(&self, tally: Tally) -> Sampled {
        let empty = match tally {
            Tally::Deliveries => self
                .deliveries
                .map(|it| sprag_plugin::Deliveries::from(it).is_empty()),
            Tally::FoldsByReason => self
                .folds_by_reason
                .clone()
                .map(|it| sprag_plugin::FoldsByReason::from(it).is_empty()),
            Tally::DeliveredByRoad => self
                .delivered_by_road
                .clone()
                .map(|it| sprag_plugin::DeliveredByRoad::from(it).is_empty()),
            Tally::SaidBySentence => self
                .said_by_sentence
                .clone()
                .map(|it| sprag_plugin::SaidBySentence::from(it).is_empty()),
            // ⚠⚠ EMPTY IS *ADOPTED NOTHING*, never *nothing was withheld* — see
            // `sprag_plugin::WidthWithheld::is_empty`. A run whose answers all fitted on one row
            // is COUNTED, because that reading is the alarm.
            Tally::WidthWithheld => self
                .width_withheld
                .map(|it| sprag_plugin::WidthWithheld::from(it).is_empty()),
        };
        match empty {
            None => Sampled::Unsaid,
            Some(true) => Sampled::Zeroed,
            Some(false) => Sampled::Counted,
        }
    }

    /// ⚠⚠⚠⚠⚠ **WHERE THIS RUN STOPPED, IF THAT WORD STILL MEANS ANYTHING HERE** — the recorded
    /// position, but only when it came from the documents THIS build compiled.
    ///
    /// # Why the comparison lives here and not at each reader — register items 543 and 544
    ///
    /// [`at`](Self::at) and [`document`](Self::document) are two halves of one fact, and a reader
    /// handed both must remember to check the second before believing the first. That is a rule
    /// prose can state and nothing can enforce, and this file already carries what happens then:
    /// three flags whose comments explained that nothing read them. **One place decides, and what
    /// it hands back is either a word a reader may trust or nothing at all** — so a caller cannot
    /// hold a position it has not earned the right to read.
    ///
    /// ⚠⚠⚠ **A DIFFERENT FINGERPRINT IS NOT AN ERROR, IT IS A DIFFERENT RUN.** Item 544's answer
    /// to version skew is structural rather than defensive: nothing migrates a configuration
    /// between documents, nothing guesses which state the old word corresponds to, and a changed
    /// document therefore ends a run rather than resuming a fiction of it. `None` here is that
    /// decision, taken by construction.
    ///
    /// ⚠⚠ **IT IS NOT A CLAIM THAT THE RUN CAN BE RE-ENTERED**, and the difference is measured
    /// rather than hedged: SCE at the pinned rev exposes `get_active_states` and no way to ENTER at
    /// one, so nothing anywhere can resume a machine today (checked at `e0fdd46` and against a
    /// newer local clone; the C++ core's "restore" is SCXML history states, a different thing).
    /// This answers the question that IS answerable — *is this position readable in my
    /// vocabulary?* — which is what a person asking *where did my run stop* needs, and what the
    /// re-entry will need first when it exists.
    #[must_use]
    pub fn resumable_here(&self) -> Option<&str> {
        // ⚠ BOTH must be present. A position with no document is a word from an unknown
        // vocabulary — older logs carry exactly that — and treating it as local would be the skew
        // this pair exists to prevent, arrived at by an absence instead of by a mismatch.
        let at = self.at.as_deref()?;
        let document = self.document.as_deref()?;
        (document == sprag_plugin::STATECHARTS_FINGERPRINT).then_some(at)
    }

    /// ⚠⚠⚠⚠⚠ **THE PLACE THIS RUN'S MACHINE WAS IN, IF IT IS A PLACE IN *THESE* DOCUMENTS** —
    /// [`resumable_here`](Self::resumable_here)'s door, for the thing an engine can actually be
    /// re-entered at. Register item 543.
    ///
    /// # Why it is its own reader and not a second field somebody must remember to check
    ///
    /// Same argument as the word beside it, one step sharper: a configuration read against a
    /// document it did not come from **decodes cleanly and is wrong**. Nothing here migrates a
    /// configuration between documents and nothing guesses; a changed document simply yields no
    /// place, which is item 544's *a changed document is a NEW run* taken by construction rather
    /// than by a rule a caller has to remember.
    ///
    /// ⚠⚠ **AN EMPTY LIST IS NOT A PLACE** and is refused here rather than handed on: `enter_at`
    /// would take it as a configuration with no members, and the first thing it would reject is
    /// the current state's membership — an engine's error where the record is what was wrong.
    ///
    /// ⚠ Whether the machine can then be ENTERED at it is `sprag_plugin::OuterLoop::resume_at`'s
    /// answer, not this one. This says only *these words are in my vocabulary*.
    #[must_use]
    pub fn resumable_place(&self) -> Option<&[String]> {
        // ⚠ THE REASON IS DISCARDED HERE AND NOWHERE ELSE — see `read_place`, which is the one
        // reading. A caller that wants it asks `withheld`.
        self.read_place().ok()
    }

    /// **THE ONE READING OF A RECORDED PLACE**, which either hands back words this image can spell
    /// or says why it cannot — register item 737, and the authority both
    /// [`resumable_place`](Self::resumable_place) and [`withheld`](Self::withheld) are written from.
    ///
    /// # ⚠⚠⚠⚠ Why the refusal is a value and not a second function's re-derivation
    ///
    /// The reason a place is refused and the refusal itself are the same decision, and a second
    /// reader that computed the reason separately would be free to drift from the door — it would
    /// say *another build's documents* about a run the door had dropped for having no place at all,
    /// which is a report that sends somebody to look at the wrong thing. One `match`, and the
    /// caller that wants a `bool` gets it by discarding the reason rather than by asking again.
    fn read_place(&self) -> Result<&[String], Withheld> {
        // ⚠ AN EMPTY LIST IS NOT A PLACE — `resumable_place`'s own rule, kept here where the
        // reading happens. It is `NoPlace` rather than an arm of its own because a caller cannot
        // act differently on the two: neither is a position, and neither is anybody's fault.
        let Some(place) = self.place.as_deref().filter(|words| !words.is_empty()) else {
            return Err(Withheld::NoPlace);
        };
        let Some(document) = self.document.as_deref() else {
            return Err(Withheld::NoDocument);
        };
        if document != sprag_plugin::STATECHARTS_FINGERPRINT {
            return Err(Withheld::ForeignDocuments {
                theirs: document.to_owned(),
            });
        }
        Ok(place)
    }

    /// ⚠⚠⚠⚠⚠ **WHY THIS RUN IS NOT COMING BACK, FOR A RUN A READER EXPECTS BACK** — register item
    /// 737, and [`None`] exactly when [`resumable_request`](Self::resumable_request) answers with
    /// something.
    ///
    /// # ⚠⚠⚠ The population is UNFINISHED runs, and that is a claim rather than an exemption
    ///
    /// A run whose ending was recorded is over: its row already says what became of it, and *it is
    /// not coming back* about a converged run is noise that would bury the one line that matters.
    /// The runs this answers about are the ones a person left RUNNING and expects to find running —
    /// which is the whole population [`RunRegistry::inheritance`] walks, since an unfinished run
    /// restores as [`RunState::Interrupted`] and a finished one never does.
    ///
    /// ⚠ It is read at RESTORE time and kept, because the log is the only place the answer exists:
    /// a restored record deliberately carries neither the foreign place nor the fingerprint that
    /// refused it (see [`RunRegistry::restore`]), so a later reader could not re-derive this.
    #[must_use]
    pub fn withheld(&self) -> Option<Withheld> {
        if self.finished {
            return None;
        }
        match self.read_place() {
            Err(why) => Some(why),
            // ⚠ THE SECOND HALF, on `resumable_request`'s own rule: a place with nothing to rebuild
            // the plugin from is a configuration nothing can be entered into, and a run held back
            // for THAT is held back for a reason a reader can act on — the request is what a
            // predecessor failed to write down, and no promotion caused it.
            Ok(_) if self.request.is_none() => Some(Withheld::NoRequest),
            Ok(_) => None,
        }
    }

    /// ⚠⚠⚠⚠⚠ **THE REQUEST TO REBUILD THIS RUN'S PLUGIN FROM, IF THERE IS ANYTHING TO PUT BACK** —
    /// register item 543's sixth brick, and the door [`RunRegistry::inheritance`] reads.
    ///
    /// # Why it is guarded by the PLACE and not merely by its own presence
    ///
    /// The two halves are only useful together, and each alone is a way of being wrong. A request
    /// with no readable place would have a successor build a plugin and start it **from the top**,
    /// which re-fires every `<onentry>` and re-types the loop's opening prompt into somebody's
    /// pane — the exact failure item 543 exists to end. A place with no request is a configuration
    /// nothing can be entered into.
    ///
    /// So one door hands back either a request a caller has earned the right to act on or nothing
    /// at all, on [`resumable_place`](Self::resumable_place)'s own argument: a rule prose states and
    /// nothing enforces is a rule this file has already watched go unread.
    ///
    /// ⚠⚠ **AND A FINISHED RUN IS NOT RESUMABLE, WHATEVER IT CARRIES.** A run whose ending was
    /// recorded is over; putting one back would be this daemon starting work nobody asked for,
    /// under an id whose outcome a reader has already seen.
    #[must_use]
    pub fn resumable_request(&self) -> Option<&serde_json::Map<String, serde_json::Value>> {
        if self.finished {
            return None;
        }
        self.resumable_place()?;
        self.request.as_ref()
    }
}

/// The versioned file a daemon leaves behind for its successor.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RunLog {
    /// The format version — [`RUN_LOG_VERSION`] at write time, checked on load.
    pub version: u32,
    /// Every run the daemon held, in submit order.
    pub runs: Vec<PersistedRun>,
}

impl RunLog {
    /// ⛔⛔⛔⛔⛔ **THE PANES A LOOP WAS STILL TYPING AT WHEN THIS LOG WAS WRITTEN** — register item
    /// 869, and the one question a restore has to answer before it brings an agent back to its own
    /// conversation.
    ///
    /// # ⛔⛔⛔⛔⛔ Why these panes must come back WITHOUT their conversation
    ///
    /// A restore re-runs an allowlisted agent's argv with `--resume <uuid>` of the conversation the
    /// pane was in ([`crate::durability::restore_command`]), which is right for a person returning
    /// to their own terminal. It is wrong for the inner pane of a loop, for three reasons that
    /// compound:
    ///
    /// * Replacing that session is the ONLY move the loop has for shedding context
    ///   (`ai_loop.scxml`'s `restarting`, reached from `context_ceiling`), and a run gets few of
    ///   them. A pane restored full spends one undoing the restore rather than answering a ceiling.
    /// * **Nobody reads the resumed transcript.** A boot that puts a run back primes its peer
    ///   afresh ([`sprag_plugin::Resumed::Boot`]), so the conversation the resume paid for is
    ///   briefed over on its first turn.
    /// * A loop ORPHANS transcripts by design — every session replacement does — so the loss the
    ///   resume exists to prevent is not a loss here.
    ///
    /// **Measured across four promotions and three repositories, exception 0** (2026-09-03 15:1x
    /// and 21:2x, 2026-09-04 08:53 and 16:5x): every inner pane came back carrying `--resume`, and
    /// a supervisor killed and re-opened each one by hand. **Measured against this join on the live
    /// daemon at 2026-09-05T06:48:42Z**: three panes (1007, 1008, 1010) were driven by an
    /// unfinished run and one `claude` pane (933, hand-opened) was not — the exact split those four
    /// promotions had been making by hand.
    ///
    /// ⚠⚠⚠ **UNFINISHED IS THE POPULATION, AND [`PersistedRun::resumable_request`] IS DELIBERATELY
    /// NOT.** A run this daemon cannot put back — the documents moved, the pane is gone — still
    /// leaves a pane a loop was typing at, and the fourth promotion above restored **no run at
    /// all** while every pane still came back resumed. Narrowing to the resumable ones would answer
    /// *no* for exactly the boot that most needs a *yes*.
    ///
    /// ⚠⚠ **IT IS THE PANE'S OWN NUMBER AND NOT THE REQUEST'S** — [`PersistedRun::driving`], which
    /// is where a run that replaced its session says it ended up. The request's `pane` key is a
    /// birth certificate, and a loop three replacements in is nowhere near it.
    ///
    /// ⚠ EMPTY for a log whose runs all finished, for one written before `driving` existed, and for
    /// a daemon with no predecessor — all three mean *no pane here belongs to a loop*, which is the
    /// reading that leaves the restore exactly as it was.
    #[must_use]
    pub fn panes_a_loop_was_driving(&self) -> std::collections::HashSet<PaneId> {
        self.runs
            .iter()
            .filter(|run| !run.finished)
            .filter_map(|run| run.driving.map(PaneId))
            .collect()
    }

    /// 🎯🎯🎯🎯🎯 **HOW LONG EACH WORKING TREE HAD NOTHING DRIVING IT** — register item 872 ⑶, and
    /// the reader [`PersistedRun::ran_from`] was built for and never got.
    ///
    /// # ⛔⛔⛔⛔⛔ What this is the second half of
    ///
    /// Item 827 measured **3 h 49 m** between a loop run dying and the next one being launched, and
    /// it measured it BY HAND, once. Item 872 ⑴⑵ then put on record who is owed the next run off
    /// each ending and which endings a machine may never proceed past; ⑶ is *measure the delay
    /// again, the same way*, and it has stood open through four re-judgements. Item 888 built the
    /// two ends of the interval for it — that field's own doc names this clause as its reason —
    /// and **nothing has ever read them**: `ran_from` and `ran_to` are written by
    /// [`crate::durability`] and consumed by no surface, no row and no hook.
    ///
    /// ⇒ So the clause could only ever have been answered by somebody typing a `python3 -c` at the
    /// store, which is this workspace's rule 10 exactly: a number nothing computes is a number that
    /// gets taken once and then quoted until it is wrong. This is the command instead.
    ///
    /// # ⛔⛔⛔⛔⛔ Why the population is the harder half, and why nothing is dropped
    ///
    /// **The default answer is silence, so an under-counted population fails GREEN**: a run this
    /// cannot pair simply produces no stretch, and a report that printed only its stretches would
    /// say *nothing to see* for a store where every single row is unpairable. **Measured over the
    /// loop's own store at 2026-09-05T07:45:30Z: 228 rows, `ran_from` non-null 0, `ran_to` non-null
    /// 0, `tree` absent from every row** — the live daemon predates all three columns, so today
    /// this answers *0 measured, 228 unmeasured* and names a wall for every row.
    ///
    /// ⛔⛔ **A WALL, not THE wall, and that word was wrong here until 2026-09-05T12:11:19Z.** This
    /// doc claimed the report "NAMES which wall each row is behind"; [`NoWait`] is tried in a
    /// declared order, so it names the FIRST. Re-measured at that moment the live store held 231
    /// rows, all of them `TreeUnknown` and all of them ALSO without the watched stop a stretch
    /// starts from (212 finished with `ran_to` unset, 19 not finished) — **two walls reported as
    /// one**, and a reader could take it as *fill in item 890's column and the number appears*.
    /// [`Waits::left_ends`] is the second axis that says otherwise, and [`LeftEnd`] holds why it
    /// cannot be folded into the first.
    ///
    /// ⚠⚠ **AND THE STORE MOVED TWELVE MINUTES LATER, which is why every number here carries its
    /// moment.** The promotion of 2026-09-05 12:2x UTC put the daemon at `e528943`, and at
    /// **12:42:19Z** this reads *233 run(s) · 231 no working tree · 2 nothing has followed it on
    /// that tree yet · 0 of 233 carry the watched stop — 2 have not ended, 19 left unfinished by a
    /// daemon this log has since replaced, 212 finished with nobody watching*. Two rows left
    /// `TreeUnknown` for the first time, and the 21 that had not ended split 2/19. Re-running this
    /// takes a NEW measurement of a live store; it does not check the old one.
    ///
    /// That is the honest reading of a promotion wall and it is why every run lands in exactly one
    /// bucket: [`Waits::measured`] holds the stretches, [`Waits::unmeasured`] holds a count under
    /// each [`NoWait`], and the two add up to the log's own length. A gate holds that sum, so a
    /// seventh reason cannot be quietly swallowed by an existing one.
    ///
    /// # ⚠⚠⚠ Grouped by TREE, because a gap across repositories is a number that means nothing
    ///
    /// One daemon drives three repositories and their runs interleave by id — measured
    /// 2026-09-05T07:46:04Z, runs 224, 225 and 227 were watching-zenoh, pinion and sprag. Pairing
    /// *the next id* would time one repository's death against another's birth. So the succession
    /// is *within a tree, in id order*, which is the order one daemon issued them in.
    ///
    /// ⚠⚠ [`PersistedRun::document`] is NOT the grouping and cannot be: it is the statechart
    /// fingerprint, and every `ai_loop` run in every repository carries the same one
    /// (`f495708bb94944be` on all of them in the reading above). [`PersistedRun::tree`] — item
    /// 890's column — is the only thing that says WHERE, which is why 872 ⑶ waited on that item too.
    /// ⛔⛔⛔⛔⛔ **WHICH RUNS THIS LOG HAS PROOF THE DAEMON WALKED AWAY FROM** — the ids a LATER
    /// row of a DIFFERENT build has outlived, which is [`LeftEnd::of`]'s second argument and the
    /// only evidence a log carries that nothing is watching a row any more.
    ///
    /// # ⚠⚠⚠⚠ Why the build and not the clock
    ///
    /// *This row has not moved in a long time* is the reading that suggests itself and it is not
    /// evidence: a machine that slept, a loop whose agent is thinking, and a daemon that died two
    /// promotions ago all look identical to a timestamp. A build word is a FACT the daemon wrote
    /// about itself — register item 897 made every image say which one it is — and a different one
    /// appearing later in the same log is the log stating, in its own record, that the daemon
    /// changed. Nothing here is inferred from wall time.
    ///
    /// # ⚠⚠ It UNDER-counts, deliberately, and the direction is the safe one
    ///
    /// A daemon restarted at the SAME build leaves rows this cannot name, so a row it omits may
    /// still be abandoned. The alternative — treating *no later run of mine* as *still alive* —
    /// would name live runs dead, and the report's whole use is telling a reader whether waiting
    /// will help. ⚠ A row whose own `build` is [`None`] (a daemon older than that column) is never
    /// named: there is nothing to compare, and a missing word must not read as a different one.
    #[must_use]
    pub fn daemons_replaced_since(&self) -> std::collections::BTreeSet<u64> {
        let mut replaced = std::collections::BTreeSet::new();
        // ⚠ The rows are in the order the daemon appended them, which is what makes *later* mean
        // anything here — the same order `waits_between_runs` re-sorts by id within a tree.
        for (index, run) in self.runs.iter().enumerate() {
            let Some(build) = run.build.as_deref() else {
                continue;
            };
            if self.runs[index + 1..]
                .iter()
                .filter_map(|later| later.build.as_deref())
                .any(|later| later != build)
            {
                replaced.insert(run.id);
            }
        }
        replaced
    }

    #[must_use]
    pub fn waits_between_runs(&self) -> Waits {
        // Grouped first so succession is asked WITHIN a tree. `BTreeMap` rather than a hash so the
        // report is in a stable order a person can diff between two readings — the whole use of
        // this is comparing today's number against item 827's.
        let mut by_tree: std::collections::BTreeMap<&str, Vec<&PersistedRun>> =
            std::collections::BTreeMap::new();
        let mut unmeasured: std::collections::BTreeMap<NoWait, usize> =
            NoWait::ALL.iter().map(|why| (*why, 0)).collect();
        let mut blame = |why: NoWait| {
            *unmeasured
                .get_mut(&why)
                .expect("NoWait::ALL seeded every arm") += 1;
        };
        // ⛔⛔⛔⛔⛔ WHICH ROWS A LATER DAEMON HAS OUTLIVED, taken ONCE for the whole log and
        // shared by both axes — the two must not each derive it, for the reason the two spellings
        // of *has it a left end* were collapsed below.
        let replaced = self.daemons_replaced_since();
        // ⛔⛔⛔⛔⛔ THE SECOND AXIS, and it is taken over EVERY row before the grouping is asked —
        // that is the whole of why it exists. The loop below stops at `TreeUnknown` for a row with
        // no tree, and at 2026-09-05T12:11:19Z every one of the live store's 231 rows was such a
        // row, so the first axis could say only *no tree*. This one still answers, and its answer
        // there is `Watched 0` — no stretch can start in that log whatever a tree column says.
        let mut left_ends: std::collections::BTreeMap<LeftEnd, usize> =
            LeftEnd::ALL.iter().map(|arm| (*arm, 0)).collect();
        for run in &self.runs {
            *left_ends
                .get_mut(&LeftEnd::of(run, replaced.contains(&run.id)))
                .expect("LeftEnd::ALL seeded every arm") += 1;
        }
        for run in &self.runs {
            match run.tree.as_deref() {
                Some(tree) => by_tree.entry(tree).or_default().push(run),
                // ⚠ FIRST, and it is a precedence rather than an accident: without a tree there is
                // no set for this run to have a successor IN, so no later question can be asked of
                // it. Every other arm below presumes the grouping happened.
                None => blame(NoWait::TreeUnknown),
            }
        }
        let mut measured = Vec::new();
        for (tree, mut runs) in by_tree {
            // The daemon issues ids in submit order, so within one tree this IS the succession.
            runs.sort_by_key(|run| run.id);
            for (index, run) in runs.iter().enumerate() {
                let Some(next) = runs.get(index + 1) else {
                    // ⚠ NOT A DEFECT: the newest run of a tree has nothing after it yet. It is
                    // counted rather than skipped because *this log ends here* and *this log lost
                    // something* must not read alike — the arm exists so the sum can be held.
                    //
                    // ⛔⛔⛔⛔⛔ AND *YET* HAS TO BE EARNED, which is the same split `LeftEnd`
                    // needed on the other axis: a run whose ending opens NO next run is one
                    // nothing will ever follow, and telling a reader to come back is telling them
                    // to wait for ever. The question is put to item 872 ⑴'s own published answer
                    // rather than to a list of words kept here.
                    blame(if !run.finished {
                        NoWait::NothingFollowed
                    } else {
                        match run
                            .outcome
                            .as_deref()
                            .and_then(sprag_plugin::driver::Disposition::of_outcome_word)
                        {
                            // ⛔⛔⛔⛔⛔ AN EXHAUSTIVE MATCH ON THE OPENER AND NOT
                            // `a_next_run_is_owed`, because THREE answers come out of three arms
                            // and a boolean cannot carry them: `nobody` closes the chain, `a
                            // person` is owed one and has not come, and `this run's own opener` is
                            // owed one ONLY WHERE THE LOG NAMES THAT PARTY — its own doc says so.
                            // With no `_` arm a fourth opener has to decide here on the day it
                            // exists rather than defaulting into *come back later*.
                            Some(next) => match next.opens_next() {
                                sprag_plugin::driver::Opener::Nobody => NoWait::SuccessionEnded,
                                sprag_plugin::driver::Opener::APerson => NoWait::NothingFollowed,
                                sprag_plugin::driver::Opener::ThisRunsOpener
                                    if run.opened_by_session.is_none() =>
                                {
                                    NoWait::OpenerUnrecorded
                                }
                                sprag_plugin::driver::Opener::ThisRunsOpener => {
                                    NoWait::NothingFollowed
                                }
                            },
                            // ⚠ Rule 6: unclassified is a RED, not a pass. `of_outcome_word`'s own
                            // doc says so, and reading it as either neighbour invents the answer.
                            None => NoWait::SuccessionUnsaid,
                        }
                    });
                    continue;
                };
                // ⛔⛔⛔⛔⛔ THE TWO AXES MEET HERE, and it is ONE reading rather than two that
                // agree. `LeftEnd` asks whether this run carries a watched stop; so did the three
                // lines this replaces, in their own spelling. Two spellings of *has it a left end*
                // is a place for them to drift, and the drift is invisible in the worst direction:
                // a stretch printed off a stop the second axis calls unwatched, both halves
                // confident. Asked once, the contradiction cannot be built.
                let stopped = match LeftEnd::of(run, replaced.contains(&run.id)) {
                    LeftEnd::NotEndedYet => {
                        blame(NoWait::StillRunning);
                        continue;
                    }
                    LeftEnd::Abandoned => {
                        blame(NoWait::EndAbandoned);
                        continue;
                    }
                    LeftEnd::Unwatched => {
                        blame(NoWait::EndUnwatched);
                        continue;
                    }
                    LeftEnd::Watched => run
                        .ran_to
                        .expect("LeftEnd::Watched is exactly a finished run whose ran_to is set"),
                };
                let Some(started) = next.ran_from else {
                    blame(NoWait::SuccessorStartUnwatched);
                    continue;
                };
                let Some(seconds) = started.checked_sub(stopped) else {
                    // ⛔ NOT SATURATED TO ZERO. Two runs can be on one tree at once, and a `0` here
                    // would read as *the handover was instant* — the strongest possible claim, made
                    // from the weakest possible evidence. It is its own arm and says so.
                    blame(NoWait::SuccessorStartedFirst);
                    continue;
                };
                measured.push(Wait {
                    tree: tree.to_owned(),
                    after: run.id,
                    before: next.id,
                    seconds,
                });
            }
        }
        Waits {
            measured,
            unmeasured: NoWait::ALL
                .iter()
                .map(|why| (*why, unmeasured[why]))
                .collect(),
            left_ends: LeftEnd::ALL
                .iter()
                .map(|arm| (*arm, left_ends[arm]))
                .collect(),
        }
    }

    /// 🎯🎯🎯🎯🎯 **HOW FULL EACH SESSION WAS WHEN IT FOLDED THE PROMPTS IT WAS SENT** — register
    /// item 856 ⑴, and the number five re-judgements have answered *after the promotion* without
    /// once putting a command behind it.
    ///
    /// # ⛔⛔⛔⛔⛔ The clause, and why counting folds alone can never answer it
    ///
    /// Item 856's axis is that a session folds a prompt because of **how full it is**, and the item
    /// states its own refutation: `sprag_plugin`'s outer loop says it in the product's words — *one
    /// capacity reflection whose prompt LANDS*. Measured 2026-09-05 over the loop's own store, that
    /// refutation had arrived **29 times** and every one of them came from a run whose ceiling a
    /// caller had MOVED to `20000`. At a moved ceiling a `capacity` reflection is not *the session
    /// filled up*, it is *we handed over early* — so the condition silently assumed **ceiling =
    /// fullness**, and the 29 say nothing about the axis at all.
    ///
    /// ⇒ Telling those apart needs three columns beside the fold split, and each one is an item:
    /// [`PersistedRun::context_high_water`] (894, how full it actually got),
    /// [`PersistedRun::context_ceiling`] (856 ⑴b, what it was judged by) and
    /// [`PersistedRun::overridden`] (859, whose numbers those were). Until this method there was no
    /// reader of the three together, so the answer was a `python3 -c` typed at the store — this
    /// workspace's rule 10 exactly, and the same absence [`RunLog::waits_between_runs`] was written
    /// for one item over.
    ///
    /// # ⛔⛔⛔⛔⛔ IT COUNTS WHAT IT CANNOT MEASURE, and today that is the whole answer
    ///
    /// A run this cannot read yields no row, so a report of rows alone says *nothing to see* about
    /// a store where nothing is readable — **and that is today's store**: measured
    /// 2026-09-05T10:13:40Z, 229 rows carrying `context_high_water` 0 times, `context_ceiling` 0
    /// times and `overridden` 0 times, because the daemon driving that loop predates all three.
    /// So every run lands in exactly one bucket — a [`FoldAtFullness`] or one [`NoFullness`] — and
    /// [`Folds::runs`] is that sum, which a gate holds. A sixth reason cannot be swallowed by an
    /// existing one, and *the promotion has not happened yet* cannot read as *nothing folded*.
    ///
    /// # ⚠⚠ The population predicate is ASKED, never re-spelled — register item 895
    ///
    /// Whether a row is in the population is [`PersistedRun::sampled`]'s answer and not a filter
    /// written here: item 895 measured four readers of this same store each inventing their own,
    /// with two counts of one population differing 8 against 10. Its middle answer
    /// ([`Sampled::Zeroed`]) is carried as its own arm for that item's reason — folding it into
    /// either neighbour decides 209 of 220 rows by fiat.
    #[must_use]
    pub fn folds_against_fullness(&self) -> Folds {
        let mut unmeasured: std::collections::BTreeMap<NoFullness, usize> =
            NoFullness::ALL.iter().map(|why| (*why, 0)).collect();
        let mut blame = |why: NoFullness| {
            *unmeasured
                .get_mut(&why)
                .expect("NoFullness::ALL seeded every arm") += 1;
        };
        let mut measured = Vec::new();
        // ⛔ THE SIZE OF ITEM 894's WALL, accumulated at the ONE site that decides a row is behind
        // it — a second pass over the log would be a second population, and this report's whole
        // discipline is that its halves are one.
        let mut stranded = 0u32;
        for run in &self.runs {
            // ⚠ FIRST, and it is a precedence rather than an accident: without a split there is no
            // fold to put a fullness beside, so no later question can be asked of this row.
            match run.sampled(Tally::FoldsByReason) {
                Sampled::Unsaid => {
                    blame(NoFullness::SplitUnsaid);
                    continue;
                }
                Sampled::Zeroed => {
                    blame(NoFullness::SplitZeroed);
                    continue;
                }
                Sampled::Counted => {}
            }
            let Some(stored) = run.folds_by_reason.clone() else {
                // ⚠ Unreachable by `sampled`'s own definition — `Unsaid` is exactly the absent
                // table. It BLAMES rather than panics because the one fatal defect of this
                // instrument is a row that leaves the population silently, and `Folds::runs` is
                // what would catch that.
                blame(NoFullness::SplitUnsaid);
                continue;
            };
            // ⚠ Converted BEFORE the axis is asked for, because the questions below it are asked
            // of rows that never reach the axis at all — see `on_the_capacity_road`.
            let split: sprag_plugin::FoldsByReason = stored.into();
            let (Some(fullest), Some(ceiling)) = (run.context_high_water, run.context_ceiling)
            else {
                blame(if run.context_high_water.is_some() {
                    NoFullness::CeilingUnrecorded
                } else if took_the_capacity_road(&split) {
                    // ⛔⛔⛔⛔⛔ THE ROW HOLDS EVIDENCE AND THE AXIS MAY NOT USE IT, and those are
                    // two different silences. Counting it under the arm beside this told a reader
                    // that nothing here had ever walked the road — over a live store holding 29
                    // landings that had.
                    stranded += landed_on_the_capacity_road(&split);
                    NoFullness::CapacityUnjudgeable
                } else {
                    NoFullness::FullnessUnread
                });
                continue;
            };
            // ⛔⛔⛔⛔⛔ AND BEFORE WHOSE NUMBERS THEY WERE, WHETHER THERE IS A BOUND AT ALL. The
            // document guards every edge into the `capacity` road on `context_ceiling > 0` and
            // sends `<= 0` somewhere else entirely, so a zero is NO ceiling rather than a low one
            // — a run carrying it could never take the road this axis is read from. Asked ahead of
            // `Judged` because it holds whoever authored the number: a caller who moved the
            // ceiling TO zero has made the axis just as inapplicable as a document that did.
            if ceiling <= 0 {
                blame(NoFullness::CeilingUnbounded);
                continue;
            }
            let Some(judged) = run.overridden.as_deref().and_then(Judged::of) else {
                blame(NoFullness::ExperimentUnsaid);
                continue;
            };
            measured.push(FoldAtFullness {
                id: run.id,
                fullest,
                ceiling,
                judged,
                folds: split,
            });
        }
        Folds {
            measured,
            unmeasured: NoFullness::ALL
                .iter()
                .map(|why| (*why, unmeasured[why]))
                .collect(),
            stranded,
        }
    }
}

/// 🎯 **WHAT A LOG CAN AND CANNOT SAY ABOUT THE DELAY BETWEEN ITS RUNS** — the whole answer of
/// [`RunLog::waits_between_runs`], register item 872 ⑶.
///
/// ⚠⚠ **THE TWO HALVES ARE ONE POPULATION**: every run in the log is either the left end of exactly
/// one [`measured`](Self::measured) stretch or counted under exactly one
/// [`unmeasured`](Self::unmeasured) reason, and [`Waits::runs`] is that sum. A reader that took the
/// first half alone would read a store where nothing is measurable as a store with no delay in it,
/// which is the reading this item exists to stop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Waits {
    /// Every stretch this log can actually put a number on, tree by tree in name order.
    pub measured: Vec<Wait>,
    /// How many runs could not be a left end, under each [`NoWait`] — every arm, **including the
    /// zeros**, in [`NoWait::ALL`]'s order.
    ///
    /// ⚠ The zeros are carried rather than filtered because the POPULATION is the enum: a reader
    /// deciding what to print may drop an empty line, and a reader deciding whether the table is
    /// whole may not. Item 856 ⑹ measured what a table that builds its own population does.
    pub unmeasured: Vec<(NoWait, usize)>,
    /// ⛔⛔⛔ **THE SECOND AXIS** — how many runs carry the watched stop a stretch starts from,
    /// under each [`LeftEnd`], every arm including the zeros, over **every run in the log**.
    ///
    /// ⚠⚠ This is a SECOND partition of the SAME population, not a subdivision of the first: it is
    /// asked of runs that measured a stretch as well as runs that did not, and it sums to
    /// [`Waits::runs`] on its own. [`LeftEnd`] holds why it cannot be a seventh [`NoWait`] — the
    /// first axis stops at `TreeUnknown` and today's store is entirely made of that row.
    pub left_ends: Vec<(LeftEnd, usize)>,
}

impl Waits {
    /// How many runs this answer accounts for — the sum of both halves, and what a caller checks
    /// against the log's own length.
    #[must_use]
    pub fn runs(&self) -> usize {
        self.measured.len()
            + self
                .unmeasured
                .iter()
                .map(|(_, count)| count)
                .sum::<usize>()
    }

    /// How many runs the second axis accounts for — [`left_ends`](Self::left_ends) summed, which
    /// must equal [`runs`](Self::runs). ⚠ A gate holds the two against each other: an axis that
    /// counted a different population would print a fraction whose denominator is a different
    /// store's, and both numbers would look reasonable.
    #[must_use]
    pub fn left_ends_counted(&self) -> usize {
        self.left_ends.iter().map(|(_, count)| count).sum()
    }

    /// How many runs could be the left end of a stretch **if the grouping said nothing at all** —
    /// [`LeftEnd::Watched`]'s count.
    ///
    /// ⇒ ⭐ **Zero is the decisive reading of register item 872 ⑶b**, and it is the one this store
    /// gives: no stretch can exist in this log whatever any tree column later says, so the number
    /// that clause wants cannot come from these rows and waits on runs a live daemon watches.
    ///
    /// # ⛔⛔⛔⛔⛔ The promotion came, and this is STILL zero — which is the sharper reading
    ///
    /// The daemon driving this loop went from `7181c74` to `e528943` on 2026-09-05 at 12:2x UTC,
    /// and the store answered within two minutes: **at 2026-09-05T12:27:22Z, 233 rows, `tree`
    /// non-null 2, `ran_from` non-null 2, `ran_to` non-null 0** — the first rows this repository
    /// has ever held that a successor could pair with. This number did not move, and the reason it
    /// did not is the one that matters: a stretch starts at a stop, and a run that has just STARTED
    /// has none. ⇒ ⭐⭐⭐⭐⭐ So *the wall is the promotion* was necessary and NOT sufficient, which
    /// five re-judgements of item 872 ⑶ wrote as one word. What is left is the loop's own next
    /// handover on a tree — one run of this build ending, and its successor beginning — and no
    /// round can hurry it.
    #[must_use]
    pub fn watched_left_ends(&self) -> usize {
        self.left_ends
            .iter()
            .find(|(arm, _)| *arm == LeftEnd::Watched)
            .map_or(0, |(_, count)| *count)
    }

    /// The longest stretch, which is the number item 827 reported as **3 h 49 m** — [`None`] when
    /// nothing here is measurable, which is *nobody knows* and never *there was no delay*.
    #[must_use]
    pub fn longest(&self) -> Option<&Wait> {
        self.measured.iter().max_by_key(|wait| wait.seconds)
    }
}

/// One stretch during which a working tree had no run driving it — [`Waits::measured`]'s member.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wait {
    /// The working tree that waited — [`PersistedRun::tree`], and the grouping this is within.
    pub tree: String,
    /// The run whose stop opened the stretch.
    pub after: u64,
    /// The run whose start closed it.
    pub before: u64,
    /// How long it lasted, in seconds — `ran_from` of the second minus `ran_to` of the first, both
    /// of them moments a daemon WATCHED rather than inferred.
    pub seconds: u64,
}

/// ⛔⛔⛔⛔⛔ **WHY A RUN IS NOT THE LEFT END OF A MEASURABLE STRETCH** — [`Waits::unmeasured`]'s
/// population, and the half register item 872 ⑶ cannot be answered without.
///
/// # ⚠⚠⚠⚠⚠ It is a closed vocabulary because the alternative is a silent drop
///
/// A run that cannot be paired produces no stretch, and *no stretch* and *no delay* are the same
/// silence. Naming each way is what lets a reader tell **the promotion wall** (`ran_from` and
/// `ran_to` exist in this build and in no row yet) from **a real handover that was instant** — and
/// today, over the loop's own store, every single row is behind a wall.
///
/// ⚠ The arms are tried in the order they are declared, and that order is a claim: a run with no
/// tree has no set to have a successor in, so nothing further can be asked of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum NoWait {
    /// No [`PersistedRun::tree`] — a row from a daemon older than item 890's column, so nothing
    /// says which repository it belongs to and it has no successor to be found.
    TreeUnknown,
    /// The newest run this log holds for its tree. Nothing has followed it, which is not a loss.
    ///
    /// ⚠ AHEAD of [`StillRunning`](Self::StillRunning), and the pair is why this order is a claim
    /// rather than a listing: the newest run of a tree is usually BOTH, and *the chain ends here*
    /// is what a reader can act on, where *it has not finished* would describe every healthy loop
    /// in the store as a thing that failed to measure.
    NothingFollowed,
    /// **THE NEWEST RUN OF ITS TREE, AND NOTHING WILL EVER FOLLOW IT** — it ended under a
    /// disposition whose [`Opener`](sprag_plugin::driver::Opener) is `nobody`, so the product's own
    /// answer is *nothing opens a next run off this ending*.
    ///
    /// ⛔⛔⛔⛔⛔ **This is not a slower [`NothingFollowed`](Self::NothingFollowed)**, and the two
    /// are the same pair [`LeftEnd::Abandoned`] was split out of on the other axis for the same
    /// reason: *yet* invites *wait, and one will come*, and here nobody is coming. A reader
    /// watching this count fall waits for ever, and a predicate that pooled them has a population
    /// with a role in it that cannot reach zero.
    ///
    /// ⚠ Measured over this store at **2026-09-05T14:34:55Z**: of 212 finished runs, `cancelled`
    /// **36** and `taken_over` **0** answer `nobody` — **17 %** of endings — against 83 whose
    /// ending a machine may open a successor off and 93 that owe one to a person. So the pooled
    /// arm was not a corner: one ending in six is final.
    ///
    /// ⚠⚠ Asked of [`Disposition::of_outcome_word`](sprag_plugin::driver::Disposition), which is
    /// item 872 ⑴'s own published answer, rather than of a list of words kept here — a second list
    /// is the *one value, two homes* defect that item's own doc was written against.
    SuccessionEnded,
    /// **ITS ENDING OWES THE NEXT RUN TO *THIS RUN'S OWN OPENER*, AND THE LOG CARRIES NONE** — so
    /// by that word's own definition nobody is owed anything.
    ///
    /// [`Opener::ThisRunsOpener`](sprag_plugin::driver::Opener::ThisRunsOpener) says it outright:
    /// *"Not `anybody` and not `the daemon`: the party is a recorded fact of the run, and an
    /// ending that answers this with no opener on record has nobody owed it."* The party's column
    /// is [`PersistedRun::opened_by_session`], and a finished run keeps no `request` to re-derive
    /// it from.
    ///
    /// ⛔⛔ **ITS OWN ARM RATHER THAN [`SuccessionEnded`](Self::SuccessionEnded)'s**, on
    /// [`LeftEnd::Abandoned`]'s distinction exactly: that one is a chain a PERSON closed and it
    /// can never reopen; this is a column register item 893 can put back, after which the same
    /// ending owes a successor again. A reader deciding whether to wait needs those apart.
    ///
    /// ⛔⛔⛔⛔⛔ **Measured over the live store at 2026-09-05T15:33:48Z**, on the first row this
    /// repository ever gave item 872 ⑶b a watched stop: run 233 ended `converged` on
    /// `/home/coin/pinion` at 15:16:31Z with `opened_by_session` **null** and no request. The
    /// report called it *nothing has followed it on that tree yet*, and nothing in that row could
    /// say who would make that true.
    ///
    /// ⚠⚠⚠⚠⚠ **AND THE SAME STORE FALSIFIED THE STRONGER READING WITHIN THE HOUR**, which is why
    /// this arm's sentence is about the RECORD and not about the future: at **15:41:13Z** run 234
    /// began on that tree and `waits` measured *1348s*. Somebody opened it — the log simply never
    /// said who could. So *nobody is owed one* would have been a claim about what will happen,
    /// made from a column that is missing, and this workspace had it disproved in twenty-two
    /// minutes. Register item 893's own framing, and the reason it is `ordinary`: the run does not
    /// die, the RECORD does.
    OpenerUnrecorded,
    /// The newest run of its tree has ended and **nothing here can say whether a successor is
    /// owed**: no outcome was recorded, or the word is one this build cannot classify.
    ///
    /// ⛔ Counted rather than read as either neighbour, on this workspace's rule 6: an
    /// unclassified row is a RED and not a pass. Folded upward it would promise a successor that
    /// may never come; folded downward it would declare a finality the row never states.
    SuccessionUnsaid,
    /// It has not ended, so there is no left end yet — a run with a successor already beside it,
    /// which is two runs on one tree at once.
    StillRunning,
    /// It has not ended and it never will: a later row of this log was opened by a different build,
    /// so the daemon that would have watched it stop is gone. See [`LeftEnd::Abandoned`], which
    /// holds the whole argument and the count that forced the split.
    ///
    /// ⚠ IMMEDIATELY AFTER [`StillRunning`](Self::StillRunning) because that is the pair it was
    /// wrongly inside, and the two are separated by a fact about the LOG rather than about the run.
    EndAbandoned,
    /// Finished, but no [`PersistedRun::ran_to`] — nobody was watching when it stopped. Item 888's
    /// own residue: a run inherited already-finished, or one whose daemon died with it.
    EndUnwatched,
    /// A successor exists and carries no [`PersistedRun::ran_from`] — nobody watched it begin, so
    /// the right end of the interval is missing rather than the left.
    SuccessorStartUnwatched,
    /// The next run on this tree began before this one stopped, so there was no stretch with
    /// nothing driving it. ⛔ Counted here rather than reported as a zero: *the handover was
    /// instant* is the strongest claim this data can make and two overlapping runs are the weakest
    /// evidence for it.
    SuccessorStartedFirst,
}

impl NoWait {
    /// Every way, as the population [`Waits::unmeasured`] is built from — an eleventh reason added
    /// to the type appears in every report without anybody widening a list.
    pub const ALL: [Self; 10] = [
        Self::TreeUnknown,
        Self::NothingFollowed,
        Self::SuccessionEnded,
        Self::OpenerUnrecorded,
        Self::SuccessionUnsaid,
        Self::StillRunning,
        Self::EndAbandoned,
        Self::EndUnwatched,
        Self::SuccessorStartUnwatched,
        Self::SuccessorStartedFirst,
    ];

    /// What a reader is looking at, in one clause — ⛔ an exhaustive `match` with no `_` arm, so an
    /// eighth way cannot reach a report wearing a seventh's sentence.
    #[must_use]
    pub fn describe(self) -> &'static str {
        match self {
            Self::TreeUnknown => "no working tree recorded, so nothing can be its successor",
            Self::StillRunning => "still running, so it has no end to measure from",
            Self::EndAbandoned => {
                "left unfinished by a daemon this log has since replaced, so nothing will ever \
                 watch it stop"
            }
            Self::EndUnwatched => "nobody was watching when it stopped",
            Self::NothingFollowed => "nothing has followed it on that tree yet",
            Self::SuccessionEnded => {
                "nothing will ever follow it — its ending opens no next run at all, so this is \
                 where that tree's chain stops"
            }
            Self::OpenerUnrecorded => {
                "its ending owes the next run to whoever opened it and the log names nobody, so \
                 this record cannot say who would open one"
            }
            Self::SuccessionUnsaid => {
                "nothing has followed it and nothing says whether anything is owed to — its \
                 ending was never recorded, or is a word this build cannot classify"
            }
            Self::SuccessorStartUnwatched => "nobody was watching when the next one began",
            Self::SuccessorStartedFirst => "the next one began before it stopped",
        }
    }
}

/// ⛔⛔⛔⛔⛔ **WHETHER A RUN CARRIES THE END A STRETCH IS MEASURED FROM** — the second axis
/// [`Waits`] reports on.
///
/// # ⛔⛔⛔⛔⛔ Why a second axis rather than a seventh [`NoWait`]
///
/// [`NoWait`] is tried in a declared order and [`NoWait::TreeUnknown`] is FIRST, so a run with no
/// tree is blamed there and **nothing further is asked of it**. That precedence is right for the
/// questions below it — they are all about a run's PLACE IN A SET, and a run with no set has no
/// place in one. It is wrong for this question, which is about the run BY ITSELF.
///
/// **Measured over the loop's own store at 2026-09-05T12:11:19Z**: 231 rows, every one of them
/// blamed `TreeUnknown`, and *also* 212 of them finished with `ran_to` unset and 19 not finished at
/// all — **`ran_to` non-null 0 across the whole store**. So each of those rows sits behind TWO
/// walls and the report named one. A reader could not tell *item 890's column is what stands
/// between me and the number* from *nothing in this store can ever yield the number*, and only the
/// second is true: backfilling `tree` on all 231 would still measure nothing, because a stretch
/// starts at a stop somebody WATCHED and not one of them has one.
///
/// ⇒ ⭐ That is register item 872 ⑶b's own question — *so what is the number* — answered as far as
/// it can be answered WITHOUT the promotion: not *the wall is the tree column*, but *the wall is
/// two deep and only runs a live daemon watches lift it*.
///
/// # ⚠⚠ The pair separates what neither half separates alone
///
/// This is the shape item 872 ⑴ already paid for, where `Unattended` and `Opener` each merge a
/// pair the other splits. Here: the set axis merges *watched stop* and *no stop at all* under
/// `TreeUnknown`, and this axis merges *no tree* and *no successor* under [`Watched`](Self::Watched).
/// Neither ordering can be re-arranged into the other, because the columns arrived in different
/// builds — `tree` is item 890's and `ran_to` is item 888's — so a row carrying one and not the
/// other is a row the store can really hold.
///
/// ⚠ The RIGHT end of a stretch is deliberately not here: it belongs to the SUCCESSOR, and the set
/// axis already blames its absence on the predecessor as [`NoWait::SuccessorStartUnwatched`]. A run
/// owns its own stop and nothing else.
///
/// # ⛔⛔⛔⛔⛔ Why this stopped being *asked of the run alone* — the promotion of 2026-09-05
///
/// It was, and the first thing the promoted daemon printed is what took it away. Three arms make
/// *has not finished* one fact, and the phrase it deserves is *not yet*: a reader is told to come
/// back. Read against the live store **at 2026-09-05T12:35:11Z** — 233 rows, 21 not finished —
/// that sentence was true of **2** of them and false of **19**, whose daemons died in twelve
/// separate promotions going back to run 15 and which no daemon will ever watch stop. The report
/// said *21 have not ended* and invited the reading *wait, and they will*.
///
/// ⇒ ⭐⭐⭐⭐⭐ **So the run alone cannot answer it, and no column on the run can**: whether
/// anything is still watching is a fact about the LOG, and [`Abandoned`](Self::Abandoned) is the
/// arm that reads it. That is why [`of`](Self::of) takes a second argument, and the argument is
/// derived rather than guessed — see [`RunLog::daemons_replaced_since`].
///
/// ⇒ 🎯 **And it is register item 872 ⑶b's own subject rather than a neighbour's.** The stretch
/// this file measures is a HANDOVER, and the handover with the longest gap in this store is a
/// PROMOTION — where the outgoing daemon dies mid-run. At the next promotion run 232 (the first
/// row this repository ever held with a `tree` and a `ran_from`) becomes exactly such a row, and
/// without this arm the first axis would call it *still running* for as long as the log exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LeftEnd {
    /// Finished, and [`PersistedRun::ran_to`] says when a daemon watched it stop — the only arm a
    /// [`Waits::measured`] stretch can start from.
    Watched,
    /// It has not finished, and this log holds no proof that the daemon which opened it is gone —
    /// so there is no stop YET for anybody to have watched, and coming back later is the right
    /// advice. ⚠ AHEAD of [`Unwatched`](Self::Unwatched) and for [`NoWait::StillRunning`]'s reason:
    /// *the chain has not ended here* and *the chain ended and nobody saw* are different facts, and
    /// the first must not be reported as the second.
    NotEndedYet,
    /// It has not finished, and a LATER row of this log was opened by a different build — so the
    /// daemon that would have watched it stop has been replaced and never will.
    ///
    /// ⛔⛔ **This is not a slower [`NotEndedYet`](Self::NotEndedYet)**, which is the whole reason
    /// it is its own arm: `NotEndedYet` can become [`Watched`](Self::Watched) and this can never.
    /// A predicate that counts them together has a population with a role in it that cannot reach
    /// zero, and a reader waiting for that count to fall waits for ever.
    Abandoned,
    /// Finished, and nothing recorded when — item 888's own residue: a run inherited already
    /// finished, or one whose daemon died along with it.
    Unwatched,
}

impl LeftEnd {
    /// Every arm, which is the population [`Waits::left_ends`] is built from — a fifth appears in
    /// every report without anybody widening a list.
    pub const ALL: [Self; 4] = [
        Self::Watched,
        Self::NotEndedYet,
        Self::Abandoned,
        Self::Unwatched,
    ];

    /// Which one this run is — ⛔ an exhaustive `match` with no `_` arm, and the ordering is
    /// [`RunLog::waits_between_runs`]'s own: unfinished is asked before the stamp, so the two axes
    /// cannot come to disagree about a row that is both.
    ///
    /// `replaced` is *this log has since recorded a run from a different build*, which is the only
    /// evidence a log carries that the daemon which opened this row is gone. It comes from
    /// [`RunLog::daemons_replaced_since`] and is **never** inferred from a clock: a row can be
    /// stale because the machine slept.
    #[must_use]
    pub fn of(run: &PersistedRun, replaced: bool) -> Self {
        match (run.finished, replaced, run.ran_to) {
            (false, false, _) => Self::NotEndedYet,
            (false, true, _) => Self::Abandoned,
            (true, _, Some(_)) => Self::Watched,
            (true, _, None) => Self::Unwatched,
        }
    }

    /// What a reader is looking at, in one clause — ⛔ exhaustive, so a fifth arm cannot reach a
    /// report wearing a fourth's sentence.
    ///
    /// ⚠ Each is a PREDICATE about runs, so `{count} run(s) {describe}` reads as a sentence. Every
    /// arm has to fit that one slot: written with their own subjects, the first reading of the real
    /// store printed *212 they finished with nobody watching*.
    #[must_use]
    pub fn describe(self) -> &'static str {
        match self {
            Self::Watched => "carry the stop a daemon watched",
            Self::NotEndedYet => "have not ended",
            Self::Abandoned => "were left unfinished by a daemon this log has since replaced",
            Self::Unwatched => "finished with nobody watching",
        }
    }
}

/// 🎯 **WHAT A LOG CAN AND CANNOT SAY ABOUT ITEM 856's AXIS** — the whole answer of
/// [`RunLog::folds_against_fullness`].
///
/// ⚠⚠ **THE TWO HALVES ARE ONE POPULATION**: every run in the log is either exactly one
/// [`measured`](Self::measured) row or counted under exactly one
/// [`unmeasured`](Self::unmeasured) reason, and [`Folds::runs`] is that sum. A reader that took
/// the first half alone would read a store where nothing is readable as a store where nothing
/// folded — which is what five re-judgements of that item each had to say by hand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Folds {
    /// Every run this log can put a fullness beside, in the order the log holds them.
    pub measured: Vec<FoldAtFullness>,
    /// How many runs could not be read, under each [`NoFullness`] — every arm, **including the
    /// zeros**, in [`NoFullness::ALL`]'s order.
    ///
    /// ⚠ The zeros are carried rather than filtered because the POPULATION is the enum: a reader
    /// deciding what to print may drop an empty line, and a reader deciding whether the table is
    /// whole may not. Item 856 ⑹ measured what a table that builds its own population does.
    pub unmeasured: Vec<(NoFullness, usize)>,
    /// ⛔⛔⛔⛔⛔ **THE `capacity` LANDINGS THIS ANSWER HOLDS AND MAY NOT JUDGE** — how big item
    /// 894's wall is, summed over the rows counted under [`NoFullness::CapacityUnjudgeable`].
    ///
    /// ⛔⛔ **NEVER ADDED TO EITHER COUNT ABOVE, AND NOT A THIRD REFUTATION.** A row that does not
    /// say which ceiling it reflected on cannot be told from an experiment's, which is the whole
    /// reason it sits in the unmeasured half; [`refutations`](Self::refutations)' own note says
    /// what pooling those costs.
    ///
    /// ⇒ ⭐ It is published for the OPPOSITE reason: so a reader of a report whose measurable half
    /// is empty is not left to conclude that nothing here ever walked that road, when the log in
    /// front of them holds 29 prompts that did.
    pub stranded: u32,
}

impl Folds {
    /// How many runs this answer accounts for — the sum of both halves, and what a caller checks
    /// against the log's own length.
    #[must_use]
    pub fn runs(&self) -> usize {
        self.measured.len()
            + self
                .unmeasured
                .iter()
                .map(|(_, count)| count)
                .sum::<usize>()
    }

    /// ⛔⛔⛔⛔⛔ **THE AXIS'S OWN STATED REFUTATION, COUNTED** — a prompt asked during a
    /// `capacity` reflection that LANDED, in a run judged by the ceiling its own document
    /// authored.
    ///
    /// The loop's own source states the condition, in the product's words: *item 856's axis says a
    /// full session folds, its discriminator is the `capacity` reflection, and one capacity
    /// reflection whose prompt LANDS is the register's own stated refutation.*
    ///
    /// ⚠⚠ **[`landings_at_a_moved_ceiling`](Self::landings_at_a_moved_ceiling) IS NEVER ADDED TO
    /// THIS**, and keeping them apart is why this is a method rather than a filter at each reader.
    /// Measured 2026-09-05, the store held 29 such landings and every one belonged to a run whose
    /// ceiling a caller had moved to `20000` — where a `capacity` reflection means *we handed over
    /// early* and not *the session filled up*. Pooling them would refute the axis with the
    /// experiment's own definition of *full*.
    #[must_use]
    pub fn refutations(&self) -> u32 {
        self.landings(Judged::ByItsDocument)
    }

    /// **THE SAME LANDINGS, IN RUNS WHOSE CEILING A CALLER MOVED** — reported beside
    /// [`refutations`](Self::refutations) and never inside it; see that method.
    ///
    /// ⚠ Not a lesser number and not noise: it is what an EXPERIMENT produced, and item 856's own
    /// entry states the cost of losing it — *an experiment nobody is told about does not go
    /// unnoticed, it contaminates the denominator it was run in.*
    #[must_use]
    pub fn landings_at_a_moved_ceiling(&self) -> u32 {
        self.landings(Judged::ByACallerWhoMovedIt)
    }

    /// 🎯🎯🎯🎯🎯 **THE RATE ITEM 856's AXIS IS ACTUALLY READ FROM** — register item 894 ⑶, and
    /// the number five re-judgements of that clause each said was waiting on a population.
    ///
    /// # ⛔⛔⛔⛔⛔ A per-run row is not a rate, and adding rows by hand is what this replaces
    ///
    /// [`measured`](Self::measured) prints one line per run, and item 856's question is a
    /// COMPARISON between roads — *does a prompt sent while the session is full fold more often
    /// than one sent for any other reason*. Answering it off the rows means a reader summing
    /// columns in their head, which is exactly the `python3 -c` item 856 ⒝ was built to retire one
    /// level down. Register item 894 ⑶ states the deliverable in those words: read the ratio over
    /// PRODUCTION runs, with no ceiling moved for it.
    ///
    /// ⛔⛔ **ONLY THE ROWS JUDGED BY THEIR OWN DOCUMENT.** A run whose caller moved the ceiling
    /// reflects on `capacity` because it was told to hand over early, so pooling it would answer
    /// this axis with the experiment's own definition of *full* — [`refutations`](Self::refutations)
    /// holds the whole argument and the 29 landings that forced it.
    ///
    /// ⚠ Every road in [`sprag_plugin::Occasion::ALL`]'s order **including the empty ones**, for
    /// [`unmeasured`](Self::unmeasured)'s reason: the population is the enum, and a caller deciding
    /// what to print may drop a line where a caller deciding whether the table is whole may not.
    /// The roads with nothing on them are the CONTROL and the whole reason the split exists.
    #[must_use]
    pub fn folded_by_road(&self) -> Vec<(sprag_plugin::Occasion, u32, u32)> {
        sprag_plugin::Occasion::ALL
            .into_iter()
            .map(|occasion| {
                let (folded, delivered) = self
                    .measured
                    .iter()
                    .filter(|row| row.judged == Judged::ByItsDocument)
                    .map(|row| row.folds.under(occasion))
                    .fold((0, 0), |(folded, delivered), row| {
                        (folded + row.folded, delivered + row.delivered)
                    });
                (occasion, folded, delivered)
            })
            .collect()
    }

    /// **HOW MANY RUNS THAT RATE IS OVER** — the rows [`folded_by_road`](Self::folded_by_road)
    /// sums, and the number that has to travel beside it.
    ///
    /// ⚠ Published rather than left to a caller's filter for item 856's most repeated failure: a
    /// rate with no population is a number a reader cannot argue with, and `0 of 0` on every road
    /// reads as *nothing folded* where it means *nobody has run yet*.
    #[must_use]
    pub fn production_runs(&self) -> usize {
        self.measured
            .iter()
            .filter(|row| row.judged == Judged::ByItsDocument)
            .count()
    }

    /// **HOW MANY RUNS [`stranded`](Self::stranded) SITS IN** — the count under
    /// [`NoFullness::CapacityUnjudgeable`], read here so no mouth re-derives it from the map.
    ///
    /// ⚠ A mouth needs BOTH numbers or it cannot print the stranded count honestly: over a
    /// population it does not name, a `0` there is the same reading this report withholds its
    /// landing counts to avoid.
    #[must_use]
    pub fn unjudgeable_runs(&self) -> usize {
        self.unmeasured
            .iter()
            .find(|(why, _)| *why == NoFullness::CapacityUnjudgeable)
            .map_or(0, |(_, count)| *count)
    }

    /// The two above, over one arm — written once so the pair cannot come to mean different sums.
    fn landings(&self, judged: Judged) -> u32 {
        self.measured
            .iter()
            .filter(|row| row.judged == judged)
            .map(FoldAtFullness::landed_on_the_capacity_road)
            .sum()
    }
}

/// One run whose fold split can be read against how full its session got — [`Folds::measured`]'s
/// member, and the row register item 894 built its column for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FoldAtFullness {
    /// The run.
    pub id: u64,
    /// **HOW FULL ITS SESSION EVER GOT** — [`PersistedRun::context_high_water`], the left-hand term
    /// of item 856's comparison and a PEAK rather than a level.
    pub fullest: i64,
    /// **WHAT IT WAS JUDGED BY** — [`PersistedRun::context_ceiling`], the right-hand term.
    pub ceiling: i64,
    /// **WHOSE NUMBERS THOSE WERE** — derived from [`PersistedRun::overridden`], register item 859.
    pub judged: Judged,
    /// Its whole split, every occasion — never the `capacity` row alone.
    ///
    /// ⚠⚠ **THE OTHER ROADS ARE THE CONTROL AND THE ROW CARRIES THEM FOR THAT REASON.** Item 856's
    /// design note says it outright: counting `capacity` alone is what the split was built to stop,
    /// because with no control group the axis cannot be told from *a reflection prompt is the
    /// longest thing this loop builds*. The other occasions hold the prompt's SHAPE roughly fixed
    /// and vary only what brought the loop there, so line against line is the axis.
    pub folds: sprag_plugin::FoldsByReason,
}

/// A fold split's `capacity` row — the discriminator, named ONCE so no reader picks the occasion
/// itself.
///
/// ⚠⚠ Free of [`FoldAtFullness`] because **both halves of [`Folds`] ask it**: the measurable rows
/// through the methods below, and the rows behind item 894's wall through
/// [`NoFullness::CapacityUnjudgeable`], which is a count of exactly this question. A row that
/// cannot be put on the axis can still be asked whether it holds evidence.
/// ⚠ PUBLIC because it is the crate's ONE answer to that question and two public types lean on it
/// ([`FoldAtFullness`] and, through [`NoFullness::CapacityUnjudgeable`], [`Folds`]). A private
/// authority that public docs have to name is an authority a reader cannot follow.
#[must_use]
pub fn on_the_capacity_road(folds: &sprag_plugin::FoldsByReason) -> sprag_plugin::FoldsUnder {
    folds.under(sprag_plugin::Occasion::Reflecting(
        sprag_plugin::ReflectReason::Capacity,
    ))
}

/// **HOW MANY `capacity` PROMPTS LANDED** — delivered minus folded on that road.
///
/// ⚠ `delivered` is every prompt asked under the occasion and `folded` is the subset the composer
/// swallowed, so the difference is the landings — the arithmetic item 856 ⑴ got wrong once by
/// reading `delivered` as *landed*. Written here rather than at two readers so the judged count
/// and [`Folds::stranded`] cannot come to mean different sums.
#[must_use]
pub fn landed_on_the_capacity_road(folds: &sprag_plugin::FoldsByReason) -> u32 {
    let row = on_the_capacity_road(folds);
    row.delivered.saturating_sub(row.folded)
}

/// **WHETHER THAT ROAD WAS WALKED AT ALL** — a prompt asked under the reason, or one that hardened
/// into `prompt.unasked` under it.
///
/// ⚠ The unasked half counts: the transition is what says the session reached its ceiling, and
/// whether a question came out of it is a later fact.
#[must_use]
pub fn took_the_capacity_road(folds: &sprag_plugin::FoldsByReason) -> bool {
    let row = on_the_capacity_road(folds);
    row.delivered > 0 || !row.unasked.is_empty()
}

impl FoldAtFullness {
    /// Its `capacity` row — see [`on_the_capacity_road`], whose doc holds why the arithmetic lives
    /// outside this type.
    #[must_use]
    pub fn on_the_capacity_road(&self) -> sprag_plugin::FoldsUnder {
        on_the_capacity_road(&self.folds)
    }

    /// **HOW MANY OF ITS `capacity` PROMPTS LANDED** — see [`landed_on_the_capacity_road`].
    #[must_use]
    pub fn landed_on_the_capacity_road(&self) -> u32 {
        landed_on_the_capacity_road(&self.folds)
    }

    /// Whether the session had really reached the bound it was judged by.
    ///
    /// ⚠⚠ It is NOT what tells an experiment from an ordinary run — [`judged`](Self::judged) is,
    /// and this is true of both by construction whenever the `capacity` road was taken at all
    /// (`ai_loop.scxml` turns on `context >= context_ceiling`, and [`fullest`](Self::fullest) is a
    /// peak over those readings).
    ///
    /// ⛔⛔⛔⛔⛔ **THAT CONDITION IS THE WHOLE MEANING OF THIS ANSWER, AND ASKING THIS ALONE READS
    /// IT OFF.** `false` on a run that never took that road is *this session has not filled up
    /// yet*, which is what a healthy long run looks like every moment before its first reflection.
    /// Use [`columns_disagree`](Self::columns_disagree), which asks both halves.
    #[must_use]
    pub const fn reached_its_ceiling(&self) -> bool {
        self.fullest >= self.ceiling
    }

    /// Whether this run's document ever walked the `capacity` road at all — see
    /// [`took_the_capacity_road`], which the unmeasurable half of [`Folds`] asks of the same rows.
    #[must_use]
    pub fn took_the_capacity_road(&self) -> bool {
        took_the_capacity_road(&self.folds)
    }

    /// ⛔⛔⛔⛔⛔ **WHETHER THE TWO COLUMNS DISAGREE WHERE THEY WERE EVER PROMISED TO AGREE** — the
    /// question [`reached_its_ceiling`](Self::reached_its_ceiling) only half answers, and the one a
    /// reader of a row actually has.
    ///
    /// # ⛔⛔⛔⛔⛔ Measured on the first real row this item ever produced
    ///
    /// Item 856 ⑴⒞ waited five re-judgements for one ordinary run to carry a fullness. It arrived
    /// **2026-09-05T13:01:38Z** — run 232, peak `303328` of a `800000` ceiling its own document
    /// authored, `overridden []`, **`capacity` road `0 delivered · 0 unasked`** — and the mouth
    /// printed *⚠ ITS PEAK IS BELOW THAT CEILING, so the two columns disagree* over it. That is
    /// false: a session that has not reflected on capacity is SUPPOSED to sit below the ceiling,
    /// and every ordinary run does until the moment it does not.
    ///
    /// ⇒ ⭐⭐⭐⭐⭐ **The condition was written down and not asked.** `reached_its_ceiling`'s doc
    /// already said *whenever the `capacity` road was taken at all*; the renderer read the answer
    /// without it. A sentence in a doc is not a guard, and the arm was never exercised — the
    /// fixture's rows all reached their ceilings, so nothing could have found this but the real
    /// store. This workspace's own rule, printed: an arm nothing reaches will be wrong.
    #[must_use]
    pub fn columns_disagree(&self) -> bool {
        self.took_the_capacity_road() && !self.reached_its_ceiling()
    }
}

/// 🎯🎯🎯🎯🎯 **WHOSE NUMBERS A RUN WAS JUDGED BY** — register item 859, as item 856's measurement
/// needs it.
///
/// # ⛔⛔⛔⛔⛔ Why this is asked of the row and not kept as a note beside it
///
/// Item 856's two experiment arms were launched by moving `context_ceiling` off the document's
/// number, and on 2026-09-05 a round had to separate them from the ordinary runs **by reading a
/// human note in a memory file**. The number that came out of that hand-split — 29 landings — is
/// the one this type exists to keep from being quoted as a refutation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Judged {
    /// The caller took none of the numbers this run's document authored — the healthy launch, and
    /// the only population item 856's axis can be read over.
    ByItsDocument,
    /// A caller named `context_ceiling` itself, so this run is an EXPERIMENT: its `capacity`
    /// reflections say *we handed over at the number we chose*, never *the session filled up*.
    ByACallerWhoMovedIt,
}

impl Judged {
    /// Both answers, so a reader printing a partition cannot leave one out.
    pub const ALL: [Self; 2] = [Self::ByItsDocument, Self::ByACallerWhoMovedIt];

    /// What a stored [`PersistedRun::overridden`] says, or [`None`] when nothing does.
    ///
    /// ⚠⚠ [`None`] is *nobody answered* AND *this build cannot spell a word in that list*, which
    /// [`crate::plugins::Overridden::restored`] folds together and states its reason for: a shorter
    /// list is not a weaker claim, it names a different set of flags. Both are *this row cannot be
    /// told from an experiment*, which is the only thing this method is asked.
    fn of(words: &[String]) -> Option<Self> {
        crate::plugins::Overridden::restored(words).map(|taken| {
            if taken.moved_the_context_ceiling() {
                Self::ByACallerWhoMovedIt
            } else {
                Self::ByItsDocument
            }
        })
    }

    /// What a reader is looking at, in one clause — ⛔ an exhaustive `match` with no `_` arm.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::ByItsDocument => "its document's ceiling",
            Self::ByACallerWhoMovedIt => "A CEILING A CALLER MOVED, so this run is an experiment",
        }
    }
}

/// ⛔⛔⛔⛔⛔ **WHY A RUN'S FOLDS CANNOT BE READ AGAINST A FULLNESS** — [`Folds::unmeasured`]'s
/// population, and the half register item 856 ⑴ cannot be answered without.
///
/// # ⚠⚠⚠⚠⚠ It is a closed vocabulary because the alternative is a silent drop
///
/// A run that cannot be read yields no row, and *no row* and *no fold* are the same silence.
/// Naming each way is what lets a reader tell **the promotion wall** (these three columns exist in
/// this build and in no stored row yet) from **a session that folded nothing** — and today, over
/// the loop's own store, every run that ever delivered a prompt is behind one wall.
///
/// ⚠ The arms are tried in the order they are declared, and that order is a claim: a run with no
/// split has no fold for a fullness to be beside, so nothing further can be asked of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum NoFullness {
    /// No split at all — [`Sampled::Unsaid`], a row from a daemon older than the table.
    SplitUnsaid,
    /// A split that is present and all zero — [`Sampled::Zeroed`]. *Delivered nothing* for a build
    /// that had the counter and *never counted* for one written before register item 891, and the
    /// row cannot say which.
    ///
    /// ⚠ Its own arm rather than either neighbour's, which is register item 895's whole finding:
    /// measured over 220 stored rows, folding it would decide 209 of them by fiat.
    SplitZeroed,
    /// It delivered prompts and no [`PersistedRun::context_high_water`] — **the promotion wall**,
    /// register item 894 — and its `capacity` road was never walked, so nothing says how full that
    /// session was and no landing this axis is looking for was inside it to lose.
    FullnessUnread,
    /// **THE SAME WALL, OVER A ROW THAT DID WALK THE `capacity` ROAD** — landings this log HOLDS
    /// and this axis may not use, because nothing on the row says whose ceiling that session
    /// reflected on.
    ///
    /// ⚠ IMMEDIATELY AFTER [`FullnessUnread`](Self::FullnessUnread), because that is the arm it
    /// was wrongly inside. The two share the missing column and differ on whether the row carries
    /// evidence at all — the difference between *nobody measured this* and *somebody measured it
    /// and the reading cannot be attributed*. Only the second has a size, and
    /// [`Folds::stranded`] is that size.
    ///
    /// ⛔⛔⛔⛔⛔ **Measured over the live store at 2026-09-05T13:41:36Z: 3 of the 18 rows behind
    /// the wall, carrying 32 `capacity` deliveries and 3 folds — 29 landings.** They are the same
    /// 29 [`Folds::refutations`] names, which a round quoted as refutations for a day; the rows
    /// themselves state neither `context_ceiling` nor `overridden`, so *whose ceiling* was a fact
    /// only a person's memory held. Pooled under the arm above, a reader of this report was told
    /// this store had never walked that road at all.
    CapacityUnjudgeable,
    /// How full it got is recorded and [`PersistedRun::context_ceiling`] is not, so the row states
    /// a reading and never what it was measured against — register item 856 ⑴b.
    CeilingUnrecorded,
    /// **ITS CEILING IS RECORDED AND IS NOT IN FORCE** — `context_ceiling <= 0`, which the loop's
    /// own document reads as *unbounded*, so **no capacity reflection could ever have fired on
    /// this run.**
    ///
    /// ⛔⛔⛔⛔⛔ **THE DOCUMENT SAYS SO AND THE COUNT IS ITS OWN**, asked of `ai_loop.scxml`
    /// rather than argued: every edge that can reach the `capacity` road is guarded
    /// `context_ceiling > 0 && context > 0 && context >= context_ceiling` (**3** of them), and
    /// four further edges are guarded `context_ceiling <= 0` and go somewhere else — measured
    /// 2026-09-05T15:55:05Z. A zero is therefore not a small bound, it is NO bound.
    ///
    /// ⛔⛔ **So a row like this may not sit in the axis's denominator.** Item 856 waits for one
    /// `capacity` prompt to LAND; a run that cannot take that road can never supply one, and
    /// counting it makes the rate quieter without making it truer — this workspace's rule 5, and
    /// its rule 6 in the same breath, since *unbounded* is precisely the escape hatch that would
    /// nullify the gate.
    ///
    /// ⚠⚠ **REACHABLE, AND IT HAS ARRIVED.** Register item 894's own round measured that
    /// `authored_number` accepts a zero and that it travels all the way to the row; on
    /// 2026-09-05T15:33:48Z the live store held one — run 233, `context_ceiling` **0** beside a
    /// peak of **417,509**. Today that row is excluded by accident, because its `overridden` is
    /// silent and [`ExperimentUnsaid`](Self::ExperimentUnsaid) catches it first. One that answered
    /// item 859 would walk straight into the rate.
    ///
    /// ⚠ And it removes a second nonsense the same row produces: with a ceiling of `0` every peak
    /// is `>=` it, so [`FoldAtFullness::reached_its_ceiling`] reads *this session filled up* about
    /// a session that had nothing to fill.
    CeilingUnbounded,
    /// Both readings are there and [`PersistedRun::overridden`] answers nothing, so an EXPERIMENT
    /// cannot be told from an ordinary run — register item 859. ⛔ Counted here rather than assumed
    /// to be the document's: assuming would put a moved-ceiling run into the axis's own
    /// denominator, which is the contamination item 856 filed 859 to stop.
    ExperimentUnsaid,
}

impl NoFullness {
    /// Every way, as the population [`Folds::unmeasured`] is built from — an eighth reason added
    /// to the type appears in every report without anybody widening a list.
    pub const ALL: [Self; 7] = [
        Self::SplitUnsaid,
        Self::SplitZeroed,
        Self::FullnessUnread,
        Self::CapacityUnjudgeable,
        Self::CeilingUnrecorded,
        Self::CeilingUnbounded,
        Self::ExperimentUnsaid,
    ];

    /// What a reader is looking at, in one clause — ⛔ an exhaustive `match` with no `_` arm, so an
    /// eighth way cannot reach a report wearing a seventh's sentence.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::SplitUnsaid => {
                "no fold split recorded at all, so there is no fold to put a fullness beside"
            }
            Self::SplitZeroed => {
                "a fold split present and all zero, which is *delivered nothing* and *never \
                 counted* at once"
            }
            Self::FullnessUnread => {
                "nothing recorded how full that session ever got, and it never walked the \
                 capacity road, so its folds sit on no axis"
            }
            Self::CapacityUnjudgeable => {
                "it walked the capacity road and nothing recorded how full that session got, so \
                 its landings cannot be told from an experiment's"
            }
            Self::CeilingUnrecorded => "nothing recorded which ceiling it was judged by",
            Self::CeilingUnbounded => {
                "its ceiling is not in force — the document reads that as unbounded, so no \
                 capacity reflection could ever have fired on it"
            }
            Self::ExperimentUnsaid => {
                "nothing says whether its numbers were its document's, so an experiment cannot be \
                 told from an ordinary run"
            }
        }
    }
}

/// The run log's format version. A file written by a different one is IGNORED rather than guessed
/// at: a run record is a convenience, and a wrong reading of one would be worse than its absence.
pub const RUN_LOG_VERSION: u32 = 1;

/// The registry of background plugin runs. Owned by the host (`serve`),
/// shared into each per-request `PluginsExternal` via `Arc<Mutex<_>>`.
#[derive(Default)]
pub struct RunRegistry {
    runs: Vec<RunRecord>,
    next_id: u64,
    /// ⛔⛔⛔⛔⛔ **WHAT THIS REGISTRY STAMPS ITS RUNS WITH** — register item 887. Minted when the
    /// registry is made and never afterwards, so every run it admits carries the same one and no
    /// other registry's runs can.
    born: Minting,
}

impl RunRegistry {
    /// **HOW LONG A SHUTDOWN WAITS FOR A WORKER IT HAS ASKED TO STOP**, and the number a reader
    /// should argue with — [`join_all_within`](Self::join_all_within)'s bound at every shutdown this
    /// product has.
    ///
    /// # ⚠⚠⚠ Measured, because a guessed one detaches live runs on every shutdown
    ///
    /// A run hears [`cancel`](Self::cancel) at its driver's loop top and inside every bounded wait
    /// it takes (`sprag_plugin::poll_until` asks the flag FIRST, every 10 ms), so the latency is a
    /// poll interval plus whatever it is inside that cannot see the flag. Over a real pane and the
    /// real orchestrator, a run that had been round its loop honoured a cancel in **2.7 – 10.5 ms**
    /// (six samples, 2026-08-17 — `rpc`'s
    /// `a_running_run_honours_cancel_well_inside_the_join_deadline`).
    ///
    /// The one thing a worker can be inside that does NOT consult the flag is a pane write, and that
    /// is bounded at `sprag_terminal`'s `DEVICE_TAKES_INPUT_WITHIN` — 500 ms, once, since the driver
    /// stops at its next loop top rather than starting another step. So **500 ms is the structural
    /// worst case** and five seconds is ten times it, some five hundred times the measured latency,
    /// and still short enough that a person who signalled the daemon gets their prompt back.
    pub const JOIN_DEADLINE: Duration = Duration::from_secs(5);

    /// How often [`join_all_within`](Self::join_all_within) asks whether a worker has come back.
    ///
    /// ⚠ There is no timed `join` in the standard library, so the wait is a poll — the primitive
    /// [`sweep`](Self::sweep) already uses. It costs a shutdown at most this much over a blocking
    /// join, which against a measured 2.7 – 10.5 ms is noise, and it is what makes the deadline
    /// keepable at all.
    const JOIN_POLL: Duration = Duration::from_millis(5);

    /// Take the next id WITHOUT registering anything — what a caller needs when the run's worker
    /// must know its own id before the record exists.
    ///
    /// # ⚠ Why this is a separate call and not read back off [`submit`](Self::submit)
    ///
    /// The worker thread ANNOUNCES its own end, so it has to close over the id — and it is spawned
    /// before `submit` can return one. Reading `next_id` and then calling `submit` would take the
    /// lock twice with a window between them, in which another request's `submit` takes the id this
    /// one is about to announce under. Reserving is one lock and no window.
    ///
    /// An id reserved and never submitted is simply skipped: within ONE registry these are
    /// monotonic, so a gap in them means only that a run did not start.
    ///
    /// # ⛔⛔⛔⛔⛔ **AND ACROSS REGISTRIES THEY ARE REUSED** — register item 887
    ///
    /// What stood here was *"ids are monotonic and never reused"*, and that sentence was measured
    /// false in this daemon's own state on 2026-09-04. [`restore`](Self::restore) sets `next_id` to
    /// `max(saved.id) + 1` **over the rows it finds**, so a successor restoring a log that is
    /// MISSING rows begins issuing numbers a predecessor already spent. Three at once were
    /// measured, each with a `/run/user/1000/loop/run<N>.log` finished before the row now bearing
    /// its number began.
    ///
    /// ⚠⚠⚠ **THE HARDENING THAT WOULD NOT HELP, AND WHY IT IS NOT HERE.** Refusing an id some live
    /// record already holds is easy and answers nothing: the rows that made the numbers collide
    /// were **gone from the log**, so this registry cannot know they existed. A guard that catches
    /// the case that never happens and misses the one that did reads as a check, which is worse
    /// than no guard — the same argument [`WhichRun`] makes against qualifying the number with the
    /// build. What identifies a run is [`WhichRun`]; this stays an ADDRESS.
    pub fn reserve(&mut self) -> RunId {
        let id = RunId(self.next_id);
        self.next_id += 1;
        id
    }

    /// Register a run under the id [`reserve`](Self::reserve) gave it.
    pub fn submit(&mut self, run: NewRun) -> RunId {
        let id = run.id;
        self.runs.push(RunRecord {
            id,
            label: run.label,
            plugin: Some(run.plugin),
            request: run.request,
            opened_by: run.opened_by,
            opened_by_session: run.opened_by_session,
            // ⛔⛔⛔ AND WHICH TREE IT IS FOR — register item 890. Taken from the caller, which is
            // the layer that resolved it to refuse a pane standing in the wrong place; this
            // directory has no workspace to ask and must not guess one.
            tree: run.tree,
            state: run.state,
            run: run.run,
            progress: run.progress,
            // ⚠ NOTHING REPORTED YET, whatever kind of driver this run has. A run whose driver is
            // another process fills this on its first `report_progress`; until then its row shows
            // the cell above, which is honestly zero — the run has not said it did anything.
            reported: Arc::new(Mutex::new(None)),
            // ⚠ STAMPED HERE AND NOWHERE ELSE ON THIS PATH — see `RunRecord::build`. The worker
            // about to run is inside THIS image, so this image is the only honest answer, and it
            // is read from the constant the same binary published at `client/hello`.
            build: Some(crate::wire::BUILD.to_owned()),
            // ⛔⛔⛔⛔⛔ AND WHICH RUN THIS IS — register item 887, stamped HERE for the line
            // above's reason and one of its own: this is the one moment a run becomes a record, so
            // it is the only moment at which a value can be given to that run and to no other.
            // The number it is built from is spent by `reserve` and never spent twice by THIS
            // registry; what the minting adds is everything that separates this registry from the
            // predecessor whose numbers it is about to start reissuing.
            which_run: Some(self.born.stamping(id)),
            // ⚠ A FRESH RUN HAS SAID NOTHING AND HAS LOST NOTHING — register item 671.
            reports: AtomicU64::new(0),
            revived_at: None,
            // ⚠ NOTHING WAS WITHHELD FROM A RUN NOBODY INHERITED — register item 737. This daemon
            // is starting it, so there is no predecessor's record for anything to have been kept
            // out of, and the absence is that fact rather than an unanswered question.
            withheld: None,
            // 🎯🎯🎯🎯🎯 AND WHICH OF ITS BOUNDS ARE NOT ITS DOCUMENT'S — register item 853,
            // carried from the submit that answered it. It is the CALLER's `NewRun` field and not a
            // `None` written here, because this is the one moment the question has an answer.
            overridden: run.overridden,
            // ⚠ AND NO PREDECESSOR LEFT A PROCESS DRIVING IT — register item 740, on the line
            // above's argument: this daemon is spawning this run's only driver, right now.
            ended_driver: None,
            // ⚠ AND WHERE A LIVE RUN IS TYPING IS `Progress::driving` AND NOT THIS — register item
            // 771. This field is a predecessor's last word about a run nobody is driving; a run
            // this daemon is starting has a driver that answers the question fresh on every step.
            drove: None,
            // ⚠ AND NOTHING FAILED TO PUT BACK A RUN NOBODY INHERITED — register item 771, on
            // `withheld` above's argument.
            not_resumed: None,
            // ⚠ AND NOBODY PUT THIS ONE BACK — register item 774. This daemon is starting it, so a
            // row claiming it was rescued would point a reader at a restart that never happened.
            resumed: false,
        });
        id
    }

    /// **EVERY RUN A PREDECESSOR LEFT THAT THIS DAEMON COULD PICK UP, AND EVERY ONE IT COULD NOT**
    /// — register item 543's sixth brick and register item 737's other half, both in submit order.
    ///
    /// A run appears in [`Inheritance::resumed`] exactly when [`restore`](Self::restore) kept both
    /// halves of what a resume needs — a place spelled in this image's documents and the request
    /// that built the plugin — which is `PersistedRun::resumable_request`'s rule, taken once at read
    /// time. Everything else a predecessor left is a record of something that is over.
    ///
    /// ⚠⚠⚠⚠⚠ **AND THE UNFINISHED RUNS THAT STAYED BEHIND ARE NAMED RATHER THAN OMITTED** —
    /// register item 737. This used to return a bare `Vec`, so *the predecessor left nothing* and
    /// *a promotion changed the documents and took every run with it* were the same empty list. The
    /// second is the COMMON case, because changing a document is what a promotion is usually for.
    ///
    /// ⚠ It says nothing about whether any of the resumed ones CAN be resumed: the pane may be
    /// gone, the brief may name a plugin this build no longer has, and the machine may refuse the
    /// place. Those are answers only the layer holding a workspace can give — see
    /// [`crate::plugins::PluginsExternal::put_back`], which gives them one at a time.
    #[must_use]
    pub fn inheritance(&self) -> Inheritance {
        let restored = self
            .runs
            .iter()
            .filter(|record| matches!(*lock(&record.state), RunState::Interrupted));
        let mut answer = Inheritance::default();
        for record in restored {
            // ⚠⚠ THE REASON IS PREFERRED OVER THE CELL, and the two cannot disagree: `withheld` is
            // `None` exactly when `resumable_request` handed the record both halves, which is
            // exactly when the two reads below are `Some`. Written this way round so a run that is
            // held back is reported ONCE, under the reason the log gave, rather than falling
            // through to a silent `continue` the way it did before this existed.
            if let Some(why) = record.withheld.clone() {
                answer.withheld.push(WithheldRun {
                    id: record.id,
                    label: record.label.clone(),
                    why,
                    driver: record.run.driver_pid(),
                });
                continue;
            }
            // ⚠⚠⚠ AND A RECORD WITH NO REASON MUST HAVE BOTH HALVES — `PersistedRun::withheld`
            // answers `None` exactly when `resumable_request` handed them over, so this pair is
            // present by that function's own rule. It is CHECKED rather than trusted because the
            // alternative is the silence this door was filed for: a run that fell through here
            // unclassified would vanish from both lists and be indistinguishable from a
            // predecessor that left nothing.
            let (Some(place), Some(request)) =
                (lock(&record.progress).place.clone(), record.request.clone())
            else {
                tracing::error!(
                    target: "sprag_host::runs",
                    run = record.id.0,
                    "a restored run named no reason for staying behind and carries no place or \
                     request either, so this boot can neither put it back nor say why — \
                     `PersistedRun::withheld` and `resumable_request` have stopped agreeing",
                );
                continue;
            };
            answer.resumed.push(InheritedRun {
                id: record.id,
                label: record.label.clone(),
                place,
                request,
                progress: Arc::clone(&record.progress),
                driver: record.run.driver_pid(),
                // ⛔⛔⛔ **AND WHERE IT HAD GOT TO ON THE SCREEN** — register item 771, beside where
                // its machine had got to. A loop that replaced its session is not on the pane its
                // request names, and this is the only record that says so.
                drove: record.drove,
            });
        }
        answer
    }

    /// ⛔⛔⛔⛔⛔ **THIS BOOT ENDED THE PROCESS ITS PREDECESSOR LEFT DRIVING A RUN IT IS NOT PUTTING
    /// BACK** — register item 740, and [`inheritance`](Self::inheritance)'s companion: that door
    /// says which runs stayed behind, and this one records what was done about what was still
    /// typing at them.
    ///
    /// Returns whether such a run is held here, so a caller that killed a process for a run this
    /// registry has never heard of learns it rather than writing into nothing.
    ///
    /// # ⚠⚠⚠⚠⚠ Only over a run that is NOT coming back, and the guard is structural
    ///
    /// The sentence this field becomes claims two things at once: that nothing is typing at that
    /// pane any more, and that `interrupted` is this daemon's ANSWER for the run rather than a word
    /// that fell out of who reached the driver first. Both are statements about a run nobody is
    /// resuming. A run being put back has its leftover ended too — that is register item 526's loop
    /// and it is older than this — but there the fact is invisible by design: the run comes back
    /// `running` on a new driver, and a clause explaining a process that no longer matters would be
    /// noise on the one row a person does not have to act on.
    ///
    /// So this refuses a record with no [`withheld`](RunSummary::withheld) reason rather than
    /// trusting its caller to be the boot's withheld loop. The row's own publish guard
    /// (`crate::plugins::run_to_json`) is nested inside the withheld one for the same reason: a
    /// clause that can only be SET where it can be PRINTED cannot drift apart from it.
    ///
    /// ⚠ It does not touch the run's state. The record is already [`RunState::Interrupted`] — that
    /// is what [`restore`](Self::restore) leaves — and moving it here would be this daemon
    /// inventing an ending for work its predecessor was doing.
    pub fn ended_leftover_driver(&mut self, id: RunId, pid: u32) -> bool {
        let Some(record) = self
            .runs
            .iter_mut()
            .find(|record| record.id == id && record.withheld.is_some())
        else {
            return false;
        };
        record.ended_driver = Some(pid);
        true
    }

    /// ⛔⛔⛔⛔⛔ **THIS BOOT TRIED TO PUT AN INHERITED RUN BACK AND COULD NOT, AND HERE IS WHY** —
    /// register item 771, and [`inheritance`](Self::inheritance)'s other companion:
    /// [`ended_leftover_driver`](Self::ended_leftover_driver) records what a boot did to a process,
    /// and this records what a boot could not do about a run.
    ///
    /// Returns whether such a run is held here, so a caller writing about a run this registry has
    /// never heard of learns it rather than writing into nothing —
    /// [`ended_leftover_driver`](Self::ended_leftover_driver)'s rule.
    ///
    /// # ⚠⚠⚠⚠⚠ Only over a run [`Withheld`] does NOT already explain, and the guard is structural
    ///
    /// The two answer the same question — *why is this row still `interrupted`?* — from opposite
    /// sides of the log's own door, and a row carrying both would be telling a person that its
    /// documents were foreign AND that its pane was gone, when only the first was ever asked. A
    /// withheld run is never handed to the boot's put-back loop at all
    /// ([`Inheritance::withheld`] is the other list), so this refuses such a record rather than
    /// trusting its caller — and the row's publish guard is nested the same way, on
    /// `RunRegistry::ended_leftover_driver`'s argument that a clause which can only be SET where it
    /// can be PRINTED cannot drift apart from it.
    ///
    /// ⚠ It does not touch the run's state, on that door's argument: the record is already
    /// [`RunState::Interrupted`], and the whole finding of item 771 is that the WORD was never the
    /// problem — the missing thing was the clause beside it.
    ///
    /// ⚠⚠ **AND THE RESIDUE, MEASURED RATHER THAN CLAIMED: no gate can reach that guard.**
    /// [`inheritance`](Self::inheritance) pushes a withheld record onto its OTHER list and
    /// `continue`s, so the boot's put-back loop — this door's only caller — never holds one. Struck
    /// out under mutation on 2026-08-30, the end-to-end gate stayed GREEN. It is the shape item 740
    /// already named: one rule spelled in two places, where neither spelling alone is reachable. It
    /// is kept because the two spellings are what make the exclusion true of the ROW rather than of
    /// this caller, and a second caller is the day it starts mattering.
    pub fn not_resumed(&mut self, id: RunId, why: NotResumed) -> bool {
        let Some(record) = self
            .runs
            .iter_mut()
            .find(|record| record.id == id && record.withheld.is_none())
        else {
            return false;
        };
        record.not_resumed = Some(why);
        true
    }

    /// **THIS RESTORED RUN HAS A DRIVER AGAIN** — register item 543's sixth brick, and the one door
    /// in this file that turns an ending back into a beginning.
    ///
    /// Returns whether it happened: [`false`] both for a run this daemon does not hold and for one
    /// in a state a driver may not be handed to.
    ///
    /// # ⚠⚠⚠⚠ The two states that may take one, and why they are exactly two
    ///
    /// [`RunState::Interrupted`] is what a predecessor's log restores to, and
    /// [`RunState::Panicked`] is what a driver that died without an outcome leaves behind
    /// (register item 671) — **both mean NOTHING IS DRIVING THIS RUN**, which is the only property
    /// this door actually needs. `Running` must be refused because a second driver over one pane is
    /// two processes typing at one agent (register item 526), and the two finished states must be
    /// refused because a run that ENDED is not waiting for anything.
    ///
    /// ⚠ The `Panicked` arm was added rather than having the caller write `Interrupted` first: that
    /// left a window in which the row said *interrupted* while a rescue was already under way, and
    /// a reader who looked into it saw a state nobody was ever in for a reason. Measured — it broke
    /// `plugins`'s own resume gate, which reads the row straight after a put-back.
    ///
    /// # ⚠⚠⚠⚠⚠ Why the row is replaced rather than a second one submitted
    ///
    /// A resumed run is the SAME run. Submitting a new one would leave the old row standing as
    /// `interrupted` for ever beside a new id doing its work — two rows for one piece of work, and
    /// the reader watching the first would watch it not move. Ids are the thing every reader holds
    /// on to (`cancel` takes one, an agent's own-run filter compares one), so the id is what must
    /// survive the restart, exactly as it survives the log.
    ///
    /// ⚠⚠ **AND `Interrupted` IS A GUARD AND NOT A FORMALITY.** It is the one state that means *no
    /// process is driving this*, so it is the only one where swapping the handle cannot orphan a
    /// live driver — a `Running` row's worker would go on stepping a plugin nothing could then
    /// cancel, because the flags a cancel reaches would be the new handle's.
    ///
    /// ⚠ The progress cell is NOT replaced: the caller was handed it by
    /// [`inheritance`](Self::inheritance) precisely so the new driver writes where the row already
    /// reads.
    pub fn put_back(
        &mut self,
        id: RunId,
        plugin: crate::plugins::PluginName,
        state: Arc<Mutex<RunState>>,
        run: Box<dyn RunHandle>,
    ) -> bool {
        let taken = self
            .runs
            .iter_mut()
            .find(|record| record.id == id)
            .filter(|record| {
                matches!(
                    *lock(&record.state),
                    RunState::Interrupted | RunState::Panicked(_)
                )
            });
        let Some(record) = taken else {
            // ⚠⚠⚠ THE DRIVER THIS DOOR WILL NOT INSTALL IS STOOD DOWN BEFORE IT IS DROPPED. A
            // caller builds the driver first — it cannot know what plugin to name until it has —
            // so a refusal here would otherwise leave a worker stepping a plugin that no row can
            // cancel, hold or read. `Shutdown` and not `Person`, because nobody decided anything
            // about this run (register item 596).
            run.deliver(RunOrder::Cancel(Canceller::Shutdown));
            return false;
        };
        record.state = state;
        record.run = run;
        // ⚠ NAMED AGAIN, where `restore` left it `None`: there is a driver now, so *which plugin
        // would an order reach* has an answer again — see `RunRecord::plugin`.
        record.plugin = Some(plugin);
        // ⚠⚠ AND THE BUILD IS THIS ONE NOW. `restore` kept the dead daemon's stamp because that
        // image is what drove the work being reported; from here the work is THIS image's, and
        // register item 438 is exactly the confusion of dating one daemon's work to another.
        record.build = Some(crate::wire::BUILD.to_owned());
        // ⛔⛔⛔ AND THE ROW CAN NOW SAY THIS RUN WAS RESCUED — register item 774. Without it a
        // resumed row is byte-identical to one somebody started a second ago, and *came back and
        // has not typed anything in two hours* had no way to be said at all.
        //
        // ⚠ SET HERE AND NEVER CLEARED. It is a fact about this run's history, not a state: a run
        // that came back and then worked for a day was still put back by a boot, and a reader
        // asking *why has this one been quiet* wants that in either case.
        record.resumed = true;
        true
    }

    /// **A DRIVER IN ANOTHER PROCESS SAYS WHAT ITS RUN HAS DONE** — register item 650, refusing
    /// with a REASON where nothing here will ever drive that run — register item 764.
    ///
    /// ⚠⚠ `progress` is stored WITHOUT being read apart — see `RunRecord::reported`. It is
    /// `crate::plugins::progress_to_json`'s own output, so a key that renderer grows reaches the
    /// row with nothing here to update.
    ///
    /// ⚠ EACH REPORT REPLACES THE LAST. What a reader wants is what the run has done so far, and a
    /// report that arrived late or not at all costs nothing once the next one lands — the same
    /// *this is a LEVEL* reasoning [`sprag_plugin::ProgressCell`] states for itself.
    ///
    /// # ⛔⛔⛔⛔⛔ What the refusal is FOR, and the one thing it must not do
    ///
    /// [`Unreported`] carries the whole argument. The line to hold here is that it refuses on a
    /// **decision this daemon took**, never on a state it is passing through: `put_back` validates
    /// a plugin, spawns its driver and only THEN installs the row (its own *everything is checked
    /// before the row is touched* rule), so between those two acts a run that is being rescued is
    /// still `Interrupted` — and a door that refused on the WORD would answer *nothing is driving
    /// you* to the very driver this daemon had just stood up for it. `Panicked` is the same window
    /// one item over (register item 671: a lost driver's replacement is built while the row still
    /// says the last one died), and the gate above it already asserts that a report puts such a run
    /// back in business.
    ///
    /// So the refusing arms are the two that RECORD a decision — item 737's `withheld` and item
    /// 771's `not_resumed` — plus a run whose ending is in hand. ⚠ No `_` arm: a sixth
    /// [`RunState`] is classified on the day it exists rather than defaulting into *take the
    /// report*, which is the answer that made this defect invisible.
    ///
    /// # Errors
    ///
    /// [`Unreported`], whose arms are four different things for the reporter to do.
    pub fn report(&self, id: RunId, progress: serde_json::Value) -> Result<(), Unreported> {
        let Some(record) = self.runs.iter().find(|run| run.id == id) else {
            return Err(Unreported::NoSuchRun);
        };
        // ⚠⚠ THE VERDICT IS TAKEN AS A VALUE AND THE STATE LOCK IS GONE BEFORE ANYTHING ACTS ON IT.
        // A `match lock(..)` holds its guard for the whole match — the scrutinee's temporaries live
        // that long — and both arms below go on to take a SECOND lock on the same record. This
        // workspace has measured that shape as a hang that is green for as long as the assertion
        // passes; `crate::remote_access::RemotePaneAccess::read` states it at length.
        let refusal = {
            let state = lock(&record.state);
            match &*state {
                // Driven here, or about to be: see the doc above on why the second of these is an
                // accept and not an oversight.
                RunState::Running | RunState::Panicked(_) => None,
                RunState::Interrupted => record
                    .withheld
                    .clone()
                    .map(Unreported::Withheld)
                    .or_else(|| record.not_resumed.clone().map(Unreported::NotResumed)),
                RunState::Done { .. } | RunState::Reported(_) => Some(Unreported::Ended),
            }
        };
        if let Some(why) = refusal {
            return Err(why);
        }
        *lock(&record.reported) = Some(progress);
        // ⚠⚠⚠ AND THE COUNT GOES UP EVEN THOUGH THE VALUE ABOVE WAS REPLACED — register item 671.
        // What the row shows is a LEVEL and each report overwrites the last; what
        // [`Self::revival`] needs is the opposite question — *has the driver I started said
        // anything at all* — and only something that never goes down can answer it.
        record.reports.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// **WHAT TO DO ABOUT A RUN WHOSE DRIVER PROCESS DIED WITHOUT REPORTING AN OUTCOME** — register
    /// item 671, and the door the daemon's *supervisor of the supervisor* goes through.
    ///
    /// # ⚠⚠⚠⚠⚠ Why a live daemon has to answer this at all
    ///
    /// A run driven on a thread of this daemon's own could not lose its driver without the daemon
    /// losing everything; register item 544 moved the driver into a process, and the answer went
    /// with it. A boot already puts back every run a DEAD daemon left
    /// ([`crate::plugins::PluginsExternal::put_back`], reached from `put_back_inherited_runs`), so
    /// without this the same run gets two different fates depending on an accident nobody chose:
    /// the daemon happening to restart. This makes the answer depend on the RUN.
    ///
    /// # ⚠⚠⚠⚠ The place is read from what the driver REPORTED, not from the cell
    ///
    /// [`Self::inheritance`] reads `progress.place` because a restored record's cell is filled from
    /// the log. A run driven in another process never moves that cell at all — its counters arrive
    /// by [`report`](Self::report) and the row prefers them (register item 662) — so reading the
    /// cell here would answer *no place* for every live run there is. The fallback is kept for the
    /// run this daemon drives on a thread, whose cell IS the truth.
    ///
    /// ⚠ **AND NO FINGERPRINT IS CHECKED**, unlike the boot's path: these words were written by
    /// THIS image's own driver minutes ago, so *did a different build spell this place* is a
    /// question that cannot arise. `PersistedRun::resumable_place` exists for words that crossed a
    /// file.
    ///
    /// ⚠⚠ **A REFUSAL IS WRITTEN INTO THE ROW, NOT ONLY RETURNED.** The person who meets one of
    /// these is looking at a run that has stopped, and *its driver died* without *and nothing is
    /// going to pick it up* leaves them waiting for a daemon that has already decided not to.
    pub fn revival(&mut self, id: RunId) -> Revival {
        let Some(record) = self.runs.iter_mut().find(|record| record.id == id) else {
            return Revival::NoSuchRun;
        };
        /// Say in the ROW that nothing is coming, on top of what the ending already says.
        ///
        /// ⚠ Only over the death this door is about: any other state is somebody else's sentence
        /// about this run and appending to it would be this daemon talking over them.
        fn stays_dead(record: &RunRecord, why: &str) {
            let mut state = lock(&record.state);
            if let RunState::Panicked(said) = &*state {
                *state = RunState::Panicked(format!(
                    "{said}; and this daemon did not put it back on a new driver: {why}"
                ));
            }
        }
        let said = record.reports.load(Ordering::Relaxed);
        let place = lock(&record.reported)
            .as_ref()
            .and_then(|reported| crate::plugins::progress_from_report(reported).place)
            .or_else(|| lock(&record.progress).place.clone());
        // ⛔⛔⛔⛔⛔ **AND WHICH PANE, ON THE LINE ABOVE'S ARGUMENT AND FOR REGISTER ITEM 771's
        // REASON.** This door is a boot's one fact over: nothing is driving the run, and something
        // is about to be stood up over a pane. A loop that replaced its inner session while it
        // worked is not on the pane its request names, and the report is the only thing that says
        // where it went — so reading the request alone here would rebuild the run over a pane that
        // closed, exactly as a promotion did on 2026-08-30.
        let drove = lock(&record.reported)
            .as_ref()
            .and_then(|reported| crate::plugins::progress_from_report(reported).driving)
            .or_else(|| lock(&record.progress).driving);
        let outcome = if record.revived_at.is_some_and(|watermark| said <= watermark) {
            Revival::NoProgress
        } else if let Some(place) = place {
            if let Some(request) = record.request.clone() {
                record.revived_at = Some(said);
                // ⚠⚠⚠⚠⚠ AND THE ROW IS LEFT EXACTLY AS THE DEATH FOUND IT — see
                // [`Self::put_back`], which takes a `Panicked` record for this reason. Writing
                // `Interrupted` here first read better and was worse: it put the row through a
                // word nobody was ever in, and a reader who looked into that window was told the
                // run was over while its replacement was being built. The row moves once, from
                // *its driver died* straight to *it is running again*.
                Revival::PutBack(Box::new(InheritedRun {
                    id: record.id,
                    label: record.label.clone(),
                    place,
                    request,
                    progress: Arc::clone(&record.progress),
                    // ⚠ NOTHING TO END. The field exists so a BOOT can kill a driver that outlived
                    // the daemon which spawned it (register item 526); the driver this answer is
                    // about died in front of the thread asking, which is how the question got here.
                    driver: None,
                    drove,
                }))
            } else {
                Revival::NoRequest
            }
        } else {
            Revival::NoPlace
        };
        if let Some(why) = outcome.not_put_back() {
            stays_dead(record, why);
        }
        outcome
    }

    /// Raise the cancel flag for run `id`, returning whether such a run exists.
    /// The worker observes it at its next loop-top / wait-poll and ends
    /// [`crate::runs::RunState`]'s outcome as cancelled.
    pub fn cancel(&self, id: RunId) -> bool {
        // ⚠ A PERSON — register item 596. Every caller of this verb is somebody saying stop: the
        // CLI's `cancel-run`, the agent mouth's `cancel_run` on a run it opened, and a test
        // standing in for one. The daemon's own sweep has its own door beside this one, and the
        // whole point of the pair is that the two answers differ.
        self.order(id, RunOrder::Cancel(Canceller::Person))
    }

    /// **FORWARD `order` TO RUN `id`**, returning whether such a run exists — the ONE place this
    /// directory turns a caller's word into something delivered.
    ///
    /// # ⚠⚠⚠⚠⚠ Why the three public orders funnel through here — register item 544
    ///
    /// They used to be three near-identical bodies that each found the record and stored into a
    /// flag on it, which quietly made *"order a run"* mean *"reach into a thread's memory"*. A run
    /// whose driver is another PROCESS cannot be ordered that way at all, so the lookup and the
    /// delivery are separated: **finding the run is the directory's job, and knowing what delivery
    /// means is [`RunHandle`]'s.** The boolean is unchanged and still answers the only question the
    /// registry can answer — whether there is such a run — never whether the driver has acted.
    fn order(&self, id: RunId, order: RunOrder) -> bool {
        match self.runs.iter().find(|record| record.id == id) {
            Some(record) => {
                record.run.deliver(order);
                true
            }
            None => false,
        }
    }

    /// **FORWARD A STANDING ORDER TO RUN `id`, OR SAY WHY IT CANNOT BE** — register items 539 and
    /// 597, and the door [`hold`](Self::hold) and [`stand_down`](Self::stand_down) go through.
    ///
    /// # ⚠⚠⚠⚠⚠ Why this could not be [`order`](Self::order)'s boolean
    ///
    /// That boolean answers *is there such a run*, and there is a THIRD state of the world it
    /// cannot express: **the run exists and its plugin has no reader for the order.** Both orders
    /// used to collapse it into `true` — the caller was told it worked, the run drove straight on,
    /// and the CLI printed a promise about a pane that was never going to go still.
    ///
    /// ⚠⚠⚠ **A `Result` AND NOT A SECOND BOOLEAN**, which is register item 593's rule reaching a
    /// second surface: the type carries the reason so no caller can drop it, and the compiler makes
    /// each of the two doors decide what to say rather than letting one of them forget.
    ///
    /// ⚠⚠ **THE PLUGIN IS ASKED, NEVER LOOKED UP.** `RunHandle::honours` forwards the question to
    /// the plugin's own answer, so the day a second one grows a reader nothing here changes.
    ///
    /// ⚠⚠⚠⚠ **IT FINDS AND REFUSES; IT DOES NOT DELIVER** — register item 694. Its two callers used
    /// to hand it the order and get back a bare `Ok(())`, which left them unable to say anything
    /// about the run they had just ordered: [`hold`](Self::hold) has to read the LEVEL this order
    /// found, and a door that had already delivered by the time it answered could only be asked
    /// afterwards, when the answer is the level this very call just wrote. So the lookup answers the
    /// record and each caller delivers — which is this file's own sentence one line up, *finding the
    /// run is the directory's job*, carried through to its end.
    fn orderable(&self, id: RunId, order: &RunOrder) -> Result<&RunRecord, Unordered> {
        let Some(record) = self.runs.iter().find(|record| record.id == id) else {
            return Err(Unordered::NoSuchRun);
        };
        let Some(standing) = order.standing() else {
            // ⚠ UNREACHABLE BY CONSTRUCTION and answered rather than asserted: the two callers pass
            // `Hold` and `StandDown`. A `RunOrder` a person cannot raise over a running run has no
            // plugin to ask, so *nobody can be asked about it* is the honest answer, not a panic in
            // a daemon holding somebody's session.
            return Err(Unordered::NotAStandingOrder);
        };
        if !record.run.honours(standing) {
            // ⚠⚠ THE TWO CAUSES ARE DIFFERENT THINGS TO TELL A PERSON, so they are different arms:
            // a live run of the wrong plugin is one they should cancel instead, and a run restored
            // from a dead daemon is one there is nothing left to steer at all.
            return Err(match record.plugin {
                Some(plugin) => Unordered::Unread {
                    plugin,
                    order: standing,
                },
                None => Unordered::NoDriver,
            });
        }
        Ok(record)
    }

    /// **ASK RUN `id` TO FINISH WHAT IT IS DOING AND THEN STOP**, returning whether such a run
    /// exists. Its worker carries the order into the loop document at its next pass, and the
    /// document decides — at its own next milestone — what to do about it.
    ///
    /// ⚠⚠ NOTHING IS INTERRUPTED. That is the whole difference from [`cancel`](Self::cancel), and it
    /// is why a caller reaches for one or the other rather than for a flag with a mode: the turn in
    /// flight runs to its end and its work is banked.
    ///
    /// ⚠ IDEMPOTENT AND ONE-WAY. A second call changes nothing, and there is no un-ordering: a
    /// *stand down, no wait, carry on* racing a milestone would make a run's ending depend on which
    /// message arrived first.
    ///
    /// ⛔⛔⛔ **`by` IS WHERE THE ORDER CAME FROM, ALREADY RESOLVED** — register item 835. The
    /// caller points at a pane and the DAEMON reads the conversation off it, so what arrives here
    /// is a fact rather than a claim; see [`StoodDownBy`]. [`None`] is *nobody wrote it down*, and
    /// it must never be rendered as *a person*.
    pub fn stand_down(&self, id: RunId, by: Option<StoodDownBy>) -> Result<(), Unordered> {
        let order = RunOrder::StandDown(by);
        self.orderable(id, &order)?.run.deliver(order);
        Ok(())
    }

    /// **HALT RUN `id` BETWEEN TURNS, OR LET IT GO AGAIN**, returning whether such a run exists.
    ///
    /// # ⚠⚠⚠⚠⚠ The word a person did not have — register item 9
    ///
    /// `ai_loop.scxml` has carried *"a watching person can halt the loop between turns"* as an edge
    /// since R378 with **nothing able to raise it**. What a person had were the two ENDINGS —
    /// [`cancel`](Self::cancel) loses the turn, [`stand_down`](Self::stand_down) banks the milestone
    /// and converges — and neither of them is *wait, let me read this*.
    ///
    /// ⚠⚠⚠ **TWO-WAY, WHICH IS WHY IT TAKES AN ARGUMENT WHERE ITS NEIGHBOURS TAKE NONE.** Those two
    /// are latches and must be: an un-ordering racing a milestone would make a run's ending depend
    /// on message arrival. This one is a level a person raises and lowers, and the document's
    /// `resume` is the way back it was built with.
    ///
    /// ⚠ NOTHING IS INTERRUPTED and nothing ends. The turn in flight runs to its end, the loop then
    /// stops at `awaiting_human` — which sends nothing, so the pane stays exactly as the person
    /// found it — and their declared patience does not run while they hold it.
    ///
    /// # ⚠⚠⚠⚠⚠ It answers [`Holding`], and the level is read BEFORE the order lands
    ///
    /// Register item 694. A level's *delivered* is not its *changed something*, and the order is
    /// what makes them differ — so the read has to happen on the near side of the write or it
    /// answers what this very call just stored. Both happen under the caller's lock on this
    /// directory, which is what makes the pair one observation: every other door that delivers an
    /// order takes the same lock, and a driver only ever LOADS the flag.
    pub fn hold(&self, id: RunId, held: bool) -> Result<Holding, Unordered> {
        let order = RunOrder::Hold(held);
        let record = self.orderable(id, &order)?;
        let before = record.run.held();
        record.run.deliver(order);
        Ok(Holding::of(before, held))
    }

    /// Raise every run's cancel flag — used on host shutdown so in-flight runs abort promptly
    /// instead of being waited out and detached by [`join_all_within`](Self::join_all_within).
    ///
    /// ⛔⛔⛔⛔⛔ **AND IT PUBLISHES, WHICH IS THE WHOLE OF REGISTER ITEM 664.** A run driven in a
    /// process of its own reads its orders off its ROW and is woken to re-read it; this door used
    /// to be the one caller that reached the flags without anything being announced, so such a
    /// driver was never told and the bounded join below waited it out in full. Nothing is done
    /// about that here — [`Orders`]'s own `deliver` announces, so this gets it by going through
    /// the same delivery every other order does.
    pub fn cancel_all(&self) {
        for record in &self.runs {
            // ⚠ NOBODY DECIDED ANYTHING ABOUT THIS RUN — register item 596. The daemon is going
            // away and is raising every flag so no worker is waited out and detached; a reader told
            // only `cancelled` would go looking for the person who asked, and there is none.
            record.run.deliver(RunOrder::Cancel(Canceller::Shutdown));
        }
    }

    /// Join any finished worker threads (non-blocking via `is_finished`),
    /// turning a panicked worker into [`RunState::Panicked`]. Call before
    /// reading the registry so finished threads are reaped, not leaked.
    pub fn sweep(&mut self) {
        for record in &mut self.runs {
            if record.run.reapable()
                && let Some(why) = record.run.reap()
            {
                *lock(&record.state) = RunState::Panicked(why);
            }
        }
    }

    /// Every run in the durable shape its successor daemon reads.
    #[must_use]
    pub fn persistable(&self) -> RunLog {
        RunLog {
            version: RUN_LOG_VERSION,
            runs: self
                .runs
                .iter()
                .zip(self.snapshot())
                .map(|(record, run)| {
                    // ⛔⛔⛔ THE FIFTH IS THE ENDING'S OWN WORD — register item 706. It joins the
                    // four here rather than being read somewhere else because these arms are the
                    // one place that knows WHICH kind of driver produced the ending, and the word
                    // lives in a different place for each: in the `Outcome` this process computed,
                    // and in the report a driver on the far side of a socket sent back.
                    // ⛔⛔⛔⛔⛔ AND THE SIXTH IS WHY IT FAILED — register item 903, joining the
                    // five for their reason exactly: the sentence lives in the `Outcome` this
                    // process computed and in the report a driver across a socket sent back, and
                    // these arms are the one place that knows which. See `PersistedRun::failure`.
                    let (finished, outcome, ceiling, output, done_reason, failure, blocked_by) =
                        match &run.state {
                            RunState::Running | RunState::Interrupted => {
                                (false, None, None, None, None, None, None)
                            }
                            // ⚠ `uncommitted` is NOT persisted, and the omission is stated rather than
                            // an oversight: what a tree was holding is a fact about a moment that has
                            // passed, and a successor daemon publishing it would be vouching for a
                            // reading it never took. A restored run answers *cannot say*.
                            RunState::Done {
                                outcome, output, ..
                            } => (
                                true,
                                Some(crate::plugins::outcome_word(outcome).to_owned()),
                                crate::plugins::outcome_ceiling(outcome).map(str::to_owned),
                                output.clone(),
                                // ⛔ AND WHICH ENDING IT CLOSED UNDER — register item 706. Owned here
                                // and borrowed live, which is the `Cow`'s whole point: the log is the
                                // one reader that outlives the plugin that spelled the word.
                                outcome.done_reason.as_deref().map(str::to_owned),
                                // ⛔⛔⛔⛔⛔ AND WHY IT FAILED — register item 903. The SENTENCE, taken
                                // through the same `Display` every reader of a live run sees, so a row
                                // restored from this file and a row read off the running daemon say the
                                // same words. `None` for every ending that is not a failure, which is
                                // what `Outcome::failure` already means.
                                outcome.failure.as_ref().map(ToString::to_string),
                                // ⛔⛔⛔⛔⛔ AND WHY A BLOCKED RUN WAS NEVER ANSWERED — register item
                                // 903. THE REFUSAL'S WORD, out of a closed set of eleven; the QUESTION
                                // beside it does not cross, on `outcome_from_words`' stated argument.
                                // See `PersistedRun::blocked_by` for why that argument does not reach
                                // this half.
                                match &outcome.state {
                                    sprag_plugin::OutcomeState::Blocked(Some(unanswered)) => {
                                        Some(unanswered.why().wire_str().to_owned())
                                    }
                                    _ => None,
                                },
                            ),
                            // ⚠⚠⚠⚠ A RUN THAT ENDED IN ANOTHER PROCESS — register items 650 / 544, and
                            // the durable log loses NOTHING here: what it keeps of an ending is the
                            // word, the ceiling and the capture, and all three are what
                            // `outcome_to_json` has always carried. Read out of the report rather than
                            // recomputed, because the process that computed them is gone.
                            //
                            // ⚠ `PersistedRun`'s own fields are `Option<String>`, so a key the report
                            // does not carry lands as [`None`] — which this log already reads as
                            // *nobody wrote that down* rather than as a zero. An older daemon reading
                            // this file meets exactly the shape it meets for a thread-driven run.
                            RunState::Reported(reported) => (
                                true,
                                reported
                                    .get("state")
                                    .and_then(Value::as_str)
                                    .map(str::to_owned),
                                reported
                                    .get(crate::plugins::RUN_CEILING_KEY)
                                    .and_then(Value::as_str)
                                    .map(str::to_owned),
                                reported
                                    .get("output")
                                    .and_then(Value::as_str)
                                    .map(str::to_owned),
                                // ⛔ THE SAME WORD OUT OF THE REPORT — register item 706. The driver
                                // on the far side wrote it with THIS daemon's own `outcome_to_json`,
                                // so there is one spelling and this side reads rather than recomputes.
                                reported
                                    .get(crate::plugins::RUN_DONE_REASON_KEY)
                                    .and_then(Value::as_str)
                                    .map(str::to_owned),
                                // ⛔⛔⛔⛔⛔ AND THE SENTENCE OUT OF THE REPORT — register item 903, on
                                // the line above's argument: the far driver composed it with THIS
                                // daemon's `outcome_to_json`, so there is one spelling and this side
                                // reads rather than recomputes. ⚠ THE KEY IS THE ONE THAT SIDE WROTE
                                // (`failure`), which is why it is spelled here exactly as that composer
                                // spells it.
                                reported
                                    .get("failure")
                                    .and_then(Value::as_str)
                                    .map(str::to_owned),
                                // ⛔⛔⛔ AND THE REFUSAL'S WORD OUT OF THE REPORT — register item 903,
                                // on the line above's argument. Spelled as that composer spells it.
                                reported
                                    .get(crate::plugins::RUN_BLOCKED_BY_KEY)
                                    .and_then(Value::as_str)
                                    .map(str::to_owned),
                            ),
                            // ⚠ A DRIVER THAT DIED NAMED NO ENDING, and `why` is not one: it is the
                            // exit status of a process that stopped saying anything, which is the
                            // opposite of a run closing on its own terms.
                            //
                            // ⚠⚠ NOR IS IT A FAILURE SENTENCE — register item 903. This `why` is
                            // already published as `crate::plugins::RUN_ERROR_KEY` and means *the
                            // driver stopped saying anything*; `failure` means *the plugin met a cause
                            // and named it*. Putting an exit status in that column would hand a reader
                            // a diagnosis nobody wrote.
                            RunState::Panicked(why) => {
                                (true, Some(why.clone()), None, None, None, None, None)
                            }
                        };
                    // ⚠⚠⚠⚠⚠ **A DRIVER'S REPORT IS PREFERRED OVER THE CELL, AND THE ROW HAD ALREADY
                    // DECIDED THIS** — register item 662. For a run driven in another process the
                    // cell NEVER MOVES (`spawn_driven_run` files an empty one and says so), so
                    // reading it first wrote a durable record of `iterations: 0, place: None` for a
                    // run that had been going for hours. `crate::plugins::run_to_json` prefers the
                    // report for exactly this reason (item 650) — so before this line one daemon
                    // answered the same question two ways depending on who asked, and the FILE, the
                    // answer that outlives the process, was the wrong one. Item 606's whole finding
                    // is that a run is READ after it ends, when its daemon is already gone.
                    //
                    // ⚠⚠ A MISSING KEY FALLS BACK TO THE CELL AND NEVER TO A ZERO. There is no
                    // report at all for a run this daemon drives on a thread of its own — since
                    // 2026-08-24 that is a daemon told `RUN_DRIVER_PROCESS = off`, the way back
                    // from the new default — and an older driver reports only the keys its build
                    // knew. Both are *nobody said*, and the cell is the honest answer to that.
                    let reported = run
                        .reported
                        .as_ref()
                        .map(crate::plugins::progress_from_report)
                        .unwrap_or_default();
                    let cost = reported.cost.or(run.progress.cost);
                    // ⚠⚠⚠ WHERE IT WAS, MERGED BEFORE `document` IS STAMPED, because that
                    // fingerprint vouches for whichever of these two is present — see the field.
                    let at = reported.at.or_else(|| run.progress.at.map(str::to_owned));
                    let place = reported.place.or_else(|| run.progress.place.clone());
                    PersistedRun {
                        id: run.id.0,
                        label: run.label.clone(),
                        iterations: reported.iterations.unwrap_or(run.progress.iterations),
                        cost: cost.map(sprag_plugin::Cost::amount),
                        unit: cost.map(|c| sprag_plugin::Cost::unit(c).to_owned()),
                        // ⚠⚠ THE TIMES ARE NOT THIS MODULE'S TO KNOW — register item 801. Nothing
                        // here holds a clock, and the fact wanted is *when did this record last
                        // DIFFER*, which needs the previous record: `crate::durability`'s
                        // `stamp_run_times` has it and stamps both before the log is written. A
                        // guess taken here would be *when was this asked for*, which is the
                        // reading item 801 exists to remove.
                        moved_at: None,
                        ended_at: None,
                        // ⚠⚠ NOR THE INTERVAL SOMEBODY WATCHED — register item 888, and here the
                        // absence is stronger than a missing clock: `ran_to` is stamped from
                        // whether the PREVIOUS log already said this run had finished, and this
                        // module has no previous log. A value written here could only repeat
                        // `finished`, which is the conflation the item was filed for.
                        ran_from: None,
                        ran_to: None,
                        finished,
                        outcome,
                        ceiling,
                        output,
                        done_reason,
                        // ⛔⛔⛔⛔⛔ AND WHY IT FAILED, AND WHY A BLOCKED ONE WAS NEVER ANSWERED —
                        // register item 903. See the fields.
                        failure,
                        blocked_by,
                        build: run.build.clone(),
                        // ⛔⛔⛔⛔⛔ AND WHICH RUN IT IS — register item 887, and **the crossing
                        // the whole item turns on**: the numbers a successor reissues are the ones
                        // it read out of THIS file, so a stamp that stopped here would leave every
                        // restored run identified by the one thing that repeats.
                        which_run: run.which_run.as_ref().map(ToString::to_string),
                        // ⚠⚠⚠ ASKED OF THE HANDLE, NOT OF THE SNAPSHOT — register item 526. The
                        // snapshot is what a READER is told about a run, and where its driver lives
                        // is deliberately not part of that (`RUN_DRIVER_PROCESS`'s own promise is
                        // that a request means the same thing either way). This is a fact the
                        // directory keeps for its SUCCESSOR, and the handle is what holds it.
                        driver: record.run.driver_pid(),
                        // ⛔⛔⛔⛔⛔ **AND WHICH PANE IT IS ON NOW** — register item 771, read the
                        // same way and in the same order as `at` and `place` above: the report
                        // first, the cell behind it. For an out-of-process driver the cell never
                        // moves (register item 662), so reading the cell first would write `null`
                        // for exactly the runs a promotion has to put back.
                        //
                        // ⚠ NOT filled in from the request when neither says. *Nobody reported a
                        // pane* and *the run is on the pane it was asked over* are different facts,
                        // and `InheritedRun::pane` is the one place allowed to weigh them.
                        driving: reported.driving.or(run.progress.driving).map(|pane| pane.0),
                        opened_by_session: run.opened_by_session.clone(),
                        // ⚠⚠⚠ WHERE IT WAS, AND WHOSE WORD THAT IS — register items 543 and 544,
                        // written as a PAIR because either alone misleads. The fingerprint is
                        // stamped from THIS image, which is the only honest answer: it is the
                        // build whose documents produced the word beside it. A run with no
                        // recorded position records no document either, so a reader never sees a
                        // fingerprint vouching for nothing.
                        //
                        // ⚠⚠⚠⚠ **AND THE FINGERPRINT IS STILL HONEST WHEN THE WORD CAME OFF A
                        // DRIVER PROCESS** — register item 662. `crate::drive`'s whole design is
                        // that a driver IS this daemon's image (`std::env::current_exe`), so the
                        // documents that produced a reported word are the documents this constant
                        // names. A driver built from another image would be a different question,
                        // and there is no way to start one.
                        at: at.clone(),
                        // ⚠⚠⚠ AND THE WHOLE PLACE BESIDE THE WORD — register item 543. `at` is what
                        // a person reads; this is what an engine can be re-entered at, and the run
                        // log is the only thing a successor daemon has.
                        place: place.clone(),
                        // ⚠ VOUCHED FOR BY EITHER, because either alone is a position: a document
                        // recorded for neither would be a fingerprint standing over nothing, and a
                        // place recorded without one is a vocabulary nobody can check.
                        document: (at.is_some() || place.is_some())
                            .then(|| sprag_plugin::STATECHARTS_FINGERPRINT.to_owned()),
                        // ⛔⛔⛔⛔⛔ AND WHICH CEILING IT RAN UNDER — register item 856(1b). This is
                        // the hop the fact used to die at: it lived on the run REQUEST, which the
                        // restore path does not carry, so every ended run forgot the one number
                        // its fold rate had to be compared against.
                        //
                        // ⚠ Taken from the run's own progress rather than from the request even
                        // now, because the value resolves in three steps and the last of them
                        // happens inside the machine — the request holds the caller's number, and
                        // what a reader needs is the one the run OBEYED.
                        //
                        // ⛔⛔⛔⛔⛔ **AND THE REPORT IS READ FIRST, WHICH IS THE HALF THAT WAS
                        // MISSING** — register item 894, and a correction to item 856(1b)'s own
                        // crossing. This line read the CELL alone, and for an out-of-process run
                        // the cell never moves (`spawn_driven_run`: *"AN EMPTY CELL, AND IT STAYS
                        // EMPTY"*) — the default since 2026-08-24. Measured 2026-09-05, every
                        // other fact in this struct reads `reported…or(cell)` and this was the one
                        // exception, so the ceiling reached a LIVE row and never a durable log.
                        // Item 856's rate is computed over runs that have ENDED, so the fact was
                        // knowable exactly while nobody needed it.
                        context_ceiling: reported.context_ceiling.or(run.progress.context_ceiling),
                        // ⛔⛔⛔⛔⛔ AND HOW FULL ITS SESSION EVER GOT — register item 894, on the
                        // line above's terms and travelling with it: the pair is a comparison, and
                        // a log that kept one side would record which bound a run ran under and
                        // never how close it came.
                        context_high_water: reported
                            .context_high_water
                            .or(run.progress.context_high_water),
                        // 🎯🎯🎯🎯🎯 AND WHICH OF ITS NUMBERS THE CALLER TOOK — register item 859,
                        // and the third fact in a row to have been readable only while the run was
                        // alive. ⚠ NOT `reported…or(cell)` like the pair above: this is not a
                        // number the machine resolves as it goes but the answer the DOOR gave when
                        // the run was submitted, and a driver has no business reporting it. Its
                        // level never moves, which is `RunRecord::overridden`'s own doc.
                        overridden: run.overridden.as_ref().map(|took| {
                            took.taken().iter().map(|word| (*word).to_owned()).collect()
                        }),
                        // ⚠⚠⚠ ALWAYS `Some`, INCLUDING `false` — item 594. This image DID look, so
                        // `Some(false)` is a claim it is entitled to make; the `None` this field
                        // documents belongs to a log written before the field existed, and only a
                        // reader of such a log may see it. Writing `None` for *no order* would make
                        // this daemon's own silence indistinguishable from an older daemon's.
                        stood_down: Some(run.stood_down),
                        // ⛔⛔⛔ AND WHO GAVE IT — register item 835, written beside the flag it is
                        // the other half of. ⚠ NOT forced to `Some`, unlike the line above: this
                        // one's `None` is a real answer (*nobody wrote down who*) rather than an
                        // older daemon's silence, and the two are rendered the same way on purpose
                        // — neither is a claim about a person.
                        stood_down_by: run.stood_down_by.clone(),
                        // ⛔⛔⛔⛔⛔ **WRITTEN DOWN BY WHOEVER COUNTED IT, AND BY NOBODY ELSE** —
                        // register item 891, and this line used to read `Some(…unwrap_or(cell))`.
                        //
                        // ⚠⚠⚠ AND THE REPORT IS PREFERRED HERE TOO — register item 663. This is
                        // the column item 606 was filed for, and for a run driven in another
                        // process the cell it used to be read from is all zeros for ever: the log
                        // said `0 of 0` about a run that had filled somebody's pane, on exactly
                        // the runs anybody reads (a run is read after it ends, when its daemon is
                        // already gone).
                        deliveries: counted(reported.deliveries, run.progress.deliveries),
                        // ⛔⛔⛔ AND THE SPLIT OF THE SAME FOLDS — register item 856(1), written
                        // through [`counted`] the way the pair above is and for its reasons, and
                        // this is the column register item 891 was OPENED over: 220 of 220 stored
                        // rows carried a table and 11 of them carried a number.
                        folds_by_reason: counted(
                            reported.folds_by_reason,
                            run.progress.folds_by_reason,
                        ),
                        // ⛔⛔⛔⛔⛔ AND WHAT PROVED EACH DELIVERY — register item 856, written the
                        // way the two above are and for their reasons. This is the column a LANDING
                        // is read from, and it is read off a finished run whose daemon has since
                        // been restarted, which is what item 606 measured.
                        delivered_by_road: counted(
                            reported.delivered_by_road,
                            run.progress.delivered_by_road,
                        ),
                        // ⛔⛔⛔⛔⛔ AND WHICH SENTENCE EACH PROMPT WAS — register item 889, written
                        // the way the three above are and for their reasons, with one of its own:
                        // this column is the only place *which prompt gets stuck* is written down
                        // at all, and the ratio it answers is taken ACROSS runs — 197 of them, in
                        // the measurement that opened the item.
                        said_by_sentence: counted(
                            reported.said_by_sentence,
                            run.progress.said_by_sentence,
                        ),
                        // ⛔⛔⛔⛔⛔ AND WHAT THE WIDTH WOULD HAVE WITHHELD — register item 866(2),
                        // written the way the four above are and for their reasons, with one of
                        // its own: this number is only worth anything read ACROSS runs, because a
                        // single run whose answers were short is not evidence of anything and a
                        // hundred of them is the alarm that a build stopped reading logical lines.
                        width_withheld: counted(
                            reported.width_withheld,
                            run.progress.width_withheld,
                        ),
                        // ⚠⚠⚠⚠⚠ AND HOW MUCH OF THE WORK IS KEPT — register item 616. `None` here
                        // is the PLUGIN's own answer (*I count no completed work*) rather than
                        // this daemon's silence, which is why it is mapped through rather than
                        // forced to `Some` the way `stood_down` above is: that field's `None` had
                        // to be reserved for an older log, and this one's is a real answer a
                        // reader must be able to see.
                        //
                        // ⚠⚠ THE REPORT FIRST, the field above's argument — item 663. Note this
                        // one keeps a real `None` (*this plugin counts no work*) on the fallback,
                        // which is why the two are `.or`ed rather than defaulted.
                        banked: reported
                            .banked
                            .map(Into::into)
                            .or_else(|| run.progress.banked.clone().map(Into::into)),
                        // ⚠⚠⚠⚠⚠ AND HOW BIG THE BRIEF WAS — register item 719's second direction,
                        // written on the line above's terms exactly: the report first, the cell as
                        // the fallback, and a `None` that is the PLUGIN's own answer (*nobody
                        // briefs me*) rather than this daemon's silence.
                        briefed: reported
                            .briefed
                            .map(Into::into)
                            .or_else(|| run.progress.briefed.map(Into::into)),
                        // ⚠ AND HERE `None` REALLY IS *no cancel*, unlike the field above — item
                        // 596. A stand-down is a bool and needs `Some(false)` to distinguish a
                        // silent daemon from an old log; a canceller is an option already, so the
                        // absent case carries its own meaning and needs no second one.
                        cancelled_by: run.cancelled_by,
                        // ⚠⚠⚠⚠⚠ AND WHAT WOULD BE NEEDED TO START IT AGAIN — register item 543's
                        // sixth brick, and it is written under the SAME condition its reader
                        // checks: a run still going, with a place recorded beside it. Either half
                        // missing and a successor could do nothing with this but hold somebody's
                        // prose on disk — see `PersistedRun::request`, which holds the argument.
                        //
                        // ⚠⚠ THE PAIR IS DECIDED HERE AND READ AT `resumable_request`, which is
                        // two places agreeing rather than one deciding — deliberately, and the
                        // asymmetry is the reason: this end can only ever write LESS than the
                        // reader will take, so a log from an older daemon (request present, rule
                        // absent) is still read safely, while a log this daemon wrote carries
                        // nothing it could not use.
                        //
                        // ⚠⚠⚠ IT READS THE MERGED PLACE AND NOT THE CELL — register item 662. A run
                        // driven in another process has its place only in its report, so a rule
                        // that asked the cell here would record a place beside no request for
                        // exactly the driver kind that can read one, and item 543's resume would
                        // stop at the last step for the runs it was built for.
                        request: (!finished && place.is_some())
                            .then(|| record.request.clone())
                            .flatten(),
                        // ⛔⛔⛔⛔⛔ AND WHICH TREE IT WAS FOR, WITH NO GUARD AT ALL — register
                        // item 890, and the line above is what it is a reaction to. That one
                        // writes the request only for a resumable run, correctly; this daemon
                        // drives three repositories, so the effect was that **206 of 209 rows
                        // could not say which one they belonged to** — every finished run, which
                        // is every run anybody reads. One path is not a person's prose.
                        tree: record.tree.clone(),
                    }
                })
                .collect(),
        }
    }

    /// Take a predecessor daemon's run log into this registry.
    ///
    /// # ⚠⚠ Two rules, and both are authority decisions rather than conveniences
    ///
    /// 1. **THE PANE ID IS DROPPED AND THE CONVERSATION IS KEPT** — the decision this round
    ///    re-took, on what turned out to be true rather than on the sentence that used to justify
    ///    it.
    ///
    ///    The old rule dropped provenance ENTIRELY, reasoning that *"a restored pane's OCCUPANT is
    ///    a plain shell and never the agent that asked … A restored run is nobody's"*. Measured
    ///    2026-08-18 and **false**: `durability`'s default restore allowlist contains `claude` and
    ///    [`crate::durability::restore_command`] appends `--resume <uuid>`, so a restored agent
    ///    pane comes back holding **the same conversation** — pane 91 did, on this machine, from
    ///    the snapshot a `kill-server` had just written. The asker is not gone.
    ///
    ///    ⚠⚠⚠⚠⚠ **WHAT THE FALSE SENTENCE HID IS THAT THE HOLE HAD CHANGED SHAPE.** It was never
    ///    going to be *a new agent inherits a stranger's runs* once panes came back occupied; it
    ///    is *the same agent cannot see the runs it started* — a different question, and its right
    ///    answer is a different key. **Identity is the conversation, not the seat.** A pane id is
    ///    stable across a restart (`spawn_restored` is handed `pane.id`) and that is exactly what
    ///    made it seductive and wrong: stability is not identity, and the same seat can hold a
    ///    stranger.
    ///
    ///    So: `opened_by` (a seat) is still dropped, because a successor genuinely cannot know who
    ///    is sitting there; `opened_by_session` (a conversation) is KEPT, because it is the same
    ///    thing on both sides of the restart. The seat is then RE-DERIVED at read time by
    ///    `crate::plugins`, which is the layer that can see who is sitting where.
    ///
    ///    ⚠⚠⚠⚠ **RE-DERIVED PER READ, NEVER STAMPED ONCE.** Ownership is a LEVEL — *whoever is
    ///    currently holding this conversation* — and not an event that happened at boot. Stamping
    ///    it would be wrong the moment `ai_loop`'s `restarting` replaces a session, because that
    ///    replacement mints a FRESH conversation on purpose; a level stops matching by itself,
    ///    where a stamp would keep asserting a claim nobody could correct. It also cannot be done
    ///    at boot even if one wanted to: runs are restored BEFORE panes are
    ///    (`sprag-term`'s daemon arm), so at this point there is no pane to ask.
    /// 2. **THE ID COUNTER IS SEEDED ABOVE THEM.** Ids are monotonic and never reused
    ///    ([`reserve`](Self::reserve)); a successor that started from zero would mint ids that
    ///    already name a run in its own list.
    ///
    /// A restored run has no driver, and since register item 544's stage 2 it says so as a TYPE:
    /// its [`RunHandle`] is an [`EndedRun`], which accepts every order and delivers none. `cancel`
    /// still finds it and returns `true` — the honest answer to *does this run exist* for a run that
    /// is already over — but nothing here mints a flag whose own comment has to explain that setting
    /// it does nothing.
    pub fn restore(&mut self, log: &RunLog) {
        if log.version != RUN_LOG_VERSION {
            return; // a format this build cannot read is worse than no record at all
        }
        for saved in &log.runs {
            let cost = match (saved.cost, saved.unit.as_deref()) {
                (Some(amount), Some("tokens")) => Some(sprag_plugin::Cost::Tokens(amount)),
                (Some(amount), Some(_)) => Some(sprag_plugin::Cost::Bytes(amount)),
                _ => None,
            };
            let state = if saved.finished {
                RunState::Done {
                    outcome: Box::new(Outcome {
                        state: crate::plugins::outcome_from_words(
                            saved.outcome.as_deref(),
                            saved.ceiling.as_deref(),
                            // ⛔⛔⛔⛔⛔ AND WHY A BLOCKED ONE WAS NEVER ANSWERED — register item
                            // 903. See `PersistedRun::blocked_by`.
                            saved.blocked_by.as_deref(),
                        ),
                        iterations: saved.iterations,
                        cost,
                        // ⛔⛔⛔⛔⛔ **AND WHY IT FAILED COMES BACK** — register item 903, which
                        // reversed the decision this line used to record. It read `failure: None`
                        // under *the log carries a run's SUMMARY, not its whole outcome*, and that
                        // sentence is true of a typed cause and false of a diagnosis: item 903
                        // measured 78 failed runs of which **0** could still say why, because a
                        // promotion is a daemon restart and the driver that met the failure dies
                        // with it. The moment somebody needs the reason is exactly the moment it
                        // was gone.
                        //
                        // ⚠⚠ THE SENTENCE, wrapped in `PaneError::Recorded`, whose whole doc is
                        // that this daemon did not observe it. Parsing it back into a typed cause
                        // would be inventing structure the file never held — so what crosses is
                        // what a person reads, and the type says where it came from.
                        // ⚠⚠ A BLANK IS *NOBODY WROTE IT DOWN*, filtered at the door rather than
                        // carried: a record whose column exists and holds nothing is the same
                        // claim as a record with no column, and letting the two differ would put
                        // an empty failure in front of an agent — the leak
                        // `every_pane_failure_reads_as_a_sentence_rather_than_a_rust_variant`
                        // exists to stop.
                        failure: saved
                            .failure
                            .clone()
                            .filter(|said| !said.trim().is_empty())
                            .map(sprag_plugin::PaneError::Recorded),
                        // ⚠ AND `stopped` IS STILL DROPPED, and the split is the point: that one
                        // describes a job that is STILL RUNNING somewhere, which is a claim about
                        // NOW that a dead daemon's log cannot make. This column is about a moment
                        // that is over and stays true however long ago it was.
                        stopped: None,
                        // ⚠ AND THE ANSWER TALLY IS NOT RESTORED EITHER, for a reason worth
                        // stating rather than folding into the two above: this one is a count of
                        // decisions taken on somebody's behalf, so `0` here is a claim the log
                        // cannot back. What survives a restart is the run's WORD; the durable log
                        // does not carry this column, and inventing one would be the record
                        // asserting something nobody wrote down.
                        answered: 0,
                        // ⚠ AND THE SCREENING TALLY WITH IT, on the same argument for the opposite
                        // decision: this one counts the peer's tool calls a run REFUSED, and the
                        // log has no column for it either.
                        screened: 0,
                        deferred: None,
                        // ⚠ NOR HOW MANY OF ITS DIRECTIONS NOBODY CHECKED, on `deferred`'s reason
                        // exactly — register item 847. A restored run's log has no column for it,
                        // and `None` is *nobody was counting* rather than *nothing went unchecked*.
                        unchecked: None,
                        // ⚠ NOR HOW MANY OF ITS DEFERRALS WERE REFUSALS — register item 833, on
                        // `deferred`'s reason exactly: the log has no column for it, and `None` is
                        // *nobody was counting* rather than *none of them were refused*.
                        unadmitted: None,
                        // ⚠⚠⚠ AND NOT HERE, THOUGH THE LOG NOW CARRIES IT — register item 606. The
                        // restored pair goes into `Progress` below, which is where every reader
                        // takes it from: `crate::plugins::run_to_json` publishes `delivered` out of
                        // `progress`, not out of the outcome. Filling both from one column would
                        // make two authorities on one number, which is register item 445's whole
                        // finding, and nothing would be watching them agree.
                        deliveries: sprag_plugin::Deliveries::NONE,
                        // ⚠⚠ NOR WHAT ITS CHECKS CAME TO — register item 601, and here the absence
                        // is load-bearing rather than merely honest: `asked: 0` means *nobody was
                        // meant to check this*, and a restored run must not be made to say that
                        // about a run whose checker the log never recorded. `NONE` claims nothing.
                        checks: sprag_plugin::Checks::NONE,
                        // ⚠⚠⚠⚠⚠ **AND THIS ONE IS RESTORED, WHICH IS THE WHOLE OF ITEM 616.** It is
                        // the outcome `stand_down_sentence` reads, so without it a restored run
                        // tells the person their ending cannot say what was kept — on exactly the
                        // runs anybody reads, because a run is read after it ends and its daemon
                        // is usually gone by then (item 606 measured that: thirteen live runs,
                        // every one restored). See `PersistedBanked` for why a count may cross
                        // where a state name may not.
                        banked: saved.banked.clone().map(Into::into),
                        // ⚠⚠⚠⚠ AND SO IS THIS, on the line above's argument and one of its own —
                        // register item 719. *What was that run handed?* is asked about runs that
                        // are OVER, which after a restart is all of them; a level dropped here
                        // would be readable only on the rows nobody has a question about.
                        briefed: saved.briefed.map(Into::into),
                        // ⛔⛔⛔⛔⛔ **AND SO IS THE ENDING'S WORD** — register item 706's third
                        // requirement, restored on `banked`'s argument at its strongest. A run's
                        // WALK does not survive its daemon (that item's third cost: every run
                        // before a restart held zero walk lines), and the walk's note is where
                        // this word used to live. So a restore that dropped it would leave a
                        // reader with `converged` and nothing beside it — register item 594's
                        // collapse, reappearing on exactly the rows anybody actually reads.
                        //
                        // ⚠ Owned, which is the arm of the `Cow` that exists for this line: the
                        // plugin that spelled the word is gone, and a `&'static str` cannot come
                        // out of a file. `PersistedBanked`'s `unit` crosses by the same road.
                        done_reason: saved.done_reason.clone().map(std::borrow::Cow::Owned),
                    }),
                    output: saved.output.clone(),
                    // ⛔ **CANNOT SAY, and this is the honest answer rather than a gap** — register
                    // item 682. What a tree was holding is a reading somebody took at a moment that
                    // has passed; this daemon did not take it and the log does not carry it, so
                    // `None` is the only thing it may publish. `Some(0)` here would tell a reader
                    // *nothing was left behind* about a run it never watched end.
                    uncommitted: None,
                }
            } else {
                RunState::Interrupted
            };
            self.next_id = self.next_id.max(saved.id + 1);
            self.runs.push(RunRecord {
                id: RunId(saved.id),
                label: saved.label.clone(),
                // ⚠⚠ NOT KNOWN, AND NOT GUESSED — items 539 and 597. The log records what became
                // of the run, not the request that started it, and the label is prose. A restored
                // run has no driver either, so an order over it is refused as `NoDriver` whatever
                // plugin it once was — this `None` costs a reader nothing they could have used.
                //
                // ⚠⚠ AND IT IS STILL `None` THOUGH THE REQUEST BELOW MAY NAME ONE, which is not an
                // oversight but the same rule one line down: this record describes a run with no
                // driver, and the word is read by `orderable` to say what an order would
                // reach. What names the plugin again is `crate::plugins::PluginsExternal::put_back`
                // — the moment a driver exists to be named.
                plugin: None,
                // ⚠⚠⚠⚠⚠ **AND HERE IS WHAT MAKES A RESTORED RUN MORE THAN A HEADSTONE** — register
                // item 543's sixth brick. Through the door and only the door: `resumable_request`
                // hands this back exactly when the run is unfinished AND its place is spelled in
                // THIS image's documents, so a request that survived beside a foreign
                // configuration is dropped here rather than carried to a boot that would build a
                // plugin and start it from the top.
                request: saved.resumable_request().cloned(),
                // ⚠ THE SEAT IS DROPPED AND THE CONVERSATION IS KEPT — rule 1 above. A successor
                // cannot know who is sitting in pane 3; it can know which conversation asked, and
                // `crate::plugins` re-derives the seat from that at read time.
                opened_by: None,
                opened_by_session: saved.opened_by_session.clone(),
                // ⛔⛔⛔⛔⛔ AND THE TREE COMES BACK — register item 890, and it is the seat's
                // OPPOSITE: a pane id is this daemon's answer and does not survive, but the
                // directory a run worked in is a fact about the world that does. Item 606 measured
                // that every run anybody reads has been restored, so a tree that stopped here
                // would name a repository only while nobody was asking.
                //
                // ⚠ An older log answers `None`, which is *nobody recorded which tree* — never
                // *this run had none*, and never a guess re-derived from a pane that has since
                // been reused (register item 887's shape, one field over).
                tree: saved.tree.clone(),
                state: Arc::new(Mutex::new(state)),
                // ⚠⚠⚠ A RESTORED RUN HAS NO DRIVER, AND THAT IS NOW SAID BY A TYPE — see
                // `EndedRun`. Three fresh `AtomicBool`s used to sit here, each with a comment
                // explaining that setting it did nothing because *"the worker that would have read
                // it died with its daemon"*: a hold is a level somebody is CURRENTLY holding and
                // nobody can be holding a run that is not moving, and persisting an order would let
                // a restart resurrect an instruction nobody could act on. Those sentences were
                // right and were the only thing enforcing them.
                //
                // ⚠⚠⚠⚠ **AND THE STAND-DOWN IS THE ONE EXCEPTION, WHICH IS NOT A CRACK IN THAT
                // RULE BUT ITS OTHER HALF** — item 594. Those sentences are about resurrecting an
                // INSTRUCTION, and nothing here can: `EndedRun` accepts every order and delivers
                // none. What comes back is the RECORD that somebody gave one, which is the only
                // thing that can explain the ending a reader is looking at. An absent field reads
                // `false` — a log written before this existed cannot be made to say a person spoke.
                run: Box::new(
                    EndedRun::restored(
                        saved.stood_down.unwrap_or(false),
                        // ⚠⚠⚠ ITEM 596. Without this the ONE canceller a person ever meets after a
                        // restart would be unanswerable: `Shutdown` is raised by a daemon that then
                        // exits, so the only daemon left to be asked is this one.
                        saved.cancelled_by,
                        // ⚠⚠⚠ ITEM 526: the process the dead daemon had driving it, which may still be
                        // ALIVE — a driver outlives the daemon that spawned it (item 544's stage 1),
                        // and the boot has to know before it starts a second one over the same pane.
                        saved.driver,
                    )
                    // ⛔⛔⛔⛔⛔ AND WHO GAVE THE STAND-DOWN — register item 835. This is the crossing
                    // that decides whether the item is paid at all: the run another supervisor reads is
                    // a RESTORED one (item 606 measured thirteen live runs, every one restored), so an
                    // orderer that died with its daemon would leave every reader in exactly the state
                    // that had a stopped run re-launched twice.
                    .ordered_by(saved.stood_down_by.clone()),
                ),
                progress: Arc::new(Mutex::new(Progress {
                    iterations: saved.iterations,
                    cost,
                    // ⛔ AND THE CEILING IT RAN UNDER — register item 856(1b), carried across the
                    // restart on the ending's terms above: a restored run's row must still say
                    // which experiment it was, or the restart itself becomes the thing that erases
                    // the distinction.
                    context_ceiling: saved.context_ceiling,
                    // ⛔ AND HOW FULL ITS SESSION GOT — register item 894, carried across the
                    // restart on the line above's terms and for the same reason item 606 measured:
                    // every run anybody reads is a restored one, so a peak that stopped at the
                    // daemon boundary would be the left-hand side of a comparison nobody can make.
                    context_high_water: saved.context_high_water,
                    // ⚠⚠⚠⚠⚠ **THE PLACE IS CARRIED FORWARD, AND ONLY THROUGH THE DOOR THAT CHECKS
                    // THE DOCUMENT** — register items 543 and 544. `saved.place` is words from
                    // whatever build wrote the log; `resumable_place` hands them back only when
                    // they came from the documents THIS image compiled. A restored run that
                    // carried a foreign configuration would look resumable and place a machine
                    // somewhere nobody chose, which is worse than the honest `interrupted` it
                    // comes back as today.
                    place: saved.resumable_place().map(<[String]>::to_vec),
                    // ⚠ THE JOURNAL IS NOT PERSISTED. It is the per-step account of a run that is
                    // over and unresumable, and keeping it would grow the file with every step of
                    // every run this daemon ever ran. The totals survive; the steps do not.
                    journal: Vec::new(),
                    // ⚠ NOR IS THE ANSWER TALLY, for `Outcome::answered`'s reason at this end too:
                    // the durable log has no column for it, and `0` would be this record asserting
                    // that a restored run approved nothing when nobody wrote that down.
                    answered: 0,
                    // ⚠ Nor the count of calls it refused, for the same reason.
                    screened: 0,
                    deferred: None,
                    // ⚠ Nor how many of its directions nobody checked (register item 847), for the
                    // reason on the line above it.
                    unchecked: None,
                    // ⚠ Nor how many of its deferrals were refusals (register item 833), for the
                    // reason on the line above it.
                    unadmitted: None,
                    // ⛔⛔⛔ AND NOTHING IS WAITED ON BY A RUN NOBODY IS DRIVING — register item
                    // 755. A restored run has no driver asking its plugin anything, so *is a
                    // person needed* has no answerer; `None` is the honest reading and the same one
                    // `journal` and `answered` above take. ⚠ It is a LEVEL a live driver republishes
                    // on its next step, so a run that is put back and resumes says so at once — and
                    // one that never resumes never claims somebody is waiting on it.
                    waiting: None,
                    // ⚠⚠⚠⚠⚠ AND WHAT ITS DELIVERIES CAME TO **IS** RESTORED — register item 606.
                    // This used to say the log had no column for it, which was true and was the
                    // reason item 599 could not be answered by looking: measured on this machine,
                    // thirteen live runs across two daemons and not one carried the pair, because
                    // every one of them had been restored. A run is READ after it ends, and the
                    // daemon that drove it is restarted between rounds.
                    //
                    // ⚠⚠ A RECORD, NOT AN ORDER, which is what separates this from a hold: how
                    // many prompts a finished run typed is a fact about what already happened, and
                    // it is the only thing that explains a pane that looks empty. An older log
                    // still reads as `NONE`, which claims nothing.
                    //
                    // ⛔⛔⛔⛔⛔ **AND `NONE` IS NOT WHAT AN OLDER LOG READS AS ANY MORE** —
                    // register item 891. `map_or(…::NONE, …)` was this hop's half of the
                    // laundering: it turned *the file had no column* into a table of zeros, and
                    // [`counted`] then signed those zeros on the way back out. `map` keeps the
                    // absence, and every reader of the cell already had an arm for it.
                    deliveries: saved.deliveries.map(Into::into),
                    // ⛔⛔⛔ AND THE SPLIT OF THE SAME FOLDS IS RESTORED WITH IT — register item
                    // 856(1), on `deliveries`' argument exactly and for the reason item 606
                    // MEASURED: the runs anybody reads are restored ones, so a split that stopped
                    // at the daemon boundary would be an instrument nobody could ever consult.
                    //
                    // ⚠ An older log reads as every row `0 of 0`, which publishes no sentence at
                    // all (`is_empty`) rather than a clean bill. That is the honest reading of a
                    // file whose writer could not count this, and it is the same shape the pair
                    // above takes.
                    // ⛔⛔⛔ AND ITS ABSENCE IS KEPT ON `deliveries`' TERMS — register item 891.
                    folds_by_reason: saved.folds_by_reason.clone().map(Into::into),
                    // ⛔⛔⛔⛔⛔ AND WHAT PROVED EACH DELIVERY IS RESTORED WITH THEM — register item
                    // 856, on the two arguments above and for the sharpest instance of them: a
                    // LANDING is only ever read off a finished run, and item 606 measured that
                    // every run anybody reads has been restored. A road table that stopped at the
                    // daemon boundary would be the third instrument that cannot count a landing.
                    //
                    // ⚠ An older log reads as every road `0`, which publishes nothing at all
                    // (`is_empty`) rather than *this run landed none*.
                    // ⛔⛔⛔ AND ITS ABSENCE IS KEPT ON `deliveries`' TERMS — register item 891.
                    delivered_by_road: saved.delivered_by_road.clone().map(Into::into),
                    // ⛔⛔⛔⛔⛔ AND WHICH SENTENCE EACH PROMPT WAS IS RESTORED WITH THEM — register
                    // item 889, on the three arguments above and for the sharpest instance of
                    // them: the rate this table publishes is only meaningful compared across many
                    // finished runs, and item 606 measured that every run anybody reads has been
                    // restored. A table that stopped at the daemon boundary would leave the 15×
                    // exactly where item 889 found it — in a person's reading of log files.
                    //
                    // ⚠ An older log reads as every sentence `0 of 0`, which publishes nothing at
                    // all (`is_empty`) rather than *every prompt of this run was asked*.
                    // ⛔⛔⛔ AND ITS ABSENCE IS KEPT ON `deliveries`' TERMS — register item 891.
                    said_by_sentence: saved.said_by_sentence.clone().map(Into::into),
                    // ⛔⛔⛔⛔⛔ AND WHAT THE WIDTH WOULD HAVE WITHHELD IS RESTORED WITH THEM —
                    // register item 866(2), on the four arguments above and for the sharpest
                    // instance of them: this tally answers *is this build still reading logical
                    // lines*, and that question is only asked of runs that have ENDED — every one
                    // of which has been through a restore (item 606). A column that stopped at the
                    // daemon boundary would answer it for no run at all.
                    //
                    // ⚠ An older log reads as `None`, which claims nothing — and specifically
                    // does NOT claim *this run's answers all fitted on one row*, which is the
                    // reading a regressed build would produce.
                    // ⛔⛔⛔ AND ITS ABSENCE IS KEPT ON `deliveries`' TERMS — register item 891.
                    width_withheld: saved.width_withheld.map(Into::into),
                    // ⚠ NOR WHAT ITS CHECKS CAME TO — register item 601, on the same argument.
                    checks: sprag_plugin::Checks::NONE,
                    // ⚠⚠⚠⚠⚠ AND HOW MUCH OF ITS WORK IS KEPT **IS** RESTORED — register item 616,
                    // `deliveries` above's argument and one of its own. The position two comments
                    // down is NOT restored because a state name means what a `.scxml` says it
                    // means; a count of completed turns means the same thing in any vocabulary,
                    // and `"turn"` is a plain noun rather than a document symbol. An older log
                    // still reads as `None`, which claims nothing.
                    banked: saved.banked.clone().map(Into::into),
                    // ⚠⚠⚠⚠ AND SO IS THE BRIEF'S SIZE — register item 719, on the line above's
                    // argument. It is a level that never moved, so restoring it is restoring the
                    // whole of what it ever said.
                    briefed: saved.briefed.map(Into::into),
                    // ⚠⚠⚠ AND NOT WHICH PANE IT WAS DRIVING — register items 540 and 595, and here
                    // the absence is the CORRECT answer rather than a lossy one: this run's driver
                    // died with its daemon, so nothing is driving that pane now. Restoring a pane
                    // id would say the opposite of what item 595 exists to make visible.
                    driving: None,
                    // ⚠⚠⚠⚠⚠ AND THE POSITION IS NOT RESTORED INTO THE LIVE CELL, WHICH IS THE
                    // POINT OF THE FINGERPRINT RATHER THAN A GAP IN IT — register items 543, 544.
                    // `Progress::at` is `&'static str`: a word from THIS binary's documents. The
                    // saved one is a `String` from the DEAD daemon's, and the two are only the
                    // same fact when the fingerprints agree — which is a question for a reader
                    // holding both, not an assumption to bake in here by leaking a stale word
                    // into a live cell that everything treats as *where this run is now*.
                    // `PersistedRun::at` keeps the record; `resumable_here` is where the two are
                    // compared, once, by something that can see both.
                    at: None,
                    // ⛔⛔⛔⛔⛔ **AND EVERY NUMBER ABOVE IS A DEAD DAEMON'S, WHICH IS NOW SAID** —
                    // register item 815, and the ONE site that sets this.
                    //
                    // ⚠⚠ Restoring `iterations` and `deliveries` is right (items 606 and 616: a run
                    // is read after it ends, and a row of zeros would claim it typed nothing). What
                    // was missing is the word for WHOSE they are, and the missing word has a cost
                    // the moment a boot puts the run back: item 774's clause reads an absent
                    // delivery count as *nothing has been typed since*, so a restored count
                    // silences it on exactly the run that item was filed over.
                    //
                    // ⚠ It is cleared by the first step any driver takes — `sprag_plugin::Driver`
                    // republishes this cell whole — so nothing here has to remember to unset it.
                    inherited: true,
                })),
                // ⚠⚠ NOTHING REPORTED, and here that is the CORRECT answer rather than a lossy
                // one — `driving: None`'s argument two comments up. A restored run's driver died
                // with its daemon, whichever kind it was, so nothing is going to say what it is
                // doing now. What it MANAGED is above, taken from the log.
                reported: Arc::new(Mutex::new(None)),
                // ⚠⚠⚠ AND THIS ONE IS TAKEN FROM THE LOG RATHER THAN STAMPED, which is the
                // opposite decision to every field above and the reason the field exists. The rest
                // of this record is about a run that is over, so inventing a value would assert
                // something nobody wrote; the BUILD was written down, by the image that actually
                // drove it. Stamping this daemon's here would date a dead daemon's work to its
                // successor — which is precisely the confusion register item 438 was filed for.
                build: saved.build.clone(),
                // ⛔⛔⛔⛔⛔ AND SO IS THIS ONE, FOR THE LINE ABOVE'S REASON AT ITS SHARPEST —
                // register item 887. Minting a fresh stamp here would give a restored run a new
                // identity on every boot, so *the same run* and *a different run* would read alike
                // to anybody comparing across a restart — and a restart is the only moment the
                // number it is qualifying can go wrong.
                //
                // ⚠ [`None`] for a log written before the field existed, and it stays `None`: this
                // daemon did not mint that run and has nothing true to say about which run it was.
                which_run: saved.which_run.clone().map(WhichRun::said),
                // ⚠⚠ NOT CARRIED OVER, AND THAT IS THE CORRECT ANSWER — register item 671. The
                // count is *what THIS daemon has been told*, and it has been told nothing about a
                // run it is reading out of a file; the watermark beside it is what THIS daemon has
                // already tried, and it has tried nothing. A boot's own `put_back` is a different
                // act from reviving a driver that died under a living daemon, and starting this
                // record at zero is what keeps the two from being counted as one.
                reports: AtomicU64::new(0),
                revived_at: None,
                // ⚠⚠⚠⚠⚠ **AND WHAT THIS READING KEPT OUT, KEPT** — register item 737. Every field
                // above takes the log's answer or refuses it; this is the refusal itself, held
                // because the two lines that refuse it (`resumable_request` above and
                // `resumable_place` below) drop the evidence they judged. A boot that puts nothing
                // back can now say whether there was nothing to put back or whether a promotion
                // took the documents out from under all of it.
                withheld: saved.withheld(),
                // 🎯🎯🎯🎯🎯 AND WHICH AUTHORS ITS NUMBERS CAME FROM, WHICH THE LOG NOW HOLDS —
                // register item 859, closing the residue item 853's entry stated and this line
                // used to BE. It read `None`, on the ground that the answer was re-derivable from
                // the request; measured 2026-09-05, no ended run keeps a request either, so the
                // fact was not re-derivable, it was gone — 220 of 220 rows.
                //
                // ⛔⛔⛔ ONE AUTHOR AND NOT TWO, which item 859's own done-when spells out: the log
                // answers, so `PluginsExternal::put_back` must NOT re-parse the request for it.
                // Two parties answering one question is the disease item 853 was filed to cure,
                // and a restore that re-derived would disagree with the log the moment a caller's
                // spelling and this build's resolver did.
                overridden: saved
                    .overridden
                    .as_deref()
                    .and_then(crate::plugins::Overridden::restored),
                // ⚠⚠ AND NOTHING HAS BEEN ENDED YET — register item 740. This reads the log; the
                // boot that acts on it (`put_back_inherited_runs`) is a later call with the socket
                // in hand, and this field is its answer rather than the file's. A record that
                // arrived here claiming a process had been dealt with would be asserting something
                // no reading can know.
                ended_driver: None,
                // ⛔⛔⛔⛔⛔ **AND THE PANE THE RUN WAS ON IS KEPT, WHERE `Progress::driving` ABOVE
                // REFUSES IT** — register item 771. That cell is a level about NOW and nothing is
                // driving now; this is a record of where the work was, which is the only thing that
                // lets a boot put a loop that replaced its session back where it actually is.
                // `InheritedRun::pane` is its one reader.
                drove: saved.driving.map(PaneId),
                // ⚠⚠ AND NOTHING HAS BEEN TRIED YET — register item 771, `ended_driver` above's
                // argument verbatim. This reads the log; `put_back_inherited_runs` is what learns
                // whether a driver could be stood up, and a record that arrived here already
                // claiming a reason would be the file answering a question only a boot can ask.
                not_resumed: None,
                // ⚠ AND IT HAS NOT BEEN PUT BACK EITHER — register item 774, the line above's
                // argument exactly: this record is a LOG being read, and `put_back` is the only
                // act that can answer it.
                resumed: false,
            });
        }
    }

    /// A snapshot of every run, in submit order.
    #[must_use]
    pub fn snapshot(&self) -> Vec<RunSummary> {
        self.runs
            .iter()
            .map(|record| RunSummary {
                id: record.id,
                label: record.label.clone(),
                opened_by: record.opened_by,
                opened_by_session: record.opened_by_session.clone(),
                state: lock(&record.state).clone(),
                progress: lock(&record.progress).clone(),
                // ⚠ SAME PASS as the two above, for their reason: a row weighs what a run has done
                // against where it has got to, and reading them a moment apart is this
                // repository's *비교하는 두 값은 같은 순간에* rule broken at its cheapest.
                reported: lock(&record.reported).clone(),
                build: record.build.clone(),
                // ⛔⛔⛔ AND WHICH RUN IT IS — register item 887, republished beside the build so a
                // reader holding a row can say whether it is the run their own record is about.
                which_run: record.which_run.clone(),
                // ⛔⛔⛔ AND WHICH TREE IT WAS FOR — register item 890, beside the two above for
                // their reason: a reader holding a row asks *whose run is this* in three senses
                // (which run, which code, which repository) and only two of them could be answered.
                tree: record.tree.clone(),
                // ⚠ ASKED OF THE HANDLE, on the same pass that reads the state — item 594's
                // sentence weighs the two against each other, and reading them a moment apart is
                // this repository's *비교하는 두 값은 같은 순간에* rule at its cheapest.
                stood_down: record.run.stood_down(),
                // ⛔⛔⛔ AND WHO GAVE IT, ON THE SAME PASS — register item 835, for the line above's
                // reason: the flag and the orderer are ONE fact read two ways, and the sentence
                // that renders them weighs the pair. Read a moment apart, a row could say an order
                // stands and name nobody, which is the state item 835 was filed on.
                stood_down_by: record.run.stood_down_by(),
                // ⚠ SAME PASS AGAIN — item 699. A hold read a moment later than the state would let
                // a row say *running, and nobody is holding it* about a run that was held between
                // the two reads, which is the one moment a person is watching for.
                held: record.run.held(),
                // ⛔⛔⛔⛔⛔ WHOSE DECISIONS IT RUNS UNDER — register item 870, read off the
                // REQUEST because that map is already the one authority on what this run was asked
                // with (it is what a successor puts the run back from). See the field.
                loop_kind: record
                    .request
                    .as_ref()
                    .and_then(|asked| asked.get(crate::plugins::LOOP_KIND_KEY))
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned),
                // ⚠ SAME PASS, SAME REASON — item 596. The sentence a mouth prints weighs this
                // against `state`, so the two must not be read a moment apart either.
                cancelled_by: record.run.cancelled_by(),
                // ⚠ A LEVEL THAT NEVER MOVES — item 737. It is decided once, by the boot that read
                // the log, and a row that showed it changing would be showing a fact about the
                // reading rather than about the run.
                withheld: record.withheld.clone(),
                // 🎯 AND SO IS THIS — item 853, on the line above's argument exactly: it is decided
                // by the submit and nothing later can change which authors a run was started under.
                overridden: record.overridden.clone(),
                // ⚠ A LEVEL THAT NEVER MOVES EITHER — item 740, on the line above's argument. A
                // boot ends a leftover once and writes it here once; a row that showed this
                // appearing and going away would be reporting on the daemon, not on the run.
                ended_driver: record.ended_driver,
                // ⚠ AND SO IS THIS — item 771, on the line above's argument: a boot decides it once,
                // writes it once, and nothing later takes it away.
                not_resumed: record.not_resumed.clone(),
                // ⚠ ITS TWIN, item 774: `not_resumed` says why a run stayed behind, this says one
                // came back. A boot writes it once and nothing takes it away — see the field.
                resumed: record.resumed,
            })
            .collect()
    }

    /// Join every outstanding worker, waiting at most `within` FOR THE LOT, and answer the runs that
    /// did not come back in time.
    ///
    /// Called on host shutdown so threads and their child processes reap promptly. Raise
    /// [`cancel_all`](Self::cancel_all) first: this waits for workers, it does not ask them to stop.
    ///
    /// # ⚠⚠⚠⚠ Why a deadline, when a run always honours its cancel flag
    ///
    /// Because *always* is a property of the run's own loop and not of the thread. A worker parked
    /// in a syscall never reaches a loop top, never reads the flag, and never returns — and this is
    /// called from [`Drop`], which can neither fail nor panic, so an unbounded join there is a
    /// process that cannot be shut down. That is exactly what happened: one pane's blocked `write(2)`
    /// held a build machine for 43 hours with ten workers queued behind it (register items 304, 305).
    /// The write is bounded now; the shape of *a thread that will not come back* is not, and this is
    /// the answer to it rather than to that one cause.
    ///
    /// # ⚠⚠⚠ What a caller is promised, and what it is not
    ///
    /// Every worker that comes back within `within` is JOINED — reaped, with a panicking one turned
    /// into [`RunState::Panicked`], exactly as [`sweep`](Self::sweep) does (it IS `sweep`, on a
    /// timer). A worker that does not is left where it is: **its id is returned and its thread is
    /// DETACHED**, since dropping the registry drops the handle. Such a worker keeps its pane and
    /// its child alive until the process exits — which both real callers do immediately. That is
    /// the residue of choosing a deadline, and it is smaller than the alternative, which is a
    /// daemon that never dies.
    ///
    /// ⚠⚠⚠ **AND ITS ENDING IS STILL ITS OWN — NOTHING HERE STAMPS ONE ON IT.** A terminal state
    /// written for a thread that is still stepping would be a lie about a live worker, and it would
    /// race the only author there is: a worker publishes its outcome as its last act, so a stamped
    /// ending is either overwritten a moment later or overwrites the real answer. The record stays
    /// `Running`, which is what makes the durable log's story true — unfinished on disk, and
    /// [`RunState::Interrupted`] when a successor daemon reads it back.
    ///
    /// ⚠⚠ THE DEADLINE IS OVER THE WHOLE SET AND NOT PER WORKER — `n` wedged runs must not cost `n`
    /// deadlines — and every outstanding worker is asked on every pass, so one that will not come
    /// back cannot starve one that would have.
    pub fn join_all_within(&mut self, within: Duration) -> Vec<RunId> {
        let all_back = join_until(within, || {
            self.sweep();
            // ⚠ ASKED, not collected: the answer is built once, on the way out, rather than
            // allocated on each of the thousand passes a full deadline takes.
            self.runs.iter().any(|record| record.run.outstanding())
        });
        if all_back {
            Vec::new()
        } else {
            self.detached(within)
        }
    }

    /// ⛔⛔⛔⛔⛔ **ASK EVERY RUN TO STOP AND WAIT FOR THEM, WITHOUT HOLDING THIS REGISTRY SHUT** —
    /// what a daemon on its way out calls, and the second half of register item 664.
    ///
    /// # ⚠⚠⚠⚠⚠ Why the lock is taken per pass instead of once around the whole shutdown
    ///
    /// [`cancel_all`](Self::cancel_all) now publishes, so a run driven in a process of its own is
    /// WOKEN — and what it is woken to do is **ask this daemon for its row**, because that row is
    /// where its orders are written. A shutdown that held the registry lock across the join would
    /// be a daemon that cannot answer the one question it has just asked its drivers to ask: the
    /// `runs` slot blocks, the driver learns nothing, and the wake buys exactly nothing.
    ///
    /// Measured on 2026-08-25: with the announce in place and the lock held, a signalled daemon
    /// still cost the full [`JOIN_DEADLINE`](Self::JOIN_DEADLINE).
    ///
    /// ⚠⚠ **THE ORDER IS THE POINT AND IT IS HELD HERE**, not at the call site. `install_shutdown`'s
    /// own doc used to say the two lines that matter are an ORDER *"that nothing outside this file
    /// can observe"* — a binary is not a place a rule can be gated, and this is.
    ///
    /// ⚠ [`Drop`] does not use this and cannot: it holds `&mut self` and there is no `Mutex` left
    /// to lock. Its residue is unchanged and smaller — a registry being dropped has no daemon left
    /// to serve anybody's row.
    pub fn stop_all_within(shared: &Arc<Mutex<Self>>, within: Duration) -> Vec<RunId> {
        // ⚠ THE ASK, under the lock and then released: this is where the announcements are raised,
        // and every one of them is a driver about to come back with a question.
        lock(shared).cancel_all();
        let all_back = join_until(within, || {
            let mut held = lock(shared);
            held.sweep();
            held.runs.iter().any(|record| record.run.outstanding())
        });
        if all_back {
            Vec::new()
        } else {
            lock(shared).detached(within)
        }
    }

    /// The runs whose driver is still uncollected, each named in the log — what a spent deadline
    /// leaves behind, and the sentence [`join_all_within`](Self::join_all_within) and
    /// [`stop_all_within`](Self::stop_all_within) must not spell twice.
    fn detached(&self, within: Duration) -> Vec<RunId> {
        let outstanding: Vec<RunId> = self
            .runs
            .iter()
            .filter(|record| record.run.outstanding())
            .map(|record| record.id)
            .collect();
        for id in &outstanding {
            tracing::warn!(
                target: "sprag_host::runs",
                "run {} did not come back within {within:?}; its worker is left running",
                id.0,
            );
        }
        outstanding
    }
}

/// **WAIT UNTIL NOTHING IS OUTSTANDING, OR UNTIL `within` IS SPENT**, answering whether everything
/// came back — the loop [`RunRegistry::join_all_within`] and [`RunRegistry::stop_all_within`] share.
///
/// The two differ in ONE thing — whether the registry lock is held across the wait — so what
/// crosses is a closure and not a receiver. It answers a BOOL rather than the outstanding runs
/// because naming them costs an allocation and a full deadline is a thousand passes; the caller
/// names them once, on the way out.
fn join_until(within: Duration, mut any_outstanding: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + within;
    loop {
        if !any_outstanding() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(RunRegistry::JOIN_POLL);
    }
}

impl Drop for RunRegistry {
    fn drop(&mut self) {
        // Catch-all: no run thread outlives the registry BY MORE THAN ITS DEADLINE (so no detached
        // worker keeps a pane/child alive for longer than that). Cancel first so an in-flight run
        // aborts promptly rather than the join waiting on it (e.g. a slow AI turn). `serve` also
        // does this for deterministic shutdown; the take() / flag make both idempotent.
        //
        // ⚠⚠⚠ THE BOUND IS THE WHOLE POINT AND NOT A TIDY-UP. `Drop` can neither return an error
        // nor panic, so a worker that will not come back used to mean a process that could not be
        // shut down; the runs that outlast the deadline are named in the warning
        // `join_all_within` logs and detached. See its doc for what that costs.
        self.cancel_all();
        let _ = self.join_all_within(Self::JOIN_DEADLINE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠⚠ **EVERY TERMINAL STATE SURVIVES THE ROUND TRIP THROUGH ITS OWN WORDS** — the property
    /// the run log rests on, over the whole type rather than the one case the reboot gate drives.
    ///
    /// `a_run_whose_daemon_died_is_reported_as_interrupted_and_belongs_to_nobody` drives a run that
    /// was STILL GOING, which never touches this path: a run that had FINISHED comes back through
    /// `outcome_from_words`, and nothing was reading it back. A writer that quotes and a reader
    /// that unquotes are stated as inverses (R350's rule) — here that is an equality over
    /// `Ceiling`'s three arms and the four states, so a fifth of either fails this rather than
    /// silently reloading as something else.
    #[test]
    fn every_outcome_survives_the_round_trip_through_its_own_words() {
        use sprag_plugin::{Ceiling, OutcomeState};
        // ⚠⚠ THE CEILINGS ARE WALKED, NOT LISTED. They were spelled out here, and a fourth added
        // to the type would have been round-tripped by nothing while this gate went on passing —
        // which is exactly what happened to `outcome_from_words`, whose hand-written match
        // silently restored an unknown ceiling as `iterations`.
        let every: Vec<OutcomeState> = [
            OutcomeState::Converged,
            OutcomeState::Cancelled,
            OutcomeState::Failed,
        ]
        .into_iter()
        .chain(Ceiling::ALL.map(OutcomeState::Exhausted))
        .collect();
        for state in every {
            let outcome = Outcome {
                state: state.clone(),
                iterations: 3,
                cost: None,
                failure: None,
                stopped: None,
                answered: 0,
                screened: 0,
                deferred: None,
                unchecked: None,
                unadmitted: None,
                deliveries: sprag_plugin::Deliveries::NONE,
                checks: sprag_plugin::Checks::NONE,
                banked: None,
                briefed: None,
                // ⚠ This gate is about the outcome WORD, and every ending it walks is one no
                // plugin names a reason for — see `Outcome::done_reason`.
                done_reason: None,
            };
            let read_back = crate::plugins::outcome_from_words(
                Some(crate::plugins::outcome_word(&outcome)),
                crate::plugins::outcome_ceiling(&outcome),
                None,
            );
            assert_eq!(
                read_back, state,
                "a {state:?} written to the run log must come back as itself",
            );
        }

        // ⚠ AND AN UNREADABLE PAIR IS `Failed`, never a happier guess: a record this build cannot
        // parse must not be reported as having converged.
        assert_eq!(
            crate::plugins::outcome_from_words(Some("a word from a newer build"), None, None),
            OutcomeState::Failed,
        );
        assert_eq!(
            crate::plugins::outcome_from_words(None, None, None),
            OutcomeState::Failed,
        );
    }

    /// ⛔⛔⛔⛔⛔ **A FAILED RUN'S REASON SURVIVES ITS DAEMON, AND A BLOCKED ONE'S DOES TOO** —
    /// register item 903's done-when ⑴, and the round trip that makes it true.
    ///
    /// # ⛔⛔⛔⛔⛔ What was lost, and why nothing could ever have found it
    ///
    /// The sentence a failed run reports is composed by the driver that MET the failure
    /// (`sprag_plugin::PaneError`'s `Display`), and that process dies with its daemon — while a
    /// promotion IS a daemon restart. **Measured 2026-09-05T04:59:20Z over the loop's own store**
    /// (a live store: counting again takes a NEW reading rather than checking this one): 228 runs,
    /// **78 `failed`**, of which `done_reason` 0, `output` 0, `request` 0, `ceiling` 0. The ending
    /// WORD survived and the narrative did not — so the loop's post-mortems were impossible in
    /// principle rather than merely neglected.
    ///
    /// ⚠⚠ **THE QUESTION IS STILL NOT RESTORED**, and this gate asserts that too: a blocked run's
    /// question was read off a pane a restart has outlived, and only the REFUSAL — a word out of a
    /// closed set of eleven, about what THIS host could not do — crosses.
    #[test]
    fn the_reason_a_run_failed_or_blocked_comes_back_out_of_the_log() {
        use sprag_plugin::consent::{Refusal, Unanswered};
        use sprag_plugin::{OutcomeState, PaneError};

        // ── ① A FAILURE SENTENCE CROSSES WHOLE ───────────────────────────────────────────────
        let met =
            PaneError::Undrivable("the document reached a state this build has none for".into());
        let said = met.to_string();
        let recorded = PaneError::Recorded(said.clone());
        assert_eq!(
            recorded.to_string(),
            said,
            "⛔⛔⛔⛔⛔ REGISTER ITEM 903: a sentence read back out of a log must render as the \
             words that were written, or a row that survived two restarts doubles its own prefix",
        );

        // ── ② AND A BLOCKED RUN'S REFUSAL DOES, THROUGH THE WORD AND NOT THE QUESTION ────────
        for why in Refusal::ALL {
            let back =
                crate::plugins::outcome_from_words(Some("blocked"), None, Some(why.wire_str()));
            assert_eq!(
                back,
                OutcomeState::Blocked(Some(Unanswered::recorded(why))),
                "⛔⛔⛔⛔ REGISTER ITEM 903: every refusal this build knows has to survive the \
                 log, and the list is WALKED rather than typed — a twelfth arm added to `Refusal` \
                 must not come back as *blocked, and nobody wrote down why*",
            );
            let OutcomeState::Blocked(Some(carried)) = back else {
                unreachable!("the assertion above settled the shape")
            };
            assert!(
                carried.question().is_none(),
                "⚠⚠⚠ AND THE QUESTION MUST NOT: it was read off a pane this restart has outlived, \
                 and republishing it would be a claim about a screen nobody has looked at since. \
                 That is `outcome_from_words`' own standing argument and this half keeps it",
            );
        }

        // ── ③ AND THE WHOLE OF IT SURVIVES AN ACTUAL RESTART, ROUND TRIP AND ALL ────────────
        //
        // ⛔⛔⛔⛔⛔ THIS ARM EXISTS BECAUSE THE FIRST DRAFT OF THIS GATE DID NOT HAVE IT, and a
        // mutation proved the hole: dropping the restore of `failure` altogether left the two arms
        // above GREEN, because they measure the pieces and never drove the door. That is the shape
        // item 868 records — *build an instrument and point it at the real subject* — and it is
        // exactly the defect item 903 is about, one level up.
        //
        // ⚠⚠ READ BACK THROUGH `persistable`, which is the ROUND TRIP and not a peek: a sentence
        // that came out of the log and did not go back into it would be lost at the NEXT restart,
        // and surviving one hop and not the next is this fact's whole defect.
        let saved: RunLog = serde_json::from_str(
            r#"{"version":1,"runs":[
                {"id":7,"label":"ai_loop pane=3","iterations":9,"cost":null,"unit":null,
                 "finished":true,"outcome":"failed","ceiling":null,"output":null,
                 "failure":"this run's machine could not be driven on: no effect for that state"},
                {"id":8,"label":"ai_loop pane=4","iterations":3,"cost":null,"unit":null,
                 "finished":true,"outcome":"blocked","ceiling":null,"output":null,
                 "blocked_by":"unreadable"},
                {"id":9,"label":"ai_loop pane=5","iterations":1,"cost":null,"unit":null,
                 "finished":true,"outcome":"failed","ceiling":null,"output":null}]}"#,
        )
        .expect("a log naming the reasons parses, and so does one that does not");
        let mut successor = RunRegistry::default();
        successor.restore(&saved);
        let back = successor.persistable();
        let reasons: Vec<(Option<&str>, Option<&str>)> = back
            .runs
            .iter()
            .map(|run| (run.failure.as_deref(), run.blocked_by.as_deref()))
            .collect();
        assert_eq!(
            reasons,
            vec![
                (
                    Some("this run's machine could not be driven on: no effect for that state"),
                    None
                ),
                (None, Some("unreadable")),
                (None, None),
            ],
            "⛔⛔⛔⛔⛔ REGISTER ITEM 903: a failed run's sentence and a blocked run's refusal have \
             to cross a daemon restart AND go back into the next log, or the diagnosis dies at the \
             promotion that made somebody want it. The third row is the control — a log that wrote \
             neither must not gain one. Got: {reasons:?}",
        );

        // ── ④ AND AN UNKNOWN WORD IS *NOBODY SAID*, never a nearest guess ────────────────────
        assert_eq!(
            crate::plugins::outcome_from_words(
                Some("blocked"),
                None,
                Some("a word from a newer build")
            ),
            OutcomeState::Blocked(None),
            "⚠⚠ A refusal this build cannot read must land where every blocked run landed before \
             this column existed. Naming the closest arm would send somebody to fix a thing that \
             was never wrong",
        );
        assert_eq!(
            crate::plugins::outcome_from_words(Some("blocked"), None, None),
            OutcomeState::Blocked(None),
            "⚠ AND THE CONTROL: an older log carries no such column and must still read as blocked",
        );
    }

    /// ⛔⛔⛔⛔⛔ **EVERY ENDING SAYS WHY, EACH IN ITS OWN COLUMN — AND THE COMPILER HOLDS THE
    /// LIST** — register item 903's done-when ⑶, in the shape the measurement supports rather than
    /// the one the item first wrote down.
    ///
    /// # ⛔⛔⛔⛔⛔ The item's own prescription was disproved by re-measuring it
    ///
    /// Item 903 asked for `done_reason` to be *attached to every ending*, on the reading that a
    /// field set for one ending gives the illusion that a reason exists. The first half is right and
    /// the prescription is not, and [`sprag_plugin::Outcome::done_reason`]'s own doc says why: it
    /// means **the plugin named this ending out of a closed vocabulary it holds**, and reserves
    /// [`None`] for every run that did not end on its own terms. A `failed` run did not close
    /// itself. Widening that field to cover it would be widening a gate until it went green.
    ///
    /// **Measured 2026-09-05T05:05:23Z over the loop's own store**, and this is what settles it:
    ///
    /// | ending | runs | its column | carried |
    /// | --- | ---: | --- | ---: |
    /// | `cancelled` | 36 | `cancelled_by` | **36** |
    /// | `exhausted` | 26 | `ceiling` | **26** |
    /// | `converged` | 54 | `done_reason` | 35 |
    /// | `failed` | 78 | *none* | **0** |
    /// | `blocked` | 14 | *none* | **0** |
    ///
    /// ⇒ ⭐⭐⭐ **Two endings already answered in full, in vocabularies of their own.** The debt was
    /// never *one field is attached to one ending*; it was *two endings had no column at all*.
    ///
    /// # ⛔⛔ Why an exhaustive `match` and not a list
    ///
    /// [`sprag_plugin::OutcomeState`]'s arms carry data, so there is no `ALL` to walk. A match with
    /// no `_` is stronger than one anyway: **a seventh ending will not compile until somebody names
    /// the column that says why it happened**, which is this workspace's rule 6 enforced by the
    /// compiler rather than by a reviewer.
    #[test]
    fn every_ending_names_a_column_that_says_why_it_happened() {
        use sprag_plugin::{Ceiling, OutcomeState};

        /// The column a reader opens to learn why THIS ending happened, or the statement that the
        /// ending word is itself the whole answer.
        ///
        /// ⛔ There is no `_` arm and there must never be one: an unclassified ending is a RED, not
        /// a pass.
        fn why_column(state: &OutcomeState) -> Result<&'static str, &'static str> {
            match state {
                // The plugin closed itself and named the ending out of its own vocabulary.
                OutcomeState::Converged => Ok("done_reason"),
                // A guardrail stopped it, and WHICH one is the remedy.
                OutcomeState::Exhausted(_) => Ok("ceiling"),
                // ⛔ REGISTER ITEM 903: this column is the one this round built.
                OutcomeState::Failed => Ok("failure"),
                // A person decided, and the row says which conversation they decided from.
                OutcomeState::Cancelled => Ok("cancelled_by"),
                // ⛔ REGISTER ITEM 903: and so is this one.
                OutcomeState::Blocked(_) => Ok("blocked_by"),
                // ⚠⚠ THE ONE ENDING WHOSE WORD IS ITS OWN REASON, stated rather than defaulted:
                // *a person took the pane* is the whole event, and a column beside it could only
                // repeat it. This arm exists so that saying so is a DECISION somebody wrote down,
                // which is what separates it from an ending nobody classified.
                OutcomeState::TakenOver(_) => Err("the ending word is the reason"),
            }
        }

        let every: Vec<OutcomeState> = [
            OutcomeState::Converged,
            OutcomeState::Cancelled,
            OutcomeState::Failed,
            OutcomeState::Blocked(None),
            OutcomeState::TakenOver(None),
        ]
        .into_iter()
        .chain(Ceiling::ALL.map(OutcomeState::Exhausted))
        .collect();

        // ⚠⚠ THE COLUMNS ARE CHECKED AGAINST THE RECORD'S OWN SHAPE, never a hand-written list:
        // a column named here that the row does not actually have would be a gate vouching for a
        // field nobody stores, which is the shape item 895 was filed over.
        let row = serde_json::to_value(PersistedRun {
            id: 7,
            label: "ai_loop pane=2".to_owned(),
            iterations: 3,
            finished: true,
            outcome: Some("failed".to_owned()),
            // ⚠ THE FIVE THIS GATE IS ABOUT, each holding something, so *the column is absent* and
            // *the column is empty* cannot pass for one another.
            done_reason: Some("no_successor".to_owned()),
            ceiling: Some("iterations".to_owned()),
            cancelled_by: Some(Canceller::Person),
            failure: Some("this run's machine could not be driven on: ...".to_owned()),
            blocked_by: Some("unreadable".to_owned()),
            // ⚠ AND EVERYTHING ELSE ABSENT, deliberately: what this gate reads is which columns
            // EXIST on the record, and a fixture that filled them all would pass even if the five
            // above were spelled wrong.
            cost: None,
            unit: None,
            moved_at: None,
            ended_at: None,
            ran_from: None,
            ran_to: None,
            output: None,
            build: None,
            which_run: None,
            driver: None,
            driving: None,
            opened_by_session: None,
            at: None,
            document: None,
            context_ceiling: None,
            context_high_water: None,
            overridden: None,
            place: None,
            stood_down: None,
            stood_down_by: None,
            deliveries: None,
            folds_by_reason: None,
            delivered_by_road: None,
            said_by_sentence: None,
            width_withheld: None,
            banked: None,
            briefed: None,
            request: None,
            tree: None,
        })
        .expect("a record serialises");

        let mut answered = 0;
        for state in &every {
            match why_column(state) {
                Ok(column) => {
                    assert!(
                        row.get(column).is_some_and(|held| !held.is_null()),
                        "⛔⛔⛔⛔⛔ REGISTER ITEM 903: a {state:?} run is supposed to say why in \
                         `{column}`, and the durable row has no such column holding a value. A \
                         gate that named a field nobody stores would pass while the reason went on \
                         dying with the daemon. Row: {row}",
                    );
                    answered += 1;
                }
                Err(said) => assert_eq!(
                    said, "the ending word is the reason",
                    "⛔ RULE 6: an ending may be excused from having a column ONLY by a sentence \
                     somebody wrote deciding so. {state:?} carries {said:?}",
                ),
            }
        }
        assert!(
            answered >= every.len() - 1,
            "⚠⚠⚠ THE POPULATION ARM: exactly one ending is excused, and if that ever grows this \
             gate has become a list of exemptions instead of a check. {answered} of {} answered \
             with a column",
            every.len(),
        );
    }

    #[test]
    fn submit_sweep_join_lifecycle() {
        let mut registry = RunRegistry::default();
        let state = Arc::new(Mutex::new(RunState::Running));
        let worker_state = Arc::clone(&state);
        let handle = std::thread::spawn(move || {
            // A trivial "run" that completes immediately.
            *lock(&worker_state) = RunState::Done {
                outcome: Box::new(Outcome {
                    state: sprag_plugin::OutcomeState::Exhausted(sprag_plugin::Ceiling::Iterations),
                    iterations: 0,
                    cost: None,
                    failure: None,
                    stopped: None,
                    answered: 0,
                    screened: 0,
                    deferred: None,
                    unchecked: None,
                    unadmitted: None,
                    deliveries: sprag_plugin::Deliveries::NONE,
                    checks: sprag_plugin::Checks::NONE,
                    banked: None,
                    briefed: None,
                    // ⚠ A run out of iterations named no ending — see `Outcome::done_reason`.
                    done_reason: None,
                }),
                output: None,
                uncommitted: None,
            };
        });
        let cancel = Arc::new(AtomicBool::new(false));
        let id = registry.reserve();
        assert_eq!(
            registry.submit(NewRun {
                id,
                label: "test".to_string(),
                plugin: crate::plugins::PluginName::Orchestrator,
                // ⚠ Not what this gate measures — item 543. A run submitted with no request is one
                // a successor cannot put back, which every run in this file was before it.
                request: None,
                opened_by: Some(7),
                opened_by_session: None,
                tree: None,
                // ⚠ NOR WHICH AUTHORS ITS BOUNDS CAME FROM — item 853. These fixtures submit
                // without parsing a request, so *nobody answered* is the only honest value; the
                // gates that drive the answer are `parse_guardrails`'s own, in `plugins`.
                overridden: None,
                state,
                run: Box::new(ThreadRun::new(
                    Orders::new(
                        cancel,
                        Arc::new(AtomicBool::new(false)),
                        Arc::new(AtomicBool::new(false)),
                        // ⚠ BOTH: this fixture is about the directory holding a record, and a
                        // handle that refused every order would make the orders below untestable
                        // here.
                        sprag_plugin::StandingOrder::ALL.to_vec(),
                        id,
                        // ⚠ NOWHERE TO ANNOUNCE, which is a registry off a daemon — item 664. What
                        // this fixture measures is the record, and a channel would be a collaborator
                        // it has no reader for.
                        None,
                    ),
                    handle,
                )),
                progress: ProgressCell::default(),
            }),
            RunId(0),
            "a reserved id is the id the record carries",
        );

        // Join (the worker is trivial, so this returns on its first pass) then observe Done.
        assert!(
            registry
                .join_all_within(RunRegistry::JOIN_DEADLINE)
                .is_empty(),
            "a worker that has already finished comes back",
        );
        registry.sweep();
        let snap = registry.snapshot();
        assert_eq!(snap.len(), 1);
        assert!(matches!(snap[0].state, RunState::Done { .. }));
        assert_eq!(
            snap[0].opened_by,
            Some(7),
            "the pane that asked for a run is what the agent-facing mouth keeps an agent to",
        );
    }

    /// A run whose worker IGNORES ITS CANCEL FLAG — which is what a thread parked in a syscall is,
    /// from the registry's side: the flag is raised, nothing reads it, the thread does not return.
    ///
    /// ⚠ It comes back when `released` is raised AND unconditionally after a minute, so a gate that
    /// fails cannot leave a thread behind for the rest of the test binary — and when it does come
    /// back it PUBLISHES ITS OWN OUTCOME, as a real worker's last act. That is what makes it usable
    /// for the claim that a detached run's ending is still its own.
    fn a_worker_that_will_not_come_back(id: RunId, released: &Arc<AtomicBool>) -> NewRun {
        let state = Arc::new(Mutex::new(RunState::Running));
        let worker_state = Arc::clone(&state);
        let flag = Arc::clone(released);
        let handle = std::thread::spawn(move || {
            let start = Instant::now();
            while !flag.load(Ordering::Acquire) && start.elapsed() < Duration::from_secs(60) {
                std::thread::sleep(Duration::from_millis(5));
            }
            *lock(&worker_state) = RunState::Done {
                outcome: Box::new(a_cancelled_outcome()),
                output: None,
                uncommitted: None,
            };
        });
        NewRun {
            state,
            ..parked_run(id, "wedged".to_string(), handle)
        }
    }

    /// What a worker that was asked to stop publishes when it finally does.
    fn a_cancelled_outcome() -> Outcome {
        Outcome {
            state: sprag_plugin::OutcomeState::Cancelled,
            iterations: 0,
            cost: None,
            failure: None,
            stopped: None,
            answered: 0,
            screened: 0,
            deferred: None,
            unchecked: None,
            unadmitted: None,
            deliveries: sprag_plugin::Deliveries::NONE,
            checks: sprag_plugin::Checks::NONE,
            banked: None,
            briefed: None,
            // ⚠ A CANCEL IS NOT AN ENDING A PLUGIN NAMES — see `Outcome::done_reason`, which is
            // `None` for exactly the runs that did not close on their own terms.
            done_reason: None,
        }
    }

    /// A run whose worker does what every real one does: reads its cancel flag and comes back.
    /// An obedient worker, handing back a flag that says **whether it was ever ASKED** —
    /// `true` only if it left because the cancel flag was raised, never because it timed out.
    ///
    /// ⚠⚠⚠⚠⚠ This exists so *"the drop asks before it waits"* can be asked of the RUN instead of
    /// of a stopwatch. The claim was gated by a 50 ms wall-clock bound, which is a proxy: it says
    /// *the wait was short*, and infers the ask from that. On a shared CI runner the inference
    /// breaks — macOS measured 53.3 ms on 2026-08-22 and reported *"it joined without asking"*
    /// about a daemon that had asked perfectly well. A flag the worker sets cannot be wrong about
    /// this, and cannot be slow.
    fn a_worker_that_records_being_asked(id: RunId) -> (NewRun, Arc<AtomicBool>) {
        let cancel = Arc::new(AtomicBool::new(false));
        let asked = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancel);
        let saw = Arc::clone(&asked);
        let handle = std::thread::spawn(move || {
            let start = Instant::now();
            while !flag.load(Ordering::Acquire) && start.elapsed() < Duration::from_secs(60) {
                std::thread::sleep(Duration::from_millis(1));
            }
            // Recorded from the flag itself rather than from "the loop ended": the sixty-second
            // ceiling is the other way out, and a worker that fell out of it was never asked.
            saw.store(flag.load(Ordering::Acquire), Ordering::Release);
        });
        (
            parked_run_with(id, "obedient".to_string(), handle, cancel),
            asked,
        )
    }

    /// A run whose worker returns after `delay` — a healthy one, slow enough that the FIRST sweep
    /// cannot have reaped it.
    fn a_worker_that_comes_back_after(id: RunId, delay: Duration) -> NewRun {
        let handle = std::thread::spawn(move || std::thread::sleep(delay));
        parked_run(id, "healthy".to_string(), handle)
    }

    /// ⚠⚠⚠⚠⚠ **A RESTORED RUN KEEPS THE BUILD THAT DROVE IT, AND A NEW ONE IS STAMPED WITH THIS
    /// IMAGE** — the two halves of register item 438's *"a run says which build drove it"*, and
    /// they are opposite decisions made three lines apart.
    ///
    /// `submit` stamps, because the worker it is about to start runs inside THIS image and no other
    /// answer is honest. `restore` copies, because the run it is about is over and a previous image
    /// already said what it was. Stamping in `restore` is the mutation this exists to catch: it
    /// compiles, it reads as consistency, every other gate here stays green — and it dates a dead
    /// daemon's work to whichever successor happens to read the log. That is the confusion the item
    /// was filed for, reproduced by the fix meant to end it.
    ///
    /// # ⚠⚠⚠⚠ And the third case is why [`RUN_LOG_VERSION`] does not move
    ///
    /// A log written before this field must LOAD, not be refused — that constant's own rule is that
    /// a format this build cannot read is ignored wholesale, so bumping it would throw away every
    /// run record a running daemon holds to gain a column. The optional field with a default is
    /// what makes the bump unnecessary, and it is only true while the JSON actually parses without
    /// the key, which no other gate here reads.
    ///
    /// ⚠⚠ It loads as [`None`] and NOT as this image: the absence means nobody recorded it.
    #[test]
    fn a_restored_run_says_which_context_ceiling_it_ran_under() {
        // ⛔⛔⛔⛔⛔ REGISTER ITEM 856(1b). Item 856's remaining clause is an experiment — move
        // `context_ceiling` and see whether the fold onset moves with it — and measured
        // 2026-09-04 it could not be READ: 0 of 214 rows carried the ceiling they ran under, and
        // the one place that knew (the run request) is dropped by the restore path, so 0 of 214
        // finished rows kept it either. The baseline that experiment would be compared against is
        // 603 folds of 2,516 attempted (23.97 %) over 80 runs, and **nothing said what ceiling
        // that number belongs to**. A run at a moved ceiling would land in the same pile.
        //
        // ⚠⚠ THE RESTORE IS THE CLAIM, not the row. A value that reaches a live row and dies at
        // the restart is exactly what was already there — item 856's rate is computed over runs
        // that have ENDED, and every one of those has crossed a restart.
        let saved: RunLog = serde_json::from_str(
            r#"{"version":1,"runs":[
                {"id":7,"label":"ai_loop pane=3","iterations":2,"cost":null,"unit":null,
                 "finished":false,"outcome":null,"ceiling":null,"output":null,
                 "context_ceiling":4242},
                {"id":8,"label":"ai_loop pane=4","iterations":2,"cost":null,"unit":null,
                 "finished":false,"outcome":null,"ceiling":null,"output":null}]}"#,
        )
        .expect("a log naming the ceiling parses, and so does one that does not");
        let mut successor = RunRegistry::default();
        successor.restore(&saved);
        // ⚠ Read back through `persistable`, which is the ROUND TRIP and not a peek: a value that
        // came out of the log and did not go back into it would be lost at the NEXT restart, and
        // this fact's whole defect was surviving one hop and not the next.
        let ceilings: Vec<Option<i64>> = successor
            .persistable()
            .runs
            .iter()
            .map(|run| run.context_ceiling)
            .collect();
        assert_eq!(
            ceilings,
            vec![Some(4242), None],
            "⛔⛔⛔⛔⛔ REGISTER ITEM 856(1b): the ceiling a run obeyed must cross the restart, and \
             a log that names none must come back as NOBODY SAID rather than as a zero. A zero \
             here is a value the loop's own guards read as *unbounded* (`context_ceiling > 0` \
             gates every deciding edge in `reviewing`), so publishing one for silence would claim \
             a run was unbounded on behalf of a daemon that was never asked — and it would put \
             every pre-field run into the experiment's control group.",
        );
    }

    /// ⛔⛔⛔⛔⛔ **AND HOW FULL ITS SESSION EVER GOT CROSSES THE SAME RESTART** — register item
    /// 894, the LEFT-hand side of the comparison the gate above holds the right-hand side of.
    ///
    /// Written as its own gate rather than a clause on that one because the two absences differ
    /// and only one of them is spelled by the document: a missing ceiling is *no bound was
    /// authored* and a missing reading is *nobody measured*. Both must come back [`None`], and a
    /// zero for either is the claim register item 891 was filed for.
    #[test]
    fn a_restored_run_says_how_full_its_session_ever_got() {
        let saved: RunLog = serde_json::from_str(
            r#"{"version":1,"runs":[
                {"id":7,"label":"ai_loop pane=3","iterations":2,"cost":null,"unit":null,
                 "finished":false,"outcome":null,"ceiling":null,"output":null,
                 "context_ceiling":800000,"context_high_water":612000},
                {"id":8,"label":"ai_loop pane=4","iterations":2,"cost":null,"unit":null,
                 "finished":false,"outcome":null,"ceiling":null,"output":null}]}"#,
        )
        .expect("a log naming the fullness parses, and so does one that does not");
        let mut successor = RunRegistry::default();
        successor.restore(&saved);
        // ⚠ THE ROUND TRIP, its neighbour's rule: a value that came out of the log and did not go
        // back in is lost at the NEXT restart, which is the hop this pair's other half died at.
        let read: Vec<(Option<i64>, Option<i64>)> = successor
            .persistable()
            .runs
            .iter()
            .map(|run| (run.context_high_water, run.context_ceiling))
            .collect();
        assert_eq!(
            read,
            vec![(Some(612_000), Some(800_000)), (None, None)],
            "⛔⛔⛔⛔⛔ REGISTER ITEM 894: the fullness a run's session reached must cross the \
             restart beside the bound it was judged by, and a log that names neither must come \
             back as NOBODY MEASURED. A zero reading is what this document holds before a run's \
             first turn ends and what `costs_now` sends for a record it could not read, so \
             publishing one for silence would turn every pre-field run into an empty session — \
             register item 891, one field over.",
        );
    }

    /// 🎯🎯🎯🎯🎯 **AND ALL THREE CROSS ON A RUN THAT HAS ALREADY ENDED — THE ONLY POPULATION
    /// ITEM 856 ⒞ EVER READS** — register items 894 and 859.
    ///
    /// # ⛔⛔⛔⛔⛔ Both gates above restore a run that is STILL RUNNING
    ///
    /// `"finished": false` in each of their fixtures, and item 856's rate is computed over runs
    /// that have ENDED — `a_run_whose_cell_never_moved_persists_the_pair_its_driver_reported`'s
    /// own doc says so in those words. So the arm the live store will actually present was
    /// reachable by neither. Measured over the loop's own log at **2026-09-05T14:59:11Z**: of 234
    /// rows, the only build that ever wrote these columns is `e528943fd830`, it wrote them on
    /// **3** rows, and **every one of those rows is unfinished** — so the store cannot answer this
    /// question either, and nothing but a gate can.
    ///
    /// ⚠⚠ `restore` takes a different path for a finished row (`RunState::Done` with a whole
    /// rebuilt [`Outcome`]) and that path drops column after column by design — `answered`,
    /// `screened`, `deliveries`, `checks` each carry a comment saying why. Whether these three are
    /// in that set or beside it was a fact about which branch a reader had traced, and this makes
    /// it a fact a command answers.
    ///
    /// ⛔⛔ **AND `overridden` HAD NO ROUND TRIP AT ALL.** Its two neighbours each got one when
    /// item 894 built them; this one was published by item 859 into the same log and nothing ever
    /// read it back out. It is the column that decides `Judged`, so losing it does not make a row
    /// unreadable — it makes an EXPERIMENT read as an ordinary run, which is the contamination
    /// item 856 filed 859 to stop.
    #[test]
    fn a_run_that_has_already_ended_brings_all_three_of_item_856s_columns_back() {
        let saved: RunLog = serde_json::from_str(
            r#"{"version":1,"runs":[
                {"id":1,"label":"ai_loop pane=1","iterations":9,"cost":null,"unit":null,
                 "finished":true,"outcome":"converged","ceiling":null,"output":null,
                 "context_ceiling":800000,"context_high_water":612000,
                 "overridden":["max_seconds"]},
                {"id":2,"label":"ai_loop pane=2","iterations":9,"cost":null,"unit":null,
                 "finished":true,"outcome":"cancelled","ceiling":null,"output":null},
                {"id":3,"label":"ai_loop pane=3","iterations":9,"cost":null,"unit":null,
                 "finished":true,"outcome":"exhausted","ceiling":null,"output":null,
                 "context_ceiling":20000,"context_high_water":24000,"overridden":[]}]}"#,
        )
        .expect("a log of ENDED runs is what a successor daemon actually reads");
        let mut successor = RunRegistry::default();
        successor.restore(&saved);
        let back = successor.persistable();

        // ── ① THE ROUND TRIP, all three columns, through `Done` rather than through `Running` ──
        //
        // ⚠ The triple is NAMED rather than spelled at the binding: item 856's axis is these three
        // together — a reading, the bound it was judged against, and whose numbers those were —
        // and a reader of the comparison below should see three answers, not a shape.
        type Columns = (Option<i64>, Option<i64>, Option<Vec<String>>);
        let read: Vec<Columns> = back
            .runs
            .iter()
            .map(|run| {
                (
                    run.context_high_water,
                    run.context_ceiling,
                    run.overridden.clone(),
                )
            })
            .collect();
        assert_eq!(
            read,
            vec![
                (
                    Some(612_000),
                    Some(800_000),
                    Some(vec!["max_seconds".to_owned()])
                ),
                (None, None, None),
                (Some(24_000), Some(20_000), Some(Vec::new())),
            ],
            "⛔⛔⛔⛔⛔ REGISTER ITEM 856 ⒞: the three columns its axis is built out of have to \
             survive the restart on a run that has ENDED, because that is the only kind it ever \
             reads — a run is read after it is over and its daemon is gone by then. A build that \
             dropped any of them here would put every future row behind item 894's wall while \
             both gates above stayed green, and `sprag folds` would go on reporting the count \
             this store has reported all along: nothing measurable.",
        );

        // ── ② AND THE THREE SILENCES STAY THREE DIFFERENT ANSWERS ──
        //
        // ⛔ Run 3 took NONE of its document's numbers and says so with an empty list; run 2's log
        // answers nothing at all. `Overridden::joined`'s own rule is that those must not fold —
        // `Some([])` is *its document set every number it authored* and `None` is *nobody said*,
        // and only the first belongs in this axis's own denominator.
        assert_ne!(
            read[1].2, read[2].2,
            "⛔⛔⛔⛔ REGISTER ITEM 859: *nobody answered* and *the caller took nothing* came back \
             as one answer. Folded, every pre-field run joins the axis's control group and every \
             experiment leaves it — the contamination item 856 filed 859 to stop, arriving \
             through the restore rather than through the door. Got {read:?}",
        );

        // ── ③ AND THE FIXTURE IS REALLY THE ENDED POPULATION ──
        //
        // ⚠⚠ Without this the gate silently becomes its neighbours: a fixture edited to
        // `"finished": false` would satisfy ① and ② and measure the branch they already cover.
        assert!(
            back.runs.iter().all(|run| run.finished)
                && back.runs.len() == saved.runs.len()
                && back
                    .runs
                    .iter()
                    .filter(|run| run.overridden.is_some())
                    .count()
                    == 2,
            "⚠⚠⚠ THE POPULATION IS THE CLAIM: three rows in, three ENDED rows out, two of them \
             answering item 859. Got {:?}",
            back.runs,
        );
    }

    /// ⛔⛔⛔⛔⛔ **AND A RUN WHOSE CELL NEVER MOVED KEEPS WHAT ITS DRIVER REPORTED** — register
    /// item 894, and **a correction to the two gates above rather than a third of their kind.**
    ///
    /// # ⛔⛔⛔⛔⛔ Those gates restore from a LOG, so they never watch a value ENTER one
    ///
    /// Both start by putting a value into the cell and then read it back out, which measures the
    /// restart and nothing before it. The hop they cannot see is a LIVE run's value reaching the
    /// log in the first place — and measured 2026-09-05 that hop was broken for the ceiling from
    /// the day it was published: [`RunRegistry::persistable`] read `run.progress` ALONE for it,
    /// the one fact in that struct not written `reported…or(cell)`.
    ///
    /// ⇒ For a run driven in ANOTHER PROCESS the cell never moves at all
    /// (`crate::plugins::spawn_driven_run` says so of itself: *"AN EMPTY CELL, AND IT STAYS
    /// EMPTY"*), and that has been the default since 2026-08-24. So the ceiling was knowable while
    /// a run was alive and never afterwards — and item 856's rate is computed over runs that have
    /// ENDED. The fact was published exactly where nobody needed it.
    ///
    /// ⚠⚠ **THE CELL IS LEFT EMPTY ON PURPOSE, AND THAT IS THE WHOLE POPULATION.** A fixture that
    /// filled it in would pass with the defect in place — which is precisely how the defect
    /// survived the gate its own item wrote.
    #[test]
    fn a_run_whose_cell_never_moved_persists_the_pair_its_driver_reported() {
        let released = Arc::new(AtomicBool::new(false));
        let mut registry = RunRegistry::default();
        let id = registry.reserve();
        registry.submit(a_worker_that_will_not_come_back(id, &released));
        // ⚠ NOTHING TOUCHES THE CELL. This is the out-of-process shape: the driver's only channel
        // is `report`, and the `Progress` beside the record stays at its default for ever.
        registry
            .report(
                id,
                serde_json::json!({
                    "iterations": 2,
                    crate::plugins::REPORTED_BESIDE_KEY: {
                        crate::plugins::RUN_CONTEXT_CEILING_KEY: 800_000,
                        crate::plugins::RUN_CONTEXT_HIGH_WATER_KEY: 612_000,
                    },
                }),
            )
            .expect("a running run accepts its driver's report");

        let log = registry.persistable();
        let kept = log
            .runs
            .iter()
            .find(|run| run.id == id.0)
            .map(|run| (run.context_high_water, run.context_ceiling));
        assert_eq!(
            kept,
            Some((Some(612_000), Some(800_000))),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 894: a run driven in another process reported both sides of \
             the comparison it restarts by, and the log kept neither. Reading the cell first — or \
             alone — publishes *nobody said* about every run this daemon starts, because the cell \
             beside such a run never moves. That is not a defensive nicety about an unusual \
             driver: it is the DEFAULT driver, and item 856's whole axis is this pair.",
        );

        released.store(true, Ordering::Release);
    }

    /// 🎯🎯🎯🎯🎯 **AND WHICH OF ITS NUMBERS WERE NOT ITS DOCUMENT'S CROSSES THE RESTART** —
    /// register item 859(1), the third fact in this struct that was readable only while a run
    /// lived, and the one that says an EXPERIMENT is an experiment.
    ///
    /// # ⛔⛔⛔⛔⛔ Four answers, and folding any two of them loses item 856's denominator
    ///
    /// A word list that came back is *the caller took these*; `Some([])` is *its document authored
    /// numbers and the caller took none*, which is the healthy launch and the affirmative
    /// [`crate::plugins::Overridden`] exists to make readable; [`None`] is *nobody answered*. The
    /// fourth is a log naming a word THIS BUILD CANNOT SPELL, which must refuse the whole answer
    /// rather than come back one word shorter — a shorter list is not a weaker claim here, it
    /// names a different set of flags for somebody to go and delete.
    ///
    /// ⚠⚠ **THE RESTORE IS THE CLAIM, NOT THE ROW**, its two neighbours' rule: item 856's fold
    /// rate is computed over runs that have ENDED, and every one of those has crossed a restart.
    /// Measured 2026-09-05, **220 of 220 stored rows carried no such word**, so the two runs that
    /// moved a ceiling were told apart from the seventeen ordinary ones by a human note — which is
    /// exactly the contamination item 856's entry says an unannounced experiment causes.
    #[test]
    fn a_restored_run_says_which_of_its_numbers_were_not_its_documents() {
        let saved: RunLog = serde_json::from_str(
            r#"{"version":1,"runs":[
                {"id":7,"label":"ai_loop pane=3","iterations":2,"cost":null,"unit":null,
                 "finished":false,"outcome":null,"ceiling":null,"output":null,
                 "overridden":["context_ceiling","max_seconds"]},
                {"id":8,"label":"ai_loop pane=4","iterations":2,"cost":null,"unit":null,
                 "finished":false,"outcome":null,"ceiling":null,"output":null,
                 "overridden":[]},
                {"id":9,"label":"ai_loop pane=5","iterations":2,"cost":null,"unit":null,
                 "finished":false,"outcome":null,"ceiling":null,"output":null},
                {"id":10,"label":"ai_loop pane=6","iterations":2,"cost":null,"unit":null,
                 "finished":false,"outcome":null,"ceiling":null,"output":null,
                 "overridden":["max_reflections"]}]}"#,
        )
        .expect("a log naming taken numbers parses, and so does one that names none");
        let mut successor = RunRegistry::default();
        successor.restore(&saved);
        // ⚠ Read back through `persistable`, its neighbours' rule: a value that came out of the
        // log and did not go back into it is lost at the NEXT restart, and this fact's defect was
        // reaching a live row and dying at every hop after it.
        let took: Vec<Option<Vec<String>>> = successor
            .persistable()
            .runs
            .iter()
            .map(|run| run.overridden.clone())
            .collect();
        assert_eq!(
            took,
            vec![
                Some(vec!["context_ceiling".to_owned(), "max_seconds".to_owned()]),
                Some(Vec::new()),
                None,
                None,
            ],
            "⛔⛔⛔⛔⛔ REGISTER ITEM 859(1): which of a run's numbers came from its caller must \
             cross the restart, and the four answers must stay four. `Some([])` collapsing to \
             `None` would tell a reader *nobody answered* about the healthy launch this key was \
             built to make readable; a foreign word surviving would have this daemon republish \
             vocabulary it never compiled, and one dropped WORD would name a smaller set of flags \
             than the run was started with. Item 856's denominator is runs, and an experiment that \
             cannot say it is one contaminates the population it ran in: {took:?}",
        );
    }

    /// ⛔⛔⛔⛔⛔ **AND A LIVE RUN'S ANSWER REACHES THE LOG AT ALL** — register item 859(1), and
    /// **a separate gate rather than a clause on the one above, for the reason item 894 measured.**
    ///
    /// The gate above restores from a log and reads it back, so it watches the restart and nothing
    /// before it. The hop it cannot see is a value ENTERING the log, and that is the hop at which
    /// `context_ceiling` was broken from the day it was published — its own item's round-trip gate
    /// stayed green while 218 of 218 rows carried nothing. A fact gated only where it comes OUT of
    /// a log is a fact no run ever puts IN.
    ///
    /// ⚠⚠ The answer is taken from the SUBMIT and not from a driver's report, which is
    /// `RunRecord::overridden`'s own doc: it is decided once, at the door, and a level that never
    /// moves must not be reported by a party that could disagree with the door.
    #[test]
    fn a_run_whose_caller_took_a_number_says_so_in_the_log() {
        let released = Arc::new(AtomicBool::new(false));
        let mut registry = RunRegistry::default();
        let id = registry.reserve();
        let mut submitted = a_worker_that_will_not_come_back(id, &released);
        submitted.overridden = crate::plugins::Overridden::restored(&[
            "context_ceiling".to_owned(),
            "max_bytes".to_owned(),
        ]);
        assert!(
            submitted.overridden.is_some(),
            "⚠⚠ the fixture must actually carry an answer, or this gate drives the absence and \
             passes with the recording site deleted — the dead control item 856 met three times",
        );
        registry.submit(submitted);

        let kept = registry
            .persistable()
            .runs
            .iter()
            .find(|run| run.id == id.0)
            .and_then(|run| run.overridden.clone());
        assert_eq!(
            kept,
            Some(vec!["context_ceiling".to_owned(), "max_bytes".to_owned()]),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 859(1): a run submitted with numbers its caller took must \
             say so in the DURABLE log and not only on the live row. Measured 2026-09-05, 220 of \
             220 stored rows carried no such word while `sprag runs` published it for live ones — \
             the same shape item 894 found one field over, where a value reached every reader \
             except the one computing item 856's rate over ended runs.",
        );

        released.store(true, Ordering::Release);
    }

    #[test]
    fn a_restored_run_keeps_the_build_that_drove_it_and_a_new_one_is_stamped_with_this_image() {
        let mut registry = RunRegistry::default();
        let id = registry.reserve();
        registry.submit(a_worker_that_comes_back_after(id, Duration::from_millis(1)));

        assert_eq!(
            registry.snapshot()[0].build.as_deref(),
            Some(crate::wire::BUILD),
            "⚠⚠⚠ a run this daemon started was driven by this daemon, and nothing else can be the \
             honest answer",
        );
        assert_eq!(
            registry.persistable().runs[0].build.as_deref(),
            Some(crate::wire::BUILD),
            "and what it leaves on disk must carry it, or the successor has nothing to read",
        );

        // ── A PREDECESSOR'S LOG, naming a build this image is not ──
        let mut log = registry.persistable();
        log.runs[0].build = Some("0000deadbeef".to_owned());
        let mut successor = RunRegistry::default();
        successor.restore(&log);
        assert_eq!(
            successor.snapshot()[0].build.as_deref(),
            Some("0000deadbeef"),
            "⚠⚠⚠⚠⚠ THE DEAD DAEMON'S BUILD SURVIVES ITS DAEMON. A successor that stamped its own \
             here would report every restored run as driven by code that never ran it",
        );

        // ── AND A LOG FROM BEFORE THE FIELD EXISTED, as JSON rather than as a struct ──
        // ⚠ Hand-written on purpose: serialising `PersistedRun` would always EMIT the key, so a
        // fixture built from the type could never stage the file this compatibility claim is about.
        let old: RunLog = serde_json::from_str(
            r#"{"version":1,"runs":[{"id":7,"label":"ai_loop pane=3","iterations":2,
                "cost":null,"unit":null,"finished":true,"outcome":"converged",
                "ceiling":null,"output":null}]}"#,
        )
        .expect(
            "⚠⚠⚠ a log written before this field must still PARSE — if it does not, the field \
             owed `RUN_LOG_VERSION` a bump and every run record on a live daemon is being thrown \
             away",
        );
        assert_eq!(
            old.runs[0].build, None,
            "⚠⚠ and it loads as «nobody recorded it», never as the build that happens to be \
             reading it",
        );
        let mut reader = RunRegistry::default();
        reader.restore(&old);
        assert_eq!(
            reader.snapshot()[0].build,
            None,
            "the absence must survive the restore too, or the honest answer is lost one layer in",
        );
    }

    fn parked_run(id: RunId, label: String, handle: JoinHandle<()>) -> NewRun {
        parked_run_with(id, label, handle, Arc::new(AtomicBool::new(false)))
    }

    /// [`parked_run`] whose worker is already sharing `cancel` — what a fixture needs when the
    /// thing under test is whether an order REACHES the flag the worker reads.
    fn parked_run_with(
        id: RunId,
        label: String,
        handle: JoinHandle<()>,
        cancel: Arc<AtomicBool>,
    ) -> NewRun {
        NewRun {
            id,
            label,
            // ⚠ Stated rather than defaulted, for the recorder fixture's reason: these gates are
            // about workers and joins, and the plugin is not what any of them measures.
            plugin: crate::plugins::PluginName::Orchestrator,
            // ⚠ Nor is what a successor could rebuild it from — item 543, nor which authors set
            // its bounds — item 853.
            request: None,
            opened_by: None,
            opened_by_session: None,
            tree: None,
            overridden: None,
            state: Arc::new(Mutex::new(RunState::Running)),
            run: Box::new(ThreadRun::new(
                Orders::new(
                    cancel,
                    Arc::new(AtomicBool::new(false)),
                    Arc::new(AtomicBool::new(false)),
                    // ⚠ BOTH, for the fixture above's reason: these gates are about the directory,
                    // and the refusal is driven where a real plugin answers it.
                    sprag_plugin::StandingOrder::ALL.to_vec(),
                    id,
                    // ⚠ Nowhere to announce — a registry off a daemon, item 664.
                    None,
                ),
                handle,
            )),
            progress: ProgressCell::default(),
        }
    }

    /// ⚠⚠⚠⚠ **A REGISTRY HOLDING A WORKER THAT WILL NOT COME BACK IS STILL DROPPED** — register
    /// item 305, and the one thing `Drop` could not promise before it had a deadline.
    ///
    /// `Drop` can neither return an error nor panic, so an unbounded join in it is a process that
    /// cannot be shut down: the flag is raised at a thread that never reads it again and the
    /// destructor never returns. Both halves are asserted — that it WAITED (a deadline nobody
    /// consults is not a deadline) and that it CAME BACK.
    #[test]
    fn dropping_a_registry_holding_a_worker_that_will_not_come_back_still_returns() {
        let released = Arc::new(AtomicBool::new(false));
        let mut registry = RunRegistry::default();
        let id = registry.reserve();
        registry.submit(a_worker_that_will_not_come_back(id, &released));

        let raised = Instant::now();
        drop(registry);
        let waited = raised.elapsed();
        released.store(true, Ordering::Release);

        assert!(
            waited >= RunRegistry::JOIN_DEADLINE,
            "a drop that gave up in {waited:?} never waited for the worker it asked to stop",
        );
        assert!(
            waited < RunRegistry::JOIN_DEADLINE * 2,
            "the drop did not come back: {waited:?}",
        );
    }

    /// ⚠⚠⚠⚠⚠ **THE BOUND ON PUTTING A RUN BACK IS ASKED OF THE DIRECTORY, BECAUSE NOTHING ELSE CAN
    /// REACH IT** — register item 671.
    ///
    /// The end-to-end gate (`cli`'s `a_run_whose_driver_process_dies_is_put_back_on_a_new_one`)
    /// kills a driver and watches a new one appear, and it cannot stage the arm that matters most:
    /// a REPLACEMENT that dies without ever saying anything, which is what a broken image or a
    /// request its own door refuses looks like. Staging that means winning a race against a driver
    /// that reports twice a second, so the verdict is asked here instead — register item 641's
    /// rule, *an arm an end-to-end gate cannot reach is a verdict a function has to answer*.
    ///
    /// # ⚠⚠⚠⚠ Why the watermark is the REPORT COUNT and not the run's own iterations
    ///
    /// A driver put back at a saved place counts ITS OWN steps from one — [`InheritedRun::progress`]
    /// says so as a property rather than a bug — so `iterations` goes DOWN across a rescue and a
    /// replacement that had worked for minutes would read as *behind where the last one got to*.
    /// The count in the record only ever goes up and nobody reports it, which is what makes *did
    /// the driver I started say anything at all* answerable.
    ///
    /// ⚠ And the third answer is asserted too: a report between two deaths puts the run back in
    /// business, because there is no number of deaths that makes a run doing work not worth
    /// resuming.
    #[test]
    fn a_replacement_driver_that_reported_nothing_is_not_replaced_again() {
        let mut registry = RunRegistry::default();
        let id = registry.reserve();
        let cell = ProgressCell::default();
        lock(&cell).place = Some(vec!["judging".to_owned()]);
        registry.submit(NewRun {
            request: Some(serde_json::Map::new()),
            progress: cell,
            ..parked_run(id, "a loop".to_string(), std::thread::spawn(|| {}))
        });

        assert!(
            matches!(registry.revival(id), Revival::PutBack(_)),
            "a FIRST death is always answered: a run with a place took a step to write it",
        );
        // What a driver that died at its own door leaves behind: the run is exactly as it was, and
        // nothing new has been said about it.
        *lock(&registry.runs[0].state) = RunState::Panicked("it said nothing".to_owned());
        let verdict = registry.revival(id);
        assert!(
            matches!(verdict, Revival::NoProgress),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 671: a daemon put a run back on a THIRD driver having heard \
             nothing at all from the second. Nothing about the run changed between the two deaths, \
             so nothing about the next one will either — this is a spin that costs a process per \
             turn of it, and the person watching is told the run is running each time.",
        );
        let said = lock(&registry.runs[0].state);
        assert!(
            matches!(&*said, RunState::Panicked(why) if why.contains(verdict.not_put_back().unwrap_or("!"))),
            "the row does not carry the reason nothing is coming: {said:?}",
        );
        drop(said);

        // ── AND A REPORT PUTS IT BACK IN BUSINESS ───────────────────────────────────────────
        assert_eq!(
            registry.report(id, serde_json::json!({ "iterations": 1 })),
            Ok(()),
            "⛔⛔⛔ REGISTER ITEM 764's OWN LINE NOT TO CROSS: a `panicked` run is one whose \
             replacement driver is being built RIGHT NOW (`put_back_a_lost_driver`), so the door \
             that refuses a set-aside run must not refuse this one — it would answer *nothing is \
             driving you* to the driver this daemon had just stood up",
        );
        assert!(
            matches!(registry.revival(id), Revival::PutBack(_)),
            "⛔⛔⛔ a driver that reported and then died is a run DOING WORK, and there is no \
             number of times that stops being true — this bound is *did the last one say \
             anything*, not *how many have there been*",
        );
    }

    /// ⚠⚠ **A WORKER THAT PANICKED IS REAPED AND SAID SO** — what the timed wait promises beyond
    /// *the thread is over*, and the one observable that tells JOINED from merely FINISHED.
    ///
    /// Its neighbours argue from an id's ABSENCE in the answer, which is only worth anything because
    /// the handle is taken by a join and by nothing else. This is that link, asserted.
    #[test]
    fn a_worker_that_panicked_is_joined_and_recorded_as_panicked() {
        let mut registry = RunRegistry::default();
        let id = registry.reserve();
        let handle = std::thread::spawn(|| {
            panic!("a worker panicking ON PURPOSE — the gate around it reads what the registry did")
        });
        registry.submit(parked_run(id, "panicking".to_string(), handle));

        assert!(
            registry
                .join_all_within(RunRegistry::JOIN_DEADLINE)
                .is_empty(),
            "a worker that panicked has come back",
        );
        let snap = registry.snapshot();
        assert!(
            matches!(snap[0].state, RunState::Panicked(_)),
            "a panicking worker must be JOINED and recorded, not merely observed to have stopped: \
             {:?}",
            snap[0].state,
        );
    }

    /// ⚠⚠⚠⚠ **A RUN LEFT BEHIND AT THE DEADLINE KEEPS ITS OWN ENDING** — the half of the detach
    /// that is a claim about HONESTY rather than about time.
    ///
    /// The timed wait may not stamp a terminal state on a run whose thread is still going. It would
    /// be a lie told about a live worker — the thread is still stepping, still holding a pane — and
    /// it would RACE the only author there is: the worker publishes its outcome as its last act, so
    /// a stamped `Interrupted` is either overwritten a moment later or overwrites the real answer.
    /// Leaving it `Running` is also what makes the run log's story true: unfinished on disk, and
    /// [`RunState::Interrupted`] when a successor daemon reads it back, which is what a run whose
    /// daemon went away actually is.
    #[test]
    fn a_run_left_behind_at_the_deadline_keeps_its_own_ending() {
        let released = Arc::new(AtomicBool::new(false));
        let mut registry = RunRegistry::default();
        let id = registry.reserve();
        let run = a_worker_that_will_not_come_back(id, &released);
        let state = Arc::clone(&run.state);
        registry.submit(run);

        assert_eq!(
            registry.join_all_within(Duration::from_millis(100)),
            vec![id],
            "the worker was supposed to still be going",
        );
        let left_behind = lock(&state).clone();
        assert!(
            matches!(left_behind, RunState::Running),
            "the shutdown gave an ending to a thread that had not ended: {left_behind:?}",
        );

        // And the worker is still the only author of its outcome, so it still gets to publish one.
        released.store(true, Ordering::Release);
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && matches!(*lock(&state), RunState::Running) {
            std::thread::sleep(Duration::from_millis(5));
        }
        let published = lock(&state).clone();
        assert!(
            matches!(published, RunState::Done { .. }),
            "a detached worker must still be able to publish its own outcome: {published:?}",
        );
    }

    /// ⚠⚠⚠ **AND THE RUN LEFT BEHIND COMES BACK `Interrupted` TO THE NEXT DAEMON** — the last
    /// sentence of [`RunRegistry::join_all_within`]'s doc, held as one chain rather than as three
    /// facts that happen to sit near each other.
    ///
    /// Its neighbour above proves the shutdown leaves the record `Running`. What that is FOR is the
    /// durable log: `Running` means unfinished on disk, and a successor reading an unfinished record
    /// answers [`RunState::Interrupted`] — *this run's daemon went away and nothing resumed it*,
    /// which is exactly what a detached worker's run is. Written as one gate because the value of
    /// the first fact is entirely in the second: a shutdown that stamped an ending would have
    /// persisted a FINISHED run, and the person who came back to a restarted daemon would be told
    /// their loop converged.
    ///
    /// ⚠⚠⚠⚠⚠ **AND IT COMES BACK WITHOUT A SEAT BUT WITH ITS CONVERSATION**, which is
    /// [`restore`](RunRegistry::restore)'s rule 1 and was re-taken on 2026-08-18. This used to
    /// assert that the run belonged to NOBODY, justified by *"the pane the run drove came back as
    /// a plain shell"* — measured false: an allowlisted agent is restored `--resume`d into the
    /// same conversation. Dropping the pane id is still right (a successor cannot know who is
    /// sitting in a seat); dropping everything was not, because it left the ASKER unable to find
    /// its own runs. The conversation is what is the same on both sides of a restart.
    ///
    /// # ⚠⚠ What this adds over the daemon-death gate, checked rather than assumed
    ///
    /// `cli`'s `a_run_whose_daemon_died_is_reported_as_interrupted_and_belongs_to_nobody` drives the
    /// same chain end to end and catches most of what is below — it was RUN against the mutation to
    /// find that out, not guessed at. Two things are this gate's alone:
    ///
    /// * **the starting point is a DETACH** — a run the deadline left behind — which is the state
    ///   item 305 introduced and which no daemon fixture can stage, since wedging a real worker
    ///   needs a stopped pane device and some eighty kilobytes pushed at it;
    /// * **the id counter**. That gate asserts the restored run KEEPS its id; it never starts a
    ///   second run, so it cannot see the successor REISSUE it. `restore`'s second authority
    ///   decision — *a successor that started from zero would mint ids that already name a run in
    ///   its own list* — is held here and nowhere else: deleting the seeding line reddens this and
    ///   leaves the other 855 lib gates green.
    #[test]
    fn the_run_a_shutdown_left_behind_comes_back_interrupted_and_keeps_the_conversation_that_asked()
    {
        let released = Arc::new(AtomicBool::new(false));
        let mut registry = RunRegistry::default();
        let id = registry.reserve();
        let mut run = a_worker_that_will_not_come_back(id, &released);
        run.opened_by = Some(3);
        // The SEAT and the CONVERSATION, which a restart answers differently — the seat is dropped
        // and this is kept. `PluginsExternal::session_in` is what fills this in the product.
        run.opened_by_session = Some(A_CONVERSATION.to_owned());
        registry.submit(run);

        assert_eq!(
            registry.join_all_within(Duration::from_millis(100)),
            vec![id],
            "the worker was supposed to still be going, or this chain starts from the wrong place",
        );

        // What the daemon leaves on disk for its successor, and what the successor makes of it.
        let log = registry.persistable();
        released.store(true, Ordering::Release);
        assert_eq!(log.runs.len(), 1);
        assert!(
            !log.runs[0].finished,
            "a run whose worker was detached is UNFINISHED on disk — it never published an outcome",
        );

        let mut successor = RunRegistry::default();
        successor.restore(&log);
        let restored = successor.snapshot();
        assert!(
            matches!(restored[0].state, RunState::Interrupted),
            "the successor must say the daemon went away, not invent an ending: {:?}",
            restored[0].state,
        );
        assert_eq!(
            restored[0].opened_by, None,
            "⚠⚠ THE SEAT IS DROPPED — a successor cannot know who is sitting in pane 3, so it does \
             not claim to. This half is unchanged; what changed is that it is no longer the WHOLE \
             answer. See `RunRegistry::restore` rule 1",
        );
        assert_eq!(
            restored[0].opened_by_session.as_deref(),
            Some(A_CONVERSATION),
            "⚠⚠⚠⚠⚠ AND THE CONVERSATION SURVIVES, WHICH IS THE WHOLE POINT OF THE ROUND THAT \
             WROTE THIS LINE. The old rule dropped provenance entirely on a premise measured FALSE \
             (that a restored pane holds a plain shell — an allowlisted agent comes back \
             `--resume`d, in the same conversation). With the seat alone there was nothing a \
             successor could match on, so the same agent could not see the runs it started; with \
             the conversation there is. Deleting the carry in `restore` reddens exactly here",
        );
        assert_eq!(
            successor.reserve(),
            RunId(id.0 + 1),
            "and its id is never reissued",
        );
    }

    /// ⛔⛔⛔⛔ **WHAT A RUN PUT INTO ITS PANE SURVIVES THE DAEMON THAT PUT IT THERE** — register
    /// item 606, and the reason register item 599 could not be answered by looking.
    ///
    /// # What was measured, on this machine, on 2026-08-22
    ///
    /// Item 591 built the instrument 599 needs: a run publishes `delivered` and `folded`, so a
    /// reader can ask whether the prompts a loop typed were ones its peer's composer folded away.
    /// Asked of the two live daemons here, **thirteen runs answered and not one carried the
    /// number** — every one of them was `(build not recorded)`, which is what a run restored from
    /// the log looks like. Several had obviously delivered: one spent 17203 bytes over 90
    /// iterations.
    ///
    /// ⚠⚠⚠⚠⚠ **SO THE INSTRUMENT IS UNREADABLE ON EXACTLY THE RUNS A PERSON READS.** A run is
    /// looked at after it ends, and the daemon that drove it is restarted constantly — this
    /// repository's own debt loop promotes a build and restarts between rounds. The fact reached
    /// the wire and died at the first restart.
    ///
    /// ⚠⚠ **IT IS A RECORD, NOT AN ORDER**, which is what makes persisting it right where
    /// persisting a hold would be wrong. `RunRegistry::restore`'s rule refuses to resurrect an
    /// INSTRUCTION nobody can act on; how many prompts a finished run typed is a fact about what
    /// already happened, and it is the only thing that explains a pane that looks empty.
    ///
    /// ⚠ The PAIR travels or neither does — `sprag_plugin::Deliveries`' own rule. A fold count
    /// without its denominator says nothing.
    #[test]
    fn what_a_run_put_into_its_pane_survives_the_daemon_that_put_it_there() {
        let mut registry = RunRegistry::default();
        let id = registry.reserve();
        let progress = ProgressCell::default();
        lock(&progress).deliveries = Some(sprag_plugin::Deliveries {
            made: 14,
            folded: 3,
            // ⚠ THE THIRD COUNT TRAVELS TOO — register item 617, and it is set to a value distinct
            // from both above so a restore that dropped it, or that filled it from a neighbour,
            // fails here rather than agreeing by coincidence. A wedged prompt is the fact that
            // MOST needs to survive its daemon: the text is still sitting in a composer somebody
            // can walk over and look at, long after the run that typed it is gone.
            unsubmitted: 5,
            // ⚠ AND THE FOURTH, distinct from all three above for the same reason — register item
            // 762. A restore that dropped it, or filled it from a neighbour, would publish *no
            // question of this run went missing* about the one run that most needs the opposite
            // said, and it would agree with `folded` by coincidence if it were 3.
            unreported: 7,
            // ⛔⛔⛔⛔⛔ AND THE FIFTH — register item 669, the sub-count that says WHICH witness
            // closed those three folds. `2` rather than `3` on purpose: equal to `folded` would be
            // satisfied by a restore that filled this from its container, and that is the one wrong
            // answer that reads as a real diagnosis (*every fold was the composer, so that peer's
            // hooks report nothing*).
            released: 2,
        });
        registry.submit(NewRun {
            id,
            label: "ai_loop pane=2".to_owned(),
            plugin: crate::plugins::PluginName::AiLoop,
            // ⚠ These gates read what a FINISHED run persists, and item 543's door refuses a
            // finished run whatever it carries — so there is nothing here to carry.
            request: None,
            opened_by: None,
            opened_by_session: None,
            tree: None,
            overridden: None,
            state: Arc::new(Mutex::new(RunState::Done {
                outcome: Box::new(an_outcome()),
                output: None,
                uncommitted: None,
            })),
            run: Box::new(EndedRun::restored(false, None, None)),
            progress,
        });

        // ⚠⚠ THROUGH THE FILE, not through `persistable` alone: a field `serde` never writes would
        // still satisfy an in-process round trip, which is the neighbouring gate's argument.
        let on_disk = serde_json::to_string(&registry.persistable()).expect("the run log encodes");
        let read_back: RunLog = serde_json::from_str(&on_disk).expect("and decodes");
        let mut successor = RunRegistry::default();
        successor.restore(&read_back);

        let restored = successor.snapshot();
        let carried = restored[0].progress.deliveries.expect(
            "⛔ REGISTER ITEM 891: a stored table came back as *nobody counted*, which is the \
             absence this cell learned to say and NOT what a log holding a table means",
        );
        assert_eq!(
            (
                carried.made,
                carried.folded,
                carried.unsubmitted,
                carried.unreported,
                carried.released,
            ),
            (14, 3, 5, 7, 2),
            "⛔⛔⛔ ITEM 606: this run typed 14 prompts, 3 of them were folded away and 5 were \
             never asked at all, and a daemon restart lost them. Those numbers are the whole of \
             item 591's instrument, and a run is READ after it has ended — by which time the \
             daemon that drove it has usually been restarted. ⚠ THE THIRD IS ITEM 617's, asserted \
             beside the pair rather than in its own gate because it is one value: it says a prompt \
             is STILL SITTING in a composer, which is the one of the three a person can act on \
             after the fact. ⛔⛔⛔⛔⛔ THE FOURTH AND FIFTH JOINED THIS ASSERTION ON 2026-09-04 — \
             item 762's count was SET in this fixture and never read, so its durable crossing was \
             unwatched for four days; the fifth is item 669's, and it is the one whose absence \
             reads as a diagnosis rather than as a gap. Got {carried:?} from {on_disk}",
        );
    }

    /// ⛔⛔⛔⛔⛔ **AND THE SPLIT OF THOSE FOLDS SURVIVES THE DAEMON TOO** — register item 856(1),
    /// on item 606's measurement one gate up and **written because a mutation showed nothing was
    /// watching.**
    ///
    /// # ⛔⛔⛔⛔ Measured 2026-09-04, exactly as item 847's crossing gate was
    ///
    /// Writing `folds_by_reason: None` at the persist site was run against `sprag-host` and
    /// `sprag-gate` together: **the only red was the standing one (register item 837).** The
    /// type-level gate drives the wire shape, the loop-level gate stops inside the plugin, and the
    /// durable log — the one crossing item 606 proved matters most — was watched by nothing.
    ///
    /// # ⚠⚠⚠⚠⚠ Why this crossing is the one item 856 cannot do without
    ///
    /// Item 606 asked two live daemons for their runs' delivery pairs and **thirteen answered with
    /// none, every one restored**: *a run is READ after it ends, and the daemon that drove it is
    /// restarted between rounds.* Item 856's split is read on exactly those runs, so a split that
    /// died with its daemon would be an instrument whose readings are available only while nobody
    /// is reading — item 856's own shape, one layer down.
    ///
    /// ⚠⚠ **THROUGH THE FILE**, the neighbouring gates' argument: a field `serde` never writes
    /// would still satisfy an in-process round trip. It matters more here than for the pair above,
    /// because this value is a MAP and `#[serde(flatten)]` is the one attribute that can silently
    /// swallow a whole table.
    ///
    /// ⚠ The two rows differ in BOTH numbers and in reason, so a restore that dropped one, or
    /// filled it from its neighbour, fails here rather than agreeing by coincidence — and the
    /// LANDING row is the one whose loss would be silent, since a table of pure folds looks exactly
    /// like the hand tally this item replaced.
    #[test]
    fn the_split_of_a_runs_folds_survives_the_daemon_that_counted_it() {
        let mut registry = RunRegistry::default();
        let id = registry.reserve();
        let progress = ProgressCell::default();
        let mut folds = sprag_plugin::FoldsByReason::NONE;
        for _ in 0..3 {
            folds.record(sprag_plugin::ReflectReason::Capacity.occasion(), true);
        }
        // ⚠⚠ THE CONTROL ROW, and the one that must not be lost: same prompt shape, a different
        // reason, and it LANDED. It is the only shape that can refute item 856's axis, so a
        // restore that kept only the folds would leave the register unable to be contradicted —
        // which is the state this item exists to end.
        for _ in 0..4 {
            folds.record(sprag_plugin::ReflectReason::Budget.occasion(), false);
        }
        // ⛔⛔⛔⛔⛔ **AND THE HARDENINGS, WHICH ARE IN NEITHER ROW ABOVE** — register item 856(3).
        // `capacity` gets one of EACH road, so a restore that carried the pair as one number, or
        // that put a hardening under the fold count, fails here rather than agreeing by
        // coincidence. `budget` deliberately gets NONE, which is the control: a reason that
        // reflected and never hardened is the shape item 856's axis can be refuted by.
        folds.record_unasked(
            sprag_plugin::ReflectReason::Capacity.occasion(),
            sprag_plugin::UnaskedRoad::AfterAFold,
        );
        folds.record_unasked(
            sprag_plugin::ReflectReason::Capacity.occasion(),
            sprag_plugin::UnaskedRoad::OnThePane,
        );
        // ⛔⛔⛔⛔⛔ **AND THE ORDINARY TRAFFIC, WHICH IS THE ROW A PERSON ACTUALLY READS AGAINST** —
        // register item 856's widening. It exists so the split can be reconciled with the run's
        // totals, and item 606 measured that the split anybody reads is ALWAYS a restored one —
        // thirteen live runs, every one restored. A row that died with its daemon would leave the
        // identity checkable only on a run nobody looks at.
        folds.record(sprag_plugin::Occasion::Ordinary, true);
        folds.record(sprag_plugin::Occasion::Ordinary, false);
        folds.record_unasked(
            sprag_plugin::Occasion::Ordinary,
            sprag_plugin::UnaskedRoad::OnThePane,
        );
        lock(&progress).folds_by_reason = Some(folds);
        registry.submit(NewRun {
            id,
            label: "ai_loop pane=2".to_owned(),
            plugin: crate::plugins::PluginName::AiLoop,
            request: None,
            opened_by: None,
            opened_by_session: None,
            tree: None,
            overridden: None,
            state: Arc::new(Mutex::new(RunState::Done {
                outcome: Box::new(an_outcome()),
                output: None,
                uncommitted: None,
            })),
            run: Box::new(EndedRun::restored(false, None, None)),
            progress,
        });

        let on_disk = serde_json::to_string(&registry.persistable()).expect("the run log encodes");
        let read_back: RunLog = serde_json::from_str(&on_disk).expect("and decodes");
        let mut successor = RunRegistry::default();
        successor.restore(&read_back);

        let carried = successor.snapshot()[0].progress.folds_by_reason.expect(
            "⛔ REGISTER ITEM 891: a stored split came back as *nobody counted* — the absence \
             belongs to a log with no such column, never to one holding a table",
        );
        assert_eq!(
            carried.under(sprag_plugin::ReflectReason::Capacity.occasion()),
            sprag_plugin::FoldsUnder {
                delivered: 3,
                folded: 3,
                // ⛔⛔⛔⛔⛔ REGISTER ITEM 856(3): the two roads, told apart, across the file. A
                // restore that summed them would publish `2 unasked` and lose the only thing that
                // distinguishes a run that folded from one that never did — which is the whole
                // reading runs 194 and 197 were invisible to.
                unasked: sprag_plugin::Unasked {
                    after_a_fold: 1,
                    on_the_pane: 1,
                },
            },
            "⛔⛔⛔⛔⛔ REGISTER ITEM 856(1) AND 856(3): the split did not survive its daemon, and item 606 \
             MEASURED that this is the crossing every reader takes — thirteen live runs, every one \
             restored, none carrying its delivery pair. An instrument that empties at the daemon \
             boundary can be consulted only while nobody is consulting it. Got {carried:?} from \
             {on_disk}",
        );
        assert_eq!(
            carried.under(sprag_plugin::ReflectReason::Budget.occasion()),
            sprag_plugin::FoldsUnder {
                delivered: 4,
                folded: 0,
                // ⚠⚠ AND IT HARDENED NOTHING — register item 856(3)'s control. A restore that
                // copied `capacity`'s hardenings across the table would satisfy the assertion
                // above and destroy the comparison, which is this gate's own stated hazard.
                unasked: sprag_plugin::Unasked::default(),
            },
            "⛔⛔⛔⛔ AND THE ROW THAT LANDED SURVIVED. A restore that kept the folds and dropped \
             this one leaves a table whose denominator is its numerator — the hand tally item \
             856(1) replaced, restored faithfully. Got {carried:?} from {on_disk}",
        );
        assert_eq!(
            carried.under(sprag_plugin::Occasion::Ordinary),
            sprag_plugin::FoldsUnder {
                delivered: 2,
                folded: 1,
                unasked: sprag_plugin::Unasked {
                    after_a_fold: 0,
                    on_the_pane: 1,
                },
            },
            "⛔⛔⛔⛔⛔ REGISTER ITEM 856's WIDENING, ACROSS THE DAEMON: the ordinary row is what \
             makes this table reconcilable with the run's own totals, and item 606 measured that \
             every split a person reads came out of this file. Dropped here, the identity holds \
             only on a live run nobody is looking at, and the difference between the split and the \
             totals goes back to being a number that explains itself to nobody. Got {carried:?} \
             from {on_disk}",
        );

        // ⛔⛔⛔⛔⛔ ── AND A LOG WRITTEN BEFORE THE PAIR EXISTED READS AS ZERO, NOT AS A REFUSAL ──
        //
        // Register item 856(3), and the OPPOSITE call from the live wire's
        // (`crate::plugins::folds_by_reason_in` refuses a report missing these keys). A stored row
        // is a fact from another build and its silence is nothing a reader can act on; a live
        // driver's silence is a build skew. Driven through a real decode of a real older shape,
        // because a `#[serde(default)]` that was dropped would leave every pre-existing run log
        // unreadable — and this daemon restores from one on every boot.
        // ⚠ Built by EDITING THE PARSED DOCUMENT rather than by string surgery on it: a `replace`
        // over the text matched only the rows whose values happened to differ from the others, so
        // the fixture stripped one row and left five — and the premise assertion below is what
        // caught that. A shape this gate claims to be reading has to be built by construction.
        let mut older: Value = serde_json::from_str(&on_disk).expect("the log just written parses");
        for row in older["runs"][0][crate::plugins::RUN_FOLDS_BY_REASON_KEY]
            .as_object_mut()
            .expect("the run carries a split")
            .values_mut()
        {
            let row = row.as_object_mut().expect("each reason carries a row");
            row.remove("unasked_after_a_fold");
            row.remove("unasked_on_the_pane");
        }
        // ⚠⚠ THE PREMISE IS ASKED OF THE FOLD TABLE AND NOT OF THE WHOLE DOCUMENT, which it used
        // to be — register item 889. `said_by_sentence` carries the same two field names on its
        // own rows, so a `!older.contains("unasked_")` over the file went red the day that table
        // arrived, about a fixture that was stripping exactly what it claimed to. A premise has to
        // name the thing it is a premise about.
        let split = older["runs"][0][crate::plugins::RUN_FOLDS_BY_REASON_KEY].to_string();
        assert!(
            !split.contains("unasked_"),
            "⚠ THE PREMISE: this fixture must actually strip the pair, or the decode below is \
             reading the same document twice. Got {split}",
        );
        let older = older.to_string();
        let old_log: RunLog = serde_json::from_str(&older).expect(
            "⛔⛔⛔⛔ REGISTER ITEM 856(3): a run log written before the hardening pair existed \
             must still decode. Every boot of this daemon restores from one.",
        );
        let mut before = RunRegistry::default();
        before.restore(&old_log);
        assert_eq!(
            before.snapshot()[0]
                .progress
                .folds_by_reason
                .expect(
                    "⚠ THE OUTER TABLE IS PRESENT IN THIS FIXTURE — register item 891. What is \
                     stripped is the pair INSIDE each row, so the cell must still say *something \
                     counted*; a `None` here would mean the fixture stripped the column instead \
                     and the assertion below would be about the wrong absence"
                )
                .under(sprag_plugin::ReflectReason::Capacity.occasion())
                .unasked,
            sprag_plugin::Unasked::default(),
            "⚠⚠ and it reads as *nobody counted this*, which for a stored row is the same number \
             as *nothing hardened* — the honest answer, since the build that wrote it could not \
             have said otherwise. ⚠⚠⚠ THAT IS THE INNER AXIS AND IT IS DELIBERATE (see the \
             comment above): register item 891 is about the TABLE's own absence, which this cell \
             now says with `None`, and it does not reach the pair inside a row a build did write",
        );
    }

    /// ⛔⛔⛔⛔⛔ **AND THE ROAD OF EVERY DELIVERY SURVIVES THE DAEMON TOO** — register item 856, on
    /// item 606's measurement and the crossing a LANDING count is only ever read across.
    ///
    /// # ⛔⛔⛔⛔⛔ The run anybody reads has been restored, and this is the value that proves it
    ///
    /// Item 606 asked two live daemons for their runs' delivery pairs and **thirteen answered with
    /// none, every one restored**: *a run is READ after it ends, and the daemon that drove it is
    /// restarted between rounds.* A landing count is read on exactly those runs — the whole
    /// measurement that opened this item was six FINISHED runs of this repository compared against
    /// their logs — so a table that died with its daemon would be a fourth instrument nobody can
    /// consult.
    ///
    /// ⚠⚠ **THROUGH THE FILE**, its neighbours' argument, and it binds hardest here: the value is a
    /// MAP behind `#[serde(flatten)]`, the one attribute that can swallow a whole table in silence.
    ///
    /// ⚠ The fixture puts different counts on roads that mean different things — a proven landing,
    /// a fold that landed, a road that proves nothing, and one that establishes nothing was asked —
    /// so a restore that dropped a row, or filled one from its neighbour, fails here rather than
    /// agreeing by coincidence.
    #[test]
    fn the_road_of_every_delivery_survives_the_daemon_that_counted_it() {
        let mut registry = RunRegistry::default();
        let id = registry.reserve();
        let progress = ProgressCell::default();
        let mut roads = sprag_plugin::DeliveredByRoad::NONE;
        for _ in 0..5 {
            roads.record(sprag_plugin::Witnessed::Painted);
        }
        // ⚠⚠ A FOLD THAT LANDED — register item 762's second road, inside `folded` and inside
        // `landed` at once. A restore that let one classification stand in for the other pools them.
        roads.record(sprag_plugin::Witnessed::LetGo);
        // ⚠⚠⚠ AND THE TWO SHAPES `made - folded` COUNTED AS LANDINGS. They are the reason the
        // subtraction was never the number it was read as, so a restore that lost them would leave
        // a landing count that agrees with the wrong arithmetic.
        for _ in 0..3 {
            roads.record(sprag_plugin::Witnessed::Unchecked);
        }
        roads.record(sprag_plugin::Witnessed::Unasked);
        lock(&progress).delivered_by_road = Some(roads);
        registry.submit(NewRun {
            id,
            label: "ai_loop pane=2".to_owned(),
            plugin: crate::plugins::PluginName::AiLoop,
            request: None,
            opened_by: None,
            opened_by_session: None,
            tree: None,
            overridden: None,
            state: Arc::new(Mutex::new(RunState::Done {
                outcome: Box::new(an_outcome()),
                output: None,
                uncommitted: None,
            })),
            run: Box::new(EndedRun::restored(false, None, None)),
            progress,
        });

        let on_disk = serde_json::to_string(&registry.persistable()).expect("the run log encodes");
        let read_back: RunLog = serde_json::from_str(&on_disk).expect("and decodes");
        let mut successor = RunRegistry::default();
        successor.restore(&read_back);

        let carried = successor.snapshot()[0].progress.delivered_by_road.expect(
            "⛔ REGISTER ITEM 891: a stored road table came back as *nobody counted* — the \
             absence belongs to a log with no such column, never to one holding a table",
        );
        assert_eq!(
            carried, roads,
            "⛔⛔⛔⛔⛔ REGISTER ITEM 856: the road table did not survive its daemon, and item 606 \
             MEASURED that this is the crossing every reader takes — thirteen live runs, every one \
             restored. The landing count this item exists to create would be available only while \
             nobody was reading it. Got {carried:?} from {on_disk}",
        );
        assert_eq!(
            (carried.landed(), carried.unproven(), carried.not_asked()),
            (6, 3, 1),
            "⛔⛔⛔⛔ AND THE THREE ANSWERS COME BACK APART. A restore that summed them, or that \
             lost the road proving nothing, would publish `9 of 10 landed` for a run where four \
             prompts never became a question — which is exactly what `made - folded` says. Got \
             {carried:?} from {on_disk}",
        );

        // ⛔⛔⛔⛔⛔ ── AND A LOG WRITTEN BEFORE THE TABLE EXISTED READS AS EMPTY, NOT AS A REFUSAL ──
        //
        // The OPPOSITE call from the live wire's (`crate::plugins::delivered_by_road_in` refuses a
        // report naming a road this build cannot spell). A stored row is a fact from another build
        // and its silence is nothing a reader can act on; a live driver's silence is a skew.
        // Driven through a real decode, because a `#[serde(default)]` that was dropped would leave
        // every pre-existing run log unreadable — and this daemon restores from one on every boot.
        let mut older: Value = serde_json::from_str(&on_disk).expect("the log just written parses");
        older["runs"][0]
            .as_object_mut()
            .expect("a run is an object")
            .remove(crate::plugins::RUN_DELIVERED_BY_ROAD_KEY);
        let older = older.to_string();
        let old_log: RunLog = serde_json::from_str(&older).expect(
            "⛔⛔⛔⛔ REGISTER ITEM 856: a run log written before the road table existed must still \
             decode. Every boot of this daemon restores from one.",
        );
        let mut before = RunRegistry::default();
        before.restore(&old_log);
        // ⛔⛔⛔⛔⛔ **`None` AND NOT A TABLE OF ZEROS** — register item 891, and this assertion is
        // where that item's headline was written down as a PASS. It used to read `is_empty()`,
        // and its own message said *it reads as nobody counted this* about a value that was a
        // table of zeros — indistinguishable from a run that counted and found none. The two are
        // now different values, and this is the gate that says so.
        assert!(
            before.snapshot()[0].progress.delivered_by_road.is_none(),
            "⛔⛔⛔ REGISTER ITEM 891: a log with no road column restored as a TABLE, so *nobody \
             counted* and *counted nothing* are one value again. Measured over the live store \
             before this was fixed: 220 rows carried a table and 11 carried a number, and the \
             row from 2026-08-26 carried six rows of zeros for a concept its build never had",
        );
    }

    /// ⛔⛔⛔⛔⛔ **AND WHICH SENTENCE EACH PROMPT WAS SURVIVES THE DAEMON TOO** — register item
    /// 889, on item 606's measurement and the crossing this table is ONLY ever read across.
    ///
    /// # ⛔⛔⛔⛔⛔ The measurement that opened the item was 197 finished runs
    ///
    /// Item 606 asked two live daemons for their runs' delivery pairs and **thirteen answered with
    /// none, every one restored**. That binds harder here than anywhere: item 889's fifteen-fold
    /// ratio is not a fact about one run at all — it is a rate compared over a whole log of
    /// finished ones — so a table that died with its daemon would leave the ratio exactly where the
    /// item found it, in a person's `python3` heredoc over `/run/user/1000/loop/`.
    ///
    /// ⚠⚠ **THROUGH THE FILE**, its neighbour's argument, and for its neighbour's reason: the value
    /// is a MAP behind `#[serde(flatten)]` whose entries are themselves objects, so both a dropped
    /// row and a dropped FIELD are failures a comparison would never show.
    ///
    /// ⚠ The fixture stages the pair `asks` cannot carry — a clean `brief` and a stuck `turn`, two
    /// sentences declaring one word — so a restore that pooled them fails here rather than agreeing
    /// by coincidence.
    #[test]
    fn which_sentence_each_prompt_was_survives_the_daemon_that_counted_it() {
        let mut registry = RunRegistry::default();
        let id = registry.reserve();
        let progress = ProgressCell::default();
        let mut said = sprag_plugin::SaidBySentence::NONE;
        said.record(sprag_plugin::Sentence::Brief);
        for _ in 0..26 {
            said.record(sprag_plugin::Sentence::Turn);
        }
        // ⚠⚠ ONE ON EACH ROAD — the two carry opposite remedies, so a restore that summed them
        // would send a reader to a pane holding nothing.
        said.record_unasked(
            sprag_plugin::Sentence::Turn,
            sprag_plugin::UnaskedRoad::OnThePane,
        );
        said.record_unasked(
            sprag_plugin::Sentence::Turn,
            sprag_plugin::UnaskedRoad::AfterAFold,
        );
        lock(&progress).said_by_sentence = Some(said);
        registry.submit(NewRun {
            id,
            label: "ai_loop pane=2".to_owned(),
            plugin: crate::plugins::PluginName::AiLoop,
            request: None,
            opened_by: None,
            opened_by_session: None,
            tree: None,
            overridden: None,
            state: Arc::new(Mutex::new(RunState::Done {
                outcome: Box::new(an_outcome()),
                output: None,
                uncommitted: None,
            })),
            run: Box::new(EndedRun::restored(false, None, None)),
            progress,
        });

        let on_disk = serde_json::to_string(&registry.persistable()).expect("the run log encodes");
        let read_back: RunLog = serde_json::from_str(&on_disk).expect("and decodes");
        let mut successor = RunRegistry::default();
        successor.restore(&read_back);

        let carried = successor.snapshot()[0].progress.said_by_sentence.expect(
            "⛔ REGISTER ITEM 891: a stored sentence table came back as *nobody counted* — the \
             absence belongs to a log with no such column, never to one holding a table",
        );
        assert_eq!(
            carried, said,
            "⛔⛔⛔⛔⛔ REGISTER ITEM 889: the sentence table did not survive its daemon, and item \
             606 MEASURED that this is the crossing every reader takes — thirteen live runs, every \
             one restored. The rate this item exists to create would be available only while \
             nobody was reading it. Got {carried:?} from {on_disk}",
        );
        assert_eq!(
            (
                carried.of(sprag_plugin::Sentence::Brief),
                carried.of(sprag_plugin::Sentence::Turn),
            ),
            (
                sprag_plugin::SaidUnder {
                    sent: 1,
                    unasked: sprag_plugin::Unasked::default(),
                },
                sprag_plugin::SaidUnder {
                    sent: 28,
                    unasked: sprag_plugin::Unasked {
                        after_a_fold: 1,
                        on_the_pane: 1,
                    },
                },
            ),
            "⛔⛔⛔⛔ AND THE TWO SENTENCES THAT DECLARE ONE `asks` WORD COME BACK APART. A restore \
             that pooled them would publish 2 of 29 about both — the 1.86 % that hid a fifteen-fold \
             ratio for as long as `asks` was the only vocabulary. Got {carried:?} from {on_disk}",
        );

        // ⛔⛔⛔⛔⛔ ── AND A LOG WRITTEN BEFORE THE TABLE EXISTED READS AS EMPTY, NOT AS A REFUSAL ──
        //
        // The OPPOSITE call from the live wire's (`crate::plugins::said_by_sentence_in` refuses a
        // report naming a sentence this build cannot spell), its neighbour's argument verbatim.
        let mut older: Value = serde_json::from_str(&on_disk).expect("the log just written parses");
        older["runs"][0]
            .as_object_mut()
            .expect("a run is an object")
            .remove(crate::plugins::RUN_SAID_BY_SENTENCE_KEY);
        let older = older.to_string();
        let old_log: RunLog = serde_json::from_str(&older).expect(
            "⛔⛔⛔⛔ REGISTER ITEM 889: a run log written before the sentence table existed must \
             still decode. Every boot of this daemon restores from one.",
        );
        let mut before = RunRegistry::default();
        before.restore(&old_log);
        // ⛔⛔⛔⛔⛔ **`None` AND NOT A TABLE OF ZEROS** — register item 891, its road-table
        // neighbour's argument verbatim: this read `is_empty()` while its message claimed to be
        // reading *nobody counted this*, and a table of eleven zero rows is what a run that
        // counted and asked everything also has.
        assert!(
            before.snapshot()[0].progress.said_by_sentence.is_none(),
            "⛔⛔⛔ REGISTER ITEM 891: a log with no sentence column restored as a TABLE, so \
             *nobody counted* and *every prompt was asked* are one value again",
        );
    }

    /// ⛔⛔⛔⛔⛔ **A TALLY NOBODY KEPT IS NOT A TALLY OF NONE** — register item 891, and the three
    /// hops one fact has to be asked at.
    ///
    /// # ⛔⛔⛔⛔⛔ What was green, and the shape of the laundering
    ///
    /// Four columns here are TALLIES, and each was written `Some(report.unwrap_or(cell))` on the
    /// argument *this image looked, so a zero is a claim it may make*. A tally is not a flag: it
    /// had to be INCREMENTED WHILE THE RUN RAN, and a run this daemon inherited already finished
    /// was incremented by nothing here. The restore filled its cell with `…::NONE` for a log that
    /// had no such column, and the next save signed those zeros — so
    /// `None` → `NONE` → `Some(zeros)` turned a predecessor's SILENCE into this image's COUNT, and
    /// the store re-serialises every row on every save, so it reached rows whose build had no such
    /// concept.
    ///
    /// Measured 2026-09-05 over the live loop's store: **220 rows, 220 carrying a
    /// `folds_by_reason` table, 11 carrying a number**, and row `id 0` (build `52459b9ebf78`,
    /// 2026-08-26) carrying six rows of four zeros. The number register item 856's done-when is
    /// judged by — *how many runs have a sample* — therefore read 220 instead of 11.
    ///
    /// # ⚠⚠⚠ THREE HOPS, because a round-trip gate cannot see the hop the value ENTERS on
    ///
    /// This session learnt that twice (items 889 and 894): a gate over the restore hop alone stays
    /// green while the producer publishes nothing, and one over the producer alone stays green
    /// while the log drops it. So:
    ///
    /// | hop | asked here |
    /// |---|---|
    /// | file → cell | a log with no column restores as [`None`], never as a zeroed table |
    /// | cell → file | that [`None`] is written back OUT as absent, never re-signed |
    /// | cell → row | the wire carries `null` for it, so no reader meets a zero either |
    ///
    /// # ⛔⛔⛔ AND THE FOURTH ASSERTION IS THE ONE THAT COVERS THE NEXT COLUMN
    ///
    /// Item 891's third clause is *answer this for every answer key at once, because fixing one
    /// key leaves the next to land in the same place* — and it had already come true once, half a
    /// day after it was written, when item 856 added an `ordinary` row that restored as a zero. So
    /// the last assertion reads THIS MODULE'S OWN SOURCE and holds that
    /// [`RunRegistry::persistable`] writes no field of [`PersistedRun`] as an unconditional `Some`
    /// except the ones classified below. A fifth tally cannot reach the log any other way without
    /// going red first.
    #[test]
    fn a_tally_nobody_kept_is_not_a_tally_of_none() {
        // ── HOP 1 and 2: a log with no counter column at all, restored and written back ───────
        let mut registry = RunRegistry::default();
        let id = registry.reserve();
        let progress = ProgressCell::default();
        {
            let mut moving = lock(&progress);
            moving.deliveries = Some(sprag_plugin::Deliveries {
                made: 9,
                folded: 2,
                released: 1,
                unsubmitted: 3,
                unreported: 4,
            });
            let mut folds = sprag_plugin::FoldsByReason::NONE;
            folds.record(sprag_plugin::ReflectReason::Capacity.occasion(), true);
            moving.folds_by_reason = Some(folds);
            let mut roads = sprag_plugin::DeliveredByRoad::NONE;
            roads.record(sprag_plugin::Witnessed::Painted);
            moving.delivered_by_road = Some(roads);
            let mut said = sprag_plugin::SaidBySentence::NONE;
            said.record(sprag_plugin::Sentence::Brief);
            moving.said_by_sentence = Some(said);
        }
        registry.submit(NewRun {
            id,
            label: "ai_loop pane=2".to_owned(),
            plugin: crate::plugins::PluginName::AiLoop,
            request: None,
            opened_by: None,
            opened_by_session: None,
            tree: None,
            overridden: None,
            state: Arc::new(Mutex::new(RunState::Done {
                outcome: Box::new(an_outcome()),
                output: None,
                uncommitted: None,
            })),
            run: Box::new(EndedRun::restored(false, None, None)),
            progress,
        });
        let written = serde_json::to_string(&registry.persistable())
            .expect("a run log serialises to its own file");

        // ⛔⛔⛔⛔⛔ **AND THE FOUR COLUMNS ARE DERIVED, NOT NAMED** — the rule `watching-zenoh`
        // handed this register: take the population out of the PUBLISHER'S STRUCTURE rather than
        // out of a list somebody typed, because the list is what goes stale. A durable key is not
        // the wire key either (`deliveries` against `delivered`), so a hand list would be two
        // vocabularies deep. Write the same row twice — once holding the tallies and once with
        // them cleared through the TYPE — and the keys whose value goes null are exactly theirs.
        let full: Value = serde_json::from_str(&written).expect("the log just written parses");
        let mut cleared = registry.persistable();
        {
            let row = &mut cleared.runs[0];
            row.deliveries = None;
            row.folds_by_reason = None;
            row.delivered_by_road = None;
            row.said_by_sentence = None;
        }
        let bare = serde_json::to_value(&cleared).expect("and so does the cleared one");
        let keys: Vec<String> = full["runs"][0]
            .as_object()
            .expect("a run is an object")
            .iter()
            .filter(|(name, held)| !held.is_null() && bare["runs"][0][name].is_null())
            .map(|(name, _)| name.clone())
            .collect();
        assert_eq!(
            keys.len(),
            4,
            "⚠ THE PREMISE: exactly the four tallies must go null when the TYPE's four tally \
             fields are cleared, or this gate is reading a population it did not derive. Got \
             {keys:?} from {written}",
        );

        // ⚠ THE FIXTURE IS BUILT BY EDITING THE PARSED DOCUMENT — the lesson item 856(3)'s own
        // fixture paid for: a `replace` over the text matched only the rows whose values happened
        // to differ, so the shape the gate claimed to read was not the shape it built.
        let mut older = full.clone();
        {
            let row = older["runs"][0]
                .as_object_mut()
                .expect("a run is an object");
            for key in &keys {
                assert!(
                    row.remove(key).is_some(),
                    "⚠ THE PREMISE: this daemon must have written `{key}`, or what follows is a \
                     statement about a column that was never there. Wrote {written}",
                );
            }
        }
        let older = older.to_string();
        let old_log: RunLog = serde_json::from_str(&older).expect(
            "⛔⛔⛔⛔ a run log written before any of the four tallies existed must still decode — \
             every boot of this daemon restores from one",
        );
        let mut before = RunRegistry::default();
        before.restore(&old_log);

        let cell = &before.snapshot()[0].progress;
        assert_eq!(
            (
                cell.deliveries.is_none(),
                cell.folds_by_reason.is_none(),
                cell.delivered_by_road.is_none(),
                cell.said_by_sentence.is_none(),
            ),
            (true, true, true, true),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 891, HOP 1: a log with no counter column restored as a table \
             of zeros, so *nobody was counting* and *counted and found none* are one value in the \
             cell. Got {cell:?} from {older}",
        );

        // ⚠⚠ ASKED OF THE TYPE AND NOT OF THE KEYS — the four fields are what the persist site
        // writes, and a gate reading names would pass a build that renamed one. The derived key
        // list above is for BUILDING the older shape; this is for judging what came out of it.
        let again = before.persistable();
        let row = &again.runs[0];
        assert_eq!(
            (
                row.deliveries.is_none(),
                row.folds_by_reason.is_none(),
                row.delivered_by_road.is_none(),
                row.said_by_sentence.is_none(),
            ),
            (true, true, true, true),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 891, HOP 2: the absence went IN and a count came OUT — a run \
             this image never incremented had zeros signed as its own. This is the round trip \
             that put a zeroed table on 209 of the live store's 220 rows, including one from a \
             build with no such concept. Got {row:?}",
        );

        // ── HOP 3 and HOP 4: and no READER meets a zero either, at EITHER mouth ─────────────
        //
        // ⚠⚠ SEPARATE HOPS AND NOT A COROLLARY — items 889 and 894 both found a wire outside
        // their gate while the durable round trip was green, and the first draft of THIS gate
        // asked `run_to_json` for a key only `progress_to_json` publishes, so a mutation that
        // stamped zeros on silence came back GREEN. There are two mouths and they answer
        // differently by design (item 663): the report block is UNCONDITIONAL, so an absent count
        // crosses it as `null`; the row publishes each key only once there is something to say,
        // so an absent count leaves no key at all.
        let wire_keys = [
            crate::plugins::RUN_DELIVERED_KEY,
            crate::plugins::RUN_FOLDED_KEY,
            crate::plugins::RUN_RELEASED_KEY,
            crate::plugins::RUN_UNSUBMITTED_KEY,
            crate::plugins::RUN_UNREPORTED_KEY,
            crate::plugins::RUN_FOLDS_BY_REASON_KEY,
            crate::plugins::RUN_DELIVERED_BY_ROAD_KEY,
            crate::plugins::RUN_SAID_BY_SENTENCE_KEY,
        ];
        let reported = crate::plugins::progress_to_json(&before.snapshot()[0].progress);
        let beside = &reported[crate::plugins::REPORTED_BESIDE_KEY];
        assert!(
            beside.is_object(),
            "⚠ THE PREMISE: the report block is unconditional (item 663), so it must be here or \
             the loop below is vacuous — which is exactly how this gate's first draft passed a \
             mutation that stamped zeros on silence. Got {reported}",
        );
        for key in wire_keys {
            assert!(
                beside[key].is_null(),
                "⛔⛔⛔⛔⛔ REGISTER ITEM 891, HOP 3: the report crossed `{key}` as a number for a \
                 run nobody counted. `progress_from_report` reads this block, so a zero here is \
                 a count the host will believe and write down. Got {beside}",
            );
        }
        let published = crate::plugins::run_to_json(
            &before.snapshot()[0],
            None,
            crate::plugins::LiveLook::default(),
        );
        for key in wire_keys {
            assert!(
                published.get(key).is_none(),
                "⛔⛔⛔⛔ REGISTER ITEM 891, HOP 4: the ROW published `{key}` for a run nobody \
                 counted. A reader cannot tell that from a run that counted and found none, and \
                 item 815's clause reads the ABSENCE of this key as its evidence. Got {published}",
            );
        }

        // ── AND THE RATCHET: the next tally cannot arrive the old way ────────────────────────
        //
        // ⛔⛔⛔⛔⛔ **THE NEEDLES ARE SYNTHESISED AT RUN TIME** so this gate is not its own
        // counter-example — item 872 built the same kind of ratchet three times before it stopped
        // matching its own source. Nothing below appears in this file as a literal.
        let source = include_str!("runs.rs");
        let opener = format!("{}{}", "PersistedRun", " {");
        let producer = format!("{} {}", "pub fn", "persistable");
        let at = source
            .find(&producer)
            .expect("⚠ THE PREMISE: this module produces the durable log in a named function");
        let body = &source[at..];
        let literal = body
            .find(&opener)
            .expect("⚠ THE PREMISE: that function builds the record by naming its fields");
        // ⚠ BRACE-MATCHED RATHER THAN LINE-COUNTED — register item 892's rule. A positional
        // window is a second authority on where the literal ends, and the day somebody adds a
        // field the window is silently reading somebody else's code.
        let block = {
            let from = &body[literal + opener.len() - 1..];
            let mut depth = 0usize;
            let mut end = from.len();
            for (offset, byte) in from.bytes().enumerate() {
                match byte {
                    b'{' => depth += 1,
                    b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = offset;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            &from[..end]
        };
        let forced = format!("{}{}", ": Some", "(");
        let signed: Vec<&str> = block
            .match_indices(&forced)
            .map(|(offset, _)| {
                block[..offset]
                    .rsplit(|it: char| !(it.is_alphanumeric() || it == '_'))
                    .next()
                    .unwrap_or_default()
            })
            .collect();
        // ⛔⛔⛔⛔⛔ **THE CLASSIFIED LIST, AND ADDING TO IT IS A CLAIM** — this workspace's rule
        // 6: an unclassified field is RED and never a pass. A name belongs here only when its
        // value is something THIS IMAGE CAN READ NOW rather than something it had to count while
        // the run ran, which is exactly the distinction item 891 turns on.
        //
        // ⚠ `stood_down` qualifies: it is a flag on the record in front of us, so `Some(false)` is
        // a claim this image is entitled to make and its `None` belongs to an older log. A TALLY
        // never qualifies — route it through [`counted`].
        const MAY_BE_FORCED: [&str; 1] = ["stood_down"];
        let unclassified: Vec<&&str> = signed
            .iter()
            .filter(|name| !MAY_BE_FORCED.contains(name))
            .collect();
        assert!(
            unclassified.is_empty(),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 891 ⑶: {unclassified:?} reach the durable log as an \
             unconditional `Some`. If it is a TALLY, that is the laundering this item was filed \
             over — a run this daemon never incremented gets a zero signed as its count — and it \
             belongs in `counted`. If it is a fact this image can read NOW, classify it in \
             `MAY_BE_FORCED` with the reason. Unclassified is RED, not a pass. Found {signed:?}",
        );
    }

    /// ⛔⛔⛔⛔⛔ **EVERY TALLY THIS RECORD CARRIES IS ONE A POPULATION CAN BE ASKED ABOUT** —
    /// register item 895, and the three shapes a quoted number has to be able to tell apart.
    ///
    /// # ⛔⛔⛔⛔⛔ What was green: four predicates for one question, and two of them wrong
    ///
    /// [`Sampled`]'s doc records the four; two of them are not merely different spellings but give
    /// the WRONG answer on shapes this repository has already met and filed items over:
    ///
    /// | shape | `made + folded` (item 856's baseline) | `made > 0` | the truth |
    /// |---|---|---|---|
    /// | wedged: a prompt sat in a composer | *nothing typed* | *nothing typed* | it typed |
    /// | swallowed: every prompt vanished | *nothing typed* | *nothing typed* | it typed |
    ///
    /// Those are items 617 and 762 — each filed because a reader answered *this run typed nothing*
    /// about the one run that most needed the opposite said — and a population filter written the
    /// same way puts both runs OUTSIDE the denominator. So this gate drives the two shapes through
    /// the stored record and holds that both come back [`Sampled::Counted`].
    ///
    /// # ⚠⚠ And the third assertion is the one that covers the NEXT tally
    ///
    /// [`Tally::ALL`] is a hand-ordered array, which is the shape item 891 ⑶ warned about: fix one
    /// key and the next lands in the same place. So the record's REAL counter columns are derived
    /// from the type — write one row twice, once holding the tallies and once with them cleared
    /// through the fields, and the keys whose value goes null are exactly theirs — and compared
    /// with what this enum claims. A fifth counter added to [`PersistedRun`] and not named here is
    /// red before anybody quotes a number over it.
    #[test]
    fn every_tally_this_record_carries_is_one_a_population_can_be_asked_about() {
        /// A stored row carrying `deliveries` and nothing else counted.
        fn stored(deliveries: Option<sprag_plugin::Deliveries>) -> PersistedRun {
            let mut registry = RunRegistry::default();
            let id = registry.reserve();
            let progress = ProgressCell::default();
            lock(&progress).deliveries = deliveries;
            registry.submit(NewRun {
                id,
                label: "ai_loop pane=2".to_owned(),
                plugin: crate::plugins::PluginName::AiLoop,
                request: None,
                opened_by: None,
                opened_by_session: None,
                tree: None,
                overridden: None,
                state: Arc::new(Mutex::new(RunState::Done {
                    outcome: Box::new(an_outcome()),
                    output: None,
                    uncommitted: None,
                })),
                run: Box::new(EndedRun::restored(false, None, None)),
                progress,
            });
            registry.persistable().runs.remove(0)
        }

        // ── The two shapes the wrong predicates get wrong, both filed as their own items ──────
        let wedged = stored(Some(sprag_plugin::Deliveries {
            made: 0,
            folded: 0,
            released: 0,
            unsubmitted: 1,
            unreported: 0,
        }));
        let swallowed = stored(Some(sprag_plugin::Deliveries {
            made: 0,
            folded: 0,
            released: 0,
            unsubmitted: 0,
            unreported: 1,
        }));
        assert_eq!(
            [
                wedged.sampled(Tally::Deliveries),
                swallowed.sampled(Tally::Deliveries),
            ],
            [Sampled::Counted, Sampled::Counted],
            "⛔⛔⛔⛔⛔ REGISTER ITEM 895: a run whose prompt sat in a composer, or whose every \
             prompt was swallowed, fell OUT of its own population. Both have `made == 0` by \
             definition — items 617 and 762 — so a filter over `made`, or over `made + folded` \
             (which is what this register's own baseline command wrote), puts the runs it most \
             needs to count outside the denominator.",
        );

        // ── And the three arms are DISTINCT, driven through a real stored row each ────────────
        let counted = stored(Some(sprag_plugin::Deliveries {
            made: 3,
            folded: 1,
            released: 0,
            unsubmitted: 0,
            unreported: 0,
        }));
        let zeroed = stored(Some(sprag_plugin::Deliveries::NONE));
        let unsaid = stored(None);
        assert_eq!(
            [
                counted.sampled(Tally::Deliveries),
                zeroed.sampled(Tally::Deliveries),
                unsaid.sampled(Tally::Deliveries),
            ],
            [Sampled::Counted, Sampled::Zeroed, Sampled::Unsaid],
            "⛔⛔⛔⛔⛔ REGISTER ITEM 895: the three answers are not three. `zeroed` folded into \
             either neighbour decides 209 of the live store's 220 rows by fiat — into `counted` it \
             claims a sample from a build that may never have had the counter, and into `unsaid` \
             it throws away every genuine zero. Item 891's rule is that a column's shape is \
             retroactive and its values are not, so the middle arm is permanent.",
        );

        // ── And no tally is missing from the vocabulary ───────────────────────────────────────
        //
        // ⚠ DERIVED, NOT NAMED — the rule `watching-zenoh` handed this register and the one item
        // 891's gate above uses: take the population from the record's own structure, because a
        // hand list is what goes stale.
        //
        // ⚠ THE ROW HAS TO HOLD EVERY ONE OF THEM, or the difference below finds only the one this
        // fixture bothered to fill and the gate passes while claiming a population of one. The
        // first draft did exactly that and said so: `left: ["deliveries"]`.
        // ⚠⚠ AND IT IS WRITTEN AS A COUNT NOBODY TYPES — the list used to say *all four* in prose
        // and the fifth column (item 866(2)) arrived the day after the fourth, exactly as the
        // refusal below predicts. The arms are enumerated; the number is not.
        let holding = {
            let mut row = stored(Some(sprag_plugin::Deliveries::NONE));
            row.folds_by_reason = Some(sprag_plugin::FoldsByReason::NONE.into());
            row.delivered_by_road = Some(sprag_plugin::DeliveredByRoad::NONE.into());
            row.said_by_sentence = Some(sprag_plugin::SaidBySentence::NONE.into());
            row.width_withheld = Some(sprag_plugin::WidthWithheld::NONE.into());
            row
        };
        let full = serde_json::to_value(holding.clone()).expect("a record serialises");
        let cleared = {
            let mut row = holding;
            row.deliveries = None;
            row.folds_by_reason = None;
            row.delivered_by_road = None;
            row.said_by_sentence = None;
            row.width_withheld = None;
            serde_json::to_value(row).expect("and so does the cleared one")
        };
        let mut columns: Vec<&str> = full
            .as_object()
            .expect("a record is an object")
            .iter()
            .filter(|(name, held)| !held.is_null() && cleared[name].is_null())
            .map(|(name, _)| name.as_str())
            .collect();
        columns.sort_unstable();
        let mut claimed: Vec<&str> = Tally::ALL.map(Tally::word).to_vec();
        claimed.sort_unstable();
        assert_eq!(
            columns, claimed,
            "⛔⛔⛔⛔ REGISTER ITEM 895: `Tally::ALL` and the record's real counter columns have \
             come apart. A counter nobody can ask a population question about is one whose rate \
             gets taken with a fresh hand-written filter — which is this item — and item 891 ⑶ \
             measured that a new key lands in the same place within half a day of the last one \
             being fixed.",
        );
    }

    /// ⛔⛔⛔⛔⛔ **A FINISHED RUN STILL NAMES THE CONVERSATION THAT OPENED IT** — register item
    /// 893 ⑵, and the arm every existing gate for this column leaves out.
    ///
    /// # ⛔⛔⛔⛔⛔ What was green, and why the FINISHED arm is the whole gate
    ///
    /// `the_run_a_shutdown_left_behind_comes_back_interrupted_and_keeps_the_conversation_that_asked`
    /// drives this column across a restore already — of a run that is **still going**
    /// ([`RunState::Interrupted`], `finished == false`). Item 893's population is the opposite
    /// one: it is *which finished run is owed a next launch and by whom*, and a finished run is
    /// where this column becomes the ONLY evidence, because [`request`](PersistedRun::request)
    /// drops its map once a run ends — item 890 was filed after that guard erased the tree for
    /// **209 of 211 rows** and its own gate's doc says the same thing in as many words: *the
    /// FINISHED arm is the whole gate, and it is why this is not `request`'s test.*
    ///
    /// So the plausible defect is not exotic: it is this column acquiring `request`'s
    /// `!finished` guard, which no gate in this file would have noticed.
    ///
    /// # 📊 What the live store says, which is why this is ⑵ and not ⑴
    ///
    /// Item 893 was opened on `named == 0` over 74 owed runs. Re-measured 2026-09-05 through the
    /// product's own disposition table:
    ///
    /// ```text
    /// 220 rows · ended 202 · {a_person: 90, this_runs_opener: 77, nobody: 35}
    /// owed 77 · named 1 ⇒ [214]
    /// ```
    ///
    /// ⇒ **Run 214 is finished, is owed to its own opener, carries NO `request`, and names its
    /// conversation anyway** — so the column already crosses the restore in production and item
    /// 893's cause was *nobody was filling it* (item 871's half), not *the restore drops it*. What
    /// was never true is that anything held it there.
    ///
    /// # ⚠⚠ TWO ASKERS, because one is a fixture a constant passes
    ///
    /// Item 890's rule, learnt on the same file: *한 저장소짜리 픽스처에서는 미상과 정답이 같은
    /// 값이다.* One asker in, and a build that wrote a constant — the first row's answer, the
    /// daemon's own session — is green while every run is attributed to one conversation. And a
    /// third run asks nothing, so *absent* stays a real answer rather than becoming unreachable.
    #[test]
    fn a_finished_run_still_names_the_conversation_that_opened_it() {
        /// A run that has ENDED, opened by `asker`.
        fn ended(registry: &mut RunRegistry, asker: Option<&str>) -> RunId {
            let id = registry.reserve();
            registry.submit(NewRun {
                id,
                label: "ai_loop pane=2".to_owned(),
                plugin: crate::plugins::PluginName::AiLoop,
                request: None,
                opened_by: None,
                opened_by_session: asker.map(str::to_owned),
                tree: None,
                overridden: None,
                state: Arc::new(Mutex::new(RunState::Done {
                    outcome: Box::new(an_outcome()),
                    output: None,
                    uncommitted: None,
                })),
                run: Box::new(EndedRun::restored(false, None, None)),
                progress: ProgressCell::default(),
            });
            id
        }

        const ONE: &str = "a-conversation-that-asked";
        const OTHER: &str = "a-different-conversation";
        let mut registry = RunRegistry::default();
        let first = ended(&mut registry, Some(ONE));
        let second = ended(&mut registry, Some(OTHER));
        let nobody = ended(&mut registry, None);

        // ⚠ THROUGH THE FILE, never through the struct — a `#[serde(skip)]` or a guard that fires
        // only on the way out is invisible to a round trip that stays in memory.
        let on_disk = serde_json::to_string(&registry.persistable()).expect("the run log encodes");
        let read_back: RunLog = serde_json::from_str(&on_disk).expect("and decodes");
        assert!(
            read_back.runs.iter().all(|run| run.finished),
            "⚠ THE PREMISE: every row here must be FINISHED, or this gate is a second copy of the \
             interrupted-arm one it exists beside. Got {on_disk}",
        );

        let mut successor = RunRegistry::default();
        successor.restore(&read_back);
        let restored = successor.snapshot();
        let asker_of = |id: RunId| {
            restored
                .iter()
                .find(|run| run.id == id)
                .map(|run| run.opened_by_session.clone())
        };
        assert_eq!(
            [asker_of(first), asker_of(second), asker_of(nobody)],
            [
                Some(Some(ONE.to_owned())),
                Some(Some(OTHER.to_owned())),
                Some(None),
            ],
            "⛔⛔⛔⛔⛔ REGISTER ITEM 893 ⑵: a FINISHED run lost the conversation that opened it \
             across a restore, so the 77 runs this register says are owed a next launch are owed \
             to nobody it can name. `request` is dropped for a finished run — item 890 — which \
             makes this column the only evidence there is, and the plausible defect is exactly \
             that guard arriving here. Restored: {restored:?}",
        );

        // ── AND THE ROW SAYS IT, which is a hop of its own ───────────────────────────────────
        //
        // ⚠⚠ SEPARATE FROM THE ROUND TRIP — items 889, 894 and 891 each found a wire outside
        // their gate while the durable crossing was green. A reader asking *whose run is this*
        // reads the ROW, so a column that survives the file and never reaches the row is a fact
        // nobody can act on.
        let row = |id: RunId| {
            let run = restored
                .iter()
                .find(|run| run.id == id)
                .expect("the run came back");
            crate::plugins::run_to_json(run, None, crate::plugins::LiveLook::default())
                [crate::plugins::RUN_ASKED_BY_KEY]
                .clone()
        };
        assert_eq!(
            [row(first), row(second), row(nobody)],
            [
                serde_json::json!(ONE),
                serde_json::json!(OTHER),
                serde_json::Value::Null,
            ],
            "⛔⛔⛔⛔ REGISTER ITEM 893 ⑵, the row hop: a finished run's row did not name its \
             conversation, or named the same one twice. Item 865 was opened after a promotion had \
             to find a run's owner by messaging three sessions while this string sat unread — and \
             the third row is the control: *nobody asked* must stay an absence rather than \
             becoming a value nothing can produce.",
        );
    }

    /// ⛔⛔⛔⛔⛔ **A FINISHED RUN STILL SAYS WHICH REPOSITORY IT WAS FOR** — register item 890, and
    /// the one column here that does NOT wait for a run to be resumable.
    ///
    /// # ⛔⛔⛔⛔⛔ One daemon, three repositories, and 209 of 211 rows could name none
    ///
    /// Measured 2026-09-04 on this daemon's own store: 211 rows, **2 carrying a `request`** — the
    /// two that had not finished — and the repository lived nowhere else. [`PersistedRun::request`]
    /// drops the map for a finished run, correctly and for a stated reason (*a brief is a person's
    /// prose*), so the effect was that **every run anybody reads named no tree**. A watcher
    /// attributing runs 194–198 could not: the only surviving evidence was the live drivers'
    /// command lines and those drivers had exited.
    ///
    /// # ⚠⚠⚠ The FINISHED arm is the whole gate, and it is why this is not `request`'s test
    ///
    /// A run that is still going was never the problem. So the fixture ends its runs before
    /// persisting: a build that reused `request`'s `!finished && place.is_some()` guard passes
    /// every other assertion here and fails this one.
    ///
    /// # ⛔⛔⛔⛔⛔ AND THE POPULATION IS TWO REPOSITORIES, WHICH IS THE ITEM'S OWN DONE-WHEN
    ///
    /// *한 저장소짜리 픽스처에서는 미상과 정답이 같은 값이다.* One tree in, and a build that wrote
    /// a constant — the daemon's cwd, the first row's answer, the string this fixture happens to
    /// use — is green, while every run of every repository is still attributed to one. So two
    /// finished runs go in, in different trees, and the claim is that each keeps **its own** across
    /// the file. A third run records none, because *nobody wrote it down* is the answer 209 of the
    /// 211 measured rows have and it must not read as either tree.
    #[test]
    fn a_finished_run_still_says_which_repository_it_was_for() {
        /// One of the three trees this daemon drives — an absolute path, as `pane_start_dir` answers.
        const MINE: &str = "/home/coin/sprag";
        /// And another, so *unknown* and *right* are not the same value here.
        const ANOTHER: &str = "/home/coin/watching-zenoh";

        let mut registry = RunRegistry::default();
        let finished_in = |registry: &mut RunRegistry, tree: Option<&str>| -> RunId {
            let id = registry.reserve();
            registry.submit(NewRun {
                id,
                label: format!("ai_loop pane={}", id.0),
                plugin: crate::plugins::PluginName::AiLoop,
                // ⚠⚠ NO REQUEST, WHICH IS THE POINT. The map is what used to carry the repository,
                // and a finished run has none — so a build that answered out of it would answer
                // nothing here, exactly as the store measured.
                request: None,
                opened_by: None,
                opened_by_session: None,
                tree: tree.map(str::to_owned),
                overridden: None,
                // ⛔ FINISHED, and that is the arm: `request`'s guard would drop the column here.
                state: Arc::new(Mutex::new(RunState::Done {
                    outcome: Box::new(an_outcome()),
                    output: None,
                    uncommitted: None,
                })),
                run: Box::new(EndedRun::restored(false, None, None)),
                progress: ProgressCell::default(),
            });
            id
        };
        let mine = finished_in(&mut registry, Some(MINE));
        let another = finished_in(&mut registry, Some(ANOTHER));
        let unrecorded = finished_in(&mut registry, None);

        /// The tree a named run answers, out of a registry — by ID and never by position, so the
        /// gate keeps meaning what it says if the order these are held in ever changes.
        fn tree_of(registry: &RunRegistry, id: RunId) -> Option<String> {
            registry
                .snapshot()
                .into_iter()
                .find(|run| run.id == id)
                .unwrap_or_else(|| panic!("the fixture's own run {id:?} is in the registry"))
                .tree
        }

        // ══ ① EACH LIVE ROW SAYS ITS OWN ═══════════════════════════════════════════════════════
        for (id, tree) in [(mine, MINE), (another, ANOTHER)] {
            assert_eq!(
                tree_of(&registry, id).as_deref(),
                Some(tree),
                "⛔⛔⛔⛔⛔ REGISTER ITEM 890: the run's own summary cannot say which repository it \
                 was for, so nothing downstream can either",
            );
        }
        assert_eq!(
            tree_of(&registry, unrecorded),
            None,
            "⚠⚠ AND THE RUN NOBODY RECORDED A TREE FOR MUST NOT BORROW A NEIGHBOUR'S — this is 209 \
             of the 211 measured rows, and a build filling it in attributes them all to one tree",
        );

        // ══ ② THEY CROSS THE DAEMON, STILL TOLD APART ══════════════════════════════════════════
        //
        // ⚠⚠ THROUGH THE FILE, its neighbours' argument: item 606 measured that every run anybody
        // reads has been restored, so a tree that stopped at the daemon boundary would name a
        // repository only while nobody was asking.
        let on_disk = serde_json::to_string(&registry.persistable()).expect("the run log encodes");
        for tree in [MINE, ANOTHER] {
            assert!(
                on_disk.contains(tree),
                "⛔⛔⛔⛔⛔ REGISTER ITEM 890: the durable log of a FINISHED run does not carry its \
                 tree. That is the exact shape the store was measured in — 2 of 211 rows able to \
                 name a repository, and both unfinished. Wanted {tree}, got {on_disk}",
            );
        }
        let read_back: RunLog = serde_json::from_str(&on_disk).expect("and decodes");
        let mut successor = RunRegistry::default();
        successor.restore(&read_back);
        for (id, tree) in [(mine, MINE), (another, ANOTHER)] {
            assert_eq!(
                tree_of(&successor, id).as_deref(),
                Some(tree),
                "⛔⛔⛔⛔ REGISTER ITEM 890: the tree did not survive the daemon that recorded it — \
                 and the restore path is what the experiment of 2026-09-04 16:5x measured as the \
                 thing that empties this column, not the ending and not the kill",
            );
        }
        assert_ne!(
            tree_of(&successor, mine),
            tree_of(&successor, another),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 890's DONE-WHEN: two runs driven in DIFFERENT repositories \
             come back from the log naming the same one, so the column is carrying something other \
             than each run's own tree — which leaves the daemon's three loops exactly as \
             inseparable as they were",
        );
        assert_eq!(
            tree_of(&successor, unrecorded),
            None,
            "⚠⚠ AND THE UNRECORDED RUN STILL SAYS SO ACROSS THE FILE",
        );

        // ══ ③ AND A LOG WRITTEN BEFORE THE COLUMN READS AS *NOBODY RECORDED*, NOT AS A TREE ════
        //
        // ⛔⛔⛔ Rule 6, and register item 891's lesson one key over: the flattering misreading
        // here is worse than a blank, because a reader who filled it in would attribute the run to
        // whichever repository they happened to be standing in.
        let mut older: Value = serde_json::from_str(&on_disk).expect("the log just written parses");
        for run in older["runs"]
            .as_array_mut()
            .expect("a log holds an array of runs")
        {
            run.as_object_mut()
                .expect("a run is an object")
                .remove("tree");
        }
        let older = older.to_string();
        for tree in [MINE, ANOTHER] {
            assert!(
                !older.contains(tree),
                "⚠ THE PREMISE: this fixture must actually strip the column, or the decode below \
                 is reading the same document twice. Got {older}",
            );
        }
        let old_log: RunLog = serde_json::from_str(&older).expect(
            "⛔⛔⛔⛔ REGISTER ITEM 890: a run log written before this column existed must still \
             decode. Every boot of this daemon restores from one.",
        );
        let mut before = RunRegistry::default();
        before.restore(&old_log);
        for id in [mine, another, unrecorded] {
            assert_eq!(
                tree_of(&before, id),
                None,
                "⚠⚠ and it reads as *nobody recorded which tree*, which is the honest answer for a \
                 build that could not have said otherwise — never a repository this reader invented",
            );
        }
    }

    /// ⛔⛔⛔⛔⛔ **TWO RUNS UNDER ONE NUMBER ARE STILL TOLD APART** — register item 887, and the
    /// fixture is the failure itself rather than a model of it.
    ///
    /// # ⛔⛔⛔⛔⛔ `reserve`'s own doc said ids are never reused, and this daemon's state said no
    ///
    /// *"ids are monotonic and never reused, so a gap in them means only that a run did not start."*
    /// [`RunRegistry::restore`] raises `next_id` to `max(saved.id) + 1` **over the rows it finds**,
    /// so a successor restoring a log that is MISSING rows starts issuing numbers a predecessor
    /// already spent. Measured 2026-09-04 in this repository's own store: rows 199, 200 and 202 each
    /// name a run that began after the `/run/user/1000/loop/run<N>.log` bearing that number had
    /// already been finished by a different run, and the two runs the ledger had measured under 199
    /// and 200 have no row left at all.
    ///
    /// ⇒ **A number that names two runs joins them into one with no wrong line anywhere.** Every
    /// table this repository builds about its own loop — item 856's landing measurement included —
    /// joins a log to a row by that number.
    ///
    /// # ⚠⚠⚠⚠⚠ The staging IS the defect, which is what this gate could not be written without
    ///
    /// The predecessor's row is dropped from the log before the successor restores it, because that
    /// is what produces the collision: a successor that saw the row would never reissue its number.
    /// A fixture that handed two registries the same id directly would be asserting about a state
    /// this product cannot reach, and the assertion below opens by demanding the collision — a
    /// build that fixed the reuse instead makes this gate fail LOUDLY rather than pass vacuously,
    /// which is the correct outcome for a gate whose subject has been removed.
    ///
    /// ⚠ `(build, id)` is deliberately NOT the discriminator: all three reused rows measured were
    /// the SAME build, and both registries here are this image. A qualifier that works sometimes
    /// reads as a check and is worse than none.
    #[test]
    fn two_runs_under_one_number_are_still_told_apart() {
        /// A registry holding one submitted run, and the run's stamp.
        fn one_run(registry: &mut RunRegistry) -> (RunId, Option<WhichRun>) {
            let id = registry.reserve();
            registry.submit(NewRun {
                id,
                label: format!("ai_loop pane={}", id.0),
                plugin: crate::plugins::PluginName::AiLoop,
                request: None,
                opened_by: None,
                opened_by_session: None,
                tree: None,
                overridden: None,
                state: Arc::new(Mutex::new(RunState::Done {
                    outcome: Box::new(an_outcome()),
                    output: None,
                    uncommitted: None,
                })),
                run: Box::new(EndedRun::restored(false, None, None)),
                progress: ProgressCell::default(),
            });
            let summary = registry
                .snapshot()
                .into_iter()
                .find(|run| run.id == id)
                .expect("the run just submitted is in the directory");
            (id, summary.which_run)
        }

        // ══ ① THE PREDECESSOR RUNS, AND ITS LOG LOSES THE ROW ══════════════════════════════════
        let mut predecessor = RunRegistry::default();
        let (first, first_stamp) = one_run(&mut predecessor);
        let mut lossy: Value =
            serde_json::to_value(predecessor.persistable()).expect("the predecessor's log encodes");
        // ⚠⚠⚠⚠⚠ **THIS LINE IS THE DEFECT AND NOT A CONVENIENCE.** A log that still holds the row
        // makes the successor's `next_id` skip past it, and there is no collision to test. What was
        // measured is a successor restoring a log that had lost rows — which is why the numbers it
        // reissued were numbers a predecessor had spent.
        lossy["runs"] = serde_json::json!([]);
        let lossy: RunLog = serde_json::from_value(lossy).expect("and decodes");

        // ══ ② THE SUCCESSOR REISSUES THE SAME NUMBER ═══════════════════════════════════════════
        let mut successor = RunRegistry::default();
        successor.restore(&lossy);
        let (second, second_stamp) = one_run(&mut successor);

        assert_eq!(
            first, second,
            "⚠⚠⚠⚠⚠ THE STAGING: this gate is about two runs under ONE number, and these two have \
             different ones — so every assertion below is about a collision that did not happen. \
             If the reuse itself has been fixed, DELETE this gate and say so; do not leave it \
             passing on a premise that stopped being true",
        );

        // ══ THE INVARIANT ══════════════════════════════════════════════════════════════════════
        let first_stamp = first_stamp.expect("a run this image admitted carries a stamp");
        let second_stamp = second_stamp.expect("and so does the successor's");
        assert_ne!(
            first_stamp, second_stamp,
            "⛔⛔⛔⛔⛔ REGISTER ITEM 887: two different runs bear one number AND one stamp, so \
             nothing in this product can tell them apart. Every table that joins a run log to a \
             row by its number then reads two runs as one, with no wrong line anywhere — the \
             arithmetic stays clean and the population is wrong, which is the failure mode nothing \
             goes red for. Both are {first_stamp}",
        );

        // ── AND A PROGRAM CAN REFUSE THE JOIN ──
        let row = crate::plugins::run_to_json(
            successor
                .snapshot()
                .first()
                .expect("the successor holds its run"),
            None,
            crate::plugins::LiveLook::default(),
        );
        assert_eq!(
            crate::plugins::the_same_run(&row, first_stamp.as_str()),
            crate::plugins::SameRun::No,
            "⛔⛔⛔⛔⛔ REGISTER ITEM 887: a record written by the FIRST run joins cleanly onto the \
             SECOND run's row. That is the whole defect, and a predicate that cannot refuse it \
             leaves the refusal to whoever remembers to look: {row}",
        );
        assert_eq!(
            crate::plugins::the_same_run(&row, second_stamp.as_str()),
            crate::plugins::SameRun::Yes,
            "⚠⚠⚠ AND THE CONTROL: the row's OWN stamp must join. A predicate that refused \
             everything would satisfy the assertion above and be useless: {row}",
        );
    }

    /// ⛔⛔⛔⛔⛔ **A RUN'S STAMP SURVIVES THE DAEMON, AND IS NOT MINTED AGAIN** — register item 887,
    /// and the crossing the whole item turns on.
    ///
    /// # ⛔⛔⛔⛔⛔ The file is where the reuse comes FROM
    ///
    /// A successor sets `next_id` from the rows in this file, so the numbers in it are the exact
    /// numbers that repeat — and a stamp that lived only in memory would be gone on precisely the
    /// boot that needed it. Item 606's measurement is the other half: thirteen live runs on two
    /// daemons, **every one restored**, because a run is read after it ends.
    ///
    /// # ⚠⚠⚠⚠⚠ And it must NOT be re-minted, which is the assertion a round trip alone would miss
    ///
    /// A restore that stamped the restoring registry's own minting would give a run a new identity
    /// on every boot. Then *the same run seen twice* and *two runs under one number* would read
    /// alike — and a restart is the only moment the number can go wrong in the first place, so the
    /// re-minting build would be broken exactly where the fix is needed and green everywhere else.
    #[test]
    fn a_runs_stamp_survives_the_daemon_and_is_not_minted_again() {
        let mut registry = RunRegistry::default();
        let id = registry.reserve();
        registry.submit(NewRun {
            id,
            label: "ai_loop pane=2".to_owned(),
            plugin: crate::plugins::PluginName::AiLoop,
            request: None,
            opened_by: None,
            opened_by_session: None,
            tree: None,
            overridden: None,
            state: Arc::new(Mutex::new(RunState::Done {
                outcome: Box::new(an_outcome()),
                output: None,
                uncommitted: None,
            })),
            run: Box::new(EndedRun::restored(false, None, None)),
            progress: ProgressCell::default(),
        });
        let minted = registry.snapshot()[0]
            .which_run
            .clone()
            .expect("⚠⚠ THE PREMISE: a run this image admitted must carry a stamp");

        // ⚠⚠ THROUGH THE FILE, the neighbouring gates' argument: a field `serde` never writes would
        // still satisfy an in-process round trip.
        let on_disk = serde_json::to_string(&registry.persistable()).expect("the run log encodes");
        let read_back: RunLog = serde_json::from_str(&on_disk).expect("and decodes");
        let mut successor = RunRegistry::default();
        successor.restore(&read_back);

        assert_eq!(
            successor.snapshot()[0].which_run.as_ref(),
            Some(&minted),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 887: the stamp did not cross the daemon, or the restore minted \
             a fresh one. Either way the identity of a run changes when nobody touched the run — \
             and a restart is the only moment its NUMBER can go wrong, so this is the one crossing \
             the item cannot be paid without. Got {:?} from {on_disk}",
            successor.snapshot()[0].which_run,
        );
        // ⚠⚠⚠ AND THE SUCCESSOR'S MINTING REALLY IS A DIFFERENT ONE, which is what makes the
        // assertion above a claim rather than a coincidence: if two registries stamped alike, a
        // restore that re-minted would pass it.
        let fresh = successor.reserve();
        successor.submit(NewRun {
            id: fresh,
            label: "ai_loop pane=3".to_owned(),
            plugin: crate::plugins::PluginName::AiLoop,
            request: None,
            opened_by: None,
            opened_by_session: None,
            tree: None,
            overridden: None,
            state: Arc::new(Mutex::new(RunState::Done {
                outcome: Box::new(an_outcome()),
                output: None,
                uncommitted: None,
            })),
            run: Box::new(EndedRun::restored(false, None, None)),
            progress: ProgressCell::default(),
        });
        let born_here = successor
            .snapshot()
            .into_iter()
            .find(|run| run.id == fresh)
            .and_then(|run| run.which_run)
            .expect("the successor's own run carries its own stamp");
        assert_ne!(
            born_here, minted,
            "⚠⚠⚠⚠⚠ THE CONTROL: two registries must not stamp alike, or `the stamp survived` is a \
             claim two identical values would satisfy however they were produced",
        );

        // ⛔⛔⛔⛔⛔ ── AND A LOG WRITTEN BEFORE THE STAMP EXISTED READS AS *NOBODY SAID* ──
        //
        // Never as *the same run*. Driven through a real decode of a real older shape, because a
        // `#[serde(default)]` that was dropped would leave every pre-existing run log unreadable —
        // and this daemon restores from one on every boot.
        let mut older: Value = serde_json::from_str(&on_disk).expect("the log just written parses");
        older["runs"][0]
            .as_object_mut()
            .expect("a run is an object")
            .remove(crate::plugins::RUN_WHICH_RUN_KEY);
        let older = older.to_string();
        let old_log: RunLog = serde_json::from_str(&older).expect(
            "⛔⛔⛔⛔ REGISTER ITEM 887: a run log written before the stamp existed must still \
             decode. Every boot of this daemon restores from one.",
        );
        let mut before = RunRegistry::default();
        before.restore(&old_log);
        assert_eq!(
            before.snapshot()[0].which_run,
            None,
            "⛔⛔⛔⛔ AND THE ABSENCE IS CARRIED RATHER THAN FILLED IN. A restore that stamped its \
             own minting onto a run it did not mint would assert an identity for the one run whose \
             identity nobody recorded — which reads as *a different run* to every later comparison",
        );
    }

    /// ⛔⛔⛔⛔⛔ **WHO STOOD A RUN DOWN SURVIVES THE DAEMON THAT WAS TOLD** — register item 835,
    /// and **the crossing that decides whether that item is paid at all.**
    ///
    /// # ⛔⛔⛔⛔ The run another supervisor reads is always a restored one
    ///
    /// Item 835 is another repository's watcher meeting a run it had not stopped, reading *"a
    /// person asked this run to stand down"*, and re-launching it twice because *person* named
    /// nobody it could ask. **That reader is looking at a run that is over** — and item 606
    /// measured what that means here: thirteen live runs on two daemons, every one of them
    /// restored, because a run is read after it ends and the daemon that drove it is restarted
    /// between rounds. An orderer that died with its daemon would leave every reader in exactly
    /// the state this item was filed on.
    ///
    /// ⚠⚠⚠ **MEASURED, ON THIS ROUND'S OWN RULE.** Writing `stood_down_by: None` at the persist
    /// site was run against `sprag-host` and `sprag-gate` together on 2026-09-04: **the only red
    /// was the standing one (register item 837)** — the wire gate drives the door, the sentence
    /// gate drives the words, and the two lines between them were watched by nothing.
    ///
    /// ⚠⚠ **THROUGH THE FILE**, its neighbours' argument: a field `serde` never writes would still
    /// satisfy an in-process round trip, and this value is a STRUCT whose `session` is the half a
    /// reader actually goes to.
    #[test]
    fn who_stood_a_run_down_survives_the_daemon_that_was_told() {
        const WATCHER: &str = "the-other-repositorys-watcher";

        let mut registry = RunRegistry::default();
        let id = registry.reserve();
        let log = Arc::new(Mutex::new(Vec::new()));
        registry.submit(NewRun {
            id,
            label: "ai_loop pane=2".to_owned(),
            plugin: crate::plugins::PluginName::AiLoop,
            request: None,
            opened_by: None,
            opened_by_session: None,
            tree: None,
            overridden: None,
            state: Arc::new(Mutex::new(RunState::Done {
                outcome: Box::new(an_outcome()),
                output: None,
                uncommitted: None,
            })),
            run: Box::new(RecordingRun(Arc::clone(&log))),
            progress: ProgressCell::default(),
        });
        registry
            .stand_down(
                id,
                Some(StoodDownBy {
                    pane: 12,
                    session: Some(WATCHER.to_owned()),
                }),
            )
            .expect("the run is in the directory");

        // ⚠ THE PREMISE: the live registry really did learn it, or the restore below is carrying
        // an absence and every assertion is about nothing.
        let live = registry.snapshot();
        assert_eq!(
            live[0]
                .stood_down_by
                .as_ref()
                .and_then(|who| who.session.as_deref()),
            Some(WATCHER),
            "⚠⚠⚠ THE PREMISE: the order's provenance must reach the LIVE row first: {:?}",
            live[0].stood_down_by,
        );

        let on_disk = serde_json::to_string(&registry.persistable()).expect("the run log encodes");
        let read_back: RunLog = serde_json::from_str(&on_disk).expect("and decodes");
        let mut successor = RunRegistry::default();
        successor.restore(&read_back);

        let carried = successor.snapshot();
        assert_eq!(
            carried[0].stood_down_by,
            Some(StoodDownBy {
                pane: 12,
                session: Some(WATCHER.to_owned()),
            }),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 835: who stood this run down did not survive its daemon — and \
             the reader this item is about is looking at a RESTORED run by construction (item 606: \
             thirteen live runs, every one restored). A provenance that empties at the daemon \
             boundary is one nobody will ever be holding when they need it, which is the state \
             that had a stopped run re-launched twice. Got {:?} from {on_disk}",
            carried[0].stood_down_by,
        );

        // ── AND THE SENTENCE A RESTORED RUN PUBLISHES NAMES THEM ──
        //
        // ⚠⚠ Asserted here rather than left to the sentence's own gate: this is the one place the
        // RESTORED value and the renderer meet, and a provenance that crossed the file but never
        // reached the words would be a fact that dies at the mouth.
        let said = crate::plugins::stand_down_sentence(
            &carried[0].state,
            carried[0].stood_down_by.as_ref(),
        );
        assert!(
            said.contains(WATCHER) && !said.contains("a person asked"),
            "⛔⛔⛔⛔ AND THE RESTORED RUN'S OWN SENTENCE MUST NAME THEM. This is the sentence the \
             next supervisor reads, and *a person* is what it read before: {said:?}",
        );
    }

    /// ⛔⛔⛔⛔ **THE ANSWER TO *WAS MY WORK KEPT* SURVIVES THE DAEMON THAT MEASURED IT** — register
    /// item 616, the residue item 604 left behind and named rather than hid.
    ///
    /// # Why the answer had to travel
    ///
    /// Item 604 stopped [`crate::plugins::stand_down_sentence`] asserting a loss it had no way to
    /// know, by giving it a fact: the plugin says how much work it completed, in its own unit. That
    /// fact lived only in the live `Outcome`, so a restored run fell back to *this run does not
    /// report completed work* — honest, and useless to the person it is for.
    ///
    /// ⚠⚠⚠⚠⚠ **AND A RUN IS READ AFTER IT HAS ENDED**, by which time the daemon that drove it has
    /// usually been restarted — item 606 measured exactly that on this machine and found thirteen
    /// live runs, every one of them restored. A fact that dies at the first restart is a fact
    /// nobody will ever be holding when they need it.
    ///
    /// # ⚠⚠⚠ Why this may be restored where [`PersistedRun::at`] may not
    ///
    /// That field is a STATE NAME, a symbol whose meaning lives in a `.scxml`, so the saved word
    /// and this binary's vocabulary are only the same fact when the fingerprints agree — which is
    /// why `restore` refuses to leak it into the live cell. A banked COUNT has no such scope: three
    /// completed turns are three completed turns whatever the document said, and `"turn"` is a
    /// plain noun rather than a document symbol. The two decisions look alike and are not.
    ///
    /// ⚠⚠ **THROUGH THE FILE**, not through `persistable` alone — the neighbouring gate's argument:
    /// a field `serde` never writes would still satisfy an in-process round trip.
    #[test]
    fn the_answer_to_whether_work_was_kept_survives_the_daemon_that_measured_it() {
        let mut registry = RunRegistry::default();
        let id = registry.reserve();
        let progress = ProgressCell::default();
        lock(&progress).banked = Some(sprag_plugin::Banked {
            completed: 3,
            unit: "turn".into(),
        });
        registry.submit(NewRun {
            id,
            label: "ai_loop pane=2".to_owned(),
            plugin: crate::plugins::PluginName::AiLoop,
            // ⚠ These gates read what a FINISHED run persists, and item 543's door refuses a
            // finished run whatever it carries — so there is nothing here to carry.
            request: None,
            opened_by: None,
            opened_by_session: None,
            tree: None,
            overridden: None,
            state: Arc::new(Mutex::new(RunState::Done {
                outcome: Box::new(sprag_plugin::Outcome {
                    // ⚠ NOT a convergence, which is the whole point: the sentence's *work is
                    // banked* arm is the easy one, and item 604's harm was in every other ending.
                    state: sprag_plugin::OutcomeState::Failed,
                    banked: Some(sprag_plugin::Banked {
                        completed: 3,
                        unit: "turn".into(),
                    }),
                    ..an_outcome()
                }),
                output: None,
                uncommitted: None,
            })),
            run: Box::new(EndedRun::restored(false, None, None)),
            progress,
        });
        // ⚠⚠⚠ **NO ORDER IS GIVEN HERE, AND NONE IS NEEDED.** The assertions below call
        // `stand_down_sentence` directly, so what is under test is what that renderer SAYS about a
        // restored ending — the order flag only governs whether the host publishes the line at
        // all. ⚠ A `stand_down` call would fail anyway: this fixture's run is `EndedRun::restored`,
        // which answers `Unread` because a restored run reads no orders (register items 539/597) —
        // and reaching for one here cost a red before that was noticed.

        let on_disk = serde_json::to_string(&registry.persistable()).expect("the run log encodes");
        let read_back: RunLog = serde_json::from_str(&on_disk).expect("and decodes");
        let mut successor = RunRegistry::default();
        successor.restore(&read_back);

        let restored = successor.snapshot();
        // ⚠ The ORDERER is this gate's neighbour's subject (register item 835); what is under test
        // here is item 616's banked count surviving a restart, so it is left unrecorded.
        let said = crate::plugins::stand_down_sentence(
            &restored[0].state,
            restored[0].stood_down_by.as_ref(),
        );
        assert!(
            said.contains("3 turns"),
            "⛔⛔⛔⛔ ITEM 616: this run completed three turns and a restart lost the count, so the \
             person who typed `sprag stand-down` is told their ending cannot say what was kept — \
             on exactly the runs anybody reads. Said {said:?} from {on_disk}",
        );
        assert!(
            said.contains("BANKED and kept"),
            "⚠⚠⚠⚠ AND IT MUST BE THE SAME ANSWER A LIVE RUN GIVES, not a weaker one worded around \
             the gap: a restored run that says *cannot say what was kept* has not been repaired, \
             it has been made polite. Said {said:?}",
        );
    }

    /// ⛔⛔⛔⛔⛔ **WHICH ENDING A RUN CLOSED UNDER REACHES THE ROW AS A WORD, AND OUTLIVES THE
    /// DAEMON THAT HEARD IT** — register item 706's third requirement, the half that lives on this
    /// side of the crate boundary.
    ///
    /// # ⚠⚠⚠⚠⚠ What a reader had, and why it was not enough
    ///
    /// The word existed. `sprag_plugin::DoneReason` renders `word(): describe()` into the walk, so
    /// a consumer asking *did the stand-down I gave land?* could get an answer — **by parsing a
    /// sentence somebody else composed, against a vocabulary it had to re-spell.** Item 594
    /// measured the same collapse from the other side: all three endings publish `converged`, so a
    /// stood-down run's row was byte-identical to one nobody had ordered anything of.
    ///
    /// So this asserts the KEY: the word is on the row, under its own name, and a reader reaches it
    /// without a parse. ⚠ The other half — that the word on the ending is the one the DOCUMENT
    /// took, not a fixture's invention — is `the_walk_and_the_ending_both_say_which_close_it_was`
    /// in `sprag-plugin`, which drives three real runs to three real closes. Neither half is worth
    /// anything alone: that one cannot see a wire, and this one cannot see a document.
    ///
    /// # ⚠⚠⚠ Why the restart is part of the claim rather than a nicety
    ///
    /// **A run's WALK does not survive its daemon** — item 706's own third cost, measured across a
    /// restart: every run before the boundary held zero walk lines and every run after it kept
    /// them. The sentence this word used to live inside is therefore exactly what a restore cannot
    /// get back. So a `done_reason` that died with its daemon would leave a restored row saying
    /// `converged` with nothing beside it and no prose to fall back on — a strictly worse position
    /// than before this field existed. Item 606's finding is the general form: thirteen live runs
    /// on this machine, **every one of them restored**.
    ///
    /// ⚠⚠ **THROUGH THE FILE**, not through `persistable` alone — the neighbouring gates'
    /// argument: a field `serde` never writes would still satisfy an in-process round trip.
    ///
    /// ⚠⚠⚠ **AND THE CONTROL IS THE ABSENCE.** A run that named no ending must publish NO KEY —
    /// not a `null`, which a reader would have to have an opinion about. Without that clause a
    /// `done_reason` hard-wired to any constant would satisfy the assertion above, and the
    /// distinction *nobody named an ending* / *it ended for no reason* would be gone.
    #[test]
    fn which_ending_closed_a_run_is_a_word_on_the_row_and_survives_the_daemon() {
        /// The ending this fixture's run closed under — a person's order, which is the arm item
        /// 594 measured being lost and the one a reader is likeliest to be asking about.
        const STOOD_DOWN: &str = "stood_down";

        let mut registry = RunRegistry::default();
        let id = registry.reserve();
        let ended = sprag_plugin::Outcome {
            // ⚠⚠⚠ CONVERGED, and that is the whole reason the word beside it has to exist: this
            // row is byte-identical to a run nobody ordered anything of until `done_reason`
            // separates them.
            state: sprag_plugin::OutcomeState::Converged,
            done_reason: Some(std::borrow::Cow::Borrowed(STOOD_DOWN)),
            ..an_outcome()
        };

        // ── THE CONTROL COMES FIRST: a run that named no ending publishes no key at all ──
        let quiet = crate::plugins::outcome_to_json(&an_outcome());
        assert!(
            quiet.get(crate::plugins::RUN_DONE_REASON_KEY).is_none(),
            "⚠⚠⚠⚠⚠ THE PREMISE: an outcome that names no ending must carry NO key — absent is how \
             this wire says *nobody named one*, and a `null` would be a value every reader then \
             needs an opinion about. Without this the assertion below would pass on a build that \
             published a constant: {quiet}",
        );

        // ── AND THE WORD IS ON THE LIVE ROW ──
        let live = crate::plugins::outcome_to_json(&ended);
        assert_eq!(
            live.get(crate::plugins::RUN_DONE_REASON_KEY)
                .and_then(Value::as_str),
            Some(STOOD_DOWN),
            "⛔⛔⛔⛔⛔ ITEM 706 ③: this run closed because a person's order landed, and the row must \
             say so in one word. `state` cannot — all three of this loop's endings converge — so a \
             row without this key sends every consumer back into the walk's prose, which is where \
             the word already was: {live}",
        );

        registry.submit(NewRun {
            id,
            label: "ai_loop pane=2".to_owned(),
            plugin: crate::plugins::PluginName::AiLoop,
            // ⚠ Item 543's door refuses a finished run whatever it carries, so there is nothing
            // here to put back with — the neighbouring restore gates' shape.
            request: None,
            opened_by: None,
            opened_by_session: None,
            tree: None,
            overridden: None,
            state: Arc::new(Mutex::new(RunState::Done {
                outcome: Box::new(ended),
                output: None,
                uncommitted: None,
            })),
            run: Box::new(EndedRun::restored(false, None, None)),
            progress: ProgressCell::default(),
        });

        let on_disk = serde_json::to_string(&registry.persistable()).expect("the run log encodes");
        let read_back: RunLog = serde_json::from_str(&on_disk).expect("and decodes");
        let mut successor = RunRegistry::default();
        successor.restore(&read_back);

        let restored = successor.snapshot();
        let RunState::Done { outcome, .. } = &restored[0].state else {
            panic!(
                "a finished run comes back finished: {:?}",
                restored[0].state
            );
        };
        // ⚠⚠ ASSERTED THROUGH `outcome_to_json` AND NOT OFF THE FIELD, because the row is what a
        // person reads and a restore that filled the struct without reaching the wire would be
        // green against the field and silent for every reader.
        let after = crate::plugins::outcome_to_json(outcome);
        assert_eq!(
            after
                .get(crate::plugins::RUN_DONE_REASON_KEY)
                .and_then(Value::as_str),
            Some(STOOD_DOWN),
            "⛔⛔⛔⛔⛔ ITEM 706 ③ ACROSS A RESTART: the walk this word used to live inside does NOT \
             survive the daemon, so a `done_reason` that died with it would leave a restored row \
             saying `converged` with no prose left to parse — worse than the position this field \
             was written to repair. Restored {after} from {on_disk}",
        );
    }

    /// ⛔⛔⛔⛔⛔ **HOW BIG A BRIEF WAS COMES BACK AFTER THE DAEMON THAT TOOK IT DIED** — register
    /// item 719's second direction, and the RESTORE half of a claim whose write half is gated one
    /// crate over (`a_run_driven_somewhere_else_shows_what_it_delivered_and_banked`).
    ///
    /// # Why this level needs the restart more than its neighbours do
    ///
    /// *Was my work kept?* is at least asked of a run still going. **_What was that run handed?_ is
    /// asked almost only about a run that is over** — it is the question somebody asks while
    /// looking at a churn and wondering why, and item 719's own answer had to be measured by hand
    /// afterwards for exactly that reason. Item 606's finding is the general form: thirteen live
    /// runs on this machine, **every one of them restored**. A level that dies at the first restart
    /// is one nobody is ever holding when they need it.
    ///
    /// ⚠⚠ **THROUGH THE FILE AND INTO BOTH SLOTS.** `restore` fills an ending AND a live cell, and
    /// the row reads the report-or-cell pair while a stand-down-style reader reads the ending — so
    /// a restore that filled one of them would be green against whichever reader the gate happened
    /// to pick. Both are asserted.
    ///
    /// ⚠ Three byte counts and no document vocabulary, so unlike [`PersistedRun::at`] there is
    /// nothing here for a fingerprint to disagree about — [`PersistedBanked`]'s line, one value
    /// over.
    #[test]
    fn how_big_a_brief_was_comes_back_after_the_daemon_that_took_it_died() {
        let briefed = sprag_plugin::Briefing {
            north_star: 41,
            milestone: 1_984,
            reference: 7_000,
            // ⚠ AND THE PART A CALLER DID NOT WRITE — register item 762. It rides the same
            // whole-or-nothing road, so a round trip that dropped it would report a smaller run.
            working_rules: 1_195,
        };
        let mut registry = RunRegistry::default();
        let id = registry.reserve();
        let progress = ProgressCell::default();
        lock(&progress).briefed = Some(briefed);
        registry.submit(NewRun {
            id,
            label: "ai_loop pane=2".to_owned(),
            plugin: crate::plugins::PluginName::AiLoop,
            request: None,
            opened_by: None,
            opened_by_session: None,
            tree: None,
            overridden: None,
            state: Arc::new(Mutex::new(RunState::Done {
                outcome: Box::new(sprag_plugin::Outcome {
                    // ⚠ NOT a convergence: the run this question is asked about is the one that
                    // went wrong, which is the neighbour gate's argument too.
                    state: sprag_plugin::OutcomeState::Failed,
                    briefed: Some(briefed),
                    ..an_outcome()
                }),
                output: None,
                uncommitted: None,
            })),
            run: Box::new(EndedRun::restored(false, None, None)),
            progress,
        });

        let on_disk = serde_json::to_string(&registry.persistable()).expect("the run log encodes");
        assert!(
            on_disk.contains("7000"),
            "⚠⚠⚠ THE STAGING: the size has to be IN THE FILE, or whatever comes back below came \
             out of memory and this gate is about nothing: {on_disk}",
        );
        let read_back: RunLog = serde_json::from_str(&on_disk).expect("and decodes");
        let mut successor = RunRegistry::default();
        successor.restore(&read_back);

        let restored = successor.snapshot();
        let ended = match &restored[0].state {
            RunState::Done { outcome, .. } => outcome.briefed,
            other => panic!("the fixture's run must come back finished, not {other:?}"),
        };
        assert_eq!(
            ended,
            Some(briefed),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 719: this run was handed 9,025 bytes and the restart lost the \
             number, so the one question anybody asks about a churn — *what was it given?* — is \
             unanswerable on exactly the rows people read. A run is read after it ends, when the \
             daemon that took its brief is already gone. From {on_disk}",
        );
        assert_eq!(
            restored[0].progress.briefed,
            Some(briefed),
            "⚠⚠⚠⚠ AND INTO THE LIVE CELL TOO, because the ROW reads the report-or-cell pair while \
             an ending's reader reads the outcome — a restore that filled one slot would be green \
             against whichever reader a gate happened to choose, and silent for the other",
        );
    }

    /// An outcome for a run whose ending is not what the gate is about.
    fn an_outcome() -> sprag_plugin::Outcome {
        sprag_plugin::Outcome {
            state: sprag_plugin::OutcomeState::Converged,
            iterations: 6,
            cost: None,
            failure: None,
            stopped: None,
            answered: 0,
            screened: 0,
            deferred: None,
            unchecked: None,
            unadmitted: None,
            deliveries: sprag_plugin::Deliveries::NONE,
            checks: sprag_plugin::Checks::NONE,
            banked: None,
            briefed: None,
            // ⚠ The ending these gates are about is not a loop's, so no word is named — see
            // `Outcome::done_reason`.
            done_reason: None,
        }
    }

    /// The conversation the run in the gate above was started from — an opaque id, exactly as
    /// `Pane::agent_session` carries one, because nothing in this layer parses it.
    const A_CONVERSATION: &str = "13cac637-d86c-4fa3-8411-785d552cee16";

    /// ⚠⚠⚠⚠⚠ **A RUN'S PROVENANCE IS NOT LOST BY A ROUND TRIP THROUGH THE DISK** — the durable
    /// half of [`RunRegistry::restore`]'s rule 1, which the gate above cannot see.
    ///
    /// That gate calls `persistable` and `restore` in one process, so it would still pass if the
    /// conversation travelled in a field `serde` never wrote. This drives the actual FILE: encode,
    /// decode, restore. ⚠ It is the same distinction the run log's own version constant exists for
    /// — a format that decodes cleanly and answers wrongly is the failure mode here, not a refusal.
    ///
    /// ⚠⚠⚠ **AND IT PINS THAT THE VERSION DID NOT MOVE.** `opened_by_session` is `#[serde(default)]`
    /// on `build`'s argument, so a log written before the field existed must still LOAD rather than
    /// be thrown away — the second half below. Bumping `RUN_LOG_VERSION` would discard every run
    /// record a running daemon holds, which is a real cost paid for nothing.
    #[test]
    fn the_conversation_that_asked_survives_the_run_log_and_an_older_log_still_loads() {
        let log = RunLog {
            version: RUN_LOG_VERSION,
            runs: vec![PersistedRun {
                id: 4,
                label: "agent pane=3".to_owned(),
                // ⚠ Nothing to put this run back with — item 543. What this fixture measures is
                // what a restored run REPORTS, which is a different question from resuming one.
                request: None,
                iterations: 2,
                cost: None,
                unit: None,
                moved_at: None,
                ended_at: None,
                // ⚠ NOR THE INTERVAL ANYBODY WATCHED — item 888. Stamped by `durability`.
                ran_from: None,
                ran_to: None,
                finished: false,
                outcome: None,
                ceiling: None,
                output: None,
                build: None,
                which_run: None,
                driver: None,
                driving: None,
                opened_by_session: Some(A_CONVERSATION.to_owned()),
                tree: None,
                at: None,
                document: None,
                context_ceiling: None,
                context_high_water: None,
                // ⚠ NOR WHICH NUMBERS ITS CALLER TOOK — item 859. A log fixture answers no door.
                overridden: None,
                stood_down: None,
                stood_down_by: None,
                cancelled_by: None,
                deliveries: None,
                folds_by_reason: None,
                delivered_by_road: None,
                said_by_sentence: None,
                width_withheld: None,
                // ⚠ `None` is what an OLDER LOG reads as, which is what these fixtures are about.
                banked: None,
                briefed: None,
                // ⚠ Item 706's field, absent on the line above's argument.
                done_reason: None,
                // ⚠ And item 903's two, on the same argument.
                failure: None,
                blocked_by: None,
                place: None,
            }],
        };
        let on_disk = serde_json::to_string(&log).expect("the run log encodes");
        let read_back: RunLog = serde_json::from_str(&on_disk).expect("and decodes");
        let mut successor = RunRegistry::default();
        successor.restore(&read_back);
        assert_eq!(
            successor.snapshot()[0].opened_by_session.as_deref(),
            Some(A_CONVERSATION),
            "the conversation must cross the FILE, not merely the struct: {on_disk}",
        );

        // ── A LOG FROM BEFORE THE FIELD EXISTED: it loads, and says it does not know. ──
        let older = on_disk.replace(&format!(",\"opened_by_session\":\"{A_CONVERSATION}\""), "");
        assert!(
            !older.contains("opened_by_session"),
            "the older-log arm must actually remove the key or it proves nothing: {older}",
        );
        let older: RunLog = serde_json::from_str(&older)
            .expect("⚠ a log written before this field must LOAD, not be refused — see the field");
        let mut reader = RunRegistry::default();
        reader.restore(&older);
        assert_eq!(
            reader.snapshot()[0].opened_by_session,
            None,
            "and it answers `None` — nothing recorded which conversation asked, never a guess",
        );
    }

    /// **A DROP THAT HAS ONLY GONE PATIENT** — the backstop, not the instrument.
    ///
    /// ⚠⚠⚠⚠⚠ **THIS USED TO BE 50 ms AND IT WAS A PROXY.** The claim is *the drop ASKS before it
    /// waits*, and a tight wall-clock bound infers that from *the wait was short*. The inference is
    /// only as good as the machine: macOS CI measured **53.3 ms** on 2026-08-22 and reported *"it
    /// joined without asking the run to stop"* about a daemon that had asked correctly. That is a
    /// gate blaming the product for the weather — item 454's fifth face — and the third wall-clock
    /// assertion to do it here in one day.
    ///
    /// The two claims that bound was carrying are now asked directly and without a clock: the ask
    /// itself by [`a_worker_that_records_being_asked`], and the poll's ORDER by
    /// [`the_join_poll_is_short_enough_that_nobody_feels_a_shutdown`]. What is left for a stopwatch
    /// is the one thing neither can see — a `Drop` that sat for its whole deadline — so the number
    /// is now a fraction of [`RunRegistry::JOIN_DEADLINE`] rather than a multiple of a measurement.
    ///
    /// ⚠ Deliberately FAR from the 5.27 - 5.47 ms this actually takes (four samples, 2026-08-17):
    /// a backstop that a loaded runner can reach is a flake, and a backstop that only an unbounded
    /// wait can reach is a backstop.
    const A_DROP_THAT_WENT_PATIENT: Duration = Duration::from_secs(1);

    /// ⚠⚠⚠ **DROPPING A REGISTRY ASKS ITS RUNS TO STOP BEFORE IT WAITS FOR THEM.**
    ///
    /// The deadline made `Drop` bounded; it must not have made it PATIENT. A destructor that joined
    /// without raising the flag would hold every shutdown for the whole deadline and then DETACH a
    /// run that would have come back in milliseconds — which is worse than the unbounded join it
    /// replaced, because it loses the outcome as well as the time.
    ///
    /// ⚠⚠ ASKED OF THE RUN, NOT OF A CLOCK. The worker records whether the cancel flag was raised
    /// before it left, so *the drop asked* is an observation rather than something inferred from a
    /// short wait. See [`A_DROP_THAT_WENT_PATIENT`] for what the remaining stopwatch is still for.
    #[test]
    fn dropping_a_registry_asks_its_runs_to_stop_before_waiting_for_them() {
        let mut registry = RunRegistry::default();
        let id = registry.reserve();
        let (run, asked) = a_worker_that_records_being_asked(id);
        registry.submit(run);

        let raised = Instant::now();
        drop(registry);
        let waited = raised.elapsed();

        assert!(
            asked.load(Ordering::Acquire),
            "the run was never asked to stop — the drop joined a worker it had not cancelled, \
             which holds every shutdown for the whole deadline and then detaches a run that would \
             have come back in milliseconds (waited {waited:?})",
        );
        assert!(
            waited < A_DROP_THAT_WENT_PATIENT,
            "the drop waited {waited:?} — it asked, and then sat there anyway",
        );
    }

    /// ⚠⚠⚠ **AND THE POLL IS SHORT ENOUGH THAT NOBODY FEELS THE SHUTDOWN** — the other half of the
    /// bound that used to be one number, asked of the constant instead of of a clock.
    ///
    /// A drop that asks correctly still makes a person wait if it only listens for the answer every
    /// half second. That is a property of [`RunRegistry::JOIN_POLL`] alone, so it is asserted
    /// against an ABSOLUTE — item 377's rule, and the reason the old bound refused to be written as
    /// a multiple of this constant: a bound expressed in terms of the thing it guards moves with it
    /// and can never catch it.
    ///
    /// Twenty milliseconds is four times today's poll and two orders below the deadline: a poll
    /// raised to 400 ms — the case the old comment named — is red here, and no machine's load can
    /// make this test say anything at all, because it reads no clock.
    #[test]
    fn the_join_poll_is_short_enough_that_nobody_feels_a_shutdown() {
        assert!(
            RunRegistry::JOIN_POLL <= Duration::from_millis(20),
            "a shutdown listens for its runs every {:?}; a person asking a well-behaved daemon to \
             stop would perceive that wait",
            RunRegistry::JOIN_POLL,
        );
        assert!(
            RunRegistry::JOIN_POLL < RunRegistry::JOIN_DEADLINE,
            "the poll must fit inside the deadline it is polling towards",
        );
    }

    /// ⚠⚠⚠ **THE WORKER THAT WILL NOT COME BACK IS NAMED, AND THE ONE BESIDE IT IS STILL JOINED.**
    ///
    /// The deadline is over the whole SET, so the two claims are one gate: `n` wedged runs must not
    /// cost `n` deadlines, and a wedged one must not eat the wait a healthy one needed. An id absent
    /// from the answer is an id whose handle was taken, and [`RunRegistry::sweep`] is the only place
    /// that takes one — so absence here means JOINED and not merely finished.
    #[test]
    fn a_wedged_worker_is_named_at_the_deadline_and_does_not_starve_its_neighbour() {
        let released = Arc::new(AtomicBool::new(false));
        let mut registry = RunRegistry::default();
        let wedged = registry.reserve();
        registry.submit(a_worker_that_will_not_come_back(wedged, &released));
        let healthy = registry.reserve();
        registry.submit(a_worker_that_comes_back_after(
            healthy,
            Duration::from_millis(30),
        ));

        let within = Duration::from_millis(300);
        let raised = Instant::now();
        let outstanding = registry.join_all_within(within);
        let waited = raised.elapsed();
        released.store(true, Ordering::Release);

        assert_eq!(
            outstanding,
            vec![wedged],
            "only the worker that would not come back is left over",
        );
        assert!(
            waited >= within,
            "the wait ended at {waited:?}, before the deadline it was given",
        );
        assert!(
            waited < within * 4,
            "the wait ran past its own deadline: {waited:?}",
        );
    }

    /// ⚠⚠⚠⚠ **TWO WORKERS THAT WILL NOT COME BACK COST ONE DEADLINE, NOT TWO** — the other
    /// direction of the sentence its neighbour above only half-checks.
    ///
    /// That gate proves a wedged worker does not eat the wait a HEALTHY one needed. This one proves
    /// the bound is over the SET: the natural wrong shape — walk the records, give each its own
    /// budget in turn — passes up there and fails down here, and on a daemon holding a handful of
    /// wedged runs it is the difference between a shutdown and a hang with extra steps. A `Drop`
    /// that can be made arbitrarily long by adding runs is not bounded in the sense item 305 asked
    /// for; it is only bounded per run, which is a promise nobody can act on.
    #[test]
    fn two_wedged_workers_cost_one_deadline_between_them() {
        let released = Arc::new(AtomicBool::new(false));
        let mut registry = RunRegistry::default();
        let first = registry.reserve();
        registry.submit(a_worker_that_will_not_come_back(first, &released));
        let second = registry.reserve();
        registry.submit(a_worker_that_will_not_come_back(second, &released));

        let within = Duration::from_millis(150);
        let raised = Instant::now();
        let outstanding = registry.join_all_within(within);
        let waited = raised.elapsed();
        released.store(true, Ordering::Release);

        assert_eq!(
            outstanding,
            vec![first, second],
            "both workers are still going, so both are named",
        );
        assert!(
            waited >= within,
            "the wait ended at {waited:?}, before the deadline it was given",
        );
        // ⚠ THE SENTENCE OFFERS BOTH CAUSES rather than naming one. Raising `JOIN_POLL` past the
        // deadline fails this too, and a message that had blamed the per-worker shape would have
        // sent a reader looking for a loop that was never there.
        assert!(
            waited < within * 2,
            "two wedged runs cost {waited:?} against a {within:?} deadline — either the wait is \
             spent per worker, or it asks less often than the deadline it was given",
        );
    }

    /// ⛔⛔⛔⛔ **A RUN THAT OUTLIVED ITS DAEMON SAYS WHERE IT STOPPED, AND WHOSE WORD THAT IS** —
    /// register item 543, stage 3a, and the fact that had no channel at all.
    ///
    /// # What it could not say before
    ///
    /// An interrupted run came back with its counters, so a reader learned HOW FAR it got and never
    /// WHERE it stopped — `awaiting_human` and `working` were the same record, which is the
    /// difference between *waiting on me* and *killed mid-turn*. The position did exist: the loop
    /// writes `working --judged--> judging` into a step note. **That is a human sentence, in a
    /// journal bounded to sixty-four steps, that is deliberately not persisted** — unreadable by
    /// any program, truncated for a long run, and gone at exactly the moment it is wanted.
    ///
    /// # ⚠⚠⚠⚠⚠ Why the pair is the claim, and why the second half is the one with teeth
    ///
    /// A state name is a fact ABOUT A DOCUMENT. The restart that motivates persisting a run at all
    /// is *the loop document changed*, so reading the word back against a different `ai_loop.scxml`
    /// is the COMMON case rather than the rare one — item 544's version skew, in the place it would
    /// actually happen. So the record carries the fingerprint of the documents the word came from,
    /// and [`PersistedRun::resumable_here`] is the ONE place the two are compared: **a foreign
    /// document yields no word at all**, which is 544's *a changed document is a new run* as data
    /// rather than as a rule somebody has to remember.
    #[test]
    fn a_run_that_outlived_its_daemon_says_where_it_stopped_only_in_its_own_documents_words() {
        let saved = |at: Option<&str>, document: Option<&str>| PersistedRun {
            id: 7,
            label: "a loop that was interrupted".to_string(),
            // ⚠ This gate is about the WORD a person reads (`resumable_here`), not about putting
            // anything back, so the run carries nothing to be put back with — item 543.
            request: None,
            iterations: 12,
            cost: None,
            unit: None,
            moved_at: None,
            ended_at: None,
            // ⚠ NOR THE INTERVAL ANYBODY WATCHED — item 888. Stamped by `durability`, not here.
            ran_from: None,
            ran_to: None,
            finished: false,
            outcome: None,
            ceiling: None,
            output: None,
            build: None,
            which_run: None,
            driver: None,
            driving: None,
            opened_by_session: None,
            tree: None,
            at: at.map(str::to_owned),
            document: document.map(str::to_owned),
            context_ceiling: None,
            context_high_water: None,
            // ⚠ NOR WHICH NUMBERS ITS CALLER TOOK — item 859. A log fixture answers no door.
            overridden: None,
            stood_down: None,
            stood_down_by: None,
            cancelled_by: None,
            deliveries: None,
            folds_by_reason: None,
            delivered_by_road: None,
            said_by_sentence: None,
            width_withheld: None,
            // ⚠ `None` is what an OLDER LOG reads as, which is what this fixture is about.
            banked: None,
            briefed: None,
            // ⚠ Item 706's field, absent on the line above's argument.
            done_reason: None,
            // ⚠ And item 903's two, on the same argument.
            failure: None,
            blocked_by: None,
            // ⚠ This fixture is about the WORD, so it carries no place — which is also the
            // shape of every log written before item 543's field existed.
            place: None,
        };

        // ── THE WORD SURVIVES THE ROUND TRIP THROUGH THE FILE, which is the whole point: this is
        // read by a SUCCESSOR daemon, so anything that did not encode would be a field that works
        // only in the process that never needed it.
        let log = RunLog {
            version: RUN_LOG_VERSION,
            runs: vec![saved(
                Some("awaiting_human"),
                Some(sprag_plugin::STATECHARTS_FINGERPRINT),
            )],
        };
        let on_disk = serde_json::to_string(&log).expect("the run log encodes");
        let read_back: RunLog = serde_json::from_str(&on_disk).expect("and decodes");
        assert_eq!(
            read_back.runs[0].resumable_here(),
            Some("awaiting_human"),
            "⛔⛔⛔⛔ REGISTER ITEM 543: a run recorded by this build's documents must hand its \
             position back after the round trip. Without it an interrupted run can still only say \
             how far it got, and *waiting on me* is indistinguishable from *killed mid-turn*",
        );

        // ── AND A POSITION FROM SOMEBODY ELSE'S DOCUMENT IS NOT HANDED BACK AT ALL ──
        assert_eq!(
            saved(Some("awaiting_human"), Some("0000000000000000")).resumable_here(),
            None,
            "⛔⛔⛔⛔ REGISTER ITEM 544: a word from documents this build did not compile must not \
             be readable as a position. `awaiting_human` may name a different state, or none — and \
             the restart this record exists for is USUALLY a document change, so this is the \
             common reading rather than the rare one",
        );

        // ⚠⚠⚠ AND AN OLDER LOG — one written before either field existed — is the same refusal
        // arrived at by an ABSENCE rather than by a mismatch. A position with no document names a
        // vocabulary nobody can check, and treating that as local is the skew read backwards.
        assert_eq!(
            saved(Some("awaiting_human"), None).resumable_here(),
            None,
            "⚠⚠⚠ a position with no document must not be trusted as this build's",
        );
        assert_eq!(
            saved(None, Some(sprag_plugin::STATECHARTS_FINGERPRINT)).resumable_here(),
            None,
            "⚠⚠ and a fingerprint vouching for no position must answer nothing rather than \
             something empty",
        );

        // ⚠⚠ THE CONTROL THAT KEEPS THE FIRST ASSERTION FROM BEING VACUOUS: the two fingerprints
        // compared above must actually differ, or `resumable_here` would pass by answering the
        // same way to everything.
        assert_ne!(
            sprag_plugin::STATECHARTS_FINGERPRINT,
            "0000000000000000",
            "this build's documents must have a fingerprint of their own, or the comparison above \
             measured nothing",
        );
    }

    /// ⛔⛔⛔⛔ **THE WHOLE PLACE CROSSES THE LOG, NOT JUST THE WORD A PERSON READS** — register
    /// item 543's third brick.
    ///
    /// # ⚠⚠⚠⚠⚠ Why [`PersistedRun::at`] cannot be what a restart re-enters
    ///
    /// `at` is ONE state name and it exists for a person: *was my run mid-turn, or waiting on me?*
    /// A machine cannot be put back with it. `Engine::enter_at` takes the whole active set AND the
    /// current state, and it REFUSES a current that is not a member of that set — so a record
    /// carrying one word has, structurally, no way to become a resumed run. This is the field that
    /// carries what the engine actually takes, in the document's own words.
    ///
    /// ⚠⚠ **IT IS GATED ON THE SAME FINGERPRINT, THROUGH THE SAME KIND OF DOOR.** A configuration
    /// is even more a fact *about a document* than a single word is: rename one state and the set
    /// still decodes, still looks well-formed, and names a place that no longer exists. So
    /// `resumable_place` compares `document` exactly as `resumable_here` does, and a foreign
    /// document yields nothing at all — item 544's *a changed document is a new run*, as data.
    #[test]
    fn a_run_that_outlived_its_daemon_hands_back_the_whole_place_or_nothing() {
        let saved = |place: Option<Vec<String>>, document: Option<&str>| PersistedRun {
            id: 9,
            label: "a loop that was interrupted mid-turn".to_string(),
            // ⚠ The PLACE is what this gate reads, and `resumable_place` answers without one —
            // the pair rule lives at `resumable_request`, which has its own gate.
            request: None,
            // ⚠ NOR WHICH NUMBERS ITS CALLER TOOK — item 859. A log fixture answers no door.
            overridden: None,
            iterations: 12,
            cost: None,
            unit: None,
            moved_at: None,
            ended_at: None,
            // ⚠ NOR THE INTERVAL ANYBODY WATCHED — item 888. Stamped by `durability`, not here.
            ran_from: None,
            ran_to: None,
            finished: false,
            outcome: None,
            ceiling: None,
            output: None,
            build: None,
            which_run: None,
            driver: None,
            driving: None,
            opened_by_session: None,
            tree: None,
            at: None,
            document: document.map(str::to_owned),
            context_ceiling: None,
            context_high_water: None,
            stood_down: None,
            stood_down_by: None,
            cancelled_by: None,
            deliveries: None,
            folds_by_reason: None,
            delivered_by_road: None,
            said_by_sentence: None,
            width_withheld: None,
            banked: None,
            briefed: None,
            // ⚠ Item 706's field: these fixtures are about a PLACE crossing the file, and a run
            // that never closed names no ending.
            done_reason: None,
            // ⚠ And item 903's two, on the same argument.
            failure: None,
            blocked_by: None,
            place,
        };
        // ⚠ THE WORDS ARE THE PLUGIN'S OWN, taken from a real place rather than spelled here — a
        // fixture that invented state names would round-trip its own invention and say nothing
        // about whether this record can carry what a loop actually produces.
        let words = vec![
            "working".to_owned(),
            "work".to_owned(),
            "working".to_owned(),
        ];

        // ── IT SURVIVES THE FILE, because the reader is a SUCCESSOR daemon ──────────────────
        let log = RunLog {
            version: RUN_LOG_VERSION,
            runs: vec![saved(
                Some(words.clone()),
                Some(sprag_plugin::STATECHARTS_FINGERPRINT),
            )],
        };
        let on_disk = serde_json::to_string(&log).expect("the run log encodes");
        let read_back: RunLog = serde_json::from_str(&on_disk).expect("and decodes");
        assert_eq!(
            read_back.runs[0].resumable_place(),
            Some(words.as_slice()),
            "⛔⛔⛔⛔ REGISTER ITEM 543: a run's PLACE must survive the log. A daemon that restarts \
             has words on a disk and nothing else — a place that lives only in the process that \
             wrote it is exactly the run that dies with its daemon, which is this item.",
        );

        // ── AND A PLACE FROM SOMEBODY ELSE'S DOCUMENT IS NOT HANDED BACK AT ALL ─────────────
        assert_eq!(
            saved(Some(words.clone()), Some("0000000000000000")).resumable_place(),
            None,
            "⛔⛔⛔⛔ REGISTER ITEM 544: a configuration from documents this build did not compile \
             must not be readable as a place. It still DECODES — that is the danger — and naming a \
             state that has moved would put a resumed run somewhere nobody chose.",
        );
        assert_eq!(
            saved(Some(words), None).resumable_place(),
            None,
            "⚠⚠⚠ a place with no document names a vocabulary nobody can check",
        );
        assert_eq!(
            saved(None, Some(sprag_plugin::STATECHARTS_FINGERPRINT)).resumable_place(),
            None,
            "⚠⚠ and a fingerprint vouching for no place must answer nothing rather than an empty \
             something — `Some(&[])` would be a place the engine is entitled to be handed",
        );
    }

    /// ⛔⛔⛔⛔⛔ **EVERY WAY OF NOT COMING BACK IS A REASON A READER CAN ACT ON, AND THEY ARE FOUR
    /// DIFFERENT ANSWERS** — register item 737, the gate over [`PersistedRun::withheld`].
    ///
    /// # What was measured, and why the sibling gate above could not see it
    ///
    /// The gate above proves the refusals happen. This one proves they are SAYABLE. Until item 737
    /// they were not: `resumable_place` answered `None` four different ways, the boot turned all
    /// four into an empty list, and an empty list is also what a predecessor with no runs at all
    /// leaves behind. **Measured on this machine 2026-08-28**: the loop daemon's log held two
    /// unfinished runs whose places were recorded against `091c26165f46a34d` while the tree they
    /// were about to be promoted into fingerprints `3eabd86deafd4848` — so the next promotion was
    /// going to discard both, and every channel a person has said `interrupted`.
    ///
    /// ⚠⚠ **THE `None` ARM IS THE ONE THAT KEEPS THIS FROM BEING A RUBBER STAMP.** A reporter that
    /// named every restored run as withheld satisfies four assertions and answers nothing, which is
    /// why the run that comes through whole is asserted to name no reason at all.
    #[test]
    fn a_run_that_is_not_coming_back_says_which_of_the_four_reasons_kept_it() {
        let saved = |place: Option<Vec<String>>,
                     document: Option<&str>,
                     request: Option<serde_json::Map<String, serde_json::Value>>,
                     finished: bool| PersistedRun {
            id: 9,
            label: "a loop that was interrupted mid-turn".to_string(),
            request,
            // ⚠ NOR WHICH NUMBERS ITS CALLER TOOK — item 859. A log fixture answers no door.
            overridden: None,
            iterations: 12,
            cost: None,
            unit: None,
            moved_at: None,
            ended_at: None,
            // ⚠ NOR THE INTERVAL ANYBODY WATCHED — item 888. Stamped by `durability`.
            ran_from: None,
            ran_to: None,
            finished,
            outcome: None,
            ceiling: None,
            output: None,
            build: None,
            which_run: None,
            driver: None,
            driving: None,
            opened_by_session: None,
            tree: None,
            at: None,
            document: document.map(str::to_owned),
            context_ceiling: None,
            context_high_water: None,
            stood_down: None,
            stood_down_by: None,
            cancelled_by: None,
            deliveries: None,
            folds_by_reason: None,
            delivered_by_road: None,
            said_by_sentence: None,
            width_withheld: None,
            banked: None,
            briefed: None,
            // ⚠ Item 706's field: these fixtures are about a PLACE crossing the file, and a run
            // that never closed names no ending.
            done_reason: None,
            // ⚠ And item 903's two, on the same argument.
            failure: None,
            blocked_by: None,
            place,
        };
        let words = vec![
            "working".to_owned(),
            "work".to_owned(),
            "working".to_owned(),
        ];
        let here = sprag_plugin::STATECHARTS_FINGERPRINT;
        let asked = || {
            serde_json::json!({ "plugin": "orchestrator", "pane": 3 })
                .as_object()
                .cloned()
                .expect("an object")
        };

        // ⚠⚠ THE PREMISE, ASSERTED INSIDE: the foreign fingerprint must really be foreign. Against
        // a fixture that happened to spell this build's own documents, the arm below is unreachable
        // and its assertion passes by never being tested — which is the shape item 737 was filed
        // about in the first place, one level down.
        assert_ne!(
            "0000000000000000", here,
            "⚠⚠ this build's documents must have a fingerprint of their own",
        );

        // ── THE ONE A PROMOTION CAUSES, and the only arm whose cause is somebody's act ───────
        assert_eq!(
            saved(
                Some(words.clone()),
                Some("0000000000000000"),
                Some(asked()),
                false,
            )
            .withheld(),
            Some(Withheld::ForeignDocuments {
                theirs: "0000000000000000".to_owned()
            }),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 737: a run held back because a promotion changed the documents \
             must SAY so, carrying the fingerprint the log recorded. Without the number a reader \
             cannot tell this from a run that recorded nothing, and the remedy is different for \
             each.",
        );
        // ── AND THE THREE ABSENCES, which are three different things to go and look at ──────
        assert_eq!(
            saved(Some(words.clone()), None, Some(asked()), false).withheld(),
            Some(Withheld::NoDocument),
            "⚠⚠⚠ a place with no fingerprint beside it is a vocabulary nobody can check, and that \
             is not the same fact as a place from a build somebody named",
        );
        assert_eq!(
            saved(None, Some(here), Some(asked()), false).withheld(),
            Some(Withheld::NoPlace),
            "⚠⚠ a run that recorded no position has nothing to be put back AT, which is not a \
             refusal and must not read as one",
        );
        assert_eq!(
            saved(Some(words.clone()), Some(here), None, false).withheld(),
            Some(Withheld::NoRequest),
            "⚠⚠⚠ a place this build can read with nothing to rebuild the plugin from is a \
             predecessor that never wrote the request down — pointing that reader at documents \
             would send them after something that is not wrong",
        );

        // ── THE CONTROL: a run that comes back whole names NO reason ────────────────────────
        assert_eq!(
            saved(Some(words.clone()), Some(here), Some(asked()), false).withheld(),
            None,
            "⚠⚠⚠ A CONTROL FAILED: a run whose place and request both crossed the log was reported \
             as staying behind. `withheld` would then be a second name for *restored* and every \
             assertion above would be satisfied by a function that always answers.",
        );
        // ── AND SO DOES A RUN THAT IS OVER, on a claim rather than an exemption ─────────────
        assert_eq!(
            saved(None, None, None, true).withheld(),
            None,
            "⚠⚠ a FINISHED run is not waiting for anybody: its row already says what became of it, \
             and *it is not coming back* printed over a converged run buries the one line that \
             matters on every row a person reads",
        );
    }

    /// A run whose driver is **NOT A THREAD IN THIS PROCESS** — it records what it is told.
    ///
    /// ⚠⚠⚠ This is what makes the three gates below measure FORWARDING rather than storing. A
    /// registry that reached into a `RunRecord`'s own flags — which is what it did before register
    /// item 544's stage 2 — could not reach this at all, so its list stays empty and every gate
    /// reds. The recorder deliberately implements the OTHER three methods as *no driver*, which is
    /// [`EndedRun`]'s answer and the shape a directory entry takes when the driving lives elsewhere.
    struct RecordingRun(Arc<Mutex<Vec<RunOrder>>>);

    impl RunHandle for RecordingRun {
        fn deliver(&self, order: RunOrder) {
            lock(&self.0).push(order);
        }
        // ⚠⚠ ANSWERED FROM WHAT IT WAS TOLD, not from a second flag — item 594. A driver living
        // outside this process knows a standing order only through its own record of the delivery,
        // and a recorder that answered a bool set beside `deliver` would be agreeing with itself.
        fn stood_down(&self) -> bool {
            lock(&self.0)
                .iter()
                .any(|order| matches!(order, RunOrder::StandDown(_)))
        }
        // ⛔⛔⛔ AND WHO GAVE IT — register item 835, answered from the recorded ORDER for the
        // reason above: what a driver outside this process has is the delivery, and a recorder
        // holding a second copy would be agreeing with itself.
        //
        // ⚠ THE FIRST NAMED ONE, which is `Orders::deliver`'s own rule: a stand-down is idempotent,
        // so the orderer a reader needs is whoever decided — not whoever repeated it — and a later
        // order naming nobody must not erase a name already written.
        fn stood_down_by(&self) -> Option<StoodDownBy> {
            lock(&self.0).iter().find_map(|order| match order {
                RunOrder::StandDown(who) => who.clone(),
                _ => None,
            })
        }
        // ⚠ THE LAST ONE IT WAS TOLD, where the stand-down above takes ANY — the difference is the
        // difference between the two orders. A hold can be taken back, so a recorder that answered
        // *a `Hold` arrived once* would report a released run as still held.
        fn held(&self) -> bool {
            lock(&self.0)
                .iter()
                .rev()
                .find_map(|order| match order {
                    RunOrder::Hold(held) => Some(*held),
                    _ => None,
                })
                .unwrap_or(false)
        }
        // ⚠ THE FIRST ONE IT WAS TOLD — item 596's rule, answered the way an out-of-process driver
        // would have to: from its own record of what arrived, and taking the earliest because a
        // person's decision must not be overwritten by the shutdown that sweeps every run.
        // ⚠⚠ EVERYTHING, so this recorder measures FORWARDING and never the refusal: a double that
        // answered `false` would make every gate below assert an empty list and pass. The refusal
        // is driven where a real plugin answers it, one crate over.
        fn honours(&self, _order: sprag_plugin::StandingOrder) -> bool {
            true
        }

        fn cancelled_by(&self) -> Option<Canceller> {
            lock(&self.0).iter().find_map(|order| match order {
                RunOrder::Cancel(who) => Some(*who),
                _ => None,
            })
        }
        fn reapable(&self) -> bool {
            false
        }
        fn reap(&mut self) -> Option<String> {
            None
        }
        fn outstanding(&self) -> bool {
            false
        }
    }

    /// WHAT THE RUN HAS BEEN TOLD, as a VALUE — the only way the gates below read the recorder.
    ///
    /// # ⚠⚠⚠⚠⚠ A gate that locks inside an assertion HANGS INSTEAD OF FAILING
    ///
    /// `assert_eq!`'s format arguments are evaluated **only on the failing path**, so
    /// `assert_eq!(*lock(&log), want, "… {:?}", lock(&log))` is invisible while the gate is green
    /// and deadlocks against its own still-live guard the moment it has something to say. Measured
    /// 2026-08-21: a mutation that should have gone red in a second sat there for **93 minutes**,
    /// and **a mutation whose red is a HANG is a half gate** — a class this register already
    /// carries (item 534). Snapshotting first makes the trap unsayable rather than merely avoided.
    fn heard(log: &Arc<Mutex<Vec<RunOrder>>>) -> Vec<RunOrder> {
        lock(log).clone()
    }

    /// A registry holding one run of id `0` that is not a thread, plus the log it writes to.
    fn a_directory_holding_a_run_that_is_not_a_thread() -> (RunRegistry, Arc<Mutex<Vec<RunOrder>>>)
    {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut registry = RunRegistry::default();
        let id = registry.reserve();
        registry.submit(NewRun {
            id,
            label: "elsewhere".to_string(),
            // ⚠ THE PLUGIN IS IRRELEVANT TO THESE GATES and is stated rather than defaulted: the
            // handle is a recorder that honours everything, so what is measured is FORWARDING.
            plugin: crate::plugins::PluginName::Orchestrator,
            // ⚠ And neither is what would rebuild it — item 543, nor its bounds' authors — 853.
            request: None,
            opened_by: None,
            opened_by_session: None,
            tree: None,
            overridden: None,
            state: Arc::new(Mutex::new(RunState::Running)),
            run: Box::new(RecordingRun(Arc::clone(&log))),
            progress: ProgressCell::default(),
        });
        (registry, log)
    }

    /// ⛔⛔⛔⛔ **A CANCEL IS FORWARDED TO THE RUN, NOT STORED INTO IT** — register item 544's
    /// stage 2, and the first of one gate per order.
    ///
    /// # Why "forwarded" is the whole claim
    ///
    /// A run is a SUPERVISOR whose natural lifetime is the work; the daemon is a terminal
    /// multiplexer whose natural lifetime is weeks. They share one process, and the price is that
    /// **changing how an AI loop reflects requires restarting the thing that holds your PTYs.** The
    /// registry's orders were `Arc<AtomicBool>` stores reaching into a worker's memory, which is
    /// only sayable about a thread — so the registry could not have held a run driven from
    /// anywhere else even in principle. This asserts it now can: the run under test has no thread,
    /// no flags and nothing to reap, and the order still arrives.
    ///
    /// ⚠⚠ **AND THAT THE DIRECTORY STILL ANSWERS ONLY WHAT IT KNOWS.** The boolean means *there is
    /// such a run*, never *the driver acted* — an unknown id is `false` and nothing is delivered.
    #[test]
    fn a_cancel_is_forwarded_to_a_run_whose_driver_is_not_a_thread() {
        let (registry, log) = a_directory_holding_a_run_that_is_not_a_thread();

        assert!(registry.cancel(RunId(0)), "the run is in the directory");
        let told = heard(&log);
        assert_eq!(
            told,
            vec![RunOrder::Cancel(Canceller::Person)],
            "⛔⛔⛔⛔ REGISTER ITEM 544: a cancel must be DELIVERED to the run. {told:?} — an empty \
             list means the registry reached for a flag of its own instead, which is the fusion \
             this stage exists to undo, because a driver in another process has no flag here",
        );

        assert!(
            !registry.cancel(RunId(41)),
            "a run this directory does not hold must answer `false`",
        );
        let told = heard(&log);
        assert_eq!(
            told.len(),
            1,
            "⚠⚠ an order aimed at a run that does not exist must reach nobody — a directory that \
             delivered to the wrong entry would cancel a stranger's work. Got {told:?}",
        );

        // ⚠⚠⚠ AND SHUTDOWN'S BROADCAST GOES THE SAME WAY. `cancel_all` is what `Drop` uses so no
        // worker outlives the registry; had it kept storing into flags, every run driven from
        // elsewhere would have been left running by a daemon that believed it had stopped them.
        registry.cancel_all();
        let told = heard(&log);
        assert_eq!(
            told,
            vec![
                RunOrder::Cancel(Canceller::Person),
                RunOrder::Cancel(Canceller::Shutdown),
            ],
            "⛔⛔⛔ shutdown's broadcast must reach a run whose driver is not a thread — AND ARRIVE \
             AS A DIFFERENT ORDER FROM THE PERSON'S ONE ABOVE (register item 596). Both used to be \
             the bare word `Cancel`, so a run stopped by a daemon going away and a run somebody \
             deliberately stopped were the same delivery, the same flag and the same `cancelled` — \
             with opposite remedies. Got {told:?}",
        );
    }

    /// ⛔⛔⛔⛔ **A STAND-DOWN IS FORWARDED, AND IT IS NOT A CANCEL** — register item 544's stage 2,
    /// the second gate per order.
    ///
    /// ⚠⚠⚠ The pair of assertions is the claim. Cancel loses the turn in flight; stand-down banks
    /// the milestone and then stops, and **those are exactly the two outcomes the person raising
    /// one is choosing between** — so an implementation that forwarded either as the other would be
    /// wrong in the one way that matters, while passing any gate that only asked *did something
    /// arrive*.
    #[test]
    fn a_stand_down_is_forwarded_and_is_not_a_cancel() {
        let (registry, log) = a_directory_holding_a_run_that_is_not_a_thread();

        assert_eq!(
            registry.stand_down(RunId(0), None),
            Ok(()),
            "the run is in the directory and its handle reads the order",
        );
        let told = heard(&log);
        assert_eq!(
            told,
            vec![RunOrder::StandDown(None)],
            "⛔⛔⛔⛔ REGISTER ITEM 544: a stand-down must be DELIVERED, and as itself. {told:?} \
             instead means the two orders collapsed — the run that banked its milestone and the \
             run that lost it become indistinguishable from here",
        );
    }

    /// ⛔⛔⛔⛔ **A HOLD AND ITS RELEASE ARE FORWARDED AS THE TWO-WAY ORDER THEY ARE** — register
    /// item 544's stage 2, the third gate per order.
    ///
    /// ⚠⚠⚠ **THE RELEASE IS THE HALF A LATCH CANNOT CARRY**, which is why this order takes an
    /// argument where its two neighbours take none. Those are one-way on purpose: an un-ordering
    /// racing a milestone would make a run's ending depend on which message arrived first. A hold is
    /// a LEVEL a person raises and lowers, so a message type that could not say *lower it* would
    /// have quietly turned the one order a person can take back into one they cannot.
    #[test]
    fn a_hold_and_its_release_are_forwarded_as_the_two_way_order_they_are() {
        let (registry, log) = a_directory_holding_a_run_that_is_not_a_thread();

        // ⚠⚠ THE ANSWERS ARE THE LEVELS THIS RECORDER CANNOT SHOW — register item 694. `heard`
        // below proves both orders were DELIVERED and as themselves; what it cannot see is that the
        // first found a run running free and the second found one a person was holding, which is
        // the pair `resume-run` printed one sentence over.
        assert_eq!(
            registry.hold(RunId(0), true),
            Ok(Holding::Took),
            "the run is in the directory, its handle reads the order, and nobody was holding it",
        );
        assert_eq!(
            registry.hold(RunId(0), false),
            Ok(Holding::LetGo),
            "and it is still there, and a release is the same order lowered — over a run the line \
             above left held, so this one has something to let go",
        );
        let told = heard(&log);
        assert_eq!(
            told,
            vec![RunOrder::Hold(true), RunOrder::Hold(false)],
            "⛔⛔⛔⛔ REGISTER ITEM 544: both halves of a hold must be DELIVERED, and they must be \
             distinguishable. {told:?} instead means a person who asked to read something can stop \
             a run but never let it go again",
        );
    }

    /// ⚠⚠⚠⚠ **AND WHERE THE ORDERS DO REACH FLAGS, EACH REACHES ITS OWN** — the in-process half of
    /// the three gates above, which a recorder cannot see.
    ///
    /// The gates above prove the registry FORWARDS; this proves [`ThreadRun`] does not then pour
    /// three distinct orders into one flag. Both halves are needed and neither implies the other:
    /// a `deliver` whose arms all stored into `cancel` would pass every recorder gate written, and
    /// a registry that stored directly would pass this one.
    ///
    /// ⚠ Each order is checked against ALL THREE flags, so a mapping that moved a neighbour as well
    /// is red too — which is the failure a `match` with a copy-pasted arm actually makes.
    #[test]
    fn each_order_reaches_its_own_flag_and_leaves_its_neighbours_alone() {
        let read = |flags: &[&Arc<AtomicBool>]| -> Vec<bool> {
            flags.iter().map(|f| f.load(Ordering::Acquire)).collect()
        };
        let build = || {
            let cancel = Arc::new(AtomicBool::new(false));
            let stand = Arc::new(AtomicBool::new(false));
            let hold = Arc::new(AtomicBool::new(false));
            let run = ThreadRun::new(
                Orders::new(
                    Arc::clone(&cancel),
                    Arc::clone(&stand),
                    Arc::clone(&hold),
                    // ⚠ BOTH: this gate is about which FLAG each order reaches, so a handle that
                    // refused one would take that order's arm out of the measurement entirely.
                    sprag_plugin::StandingOrder::ALL.to_vec(),
                    RunId(0),
                    // ⚠ Nowhere to announce: what this gate reads are the flags themselves.
                    None,
                ),
                std::thread::spawn(|| {}),
            );
            (run, cancel, stand, hold)
        };

        let (run, cancel, stand, hold) = build();
        run.deliver(RunOrder::Cancel(Canceller::Person));
        assert_eq!(
            read(&[&cancel, &stand, &hold]),
            vec![true, false, false],
            "a cancel must raise the cancel flag and only that one",
        );
        // ⚠⚠⚠ AND THE REASON IS NOT ONE OF THE FLAGS — register item 596. The three booleans are
        // the WORKER's business, read from another thread on every turn; who asked is a READER's,
        // and lives beside them rather than among them. Asserting it here is what keeps the two
        // from being fused again by somebody who sees four facts and reaches for a fourth flag.
        assert_eq!(
            run.cancelled_by(),
            Some(Canceller::Person),
            "the run must remember WHO raised the cancel, not only that one was raised",
        );

        let (run, cancel, stand, hold) = build();
        run.deliver(RunOrder::StandDown(None));
        assert_eq!(
            read(&[&cancel, &stand, &hold]),
            vec![false, true, false],
            "a stand-down must raise the stand-down flag and only that one — pouring it into \
             `cancel` would lose the milestone the order exists to bank",
        );

        let (run, cancel, stand, hold) = build();
        run.deliver(RunOrder::Hold(true));
        assert_eq!(
            read(&[&cancel, &stand, &hold]),
            vec![false, false, true],
            "a hold must raise the hold flag and only that one",
        );
        run.deliver(RunOrder::Hold(false));
        assert_eq!(
            read(&[&cancel, &stand, &hold]),
            vec![false, false, false],
            "⚠⚠⚠ and a release must LOWER it: a hold stored as a latch (`store(true)` whatever it \
             was told) leaves a run held for ever by a person who already let go",
        );
    }

    /// ⚠⚠⚠ **A RESTORED RUN HAS NO DRIVER, AND SAYS SO AS A TYPE RATHER THAN AS THREE FLAGS NOBODY
    /// READS** — the second production implementation of [`RunHandle`], and the reason the seam is
    /// not a test fixture with a trait around it.
    ///
    /// Before this, `restore` minted a fresh `AtomicBool` per order, each carrying a comment saying
    /// that setting it did nothing because the worker that would have read it died with its daemon.
    /// Three write-only flags are a claim only prose was enforcing. What must stay true is the
    /// OBSERVABLE half: the run is in the directory, so an order aimed at it answers `true`, and it
    /// holds nothing a shutdown has to wait for.
    #[test]
    fn a_restored_run_accepts_every_order_and_keeps_no_driver() {
        let mut registry = RunRegistry::default();
        registry.restore(&RunLog {
            version: RUN_LOG_VERSION,
            runs: vec![PersistedRun {
                id: 4,
                label: "from a dead daemon".to_string(),
                // ⚠ Nothing to rebuild it from — item 543. This fixture is a log written before
                // requests crossed one, which is what every log on disk today is.
                request: None,
                iterations: 3,
                cost: None,
                unit: None,
                moved_at: None,
                ended_at: None,
                // ⚠ NOR THE INTERVAL ANYBODY WATCHED — item 888. Stamped by `durability`.
                ran_from: None,
                ran_to: None,
                finished: false,
                outcome: None,
                ceiling: None,
                output: None,
                build: None,
                which_run: None,
                driver: None,
                driving: None,
                opened_by_session: None,
                tree: None,
                at: None,
                document: None,
                context_ceiling: None,
                context_high_water: None,
                // ⚠ NOR WHICH NUMBERS ITS CALLER TOOK — item 859. A log fixture answers no door.
                overridden: None,
                // ⚠ A log with no such field: `None`, which restores as *no order was recorded*.
                stood_down: None,
                stood_down_by: None,
                // ⚠ And no cancel was recorded either, which is what an interrupted run looks
                // like: the daemon holding it went away without sweeping, so nobody raised one.
                cancelled_by: None,
                // ⚠ Nor what it delivered — item 606's field, absent in a log written before it.
                deliveries: None,
                folds_by_reason: None,
                delivered_by_road: None,
                said_by_sentence: None,
                width_withheld: None,
                // ⚠ Nor how much it banked — item 616's field, absent for that field's reason.
                banked: None,
                briefed: None,
                // ⚠ Nor which ending it closed under — item 706's field, on the same argument.
                done_reason: None,
                // ⚠ Nor why it failed or blocked — item 903's two, on the same argument.
                failure: None,
                blocked_by: None,
                place: None,
            }],
        });

        // ⚠⚠⚠ AND NOTHING IT IS TOLD BECOMES A STANDING ORDER — register item 594. This is read
        // BEFORE the orders below and again after, because the claim is that the pair does not
        // move: an `EndedRun` answers what it was RESTORED with, and a `stand_down` delivered to a
        // run whose driver died would otherwise publish *a person asked this run to stand down* on
        // a run that could never have heard it.
        assert!(
            !registry.snapshot()[0].stood_down,
            "a log that recorded no order must restore as no order",
        );

        assert!(
            registry.cancel(RunId(4)),
            "⚠⚠ a cancel must still FIND a restored run — that boolean answers *does this run \
             exist*, and it does",
        );
        // ⛔⛔⛔⛔ **AND THE TWO STANDING ORDERS ARE NOW REFUSED, WHICH IS THE CHANGE ITEMS 539 AND
        // 597 MADE.** This gate used to assert that all three were ACCEPTED and call that correct
        // because *the boolean answers does this run exist*. It does — and a person who stood down
        // a run whose driver died with its daemon was told their order had landed. Existence was
        // never the whole question for an order somebody has to READ.
        // ⚠ THE HOLD'S `Ok` IS DROPPED TO `()` SO THE TWO REFUSALS SIT IN ONE LIST — item 694 gave
        // it a level to answer and this gate is about the `Err` arm, which the two share. What the
        // level says when there IS one is driven where a run has a driver.
        for (order, answer) in [
            ("stand-down", registry.stand_down(RunId(4), None)),
            ("hold", registry.hold(RunId(4), true).map(|_| ())),
        ] {
            assert_eq!(
                answer,
                Err(Unordered::NoDriver),
                "⛔⛔⛔ ITEMS 539/597: a {order} over a run restored from a dead daemon must be \
                 REFUSED and say why. Nothing is driving it, so the order reaches nothing at all — \
                 and being told it landed is worse than being told it cannot",
            );
        }
        assert!(
            !registry.snapshot()[0].stood_down,
            "⚠⚠⚠⚠ ITEM 594: the stand-down above reached nothing, so the run must not now claim \
             somebody is standing it down. `EndedRun` answers the fact it was restored with and \
             never an order it was handed — a run with no driver cannot obey, and publishing the \
             order would promise a milestone that is never coming",
        );
        assert!(
            registry
                .join_all_within(Duration::from_millis(0))
                .is_empty(),
            "⚠⚠⚠ a run with no driver must not be something a shutdown waits for, or every \
             restart would pay the join deadline for runs that ended with the daemon before it",
        );
    }

    /// ⛔⛔⛔⛔⛔ **A RUN THAT COULD BE PUT BACK RECORDS WHAT WOULD PUT IT BACK — AND NOTHING ELSE
    /// DOES** — register item 543's sixth brick, at the WRITING end.
    ///
    /// # ⚠⚠⚠⚠⚠ Why the pair is what is asserted, at both ends, rather than the field
    ///
    /// A request and a place are only useful together. A request with no readable place would have
    /// a successor build the plugin and start it **from the top** — firing every `<onentry>` and
    /// re-typing the loop's opening prompt into somebody's pane, the exact failure item 543 exists
    /// to end. A place with no request is a configuration nothing can be entered into. So one rule
    /// decides at each end, and this is the one that decides what is WRITTEN.
    ///
    /// ⚠⚠ **AND IT IS A REQUEST, WHICH IS TO SAY IT IS A PERSON'S PROSE.** A brief is paragraphs
    /// somebody wrote; keeping it beside every finished `agent` run would put those paragraphs on
    /// disk for the life of a log that could never use them. The rule is not only correctness — it
    /// is what makes the field affordable.
    ///
    /// ⚠⚠⚠ **IT GOES THROUGH THE FILE'S OWN SHAPE**, not through the struct: what a successor
    /// daemon has is bytes on a disk written by a build that is gone, so a round trip that stayed
    /// in memory would be green over a field serde silently drops.
    ///
    /// ⚠ Two controls, one for each half of the rule, because either alone would leave the claim
    /// true of a log that records everybody's brief for ever.
    #[test]
    fn only_a_run_that_could_be_put_back_records_what_would_put_it_back() {
        let asked: serde_json::Map<String, serde_json::Value> = serde_json::json!({
            "plugin": "ai_loop",
            "pane": 3,
            "north_star": "A BRIEF SOMEBODY WROTE, WHICH IS WHY IT IS NOT KEPT FOR EVER",
        })
        .as_object()
        .expect("a request is an object")
        .clone();
        // ⚠ Only the SHAPE matters here — that words were recorded at all. Whether a machine can be
        // entered at them is `PersistedRun::resumable_place`'s question and `OuterLoop::resume_at`'s
        // answer, and both have gates of their own.
        let words = vec![
            "working".to_owned(),
            "work".to_owned(),
            "working".to_owned(),
        ];

        let written = |place: Option<Vec<String>>, ended: bool| {
            let mut registry = RunRegistry::default();
            let id = registry.reserve();
            let progress = ProgressCell::default();
            lock(&progress).place = place;
            registry.submit(NewRun {
                id,
                label: "ai_loop pane=3".to_owned(),
                plugin: crate::plugins::PluginName::AiLoop,
                request: Some(asked.clone()),
                opened_by: None,
                opened_by_session: None,
                tree: None,
                overridden: None,
                state: Arc::new(Mutex::new(if ended {
                    RunState::Reported(Box::new(serde_json::json!({ "state": "converged" })))
                } else {
                    RunState::Running
                })),
                // ⚠ No worker: what this reads is what the registry WRITES, and a thread would only
                // add a join for the drop to wait out.
                run: Box::new(EndedRun::restored(false, None, None)),
                progress,
            });
            let bytes = serde_json::to_string(&registry.persistable()).expect("a log serialises");
            serde_json::from_str::<RunLog>(&bytes).expect("and a successor reads it back")
        };

        // ── THE CLAIM: still going, with a place — so the request crosses ────────────────────
        let resumable = written(Some(words.clone()), false);
        assert_eq!(
            resumable.runs[0].place.as_deref(),
            Some(words.as_slice()),
            "⚠⚠ THE PREMISE: this run's place must reach the log, or the claim below is about a \
             record nothing could act on whatever request it carried",
        );
        assert_eq!(
            resumable.runs[0].resumable_request(),
            Some(&asked),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 543: a run still going, whose machine's place was recorded, \
             left its successor NOTHING to rebuild its plugin from. A place alone is a \
             configuration with nothing to enter it into — five rounds of carrying one, and the \
             restart still kills the run.",
        );

        // ── CONTROL 1: a run whose ending was recorded carries nothing ───────────────────────
        let over = written(Some(words.clone()), true);
        assert!(
            over.runs[0].request.is_none(),
            "⚠⚠⚠ A CONTROL FAILED: a run whose ending is on the record kept somebody's brief on \
             disk. It cannot be resumed — a reader has already seen its outcome — so the prose is \
             being kept for a successor that may never do anything with it. Wrote: {:?}",
            over.runs[0].request,
        );
        assert!(
            over.runs[0].resumable_request().is_none(),
            "⚠⚠⚠ AND THE READING END MUST AGREE: a finished run is not resumable whatever it \
             carries, or an older log's request would start work nobody asked for.",
        );

        // ── CONTROL 2: THE DOOR ITSELF refuses a place from another build's documents ────────
        //
        // ⚠⚠⚠⚠⚠ **THIS CONTROL EXISTS BECAUSE A MUTATION WAS GREEN WITHOUT IT.** Deleting
        // `resumable_request`'s check of `resumable_place` changed nothing measurable, and the
        // reason is that its one caller today (`RunRegistry::restore`) fills the record's place
        // from that same guarded door and then `inherited` requires a place — so the rule was
        // being enforced twice downstream and never here. That is a door standing open behind two
        // closed ones: **this is a `pub` reader whose whole contract is *a request a caller has
        // earned the right to act on, or nothing*,** and the next caller is under no obligation to
        // re-check what it already promised. So the door is measured directly.
        //
        // ⚠ It is built by MOVING one field of the record above, so everything else about it is a
        // record this daemon really wrote — a hand-made fixture could be refused for some other
        // reason and look like this passing.
        let mut foreign = resumable.runs[0].clone();
        foreign.document = Some("0000000000000000".to_owned());
        assert!(
            foreign.resumable_request().is_none(),
            "⛔⛔⛔⛔ REGISTER ITEM 544: a request was handed back beside a place recorded against \
             documents this build does not have. Nothing migrates a configuration between \
             documents, so acting on that pair enters a run into a document it never ran in — and \
             the two halves are only ever useful together, which is why one door decides.",
        );

        // ── CONTROL 3: a run with no place carries nothing either ────────────────────────────
        let placeless = written(None, false);
        assert!(
            placeless.runs[0].request.is_none(),
            "⚠⚠⚠ A CONTROL FAILED: a run whose machine was never saved kept its request. Nothing \
             could resume it — there is no place to put it back at — so this is a person's brief \
             recorded for a reader that cannot use it, on EVERY run this daemon ever drives that \
             walks no statechart. Wrote: {:?}",
            placeless.runs[0].request,
        );
    }

    /// ⛔⛔⛔⛔⛔ **THE PANES A LOOP WAS STILL TYPING AT, AND THE THREE THAT LOOK LIKE THEM** —
    /// register item 869, the population half.
    ///
    /// # ⛔⛔⛔⛔⛔ Why the population is the assertion and not the headline
    ///
    /// This set decides which panes a restore brings back WITHOUT their conversation, so every way
    /// it can be wrong is a way the daemon takes something from somebody. Too wide and a person's
    /// own `claude` loses the conversation they left it in — silently, on a reboot they did not
    /// ask for. Too narrow and the defect this item is about comes straight back, because the
    /// **default is to resume**: a pane this set forgets is a pane that is resumed, so an
    /// under-counted population fails GREEN. Hence three controls against one headline, each a
    /// different way of being *almost* a loop's pane.
    ///
    /// ⚠⚠ **AND IT IS DRIVEN THROUGH THE PRODUCT'S OWN READER.** The fixture is the JSON a
    /// predecessor leaves on disk, decoded by the `serde` implementation `crate::load_runs` uses,
    /// so the arm about a log written before `driving` existed is a real absent key rather than a
    /// `None` this file typed. A hand-built struct would have asserted its own input there.
    #[test]
    fn a_restore_takes_the_conversation_only_from_panes_a_loop_was_still_typing_at() {
        // ⚠ `finished` and `place` are the record's required words; everything varying here rides
        // on `#[serde(default)]`, which is the compatibility this gate's third arm is about.
        let log: RunLog = serde_json::from_value(serde_json::json!({
            "version": RUN_LOG_VERSION,
            "runs": [
                // ── THE HEADLINE: a loop still going, on the pane it had REPLACED its way to ──
                { "id": 1, "label": "ai_loop pane=996", "iterations": 41,
                  "finished": false, "place": ["working"], "driving": 1010 },
                // ── ① A RUN THAT ENDED. Its pane may well be a person's now, and the run that
                //    made it a loop's is over — resuming it is right.
                { "id": 2, "label": "ai_loop pane=983", "iterations": 53,
                  "finished": true, "place": ["failed"], "driving": 983 },
                // ── ② A LOG WRITTEN BEFORE `driving` EXISTED. Measured at 0 of 111 records and
                //    again at 0 of 113 (see `PersistedRun::driving`): the field is young, and a
                //    daemon that read a missing key as *pane 0* would strip the conversation off
                //    whichever pane happened to be numbered that.
                { "id": 3, "label": "ai_loop pane=7", "iterations": 4,
                  "finished": false, "place": ["working"] },
                // ── ③ A RUN THAT IS NOT A LOOP AT ALL, still going, typing at a pane. Nothing
                //    here reads the plugin's name, and this arm is what says that is deliberate:
                //    what makes the conversation replaceable is that SOMETHING is driving the
                //    pane, which is exactly what a run being unfinished says.
                { "id": 4, "label": "answer pane=933", "iterations": 1,
                  "finished": false, "place": ["asking"], "driving": 44 },
            ]
        }))
        .expect("the log a predecessor leaves is what this reads");

        let panes = log.panes_a_loop_was_driving();
        assert!(
            panes.contains(&PaneId(1010)),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 869: the pane a loop was still typing at is not in the set, so \
             the restore resumes it — and the loop comes back holding a conversation it must spend \
             its one context-shedding move to be rid of. Measured over four promotions and three \
             repositories, exception 0. Got {panes:?}",
        );
        assert!(
            panes.contains(&PaneId(44)),
            "⚠⚠ a run that is not a loop but is still DRIVING a pane belongs here too: what makes \
             a conversation replaceable is that something is driving it. Got {panes:?}",
        );
        assert!(
            !panes.contains(&PaneId(983)),
            "⛔⛔⛔⛔⛔ A FINISHED RUN'S PANE WAS TAKEN. Nothing is driving it, so the next thing to \
             open that conversation is a PERSON — and this daemon would delete it out from under \
             them on a reboot nobody asked for. Got {panes:?}",
        );
        assert!(
            !panes.contains(&PaneId(0)),
            "⚠⚠⚠ A LOG WRITTEN BEFORE `driving` EXISTED READ AS PANE 0. The field was absent from \
             all 111 records in the first log measured for it; a build that read that as a number \
             would strip whichever pane wore it. Got {panes:?}",
        );
        assert_eq!(
            panes.len(),
            2,
            "⚠⚠ AND NOTHING ELSE — four records, two panes. A set that grew would be one of the \
             controls above passing for the wrong reason: {panes:?}",
        );

        // ── AND THE RULE THE DAEMON ACTUALLY INSTALLS, over the same log ──
        //
        // ⚠⚠⚠ `crate::replaced_conversations` is what `sprag-term`'s boot calls, so the crossing
        // *log → predicate* is driven here rather than left to the one line in a binary's `main`.
        // Item 856 measured what an ungated hop costs: gates on both sides, green, and the value
        // never arriving.
        let asks = crate::replaced_conversations(Some(&log));
        assert!(
            asks(PaneId(1010)) && !asks(PaneId(983)),
            "⚠⚠⚠⚠⚠ the predicate the daemon installs must answer as its own population does, or \
             this file's set is a fact nothing acts on",
        );
        assert!(
            !crate::replaced_conversations(None)(PaneId(1010)),
            "⚠⚠ AND A DAEMON WITH NO PREDECESSOR TAKES NOTHING: a first boot has no loop to \
             protect and every pane it restores is somebody's own",
        );
    }

    /// 🎯🎯🎯🎯🎯 **THE DELAY BETWEEN A RUN ENDING AND THE NEXT ONE STARTING IS A NUMBER THIS BUILD
    /// COMPUTES** — register item 872 ⑶, which has stood open through four re-judgements because
    /// nothing ever read the two columns item 888 built for it.
    ///
    /// # ⛔⛔⛔⛔⛔ Why the unmeasurable half is asserted as hard as the measurable one
    ///
    /// The default answer here is SILENCE: a run this cannot pair yields no stretch, so a report of
    /// stretches alone reads *nothing to see* for a store in which nothing is pairable — and that
    /// is today's store exactly (2026-09-05T07:53:28Z: 229 runs, 229 of them unmeasurable). An
    /// under-counted population therefore fails **green**, which is the shape this gate exists
    /// against. Hence the sum: every run is the left end of one stretch or counted under one reason,
    /// and both halves are checked against the log's own length.
    ///
    /// # ⚠⚠⚠ And the grouping is an assertion, not a detail
    ///
    /// One daemon drives three repositories and their runs interleave by id — measured
    /// 2026-09-05T07:46:04Z, ids 224, 225 and 227 were watching-zenoh, pinion and sprag. A build
    /// that paired *the next id* would time one repository's death against another's birth and
    /// print a number that means nothing, which is worse than the silence it replaced. The second
    /// tree here is that control.
    #[test]
    fn how_long_a_tree_had_nothing_driving_it_is_measured_and_what_cannot_be_is_named() {
        /// One row, with only the words the record requires spelled out — everything this gate
        /// varies rides on `#[serde(default)]`, which is the same compatibility the store's real
        /// rows are made of.
        fn run(
            id: u64,
            tree: Option<&str>,
            finished: bool,
            from: Option<u64>,
            to: Option<u64>,
        ) -> serde_json::Value {
            built(id, tree, finished, from, to, "b1")
        }

        /// The same row, saying WHICH DAEMON wrote it — the fact `daemons_replaced_since` reads,
        /// and the only evidence in a log that a run's daemon is gone rather than slow.
        fn built(
            id: u64,
            tree: Option<&str>,
            finished: bool,
            from: Option<u64>,
            to: Option<u64>,
            build: &str,
        ) -> serde_json::Value {
            let mut row = serde_json::json!({
                "id": id, "label": format!("ai_loop pane={id}"),
                "iterations": 3, "finished": finished, "place": ["working"],
                "build": build,
            });
            if let Some(tree) = tree {
                row["tree"] = serde_json::json!(tree);
            }
            if let Some(from) = from {
                row["ran_from"] = serde_json::json!(from);
            }
            if let Some(to) = to {
                row["ran_to"] = serde_json::json!(to);
            }
            row
        }

        /// The same row, finished and SAYING HOW — the fact
        /// [`sprag_plugin::driver::Disposition::of_outcome_word`] reads, and the only thing that
        /// can earn the word *yet* for the newest run of a tree.
        ///
        /// ⚠ Before this existed no finished row in this fixture carried an outcome, so every one
        /// of the five `NothingFollowed` rows was promising a successor on no evidence at all.
        fn ended(
            id: u64,
            tree: &str,
            from: Option<u64>,
            to: Option<u64>,
            outcome: &str,
        ) -> serde_json::Value {
            let mut row = run(id, Some(tree), true, from, to);
            row["outcome"] = serde_json::json!(outcome);
            // ⛔⛔⛔⛔⛔ AND WHO OPENED IT, because `Opener::ThisRunsOpener` owes the next run to
            // *the party on this run's own record* and answers NOBODY where that is absent. Every
            // ended row here names one so the arms below are about the ENDING; the row that does
            // not is called out where it sits.
            row["opened_by_session"] = serde_json::json!("the conversation that asked");
            row
        }

        /// The same ended row with **no opener on record** — the shape the live store handed this
        /// item, and the only thing separating it from its neighbour.
        fn ended_unopened(
            id: u64,
            tree: &str,
            from: Option<u64>,
            to: Option<u64>,
            outcome: &str,
        ) -> serde_json::Value {
            let mut row = ended(id, tree, from, to, outcome);
            row.as_object_mut()
                .expect("a run row is an object")
                .remove("opened_by_session");
            row
        }

        let log: RunLog = serde_json::from_value(serde_json::json!({
            "version": RUN_LOG_VERSION,
            "runs": [
                // ⛔⛔⛔⛔⛔ FIRST, AND IT IS A DEAD DAEMON'S ROW — the arm the promotion of
                //    2026-09-05 forced out of `StillRunning`. It never finished and it never will:
                //    every row after it was written by a different build, which is this log's own
                //    proof that the daemon which would have watched it stop is gone. ⚠ It is FIRST
                //    because the evidence is *a later row of another build*, so its position is
                //    what makes it abandoned and run 7's position is what keeps run 7 merely
                //    unfinished — the two cannot be told apart by any column on the run itself.
                built(0, Some("/e"), false, Some(5), None, "b0"),
                // ── THE HEADLINE, on tree /a: run 1 stopped at 100, run 2 began at 3629 ──
                //    3529 seconds is 58m49s, which is the shape of item 827's own 3h49m.
                run(1, Some("/a"), true, Some(40), Some(100)),
                run(2, Some("/a"), true, Some(3629), Some(4000)),
                // ⚠ THE INTERLEAVING CONTROL. By id this run sits between /a's chain and /b's, and
                //   a build pairing *the next id* would time /a's death against /b's birth.
                run(3, Some("/b"), true, Some(200), Some(300)),
                // /a's third: pairs with run 2 (4000 → 4010), a ten-second handover. ⚠ It is also
                //   /a's NEWEST, and it `converged` — an ending whose next run is owed, so *yet*
                //   is the honest word over it.
                ended(4, "/a", Some(4010), None, "converged"),
                // /b's second, which began BEFORE run 3 stopped — two runs on one tree at once.
                run(5, Some("/b"), true, Some(250), Some(400)),
                // /b's third: run 5 stopped at 400 and nobody watched this one begin. ⛔ AND IT IS
                //   `cancelled` — the product's own answer is *nothing opens a next run off this
                //   ending*, so /b's chain STOPS here and a reader told to come back waits for
                //   ever. 36 of the live store's 212 finished runs are this ending.
                ended(6, "/b", None, Some(500), "cancelled"),
                // /c: a live run with a successor beside it — the only way to be StillRunning.
                run(7, Some("/c"), false, Some(10), None),
                ended(8, "/c", Some(20), Some(30), "exhausted"),
                // /d: finished, nobody watched it stop, and something followed it.
                run(9, Some("/d"), true, Some(1), None),
                // ⚠ /d's newest carries NO outcome — half of `SuccessionUnsaid`, and the shape
                //   every row written before that column existed has.
                run(10, Some("/d"), true, Some(9), Some(11)),
                // ⚠ AND THE OTHER HALF: a finished run whose ending is a word THIS build cannot
                //   classify. `of_outcome_word` answers `None` for both, and rule 6 says an
                //   unclassified row is a RED rather than either neighbour's pass.
                // ⚠ AND THE ROW TODAY'S STORE IS ENTIRELY MADE OF: no tree at all.
                run(11, None, true, Some(1), Some(2)),
                // /e's second — the successor run 0 needs, so that the abandoned row is asked the
                // first axis's question at all rather than dropping out at `NothingFollowed`.
                // ⚠⚠ AND IT `failed`, which is the arm that keeps this split from being *a machine
                //    may act*: no machine may proceed past a failure, and a PERSON is still owed
                //    the next run — so *yet* is honest here and the two columns cross.
                ended(12, "/e", Some(9000), Some(9100), "failed"),
                // ⚠ AND THE OTHER HALF OF `SuccessionUnsaid`: a finished run whose ending is a word
                //   THIS build cannot classify. `of_outcome_word` answers `None` for that and for
                //   an absent one alike, and rule 6 says an unclassified row is a RED rather than
                //   either neighbour's pass.
                ended(13, "/f", Some(50), Some(60), "an_ending_a_later_build_authored"),
                // ⛔⛔⛔⛔⛔ AND THE SHAPE THE LIVE STORE IS ENTIRELY MADE OF — the newest run of
                //   its tree, STILL RUNNING. Measured 2026-09-05T14:33:41Z, all three of the
                //   loop's tree-bearing rows are exactly this, so it is the one row item 872 ⑶b
                //   can get its first number from. An unfinished run has no ending to classify and
                //   *yet* is honest over it without asking; a build that asked anyway would file
                //   every live loop in the store under *nothing says whether anything is owed*.
                run(14, Some("/g"), false, Some(70), None),
                // ⛔⛔⛔⛔⛔ AND THE ROW THE LIVE STORE HANDED THIS ITEM ON 2026-09-05T15:33:48Z:
                //   the SAME `converged` as /a's newest, and no `opened_by_session`. Its ending
                //   owes the next run to *whoever opened it* and the log names nobody, so nobody
                //   is owed one — `Opener::ThisRunsOpener`'s own doc. Run 233 was exactly this and
                //   the page called it *nothing has followed it on that tree yet*.
                ended_unopened(15, "/h", Some(80), Some(90), "converged"),
                // ⚠⚠ AND THE CONTROL THAT KEEPS THAT ARM FROM SPREADING: the same missing column
                //   over an ending whose opener is A PERSON. A person is owed the next run however
                //   little the log says about who asked for this one — `a_person opens the next
                //   run, and until one has, nothing else is owed` — so this row keeps *yet*. A
                //   build demanding a record here would declare nobody owed on every failure.
                ended_unopened(16, "/i", Some(100), Some(110), "failed"),
            ]
        }))
        .expect("the log a predecessor leaves is what this reads");

        let waits = log.waits_between_runs();
        let of = |why: NoWait| {
            waits
                .unmeasured
                .iter()
                .find(|(arm, _)| *arm == why)
                .map(|(_, count)| *count)
                .unwrap_or_else(|| panic!("{why:?} must be in the report's population"))
        };

        // ── ① THE HEADLINE: the stretch, its length, and which two runs bound it ──
        assert_eq!(
            waits.measured,
            vec![
                Wait {
                    tree: "/a".to_owned(),
                    after: 1,
                    before: 2,
                    seconds: 3529
                },
                Wait {
                    tree: "/a".to_owned(),
                    after: 2,
                    before: 4,
                    seconds: 10
                },
            ],
            "⛔⛔⛔⛔⛔ REGISTER ITEM 872 ⑶: the delay between a run stopping and the next one on \
             the SAME tree starting is what item 827 measured by hand at 3 h 49 m and what four \
             rounds recorded as unmeasurable. It is `ran_from` of the second minus `ran_to` of the \
             first, both of them moments a daemon WATCHED — item 888 built them for this clause \
             and nothing read them until now",
        );
        assert_eq!(
            waits.longest().map(|wait| wait.seconds),
            Some(3529),
            "⚠⚠ and the LONGEST is the number this item is compared against — a report that could \
             not name it would leave item 827's 3 h 49 m with nothing to be measured against",
        );

        // ── ② THE INTERLEAVING CONTROL, which is what stops a number that means nothing ──
        assert!(
            waits.measured.iter().all(|wait| wait.tree == "/a"),
            "⛔⛔⛔⛔⛔ A STRETCH WAS MEASURED ACROSS TWO WORKING TREES. One daemon drives three \
             repositories and their ids interleave — 224, 225 and 227 were watching-zenoh, pinion \
             and sprag on 2026-09-05 — so pairing by id times one repository's death against \
             another's birth. That is worse than the silence it replaces, because it looks like an \
             answer. Got {:?}",
            waits.measured,
        );

        // ── ③ EVERY REASON REACHED, so none of the seven is decoration ──
        for (why, expected) in [
            (NoWait::TreeUnknown, 1),
            // ⚠ FIVE, and each has EARNED the word *yet*: /a's newest `converged`, /c's
            // `exhausted` and /e's `failed` — a next run is owed off all three, to this run's own
            // opener for the first two and to a PERSON for the third — plus /g's, which has not
            // ended at all, and /i's, which is `failed` with NO opener on record and is owed one
            // anyway, because the party that word names is not read off the run.
            (NoWait::NothingFollowed, 5),
            // ⛔ ONE: /b's newest was `cancelled`, and nothing opens a next run off that ending.
            (NoWait::SuccessionEnded, 1),
            // ⛔ ONE: /h's newest `converged` exactly as /a's did, and carries no opener. The two
            // differ by that column ALONE, which is why this arm cannot be read off the ending.
            (NoWait::OpenerUnrecorded, 1),
            // ⚠ TWO: /d's newest recorded no ending at all, and /f's is a word this build cannot
            // classify. `Disposition::of_outcome_word` answers `None` to both and its own doc
            // calls that a RED — folded upward it promises a successor nobody owes.
            (NoWait::SuccessionUnsaid, 2),
            (NoWait::StillRunning, 1),
            (NoWait::EndAbandoned, 1),
            (NoWait::EndUnwatched, 1),
            (NoWait::SuccessorStartUnwatched, 1),
            (NoWait::SuccessorStartedFirst, 1),
        ] {
            assert_eq!(
                of(why),
                expected,
                "⛔⛔⛔ {why:?} — {}. An arm nothing can reach is an arm that will be wrong \
                 without anybody finding out, and this workspace's rule 6 is that an unclassified \
                 run is a RED and not a pass. Report: {:?}",
                why.describe(),
                waits.unmeasured,
            );
        }

        // ── ③b THE SECOND AXIS, asked of every row before any grouping is ──
        //
        // ⛔⛔⛔⛔⛔ REGISTER ITEM 872 ⑶b. The arms above are tried in order and `TreeUnknown` is
        // first, so a row with no tree is told in that one word — and at 2026-09-05T12:11:19Z the
        // live store was 231 such rows, every one of them ALSO without a watched stop. Two walls,
        // one of them reported. This axis is the other one, and it is answerable with no tree, no
        // successor and no promotion.
        let end = |arm: LeftEnd| {
            waits
                .left_ends
                .iter()
                .find(|(seen, _)| *seen == arm)
                .map(|(_, count)| *count)
                .unwrap_or_else(|| panic!("{arm:?} must be in the report's second population"))
        };
        for (arm, expected) in [
            // 1, 2, 3, 5, 6, 8, 10, 11, 12, 13, 15, 16 carry `ran_to` and are finished. Runs 4 and
            // 9 do not. Runs 0 and 7 never finished — and they are DIFFERENT arms, which is the
            // split the live store forced: 7 may still end, 0 cannot, and only the log says so.
            (LeftEnd::Watched, 12),
            // ⚠ TWO: run 7, and run 14 — the live store's own shape, newest of its tree and still
            // going.
            (LeftEnd::NotEndedYet, 2),
            (LeftEnd::Abandoned, 1),
            (LeftEnd::Unwatched, 2),
        ] {
            assert_eq!(
                end(arm),
                expected,
                "⛔⛔⛔ {arm:?} — {}. Every arm is reached here, because an arm nothing exercises \
                 is one that will be wrong without anybody finding out. Second axis: {:?}",
                arm.describe(),
                waits.left_ends,
            );
        }
        assert_eq!(
            waits.left_ends_counted(),
            log.runs.len(),
            "⛔⛔⛔⛔⛔ THE SECOND AXIS COUNTED A DIFFERENT POPULATION FROM THE FIRST. It is a \
             second partition of the SAME rows, not a subdivision of the unmeasured half, and a \
             fraction whose denominator came from somewhere else reads exactly as reasonable as \
             one that did not. Counted {} against {} rows: {:?}",
            waits.left_ends_counted(),
            log.runs.len(),
            waits.left_ends,
        );
        // ⚠⚠ AND THE CORRESPONDENCE BETWEEN THE AXES CARRIES NO ASSERTION HERE, deliberately.
        //
        // Item 872 ⑴ nailed its own pair together with one (`opens_next() == ThisRunsOpener` ⇔
        // `a_machine_may_act()`). The same nail was written here and then measured: mutating
        // `waits_between_runs` to build a stretch off an unwatched stop reddened assertion ① —
        // which pins `measured` exactly — and the nail never ran. **An assertion another one
        // always reaches first is decoration**, and this workspace's own rule is that an arm
        // nothing can reach will be wrong without anybody finding out.
        //
        // ⇒ So the correspondence moved out of the gate and into the CODE: `waits_between_runs`
        // asks `LeftEnd::of` for the left end rather than re-reading `finished` and `ran_to` in its
        // own spelling, so a stretch off a run this axis calls unwatched is not a thing the build
        // can express. `StillRunning` ⇔ `NotEndedYet` and `EndUnwatched` ⇔ `Unwatched` are that one
        // reading, seen from the two sides.

        // ── ④ THE SUM, which is what makes a silent drop impossible ──
        assert_eq!(
            waits.runs(),
            log.runs.len(),
            "⛔⛔⛔⛔⛔ RUNS WENT MISSING BETWEEN THE TWO HALVES. A run that is neither measured \
             nor blamed has been dropped, and a dropped run is invisible in exactly the direction \
             that reads as *no delay* — the report's whole subject. Measured {} + blamed {:?} \
             against {} rows",
            waits.measured.len(),
            waits.unmeasured,
            log.runs.len(),
        );

        // ── ⑤ AND THE SHAPE OF TODAY'S REAL STORE: a pre-890 log says so rather than saying nothing ──
        //
        // ⚠⚠ 2026-09-05T07:53:28Z, `sprag waits` over the loop's own store: 229 runs, 229 under
        // `TreeUnknown`, 0 measured. Without this arm a build that answered *no stretches* for such
        // a log would be indistinguishable from one that had measured them all at zero.
        let pre_890: RunLog = serde_json::from_value(serde_json::json!({
            "version": RUN_LOG_VERSION,
            "runs": [run(1, None, true, None, None), run(2, None, true, None, None)],
        }))
        .expect("a log from before item 890's column");
        let old = pre_890.waits_between_runs();
        assert!(
            old.measured.is_empty()
                && old
                    .unmeasured
                    .iter()
                    .find(|(why, _)| *why == NoWait::TreeUnknown)
                    .is_some_and(|(_, count)| *count == 2),
            "⛔⛔⛔⛔⛔ A LOG THAT PREDATES THE COLUMNS MUST SAY SO. This is the whole of the live \
             store today — 229 rows, none of them able to name a tree — and a report that returned \
             an empty answer for it would read as *no tree ever waited*, which is the strongest \
             possible claim made from no evidence at all. Got {old:?}",
        );
        // ⛔⛔⛔⛔⛔ ⑤b AND HOW DEEP THAT WALL IS, which is the whole of register item 872 ⑶b.
        //
        // Both rows above are `TreeUnknown`, and the first axis has nothing further to say about
        // them. The second axis does: neither carries `ran_to`, so **neither could be a left end
        // even if item 890's column were filled in for both**. Measured over the real store at
        // 2026-09-05T12:11:19Z, that is the shape of all 231 rows — `ran_to` non-null 0 — and it is
        // what turns *the tree column is the wall* into *only new runs lift this*.
        assert_eq!(
            (old.watched_left_ends(), old.left_ends_counted()),
            (0, 2),
            "⛔⛔⛔⛔⛔ A PRE-COLUMN LOG MUST SAY THAT BACKFILLING THE GROUPING WOULD BUY NOTHING. \
             Zero watched stops means no stretch can start in this log whatever any tree column \
             later says, and a report that named only `TreeUnknown` invites exactly the opposite \
             reading. Second axis: {:?}",
            old.left_ends,
        );
    }

    /// 🎯🎯🎯🎯🎯 **HOW FULL A SESSION WAS WHEN IT FOLDED IS READ, AND WHAT CANNOT BE IS NAMED** —
    /// register item 856 ⑴, and the arithmetic five re-judgements of that item each did by hand.
    ///
    /// # ⛔⛔⛔⛔⛔ The two landings that must never be one number
    ///
    /// Item 856's stated refutation is *a `capacity` reflection whose prompt LANDS*. Measured
    /// 2026-09-05 that had happened 29 times, and all 29 belonged to runs whose ceiling a caller
    /// had moved to `20000` — where a `capacity` reflection is *we handed over early*, not *the
    /// session filled up*. The condition had silently assumed **ceiling = fullness**. So the
    /// headline of this gate is that the two counts stay apart, and the control is a run whose
    /// caller overrode something ELSE: the answer keys on the ceiling's own word and not on *an
    /// override happened*.
    ///
    /// # ⚠⚠ AND THE FIXTURE IS THE JSON A PREDECESSOR LEAVES, decoded by the product's reader
    ///
    /// Item 856's own instruments died at a crossing twice — 894 measured the ceiling reaching a
    /// live row and never a stored one, and `folds_by_reason` is `#[serde(flatten)]`, the one
    /// attribute that can swallow a table whole. A hand-built struct would assert this file's own
    /// typing, so every column here arrives through `serde` as a real key or a real absence.
    #[test]
    fn how_full_a_session_was_when_it_folded_is_read_and_what_cannot_be_is_named() {
        let log: RunLog = serde_json::from_value(serde_json::json!({
            "version": RUN_LOG_VERSION,
            "runs": [
                // ── ① THE HEADLINE: an ordinary run, judged by its own document's ceiling ──
                //    4 capacity prompts, 1 folded ⇒ THREE landings, and each is a refutation.
                { "id": 1, "label": "ai_loop pane=3", "iterations": 9, "finished": true,
                  "context_high_water": 800_000, "context_ceiling": 800_000, "overridden": [],
                  "folds_by_reason": { "capacity": { "delivered": 4, "folded": 1 },
                                       "ordinary": { "delivered": 40, "folded": 0 } } },
                // ── ② AN ARM: the shape of runs 214 and 215, which produced 27 of the 29 ──
                { "id": 2, "label": "ai_loop pane=4", "iterations": 9, "finished": true,
                  "context_high_water": 24_000, "context_ceiling": 20_000,
                  "overridden": ["context_ceiling"],
                  "folds_by_reason": { "capacity": { "delivered": 28, "folded": 1 } } },
                // ── ③ THE CONTROL: a caller who moved something ELSE is NOT an experiment here ──
                { "id": 3, "label": "ai_loop pane=5", "iterations": 9, "finished": true,
                  "context_high_water": 800_000, "context_ceiling": 800_000,
                  "overridden": ["max_seconds"],
                  "folds_by_reason": { "capacity": { "delivered": 2, "folded": 2 } } },
                // ── ④ THE PROMOTION WALL, WITH THE ROAD WALKED AND NOTHING LEFT STANDING ──
                //    It reflected on capacity three times and the composer took all three, so the
                //    wall costs this row nothing: 0 landings behind it.
                { "id": 4, "label": "ai_loop pane=6", "iterations": 9, "finished": true,
                  "context_ceiling": 800_000, "overridden": [],
                  "folds_by_reason": { "capacity": { "delivered": 3, "folded": 3 } } },
                // ── ④b THE SAME WALL, AND FOUR LANDINGS BEHIND IT — the shape of runs 214/215 ──
                //    This is what the live store held on 2026-09-05T13:41:36Z and what every
                //    reading of this verb had been silent about: evidence present, unattributable.
                { "id": 11, "label": "ai_loop pane=13", "iterations": 9, "finished": true,
                  "context_ceiling": 800_000, "overridden": [],
                  "folds_by_reason": { "capacity": { "delivered": 5, "folded": 1 } } },
                // ── ④c AND THE CONTROL FOR BOTH: behind the wall having never walked that road ──
                //    Nothing was lost here, and an arm that pooled it with ④b would say so of ④b.
                { "id": 12, "label": "ai_loop pane=14", "iterations": 9, "finished": true,
                  "context_ceiling": 800_000, "overridden": [],
                  "folds_by_reason": { "ordinary": { "delivered": 4, "folded": 0 } } },
                // ── ④d AND THE ROAD WALKED BY THE UNASKED HALF ALONE, behind the same wall ──
                //    `delivered` is 0 and the document still transitioned on
                //    `context >= context_ceiling`. A classifier written as `delivered > 0` files
                //    this beside ④c as *no evidence*, which is item 856 ⑶'s own population.
                { "id": 13, "label": "ai_loop pane=15", "iterations": 9, "finished": true,
                  "context_ceiling": 800_000, "overridden": [],
                  "folds_by_reason": { "capacity": { "delivered": 0, "folded": 0,
                                                     "unasked_after_a_fold": 1,
                                                     "unasked_on_the_pane": 0 } } },
                // ── ⑤ A READING WITH NOTHING TO MEASURE IT AGAINST ──
                { "id": 5, "label": "ai_loop pane=7", "iterations": 9, "finished": true,
                  "context_high_water": 700_000, "overridden": [],
                  "folds_by_reason": { "capacity": { "delivered": 3, "folded": 3 } } },
                // ── ⑥ BOTH READINGS AND NOBODY ANSWERED WHOSE NUMBERS THEY WERE ──
                { "id": 6, "label": "ai_loop pane=8", "iterations": 9, "finished": true,
                  "context_high_water": 800_000, "context_ceiling": 800_000,
                  "folds_by_reason": { "capacity": { "delivered": 3, "folded": 0 } } },
                // ── ⑦ AND A WORD THIS BUILD CANNOT SPELL, which refuses the whole answer ──
                { "id": 7, "label": "ai_loop pane=9", "iterations": 9, "finished": true,
                  "context_high_water": 800_000, "context_ceiling": 800_000,
                  "overridden": ["a_bound_a_later_build_authored"],
                  "folds_by_reason": { "capacity": { "delivered": 3, "folded": 0 } } },
                // ── ⑧ A SPLIT PRESENT AND ALL ZERO — 214 of today's 229 rows ──
                { "id": 8, "label": "ai_loop pane=10", "iterations": 9, "finished": true,
                  "context_high_water": 800_000, "context_ceiling": 800_000, "overridden": [],
                  "folds_by_reason": { "capacity": { "delivered": 0, "folded": 0 } } },
                // ── ⑨ NO SPLIT AT ALL — a row from a daemon older than the table ──
                { "id": 9, "label": "ai_loop pane=11", "iterations": 9, "finished": true,
                  "context_high_water": 800_000, "context_ceiling": 800_000, "overridden": [] },
                // ── ⑨b A CEILING THAT IS RECORDED AND IS NOT IN FORCE ──
                //    ⛔ It ANSWERS item 859 (`overridden: []`), so nothing else would keep it out
                //    of the axis — and the document reads `context_ceiling <= 0` as unbounded, so
                //    this run could never have reflected on capacity at all. Run 233 carried
                //    exactly this zero on 2026-09-05T15:33:48Z beside a peak of 417,509.
                { "id": 14, "label": "ai_loop pane=16", "iterations": 9, "finished": true,
                  "context_high_water": 417_509, "context_ceiling": 0, "overridden": [],
                  "folds_by_reason": { "ordinary": { "delivered": 12, "folded": 6 } } },
                // ── ⑩ THE CONTROL GROUP THE AXIS IS READ AGAINST: reflected, never on capacity ──
                //    Its peak is BELOW its ceiling, which is what an unfilled session looks like.
                { "id": 10, "label": "ai_loop pane=12", "iterations": 9, "finished": true,
                  "context_high_water": 40_000, "context_ceiling": 800_000, "overridden": [],
                  "folds_by_reason": { "budget": { "delivered": 8, "folded": 8 } } },
            ]
        }))
        .expect("the log a predecessor leaves is what this reads");

        let folds = log.folds_against_fullness();
        let of = |why: NoFullness| {
            folds
                .unmeasured
                .iter()
                .find(|(arm, _)| *arm == why)
                .map(|(_, count)| *count)
                .unwrap_or_else(|| panic!("{why:?} must be in the report's population"))
        };

        // ── ① THE HEADLINE, AND THE WHOLE POINT: the two landing counts are separate numbers ──
        assert_eq!(
            (folds.refutations(), folds.landings_at_a_moved_ceiling()),
            (3, 27),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 856 ⑴: a `capacity` prompt that LANDED is this axis's own \
             stated refutation, and one that landed under a ceiling A CALLER MOVED is not — at a \
             moved ceiling that reflection means *we handed over early*. Measured 2026-09-05 the \
             live store held 29 of the second kind and 0 of the first, and a build that returned \
             their sum would refute the axis with the experiment's own definition of *full*. \
             Rows: {:?}",
            folds.measured,
        );
        assert_ne!(
            folds.refutations(),
            folds.refutations() + folds.landings_at_a_moved_ceiling(),
            "⚠⚠⚠ THE CONTROL FOR THE ARM ABOVE: a fixture whose experiment produced no landings \
             would satisfy it with the two numbers pooled",
        );

        // ── ② AND THE ANSWER KEYS ON THE CEILING'S OWN WORD, not on *an override happened* ──
        let judged = |id: u64| {
            folds
                .measured
                .iter()
                .find(|row| row.id == id)
                .unwrap_or_else(|| panic!("run {id} must be readable: {:?}", folds.measured))
                .judged
        };
        assert_eq!(
            (judged(2), judged(3)),
            (Judged::ByACallerWhoMovedIt, Judged::ByItsDocument),
            "⛔⛔⛔⛔ REGISTER ITEM 859: run 3's caller took `max_seconds` and left the ceiling \
             alone, so its `capacity` reflections still mean *the session filled up*. A build that \
             read *any override* as *an experiment* would throw a healthy run out of the axis's \
             denominator, and one that read it as neither would put run 2 into it.",
        );

        // ── ②a THE RATE, ROAD BY ROAD, OVER PRODUCTION RUNS ONLY — register item 894 ⑶ ──
        //
        // ⛔⛔⛔⛔⛔ Run 2's caller moved the ceiling to 20,000 and it folded 1 of 28 capacity
        // prompts. Summed in, the capacity denominator reads 34 and the axis is answered with the
        // experiment's own definition of *full* — the same pooling `refutations` refuses one
        // method over, arriving through the RATE instead of through the landings.
        //
        // ⚠⚠ AND THE EMPTY ROADS ARE IN THE ANSWER. They are the control group: item 856's whole
        // design note is that counting `capacity` alone cannot tell the axis from *a reflection
        // prompt is the longest thing this loop builds*, and a road nobody walked is what makes
        // the comparison readable rather than a single figure.
        assert_eq!(
            (
                folds.production_runs(),
                folds
                    .folded_by_road()
                    .into_iter()
                    .filter(|(_, _, delivered)| *delivered > 0)
                    .map(|(occasion, folded, delivered)| (occasion.word(), folded, delivered))
                    .collect::<Vec<_>>(),
                folds.folded_by_road().len(),
            ),
            (
                3,
                vec![("budget", 8, 8), ("capacity", 3, 6), ("ordinary", 0, 40)],
                sprag_plugin::Occasion::ALL.len(),
            ),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 894 ⑶: the ratio item 856 is read from is a COMPARISON over \
             production runs — `capacity` beside the roads that are its control — and it must sum \
             only the rows judged by their OWN document's ceiling. Run 2 is an experiment: its \
             caller moved the ceiling to 20,000, so its 28 capacity prompts answer *we handed \
             over early* and belong to no rate about fullness. And every road stays in the answer, \
             zeros included, because the empty ones are what make the walked ones mean anything. \
             Rows: {:?}",
            folds.measured,
        );

        // ── ②b THE WALL HAS A SIZE, AND IT IS NOT A THIRD REFUTATION ──
        assert_eq!(
            (
                folds.stranded,
                folds.unjudgeable_runs(),
                folds.refutations()
            ),
            (4, 3, 3),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 856 ⑴⒞ / 894: runs 4, 11 and 13 are behind the promotion \
             wall HAVING WALKED the capacity road — 11 left four landings there and 13 walked it \
             with the UNASKED half alone. A build that pooled them with run 12, which never took \
             that road, tells a reader the store holds no evidence, which is what every reading \
             of this report said while the live store held 29 such landings \
             (2026-09-05T13:41:36Z). And a build that ADDED them to the refutations would refute \
             the axis with rows that cannot say whose ceiling they reflected on. Rows: {:?}",
            folds.unmeasured,
        );

        // ── ③ EVERY REASON REACHED, so none of the six is decoration ──
        for (why, expected) in [
            (NoFullness::SplitUnsaid, 1),
            (NoFullness::SplitZeroed, 1),
            // ⚠ ONE: run 12, which is behind the wall and never walked the capacity road.
            (NoFullness::FullnessUnread, 1),
            // ⚠ THREE: runs 4, 11 and 13, behind the same wall having walked it — 13 by the
            // unasked half alone, which a `delivered > 0` classifier would file under the arm
            // above and report as *no evidence*.
            (NoFullness::CapacityUnjudgeable, 3),
            (NoFullness::CeilingUnrecorded, 1),
            // ⛔ ONE: run 14's ceiling is recorded, answers item 859, and is ZERO — no bound, so
            // no capacity reflection was ever possible on it. Its 6 folds of 12 must not quiet
            // the rate the axis is read from.
            (NoFullness::CeilingUnbounded, 1),
            // ⚠ TWO: nobody answered, and a word this build cannot spell. `Overridden::restored`
            // folds them and states why — both are *this row cannot be told from an experiment*.
            (NoFullness::ExperimentUnsaid, 2),
        ] {
            assert_eq!(
                of(why),
                expected,
                "⛔⛔⛔ {why:?} — {}. An arm nothing can reach is an arm that will be wrong \
                 without anybody finding out, and this workspace's rule 6 is that an unclassified \
                 run is a RED and not a pass. Report: {:?}",
                why.describe(),
                folds.unmeasured,
            );
        }

        // ── ④ THE SUM, which is what makes a silent drop impossible ──
        assert_eq!(
            folds.runs(),
            log.runs.len(),
            "⛔⛔⛔⛔⛔ RUNS WENT MISSING BETWEEN THE TWO HALVES. A run that is neither read nor \
             blamed has been dropped, and a dropped run is invisible in exactly the direction that \
             reads as *nothing folded* — this report's whole subject. Read {} + blamed {:?} \
             against {} rows",
            folds.measured.len(),
            folds.unmeasured,
            log.runs.len(),
        );

        // ── ⑤ THE CONTROL GROUP IS IN THE POPULATION, and it is what the axis is read against ──
        //
        // ⚠⚠ Item 856's own design note: counting `capacity` alone is what the split was built to
        // stop, because a reflection prompt is the longest thing this loop composes and *long
        // prompts fold* is a live rival explanation. Run 10 reflected on `budget` at 40,000 read
        // of an 800,000 ceiling and folded all eight — a row the axis has to be able to see.
        let control = folds
            .measured
            .iter()
            .find(|row| row.id == 10)
            .expect("a run that reflected on another road is READ, not blamed");
        assert!(
            !control.reached_its_ceiling()
                && control.landed_on_the_capacity_road() == 0
                && control.folds.under(sprag_plugin::Occasion::Reflecting(
                    sprag_plugin::ReflectReason::Budget
                )) == sprag_plugin::FoldsUnder {
                    delivered: 8,
                    folded: 8,
                    unasked: sprag_plugin::Unasked::default(),
                },
            "⛔⛔⛔⛔ THE CONTROL GROUP MUST SURVIVE THE READING. A build that kept only the \
             `capacity` row would publish exactly the capacity-only count item 856 filed the split \
             to replace, and this row — an unfilled session that folded everything on another road \
             — is the one that makes *full sessions fold* falsifiable. Got {control:?}",
        );
        assert!(
            folds
                .measured
                .iter()
                .find(|row| row.id == 1)
                .is_some_and(FoldAtFullness::reached_its_ceiling),
            "⚠⚠ AND THE OTHER SIDE OF THAT COMPARISON: a session that DID reach its ceiling must \
             read so, or `reached_its_ceiling` is a constant and the clause beside run 10 is empty",
        );
    }

    /// 🎯🎯🎯🎯🎯 **THE NUMBER IS BUILT OUT OF STAMPS THE PRODUCT WROTE, NOT OUT OF A FIXTURE'S
    /// ARITHMETIC** — register item 872 ⑶, the crossing half.
    ///
    /// # ⛔⛔⛔⛔⛔ What the gate beside this one cannot claim, and why that matters here
    ///
    /// `how_long_a_tree_had_nothing_driving_it_is_measured_and_what_cannot_be_is_named` hands the
    /// reader a log with `ran_from` and `ran_to` **typed into it by this file**. Every arm of it
    /// stays green on a build whose stamper never writes those fields, writes them under other
    /// names, or writes them at moments that do not bracket anything — and the whole question of
    /// item 872 ⑶ is whether a REAL store will ever yield the number. Item 856 ⑸ measured this
    /// exact shape: seven surfaces gated, the call that fills them replaced by a discard, workspace
    /// green.
    ///
    /// ⇒ So this drives [`crate::durability::stamp_run_times`] — the product's own stamper, with
    /// its clock injected — across the tick sequence a daemon actually performs, and reads the
    /// answer off what that left behind. Nothing here spells a `ran_from`.
    ///
    /// # ⚠⚠⚠ And it answers the question the promotion wall leaves open
    ///
    /// The live store cannot produce a stretch today: measured 2026-09-05T07:53:28Z, 229 rows and
    /// 229 of them unmeasurable, because the running daemon predates all three columns. That is a
    /// fact about the DAEMON, and it leaves *will this work when the daemon is new* unanswered —
    /// four re-judgements have already recorded this clause as blocked without asking it. This is
    /// the answer, and it needs no promotion to give.
    #[test]
    fn the_stretch_is_computed_from_stamps_the_daemons_own_saver_wrote() {
        /// A run as a daemon holds it — ⛔ **NO `ran_from`, NO `ran_to`, NO `ended_at`**. Those are
        /// the stamper's OUTPUT, and a fixture that supplied them would be this gate measuring its
        /// own arithmetic.
        fn held(id: u64, finished: bool) -> serde_json::Value {
            serde_json::json!({
                "id": id, "label": format!("ai_loop pane={id}"), "iterations": 7,
                "finished": finished, "place": ["working"], "tree": "/repo",
            })
        }
        fn log_of(runs: Vec<serde_json::Value>) -> RunLog {
            serde_json::from_value(serde_json::json!({
                "version": RUN_LOG_VERSION, "runs": runs,
            }))
            .expect("a log the daemon's own saver would serialize")
        }
        // Every tick goes through the product's stamper against the tick before it, which is
        // exactly what `save_runs_if_changed` does with the `last` it carries.
        let tick = |mut log: RunLog, previous: Option<&RunLog>, now: u64| -> RunLog {
            crate::durability::stamp_run_times(&mut log, previous, Some(now));
            log
        };

        // ── THE SEQUENCE, as a daemon performs it ──
        // ⚠ `previous: None` on the first tick is a daemon with NO PREDECESSOR, which is the one
        // case `ran_from` is written in — an inherited run is in the predecessor's log by
        // construction and correctly gets nothing.
        let first = tick(log_of(vec![held(1, false)]), None, 1000);
        let stopped = tick(log_of(vec![held(1, true)]), Some(&first), 1100);

        // ── THE CONTROL, FIRST: no successor yet, so there is no stretch to report ──
        //
        // ⚠⚠ Without this the gate below passes on a build that reports a stretch the moment a run
        // ends — against nothing, or against whatever came next in the file. *A run stopped* is not
        // *a tree waited*; the second needs the run that ended the waiting.
        let waiting = stopped.waits_between_runs();
        assert!(
            waiting.measured.is_empty(),
            "⛔⛔⛔ A STRETCH WAS REPORTED WITH NOTHING TO CLOSE IT. The tree is still waiting at \
             this tick — the number is not knowable until something follows — and a report that \
             answered here would be measuring against a run that does not exist. Got {:?}",
            waiting.measured,
        );

        let opened = tick(
            log_of(vec![held(1, true), held(2, false)]),
            Some(&stopped),
            4629,
        );
        let done = tick(
            log_of(vec![held(1, true), held(2, true)]),
            Some(&opened),
            4700,
        );

        // ── THE PREMISE, ASSERTED: the stamper really did write both ends ──
        //
        // ⚠⚠⚠ Without this the headline below could pass by measuring nothing and finding nothing
        // — it is `assert!(empty)`'s dual, and the failure it guards is a stamper that has stopped
        // writing while every arm about *what cannot be measured* goes on being satisfied.
        assert_eq!(
            (done.runs[0].ran_to, done.runs[1].ran_from),
            (Some(1100), Some(4629)),
            "⛔⛔⛔⛔⛔ THE STAMPER DID NOT WRITE THE INTERVAL. Item 888 built `ran_to` and \
             `ran_from` expressly for item 872 ⑶ — `ran_from`'s own doc says so — and if they stop \
             arriving then every gate about this clause measures a fixture's arithmetic while the \
             real store answers nothing for ever. Got {:?}",
            done.runs,
        );

        // ── ① THE HEADLINE: the number, built from stamps this file never typed ──
        let waits = done.waits_between_runs();
        assert_eq!(
            waits.measured,
            vec![Wait {
                tree: "/repo".to_owned(),
                after: 1,
                before: 2,
                seconds: 3529
            }],
            "⛔⛔⛔⛔⛔ REGISTER ITEM 872 ⑶: the delay item 827 measured at 3 h 49 m by hand does \
             not come out of a log this build stamped. The live store cannot show this today — its \
             daemon predates the columns — so THIS is the only thing that can say the answer will \
             arrive at all once a promotion happens, which is what four re-judgements left \
             unasked. Report: {waits:?}",
        );
        assert_eq!(
            waits.longest().map(|wait| wait.seconds),
            Some(3529),
            "⚠⚠ and the longest stretch is the number item 827's 3 h 49 m is compared against",
        );

        // ── ② AND THE POPULATION STILL ADDS UP over a log nothing hand-stamped ──
        assert_eq!(
            waits.runs(),
            done.runs.len(),
            "⚠⚠⚠ a run went missing between the halves on the stamped road, which is the road \
             that matters: {waits:?}",
        );
    }
}
