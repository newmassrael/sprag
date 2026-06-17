//! sprag-orchestrator — a guardrailed orchestration loop over panes.
//!
//! The control flow is an SCE/SCXML statechart (`orchestration.scxml`,
//! `datamodel="null"` — a pure controller); this crate is the effect executor
//! that drives it. Each step reads the statechart's current state, performs
//! the corresponding I/O against a pane (act: inject a stimulus; perceive:
//! read the screen), evaluates the guardrails (iteration / cost counters live
//! here, in plain Rust), and feeds back the next event. Termination is three
//! distinct statechart `<final>` states surfaced as [`OutcomeState`].
//!
//! Perception reuses the producer's damage tracking: [`Orchestrator::observe`]
//! waits on per-row `generation` stamps (the same mechanism the projection
//! exposes for incremental reads), not a full-screen text diff. Convergence
//! matches the sentinel against a *collapsed* screen view (trailing blanks
//! trimmed, rows joined without separators) so a sentinel the terminal wrapped
//! across rows still matches.
//!
//! This is the first dogfood of SCE for sprag's own control logic (memory
//! `use-sce-for-statecharts`); it is pinion-free (producer/control layer),
//! reusing the input encoder ([`sprag_input`]) and the session handle
//! ([`sprag_terminal::SessionHandle`]). Real AI↔AI orchestration (AI-tool
//! adapters, multi-pane relay, RPC exposure) layers on this substrate later.

mod sm {
    // Generated code: blanket-allow rustc + clippy lints (it is machine-emitted
    // and not hand-maintained).
    #![allow(warnings, clippy::all, clippy::pedantic, clippy::nursery)]
    include!(concat!(env!("OUT_DIR"), "/orchestration_sm.rs"));
}

use std::thread::sleep;
use std::time::{Duration, Instant};

use sce_rust_runtime::Engine;
use sprag_input::{encode, Modifiers};
use sprag_terminal::SessionHandle;
use sprag_vt::Screen;

use sm::{OrchestrationEvent, OrchestrationPolicy, OrchestrationState};

/// How long [`Orchestrator::observe`] waits for the pane to react before
/// giving up on a change and judging on the current screen.
const OBSERVE_TIMEOUT: Duration = Duration::from_millis(500);
/// Poll interval while observing.
const OBSERVE_POLL: Duration = Duration::from_millis(10);

/// What an orchestration drives toward and the guardrails that bound it.
#[derive(Clone, Debug)]
pub struct OrchestrationSpec {
    /// Text injected into the pane each iteration (followed by Enter).
    pub stimulus: String,
    /// Convergence condition: the run succeeds once the observed screen text
    /// contains this. `None` means "never converges" (runs until a guardrail).
    pub sentinel: Option<String>,
    /// Termination guardrail: stop after this many stimulate→observe cycles.
    pub max_iterations: u32,
    /// Cost guardrail: stop once this many bytes have been injected (a headless
    /// proxy for token/$ spend; a real adapter replaces it with token cost).
    pub max_injected_bytes: u64,
}

/// Which terminal statechart state the run reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutcomeState {
    /// The sentinel was observed.
    Converged,
    /// A guardrail (iteration or cost budget) stopped the run.
    Exhausted,
    /// An I/O failure (encode or PTY write) aborted the run.
    Failed,
}

/// Why a [`OutcomeState::Failed`] run aborted — a typed cause rather than a
/// discarded error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DriveError {
    /// A key in the stimulus had no PTY-byte encoding (the offending key).
    Encode(String),
    /// Writing the encoded bytes to the pane failed (the IO error message).
    Write(String),
}

/// The result of an orchestration run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub state: OutcomeState,
    pub iterations: u32,
    pub injected_bytes: u64,
    /// The cause when `state` is [`OutcomeState::Failed`]; `None` otherwise.
    pub failure: Option<DriveError>,
}

