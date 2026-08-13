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
//! ⚠⚠⚠ **NEITHER ATTRIBUTION IS MEASURED BY THESE GUARDS, AND BOTH WERE CHECKED BY MUTATING.**
//!
//! * R362 ran all four with `in_resize_redraw` forced OFF — the epoch never opening — and **all
//!   four still passed.**
//! * R370b did the same to the OTHER named mechanism, the one the headline above credits: with
//!   `Screen::reflowed`'s cursor anchor removed (the `cphys == cursor_line_top` arm always taken)
//!   **all four still passed** — while `sprag_vt`'s `reflow_anchors_cursor_to_logical_line_top`
//!   and `reflow_rebreaks_a_line_when_narrowed` both went red, so the mutation was plainly
//!   effective and these guards simply cannot see it.
//!
//! **So both mechanisms are held by DETERMINISTIC unit gates in `sprag-vt`, and neither by these**
//! (`resize_redraw_crlf_is_a_soft_wrap` for the soft-wrap epoch;
//! `reflow_anchors_cursor_to_logical_line_top` for the anchor). These four are the end-to-end smoke
//! they were always described as — **they are not the mechanism's proof, and a change to either
//! mechanism cannot be validated by running them.**
//!
//! # ⚠⚠⚠ WHAT THESE GUARDS WERE ACTUALLY ASSERTING, until R370b
//!
//! A TOTAL prompt count against a fixed bound of 3 — and that total carries every prompt the shell
//! has ever printed into the pane, which is the SHELL's business and not this emulator's. Measured
//! on the box where all four read 1: **three bare Enters take the count to 4**, past the bound, on
//! a healthy build; and the sweep then takes it DOWN to 2, because widening rejoins wrapped rows.
//! The number could go over the line without a resize and under it because of one.
//!
//! That is the macOS red this instrument produced twice on code that touched none of it — 4 prompts
//! at R367, 5 at R369, on two different members of the family — against a bound tuned on one shell.
//! macOS's `/bin/bash` is fifteen years older than this box's and its startup and redisplay leave a
//! couple more prompts in history.
//!
//! The guards now ask [`drag_sweep`] for what the SWEEP ADDED (see [`added_ceiling`]), which is
//! what the word *accumulate* in their names has always meant.
//!
//! # ⚠⚠⚠ AND A DELTA AGAINST A CONSTANT STILL COULD NOT ANSWER macOS
//!
//! R371b measured this box: **ZERO added, on all five, at every step gap from 1 ms to 90 ms.** The
//! macOS runner adds **3** on the two 78-step storms, with the identical `PS1`, flags and sweep. No
//! bound can say which of those is a fifteen-year-older bash leaving prompts behind and which is a
//! small real accumulation — **they are the same number.** They are not the same SHAPE, and
//! [`the_storm_does_not_scale_with_its_own_length`] is the guard that asks about the shape.
//!
//! These need a real `/bin/bash`; they are integration tests, not unit tests.
//! (Char-agnostic mechanism precision is pinned by the deterministic unit tests in
//! `sprag-vt`; these are the end-to-end smoke against the real shell.)

use std::time::{Duration, Instant};

use sprag_terminal::{CommandBuilder, PanePty};

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
fn bash_session(cols: u16, rows: u16) -> PanePty {
    let mut cmd = CommandBuilder::new("/bin/bash");
    cmd.arg("--norc");
    cmd.arg("--noprofile");
    cmd.arg("-i");
    cmd.env("PS1", PROMPT);
    cmd.env("PROMPT_COMMAND", "");
    cmd.env("TERM", "xterm");
    PanePty::spawn_with_dirty(
        cmd,
        cols,
        rows,
        sprag_terminal::PaneHooks::default(),
        &[],
        sprag_vt::DEFAULT_SCROLLBACK_LINES,
    )
    .expect("spawn bash on a PTY")
}

/// The screen's highest per-row damage stamp — a cheap "did anything change"
/// signal for waiting on bash's asynchronous redraw.
fn max_generation(session: &PanePty) -> u64 {
    session.with_screen(|s| {
        (0..s.rows())
            .filter_map(|r| s.row_generation(r))
            .max()
            .unwrap_or(0)
    })
}

