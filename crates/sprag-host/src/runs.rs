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

use sprag_plugin::Outcome;

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
    state: Arc<Mutex<RunState>>,
    handle: Option<JoinHandle<()>>,
    /// The run's cancel flag, shared with its `WorkspacePaneAccess`; setting it
    /// makes the worker's Driver/plugin stop at its next check.
    cancel: Arc<AtomicBool>,
}

/// The registry of background plugin runs. Owned by the host (`serve`),
/// shared into each per-request `PluginsExternal` via `Arc<Mutex<_>>`.
#[derive(Default)]
pub struct RunRegistry {
    runs: Vec<RunRecord>,
    next_id: u64,
}

impl RunRegistry {
    /// Register a run — its shared state cell and worker thread — and return a
    /// fresh monotonic id (never reused).
    pub fn submit(
        &mut self,
        label: String,
        state: Arc<Mutex<RunState>>,
        handle: JoinHandle<()>,
        cancel: Arc<AtomicBool>,
    ) -> RunId {
        let id = RunId(self.next_id);
        self.next_id += 1;
        self.runs.push(RunRecord {
            id,
            label,
            state,
            handle: Some(handle),
            cancel,
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

    /// A snapshot of each run's `(id, label, state)`, in submit order.
    #[must_use]
    pub fn snapshot(&self) -> Vec<(RunId, String, RunState)> {
        self.runs
            .iter()
            .map(|record| (record.id, record.label.clone(), lock(&record.state).clone()))
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
                    state: sprag_plugin::OutcomeState::Exhausted,
                    iterations: 0,
                    cost: 0,
                    failure: None,
                },
                output: None,
            };
        });
        let cancel = Arc::new(AtomicBool::new(false));
        let id = registry.submit("test".to_string(), state, handle, cancel);
        assert_eq!(id, RunId(0));

        // Join (bounded — the worker is trivial) then observe Done.
        registry.join_all();
        registry.sweep();
        let snap = registry.snapshot();
        assert_eq!(snap.len(), 1);
        assert!(matches!(snap[0].2, RunState::Done { .. }));
    }
}
