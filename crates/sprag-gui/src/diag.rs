//! `SPRAG_GUI_LOG`-gated diagnostic trace for the interactive GUI's input + dock
//! dispatch — the STANDING observability the layout path was missing.
//!
//! Always compiled in; each event writes ONE line to stderr, but only when the
//! `SPRAG_GUI_LOG` env var is set to a non-empty value (mirrors the existing
//! `SPRAG_GUI_*` knob family — [`pane_count`](crate::terminal::pane_count) etc.).
//! Off by default, so a normal run is silent; a single `SPRAG_GUI_LOG=1` makes
//! the dispatch path self-describing.
//!
//! The point is that a live misbehaviour — a chord firing more than once per
//! physical press, a toggle landing on the wrong pane, a window appearing/
//! vanishing unexpectedly — is read STRAIGHT from the log, never re-instrumented
//! by hand each session (the anti-pattern the R64/R65 double-toggle debugging
//! kept hitting). One physical key press that fans out to N dispatches reads as N
//! [`key_in`] lines, and the millisecond stamps expose the ~32 ms gaps between
//! deliveries that distinguish an OS auto-repeat from a multi-window re-delivery.
//!
//! Events are TYPED (named functions, not free-form `eprintln!`) so the wire
//! stays greppable and stable: `key_in` (a dispatch entered the binding),
//! `chord` (`route_key`'s decision for a recognised window chord), `dock_toggle`
//! (the topology actually mutated). The enabled-check gates every write, so a
//! disabled run pays only a cached atomic load per event.

use std::sync::OnceLock;
use std::time::Instant;

/// `true` once `SPRAG_GUI_LOG` is observed non-empty (read once, then cached).
fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("SPRAG_GUI_LOG").is_some_and(|v| !v.is_empty()))
}

/// Milliseconds since the first diagnostic call — a process-relative clock so the
/// per-dispatch fan-out of one press reads at a glance (no wall-clock noise).
fn stamp() -> u128 {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    ORIGIN.get_or_init(Instant::now).elapsed().as_millis()
}

/// A key dispatch ENTERED the binding — one line per `apply_key*` call. `entry`
/// names the path (`apply_key` = synthetic / RPC, `apply_key_repeat` = the live
/// shell), so one physical press fanning out to several dispatches reads as
/// several lines with their inter-arrival gaps.
pub(crate) fn key_in(
    entry: &str,
    key: &str,
    ctrl: bool,
    shift: bool,
    repeat: bool,
    focused: Option<&str>,
) {
    if enabled() {
        eprintln!(
            "sprag t={t:>7}ms key_in [{entry}] key={key} ctrl={ctrl} shift={shift} repeat={repeat} focused={focused:?}",
            t = stamp(),
        );
    }
}

/// `route_key`'s decision for a recognised window chord: `action` is `"act"` (it
/// ran) or `"drop-repeat"` (an OS auto-repeat of a discrete chord, suppressed).
/// Distinguishes "the dispatch arrived but was correctly dropped" from "it acted".
pub(crate) fn chord(name: &str, action: &str, pane: usize) {
    if enabled() {
        eprintln!(
            "sprag t={t:>7}ms chord {name} -> {action} (pane {pane})",
            t = stamp()
        );
    }
}

/// A dock toggle MUTATED the topology: direction + window count before/after, so
/// a stray extra toggle (the count jumping by more than one per press, or moving
/// the wrong way) is obvious.
pub(crate) fn dock_toggle(pane: usize, undock: bool, before: usize, after: usize) {
    if enabled() {
        eprintln!(
            "sprag t={t:>7}ms dock {dir} pane {pane}: windows {before} -> {after}",
            t = stamp(),
            dir = if undock { "undock" } else { "dock" },
        );
    }
}