/// How many times the prompt appears in the whole pane text (visible + scroll history).
///
/// # ⚠⚠⚠ THIS IS A PROPERTY OF THE SESSION, NOT OF THE RESIZE
///
/// It counts every prompt the shell has ever printed into this pane, and the four guards below
/// used to assert it against a fixed bound — which held only because `bash` 5 on the author's box
/// prints exactly ONE prompt for a `--norc -i` session and its redisplay never scrolls a copy into
/// history. Neither is a property of this emulator.
///
/// Measured (R370b, on the box where every guard reads 1): three bare Enters — an ordinary thing
/// for a shell session to have done, with nothing whatever to do with a resize — take the count to
/// **4**, past the old bound of 3, on a healthy build. And the sweep then takes it DOWN to 2,
/// because widening rejoins wrapped rows. **The number these guards were reading could go over the
/// line without a resize and under it because of one.**
///
/// That is the macOS red this instrument produced twice (4 prompts at R367, 5 at R369, on two
/// different members of the family, in commits that touched none of this): a `/bin/bash` fifteen
/// years older than this box's, whose startup and redisplay leave a couple more prompts in history,
/// against a bound tuned on one shell.
///
/// So the guards ask [`drag_sweep`] for what the SWEEP ADDED. See there.
fn prompt_count(session: &PanePty) -> usize {
    session.with_screen(|s| s.full_text().matches(MARKER).count())
}

/// HOW MANY PROMPTS ONE STORM MAY ADD before it is stacking rather than redrawing.
///
/// # Why two, and why a constant here is not the folklore the old bound was
///
/// The old `<= 3` bounded a TOTAL, so it had to hold every prompt the session had ever printed and
/// it could be spent before the storm began. This bounds the DELTA, so the only thing it has to
/// cover is what one sweep emits.
///
/// The defect these guards exist for **stacks one copy per resize step** (measured at ~40+ for the
/// bare prompt and ≈16-44 with input). A shell whose redisplay lands a prompt on a fresh row once
/// or twice during a storm is not stacking; anything that scales with the step count is.
///
/// ⚠ The sweeps are **78 steps** for the three extreme guards and **32** for the non-wrapping one —
/// which this const's first draft got wrong, saying *"~78"* of all four until [`drag_sweep`]'s
/// non-vacuity assert said otherwise. A sixteenth is the margin, and that assert is what stops a
/// later round shortening a sweep until the margin is a number nobody derived.
///
/// # ⚠⚠⚠ R371b: A SIXTEENTH OF **THIS** SWEEP, not of the shortest one applied to all
///
/// The constant was `2` — a sixteenth of the SHORTEST sweep (32), used as the bound for every
/// guard including the 78-step ones. That last step was never argued anywhere; it is the same
/// unstated simplification the TOTAL had before R370b, one level down, and it made the 78-step
/// guards four times stricter than their own derivation asks for.
///
/// macOS met it first: `ADDED 3 (bound 2)` on the two 78-step storms, where this rule gives **4**.
/// ⚠⚠⚠ **THAT IS NOT WHY THE RULE CHANGED, AND THE DIFFERENCE IS NOT EXPLAINED BY IT.** This box
/// adds **ZERO** on all five guards at every step gap from 1 ms to 90 ms (measured, R371b), so a
/// platform answering 3 is doing something this one does not, and a wider bound does not say what.
/// **`the_storm_does_not_scale_with_its_own_length` is what answers that** — it asks whether the
/// additions GROW with the storm, which is the one property that separates a shell's redisplay
/// leaving a few prompts from the defect these guards exist for.
fn added_ceiling(steps: usize) -> usize {
    steps / 16
}

/// Block until the screen's damage stamp holds steady for `quiet` (bash has
/// finished redrawing), or `cap` elapses. Bash's redraw is asynchronous (PTY →
/// reader thread → emulator), so a guard must wait on settle, never a fixed sleep.
fn settle(session: &PanePty, quiet: Duration, cap: Duration) {
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
/// keeps the redraw one logical line that collapses on widen.
///
/// # ⚠⚠⚠ Returns WHAT THE SWEEP ADDED, never the total
///
/// The baseline is taken at the last quiet moment before the first resize — after the prompt is up
/// and after any input is typed — so the answer is the storm's own contribution and nothing else.
/// [`prompt_count`] says why the total was the wrong number: it carries whatever the session had
/// already printed, which is the shell's business and differs by shell.
///
/// ⚠ SATURATING. A sweep that ends with FEWER prompts than it started with has not accumulated —
/// widening rejoins wrapped rows, and the measurement above saw a real delta of −2. Reporting that
/// as `0` is the honest reading of *"how many did this add?"*.
fn drag_sweep(
    session: &PanePty,
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
        session
            .write(input, sprag_terminal::Hand::APerson)
            .expect("type input into the session");
        settle(session, Duration::from_millis(250), Duration::from_secs(3));
    }
    // ⚠⚠ THE LAST QUIET MOMENT BEFORE THE STORM. Taken here rather than at the top because typing
    // is not part of the sweep, and a baseline that predated the input would charge the storm for
    // whatever echoing the input caused.
    let before = prompt_count(session);
    // ⚠⚠ NON-VACUITY: the bound this feeds is justified BY the step count — one copy per step is
    // the defect's signature — so a sweep that quietly got shorter would make the bound generous
    // without anybody noticing. R356's rule, applied to a margin instead of to a ceiling.
    assert!(
        widths.len() >= 30,
        "the storm is {} steps, and [`added_ceiling`] is argued against the two lengths these guards \
         actually drive (78 and 32) — a shorter one makes that margin a number nobody derived",
        widths.len(),
    );
    for &w in widths {
        session.resize(w, rows, (0, 0)).expect("resize the session");
        std::thread::sleep(step_gap);
    }
    settle(session, Duration::from_millis(300), Duration::from_secs(3));
    prompt_count(session).saturating_sub(before)
}

