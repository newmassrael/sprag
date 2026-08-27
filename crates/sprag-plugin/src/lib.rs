//! sprag-plugin — the plugin extension API, the SCE-statechart Driver, and the
//! bundled control plugins.
//!
//! This is the core's #1 design concern (README "확장 API"): the contract a
//! plugin uses to extend the terminal platform. Three layers, all pinion-free:
//!
//! * [`access`] — `PaneAccess`, the extension API: a plugin reads panes as
//!   scene-as-data and injects input, addressed by `PaneId`, through one
//!   implementation ([`WorkspacePaneAccess`]) so every plugin reads/injects the
//!   same way.
//! * [`plugin`] / [`driver`] — `Plugin::step` is the extension point (a plugin
//!   owns perceive/act/judge); `Driver` is the shared substrate (the SCE/SCXML
//!   statechart + guardrails) that runs any plugin to an `Outcome`.
//! * bundled plugins — [`orchestrator`] (fixed-stimulus drive) and [`pipe`]
//!   (relay one pane's output into another). Two consumers validate the API;
//!   real AI↔AI adapters layer on it later.
//!
//! SCE is sprag's statechart engine (memory `use-sce-for-statecharts`); the
//! `Driver` is its dogfood.

pub(crate) mod sm {
    //! Generated SCE state machines — one submodule per control statechart, each
    //! its own `include!` so the duplicate generated imports (`use
    //! core::time::Duration`, the runtime umbrella) stay isolated per machine.
    //!
    //! # ⚠⚠⚠⚠⚠ EACH MACHINE CARRIES ITS OWN AUDITED SUPPRESSION BUDGET — SCE-PR88, consumed
    //! 2026-08-20
    //!
    //! These modules used to blanket-allow `warnings, clippy::all, clippy::pedantic,
    //! clippy::nursery`, because `build.rs` stripped the machines' own `#![allow(…)]` lines to make
    //! them `include!`-able. The engine now writes an INDEX beside each machine
    //! (`<stem>_sm.include.rs`, a `#[path]` module plus a re-export), so the machine is compiled as
    //! a file with the budget SCE measured per fixture. **Measured on consumption: with the blanket
    //! gone, workspace clippy reports nothing from any machine** — the blanket had been covering
    //! nothing it needed to cover.
    //!
    //! ⚠⚠ One narrow lint stays, and it belongs to the INDEX rather than to the machine: the
    //! generated `pub use <stem>_sm::*;` is an unused import wherever this crate's only consumer of
    //! that machine is a test. It is `unused_imports` alone — a named lint over two generated
    //! lines, not a budget over a machine — and the honest repair is upstream carrying it on the
    //! line it emits.
    //!
    //! ⚠⚠⚠⚠ **A SECOND ONE JOINED IT 2026-08-21, AND IT ARRIVED WITHOUT ANY CODE CHANGING.** The
    //! local toolchain moved to 1.98.0, whose clippy added `drain_collect`, and every generated
    //! machine that invokes a child drains `pending_invokes` into a `Vec`. **A suppression budget
    //! SCE measured at one toolchain does not stay measured** — this is the second named lint the
    //! index carries for the generator, on the same terms as the first, and it is owed upstream in
    //! the same way. ⚠ It is added only where it FIRES: a machine with no `<invoke>` has no drain,
    //! so a blanket here would be back to covering nothing.
    //! ⚠⚠⚠ Neither `cargo test` nor the remote build sees this, and the gap is ONE RELEASE rather
    //! than a stale fleet: measured 2026-08-21, pc2/pc3/pc4 all answer `cargo 1.97.1` while this
    //! machine — the only one the hook's clippy runs on — answers `1.98.0`. So the remote build is
    //! not a second opinion about lints; it is the same opinion one release behind, and CI's rolling
    //! stable reaches 1.98 on its own clock. ⚠ An earlier draft of this note said 1.88, inferred
    //! from `cargo +1.88.0` succeeding there — which proves a toolchain is INSTALLED, not default.

    pub(crate) mod orchestration {
        #![allow(unused_imports)]
        include!(concat!(env!("OUT_DIR"), "/orchestration_sm.include.rs"));
    }

    pub(crate) mod session {
        #![allow(unused_imports)]
        include!(concat!(env!("OUT_DIR"), "/session_sm.include.rs"));
    }

    pub(crate) mod ai_loop {
        #![allow(unused_imports)]
        include!(concat!(env!("OUT_DIR"), "/ai_loop_sm.include.rs"));
    }