/// Drives a [`OrchestrationSpec`] against one pane: the effect executor for
/// the orchestration statechart.
pub struct Orchestrator {
    engine: Engine<OrchestrationPolicy>,
    handle: SessionHandle,
    spec: OrchestrationSpec,
    iterations: u32,
    injected_bytes: u64,
    /// Per-row damage generations captured before the last stimulus, so
    /// `observe` can wait for *this* stimulus's echo.
    baseline_generations: Vec<u64>,
    /// The collapsed screen text from the last observe (for sentinel match).
    last_observed: String,
    /// Set when a `stimulate` failed (surfaced in the [`Outcome`]).
    failure: Option<DriveError>,
}

impl Orchestrator {
    /// Build an orchestrator driving `spec` against the pane behind `handle`.
    #[must_use]
    pub fn new(handle: SessionHandle, spec: OrchestrationSpec) -> Self {
        Self {
            engine: Engine::new(OrchestrationPolicy::new()),
            handle,
            spec,
            iterations: 0,
            injected_bytes: 0,
            baseline_generations: Vec::new(),
            last_observed: String::new(),
            failure: None,
        }
    }

    /// Run the loop to a terminal state and report the [`Outcome`].
    ///
    /// The statechart owns the control topology + guardrail branching; this
    /// loop performs each state's I/O and feeds the next event.
    #[must_use]
    pub fn run(mut self) -> Outcome {
        self.engine.initialize();
        self.engine.process_event(OrchestrationEvent::Start);
        while !self.engine.is_in_final_state() {
            match self.engine.get_current_state() {
                OrchestrationState::Stimulating => {
                    let event = match self.stimulate() {
                        Ok(()) => OrchestrationEvent::Stimulated,
                        Err(error) => {
                            self.failure = Some(error);
                            OrchestrationEvent::Fail
                        }
                    };
                    self.engine.process_event(event);
                }
                OrchestrationState::Observing => {
                    self.observe();
                    self.engine.process_event(OrchestrationEvent::Observed);
                }
                OrchestrationState::Judging => {
                    let event = self.judge();
                    self.engine.process_event(event);
                }
                // Idle is left immediately after Start; finals end the loop.
                _ => break,
            }
        }
        self.outcome()
    }

    /// Act: encode the stimulus (plus Enter) and write it to the pane,
    /// baselining the pane's damage generations first so `observe` can wait
    /// for this stimulus's echo.
    fn stimulate(&mut self) -> Result<(), DriveError> {
        self.baseline_generations = self.handle.with_screen(row_generations);
        let modes = self.handle.input_modes();
        let mut bytes = Vec::new();
        for ch in self.spec.stimulus.chars() {
            let key = ch.to_string();
            let encoded = encode(&key, Modifiers::default(), modes)
                .ok_or_else(|| DriveError::Encode(key.clone()))?;
            bytes.extend_from_slice(&encoded);
        }
        let enter = encode("Enter", Modifiers::default(), modes)
            .ok_or_else(|| DriveError::Encode("Enter".to_string()))?;
        bytes.extend_from_slice(&enter);
        self.handle
            .write(&bytes)
            .map_err(|e| DriveError::Write(e.to_string()))?;
        self.iterations += 1;
        self.injected_bytes += bytes.len() as u64;
        Ok(())
    }

    /// Perceive: wait (bounded) for any row's damage `generation` to advance
    /// past the pre-stimulus baseline, then capture the collapsed screen text.
    fn observe(&mut self) {
        let start = Instant::now();
        loop {
            let advanced = self.handle.with_screen(|screen| {
                (0..screen.rows()).any(|row| {
                    let current = screen.row_generation(row).unwrap_or(0);
                    let baseline = self
                        .baseline_generations
                        .get(row as usize)
                        .copied()
                        .unwrap_or(0);
                    current > baseline
                })
            });
            if advanced || start.elapsed() >= OBSERVE_TIMEOUT {
                self.last_observed = self.handle.with_screen(collapsed_text);
                return;
            }
            sleep(OBSERVE_POLL);
        }
    }

    /// Judge: convergence wins; else a guardrail; else iterate.
    fn judge(&self) -> OrchestrationEvent {
        let converged = self
            .spec
            .sentinel
            .as_ref()
            .is_some_and(|sentinel| self.last_observed.contains(sentinel.as_str()));
        if converged {
            return OrchestrationEvent::Converge;
        }
        if self.iterations >= self.spec.max_iterations
            || self.injected_bytes >= self.spec.max_injected_bytes
        {
            return OrchestrationEvent::Exhaust;
        }
        OrchestrationEvent::Iterate
    }

