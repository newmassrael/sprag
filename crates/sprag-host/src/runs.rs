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
    /// ⚠ It is NOT resumable and does not pretend to be. The pane it drove came back as a plain
    /// shell (see the restore allowlist) and the agent that asked for it is gone with its process.
    Interrupted,
}

struct RunRecord {
    id: RunId,
    label: String,
    /// WHO ASKED for this run — the pane whose occupant wanted it, or [`None`] for a run nobody
    /// claims (what a person starting one from a shell is).
    ///
    /// [`sprag_terminal::Pane::opened_by`]'s field, one level up, and carried for its reason: the
    /// agent-facing mouth keeps an agent to its own runs, and it can only do that if the daemon
    /// remembers whose a run was. The daemon itself enforces nothing with it — see
    /// [`crate::wire::PluginGrammar`] on why this is provenance and not authorisation.
    opened_by: Option<u64>,
    state: Arc<Mutex<RunState>>,
    handle: Option<JoinHandle<()>>,
    /// WHAT THE RUN HAS SPENT SO FAR, shared with the `Driver` that is spending it.
    ///
    /// The counters were readable only in the terminal `Outcome`, so a client watching a long run
    /// could not tell progress from stuck and could not see spend until it was spent — see
    /// [`sprag_plugin::Progress`].
    progress: ProgressCell,
    /// The run's cancel flag, shared with its `WorkspacePaneAccess`; setting it
    /// makes the worker's Driver/plugin stop at its next check.
    cancel: Arc<AtomicBool>,
    /// **THE RUN'S STAND-DOWN FLAG**, shared with the `RunContext` its worker drives through.
    ///
    /// ⚠⚠⚠ A SECOND FLAG AND NOT A SECOND MEANING FOR THE FIRST. Cancel says *stop now and lose the
    /// turn*; this says *finish what you are doing and then stop*. One flag for both would make the
    /// run that banked its milestone and the run that lost it look identical from here, and those
    /// are exactly the two outcomes the person raising one is choosing between.
    order: Arc<AtomicBool>,
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
    /// Where it has got to.
    pub state: RunState,
    /// What it has spent so far — meaningful while [`state`](Self::state) is
    /// [`RunState::Running`], and the last reading the driver took once it is not.
    pub progress: Progress,
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
    /// The pane whose occupant asked for it, or [`None`] for a run nobody claims.
    pub opened_by: Option<u64>,
    /// Where the worker writes its terminal state.
    pub state: Arc<Mutex<RunState>>,
    /// The worker itself.
    pub handle: JoinHandle<()>,
    /// Where the driver writes what it has spent so far.
    pub progress: ProgressCell,
    /// The flag that asks the run to stop at its next check.
    pub cancel: Arc<AtomicBool>,
    /// The flag that asks the run to finish its milestone and then stop — see `RunRecord::order`
    /// for why it is not the one above.
    pub order: Arc<AtomicBool>,
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
            opened_by: run.opened_by,
            state: run.state,
            handle: Some(run.handle),
            progress: run.progress,
            cancel: run.cancel,
            order: run.order,
        });
        id
    }

    /// Raise the cancel flag for run `id`, returning whether such a run exists.
    /// The worker observes it at its next loop-top / wait-poll and ends
    /// [`crate::runs::RunState`]'s outcome as cancelled.
    pub fn cancel(&self, id: RunId) -> bool {
        match self.runs.iter().find(|record| record.id == id) {
            Some(record) => {
                record.cancel.store(true, Ordering::Release);
                true
            }
            None => false,
        }
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
    pub fn stand_down(&self, id: RunId) -> bool {
        match self.runs.iter().find(|record| record.id == id) {
            Some(record) => {
                record.order.store(true, Ordering::Release);
                true
            }
            None => false,
        }
    }

    /// Raise every run's cancel flag — used on host shutdown so in-flight runs abort promptly
    /// instead of being waited out and detached by [`join_all_within`](Self::join_all_within).
    pub fn cancel_all(&self) {
        for record in &self.runs {
            record.cancel.store(true, Ordering::Release);
        }
    }

    /// Join any finished worker threads (non-blocking via `is_finished`),
    /// turning a panicked worker into [`RunState::Panicked`]. Call before
    /// reading the registry so finished threads are reaped, not leaked.
    pub fn sweep(&mut self) {
        for record in &mut self.runs {
            if record.handle.as_ref().is_some_and(JoinHandle::is_finished) {
                let handle = record.handle.take().expect("just checked Some");
                if handle.join().is_err() {
                    *lock(&record.state) = RunState::Panicked("plugin run panicked".to_string());
                }
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
                    }
                })
                .collect(),
        }
    }

    /// Take a predecessor daemon's run log into this registry.
    ///
    /// # ⚠⚠ Two rules, and both are authority decisions rather than conveniences
    ///
    /// 1. **`opened_by` IS DROPPED.** Panes come back across a restart, but a restored pane's
    ///    OCCUPANT is a plain shell and never the agent that asked. The agent-facing mouth filters
    ///    `list_runs` by the caller's own pane id, so carrying the provenance would hand a NEW
    ///    agent booting into restored pane 3 the previous occupant's runs as its own — a hole in
    ///    the exact policy [`crate::wire::PluginGrammar`] describes. A restored run is nobody's,
    ///    which is what a run whose asker is gone actually is.
    /// 2. **THE ID COUNTER IS SEEDED ABOVE THEM.** Ids are monotonic and never reused
    ///    ([`reserve`](Self::reserve)); a successor that started from zero would mint ids that
    ///    already name a run in its own list.
    ///
    /// A restored run has no thread and no cancel flag: `cancel` finds it and returns true having
    /// done nothing, which is the honest answer for a run that is already over.
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
                opened_by: None,
                state: Arc::new(Mutex::new(state)),
                handle: None,
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
                })),
                cancel: Arc::new(AtomicBool::new(false)),
                // ⚠ A RESTORED RUN CANNOT BE STOOD DOWN, and the flag is fresh rather than
                // persisted because there is nothing on the other end of it: the worker that would
                // have read it died with its daemon, and the run is `Interrupted` by construction.
                // Persisting an order would let a restart resurrect an instruction nobody could act
                // on.
                order: Arc::new(AtomicBool::new(false)),
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
                state: lock(&record.state).clone(),
                progress: lock(&record.progress).clone(),
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
    /// its child alive until the process exits — which both real callers do immediately — and its
    /// run never publishes an outcome, so its record stays `Running` and comes back
    /// [`RunState::Interrupted`] to the next daemon. That is the residue of choosing a deadline, and
    /// it is smaller than the alternative, which is a daemon that never dies.
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
            if !self.runs.iter().any(|record| record.handle.is_some()) {
                return Vec::new();
            }
            if Instant::now() >= deadline {
                let outstanding: Vec<RunId> = self
                    .runs
                    .iter()
                    .filter(|record| record.handle.is_some())
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
                opened_by: Some(7),
                state,
                handle,
                progress: ProgressCell::default(),
                cancel,
                order: Arc::new(AtomicBool::new(false)),
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
    /// fails cannot leave a thread behind for the rest of the test binary.
    fn a_worker_that_will_not_come_back(id: RunId, released: &Arc<AtomicBool>) -> NewRun {
        let flag = Arc::clone(released);
        let handle = std::thread::spawn(move || {
            let start = Instant::now();
            while !flag.load(Ordering::Acquire) && start.elapsed() < Duration::from_secs(60) {
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        parked_run(id, "wedged".to_string(), handle)
    }

    /// A run whose worker does what every real one does: reads its cancel flag and comes back.
    fn a_worker_that_honours_its_cancel_flag(id: RunId) -> NewRun {
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancel);
        let handle = std::thread::spawn(move || {
            let start = Instant::now();
            while !flag.load(Ordering::Acquire) && start.elapsed() < Duration::from_secs(60) {
                std::thread::sleep(Duration::from_millis(1));
            }
        });
        NewRun {
            cancel,
            ..parked_run(id, "obedient".to_string(), handle)
        }
    }

    /// A run whose worker returns after `delay` — a healthy one, slow enough that the FIRST sweep
    /// cannot have reaped it.
    fn a_worker_that_comes_back_after(id: RunId, delay: Duration) -> NewRun {
        let handle = std::thread::spawn(move || std::thread::sleep(delay));
        parked_run(id, "healthy".to_string(), handle)
    }

    fn parked_run(id: RunId, label: String, handle: JoinHandle<()>) -> NewRun {
        NewRun {
            id,
            label,
            opened_by: None,
            state: Arc::new(Mutex::new(RunState::Running)),
            handle,
            progress: ProgressCell::default(),
            cancel: Arc::new(AtomicBool::new(false)),
            order: Arc::new(AtomicBool::new(false)),
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

    /// ⚠⚠⚠ **DROPPING A REGISTRY ASKS ITS RUNS TO STOP BEFORE IT WAITS FOR THEM.**
    ///
    /// The deadline made `Drop` bounded; it must not have made it PATIENT. A destructor that joined
    /// without raising the flag would hold every shutdown for the whole deadline and then DETACH a
    /// run that would have come back in milliseconds — which is worse than the unbounded join it
    /// replaced, because it loses the outcome as well as the time.
    #[test]
    fn dropping_a_registry_asks_its_runs_to_stop_before_waiting_for_them() {
        let mut registry = RunRegistry::default();
        let id = registry.reserve();
        registry.submit(a_worker_that_honours_its_cancel_flag(id));

        let raised = Instant::now();
        drop(registry);
        let waited = raised.elapsed();

        assert!(
            waited < RunRegistry::JOIN_DEADLINE / 10,
            "the drop waited {waited:?} — it joined without asking the run to stop",
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
}