    // ⚠⚠ THE DECISIONS ONE LOOP KIND RUNS UNDER. It invokes nothing and nothing invokes it — the
    // driver holds it beside the template and reads its datamodel — so the `<stem>_sm` naming
    // contract below does not bind it, and it is named for its document like the three above.
    pub(crate) mod debt_loop {
        #![allow(unused_imports)]
        include!(concat!(env!("OUT_DIR"), "/debt_loop_sm.include.rs"));
    }

    // ⚠⚠⚠ THE `<invoke>` PROBE, and its MODULE NAMES ARE A CONTRACT rather than a preference: the
    // generated parent reaches its child as `super::probe_child_sm::ProbeChildPolicy`, so a
    // statechart that invokes another must be included under `<stem>_sm`. The three above are named
    // for their documents because none of them invokes anything; the day one does, it joins this
    // convention. See `probe_parent.scxml` for what is being asked and why it is asked before
    // anything is built on it.
    pub(crate) mod probe_child_sm {
        #![allow(unused_imports)]
        include!(concat!(env!("OUT_DIR"), "/probe_child_sm.include.rs"));
    }

    pub(crate) mod probe_parent_sm {
        #![allow(unused_imports)]
        // The generator's `pending_invokes.drain(..).collect()` — see the module docs. Named, not
        // blanket, and owed upstream.
        #![allow(clippy::drain_collect)]
        // ⚠⚠⚠⚠ THE INDEX PUTS THE MACHINE ONE LEVEL DEEPER, AND A PARENT NAMES ITS CHILD BY
        // `super::` — SCE-PR88's consumption, 2026-08-20. The generated parent reaches its invoked
        // child as `super::probe_child_sm::ProbeChildPolicy`; while the machine was `include!`d
        // directly, `super` was this `sm` module and the sibling was right there. Including the
        // `.include.rs` index adds `mod probe_parent_sm` inside this one, so `super` became THIS
        // module — and the sibling had to be nameable from here. One `use` is that, and it is the
        // whole cost of keeping the machine's own suppression budget.
        use super::probe_child_sm;
        include!(concat!(env!("OUT_DIR"), "/probe_parent_sm.include.rs"));
    }

    // ⚠⚠ THE `<parallel>` PROBE. It invokes nothing, so the naming rule above does not bind it —
    // it is `<stem>_sm` anyway, because a supervisor that runs loops concurrently would reach it
    // as a child the day one exists, and renaming a module later is the kind of edit that is
    // remembered as a preference rather than a contract.
    pub(crate) mod probe_parallel_sm {
        #![allow(unused_imports)]
        include!(concat!(env!("OUT_DIR"), "/probe_parallel_sm.include.rs"));
    }

    // ⚠⚠ THE `<data>`-WITH-NO-VALUE PROBE. Same standing as the two above: whether an id a document
    // declares and leaves empty is readable, and safe to guard on, is a fact about THIS generator at
    // the pinned rev — and a wrong answer is a loop that exhausts on its first judged turn.
    pub(crate) mod probe_absent_sm {
        #![allow(unused_imports)]
        include!(concat!(env!("OUT_DIR"), "/probe_absent_sm.include.rs"));
    }

    // ⚠ `<stem>_sm` because `ai_loop.scxml` will reach it as a child — see the note above.
    pub(crate) mod context_review_sm {
        #![allow(unused_imports)]
        include!(concat!(env!("OUT_DIR"), "/context_review_sm.include.rs"));
    }

    // ⚠⚠ THE CUSTOM-`type` PROBE — whether a host can name an act the document asks for. It is
    // compiled here for the same reason as the three above: the question is about THIS generator at
    // the pinned rev, and register item 470's second stage is a design or a filed request depending
    // on the answer.
    pub(crate) mod probe_send_type_sm {
        #![allow(unused_imports)]
        // The generator's `pending_invokes.drain(..).collect()` — see the module docs.
        #![allow(clippy::drain_collect)]
        include!(concat!(env!("OUT_DIR"), "/probe_send_type_sm.include.rs"));
    }

    pub(crate) mod probe_unanswered_sm {
        #![allow(unused_imports)]
        include!(concat!(env!("OUT_DIR"), "/probe_unanswered_sm.include.rs"));
    }
}

/// What this crate has PROVEN about the engine it runs on — see [`probe`].
mod probe;

