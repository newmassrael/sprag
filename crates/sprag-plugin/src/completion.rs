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
//! # ⚠ Why the orchestrator's rule is named here and not yet MOVED here
//!
//! The first two are the same predicate over the same evidence, so collapsing them was a rename and
//! nothing else. The third is not: *"the pane produced output that is not the echo"* needs context
//! a turn has to carry — where this turn's output starts (a
//! [`RowTrail`](crate::access::RowTrail) marked BEFORE the injection) and what was typed, so the
//! terminal's echo of it can be discounted. [`Exits`](DoneWhen::Exits) needs neither.
//!
//! Whether that context belongs in each variant's PAYLOAD or in a uniform per-turn evaluator was a
//! decision deliberately left until a second context-needing rule existed, so the API would be read
//! off more than its least typical case. [`Settles`](DoneWhen::Settles) is that second rule and it
//! ANSWERED: it needs to remember the peer's state at the turn's start, which is not a payload the
//! caller can supply — so the evaluator is [`Completion`], armed by
//! [`begin`](Completion::begin), exactly the shape [`Readiness`](crate::readiness::Readiness) uses
//! at the other end.
//!
//! ⚠ The orchestrator's rule now has somewhere to go: its baseline is armed at the same moment
//! `begin` is, so it becomes a third variant holding a [`RowTrail`](crate::access::RowTrail) rather
//! than a fourth spelling. It has not moved yet because that is a behaviour change in the loop
//! whose echo-discounting rule is the subtlest here, and it is owed its own gate rather than a ride
//! on this one.
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
//! worth. **The evidence existed, it was published, and the end of the turn did not consult it.**
//! [`DoneWhen::Settles`] is that consultation.
//!
//! ⚠⚠ It arrives with its gates and not after them, because a completion rule that fires EARLY
//! truncates a model's answer and publishes the fragment as the reply — the exact failure class
//! this crate has paid for four times, reached from a new direction. The first gate written for it
//! is the one that holds a peer's PRE-TURN rest to not being an answer.

use std::time::Duration;

use sprag_detect::AgentState;
use sprag_terminal::PaneId;

use crate::access::PaneAccess;
use crate::run::{RunContext, Waited, poll_until};

/// WHICH EVIDENCE says a peer's turn is over.
///
/// Two kinds, and they are not degrees of one another: a ONE-SHOT tool's turn ends when the process
/// does, and a LONG-LIVED peer's ends when it goes back to waiting. Nothing about a pane says which
/// it is — **only the caller knows**, which is [`ReadyWhen`](crate::readiness::ReadyWhen)'s reason
/// for existing, met at the other end of the same turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DoneWhen {
    /// Over once the pane's CHILD HAS EXITED — the pseudoterminal reached end-of-file.
    ///
    /// The right rule for a ONE-SHOT tool (`claude -p`), which answers and leaves: its exit is
    /// what makes the capture complete, so nothing on screen can be a half-written reply.
    ///
    /// ⚠ It is the WRONG rule for a long-lived peer, which never exits — see the module doc. Use
    /// [`Settles`](Self::Settles) for those.
    Exits,
    /// Over once the AGENT THIS TURN WAS ADDRESSED TO is at rest again, **having been seen to move
    /// first**.
    ///
    /// The rule for a long-lived interactive peer — an agent CLI that answers and goes on waiting.
    /// It is the same observation [`ReadyWhen::Settles`](crate::readiness::ReadyWhen::Settles)
    /// clears the barrier on, asked at the other end of the turn.
    ///
    /// # ⚠⚠⚠ Why stillness alone is NOT the answer
    ///
    /// A peer that is waiting for a prompt is *at rest*. Ask *"is it at rest?"* right after typing
    /// one and the honest answer is YES — it has not started yet. A rule built on the state alone
    /// therefore reports every turn complete in milliseconds, and what gets captured and published
    /// AS THE MODEL'S ANSWER is the screen before the model wrote a word.
    ///
    /// That is [`ReadyWhen::Prints`](crate::readiness::ReadyWhen::Prints)' hazard exactly — a
    /// condition satisfied by what was already true when the turn began — and it is answered the
    /// same way: by ARMING. [`AgentObservation::seq`](crate::access::AgentObservation::seq) counts
    /// published changes and never decreases, so this contract holds only once the pane's state has
    /// MOVED past what it was when the turn started. Its own doc says it is for exactly this
    /// comparison.
    ///
    /// # ⚠ What it deliberately does NOT complete on
    ///
    /// * [`AgentState::Blocked`] — the peer stopped because it ASKED something. The turn did end,
    ///   but *"here is your answer"* and *"answer my question first"* are opposite instructions,
    ///   and a loop that read them the same way would submit its next stimulus INTO a menu. Waiting
    ///   the timeout out is the honest answer until a run can report the question, which is a
    ///   decision about the step vocabulary and not about this rule.
    /// * An observation naming a DIFFERENT agent than the one the turn was addressed to, or naming
    ///   none — that is not evidence about this peer.
    /// * A host with no supervisor at all, which simply never satisfies this and times out. Every
    ///   one of these fails in the direction that WAITS, because the other direction publishes a
    ///   fragment as a model's reply.
    ///
    /// # ⚠⚠ Why it does NOT name the agent, where [`ReadyWhen::Settles`] must
    ///
    /// The barrier at the other end is deciding *is the right program up yet*, so the caller has to
    /// say which program they meant. By the time a turn ENDS that question is settled: the prompt
    /// went to whatever was in the pane, and [`Completion::begin`] read WHICH AGENT that was. So
    /// the name is taken from the observation the turn was actually addressed to rather than from
    /// the caller — which is both one less argument and a stricter check, because a caller's name
    /// can be right about the pane while being the wrong pane.
    ///
    /// [`ReadyWhen::Settles`]: crate::readiness::ReadyWhen::Settles
    Settles,
}

