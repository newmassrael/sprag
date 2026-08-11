//! `RunContext` — the per-run lifecycle context, and the one bounded-wait loop.
//!
//! The signals scoped to a *run* (not to a pane): cancellation today, a
//! deadline or human-approval gate later. Threaded into every [`Plugin::step`]
//! alongside `&dyn PaneAccess`, so the [`Driver`] depends on it for control and
//! `PaneAccess` stays the pane-scoped read/inject surface — each consumer
//! depends only on what it uses (interface segregation). This is the textbook
//! home cancellation belongs in; bolting it onto `PaneAccess` was the wrong
//! seam.
//!
//! [`poll_until`] is the single bounded, cancellable wait every plugin shares,
//! instead of three hand-rolled copies (the R12-R15 await loops).
//!
//! [`Plugin::step`]: crate::plugin::Plugin::step
//! [`Driver`]: crate::driver::Driver
//! [`PaneAccess`]: crate::access::PaneAccess

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::sleep;
use std::time::{Duration, Instant};

/// Poll interval for the bounded waits ([`poll_until`]).
pub const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Default overall bound on one AI reply — generous, since a real model thinks
/// for seconds. Shared by the [`Agent`](crate::agent) and
/// [`Dialogue`](crate::dialogue) adapters.
pub const DEFAULT_REPLY_TIMEOUT: Duration = Duration::from_secs(120);

/// The run-scoped lifecycle context handed to each [`Plugin::step`]. Carries
/// the two signals that end a run from outside its own logic: the cancel flag
/// (the host raises it) and the DEADLINE (the [`Driver`] arms it from
/// [`Guardrails::max_duration`]).
///
/// # ⚠⚠ Why the deadline lives here and not only in the guardrails
///
/// The guardrails are the DECLARATION; this is the armed clock. A ceiling the
/// Driver checked only at its loop top would bound how many steps a run takes
/// and not how long it takes, because a single step is free to block for its own
/// timeout — a dialogue turn waits [`DEFAULT_REPLY_TIMEOUT`] for a model to
/// think. A run asked to stop after one second would then stop after two
/// minutes, which is a bound in name only.
///
/// So the deadline reaches the WAITS: `poll_until` and the delivery path's own
/// loop both consult [`stopped`](Self::stopped), and a step in flight when the
/// deadline passes gives up at its next poll rather than at its own timeout.
///
/// [`Plugin::step`]: crate::plugin::Plugin::step
/// [`Driver`]: crate::driver::Driver
/// [`Guardrails::max_duration`]: crate::driver::Guardrails::max_duration
#[derive(Clone)]
pub struct RunContext {
    cancel: Arc<AtomicBool>,
    /// WHEN THIS RUN MUST BE OVER, or [`None`] for a run nothing times.
    ///
    /// An `Instant` and not a `Duration`, because the question every wait asks is
    /// *"is it past yet?"* and a duration would need each of them to know when
    /// the run began. [`Driver::run`](crate::driver::Driver::run) stamps it from
    /// the guardrail at the top of the run, which is the one moment that answer
    /// is the same for every wait underneath it.
    deadline: Option<Instant>,
}

impl RunContext {
    /// A context backed by a host-shared cancel flag (set it to stop the run),
    /// with no deadline until one is [armed](Self::deadline_in).
    #[must_use]
    pub fn new(cancel: Arc<AtomicBool>) -> Self {
        Self {
            cancel,
            deadline: None,
        }
    }

    /// A context that can never be cancelled — for fire-and-forget runs and
    /// tests that don't exercise cancellation.
    #[must_use]
    pub fn uncancellable() -> Self {
        Self::new(Arc::new(AtomicBool::new(false)))
    }

    /// This context with its deadline set `within` from NOW — what the
    /// [`Driver`](crate::driver::Driver) hands the plugin, sharing the same
    /// cancel flag.
    ///
    /// `None` disarms it, which is what a run with no time ceiling wants and what
    /// every caller that drives a plugin without a host gets by default.
    #[must_use]
    pub fn deadline_in(&self, within: Option<Duration>) -> Self {
        Self {
            cancel: Arc::clone(&self.cancel),
            deadline: within.map(|d| Instant::now() + d),
        }
    }

    /// Whether the run has been asked to stop.
    #[must_use]
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }

    /// Whether the run's deadline has passed. Always false for a run with none.
    #[must_use]
    pub fn expired(&self) -> bool {
        self.deadline.is_some_and(|at| Instant::now() >= at)
    }

    /// Whether the run is OVER — cancelled, or out of time.
    ///
    /// The predicate every bounded wait consults, so neither of the two ways a
    /// run ends from outside can be honoured by one wait and missed by another.
    /// Which of the two it was is the context's to answer
    /// ([`cancelled`](Self::cancelled) / [`expired`](Self::expired)) and not the
    /// wait's, so the reason has ONE authority.
    #[must_use]
    pub fn stopped(&self) -> bool {
        self.cancelled() || self.expired()
    }
}

/// How a bounded wait ([`poll_until`]) ended.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Waited {
    /// The predicate became true.
    Ready,
    /// THE RUN ITSELF ENDED mid-wait — cancelled, or past its deadline. Ask the
    /// [`RunContext`] which; the wait does not hold a second copy of that fact.
    ///
    /// ⚠ This variant replaced a `Cancelled` one when the deadline was added, and
    /// the rename is the point rather than a tidy-up: every site that compared
    /// against `Waited::Cancelled` would have kept compiling with a new variant
    /// beside it and would have read a deadline as *keep going* — the exact class
    /// of silent unbounding this whole ceiling exists to close. A renamed variant
    /// fails to compile at each of them instead.
    Stopped,
    /// The timeout elapsed before the predicate held.
    TimedOut,
}

/// Wait until `predicate` holds, bounded by `timeout` AND by the run's own
/// deadline, pre-empted by cancel.
///
/// Polls `predicate` every [`POLL_INTERVAL`], returning [`Waited::Stopped`] the
/// moment the run is cancelled or out of time (so a long in-flight AI turn
/// aborts promptly), [`Waited::Ready`] when the predicate holds, or
/// [`Waited::TimedOut`] when the local bound elapses. The one bounded wait the
/// adapters share — they differ only in their predicate.
///
/// ⚠ The ORDER is load-bearing twice. Cancel is asked first, so a person's stop
/// beats a predicate that happens to come true in the same instant. And READY is
/// asked before the deadline, so work that finished is never thrown away by a
/// clock that ran out while it was finishing — the run ends at the Driver's next
/// loop top either way, one step later at most.
pub fn poll_until(
    run: &RunContext,
    timeout: Duration,
    mut predicate: impl FnMut() -> bool,
) -> Waited {
    let start = Instant::now();
    loop {
        if run.cancelled() {
            return Waited::Stopped;
        }
        if predicate() {
            return Waited::Ready;
        }
        if run.expired() {
            return Waited::Stopped;
        }
        if start.elapsed() >= timeout {
            return Waited::TimedOut;
        }
        sleep(POLL_INTERVAL);
    }
}
