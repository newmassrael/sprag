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
}

pub mod access;
pub mod agent;
pub mod completion;
pub mod deliver;
pub mod dialogue;
pub mod driver;
pub mod orchestrator;
pub mod pipe;
pub mod plugin;
pub mod readiness;
pub mod reply;
pub mod run;
pub(crate) mod session;
#[cfg(test)]
pub(crate) mod testing;

pub use access::{
    AgentObservation, AgentStateSource, Authority, JobLeader, KeyStroke, PaneAccess, PaneDoing,
    PaneError, PaneForegroundJob, PaneInputEcho, PaneJobControl, PaneLifecycle, PaneOutputLines,
    PaneRawCapture, PaneRow, PaneSupervision, PaneTerminalModes, RowTrail, Signalled,
    WorkspacePaneAccess, Written,
};
pub use agent::{Agent, AgentSpec};
pub use deliver::{Delivered, Delivery, deliver, has_painted};
pub use dialogue::{Dialogue, DialogueSpec, Endpoint, ReplyFormat};
pub use driver::{
    Ceiling, Driver, Guardrails, JOURNAL_LIMIT, Outcome, OutcomeState, Progress, ProgressCell,
    StepRecord, Stopped,
};
pub use orchestrator::{OrchestrationSpec, Orchestrator};
pub use pipe::{Pipe, PipeSpec};
pub use plugin::{Cost, Plugin, Step, Verdict};
pub use readiness::{DEFAULT_READY_TIMEOUT, Reached, Readiness, ReadyWhen};
pub use reply::{AgentReply, parse_claude_json};
pub use run::{RunContext, Waited, poll_until};