impl DoneWhen {
    /// The words a caller may spell, in this type's own order.
    ///
    /// Published to every mouth from here rather than retyped as literals, so a third kind reaches
    /// the wire in the compile that adds it — [`ReadyWhen::WIRE_WORDS`](crate::readiness::ReadyWhen::WIRE_WORDS)'
    /// rule, which this vocabulary is the twin of.
    pub const WIRE_WORDS: &'static [&'static str] = &["exits", "settles"];

    /// The word this kind is spelled as on the wire.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Exits => "exits",
            Self::Settles => "settles",
        }
    }

    /// The contract named by `word`, or `None` for a word outside the closed set.
    ///
    /// ⚠ A caller who sends something else has made a MALFORMED request, not a rejected one —
    /// R353's rule, and the reason this returns an `Option` for the parser to turn into the wire's
    /// own grammar refusal rather than a friendly sentence.
    ///
    /// ⚠ A BARE WORD, and that is what the published vocabulary and the parser agreeing looks
    /// like: every word here is one this surface accepts ALONE. A kind that needed a companion
    /// argument would be a word the wire advertises and the daemon refuses — which is precisely
    /// what `every_published_word_is_a_word_the_plugin_host_accepts` caught on this argument's
    /// first draft.
    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.word() == word)
    }

    /// Every kind, so the published vocabulary and the parser are read off ONE list.
    const ALL: [Self; 2] = [Self::Exits, Self::Settles];
}

/// The per-turn evaluator of a [`DoneWhen`] — the contract plus whatever the turn has to REMEMBER
/// for it.
///
/// Mirrors [`Readiness`](crate::readiness::Readiness), which exists for the same reason at the
/// other end: some conditions are not predicates over the present. [`DoneWhen::Settles`] has to
/// know what the peer's state was when the turn began, and a bare `match` on the enum could not.
///
/// ⚠ ONE door, even though [`DoneWhen::Exits`] needs none of this. Two entry points — a plain
/// predicate for the stateless rules and an evaluator for the rest — would be two ways to ask one
/// question, which is the shape this crate keeps finding defects in; and a caller that reached for
/// the cheap door with a rule that needed the other would get a contract that is silently never
/// satisfied.
#[derive(Clone, Debug)]
pub struct Completion {
    /// Which evidence ends the turn.
    when: DoneWhen,
    /// WHO the turn was addressed to and how many published changes their pane had been through,
    /// read when the turn BEGAN.
    ///
    /// `None` until [`begin`](Self::begin) arms it, and `None` after it where the host has no
    /// supervisor to ask or no agent was identified — which is why [`DoneWhen::Settles`] treats an
    /// unarmed evaluator as never satisfied rather than as immediately satisfied.
    ///
    /// ⚠ The NAME is armed, not taken from the caller: see [`DoneWhen::Settles`]. It is what makes
    /// *"the agent came back to rest"* a claim about the peer this turn was actually given to.
    addressed: Option<(String, u64)>,
}

impl Completion {
    /// An evaluator for `when`, not yet armed.
    #[must_use]
    pub const fn new(when: DoneWhen) -> Self {
        Self {
            when,
            addressed: None,
        }
    }

    /// Arm it — **called where the turn BEGINS, before a byte is injected**.
    ///
    /// ⚠ The ordering is the whole guarantee. Armed after the injection, the peer may already have
    /// started and stopped, and the change this looks for would be one the turn missed; armed
    /// before, the comparison is against the state the turn was addressed to.
    pub fn begin(&mut self, panes: &dyn PaneAccess, pane: PaneId) {
        self.addressed = panes
            .supervision()
            .and_then(|supervisor| supervisor.pane_agent_state(pane))
            .and_then(|seen| seen.agent.map(|agent| (agent, seen.seq)));
    }

