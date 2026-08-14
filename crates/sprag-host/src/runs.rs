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

    /// Raise every run's cancel flag — used on host shutdown so in-flight runs
    /// abort promptly instead of `join_all` blocking on them.
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
                })),
                cancel: Arc::new(AtomicBool::new(false)),
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

    /// Join every outstanding worker (blocks until each finishes — bounded by
    /// its guardrails). Called on host shutdown so threads + child processes
    /// reap promptly.
    pub fn join_all(&mut self) {
        for record in &mut self.runs {
            if let Some(handle) = record.handle.take() {
                let _ = handle.join();
            }
        }
    }
}

impl Drop for RunRegistry {
    fn drop(&mut self) {
        // Catch-all: no run thread outlives the registry (so no detached worker
        // keeps a pane/child alive). Cancel first so an in-flight run aborts
        // promptly rather than `join_all` blocking on it (e.g. a slow AI turn).
        // `serve` also does this for deterministic shutdown; the take() / flag
        // make both idempotent.
        self.cancel_all();
        self.join_all();
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
            }),
            RunId(0),
            "a reserved id is the id the record carries",
        );

        // Join (bounded — the worker is trivial) then observe Done.
        registry.join_all();
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
}
