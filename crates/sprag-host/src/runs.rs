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
    Done {
        outcome: Outcome,
        output: Option<String>,
    },
    /// The worker thread panicked (defensive — a plugin step should not).
    Panicked(String),
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

    #[test]
    fn submit_sweep_join_lifecycle() {
        let mut registry = RunRegistry::default();
        let state = Arc::new(Mutex::new(RunState::Running));
        let worker_state = Arc::clone(&state);
        let handle = std::thread::spawn(move || {
            // A trivial "run" that completes immediately.
            *lock(&worker_state) = RunState::Done {
                outcome: Outcome {
                    state: sprag_plugin::OutcomeState::Exhausted(sprag_plugin::Ceiling::Iterations),
                    iterations: 0,
                    cost: None,
                    failure: None,
                },
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
