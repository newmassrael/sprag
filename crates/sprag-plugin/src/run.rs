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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::sleep;
use std::time::{Duration, Instant};

/// Poll interval for the bounded waits ([`poll_until`]).
pub const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Default overall bound on one AI reply — generous, since a real model thinks
/// for seconds. Shared by the [`Agent`](crate::agent) and
/// [`Dialogue`](crate::dialogue) adapters.
pub const DEFAULT_REPLY_TIMEOUT: Duration = Duration::from_secs(120);

/// The run-scoped lifecycle context handed to each [`Plugin::step`]. Carries
/// the cancel signal (the host raises it); the natural home for a future
/// deadline / approval gate.
///
/// [`Plugin::step`]: crate::plugin::Plugin::step
#[derive(Clone)]
pub struct RunContext {
    cancel: Arc<AtomicBool>,
}

impl RunContext {
    /// A context backed by a host-shared cancel flag (set it to stop the run).
    #[must_use]
    pub fn new(cancel: Arc<AtomicBool>) -> Self {
        Self { cancel }
    }

    /// A context that can never be cancelled — for fire-and-forget runs and
    /// tests that don't exercise cancellation.
    #[must_use]
    pub fn uncancellable() -> Self {
        Self {
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Whether the run has been asked to stop.
    #[must_use]
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }
}

/// How a bounded, cancellable wait ([`poll_until`]) ended.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Waited {
    /// The predicate became true.
    Ready,
    /// The run was cancelled mid-wait (cancel wins over both ready and timeout).
    Cancelled,
    /// The timeout elapsed before the predicate held.
    TimedOut,
}

/// Wait until `predicate` holds, bounded by `timeout`, pre-empted by cancel.
///
/// Polls `predicate` every [`POLL_INTERVAL`], returning [`Waited::Cancelled`]
/// the moment the run is cancelled (so a long in-flight AI turn aborts
/// promptly), [`Waited::Ready`] when the predicate holds, or
/// [`Waited::TimedOut`] when the bound elapses. The one bounded-wait the
/// adapters share — they differ only in their predicate.
pub fn poll_until(run: &RunContext, timeout: Duration, mut predicate: impl FnMut() -> bool) -> Waited {
    let start = Instant::now();
    loop {
        if run.cancelled() {
            return Waited::Cancelled;
        }
        if predicate() {
            return Waited::Ready;
        }
        if start.elapsed() >= timeout {
            return Waited::TimedOut;
        }
        sleep(POLL_INTERVAL);
    }
}
