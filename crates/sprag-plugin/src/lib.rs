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
    //! Machine-emitted code: blanket-allow rustc + clippy lints.

    pub(crate) mod orchestration {
        #![allow(warnings, clippy::all, clippy::pedantic, clippy::nursery)]
        include!(concat!(env!("OUT_DIR"), "/orchestration_sm.rs"));
    }

    pub(crate) mod session {
        #![allow(warnings, clippy::all, clippy::pedantic, clippy::nursery)]
        include!(concat!(env!("OUT_DIR"), "/session_sm.rs"));
    }

    pub(crate) mod ai_loop {
        #![allow(warnings, clippy::all, clippy::pedantic, clippy::nursery)]
        include!(concat!(env!("OUT_DIR"), "/ai_loop_sm.rs"));
    }

    // ⚠⚠⚠ THE `<invoke>` PROBE, and its MODULE NAMES ARE A CONTRACT rather than a preference: the
    // generated parent reaches its child as `super::probe_child_sm::ProbeChildPolicy`, so a
    // statechart that invokes another must be included under `<stem>_sm`. The three above are named
    // for their documents because none of them invokes anything; the day one does, it joins this
    // convention. See `probe_parent.scxml` for what is being asked and why it is asked before
    // anything is built on it.
    pub(crate) mod probe_child_sm {
        #![allow(warnings, clippy::all, clippy::pedantic, clippy::nursery)]
        include!(concat!(env!("OUT_DIR"), "/probe_child_sm.rs"));
    }

    pub(crate) mod probe_parent_sm {
        #![allow(warnings, clippy::all, clippy::pedantic, clippy::nursery)]
        include!(concat!(env!("OUT_DIR"), "/probe_parent_sm.rs"));
    }

    // ⚠ `<stem>_sm` because `ai_loop.scxml` will reach it as a child — see the note above.
    pub(crate) mod context_review_sm {
        #![allow(warnings, clippy::all, clippy::pedantic, clippy::nursery)]
        include!(concat!(env!("OUT_DIR"), "/context_review_sm.rs"));
    }
}

pub mod access;
pub mod agent;
pub mod ai_loop;
pub mod answer;
pub mod completion;
pub mod consent;
pub mod deliver;
pub mod dialogue;
pub mod driver;
pub mod judge;
pub mod orchestrator;
pub mod outer;
pub mod pipe;
pub mod plugin;
pub mod readiness;
pub mod reply;
pub(crate) mod report;
pub mod run;
pub mod screen;
pub(crate) mod session;
pub mod spend;
#[cfg(test)]
pub(crate) mod testing;

pub use access::{
    AgentObservation, AgentStateSource, Authority, JobLeader, KeyStroke, PaneAccess, PaneDoing,
    PaneError, PaneForegroundJob, PaneHands, PaneInputEcho, PaneJobControl, PaneLifecycle,
    PaneOutputLines, PaneRawCapture, PaneRow, PaneSupervision, PaneTerminalModes, RowTrail,
    Signalled, WorkspacePaneAccess, Written,
};
pub use agent::{Agent, AgentSpec};
pub use ai_loop::{AiLoop, NotStarted};
pub use answer::Answer;
pub use completion::{Completion, DoneWhen, Over, Turn};
pub use consent::{Answered, Consent, Consents, Refusal, Taken, Unanswered};
pub use deliver::{Delivered, Delivery, deliver, has_painted};
pub use dialogue::{Dialogue, DialogueSpec, Endpoint, ReplyFormat};
pub use driver::{
    Ceiling, Driver, Guardrails, JOURNAL_LIMIT, Outcome, OutcomeState, Progress, ProgressCell,
    StepRecord, Stopped,
};
pub use orchestrator::{OrchestrationSpec, Orchestrator};
pub use outer::{
    AiLoopEvent, AiLoopSpec, AiLoopState, Authored, Brief, Briefed, INNER_SESSION_ENDS,
    NotScreenable, Noticed, OuterLoop, Pumped,
};
pub use pipe::{Pipe, PipeSpec};
pub use plugin::{Cost, Plugin, Step, Verdict};
pub use readiness::{
    Attended, Attention, DEFAULT_READY_TIMEOUT, Handback, Handover, Interruption, Reached,
    Readiness, ReadyWhen,
};
pub use reply::{AgentReply, parse_claude_json};
pub use run::{RunContext, Waited, poll_until};
pub use screen::{Malformed, REFUSES, ScreenRule, ScreenRules, Screened};
pub use spend::{CLAUDE_IDENTITY_FLAG, Spend, identity_in, record_of, spend_in, spend_of};
