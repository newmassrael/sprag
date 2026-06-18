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
    // Generated code: blanket-allow rustc + clippy lints (machine-emitted).
    #![allow(warnings, clippy::all, clippy::pedantic, clippy::nursery)]
    include!(concat!(env!("OUT_DIR"), "/orchestration_sm.rs"));
}

pub mod access;
pub mod agent;
pub mod dialogue;
pub mod driver;
pub mod orchestrator;
pub mod pipe;
pub mod plugin;
pub mod run;

pub use access::{KeyStroke, PaneAccess, PaneError, PaneLifecycle, PaneRow, WorkspacePaneAccess};
pub use agent::{Agent, AgentSpec};
pub use dialogue::{Dialogue, DialogueSpec};
pub use driver::{Driver, Guardrails, Outcome, OutcomeState};
pub use orchestrator::{OrchestrationSpec, Orchestrator};
pub use pipe::Pipe;
pub use plugin::{Plugin, Step, Verdict};
pub use run::{poll_until, RunContext, Waited};