/// Regression guard: the rapid extreme storm with SHORT ASCII input on the line.
/// bash's input redraw splits the line with `CR LF` at exact-fill widths; without
/// the resize-redraw soft-wrap fix the per-width copies stacked (≈16-44 prompts).
#[test]
fn rapid_extreme_resize_with_typed_input_does_not_accumulate() {
    let session = bash_session(80, 24);
    let widths = drag_widths(80, 4);
    let bound = added_ceiling(widths.len());
    let n = drag_sweep(&session, b"echo hi", &widths, 24, Duration::from_millis(55));
    assert!(
        n <= bound,
        "a RAPID extreme resize with typed input ADDED {n} prompts over {} steps \
         (bound {bound}) — the resize-stale bug (typed-input case)",
        widths.len(),
    );
}

/// Regression guard: the same storm with KOREAN wide-char input (`안녕하세요`) —
/// the user's real input. Wide (2-column) clusters exercise a distinct width path
/// (`char_columns`, wide/trailer cells) through bash's redraw splits; a real-bash
/// smoke that the bound holds for them, complementing the char-agnostic unit tests.
#[test]
fn rapid_extreme_resize_with_wide_char_input_does_not_accumulate() {
    let session = bash_session(80, 24);
    let widths = drag_widths(80, 4);
    let bound = added_ceiling(widths.len());
    let n = drag_sweep(
        &session,
        "\u{c548}\u{b155}\u{d558}\u{c138}\u{c694}".as_bytes(), // 안녕하세요
        &widths,
        24,
        Duration::from_millis(55),
    );
    assert!(
        n <= bound,
        "a RAPID extreme resize with Korean input ADDED {n} prompts over {} steps \
         (bound {bound}) — the resize-stale bug (wide-char input case)",
        widths.len(),
    );
}

/// ⚠⚠⚠ **A SESSION THAT HAS ALREADY PRINTED PROMPTS IS NOT AN ACCUMULATION** — the control this
/// family never had, and the shape that produced two macOS reds.
///
/// Every guard here reads 1 on this box because a `--norc -i` bash 5 prints exactly ONE prompt and
/// its redisplay never scrolls a copy into history. Neither is a property of this emulator, and a
/// bound over the TOTAL could be spent before the storm ever began: three bare Enters — the most
/// ordinary thing a shell session can do — put four prompts in the pane, and the old `<= 3` failed
/// there on a healthy build.
///
/// So this drives the identical extreme storm on a pane that has ALREADY been made to print
/// several prompts, and requires the same answer. What it asserts is that the metric belongs to the
/// RESIZE.
///
/// ⚠ It also asserts the baseline it created, because a control whose setup silently stopped
/// working would pass by measuring the plain case twice.
///
/// ⚠⚠⚠ REVERT-PROOF, AND IT IS THE WHOLE FINDING: make [`drag_sweep`] return the TOTAL again and
/// this answers **5** against the family's old bound of 3 — the exact number the macOS runner
/// reported, reproduced on Linux, on a healthy build, by a session that had pressed Enter.
///
/// ⚠ The setup's size is measured rather than chosen: with THREE Enters the total after the sweep
/// falls to 2 and the reverted metric passes, because widening rejoins wrapped rows and the
/// scrollback loses the rest. A control has to leave enough history to survive its own storm, and
/// the first draft of this one did not — it read green under the mutation it was written for.
#[test]
fn prompts_a_session_printed_before_the_storm_are_not_charged_to_it() {
    let session = bash_session(80, 24);
    settle(&session, Duration::from_millis(250), Duration::from_secs(3));
    for _ in 0..12 {
        session
            .write(b"\n", sprag_terminal::Hand::APerson)
            .expect("press Enter into the session");
        settle(
            &session,
            Duration::from_millis(60),
            Duration::from_millis(600),
        );
    }
    settle(&session, Duration::from_millis(250), Duration::from_secs(3));
    let history = prompt_count(&session);
    assert!(
        history > 3,
        "the setup must leave MORE prompts in this pane than the family's old bound, or this \
         control is the plain case wearing a different name: {history}",
    );

    let widths = drag_widths(80, 4);
    let bound = added_ceiling(widths.len());
    let added = drag_sweep(&session, b"", &widths, 24, Duration::from_millis(55));
    assert!(
        added <= bound,
        "⚠⚠⚠ the storm ADDED {added} prompts (bound {bound}) to a pane that already held \
         {history} — a guard reading the TOTAL here answers FIVE, which is the number the macOS \
         runner reported, on a session whose only crime was pressing Enter",
    );
}

