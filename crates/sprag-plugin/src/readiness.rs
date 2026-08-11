//! The readiness barrier — *do not type into a pane whose program has not started yet*.
//!
//! One home, because the three plugins that INJECT need it for the same reason and none of them is
//! the reason.
//!
//! # ⚠⚠ Why a loop needs a barrier at all
//!
//! A pane is not ready when it is OPEN. It is born running a shell with the pseudoterminal's
//! default echo on, and the program a caller means to drive — `claude`, a REPL, a test runner —
//! starts some time after that. Anything injected in that window goes to whatever IS there: the
//! shell EXECUTES it as a command, or a reader that is not the peer eats it. Either way the turns
//! are spent, the guardrails count them, and the peer never saw a word.
//!
//! The remedy cannot be a sleep — how long a program takes to come up is not a number a caller
//! knows, and it is the one thing that varies most between the programs this drives. It is a thing
//! the pane SAYS: a prompt, a banner, a marker the caller printed on purpose. So the caller names
//! it and the run waits for it, bounded by the caller's own `ready_within` and by the run's
//! deadline like every other wait here.
//!
//! # Who takes one, and why the last one needed it most
//!
//! Every plugin that TYPES INTO a pane it did not start takes this barrier; the one that does not
//! type takes none. That is the whole rule, and each half of it was measured rather than argued.
//!
//! * [`Orchestrator`](crate::orchestrator::Orchestrator) drives a pane a caller pointed it at. It
//!   got the barrier first.
//! * [`Pipe`](crate::pipe::Pipe) relays into a pane **somebody else prepared**, which is the whole
//!   shape of a relay — and it had no barrier at all. A relay into a pane that was still a shell
//!   was eaten by that shell (`SHELL-ATE relayme`, twice) while the peer that came up a second
//!   later saw nothing, and every number the run reported matched a working relay's.
//! * [`Agent`](crate::agent::Agent) is the worst case, because its failure is a WRONG ANSWER and
//!   not a missing one. It prompts the caller's pane and hands what comes back to a peer as *the
//!   model's reply*; against a shell, the prompt is run as a command and the trailing Ctrl-D makes
//!   the shell EXIT — which is exactly the completion signal it converges on. Measured:
//!   `"summarise the repo"` came back as `"…$ sh: 1: summarise: not found\n$"`, reported
//!   `converged`.
//! * [`Dialogue`](crate::dialogue::Dialogue) takes NONE, and the absence is a finding too: it
//!   passes each turn's prompt as an ARGV ARGUMENT of the pane it spawns for that turn and never
//!   injects a byte, so there is no window for a shell to be typed into.

use std::time::Duration;

use sprag_terminal::PaneId;

use crate::access::{PaneAccess, PaneError};
use crate::run::{RunContext, Waited, poll_until};

/// The bound a readiness wait is given when the caller names none.
///
/// ⚠ Deliberately the same number as [`DEFAULT_REPLY_TIMEOUT`], and deliberately its OWN name.
/// The value is shared because two minutes is this crate's one answer to *"long enough for
/// something on the other side to happen"* and inventing a second number nobody chose would be
/// worse. The name is separate because the QUESTIONS differ — one is how long a model may think,
/// the other is how long a program may take to start — and a caller who wants to change one of
/// them almost never means the other.
///
/// [`DEFAULT_REPLY_TIMEOUT`]: crate::run::DEFAULT_REPLY_TIMEOUT
pub const DEFAULT_READY_TIMEOUT: Duration = crate::run::DEFAULT_REPLY_TIMEOUT;

/// How a [`Readiness`] wait ended, for the two endings that are not an error.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reached {
    /// The pane is ready. Drive it.
    Yes,
    /// THE RUN ended while waiting — cancelled, or out of time. **Nothing was injected**, so
    /// nothing is charged; which of the two it was is the [`RunContext`]'s to answer.
    RunEnded,
}

/// A pane's readiness barrier: what it must SHOW before a plugin types into it, and how long the
/// plugin will wait for that.
///
/// Latched — the wait happens once and every later step drives straight away, so a run pays for
/// this on its first step only.
#[derive(Clone, Debug)]
pub struct Readiness {
    /// What the pane must show. `None` starts driving immediately, which is right for a pane
    /// already running the program.
    ///
    /// ⚠ **PICK SOMETHING THE PROGRAM SAYS, NOT SOMETHING YOU TYPED.** This is matched against the
    /// pane's text, and a pane echoes the command line that STARTED the program — so a marker that
    /// appears in that command line is already on screen before the program exists, and the wait
    /// ends at once against nothing. A prompt or a banner is safe; the word you typed is not.
    marker: Option<String>,
    /// How long to wait for it. See [`DEFAULT_READY_TIMEOUT`] for why this is the caller's.
    within: Duration,
    /// Whether the marker has been seen. Latched.
    seen: bool,
}

impl Readiness {
    /// A barrier for `marker`, waiting `within` (defaulting to [`DEFAULT_READY_TIMEOUT`]).
    ///
    /// A `None` marker is a barrier that is already down: the caller is saying the pane is running
    /// what they mean to drive.
    #[must_use]
    pub fn new(marker: Option<String>, within: Option<Duration>) -> Self {
        Self {
            seen: marker.is_none(),
            marker,
            within: within.unwrap_or(DEFAULT_READY_TIMEOUT),
        }
    }

    /// Wait (once, then latched) for `pane` to show the marker.
    ///
    /// # Errors
    ///
    /// [`PaneError::NeverReady`] when the caller's bound elapses with the marker unseen and the
    /// run still has time. Driving on would inject into whatever IS there and report turns
    /// against a peer that was never listening, so the caller's run stops and says exactly what it
    /// was waiting for.
    ///
    /// ⚠ The RUN's own deadline is the other bound, and it is consulted by `poll_until`: a run
    /// whose clock is shorter ends as [`Reached::RunEnded`] instead. Both are real endings and
    /// they are different findings — one is *this pane never came up*, the other is *the run was
    /// out of time*, and only the first is about the pane.
    pub fn reached(
        &mut self,
        panes: &dyn PaneAccess,
        pane: PaneId,
        run: &RunContext,
    ) -> Result<Reached, PaneError> {
        if self.seen {
            return Ok(Reached::Yes);
        }
        let marker = self.marker.as_deref().unwrap_or_default();
        match poll_until(run, self.within, || {
            panes
                .pane_collapsed(pane)
                .is_some_and(|text| text.contains(marker))
        }) {
            Waited::Ready => {
                self.seen = true;
                Ok(Reached::Yes)
            }
            Waited::Stopped => Ok(Reached::RunEnded),
            Waited::TimedOut => Err(PaneError::NeverReady(marker.to_string())),
        }
    }
}
