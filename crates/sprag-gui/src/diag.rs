//! First-party diagnostics for the interactive GUI's input + dock dispatch —
//! the STANDING observability the layout path was missing.
//!
//! Production logging (memory: `production-logging-tracing`): these are leveled
//! [`tracing`] events, NOT ad-hoc `eprintln!` gated by a boolean. Verbosity is
//! dialed per subsystem by LOG LEVEL through the env filter [`install`] wires up
//! (`SPRAG_LOG`, RUST_LOG-style) — e.g. `SPRAG_LOG=sprag_gui::dock=trace` traces
//! only the dock path, `SPRAG_LOG=sprag_gui=debug` the whole binding. Off unless
//! asked for: the default filter is `warn`, so a normal run is quiet and a
//! disabled level pays only `tracing`'s cheap enabled-check (no eager
//! formatting), the same property the old boolean gate gave.
//!
//! The point is that a live misbehaviour — a chord firing more than once per
//! physical press, a toggle landing on the wrong pane, a window appearing/
//! vanishing unexpectedly, a "끌어서 dock" that does nothing — is read STRAIGHT
//! from the log, never re-instrumented by hand each session (the anti-pattern the
//! R64/R65 double-toggle debugging kept hitting). The subscriber's uptime timer
//! stamps each line with a process-relative clock, so the per-dispatch fan-out of
//! one physical press reads at a glance (the ~32 ms gaps that distinguish an OS
//! auto-repeat from a multi-window re-delivery).
//!
//! Events stay TYPED (named functions, not free-form macros at the call site) so
//! the wire is greppable and stable, and each carries a `target:` naming its
//! subsystem (`sprag_gui::input`, `sprag_gui::dock`) for per-target filtering.
//! Levels by audience: per-key/per-chord dispatch fan-out is `trace!`; a topology
//! MUTATION (dock toggle) or a drop-zone verdict an operator would want to see is
//! `debug!`.

use tracing_subscriber::{EnvFilter, fmt};

/// Env var the [`EnvFilter`] reads for the log-level directive (RUST_LOG syntax:
/// `sprag_gui=info`, `sprag_gui::dock=trace`, `warn,sprag_gui::input=debug`, …).
const LOG_ENV: &str = "SPRAG_LOG";

/// Install sprag's global `tracing` subscriber — the production logging entry
/// point, called from `fn main()` BEFORE [`pinion_shell::run`].
///
/// Level control is the `SPRAG_LOG` [`EnvFilter`] (default `warn` = quiet);
/// output is stderr (never stdout — that carries the JSON-RPC wire); the uptime
/// timer gives the process-relative stamp the dispatch fan-out is read by.
///
/// Ordering matters: pinion's `run()` also installs a subscriber
/// (`init_tracing`, env `PINION_LOG`) via idempotent `try_init` (first-wins).
/// Installing here FIRST makes ours authoritative — sprag owns both the env-var
/// name and the defaults — and pinion's becomes the no-op fallback. `try_init`
/// (not `init`) so a test / embedder that already set a subscriber is a no-op,
/// not a panic.
pub(crate) fn install() {
    let filter = EnvFilter::try_from_env(LOG_ENV).unwrap_or_else(|_| EnvFilter::new("warn"));
    let _ = fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_timer(fmt::time::uptime())
        .try_init();
}

/// A key dispatch ENTERED the binding — one event per `apply_key*` call. `entry`
/// names the path (`apply_key` = synthetic / RPC, `apply_key_repeat` = the live
/// shell), so one physical press fanning out to several dispatches reads as
/// several lines with their inter-arrival gaps. `trace!` — the finest-grained
/// per-event fan-out.
pub(crate) fn key_in(
    entry: &str,
    key: &str,
    ctrl: bool,
    shift: bool,
    repeat: bool,
    focused: Option<&str>,
) {
    tracing::trace!(
        target: "sprag_gui::input",
        entry, key, ctrl, shift, repeat, ?focused,
        "key_in",
    );
}

/// An IME composition event ENTERED the binding — one per `apply_composition` call.
/// `kind` is the [`CompositionEvent`](pinion_core::CompositionEvent) variant
/// (`start` / `update` / `commit` / `cancel`), `text` its payload (the live preedit
/// or the committed string). Pairs with [`key_in`] so a Hangul-then-space sequence
/// reads as its true event stream — e.g. a `commit` with NO following space `key_in`
/// exposes an IME key swallowed upstream. `trace!` — per-event fan-out (the same
/// `sprag_gui::input` target as the key trace).
pub(crate) fn composition(kind: &str, text: &str, focused: Option<&str>) {
    tracing::trace!(
        target: "sprag_gui::input",
        kind, text, ?focused,
        "composition",
    );
}

/// `route_key`'s decision for a recognised window chord: `action` is `"act"` (it
/// ran) or `"drop-repeat"` (an OS auto-repeat of a discrete chord, suppressed).
/// Distinguishes "the dispatch arrived but was correctly dropped" from "it acted".
/// `trace!` — per-key-event granularity.
pub(crate) fn chord(name: &str, action: &str, pane: usize) {
    tracing::trace!(target: "sprag_gui::input", chord = name, action, pane, "chord");
}

/// A dock toggle MUTATED the topology: direction + window count before/after, so
/// a stray extra toggle (the count jumping by more than one per press, or moving
/// the wrong way) is obvious. `debug!` — a state change an operator debugging
/// dock behaviour wants to see without the full per-key trace.
pub(crate) fn dock_toggle(pane: usize, undock: bool, before: usize, after: usize) {
    tracing::debug!(
        target: "sprag_gui::dock",
        pane,
        dir = if undock { "undock" } else { "dock" },
        before,
        after,
        "dock_toggle",
    );
}

/// Cross-window redock resolution (R86): the `resolve_drop` verdict a
/// `tear_off_redock_at` classified to, so a "끌어서 dock" that does nothing can be
/// read off the log (`Float` = dead-zone, won't dock; `Dock`/`OuterDock` =
/// relocates; `SnapBack` = own slot). `resolution` is a cheap `&'static str` (the
/// variant name). Logs the full normalised drop point (`x_rel`/`y_rel`). Both
/// dock modes honor the verdict at the zone (pinion R1173's
/// `dock_panel_at_resolved_zone` is total over an absent collapse leaf). `debug!`
/// — a drop-zone decision worth seeing when a drag-to-dock misbehaves.
pub(crate) fn redock_resolution(
    pane: usize,
    target: &str,
    x_rel: f64,
    y_rel: f64,
    resolution: &str,
) {
    tracing::debug!(
        target: "sprag_gui::dock",
        pane, redock_target = target, x_rel, y_rel, resolution,
        "redock_resolution",
    );
}