    /// Whether `pane` satisfies this contract RIGHT NOW.
    fn satisfied(&self, panes: &dyn PaneAccess, pane: PaneId) -> bool {
        match &self.when {
            // ⚠ An UNKNOWN pane counts as over. A rule that answered "not yet" for a pane that is
            // not there would spin to the timeout on a question that can never be answered — and
            // both plugins that use this already spelled it `unwrap_or(true)`, which is the
            // behaviour this preserves exactly.
            DoneWhen::Exits => panes.pane_eof(pane).unwrap_or(true),
            DoneWhen::Settles => {
                // Never armed, no supervisor to arm from, or no agent identified in the pane the
                // prompt went to — see `begin`. None of those is evidence that a turn ended.
                let Some((addressed, began_at)) = &self.addressed else {
                    return false;
                };
                panes
                    .supervision()
                    .and_then(|supervisor| supervisor.pane_agent_state(pane))
                    .is_some_and(|seen| {
                        // ⚠⚠ ALL THREE, and the last one is what stops a peer's PRE-TURN rest from
                        // reading as its answer. See the variant's doc.
                        seen.state == AgentState::Idle
                            && seen.agent.as_deref() == Some(addressed.as_str())
                            && seen.seq > *began_at
                    })
            }
        }
    }

    /// Wait for this contract to be met, bounded by `within` and by the RUN's own deadline.
    ///
    /// [`Waited::TimedOut`] is *the contract was not met in `within`* — the caller decides what a
    /// partial capture is worth. [`Waited::Stopped`] is THE RUN ending underneath, which is not
    /// this wait's business to interpret: every caller here hands that back to the driver's loop
    /// top, because only it knows whether it was a cancel or the duration ceiling.
    pub fn wait(
        &self,
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
    use crate::access::WorkspacePaneAccess;
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
    /// A pane whose agent state this test DRIVES, plus a handle to move it — the question here is
    /// what the contract does with an observation, not how one is derived.
    fn supervised(state: AgentState, seq: u64) -> (WorkspacePaneAccess, PaneId, Reported) {
        let (access, pane) = sh_access("exec cat", 20, 4);
        let reported: Reported = Arc::new(Mutex::new((state, seq, Some("claude".to_string()))));
        let source = {
            let reported = Arc::clone(&reported);
            Arc::new(move |_id: PaneId| {
                let (state, seq, agent) = reported.lock().expect("the reported mutex").clone();
                Some(crate::access::AgentObservation {
                    state,
                    agent,
                    authority: crate::access::Authority::Reported {
                        source: "test".to_string(),
                    },
                    seq,
                    asking: None,
                })
            })
        };
        (access.with_agent_state(Some(source)), pane, reported)
    }

    /// What a test moves the supervised pane's observation with: the state, its published `seq`,
    /// and WHICH agent is reported there.
    type Reported = Arc<Mutex<(AgentState, u64, Option<String>)>>;

    /// ⚠⚠⚠ **A PEER'S REST FROM BEFORE THE TURN IS NOT ITS ANSWER** — the failure this rule would
    /// otherwise introduce, and the reason it was gated before it was wired to anything.
    ///
    /// An interactive agent waiting for a prompt is AT REST. Ask *"is it at rest?"* the instant
    /// after typing one and the honest answer is yes: it has not started. A contract built on the
    /// state alone therefore calls every turn complete in milliseconds, and the capture published
    /// **as the model's answer** is the screen from before the model wrote a word — this crate's
    /// most expensive failure class, arrived at from a new direction.
    ///
    /// So the peer is left EXACTLY as a real one is at that moment — `Idle`, named, and not having
    /// moved — and the contract must refuse it.
    #[test]
    fn a_peer_that_was_already_at_rest_has_not_answered_this_turn() {
        let (access, pane, _reported) = supervised(AgentState::Idle, 7);
        let mut done = Completion::new(DoneWhen::Settles);
        done.begin(&access, pane);

        assert_eq!(
            done.wait(
                &access,
                pane,
                Duration::from_millis(200),
                &RunContext::uncancellable(),
            ),
            Waited::TimedOut,
            "⚠⚠⚠ the peer is at rest and named, and it has NOT answered — it never started. A \
             contract satisfied here captures the screen from before the model wrote a word and \
             publishes it as the model's reply.",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠ **AND IT DOES COMPLETE WHEN THE PEER ACTUALLY ANSWERS** — the other half, without which
    /// the gate above is satisfied by a rule that refuses everything.
    ///
    /// The peer is working when the turn arms, then goes to rest with a fresh `seq`, which is what
    /// a real agent's turn looks like from the supervisor's side.
    #[test]
    fn a_turn_is_over_when_the_peer_it_addressed_comes_back_to_rest() {
        let (access, pane, reported) = supervised(AgentState::Working, 7);
        let mut done = Completion::new(DoneWhen::Settles);
        done.begin(&access, pane);

        // The peer answers and goes quiet — a published change, which is what `seq` counts.
        *reported.lock().expect("the reported mutex") =
            (AgentState::Idle, 8, Some("claude".to_string()));

        assert_eq!(
            done.wait(
                &access,
                pane,
                Duration::from_secs(5),
                &RunContext::uncancellable(),
            ),
            Waited::Ready,
            "a peer that worked and came back to rest has finished its turn — and this is the \
             evidence the end of a turn never consulted, while the START of one has read it since \
             R359b",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠ **A PEER STILL WORKING DOES NOT END THE TURN, even though its `seq` has moved.**
    ///
    /// The arming comparison alone is not enough: a state that CHANGED is not a state at REST, and
    /// an agent prints, calls a tool and thinks its way through several published changes before
    /// it answers. Both halves are required, and this is the half the `seq` test cannot cover.
    #[test]
    fn a_peer_that_is_still_working_has_not_finished_however_much_it_has_moved() {
        let (access, pane, reported) = supervised(AgentState::Idle, 7);
        let mut done = Completion::new(DoneWhen::Settles);
        done.begin(&access, pane);

        // It started, and has been busy through several published changes.
        *reported.lock().expect("the reported mutex") =
            (AgentState::Working, 11, Some("claude".to_string()));

        assert_eq!(
            done.wait(
                &access,
                pane,
                Duration::from_millis(200),
                &RunContext::uncancellable(),
            ),
            Waited::TimedOut,
            "a peer mid-answer is not a peer that answered — capturing here truncates it",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠ **THE PEER THAT WENT QUIET MUST BE THE PEER THAT WAS ASKED**, and every way this
    /// contract can fail to KNOW leaves it waiting.
    ///
    /// Three ways, one discipline — the other direction publishes a fragment as a model's answer:
    ///
    /// * The pane's agent CHANGED under the turn. The prompt went to one program and a different
    ///   one is at rest there now; its stillness says nothing about the question that was asked.
    ///   ⚠ This is the arm that the caller-supplied name could not have covered, and the reason
    ///   the name is ARMED from the observation instead: a caller's name can be right about the
    ///   agent while being wrong about the moment.
    /// * The observation names NOBODY — not evidence about anybody.
    /// * The host has no supervisor at all, so the evaluator cannot even arm.
    #[test]
    fn a_contract_that_cannot_know_waits_rather_than_guessing() {
        let (access, pane, reported) = supervised(AgentState::Working, 7);
        let mut done = Completion::new(DoneWhen::Settles);
        done.begin(&access, pane);
        // At rest, and moved — but it is not the program the prompt was given to.
        *reported.lock().expect("the reported mutex") =
            (AgentState::Idle, 8, Some("codex".to_string()));
        assert_eq!(
            done.wait(
                &access,
                pane,
                Duration::from_millis(200),
                &RunContext::uncancellable(),
            ),
            Waited::TimedOut,
            "⚠⚠ the turn was addressed to `claude` and `codex` is what is at rest there now — a \
             contract satisfied here reports another program's quiet as this question's answer",
        );

        // Named nobody: the same absence of evidence, spelled the other way.
        *reported.lock().expect("the reported mutex") = (AgentState::Idle, 9, None);
        assert_eq!(
            done.wait(
                &access,
                pane,
                Duration::from_millis(200),
                &RunContext::uncancellable(),
            ),
            Waited::TimedOut,
            "an observation naming no agent is not evidence about the one that was asked",
        );
        access.lifecycle().expect("lifecycle").close(pane);

        // No supervisor at all: the evaluator cannot even ARM, and must not read that as done.
        let (bare, pane) = sh_access("exec cat", 20, 4);
        let mut done = Completion::new(DoneWhen::Settles);
        done.begin(&bare, pane);
        assert_eq!(
            done.wait(
                &bare,
                pane,
                Duration::from_millis(200),
                &RunContext::uncancellable(),
            ),
            Waited::TimedOut,
            "a host that cannot see agents must not report every turn instantly complete",
        );
        bare.lifecycle().expect("lifecycle").close(pane);
    }

    /// The SUBJECT is a child that exits on its own; the CONTROL is `cat`, which holds its
    /// pseudoterminal open forever and is exactly the long-lived peer the module doc is about.
    #[test]
    fn a_turn_is_over_when_its_one_shot_peer_has_exited_and_not_before() {
        let (ended, pane) = sh_access("exit 0", 20, 4);
        assert_eq!(
            Completion::new(DoneWhen::Exits).wait(
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
            Completion::new(DoneWhen::Exits).wait(
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
