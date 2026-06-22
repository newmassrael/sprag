//! Regression guard for the RESIZE-STALE accumulation bug.
//!
//! Symptom (user-reported, live-verified by screen recording): dragging a GUI
//! splitter over a live shell — especially to an extreme-narrow width — leaves
//! the pane stacked with repeated prompt redraws. The accumulation is REAL
//! producer state (`Screen::full_text` grows to dozens of prompts and persists),
//! not just GPU paint: the emulator does not cleanly OVERWRITE bash's multi-row
//! prompt redisplay when readline reflows at a narrow width.
//!
//! This file is the reusable harness ([`bash_session`] / [`settle`] /
//! [`drag_sweep`]) plus four guards driving a real `/bin/bash` over a PTY through
//! a RAPID resize sweep — the headless equivalent of a continuous splitter drag.
//!
//! Two reproduction conditions, both load-bearing (each, alone, hides the bug):
//!   1. A prompt that WRAPS at the swept width — a 1-row prompt redraws trivially
//!      and never accumulates (any wrap triggers it; a non-wrapping sweep stays
//!      clean). With typed INPUT on the line, bash splits the redraw with an
//!      explicit `CR LF` at exact-fill widths — the harder, later-found case.
//!   2. A step gap ABOVE the PTY-resize debounce (R42, ~40ms) so each step
//!      delivers its own `SIGWINCH` (the storm); below it, the coalescer merges
//!      the sweep into one resize and bash redraws once (clean) — which is why the
//!      RACE, not the resize itself, is the trigger.
//!
//! The guards share the rapid storm:
//!
//! * [`rapid_resize_without_wrapping_stays_clean`] — storm above the wrap width
//!   (single-row redraws). GREEN guard: must stay passing (the storm alone does
//!   not accumulate; this catches a gross regression).
//! * [`rapid_extreme_resize_does_not_accumulate_prompts`] — bare prompt, storm to
//!   ~4 columns (heavy multi-row wrap). Fixed by the `Screen::reflowed` cursor
//!   anchor (R45/R46): bash's no-cursor-up redraw assumes the cursor is at the
//!   prompt top, so a reflow that left it at the rewrapped bottom stacked copies.
//! * [`rapid_extreme_resize_with_typed_input_does_not_accumulate`] (ASCII) and
//!   [`rapid_extreme_resize_with_wide_char_input_does_not_accumulate`] (Korean) —
//!   input on the line, the exact-fill `CR LF` case. Fixed by treating that as a
//!   soft wrap during the editor's redraw (`Emulator::in_resize_redraw`).
//!
//! These need a real `/bin/bash`; they are integration tests, not unit tests.
//! (Char-agnostic mechanism precision is pinned by the deterministic unit tests in
//! `sprag-vt`; these are the end-to-end smoke against the real shell.)

use std::time::{Duration, Instant};

use sprag_terminal::{CommandBuilder, TerminalSession};

/// The fixed prompt (PS1) bash redraws on `SIGWINCH`. Deliberately LONG (40
/// cols, like a real `user@host:~$` prompt) so that at an extreme-narrow width it
/// wraps to many rows — the multi-row redisplay the bug mishandles. A 1-row
/// prompt would not trigger it. No PS1-special chars (`$`, `\`, backtick).
const PROMPT: &str = "RZPROMPT-coin-legion-pro-5-16irx9-home> ";
/// What [`prompt_count`] counts: the unique prompt head. The metric is read at
/// the FINAL WIDE width (the sweep returns to 80 cols), where each redraw lands
/// on its own line and the head is unwrapped — so one match per prompt instance.
const MARKER: &str = "RZPROMPT";

/// Spawn an INTERACTIVE bash (readline active, so it redraws `PROMPT` on
/// `SIGWINCH`) with a fixed prompt and no rc/profile noise.
fn bash_session(cols: u16, rows: u16) -> TerminalSession {
    let mut cmd = CommandBuilder::new("/bin/bash");
    cmd.arg("--norc");
    cmd.arg("--noprofile");
    cmd.arg("-i");
    cmd.env("PS1", PROMPT);
    cmd.env("PROMPT_COMMAND", "");
    cmd.env("TERM", "xterm");
    TerminalSession::spawn_with_dirty(cmd, cols, rows, None).expect("spawn bash on a PTY")
}

/// The screen's highest per-row damage stamp — a cheap "did anything change"
/// signal for waiting on bash's asynchronous redraw.
fn max_generation(session: &TerminalSession) -> u64 {
    session.with_screen(|s| {
        (0..s.rows())
            .filter_map(|r| s.row_generation(r))
            .max()
            .unwrap_or(0)
    })
}

/// How many times the prompt appears in the whole pane text (visible + scroll
/// history) — the accumulation metric.
fn prompt_count(session: &TerminalSession) -> usize {
    session.with_screen(|s| s.full_text().matches(MARKER).count())
}

/// Block until the screen's damage stamp holds steady for `quiet` (bash has
/// finished redrawing), or `cap` elapses. Bash's redraw is asynchronous (PTY →
/// reader thread → emulator), so a guard must wait on settle, never a fixed sleep.
fn settle(session: &TerminalSession, quiet: Duration, cap: Duration) {
    let start = Instant::now();
    let mut last = max_generation(session);
    let mut stable_since = Instant::now();
    while start.elapsed() < cap {
        std::thread::sleep(Duration::from_millis(20));
        let now = max_generation(session);
        if now != last {
            last = now;
            stable_since = Instant::now();
        } else if stable_since.elapsed() >= quiet {
            return;
        }
    }
}