/// ⚠⚠⚠⚠⚠ **WHICH STATECHART DOCUMENTS THIS BINARY WAS COMPILED FROM** — one word, baked at build
/// time by `build.rs`'s `stamp_fingerprint` over the `.scxml` SOURCES.
///
/// # What it is FOR, and why a persisted run is a trap without it
///
/// Register items 543 and 544. A run that records where it had got to has written down a state
/// NAME, and a name's meaning lives in a document. The restart that motivates persisting a run at
/// all is *the loop document changed* — so reading `reflecting` back against a build whose
/// `ai_loop.scxml` moved is the COMMON case, not the rare one. Recorded beside the position, this
/// makes the skew a fact a reader can see instead of one it cannot: same fingerprint, same words;
/// different fingerprint, **a different run**, which is 544's structural answer stated as data.
///
/// ⚠⚠⚠ **IT IS NOT THE BUILD STAMP.** `sprag_host::wire::BUILD` moves when any file in the tree
/// does, so at that granularity every promotion discards every run — the cost item 543 exists to
/// remove. This moves only when a statechart does, which is the only thing that can invalidate a
/// state word.
///
/// ⚠ Compared for EQUALITY only. It is an identity, never an ordering: nothing here can say
/// whether one document is newer than another, and a fingerprint that invited the question would be
/// answering something it does not know.
pub const STATECHARTS_FINGERPRINT: &str = env!("SPRAG_STATECHARTS_FINGERPRINT");

pub mod access;
pub mod act;
pub mod agent;
pub mod ai_loop;
pub mod answer;
pub mod completion;
pub mod consent;
pub mod deliver;
pub mod dialogue;
pub mod document;
pub mod driver;
pub mod judge;
pub mod kind;
pub mod orchestrator;
pub mod outer;
pub mod pipe;
pub mod plugin;
pub mod readiness;
pub mod reply;
pub(crate) mod report;
pub mod review;
pub mod run;
pub mod screen;
pub(crate) mod session;
pub mod spend;
#[cfg(test)]
pub(crate) mod testing;

pub use access::{
    AgentObservation, AgentStateSource, Authority, CutCheckout, JobLeader, KeyStroke, PaneAccess,
    PaneChanges, PaneCheckout, PaneDoing, PaneError, PaneForegroundJob, PaneHands, PaneInputEcho,
    PaneInputTrail, PaneJobControl, PaneLifecycle, PaneOrigin, PaneOutputLines, PaneRawCapture,
    PaneRow, PaneSupervision, PaneTerminalModes, ReporterVoice, RowTrail, Settling, Signalled,
    WorkspacePaneAccess, Written,
};
pub use act::{Act, Asked, Asks, Does, Refused as ActRefused, Serving};
pub use agent::{Agent, AgentSpec};
pub use ai_loop::{AiLoop, NotStarted};
pub use answer::Answer;
pub use completion::{Completion, DoneWhen, Over, Turn};
pub use consent::{Answered, Consent, Consents, Refusal, Taken, Unanswered};
pub use deliver::{DEFAULT_SUBMIT_GRACE, Delivered, Delivery, SubmittedWhen, deliver, has_painted};
pub use dialogue::{Dialogue, DialogueSpec, Endpoint, ReplyFormat};
pub use document::{Faulted, faults, opened};
pub use driver::{
    Ceiling, Driver, Guardrails, JOURNAL_LIMIT, Outcome, OutcomeState, Progress, ProgressCell,
    ProgressSink, StepRecord, Stopped,
};
pub use orchestrator::{OrchestrationSpec, Orchestrator};
pub use outer::{
    AiLoopEvent, AiLoopSpec, AiLoopState, Authored, Brief, Briefed, Briefing, Counted, DoneReason,
    HOLD_TAKES_EFFECT, HOLD_WITHIN_KEY, INNER_SESSION_ENDS, LoopPlace, Made, NotResumed,
    NotScreenable, Noticed, OuterLoop, Pumped, Retyped, STAND_DOWN_TAKES_EFFECT, refusal_in_words,
};
pub use pipe::{Pipe, PipeSpec};
pub use plugin::{
    Accounting, Banked, Checks, Cost, Deliveries, Edge, Plugin, Resumption, StandingOrder, Step,
    Verdict,
};
pub use readiness::{
    Attended, Attention, DEFAULT_READY_TIMEOUT, Handback, Handover, Interruption, Reached,
    Readiness, ReadyWhen,
};
pub use reply::{AgentReply, parse_claude_json};
pub use run::{Look, RunContext, Waited, park_until, poll_until};
pub use screen::{Malformed, REFUSES, ScreenRule, ScreenRules, Screened};
pub use spend::{
    CLAUDE_IDENTITY_FLAG, Spend, identity_in, record_of, spend_at, spend_in, spend_of,
};
