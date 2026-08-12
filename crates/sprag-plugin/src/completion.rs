//! The completion contract — *what makes a peer's TURN OVER*.
//!
//! One home, for [`readiness`](crate::readiness)' reason: the plugins that wait for a peer decide
//! it for the same reason and none of them is the reason.
//!
//! # ⚠⚠⚠ The asymmetry this exists to close
//!
//! The START of a turn has had a declared, caller-chosen contract since R359b —
//! [`ReadyWhen`](crate::readiness::ReadyWhen), four kinds, and the caller says which question they
//! are asking because **only they know**. Answering the wrong one is silent, which is what earned
//! that a protocol number.
//!
//! The END of a turn had nothing. Each plugin hard-coded a rule of its own:
//!
//! * [`Agent`](crate::agent::Agent) waits for the pane's CHILD TO EXIT.
//! * [`Dialogue`](crate::dialogue::Dialogue) waits for the pane's CHILD TO EXIT — **the same rule,
//!   spelled a second time, in a second module**, with nothing comparing the two.
//! * [`Orchestrator`](crate::orchestrator::Orchestrator) waits for the pane to produce output that
//!   is not the echo of what was typed.
//!
//! One concept, three spellings, none of them nameable by a caller. That is the shape this project
//! has paid for repeatedly — a mouse button encoded in two crates, `full_text` serving two
//! contracts, ten addresses served and never declared — and the remedy is always the same: **give
//! the concept ONE definition and make the caller say which one they mean.**
//!
//! # ⚠⚠ Why the hard-coded rule is not merely untidy
//!
//! *"The child exited"* is a ONE-SHOT TOOL's completion. A long-lived interactive peer — the whole
//! class [`deliver`](mod@crate::deliver), `shows_the_prompt` and
//! [`ReadyWhen::Settles`](crate::readiness::ReadyWhen::Settles) were built for — never exits, so
//! the wait can only end on the CLOCK. The turn is then as long as the timeout (two minutes by
//! default), every time, and the capture is whatever was on screen when it ran out.
//!
//! ⚠ Note what the barrier at the other end can already ask and this one cannot:
//! [`ReadyWhen::Settles`](crate::readiness::ReadyWhen::Settles) reads the peer's own
//! [`AgentObservation`](crate::access::AgentObservation) — *"this agent is at rest, waiting for
//! input"* — carrying an [`Authority`](crate::access::Authority) that says how much the reading is
//! worth. **The evidence exists, it is published, and the end of the turn does not consult it.**
//! That is the variant this vocabulary is being opened for; it is deliberately NOT in this first
//! commit, because a completion rule that fires EARLY truncates a model's answer and publishes the
//! fragment as the reply — the exact failure class this crate has paid for four times — and it has
//! to arrive with the gates that pin it.

use std::time::Duration;

use sprag_terminal::PaneId;

use crate::access::PaneAccess;
use crate::run::{RunContext, Waited, poll_until};

/// WHICH EVIDENCE says a peer's turn is over.
///
/// One variant today, and that is the honest state of the product rather than an abbreviation: it
/// is the rule the two plugins below were already applying, now written once. See the module doc
/// for the variants this is being opened for and why they are not here yet.
///
/// ⚠ NOT on the wire and NOT on any spec, because **no caller can choose yet**. Publishing a
/// vocabulary of one would invite clients to depend on a set whose whole purpose is to grow, and
/// this crate's rule is that a published choice is a choice a caller actually has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DoneWhen {
    /// Over once the pane's CHILD HAS EXITED — the pseudoterminal reached end-of-file.
    ///
    /// The right rule for a ONE-SHOT tool (`claude -p`), which answers and leaves: its exit is
    /// what makes the capture complete, so nothing on screen can be a half-written reply.
    ///
    /// ⚠ It is the WRONG rule for a long-lived peer, which never exits — see the module doc. It
    /// stays the only one until the alternative can be gated, because *waits too long* is the safe
    /// direction and *stops early* publishes a fragment as a model's answer.
    Exits,
}

impl DoneWhen {
    /// Whether `pane` satisfies this contract RIGHT NOW.
    ///
    /// ⚠ An UNKNOWN pane counts as over. A rule that answered *"not yet"* for a pane that is not
    /// there would spin until the timeout on a question that can never be answered — and both
    /// plugins below already spelled this `unwrap_or(true)`, which is the behaviour this preserves
    /// exactly.
    fn satisfied(self, panes: &dyn PaneAccess, pane: PaneId) -> bool {
        match self {
            Self::Exits => panes.pane_eof(pane).unwrap_or(true),
        }
    }

    /// Wait for this contract to be met, bounded by `within` and by the RUN's own deadline.
    ///
    /// [`Waited::TimedOut`] is *the contract was not met in `within`* — for [`Exits`](Self::Exits)
    /// that means the peer never finished, and the caller decides what a partial capture is worth.
    /// [`Waited::Stopped`] is THE RUN ending underneath, which is not this wait's business to
    /// interpret: every caller here hands that back to the driver's loop top, because only it
    /// knows whether it was a cancel or the duration ceiling.
    pub fn wait(
        self,
        panes: &dyn PaneAccess,
        pane: PaneId,
        within: Duration,
        run: &RunContext,
    ) -> Waited {
        poll_until(run, within, || self.satisfied(panes, pane))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::{PaneLifecycle, WorkspacePaneAccess};
    use sprag_terminal::{CommandBuilder, Workspace};
    use std::sync::{Arc, Mutex};

    /// A workspace with one pane running `script`, wrapped as pane-access.
    fn sh_access(script: &str, cols: u16, rows: u16) -> (WorkspacePaneAccess, PaneId) {
        let workspace = Arc::new(Mutex::new(Workspace::new((cols, rows))));
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(script);
        command.env("TERM", "dumb");
        let id = workspace
            .lock()
            .expect("the workspace mutex")
            .spawn(command, "sh".to_string(), cols, rows)
            .expect("spawn pane");
        (WorkspacePaneAccess::new(workspace), id)
    }

    /// ⚠⚠ **THE RULE DISCRIMINATES, which is the only thing worth asserting about one variant.**
    ///
    /// A contract that answered the same thing for a peer that ended and one that is still running
    /// would be a constant, and a constant is not evidence — the shape `PaneEcho`'s gate is built
    /// on, applied here.
    ///
    /// The SUBJECT is a child that exits on its own; the CONTROL is `cat`, which holds its
    /// pseudoterminal open forever and is exactly the long-lived peer the module doc is about.
    #[test]
    fn a_turn_is_over_when_its_one_shot_peer_has_exited_and_not_before() {
        let (ended, pane) = sh_access("exit 0", 20, 4);
        assert_eq!(
            DoneWhen::Exits.wait(
                &ended,
                pane,
                Duration::from_secs(10),
                &RunContext::uncancellable(),
            ),
            Waited::Ready,
            "a one-shot peer's exit is what makes its capture complete",
        );
        ended.lifecycle().expect("lifecycle").close(pane);

        let (running, pane) = sh_access("exec cat", 20, 4);
        assert_eq!(
            DoneWhen::Exits.wait(
                &running,
                pane,
                Duration::from_millis(200),
                &RunContext::uncancellable(),
            ),
            Waited::TimedOut,
            "⚠⚠ and a peer that never exits can only end this wait on the CLOCK — the whole \
             reason a second kind of evidence is owed, spelled here as a measurement rather than \
             as a claim in a comment",
        );
        running.lifecycle().expect("lifecycle").close(pane);
    }
}