    fn outcome(self) -> Outcome {
        let state = match self.engine.get_current_state() {
            OrchestrationState::Converged => OutcomeState::Converged,
            OrchestrationState::Exhausted => OutcomeState::Exhausted,
            // Failed, or any state the loop broke out of unexpectedly.
            _ => OutcomeState::Failed,
        };
        Outcome {
            state,
            iterations: self.iterations,
            injected_bytes: self.injected_bytes,
            failure: self.failure,
        }
    }
}

/// The per-row damage generations of a screen.
fn row_generations(screen: &Screen) -> Vec<u64> {
    (0..screen.rows())
        .map(|row| screen.row_generation(row).unwrap_or(0))
        .collect()
}

/// Collapse a screen to text for sentinel matching: each row's cells with
/// trailing blanks trimmed, joined WITHOUT row separators, so a sentinel the
/// terminal wrapped across rows still matches.
fn collapsed_text(screen: &Screen) -> String {
    let mut out = String::new();
    for row in 0..screen.rows() {
        let mut line = String::new();
        for col in 0..screen.cols() {
            if let Some(cell) = screen.cell(col, row) {
                line.push_str(&cell.cluster);
            }
        }
        out.push_str(line.trim_end());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use sprag_terminal::{CommandBuilder, TerminalSession};

    /// A live `cat` pane (echoes injected input back via the line discipline).
    fn cat_session(cols: u16, rows: u16) -> TerminalSession {
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg("cat");
        command.env("TERM", "dumb");
        TerminalSession::spawn(command, cols, rows).expect("spawn pty session")
    }

    #[test]
    fn exhausts_after_max_iterations() {
        let session = cat_session(20, 4);
        let spec = OrchestrationSpec {
            stimulus: "ping".to_string(),
            sentinel: None, // never converges — the iteration budget binds
            max_iterations: 3,
            max_injected_bytes: u64::MAX,
        };
        let outcome = Orchestrator::new(session.handle(), spec).run();
        assert_eq!(outcome.state, OutcomeState::Exhausted);
        assert_eq!(outcome.iterations, 3);
        assert!(outcome.failure.is_none());
    }

    #[test]
    fn converges_on_sentinel() {
        let session = cat_session(20, 4);
        let spec = OrchestrationSpec {
            // cat echoes the stimulus, so the sentinel appears on the first
            // observe.
            stimulus: "ping".to_string(),
            sentinel: Some("ping".to_string()),
            max_iterations: 10,
            max_injected_bytes: u64::MAX,
        };
        let outcome = Orchestrator::new(session.handle(), spec).run();
        assert_eq!(outcome.state, OutcomeState::Converged);
        assert!(outcome.iterations >= 1, "iterations: {}", outcome.iterations);
        assert!(outcome.failure.is_none());
    }

    #[test]
    fn converges_on_a_wrapped_sentinel() {
        // A 4-column pane forces the 6-char echo to wrap across rows
        // ("abcd" + "ef"). The collapsed-text match still finds "abcdef",
        // which a row-newline-joined match would miss.
        let session = cat_session(4, 4);
        let spec = OrchestrationSpec {
            stimulus: "abcdef".to_string(),
            sentinel: Some("abcdef".to_string()),
            max_iterations: 10,
            max_injected_bytes: u64::MAX,
        };
        let outcome = Orchestrator::new(session.handle(), spec).run();
        assert_eq!(outcome.state, OutcomeState::Converged);
    }

    #[test]
    fn cost_budget_also_terminates() {
        let session = cat_session(20, 4);
        let spec = OrchestrationSpec {
            stimulus: "ping".to_string(), // "ping" + Enter = 5 bytes/iteration
            sentinel: None,
            max_iterations: u32::MAX,
            max_injected_bytes: 12, // exhausts within a few iterations
        };
        let outcome = Orchestrator::new(session.handle(), spec).run();
        assert_eq!(outcome.state, OutcomeState::Exhausted);
        assert!(outcome.injected_bytes >= 12, "bytes: {}", outcome.injected_bytes);
    }
}