/// GREEN guard: the SAME rapid storm, but the sweep stays ABOVE the wrap width
/// (narrow bound 50 cols > the ~40-col prompt), so every redraw is single-row and
/// the emulator overwrites it cleanly. This isolates the trigger: the storm ALONE
/// does not accumulate — wrapping does. Must stay passing.
#[test]
fn rapid_resize_without_wrapping_stays_clean() {
    let session = bash_session(80, 24);
    let widths = drag_widths(80, 50);
    let bound = added_ceiling(widths.len());
    let n = drag_sweep(&session, b"", &widths, 24, Duration::from_millis(55));
    assert!(
        n <= bound,
        "a rapid non-wrapping resize ADDED {n} prompts over {} steps (bound {bound}) — regression",
        widths.len(),
    );
}

/// HOW MUCH a doubled storm may add over the single one before it is SCALING rather than settling.
///
/// Two, and it is measurement noise and nothing else: this box adds ZERO at both lengths, and the
/// defect adds one per step — so anything between those is a shell landing a prompt on a fresh row
/// once or twice more when it is driven twice as long.
const SCALE_SLACK: usize = 2;

/// ⚠⚠⚠ **THE STORM'S CONTRIBUTION DOES NOT GROW WITH THE STORM** — the property every guard above
/// is named for (*"does not accumulate"*) and the only one that separates the two regimes without
/// a constant tuned to somebody's shell.
///
/// # ⚠⚠⚠ Why a constant could never answer the question macOS asked
///
/// Every bound above is *"how many is too many"*, and that number is not a property of this
/// emulator: this box adds **0** on all five guards at every step gap from 1 ms to 90 ms, and the
/// macOS runner adds **3** on two of them with the identical `PS1`, the identical flags and the
/// identical sweep. Nothing in a bound can say whether that 3 is a fifteen-year-older bash leaving
/// a couple of prompts behind or a small real accumulation — **the two are the same number.**
///
/// They are not the same SHAPE. The defect these guards exist for stacks **one copy per resize
/// step** (measured ~40+ bare, ≈16-44 with input), so it grows with the sweep; a shell's redisplay
/// leaving a few prompts does not. So this drives the SAME storm twice, once at double the length,
/// and asks whether the additions followed. **A platform can answer that about itself**, which is
/// what makes this the guard that reports on macOS rather than the one that gets tuned for it.
///
/// ⚠ Under the defect the arithmetic is not close: ~39 additions at 78 steps against ~78 at 156
/// fails `78 <= 39 + 2` by a factor of two, and it fails harder the longer the sweep.
///
/// ⚠⚠ NON-VACUITY, twice over, because on a healthy box both answers are zero and `0 <= 0 + 2`
/// would pass with the instrument unplugged: the long sweep must really be about twice the short
/// one, and `drag_sweep`'s own 30-step floor still applies to each.
#[test]
fn the_storm_does_not_scale_with_its_own_length() {
    let short = drag_widths(80, 4);
    // ⚠ Built by SWEEPING TWICE rather than by halving the step, so the two storms differ in
    // LENGTH and in nothing else. A finer step would change the width path each redraw takes,
    // which is a different experiment wearing this one's name.
    let long: Vec<u16> = short.iter().chain(short.iter()).copied().collect();
    assert!(
        long.len() >= short.len() * 2 && short.len() >= 30,
        "the two storms must differ by a real factor or this compares one length with itself: \
         {} vs {}",
        short.len(),
        long.len(),
    );

    let added_short = drag_sweep(
        &bash_session(80, 24),
        b"",
        &short,
        24,
        Duration::from_millis(55),
    );
    let added_long = drag_sweep(
        &bash_session(80, 24),
        b"",
        &long,
        24,
        Duration::from_millis(55),
    );

    assert!(
        added_long <= added_short + SCALE_SLACK,
        "⚠⚠⚠ THE STORM'S CONTRIBUTION SCALED WITH THE STORM: {} steps ADDED {added_short}, and \
         {} steps ADDED {added_long}. That is the shape of the resize-stale bug — one copy per \
         resize step — and it is the reading a fixed bound cannot distinguish from a shell that \
         simply leaves a prompt or two behind",
        short.len(),
        long.len(),
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
    let widths = drag_widths(80, 4);
    let bound = added_ceiling(widths.len());
    let added = drag_sweep(&session, b"", &widths, 24, Duration::from_millis(55));
    assert!(
        added <= bound,
        "a RAPID extreme resize ADDED {added} prompts over {} steps (bound {bound}) — \
         the resize-stale bug",
        widths.len(),
    );
}
