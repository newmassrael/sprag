//! `Session` — one multi-turn endpoint's server-session lifecycle, the reusable
//! resume collaborator extracted from [`Dialogue`](crate::dialogue).
//!
//! A tool like `claude -p --output-format json` returns a `session_id`; a later
//! turn can `--resume <id>` so the server keeps that side's context and the turn
//! sends only the new message instead of resending the whole transcript (the
//! O(n²) growth that also hits the OS arg-size cap on a long run). `Session`
//! answers the two questions that drives: *do I have a live session* (resume vs.
//! send-everything-fresh) and *which one* (the id).
//!
//! ## The lifecycle is an SCE statechart (the dogfood)
//!
//! The control topology — `fresh ⇄ resumed` — is [`session.scxml`], sprag's
//! SECOND SCE machine after the Driver's `orchestration.scxml`. The id STRING is
//! data the engine cannot hold under `datamodel="null"`, so it lives here beside
//! the engine, exactly as the Driver's iteration/cost counters live beside its
//! lifecycle machine: **the engine state is the SSOT for the resume-or-fresh
//! decision; `id` answers only *which* session.** They are not duplicate facts.
//!
//! ## The load-bearing invariant
//!
//! After every [`record`](Session::record), `(state == resumed) ⟺ id.is_some()`.
//! This is not polish — it is what lets [`Dialogue`] gate resume on the *state*
//! (`resuming().is_some()`) rather than re-deriving "was this a resume turn?",
//! and lets the reset rule (a resume that returns no id → start fresh) be the
//! `resumed --absent--> fresh` transition instead of a hand-rolled `match` arm.
//! `record` preserves it by ordering its two effects (store the id *before* the
//! `Opened` transition; clear it *after* an `Absent` transition lands in `fresh`)
//! and `debug_assert!`s it, matching the house discipline for an invariant a
//! single run never violates by construction (cf. [`Cost::reaches`]).
//!
//! ## Why a print-mode endpoint never resumes
//!
//! `Session` is format-blind. A [`ReplyFormat::Text`] turn decodes to
//! `session_id == None` (it has no session concept), so its `Session` only ever
//! sees `Absent` and stays in `fresh` forever — `resuming()` returns `None` and
//! it always sends the whole transcript. That is the load-bearing precondition
//! behind dropping the old explicit `format == ClaudeJson` resume gate: the
//! gate is *subsumed* because only a structured reply ever carries an id.
//!
//! [`session.scxml`]: ../../session.scxml
//! [`Dialogue`]: crate::dialogue::Dialogue
//! [`ReplyFormat::Text`]: crate::dialogue::ReplyFormat::Text
//! [`Cost::reaches`]: crate::plugin::Cost

use sce_rust_runtime::Engine;

use crate::sm::session::{SessionEvent, SessionPolicy, SessionState};

/// One endpoint's server-session lifecycle: `fresh` (no session — resend the
/// whole transcript) or `resumed` (holds an id — `--resume` it and send only the
/// delta), modeled as an SCE statechart with the id stored alongside.
pub(crate) struct Session {
    /// The `fresh ⇄ resumed` control machine — the SSOT for the resume decision.
    engine: Engine<SessionPolicy>,
    /// The server session id to resume; `Some` exactly when the engine is in
    /// `resumed` (the invariant `record` upholds).
    id: Option<String>,
}

impl Session {
    /// A new endpoint session: `fresh`, with no id (it has not spoken yet).
    pub(crate) fn new() -> Self {
        let mut engine = Engine::new(SessionPolicy::new());
        engine.initialize(); // resolve to the initial leaf (`fresh`)
        Self { engine, id: None }
    }

    /// The id to `--resume`, or `None` to start fresh (send the whole
    /// transcript). `Some` exactly in the `resumed` state.
    pub(crate) fn resuming(&self) -> Option<&str> {
        match self.engine.get_current_state() {
            SessionState::Resumed => self.id.as_deref(),
            SessionState::Fresh => None,
        }
    }

    /// Fold a turn's decoded session id into the lifecycle.
    ///
    /// `Some(id)` opens (or keeps) the session — store the id, then transition
    /// `opened` → `resumed`. `None` is the absence of an id: transition `absent`,
    /// which resets a `resumed` side to `fresh` (a bad/expired resume → self-heal
    /// to the whole-transcript path) and leaves a `fresh` side fresh (a
    /// print-mode or first turn); then drop the id a now-`fresh` state must not
    /// keep. The store-before / clear-after ordering is what holds the
    /// state↔id invariant (see the module docs).
    pub(crate) fn record(&mut self, session_id: Option<String>) {
        match session_id {
            Some(id) => {
                self.id = Some(id);
                self.engine.process_event(SessionEvent::Opened);
            }
            None => {
                self.engine.process_event(SessionEvent::Absent);
                if self.engine.get_current_state() == SessionState::Fresh {
                    self.id = None;
                }
            }
        }
        debug_assert_eq!(
            matches!(self.engine.get_current_state(), SessionState::Resumed),
            self.id.is_some(),
            "session id/state desync: resumability and the stored id must agree"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_fresh_with_no_id() {
        let session = Session::new();
        assert_eq!(session.resuming(), None, "a new session must start fresh");
    }

    #[test]
    fn a_session_id_opens_a_resumable_session() {
        let mut session = Session::new();
        session.record(Some("sess-1".to_string()));
        assert_eq!(session.resuming(), Some("sess-1"), "an id must open a resume");
    }

    #[test]
    fn an_id_less_first_turn_stays_fresh() {
        // A print-mode (Text) or id-less reply on a side that never resumed:
        // `absent` from `fresh` is a no-op, so it keeps sending the whole
        // transcript (no spurious resume).
        let mut session = Session::new();
        session.record(None);
        assert_eq!(session.resuming(), None, "an id-less fresh turn must stay fresh");
    }

    #[test]
    fn resume_is_idempotent_and_tracks_the_latest_id() {
        // The real id is stable across resume, but storing the latest carried id
        // is the honest contract; resuming stays open throughout.
        let mut session = Session::new();
        session.record(Some("a".to_string()));
        session.record(Some("a".to_string()));
        assert_eq!(session.resuming(), Some("a"), "a kept session stays resumable");
        session.record(Some("b".to_string()));
        assert_eq!(session.resuming(), Some("b"), "the latest id wins");
    }

    #[test]
    fn a_lost_resume_self_heals_to_fresh() {
        // resumed --absent--> fresh: a resume that came back with NO id
        // (bad/expired/garbled) resets the side so its next turn re-establishes
        // context via the whole-transcript fresh path.
        let mut session = Session::new();
        session.record(Some("sess-1".to_string()));
        assert_eq!(session.resuming(), Some("sess-1"));
        session.record(None);
        assert_eq!(session.resuming(), None, "a lost resume must reset to fresh");
        // And it can open a brand-new session afterwards (fully healed).
        session.record(Some("sess-2".to_string()));
        assert_eq!(session.resuming(), Some("sess-2"), "a healed side resumes again");
    }
}