/// A continuous drag from `wide` down to `narrow` and back, in 2-column steps.
fn drag_widths(wide: u16, narrow: u16) -> Vec<u16> {
    let down: Vec<u16> = (narrow..=wide).rev().step_by(2).collect();
    let up: Vec<u16> = (narrow..=wide).step_by(2).collect();
    down.into_iter().chain(up).collect()
}

/// Drive a RAPID resize sweep — the headless analogue of a continuous splitter
/// drag: each step pauses only `step_gap` (NOT a full settle), so bash is still
/// mid-redraw when the next width arrives (the race that produces accumulation).
/// `input` (empty for the bare-prompt case) is typed onto the command line first —
/// with INPUT present, bash's `SIGWINCH` redraw moves the cursor up and, at a width
/// the line exactly fills, breaks it with an explicit `CR LF`; mishandling that
/// split stacked per-width copies as ghosts. The fix (`Emulator::in_resize_redraw`)
/// keeps the redraw one logical line that collapses on widen. Returns the prompt
/// count after the sweep settles wide.
fn drag_sweep(
    session: &TerminalSession,
    input: &[u8],
    widths: &[u16],
    rows: u16,
    step_gap: Duration,
) -> usize {
    settle(session, Duration::from_millis(250), Duration::from_secs(3));
    assert!(
        prompt_count(session) >= 1,
        "the bash prompt never appeared — harness/environment problem, not the bug. \
         screen text = {:?}",
        session.with_screen(|s| s.full_text())
    );
    if !input.is_empty() {
        session.write(input).expect("type input into the session");
        settle(session, Duration::from_millis(250), Duration::from_secs(3));
    }
    for &w in widths {
        session.resize(w, rows).expect("resize the session");
        std::thread::sleep(step_gap);
    }
    settle(session, Duration::from_millis(300), Duration::from_secs(3));
    prompt_count(session)
}

/// Regression guard: the rapid extreme storm with SHORT ASCII input on the line.
/// bash's input redraw splits the line with `CR LF` at exact-fill widths; without
/// the resize-redraw soft-wrap fix the per-width copies stacked (≈16-44 prompts).
#[test]
fn rapid_extreme_resize_with_typed_input_does_not_accumulate() {
    let session = bash_session(80, 24);
    let n = drag_sweep(
        &session,
        b"echo hi",
        &drag_widths(80, 4),
        24,
        Duration::from_millis(55),
    );
    assert!(
        n <= 3,
        "a RAPID extreme resize with typed input accumulated {n} prompts \
         (expected <= 3) — the resize-stale bug (typed-input case)"
    );
}

/// Regression guard: the same storm with KOREAN wide-char input (`안녕하세요`) —
/// the user's real input. Wide (2-column) clusters exercise a distinct width path
/// (`char_columns`, wide/trailer cells) through bash's redraw splits; a real-bash
/// smoke that the bound holds for them, complementing the char-agnostic unit tests.
#[test]
fn rapid_extreme_resize_with_wide_char_input_does_not_accumulate() {
    let session = bash_session(80, 24);
    let n = drag_sweep(
        &session,
        "\u{c548}\u{b155}\u{d558}\u{c138}\u{c694}".as_bytes(), // 안녕하세요
        &drag_widths(80, 4),
        24,
        Duration::from_millis(55),
    );
    assert!(
        n <= 3,
        "a RAPID extreme resize with Korean input accumulated {n} prompts \
         (expected <= 3) — the resize-stale bug (wide-char input case)"
    );
}

/// GREEN guard: the SAME rapid storm, but the sweep stays ABOVE the wrap width
/// (narrow bound 50 cols > the ~40-col prompt), so every redraw is single-row and
/// the emulator overwrites it cleanly. This isolates the trigger: the storm ALONE
/// does not accumulate — wrapping does. Must stay passing.
#[test]
fn rapid_resize_without_wrapping_stays_clean() {
    let session = bash_session(80, 24);
    let n = drag_sweep(
        &session,
        b"",
        &drag_widths(80, 50),
        24,
        Duration::from_millis(55),
    );
    assert!(
        n <= 3,
        "a rapid non-wrapping resize accumulated {n} prompts (expected <= 3) — regression"
    );
}

/// Regression guard: the SAME rapid storm taken to an extreme-narrow width, where
/// the prompt wraps to many rows. This reproduced the resize-stale bug — a RAPID
/// drag to an extreme-narrow width (each step its own SIGWINCH) made the emulator
/// stack bash's multi-row prompt redisplay instead of overwriting it, so prompts
/// accumulated (~40+). Fixed by anchoring the reflow cursor to its logical line's
/// first physical row (`Screen::reflowed`), so bash's CR + erase + reprint redraw
/// overwrites the old prompt in place. Kept as a guard against regression.
#[test]
fn rapid_extreme_resize_does_not_accumulate_prompts() {
    let session = bash_session(80, 24);
    let n = drag_sweep(
        &session,
        b"",
        &drag_widths(80, 4),
        24,
        Duration::from_millis(55),
    );
    assert!(
        n <= 3,
        "a RAPID extreme resize accumulated {n} prompts (expected <= 3) — the resize-stale bug"
    );
}
