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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use sprag_plugin::{Outcome, Progress, ProgressCell};

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
    },
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunOrder {
    /// Stop now and lose the turn in flight — `RunRegistry::cancel`, carrying WHO said so.
    ///
    /// ⚠⚠ The word rides on the ORDER rather than being a second call, because the two arrive at
    /// the same flag and a caller that had to set a reason separately could set one and not the
    /// other. See [`Canceller`].
    Cancel(Canceller),
    /// Finish what you are doing and then stop — `RunRegistry::stand_down`. One-way.
    StandDown,
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
    pub const fn standing(self) -> Option<sprag_plugin::StandingOrder> {
        match self {
            Self::Cancel(_) => None,
            Self::StandDown => Some(sprag_plugin::StandingOrder::StandDown),
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

    /// **WHO CANCELLED THIS RUN**, or [`None`] if nobody has — register item 596.
    ///
    /// ⚠⚠ [`stood_down`](Self::stood_down)'s argument verbatim: the directory forwards an order and
    /// does not know what became of it, so the handle is what remembers. And like that one it is a
    /// FACT ABOUT THE ORDER and never about the ending — a run whose flag was raised at the same
    /// instant it converged still converged, and `crate::plugins::cancel_sentence` is the only
    /// reader allowed to weigh the two together.
    fn cancelled_by(&self) -> Option<Canceller>;

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
}

/// A run driven by **A THREAD IN THIS PROCESS** — the only kind that exists today, and the one
/// register item 544 is about moving out.
///
/// The three flags are shared with the worker's `RunContext`, so an order delivered here is seen at
/// the driver's next loop top or wait poll. That sharing is why they are handed in rather than made
/// here: the same `Arc`s go to the worker at spawn.
pub struct ThreadRun {
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
    /// **WHICH STANDING ORDERS THIS RUN'S PLUGIN ANSWERED THAT IT READS** — register items 539
    /// and 597, captured at submit because the plugin itself moves into the worker thread and is
    /// unreachable from here afterwards.
    ///
    /// ⚠⚠⚠ A LIST THE PLUGIN PRODUCED, not one anybody here composed: the caller walks
    /// [`sprag_plugin::StandingOrder::ALL`] and keeps what
    /// [`sprag_plugin::Plugin::honours`] said yes to, so an order added to that set is asked about
    /// with nothing here to update.
    honoured: Vec<sprag_plugin::StandingOrder>,
    handle: Option<JoinHandle<()>>,
}

impl ThreadRun {
    /// Take the worker and the three flags it is already sharing.
    #[must_use]
    pub fn new(
        cancel: Arc<AtomicBool>,
        stand_down: Arc<AtomicBool>,
        hold: Arc<AtomicBool>,
        honoured: Vec<sprag_plugin::StandingOrder>,
        handle: JoinHandle<()>,
    ) -> Self {
        Self {
            cancel,
            stand_down,
            hold,
            cancelled_by: Mutex::new(None),
            honoured,
            handle: Some(handle),
        }
    }
}

impl RunHandle for ThreadRun {
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
            RunOrder::StandDown => self.stand_down.store(true, Ordering::Release),
            RunOrder::Hold(held) => self.hold.store(held, Ordering::Release),
        }
    }

    fn cancelled_by(&self) -> Option<Canceller> {
        *lock(&self.cancelled_by)
    }

    /// ⚠ THE PLUGIN'S OWN ANSWER, replayed. Nothing here decides it: the list was taken from
    /// `sprag_plugin::Plugin::honours` at submit, before the plugin moved into the worker thread.
    fn honours(&self, order: sprag_plugin::StandingOrder) -> bool {
        self.honoured.contains(&order)
    }

    fn stood_down(&self) -> bool {
        // ⚠ THE SAME FLAG THE WORKER'S `RunContext` IS SHARING, read rather than copied — see the
        // struct's own note on why the three `Arc`s are handed in. A second bool set beside the
        // `store` above could disagree with what the driver is reading.
        self.stand_down.load(Ordering::Acquire)
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
}

impl EndedRun {
    /// A run with no driver left, carrying what the log said became of it.
    ///
    /// ⚠ Named rather than a struct literal at the call site: `EndedRun { stood_down: false }`
    /// reads as a decision somebody took, and these are the values a restore may not guess at.
    #[must_use]
    pub const fn restored(stood_down: bool, cancelled_by: Option<Canceller>) -> Self {
        Self {
            stood_down,
            cancelled_by,
        }
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
    /// order would be refused regardless — see [`RunRegistry::standing_order`].
    plugin: Option<crate::plugins::PluginName>,
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
    /// WHICH BUILD DROVE IT, or [`None`] when nothing recorded one — see `RunRecord::build` for
    /// why those are different answers and why a reader must not fill the second one in.
    pub build: Option<String>,
    /// **WHETHER A PERSON ASKED THIS RUN TO STAND DOWN** — [`RunHandle::stood_down`], republished so
    /// a mouth can say what became of the ORDER and not only what became of the run.
    ///
    /// ⚠⚠ **IT IS THE ORDER AND NOT THE ENDING.** `true` beside a run that ended `cancelled` means
    /// the order was given and NOT honoured, which is register item 594's whole finding.
    /// [`crate::plugins::stand_down_sentence`] is the one reader allowed to weigh the two together.
    pub stood_down: bool,
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
    /// The pane whose occupant asked for it, or [`None`] for a run nobody claims.
    pub opened_by: Option<u64>,
    /// **WHICH CONVERSATION ASKED** — the asking pane's `agent_session`, resolved by the caller
    /// (which is the layer holding the workspace) rather than looked up here. See
    /// `RunRecord::opened_by_session`.
    pub opened_by_session: Option<String>,
    /// Where the worker writes its terminal state.
    pub state: Arc<Mutex<RunState>>,
    /// **THE RUN ITSELF** — a [`RunHandle`], and deliberately not a thread plus three flags. A
    /// caller spawning an in-process worker hands a [`ThreadRun`]; see that trait for why the
    /// registry is not allowed to know which kind it got (register item 544).
    pub run: Box<dyn RunHandle>,
    /// Where the driver writes what it has spent so far.
    pub progress: ProgressCell,
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
}

impl From<sprag_plugin::Deliveries> for PersistedDeliveries {
    fn from(live: sprag_plugin::Deliveries) -> Self {
        Self {
            made: live.made,
            folded: live.folded,
        }
    }
}

impl From<PersistedDeliveries> for sprag_plugin::Deliveries {
    fn from(stored: PersistedDeliveries) -> Self {
        Self {
            made: stored.made,
            folded: stored.folded,
        }
    }
}

impl PersistedRun {
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
}

/// The versioned file a daemon leaves behind for its successor.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RunLog {
    /// The format version — [`RUN_LOG_VERSION`] at write time, checked on load.
    pub version: u32,
    /// Every run the daemon held, in submit order.
    pub runs: Vec<PersistedRun>,
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
    /// An id reserved and never submitted is simply skipped, which costs nothing: ids are monotonic
    /// and never reused, so a gap in them means only that a run did not start.
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
            opened_by: run.opened_by,
            opened_by_session: run.opened_by_session,
            state: run.state,
            run: run.run,
            progress: run.progress,
            // ⚠ STAMPED HERE AND NOWHERE ELSE ON THIS PATH — see `RunRecord::build`. The worker
            // about to run is inside THIS image, so this image is the only honest answer, and it
            // is read from the constant the same binary published at `client/hello`.
            build: Some(crate::wire::BUILD.to_owned()),
        });
        id
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
    fn standing_order(&self, id: RunId, order: RunOrder) -> Result<(), Unordered> {
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
        record.run.deliver(order);
        Ok(())
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
    pub fn stand_down(&self, id: RunId) -> Result<(), Unordered> {
        self.standing_order(id, RunOrder::StandDown)
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
    pub fn hold(&self, id: RunId, held: bool) -> Result<(), Unordered> {
        self.standing_order(id, RunOrder::Hold(held))
    }

    /// Raise every run's cancel flag — used on host shutdown so in-flight runs abort promptly
    /// instead of being waited out and detached by [`join_all_within`](Self::join_all_within).
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
                .snapshot()
                .iter()
                .map(|run| {
                    let (finished, outcome, ceiling, output) = match &run.state {
                        RunState::Running | RunState::Interrupted => (false, None, None, None),
                        RunState::Done { outcome, output } => (
                            true,
                            Some(crate::plugins::outcome_word(outcome).to_owned()),
                            crate::plugins::outcome_ceiling(outcome).map(str::to_owned),
                            output.clone(),
                        ),
                        RunState::Panicked(why) => (true, Some(why.clone()), None, None),
                    };
                    PersistedRun {
                        id: run.id.0,
                        label: run.label.clone(),
                        iterations: run.progress.iterations,
                        cost: run.progress.cost.map(sprag_plugin::Cost::amount),
                        unit: run
                            .progress
                            .cost
                            .map(|c| sprag_plugin::Cost::unit(c).to_owned()),
                        finished,
                        outcome,
                        ceiling,
                        output,
                        build: run.build.clone(),
                        opened_by_session: run.opened_by_session.clone(),
                        // ⚠⚠⚠ WHERE IT WAS, AND WHOSE WORD THAT IS — register items 543 and 544,
                        // written as a PAIR because either alone misleads. The fingerprint is
                        // stamped from THIS image, which is the only honest answer: it is the
                        // build whose documents produced the word beside it. A run with no
                        // recorded position records no document either, so a reader never sees a
                        // fingerprint vouching for nothing.
                        at: run.progress.at.map(str::to_owned),
                        document: run
                            .progress
                            .at
                            .map(|_| sprag_plugin::STATECHARTS_FINGERPRINT.to_owned()),
                        // ⚠⚠⚠ ALWAYS `Some`, INCLUDING `false` — item 594. This image DID look, so
                        // `Some(false)` is a claim it is entitled to make; the `None` this field
                        // documents belongs to a log written before the field existed, and only a
                        // reader of such a log may see it. Writing `None` for *no order* would make
                        // this daemon's own silence indistinguishable from an older daemon's.
                        stood_down: Some(run.stood_down),
                        // ⚠ ALWAYS `Some`, INCLUDING THE ZERO PAIR — the field above's argument.
                        // This image looked, so `made: 0` is a claim it may make; the `None` this
                        // field documents belongs to a log written before it existed.
                        deliveries: Some(run.progress.deliveries.into()),
                        // ⚠ AND HERE `None` REALLY IS *no cancel*, unlike the field above — item
                        // 596. A stand-down is a bool and needs `Some(false)` to distinguish a
                        // silent daemon from an old log; a canceller is an option already, so the
                        // absent case carries its own meaning and needs no second one.
                        cancelled_by: run.cancelled_by,
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
                        ),
                        iterations: saved.iterations,
                        cost,
                        failure: None,
                        // ⚠ AND NEITHER IS `stopped`, for the same reason `failure` is dropped
                        // above: the log carries a run's SUMMARY, not its whole outcome. Both are
                        // diagnostics about a moment that is over — the daemon that could have
                        // acted on them is the one that died — and a restored pane's occupant is a
                        // plain shell, so there is no job left for either to describe.
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
                        banked: None,
                    }),
                    output: saved.output.clone(),
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
                plugin: None,
                // ⚠ THE SEAT IS DROPPED AND THE CONVERSATION IS KEPT — rule 1 above. A successor
                // cannot know who is sitting in pane 3; it can know which conversation asked, and
                // `crate::plugins` re-derives the seat from that at read time.
                opened_by: None,
                opened_by_session: saved.opened_by_session.clone(),
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
                run: Box::new(EndedRun::restored(
                    saved.stood_down.unwrap_or(false),
                    // ⚠⚠⚠ ITEM 596. Without this the ONE canceller a person ever meets after a
                    // restart would be unanswerable: `Shutdown` is raised by a daemon that then
                    // exits, so the only daemon left to be asked is this one.
                    saved.cancelled_by,
                )),
                progress: Arc::new(Mutex::new(Progress {
                    iterations: saved.iterations,
                    cost,
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
                    deliveries: saved
                        .deliveries
                        .map_or(sprag_plugin::Deliveries::NONE, Into::into),
                    // ⚠ NOR WHAT ITS CHECKS CAME TO — register item 601, on the same argument.
                    checks: sprag_plugin::Checks::NONE,
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
                })),
                // ⚠⚠⚠ AND THIS ONE IS TAKEN FROM THE LOG RATHER THAN STAMPED, which is the
                // opposite decision to every field above and the reason the field exists. The rest
                // of this record is about a run that is over, so inventing a value would assert
                // something nobody wrote; the BUILD was written down, by the image that actually
                // drove it. Stamping this daemon's here would date a dead daemon's work to its
                // successor — which is precisely the confusion register item 438 was filed for.
                build: saved.build.clone(),
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
                build: record.build.clone(),
                // ⚠ ASKED OF THE HANDLE, on the same pass that reads the state — item 594's
                // sentence weighs the two against each other, and reading them a moment apart is
                // this repository's *비교하는 두 값은 같은 순간에* rule at its cheapest.
                stood_down: record.run.stood_down(),
                // ⚠ SAME PASS, SAME REASON — item 596. The sentence a mouth prints weighs this
                // against `state`, so the two must not be read a moment apart either.
                cancelled_by: record.run.cancelled_by(),
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
        let deadline = Instant::now() + within;
        loop {
            self.sweep();
            // ⚠ ASKED, not collected: the answer is built once, on the way out, rather than
            // allocated on each of the thousand passes a full deadline takes.
            if !self.runs.iter().any(|record| record.run.outstanding()) {
                return Vec::new();
            }
            if Instant::now() >= deadline {
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
                return outstanding;
            }
            std::thread::sleep(Self::JOIN_POLL);
        }
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
                deliveries: sprag_plugin::Deliveries::NONE,
                checks: sprag_plugin::Checks::NONE,
                banked: None,
            };
            let read_back = crate::plugins::outcome_from_words(
                Some(crate::plugins::outcome_word(&outcome)),
                crate::plugins::outcome_ceiling(&outcome),
            );
            assert_eq!(
                read_back, state,
                "a {state:?} written to the run log must come back as itself",
            );
        }

        // ⚠ AND AN UNREADABLE PAIR IS `Failed`, never a happier guess: a record this build cannot
        // parse must not be reported as having converged.
        assert_eq!(
            crate::plugins::outcome_from_words(Some("a word from a newer build"), None),
            OutcomeState::Failed,
        );
        assert_eq!(
            crate::plugins::outcome_from_words(None, None),
            OutcomeState::Failed,
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
                    deliveries: sprag_plugin::Deliveries::NONE,
                    checks: sprag_plugin::Checks::NONE,
                    banked: None,
                }),
                output: None,
            };
        });
        let cancel = Arc::new(AtomicBool::new(false));
        let id = registry.reserve();
        assert_eq!(
            registry.submit(NewRun {
                id,
                label: "test".to_string(),
                plugin: crate::plugins::PluginName::Orchestrator,
                opened_by: Some(7),
                opened_by_session: None,
                state,
                run: Box::new(ThreadRun::new(
                    cancel,
                    Arc::new(AtomicBool::new(false)),
                    Arc::new(AtomicBool::new(false)),
                    // ⚠ BOTH: this fixture is about the directory holding a record, and a handle
                    // that refused every order would make the orders below untestable here.
                    sprag_plugin::StandingOrder::ALL.to_vec(),
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
            deliveries: sprag_plugin::Deliveries::NONE,
            checks: sprag_plugin::Checks::NONE,
            banked: None,
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
            opened_by: None,
            opened_by_session: None,
            state: Arc::new(Mutex::new(RunState::Running)),
            run: Box::new(ThreadRun::new(
                cancel,
                Arc::new(AtomicBool::new(false)),
                Arc::new(AtomicBool::new(false)),
                // ⚠ BOTH, for the fixture above's reason: these gates are about the directory, and
                // the refusal is driven where a real plugin answers it.
                sprag_plugin::StandingOrder::ALL.to_vec(),
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
        lock(&progress).deliveries = sprag_plugin::Deliveries {
            made: 14,
            folded: 3,
        };
        registry.submit(NewRun {
            id,
            label: "ai_loop pane=2".to_owned(),
            plugin: crate::plugins::PluginName::AiLoop,
            opened_by: None,
            opened_by_session: None,
            state: Arc::new(Mutex::new(RunState::Done {
                outcome: Box::new(an_outcome()),
                output: None,
            })),
            run: Box::new(EndedRun::restored(false, None)),
            progress,
        });

        // ⚠⚠ THROUGH THE FILE, not through `persistable` alone: a field `serde` never writes would
        // still satisfy an in-process round trip, which is the neighbouring gate's argument.
        let on_disk = serde_json::to_string(&registry.persistable()).expect("the run log encodes");
        let read_back: RunLog = serde_json::from_str(&on_disk).expect("and decodes");
        let mut successor = RunRegistry::default();
        successor.restore(&read_back);

        let restored = successor.snapshot();
        let carried = restored[0].progress.deliveries;
        assert_eq!(
            (carried.made, carried.folded),
            (14, 3),
            "⛔⛔⛔ ITEM 606: this run typed 14 prompts and 3 of them were folded away, and a \
             daemon restart lost the pair. That number is the whole of item 591's instrument, and \
             a run is READ after it has ended — by which time the daemon that drove it has usually \
             been restarted. Got {carried:?} from {on_disk}",
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
            deliveries: sprag_plugin::Deliveries::NONE,
            checks: sprag_plugin::Checks::NONE,
            banked: None,
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
                iterations: 2,
                cost: None,
                unit: None,
                finished: false,
                outcome: None,
                ceiling: None,
                output: None,
                build: None,
                opened_by_session: Some(A_CONVERSATION.to_owned()),
                at: None,
                document: None,
                stood_down: None,
                cancelled_by: None,
                deliveries: None,
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
            iterations: 12,
            cost: None,
            unit: None,
            finished: false,
            outcome: None,
            ceiling: None,
            output: None,
            build: None,
            opened_by_session: None,
            at: at.map(str::to_owned),
            document: document.map(str::to_owned),
            stood_down: None,
            cancelled_by: None,
            deliveries: None,
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
                .any(|order| matches!(order, RunOrder::StandDown))
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
            opened_by: None,
            opened_by_session: None,
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
            registry.stand_down(RunId(0)),
            Ok(()),
            "the run is in the directory and its handle reads the order",
        );
        let told = heard(&log);
        assert_eq!(
            told,
            vec![RunOrder::StandDown],
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

        assert_eq!(
            registry.hold(RunId(0), true),
            Ok(()),
            "the run is in the directory and its handle reads the order",
        );
        assert_eq!(
            registry.hold(RunId(0), false),
            Ok(()),
            "and it is still there, and a release is the same order lowered",
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
                Arc::clone(&cancel),
                Arc::clone(&stand),
                Arc::clone(&hold),
                // ⚠ BOTH: this gate is about which FLAG each order reaches, so a handle that
                // refused one would take that order's arm out of the measurement entirely.
                sprag_plugin::StandingOrder::ALL.to_vec(),
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
        run.deliver(RunOrder::StandDown);
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
                iterations: 3,
                cost: None,
                unit: None,
                finished: false,
                outcome: None,
                ceiling: None,
                output: None,
                build: None,
                opened_by_session: None,
                at: None,
                document: None,
                // ⚠ A log with no such field: `None`, which restores as *no order was recorded*.
                stood_down: None,
                // ⚠ And no cancel was recorded either, which is what an interrupted run looks
                // like: the daemon holding it went away without sweeping, so nobody raised one.
                cancelled_by: None,
                // ⚠ Nor what it delivered — item 606's field, absent in a log written before it.
                deliveries: None,
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
        for (order, answer) in [
            ("stand-down", registry.stand_down(RunId(4))),
            ("hold", registry.hold(RunId(4), true)),
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
}
