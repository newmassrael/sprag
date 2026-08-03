//! What the grid path costs in TIME — the number three rounds of counting deliberately refused to
//! claim.
//!
//! R218 gated the projection on the request's method, R219 made a projected cell borrow its
//! cluster, and R220 taught a client to refetch only the panes that changed. All three were priced
//! in COUNTS — projections, cells, allocations — because a count is reproducible on a machine
//! somebody else is also using, and each round closed by saying plainly that what any of it is
//! worth in TIME was unmeasured. This is that instrument.
//!
//! ## Why this is a tool and not a test
//!
//! A count can be asserted; a duration cannot. `tests/meter.rs` and `tests/allocs.rs` are gates
//! precisely because their numbers do not depend on what else the box is doing, and a threshold in
//! microseconds would be a flake by construction — green on a quiet machine, red on a busy one, and
//! evidence of nothing either way. So a timing lives here, as a tool that REPORTS with its
//! operating point attached, rather than in `tests/`, where it would eventually be silenced.
//!
//! ## Why the MINIMUM is the number that repeats
//!
//! This machine is an i9-14900HX: performance and efficiency cores in one package, under a
//! powersave governor that ranges from 0.8 to 5.8 GHz. What a sample costs therefore depends on
//! which kind of core it landed on and what the clock was doing at the time — a BIMODAL
//! distribution, not noise scattered around a mean, and no amount of averaging collapses it. The
//! minimum does not have that problem: it converges on one well-defined operating point (the
//! fastest core the scheduler offered, warm, unpreempted), and that point is the same one on the
//! next run. It is the honest floor — the cost of the code itself, with the machine's interference
//! removed. The median is printed beside it as what this box ACTUALLY did over the battery, and it
//! is explicitly not the reproducible half: read it as the interference, not as the code.
//!
//! ## Why a control, measured in the same breath — and what it can and cannot do
//!
//! An absolute microsecond is only as portable as the machine that produced it. So every row is
//! preceded by its OWN measurement of a fixed workload this crate does not own — a 64 KiB copy —
//! taken with the same estimator and reported as a ratio against it. The control's spread over the
//! battery is printed at the end: this run's disclosure of how quiet the box was, taken from the
//! run rather than claimed about it.
//!
//! What that ratio is worth was measured, in both directions, rather than assumed. Over ten runs of
//! this battery the absolute minima moved 26% to 50%; as ratios, nine of the eleven rows moved only
//! 5% to 16%. The two exceptions are the whole-request rows (30% and 51%) — the only rows served
//! while three live PTY panes and their reader threads share the process, which is interference the
//! control cannot see and so cannot cancel.
//!
//! An allocation-heavy control was tried for the reply-building rows, on the reasoning that a
//! denominator should share its subject's bottleneck and those rows are almost entirely allocator.
//! It made them WORSE: the DOM build went from 16% spread to 53%, the cells fetch from 30% to 68%.
//! The reason is arithmetic rather than tuning — dividing by a denominator noisier than the
//! numerator adds variance, and allocation is the noisiest thing this box does. So the rule is
//! sharper than the one this was built for: A CONTROL CANCELS ONLY THE NOISE IT SHARES, AND ONLY
//! WHILE IT IS STEADIER THAN ITS SUBJECT. There is one control, and it is the steady one.
//!
//! ## What one run of this is worth
//!
//! Not as much as its decimal places suggest, and the tool says so rather than letting a reader
//! assume otherwise. WITHIN a run the estimator is very tight — the median lands within a fraction
//! of a percent of the minimum. ACROSS runs the same code moves by 20% to 30% (more when the box is
//! busy), and no amount of sampling inside a run reduces that, because whatever causes it is fixed
//! for a process and varies between processes. Pinning to the fastest core was tried and made it
//! WORSE (85% spread), so it is not core assignment; that hypothesis is recorded here as refuted
//! rather than quietly dropped.
//!
//! The practical rule: one run is not a measurement. Run it several times and quote the RANGE. Two
//! significant figures is the most any number below can support — which is exactly why the guards
//! in `tests/` are counts and this is a report.
//!
//! ## Why a subject is repeated inside the timer
//!
//! The first draft timed one call per sample and reported `projection_token` at 0.02 us — which is
//! BELOW `Instant`'s own floor on this box, so the number was the clock's resolution wearing the
//! subject's name. A measurement instrument is a hypothesis like any other, and that one was
//! refuted by its own output. Each subject is now piloted first and then repeated inside the timed
//! span enough times to make the span large against the floor, and the floor is printed at the top
//! of every run so no row can quietly claim to be faster than the thing measuring it. The
//! repetition count is a column, because a reader should not have to trust that it was chosen well.
//!
//! ## What the H3 rows measured, and where the question had to be asked from
//!
//! H3's design left the agent evaluation's cost unmeasured through four slices, and R254 recorded
//! it as owed twice over: once because the evaluation now sits on the pane list, and once because
//! putting the rule list into the quiescence gate's key destroyed the only behavioural test the
//! skip had. The rows exist to answer both. Measured on this box (i9-14900HX, powersave governor),
//! `--release`, ten runs of this battery — with the profile and the machine stated because a
//! duration that leaves them keeps its authority and loses its meaning:
//!
//! * **The comparison the debt asked for cannot be resolved at the level it asked for it.** The
//!   pane list with and without the detector differed by +0.07 to +4.22 us over ten runs of a
//!   request that costs 7.4 to 8.7 us. Every run was positive, so the SIGN is settled; the
//!   magnitude spans sixty-fold, so the size is not. Those two rows are the ones R221 already
//!   recorded as the noisiest in this battery — the only ones served while three live PTY panes
//!   and their reader threads share the process, which is interference a 64 KiB copy cannot cancel.
//! * **So the number comes from a row with no PTY in it.** One `AgentClock::observe` on a settled
//!   pane costs 0.056 to 0.086 us with 1 to 8 panes remembered. That is the DETECTOR's part of a
//!   pane's entry; the title clone and the screen lock the pane list also performs per pane are not
//!   in it, and this instrument has not separated them.
//! * **The gate is worth 3040x to 3605x what it skips**, and that ratio held to within 8% across
//!   ten runs while its two absolutes moved by 20% — a look at a quiet pane costs 0.014 to 0.023 us
//!   and the evaluation it declines to run costs 42.6 to 52.0 us. One evaluation is 11.1x to 12.8x
//!   the whole pane-list request that would carry it, which is the opposite shape from R221's
//!   projection at half a percent of its own fetch. The gate is not an optimisation on this path.
//! * **A per-look cost grew LINEARLY with the number of panes the registry remembers**, at 2.70 to
//!   3.35 ns each — 3.98x to 4.32x across a 64-fold span. `AgentClock::observe` read the nearest
//!   deadline before and after every look and each read walked every tracker, so a pane list over N
//!   panes performed 2N^2 tracker visits. The middle registry size is a control against the other
//!   explanation — a hash lookup losing its cache in a bigger map would step rather than climb — and
//!   it landed within 3% of the straight line through the other two, in every run.
//! * **R256 removed it**, and the same rows measured again over ten runs say 0.85x to 1.16x, with a
//!   slope of -0.222 to +0.175 ns per remembered pane: flat, straddling zero. A look at a registry of
//!   sixty-four went from 0.226-0.275 us to 0.079-0.092 us. THE TRADE IS MEASURED AND IS NOT FREE:
//!   two hash lookups cost about 20 ns more than two walks of a ONE-entry registry, so the row at a
//!   single remembered pane moved from 0.056-0.065 us to 0.068-0.095 us and the crossover is near
//!   eight panes. A bounded constant against an unbounded term, and the eight-pane rows are
//!   indistinguishable before and after, which is what a crossover there predicts.
//!
//! ## What the SWEEP rows measured (R260), and the paragraph they corrected
//!
//! Everything above is paid when a client asks. The settle waker's sweep is paid whether or not
//! anyone asks — one pass every five seconds for the life of the daemon — and it had never been
//! measured through four slices and three rounds that each recorded it as owed. Five runs, minima,
//! same box and profile:
//!
//! * **One sweep over a quiet workspace is 1.96-2.33 us at one pane, 2.50-2.95 us at eight, and
//!   7.01-8.38 us at sixty-four**, composed from its terms rather than measured whole. Against the
//!   five-second period that is 0.00014% of one core at the top end. Nothing here asks to be
//!   changed, which is a result and not a reason the measurement was unnecessary — the shape did.
//! * **At one pane, 94% of a sweep is a config-file read that no version of the cost argument
//!   named**, and at sixty-four it is still 26%. R254 put the manifest reload on this thread and
//!   priced its SCHEDULING (no new thread, no timer, no wake), which is true and is a different
//!   claim; the sweep's own cost paragraph priced the WORK, was written a slice earlier, and priced
//!   the term that scales instead of the term that dominates. The saver it compared itself against
//!   reads no file at all.
//! * **The per-pane question is flat**: 0.039-0.043 us to ask whether a pane owes an evaluation,
//!   -0.02 to +0.06 ns per remembered pane across a 64x span. Three hash lookups under one lock,
//!   and the lock is taken once per PANE — which the old paragraph's "a pane-id read each" did not
//!   say.
//! * **The walk is metered, not inferred, and the count was exact in all five runs**: every
//!   `deadline_visits_total` in the block is accounted for by `any_due`'s calls times its registry
//!   size, with nothing left over. A duration can show that the per-pane question is cheap; only
//!   the counter shows it walks NOTHING. Per visit it is about 1.0 ns, the same order as the
//!   1.35-1.68 ns R255 inferred from a different row.
//! * **The census is not the free by-product `retain_live`'s docs call it** — 2.9x to 3.0x the
//!   prune it exists to serve. Free in the sense meant (it needs no walk of its own), not in the
//!   sense the word carries.
//!
//! ## What the CONTENTION rows measured (R261), and the two answers that were wrong first
//!
//! R260 named one term it would not claim: what the sweep's locks cost, because a single-threaded
//! instrument acquires a lock uncontended and uncontended is not what a lock costs. What a lock
//! costs is the wait it inflicts, so these rows time a pane-list READER while a second thread runs
//! the real `sweep_once` against the same registry. Five runs:
//!
//! * **The recurring pass is free.** Shared minus a control sweeping a PRIVATE registry at the same
//!   rate: +0.4 to +0.8 us on the reader's median and -2.4 to +0.9 us at p99 — at seven to twelve
//!   MILLION times the daemon's real duty cycle. The same differences read -1.4 to +5.9 and -1.6 to
//!   +6.4 while another project was building, which is the paired design surviving a 2x box.
//! * **A churning pass is about 100x a quiet one** (44-58 us for three panes against 0.37-0.58 us),
//!   because the workspace lock is held across an evaluation per pane.
//! * **But the churning DIFFERENCE is not a lock cost and cannot be made into one.** The reader runs
//!   the same detector under the same clock, so sharing the registry changes who evaluates, not only
//!   who waits. Bounded directly by the pass's own duration instead.
//!
//! Two wrong answers came first, and both were plausible. **A single condition with no control said
//! 10x on the median** — mostly the reader's own re-evaluation. **A control that was not matched said
//! +160 to +237 us** — one background loop reloaded one clock and the other reloaded two, so they
//! churned at different rates and the difference was churn, not sharing. What caught the second was
//! not care: it was printing the sweeper's own pass and evaluation COUNTS beside the latencies, at
//! which point a condition claiming every pane owed an evaluation was visibly evaluating three panes
//! in two thousand passes. **A probe needs a control on the probe.**
//!
//! ## What the SOCKET costs (R262), and what it does to every other number here
//!
//! Every row in this tool is served in-process through [`handle_request`], and that bound has been
//! stated here since the tool was written without ever being priced. It is now. A real `sprag-term`
//! is spawned with the same panes and the same geometry, driven over its Unix socket with the same
//! request texts, and a client's wall clock is decomposed into the three things it is: the daemon's
//! own work (already measured), the client's PARSE of the reply, and the transport left over.
//! Four runs:
//!
//! * **A round trip is 22 to 35 us of FIXED cost** — for a `scene/revision` whose whole answer is
//!   fourteen bytes and whose in-process cost is about ONE microsecond. **The wire is roughly 30x
//!   the daemon's work for a small request.**
//! * A 912-byte pane list is 43 to 74 us of transport, so the size term is 15 to 45 ns per byte,
//!   **some fifty times slower than this box copies memory** — it is per-message handling on both
//!   ends, not bandwidth.
//! * **This is the proportion every other figure here has to be read in.** The projection at 0.18%
//!   of a frame, the agent gate at 3000x what it skips, the sweep at microseconds per five seconds:
//!   all of them are changes to a term that is a fraction of the round trip carrying it. What that
//!   argues for is what this project already built — the fetch gate (R220) and `waitFor` over
//!   polling both cut the NUMBER of round trips, and neither could have cut the cost of one.
//!
//! The two hosts cannot be the same process, so their equivalence is checked rather than assumed:
//! the reply's `result` is sized on both sides and printed. `scene/revision` matches to the byte.
//! The pane list does not — the two hosts' pane LABELS differ — and that residue is priced with the
//! tool's own size slope instead of being waved off: about 112 bytes, one to five microseconds,
//! inside the run-to-run band. The first version of that check compared a payload against a JSON-RPC
//! envelope and flagged both rows as answering different questions, on 34 bytes of `{"jsonrpc"...}`.
//!
//! ## What is deliberately NOT here
//!
//! Hardware counters. Retired instructions would be near-deterministic and gate-able, but this box
//! reports `kernel.perf_event_paranoid = 4`, so `perf_event_open` is unavailable without an
//! operator changing a sysctl — and, more to the point, a count of instructions is a proxy for
//! time, not time. The question this round owes an answer to is a latency budget.
//!
//! The socket WAS also not here, and now is — see the section above. Every row below the socket
//! ones is still served IN-PROCESS through [`handle_request`], the same entry point the transport
//! calls, so each of them is the daemon's share alone; what the wire adds on top is measured
//! separately rather than folded in, because they are two different questions and only one of them
//! is about sprag's own code.
//!
//! ## Running it
//!
//! ```text
//! cargo run --release -p sprag-host --bin sprag-latency
//! ```
//!
//! It refuses to run without `--release`. A debug build's timings are the wrong code by an order
//! of magnitude, and publishing them as a budget would be worse than having no budget.

// A binary crate has no public API, so every item this file links to is "private" by rustdoc's
// definition and the workspace's strict `-D rustdoc::private_intra_doc_links` fires on links that
// are perfectly correct. The same allow sits at the root of the crate's other two binaries.
#![allow(rustdoc::private_intra_doc_links)]

use std::collections::HashSet;
use std::hint::black_box;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::json;
use sprag_detect::{DEFAULT_SETTLE, Hysteresis, Ruleset, Tracker, built_ins, detect};
use sprag_grid::{project, projection_token};
use sprag_host::agent::SWEEP_INTERVAL;
use sprag_host::config::AgentManifests;
use sprag_host::wire::SESSION_ACTIVITY_DISPLAY_MAX_AGE;
use sprag_host::{
    AgentClock, CellFrame, ChannelRegistry, Host, HostState, JobWatch, PaneScrollFacts,
    handle_request, mux_action_path, sweep_once,
};
use sprag_terminal::{CommandBuilder, PaneId};
use sprag_vt::{Emulator, Palette, Screen, VtPort};

/// The pane geometry everything below is measured at — an ordinary terminal, so the numbers are
/// about a pane somebody would actually have rather than a benchmark's convenient size.
const COLS: u16 = 80;
/// Rows to match [`COLS`].
const ROWS: u16 = 24;

/// How many panes the request path is measured over. Three is the shape R217 through R220 used, so
/// the fan-out arithmetic at the end lines up with the counts those rounds recorded.
const PANE_COUNT: usize = 3;

/// Samples per subject. Odd, so the median is a sample the machine actually produced rather than
/// the average of the two it straddles.
const SAMPLES: usize = 101;

/// Calls run and discarded before each subject, so a lazily-initialised anything, a cold cache or a
/// not-yet-boosted clock is charged to none of them.
const WARMUP: usize = 16;

/// Calls used to estimate a subject's cost before choosing how many to put inside the timer.
const PILOT: u32 = 8;

/// How long one timed span should last. Three orders of magnitude above `Instant`'s floor on this
/// box, so the floor contributes a rounding error rather than a result.
const SPAN_TARGET: Duration = Duration::from_micros(50);

/// A ceiling on the repetition count, so a subject that measures as free cannot spin for minutes.
const MAX_REPEATS: u32 = 100_000;

/// The control's working set, in `u64`s: 64 KiB, the same order as the cell buffer a projection
/// writes, so the two ask comparable things of the memory system.
const CONTROL_WORDS: u64 = 8 * 1024;

/// One frame at 60 Hz — the budget every duration here is reported as a fraction of, because
/// "microseconds" answers nothing on its own and "of a frame" is the question a display client's
/// poll wake is actually asking.
const FRAME: Duration = Duration::from_nanos(16_666_667);

/// How long a spawned pane is given to paint its screenful before this gives up. Generous: it is a
/// startup wait, not part of any measurement.
const CONTENT_TIMEOUT: Duration = Duration::from_secs(10);

/// The string a pane prints last, so "the screen is ready" is a fact this can read rather than a
/// sleep it has to guess the length of.
const MARKER: &str = "sprag-latency-ready";

/// A read of one integer that walks no node — the request R217 found was costing a whole pane set.
const REVISION: &str = r#"{"jsonrpc":"2.0","id":1,"method":"scene/revision","params":{}}"#;

/// The pane list a display client reads on every wake, and the slot R220's `ProjectionToken` rides
/// on — so this row prices the fetch gate's input, tokens included.
const PANES_READ: &str = r#"{"jsonrpc":"2.0","id":2,"method":"scene/query","params":{"path":"/sprag_mux/external/panes"}}"#;

/// The registry-wide session list a display client re-reads on every poll wake — the question R281
/// found was answering a yes/no by walking `/proc`, and the subject of the pair of rows below.
const SESSIONS_READ: &str = r#"{"jsonrpc":"2.0","id":10,"method":"scene/query","params":{"path":"/sprag_mux/external/sessions"}}"#;

/// A read of every session's SAMPLED activity at tolerance `max_age_ms` — the address R282 moved
/// the `/proc` walk onto, so that asking where sessions are working is a different question from
/// asking what they are called.
fn activity_read(max_age_ms: u64) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":11,"method":"scene/query","params":{{"path":"/sprag_mux/external/{}"}}}}"#,
        sprag_host::wire::session_activity_at(max_age_ms),
    )
}

/// A read of every pane's SAMPLED processes at tolerance `max_age_ms` — R290's address, and the
/// subject of the pair of rows that says what a full `/proc` pass costs when it is asked for.
fn processes_read(max_age_ms: u64) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":12,"method":"scene/query","params":{{"path":"/sprag_mux/external/{}"}}}}"#,
        sprag_host::wire::pane_processes_at(max_age_ms),
    )
}

/// One pane's cells: the client's steady-state fetch, and the unit R220 skips.
const CELLS_READ: &str = r#"{"jsonrpc":"2.0","id":3,"method":"scene/query","params":{"path":"/pane_0/sprag_input/external/cells.0"}}"#;

/// The whole tree — the one method that can reach a `TextGrid`, and so the one that must still pay
/// for every pane.
const SNAPSHOT: &str = r#"{"jsonrpc":"2.0","id":4,"method":"scene/snapshot","params":{"path":""}}"#;

/// A `claude` pane at rest, as its footer fingerprint sees it — the FIRST manifest in the built-in
/// list, so identifying it stops after one.
const CLAUDE_FOOTER: &[&str] = &["❯", "  ⏸ manual mode on · ? for shortcuts"];

/// The title that goes with it. Present because the rules read the title as well as the screen, and
/// a row measured without one would price a cheaper evaluation than the daemon ever performs.
const CLAUDE_TITLE: &str = "✳ Claude Code";

/// An ordinary shell pane. Nothing claims it, so every manifest is offered it — which is the case
/// EVERY non-agent pane in a workspace pays, and so the one an honest hot-path row is measured at.
const SHELL: &[&str] = &["$ ls -la", "total 0", "$ "];

/// How many panes each agent registry in the scaling row remembers.
///
/// Three sizes rather than two, and the middle one is the CONTROL. Two points can only say that a
/// per-look cost grew; they cannot say what grew it, and when this row was written there were two
/// candidates in `observe` — the nearest-deadline reads, which walk every tracker by construction,
/// and the hash lookup's locality in a larger map. A cost linear in the entry count is the walk; one
/// that steps and then flattens is the cache. R255 measured it linear at 2.70 to 3.35 ns per
/// remembered pane and R256 removed the walk from that path, so the row is now a REGRESSION check:
/// the number it reports should be flat, and `sprag-host/tests/agent_cost.rs` is the half that goes
/// red if it stops being. The span is wide (64x) because a ratio between 1 and 4 would be inside
/// this box's own noise, and the top end is still an ordinary number of panes for somebody who
/// leaves a session running.
const REGISTRY_SIZES: [u64; 3] = [1, 8, 64];

/// What one subject cost, reduced to the numbers that mean different things.
#[derive(Clone, Copy)]
struct Sample {
    /// The least-interfered sample — the code's own cost, and the half that repeats.
    min: Duration,
    /// The middle sample — what the box did this run, interference included.
    median: Duration,
    /// How many calls shared one timed span.
    repeats: u32,
}

/// A duration in microseconds.
fn micros(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1e6
}

/// How many times [`measure`] called a subject's body: the warmup, the pilot, and every timed span.
///
/// Exact rather than approximate, because it is the denominator of the COUNTS this tool reports
/// beside its durations — and a count divided by an estimate of how much work produced it is an
/// estimate.
const fn calls(sample: &Sample) -> u64 {
    WARMUP as u64 + PILOT as u64 + SAMPLES as u64 * sample.repeats as u64
}

/// The smallest interval this box can observe: two reads of the clock with nothing between them.
/// Printed, so every row below can be read against the resolution that produced it.
fn timer_floor() -> Duration {
    let mut samples = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let start = Instant::now();
        samples.push(start.elapsed());
    }
    samples.sort_unstable();
    samples[0]
}

/// Time `body` over [`SAMPLES`] spans, each holding as many calls as it takes to make the span
/// large against the clock's floor, and reduce the run to its minimum and median PER CALL.
fn measure(mut body: impl FnMut()) -> Sample {
    for _ in 0..WARMUP {
        body();
    }

    // Pilot: how long one call takes, roughly, so the span below is chosen from evidence rather
    // than from a constant that would be wrong for some subject in the battery.
    let pilot = Instant::now();
    for _ in 0..PILOT {
        body();
    }
    let per_call = pilot.elapsed() / PILOT;
    let repeats = u32::try_from(
        SPAN_TARGET
            .as_nanos()
            .checked_div(per_call.as_nanos())
            .unwrap_or(u128::from(MAX_REPEATS)),
    )
    .unwrap_or(MAX_REPEATS)
    .clamp(1, MAX_REPEATS);

    let mut samples = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let start = Instant::now();
        for _ in 0..repeats {
            body();
        }
        samples.push(start.elapsed() / repeats);
    }
    samples.sort_unstable();
    Sample {
        min: samples[0],
        median: samples[SAMPLES / 2],
        repeats,
    }
}

/// The fixed workload every row is reported against: copy 64 KiB. It touches nothing this
/// repository owns, so a change in sprag can move a subject's ratio and can never move its
/// denominator, and it is the steadiest thing in the battery — which is the property that makes a
/// denominator useful at all.
fn control() -> Sample {
    let source: Vec<u64> = (0..CONTROL_WORDS).collect();
    measure(|| {
        black_box(black_box(&source).clone());
    })
}

/// Measure `body` immediately after its own control, print the row, and hand back the sample — so
/// a caller can do arithmetic on what was measured instead of on what was printed.
///
/// `bytes` is the reply or encoding this subject produces, where it produces one: the cost of the
/// rows below tracks bytes far more closely than it tracks cells, which is the round's finding and
/// so belongs in the table rather than only in the prose.
fn paired(
    name: &str,
    controls: &mut Vec<Duration>,
    bytes: Option<usize>,
    body: impl FnMut(),
) -> Sample {
    let control = control();
    controls.push(control.min);
    let subject = measure(body);
    let bytes = bytes.map_or_else(|| "-".to_string(), |count| count.to_string());
    println!(
        "{name:<38} {:>10.3} {:>10.3} {:>9.2} {:>7} {:>8} {:>8.3}",
        micros(subject.min),
        micros(subject.median),
        subject.min.as_secs_f64() / control.min.as_secs_f64(),
        subject.repeats,
        bytes,
        subject.min.as_secs_f64() / FRAME.as_secs_f64() * 100.0,
    );
    subject
}

/// A screen of `COLS` x `ROWS` filled with `fill`, built outside every measured window.
fn filled(fill: &str) -> Screen {
    let mut emulator = Emulator::new(COLS, ROWS);
    let mut line = String::new();
    while line.chars().count() < usize::from(COLS) {
        line.push_str(fill);
    }
    for row in 0..ROWS {
        emulator.advance(line.as_bytes());
        if row + 1 < ROWS {
            emulator.advance(b"\r\n");
        }
    }
    emulator.screen().clone()
}

/// A screen with `lines` painted into it top-down — the shape the detector's own fixtures use, so a
/// row measured here is measured against the same screens its tests assert on.
fn painted_screen(lines: &[&str]) -> Screen {
    let mut emulator = Emulator::new(COLS, ROWS);
    emulator.advance(lines.join("\r\n").as_bytes());
    emulator.screen().clone()
}

/// A tracker that has already published a resting verdict for `screen`, so a measured look at it is
/// the STEADY state of a settled pane rather than a first sighting.
///
/// Two observations, because a verdict resting on an absence is not published until its settle
/// window has closed — one look leaves a candidate pending, which is a different (and rarer) path
/// through `observe` than the one every client wake takes.
fn settled(screen: &Screen, rules: &Ruleset, base: Instant) -> Tracker {
    let mut tracker = Tracker::new(Hysteresis::default());
    tracker.observe(screen, Some(CLAUDE_TITLE), rules, base);
    tracker.observe(screen, Some(CLAUDE_TITLE), rules, base + DEFAULT_SETTLE);
    tracker
}

/// The text a measured pane holds: a full screen of ordinary words, ending in [`MARKER`].
///
/// A blank pane would have made the request rows a lie in the direction that matters — the reply a
/// client fetches carries the cells as data, so an empty screen serialises to almost nothing and
/// would have priced the fetch at a fraction of what a used terminal costs.
fn screenful() -> String {
    let width = usize::from(COLS) - 1;
    let mut line = String::new();
    while line.len() < width {
        line.push_str("the quick brown fox jumps over the lazy dog ");
    }
    line.truncate(width);

    let mut last = format!("{MARKER} {line}");
    last.truncate(width);

    let mut out = String::new();
    for row in 0..ROWS {
        if row + 1 == ROWS {
            out.push_str(&last);
        } else {
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

/// A pane that paints one screenful and then blocks on its PTY forever, so nothing it does can
/// land in the middle of a measurement.
fn painted() -> CommandBuilder {
    let mut command = CommandBuilder::new("/bin/sh");
    command.arg("-c");
    // The payload is letters, spaces and hyphens, so single quotes need no escaping.
    command.arg(format!("printf '%s' '{}'; exec cat", screenful()));
    command.env("TERM", "dumb");
    command
}

/// Block until `pane` has painted its screenful, reading the pane's own `full_text` rather than
/// sleeping for a guessed interval.
fn wait_for_content(state: &HostState, pane: usize) {
    let request = format!(
        r#"{{"jsonrpc":"2.0","id":9,"method":"scene/query","params":{{"path":"/pane_{pane}/sprag_input/external/full_text"}}}}"#
    );
    let deadline = Instant::now() + CONTENT_TIMEOUT;
    while Instant::now() < deadline {
        if let Some(response) = handle_request(state, &request)
            && response.contains(MARKER)
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("pane {pane} never painted its screenful within {CONTENT_TIMEOUT:?}");
}

/// A host holding [`PANE_COUNT`] panes, each with a screen full of text and each quiescent.
fn live_host() -> HostState {
    let host = Host::new((COLS, ROWS));
    for index in 0..PANE_COUNT {
        host.spawn(painted(), format!("pane{index}"), COLS, ROWS, None, None)
            .expect("spawn a pane");
    }
    let state = HostState::new(host, Arc::new(ChannelRegistry::default()), None);
    for index in 0..PANE_COUNT {
        wait_for_content(&state, index);
    }
    state
}

/// The size of the `result` an in-process request answers with — NOT the whole response.
///
/// The equivalence check between the in-process host and the socket one has to compare like with
/// like, and [`sprag_rpc::HostConn::call`] hands back the RESULT while [`handle_request`] returns the
/// whole JSON-RPC response. Comparing those two is comparing a payload against an envelope: the
/// first version of the socket rows did exactly that and flagged both of them as answering different
/// questions, on a 34-byte envelope.
fn result_bytes(state: &HostState, request: &str) -> usize {
    let response = handle_request(state, request).expect("the dispatch produced a response");
    serde_json::from_str::<serde_json::Value>(&response)
        .ok()
        .and_then(|value| serde_json::to_string(value.get("result")?).ok())
        .map_or(0, |text| text.len())
}

/// Serve one request, fail loudly if it did not succeed, and report the reply's size.
///
/// Called once per request subject OUTSIDE the measured loop: the check scans the whole response,
/// and a reply this large would have charged the scan to the thing being measured.
fn reply_bytes(state: &HostState, request: &str) -> usize {
    let response = handle_request(state, request).expect("the dispatch produced a response");
    assert!(
        !response.contains("\"error\""),
        "request failed: {request} -> {response}",
    );
    response.len()
}

/// A `sprag-term` spawned on a socket of its own, killed and unlinked when this drops.
///
/// The daemon must die with the tool even on a panic, and its socket must go with it: this mints a
/// fresh path per run, so a leak would strew one dead socket per invocation under the temp dir. The
/// kill comes first — the host holds the socket open until it exits.
struct SocketHost(std::process::Child, std::path::PathBuf);

impl Drop for SocketHost {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
        let _ = std::fs::remove_file(&self.1);
    }
}

/// Spawn a real `sprag-term` holding [`PANE_COUNT`] panes painted exactly like [`live_host`]'s, and
/// connect to it over its Unix socket.
///
/// # Why the panes have to match
///
/// The socket's cost is the DIFFERENCE between a request served over the wire and the same request
/// served in-process, and a difference between two hosts is only a transport measurement if the two
/// hosts are doing the same work. They cannot be the same host — one is this process and one is a
/// daemon — so the equivalence is established the only way it can be: the same boot command, the
/// same geometry, and the REPLY BYTES compared and printed. A row whose two replies differ in length
/// is comparing two different answers, and says so.
///
/// The binary is located beside this one. `CARGO_BIN_EXE_*` exists only for tests, and a tool that
/// took a path from the environment would silently measure whatever was there.
fn socket_host() -> Option<(SocketHost, sprag_rpc::HostConn)> {
    let exe = std::env::current_exe().ok()?;
    let daemon = exe.parent()?.join("sprag-term");
    if !daemon.is_file() {
        eprintln!(
            "sprag-latency: no sprag-term beside {} — skipping the socket rows.",
            exe.display()
        );
        eprintln!("Build it first: cargo build --release -p sprag-host --bins");
        return None;
    }
    let sock = std::env::temp_dir().join(format!("sprag-latency-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&sock);
    let child = std::process::Command::new(&daemon)
        .arg("--size")
        .arg(format!("{COLS}x{ROWS}"))
        .arg("--")
        .arg("/bin/sh")
        .arg("-c")
        .arg(format!("printf '%s' '{}'; exec cat", screenful()))
        .env("SPRAG_HOST_RPC_SOCK", &sock)
        .env("SPRAG_HOST_RPC", "1")
        .env("TERM", "dumb")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let host = SocketHost(child, sock.clone());
    let mut conn = sprag_rpc::HostConn::connect(&sock, CONTENT_TIMEOUT).ok()?;
    // The boot pane is one; the in-process host has PANE_COUNT. Spawn the rest through the same
    // mux action a client uses, so both hosts serve a pane list of the same length.
    for _ in 1..PANE_COUNT {
        let _ = conn.call(
            "scene/invoke",
            json!({
                "path": mux_action_path(sprag_host::wire::SPAWN_ACTION),
                "args": {"argv": ["/bin/sh", "-c", format!("printf '%s' '{}'; exec cat", screenful())]},
            }),
        );
    }
    // A pane list taken mid-boot is a SHORTER answer, and the byte comparison would then read as
    // "the two hosts disagree" when what differs is only how far along one of them is.
    let deadline = Instant::now() + CONTENT_TIMEOUT;
    while Instant::now() < deadline {
        let panes = conn
            .call("scene/query", json!({"path": "/sprag_mux/external/panes"}))
            .ok()
            .and_then(|value| Some(value.as_array()?.len()))
            .unwrap_or(0);
        if panes >= PANE_COUNT {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Some((host, conn))
}

/// How many pane-list requests one contention condition serves.
///
/// Large, because the subject is a TAIL rather than a centre: a reader only pays for a lock it
/// actually collides with, so the interesting samples are rare by construction and a short run can
/// miss them entirely. Two thousand at ~10 us is a fifth of a second per condition.
const CONTENTION_REQUESTS: usize = 2_000;

/// Serve [`CONTENTION_REQUESTS`] pane-list requests, timing each one, and return them SORTED.
///
/// Every other subject in this tool is estimated by its MINIMUM, for the reason the module doc
/// gives. **For a contention question the minimum is precisely the wrong estimator**: it is by
/// definition the sample that did not have to wait, so it reports the answer "no cost" whatever the
/// truth is. These rows are read at the tail instead, and the median is kept beside it to show
/// whether the whole distribution moved or only its top.
fn reader_latencies(state: &HostState) -> Vec<Duration> {
    latencies(state, |_| {})
}

/// [`reader_latencies`], running `between` OUTSIDE the timer before each request.
///
/// The isolate for "what does a churning ruleset cost the READER itself". Done from the reader's own
/// thread on purpose: the first version asked a second thread to churn, and a thread whose entire
/// loop body is `lock, reload, unlock` is a mutex hammer rather than a workload — it starved the
/// reader so badly that ADDING a sweep to that thread improved the reader's tail, which is a
/// std::sync::Mutex fairness property and not anything about this daemon. Single-threaded, the row
/// measures the thing it is named for and nothing else.
fn latencies(state: &HostState, mut between: impl FnMut(&HostState)) -> Vec<Duration> {
    let mut out = Vec::with_capacity(CONTENTION_REQUESTS);
    for _ in 0..CONTENTION_REQUESTS {
        between(state);
        let start = Instant::now();
        black_box(handle_request(black_box(state), PANES_READ));
        out.push(start.elapsed());
    }
    out.sort_unstable();
    out
}

/// [`reader_latencies`], with `background` running on a second thread as fast as it can for the
/// whole of the reader's run.
///
/// Continuously rather than once every five seconds, deliberately: the duty cycle a daemon actually
/// runs would put almost every request in the gap between passes and measure nothing, so this is the
/// worst case a reader could ever meet. A negligible answer here settles the real cadence a
/// fortiori, and a large one has to be scaled back down by the duty cycle before it means anything.
///
/// # Why the conditions come in PAIRS
///
/// The first draft of this ran one background loop — the real sweep on the reader's own registry —
/// and reported the difference as what the locks cost. It is not: the same difference is produced by
/// two other things the condition also changes, and both are large.
///
/// * **The reader re-evaluates too.** Making every pane stale is what forces the sweeper to work,
///   and the pane list runs the same detector under the same clock, so the reader's OWN request
///   starts evaluating every pane as well. Three panes of evaluation is hundreds of microseconds and
///   lands squarely in the range the tail moved to.
/// * **A second thread burns a core.** Cache, memory bandwidth and the scheduler are shared whether
///   or not a lock is.
///
/// So every sweeping condition is measured twice — once against the reader's own registry and once
/// against a PRIVATE one built the same way — and the sharing is the difference between the pair.
/// The private sweeper does identical work at an identical rate; the only thing it does not do is
/// touch a lock the reader wants. [[latency-budget-r221]]'s rule, in the one form that applies to a
/// concurrency question: a control cancels only the noise it shares.
fn under(state: &HostState, mut background: impl FnMut() + Send) -> Vec<Duration> {
    let stop = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            while !stop.load(Ordering::Relaxed) {
                background();
            }
        });
        let out = reader_latencies(state);
        stop.store(true, Ordering::Relaxed);
        out
    })
}

/// Passes run and panes evaluated by every [`pass`] call so far, process-wide.
///
/// THE CONTROL ON THE PROBE ITSELF. A background thread that never got scheduled, or one whose panes
/// turn out not to owe an evaluation after all, produces a reader distribution indistinguishable from
/// "the locks are free" — and the conclusion would be about the harness rather than about the daemon.
/// A condition that claims to sweep has to show its passes, and one that claims every pane owes an
/// evaluation has to show them evaluated.
static PASSES: AtomicU64 = AtomicU64::new(0);
static EVALUATED: AtomicU64 = AtomicU64::new(0);

/// Every pane's foreground job, read exactly as [`sweep_once`] reads it — the registry lock taken
/// and RELEASED to clone out the pools, each pool locked only long enough to read its panes' child
/// PIDS, and the `/proc/<pid>/stat` lines read after that lock is dropped.
///
/// **The split is the measurement's subject and not an incidental detail.** An instrument that held
/// the workspace lock across the reads would be measuring a shape the daemon deliberately does not
/// have — the one R291 measured at +687 us on a concurrent reader's median before moving the I/O
/// out ([`sprag_terminal::foreground_pgid_of`] carries that number).
///
/// It is deliberately the WALK and not one pane: what the daemon does every
/// [`SWEEP_INTERVAL`] is this, for every pane it has. Returns the
/// count so the compiler cannot elide the reads and so a run that measured an empty registry is
/// visible rather than fast.
fn read_every_foreground_pgid(host: &HostState) -> usize {
    let pools: Vec<_> = {
        let reg = host
            .registry()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        reg.sessions()
            .iter()
            .flat_map(|session| {
                session
                    .windows()
                    .iter()
                    .map(|window| Arc::clone(window.workspace()))
            })
            .collect()
    };
    let mut read = 0;
    for pool in &pools {
        let children: Vec<Option<u32>> = {
            let pool = pool
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            pool.panes().iter().map(|pane| pane.pty().pid()).collect()
        };
        for child in children {
            if child.and_then(sprag_terminal::foreground_pgid_of).is_some() {
                read += 1;
            }
        }
    }
    read
}

/// One sweep pass over `host`'s registry, at `now`, with discovery on — the daemon's own call.
///
/// `jobs` is the host's OWN foreground-job watch and must be the same one across calls: a watch
/// handed a fresh map each pass would re-establish every pane every time and so would never report
/// a change, which is the condition this instrument would then be measuring instead of the daemon's.
fn pass(host: &HostState, clock: &Arc<AgentClock>, jobs: &JobWatch) {
    let report = sweep_once(
        host.registry(),
        clock,
        jobs,
        host.channels(),
        Instant::now(),
        true,
    );
    PASSES.fetch_add(1, Ordering::Relaxed);
    EVALUATED.fetch_add(report.evaluated as u64, Ordering::Relaxed);
}

/// The passes and evaluations since the last call — read either side of a condition.
fn swept_since(before: (u64, u64)) -> (u64, u64) {
    (
        PASSES.load(Ordering::Relaxed) - before.0,
        EVALUATED.load(Ordering::Relaxed) - before.1,
    )
}

/// A reading of the two counters, for [`swept_since`] to subtract.
fn swept_now() -> (u64, u64) {
    (
        PASSES.load(Ordering::Relaxed),
        EVALUATED.load(Ordering::Relaxed),
    )
}

/// The sample at `fraction` through a sorted slice — the tail estimator these contention rows need.
fn percentile(sorted: &[Duration], fraction: f64) -> Duration {
    let last = sorted.len().saturating_sub(1);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let index = (fraction * last as f64).round() as usize;
    sorted[index.min(last)]
}

/// Print one derived line: a duration, and what fraction of a frame it is.
fn budget(label: &str, duration: Duration) {
    println!(
        "  {label:<46} {:>9.2} us  {:>7.3}% of a frame",
        micros(duration),
        duration.as_secs_f64() / FRAME.as_secs_f64() * 100.0,
    );
}

fn main() -> ExitCode {
    if cfg!(debug_assertions) {
        eprintln!("sprag-latency: refusing to measure a debug build.");
        eprintln!("A debug build is not the code that ships, and its timings are not a budget.");
        eprintln!("Run: cargo run --release -p sprag-host --bin sprag-latency");
        return ExitCode::from(2);
    }

    let floor = timer_floor();
    println!("sprag-latency — {COLS}x{ROWS} panes, {SAMPLES} samples per subject");
    println!(
        "clock floor {:.3} us; a span is repeated to ~{:.0} us.",
        micros(floor),
        micros(SPAN_TARGET)
    );
    println!(
        "min is the code's own cost and repeats; median is what this box did and does not.\n\
         xctl is this row in units of a 64 KiB copy measured in the same breath: exact for a row\n\
         that walks memory, a machine-scale rather than a precision claim for a row that allocates.\n"
    );
    println!(
        "{:<38} {:>10} {:>10} {:>9} {:>7} {:>8} {:>8}",
        "subject", "min us", "med us", "xctl", "reps", "bytes", "% frame"
    );

    let mut controls: Vec<Duration> = Vec::new();
    let palette = Palette::xterm_default();

    // The projection path, in the two crates that pay for it.
    let ascii = filled("abc 123 xyz ");
    let cjk = filled("世界");
    let ascii_buffer = project(&ascii, &palette);
    let cjk_buffer = project(&cjk, &palette);

    let projection = paired("project (ascii)", &mut controls, None, || {
        black_box(project(black_box(&ascii), &palette));
    });
    let clone_borrowed = paired(
        "clone, borrowed clusters (ascii)",
        &mut controls,
        None,
        || {
            black_box(black_box(&ascii_buffer).clone());
        },
    );
    let clone_owned = paired("clone, owned clusters (cjk)", &mut controls, None, || {
        black_box(black_box(&cjk_buffer).clone());
    });
    let token = paired("projection_token", &mut controls, None, || {
        black_box(projection_token(black_box(&ascii), &palette));
    });

    // The ENCODE path: what the reply to a cells fetch is made of.
    //
    // R230 CHANGED WHICH OF THESE THE REQUEST ACTUALLY PAYS. The pane external answered the
    // `cells.<offset>` family with `serde_json::to_value(frame)` until PINION-PR79 was delivered
    // (pinion R1480); it now answers `IntrospectValue::raw`, which encodes straight to text that
    // the dispatch splices. So `direct_to_string` is the live cost and the two DOM rows below are
    // kept as the PRICE OF WHAT WAS REMOVED — a comparison, no longer an attribution. Reading
    // them as parts of the request is what made this instrument's own subtraction go negative
    // the first time it was run against the change.
    let frame = CellFrame {
        cells: project(&ascii, &palette),
        facts: PaneScrollFacts {
            scrollback_len: 0,
            visible_rows: ROWS,
        },
    };
    let dom = serde_json::to_value(&frame).expect("a frame serialises");
    let encoded = serde_json::to_string(&frame).expect("a frame encodes");
    let encode_bytes = Some(encoded.len());
    let to_value = paired(
        "serde_json::to_value (one frame)",
        &mut controls,
        encode_bytes,
        || {
            black_box(serde_json::to_value(black_box(&frame)).expect("a frame serialises"));
        },
    );
    let dom_to_string = paired(
        "serde_json::to_string (that DOM)",
        &mut controls,
        encode_bytes,
        || {
            black_box(serde_json::to_string(black_box(&dom)).expect("a DOM encodes"));
        },
    );
    let direct_to_string = paired(
        "serde_json::to_string (frame direct)",
        &mut controls,
        encode_bytes,
        || {
            black_box(serde_json::to_string(black_box(&frame)).expect("a frame encodes"));
        },
    );

    // THE COMPARISON R222 EXISTS FOR, and it is taken INSIDE one run because that is the only
    // reproducible quantity this instrument has: the same cells encoded through pinion's derived
    // `Serialize` — the shape `cells.<offset>` answered until R222 — against the run-length form
    // it answers in now. Like for like: both are the grid alone, without the frame's facts.
    let derived = serde_json::to_string(&frame.cells).expect("a buffer encodes");
    let compact = serde_json::to_string(&sprag_grid::wire::encode(&frame.cells))
        .expect("a wire form encodes");
    let derived_to_string = paired(
        "serde_json::to_string (pre-R222 cells)",
        &mut controls,
        Some(derived.len()),
        || {
            black_box(serde_json::to_string(black_box(&frame.cells)).expect("a buffer encodes"));
        },
    );
    let derived_to_value = paired(
        "serde_json::to_value (pre-R222 cells)",
        &mut controls,
        Some(derived.len()),
        || {
            black_box(serde_json::to_value(black_box(&frame.cells)).expect("a buffer serialises"));
        },
    );

    // THE DETECTOR (H3), measured on its own before it is measured inside a request — because the
    // pane-list rows below are the noisiest in this battery (live PTY panes and their reader
    // threads share the process) and a difference between two of them is only as good as the band
    // it has to clear. These rows have no PTY in them at all.
    let claude_screen = painted_screen(CLAUDE_FOOTER);
    let shell_screen = painted_screen(SHELL);
    let rules = Ruleset::default();
    // The SAME rules with a different identity — a user saving `config.toml` unchanged. It is what
    // makes the gate miss without changing a single answer, which is the only way to price the skip
    // against a like-for-like alternative rather than against a different verdict.
    let reloaded = Ruleset::new(built_ins());

    let detect_shell = paired(
        "detect (shell: whole list, no claim)",
        &mut controls,
        None,
        || {
            black_box(detect(black_box(&shell_screen), None, rules.manifests()));
        },
    );
    let detect_claude = paired(
        "detect (claude: first manifest claims)",
        &mut controls,
        None,
        || {
            black_box(detect(
                black_box(&claude_screen),
                Some(CLAUDE_TITLE),
                rules.manifests(),
            ));
        },
    );

    let base = Instant::now();
    let settled_at = base + DEFAULT_SETTLE;
    let mut quiet = settled(&claude_screen, &rules, base);
    let quiet_before = sprag_detect::work();
    let gate_hit = paired(
        "Tracker::observe, quiet (gate hits)",
        &mut controls,
        None,
        || {
            black_box(quiet.observe(
                black_box(&claude_screen),
                Some(CLAUDE_TITLE),
                &rules,
                settled_at,
            ));
        },
    );
    let quiet_after = sprag_detect::work();

    // The gate MISSING, with everything else held still. The rules alternate between two lists that
    // say the same thing, so every look is a full evaluation and every look reaches the verdict
    // already published — which is what deleting the skip would cost, and nothing else. Measuring
    // it against a CHANGED screen instead would have priced a different verdict as well as a
    // different code path.
    let mut moving = settled(&claude_screen, &rules, base);
    let alternating = [&reloaded, &rules];
    let mut turn = 0_usize;
    let miss_before = sprag_detect::work();
    let gate_miss = paired(
        "Tracker::observe, reloaded (gate miss)",
        &mut controls,
        None,
        || {
            turn ^= 1;
            black_box(moving.observe(
                black_box(&claude_screen),
                Some(CLAUDE_TITLE),
                alternating[turn],
                settled_at,
            ));
        },
    );
    let miss_after = sprag_detect::work();

    // THE WRAPPER the daemon reaches the gate THROUGH, and the one part of it that could grow with
    // the workspace: `AgentClock::observe` reads the registry's nearest deadline before and after
    // each look, and that read walks every tracker. Whether it matters is a question about two
    // registry SIZES, so it is asked as one — the same pane, observed on a clock that remembers one
    // pane and on a clock that remembers many.
    let clocks: Vec<AgentClock> = REGISTRY_SIZES
        .iter()
        .map(|&remembered| {
            let clock = AgentClock::new(Ruleset::default());
            for id in 0..remembered {
                // Twice, so every tracker in the map is SETTLED: a registry full of pending
                // candidates is a different measurement, and not the one a quiet workspace makes.
                for at in [base, settled_at] {
                    clock.observe(
                        PaneId(id),
                        &claude_screen,
                        Some(CLAUDE_TITLE),
                        at,
                        Hysteresis::default,
                    );
                }
            }
            clock
        })
        .collect();
    let mut scaling: Vec<(u64, Sample)> = Vec::with_capacity(REGISTRY_SIZES.len());
    for (&remembered, clock) in REGISTRY_SIZES.iter().zip(&clocks) {
        // The SAME pane on every clock, so the only thing that differs between these rows is how
        // many OTHER panes the registry holds.
        let row = paired(
            &format!("AgentClock::observe, {remembered} kept"),
            &mut controls,
            None,
            || {
                black_box(clock.observe(
                    PaneId(0),
                    black_box(&claude_screen),
                    Some(CLAUDE_TITLE),
                    settled_at,
                    Hysteresis::default,
                ));
            },
        );
        scaling.push((remembered, row));
    }

    // THE SETTLE WAKER'S SWEEP (R260). Everything above is paid when a client asks; this is the one
    // loop in the daemon that runs whether or not anything is happening, so what it costs is paid by
    // every user for as long as the daemon lives. Its own doc priced it by ARGUMENT — a comparison
    // against the durability saver, which takes the same locks at the same interval — and the terms
    // below are what that argument enumerated, plus the ones it did not.
    //
    // Measured through `AgentClock::with`, because the lock is not incidental here: the sweep takes
    // it once per PANE rather than once per pass, so a per-pane row that measured the bare registry
    // method would be measuring something the daemon never calls.
    //
    // NOT measured, and enumerated rather than left as an absence: the registry and workspace LOCKS
    // the walk takes, and the session-name String cloned once per window to build the pool list.
    // The locks are excluded because an instrument with one thread measures them uncontended, and
    // uncontended is not what they cost — the difference IS the contention, and this tool cannot
    // produce it. They are also the terms the durability saver genuinely shares at the same
    // interval, so they are the half of the original argument that was right. The String is
    // excluded because it is per WINDOW, and a workspace has one or a few.
    let census: Vec<HashSet<PaneId>> = REGISTRY_SIZES
        .iter()
        .map(|&remembered| (0..remembered).map(PaneId).collect())
        .collect();
    let mut per_pane: Vec<(u64, Sample)> = Vec::with_capacity(REGISTRY_SIZES.len());
    let mut per_park: Vec<(u64, Sample)> = Vec::with_capacity(REGISTRY_SIZES.len());
    let mut per_prune: Vec<(u64, Sample)> = Vec::with_capacity(REGISTRY_SIZES.len());
    let mut per_census: Vec<(u64, Sample)> = Vec::with_capacity(REGISTRY_SIZES.len());
    let park_before = sprag_host::agent::work();
    for ((&remembered, clock), live) in REGISTRY_SIZES.iter().zip(&clocks).zip(&census) {
        // ONCE PER PANE PER SWEEP: the question that decides whether the screen behind this pane is
        // read at all. Asked with `sweep` true and of a settled pane under unchanged rules, which is
        // the answer every pane in a quiet workspace gives — `false`, and the evaluation never runs.
        per_pane.push((
            remembered,
            paired(
                &format!("sweep: owes_evaluation, {remembered} kept"),
                &mut controls,
                None,
                || {
                    black_box(clock.with(|state| {
                        state.owes_evaluation(black_box(PaneId(0)), settled_at, true)
                    }));
                },
            ),
        ));
        // TWICE PER WAKE: once inside `park_until_due` to choose how long to sleep, and once after
        // waking to decide whether anything is actually due. Both walk every tracker, and both have
        // to — the sleep can be cut short by a candidate appearing with a nearer deadline, so the
        // answer from before the park cannot be reused after it.
        per_park.push((
            remembered,
            paired(
                &format!("sweep: any_due, {remembered} kept"),
                &mut controls,
                None,
                || {
                    black_box(clock.with(|state| state.any_due(settled_at)));
                },
            ),
        ));
        // ONCE PER SWEEP: forget the panes that are gone. Measured against a FULL census, so nothing
        // is removed — the steady state, and the only one that recurs. A retain that actually drops
        // entries happens once per pane close.
        per_prune.push((
            remembered,
            paired(
                &format!("sweep: retain_live, {remembered} kept"),
                &mut controls,
                None,
                || {
                    clock.with(|state| state.retain_live(black_box(live)));
                },
            ),
        ));
        // ONCE PER SWEEP, built one insert at a time as the walk visits each pane. It is the argument
        // for `retain_live` being cheap — a daemon-wide census is a by-product of a walk already
        // happening — and a by-product is not free.
        per_census.push((
            remembered,
            paired(
                &format!("sweep: census build, {remembered} panes"),
                &mut controls,
                None,
                || {
                    let mut live: HashSet<PaneId> = HashSet::new();
                    for id in 0..remembered {
                        live.insert(PaneId(id));
                    }
                    black_box(live);
                },
            ),
        ));
    }
    let park_after = sprag_host::agent::work();

    // ONCE PER SWEEP, AND IN NO VERSION OF THE COST ARGUMENT: R254 put the user's manifest reload on
    // this thread, correctly — it needs a wake and this is the wake that exists. What that settled
    // was the SCHEDULING. The sweep's own cost paragraph still enumerated a walk and some locks, and
    // the durability saver it compares itself against reads no file at all: it WRITES when the shape
    // changed and is silent otherwise. So this is the one recurring term with no counterpart in the
    // thing the marginal-cost argument was measured against.
    //
    // Both operating points, because they are different syscalls and most users are the second: a
    // file that exists and is unchanged, and a config path with no file behind it.
    let manifest_path = std::env::temp_dir().join("sprag-latency-manifests.toml");
    std::fs::write(
        &manifest_path,
        "[[agent]]\nname = \"claude\"\ndisable = [\"idle-glyph\"]\n",
    )
    .expect("a manifest fixture is writable");
    let mut present = AgentManifests::at(Some(&manifest_path));
    let refresh_present = paired(
        "sweep: manifests refresh (file)",
        &mut controls,
        None,
        || {
            black_box(present.refresh());
        },
    );
    let missing_path = std::env::temp_dir().join("sprag-latency-no-such-manifests.toml");
    let _ = std::fs::remove_file(&missing_path);
    let mut absent = AgentManifests::at(Some(&missing_path));
    let refresh_absent = paired(
        "sweep: manifests refresh (no file)",
        &mut controls,
        None,
        || {
            black_box(absent.refresh());
        },
    );
    let _ = std::fs::remove_file(&manifest_path);

    // The request path, served in-process exactly as the transport serves it.
    let state = live_host();
    let revision = paired(
        "request scene/revision",
        &mut controls,
        Some(reply_bytes(&state, REVISION)),
        || {
            black_box(handle_request(black_box(&state), REVISION));
        },
    );
    let panes_bytes = reply_bytes(&state, PANES_READ);
    let panes = paired(
        "request panes slot (with tokens)",
        &mut controls,
        Some(panes_bytes),
        || {
            black_box(handle_request(black_box(&state), PANES_READ));
        },
    );
    // THE SESSION LIST, against a host that differs in ONE thing: whether any session holds a pane
    // with a live child. That is the gate the enrichment sits behind — each session's cwd, its git
    // branch, and its listening ports, the last of which reads `/proc/*/stat` for every process on
    // the box. A display client re-reads this slot on every poll wake, and a wake is a batch of PTY
    // output, so what these two rows differ by is paid at TYPING rate.
    //
    // The control moves one more thing than the walk, and saying so is cheaper than a reader
    // assuming otherwise: an idle host lists NO session (a paneless, unattached anchor is not
    // listable), so its reply is an empty array where the live one carries a row. The bytes column
    // shows both replies are small — whatever separates these two rows, it is not serialisation.
    let idle = HostState::new(
        Host::new((COLS, ROWS)),
        Arc::new(ChannelRegistry::default()),
        None,
    );
    let sessions_idle = paired(
        "request sessions slot (no live pane)",
        &mut controls,
        Some(reply_bytes(&idle, SESSIONS_READ)),
        || {
            black_box(handle_request(black_box(&idle), SESSIONS_READ));
        },
    );
    let sessions_live = paired(
        "request sessions slot (live panes)",
        &mut controls,
        Some(reply_bytes(&state, SESSIONS_READ)),
        || {
            black_box(handle_request(black_box(&state), SESSIONS_READ));
        },
    );
    budget(
        "the session list's enrichment",
        sessions_live.min.saturating_sub(sessions_idle.min),
    );
    // THE SAMPLE, at the two tolerances the design has callers for. `sprag ls` passes zero and buys
    // a `/proc` walk of the box; a display client passes a window it can live with and is answered
    // from whatever the daemon already holds. The pair is what makes the split legible as a cost:
    // the walk did not get cheaper, it stopped being attached to the question above.
    let activity_fresh = paired(
        "request session_activity (max_age 0)",
        &mut controls,
        Some(reply_bytes(&state, &activity_read(0))),
        || {
            black_box(handle_request(black_box(&state), &activity_read(0)));
        },
    );
    let tolerated = u64::try_from(SESSION_ACTIVITY_DISPLAY_MAX_AGE.as_millis()).unwrap_or(u64::MAX);
    let activity_held = paired(
        "request session_activity (display tolerance)",
        &mut controls,
        Some(reply_bytes(&state, &activity_read(tolerated))),
        || {
            black_box(handle_request(black_box(&state), &activity_read(tolerated)));
        },
    );
    budget(
        "what a poll wake used to pay, and now does",
        activity_held.min,
    );
    budget("what asking for a FRESH sample costs", activity_fresh.min);

    // R290's sample, at the same two tolerances and for the same reason: the pair is what makes the
    // trade legible. The fresh row is a full `/proc` pass — every process on the box, indexed by
    // group — and it answers for EVERY pane at once, so it is also what N panes cost together. The
    // held row is what a second reader inside the tolerance pays, which is the coalescing as a
    // number rather than as a claim.
    let processes_fresh = paired(
        "request pane_processes (max_age 0)",
        &mut controls,
        Some(reply_bytes(&state, &processes_read(0))),
        || {
            black_box(handle_request(black_box(&state), &processes_read(0)));
        },
    );
    let processes_held = paired(
        "request pane_processes (display tolerance)",
        &mut controls,
        Some(reply_bytes(&state, &processes_read(tolerated))),
        || {
            black_box(handle_request(
                black_box(&state),
                &processes_read(tolerated),
            ));
        },
    );
    budget(
        "what one /proc pass buys for every pane at once",
        processes_fresh.min,
    );
    budget(
        "what a second reader inside the tolerance pays",
        processes_held.min,
    );

    // **R291's cost argument, as the pair that makes it legible.** The job-change EVENT does not
    // watch the answer above — it watches that answer's IDENTITY. One `/proc/<pid>/stat` line per
    // pane against a pass over every process on the box, and the sweep pays it for every pane once
    // every `SWEEP_INTERVAL`, which is a thing the row above could never be: 2.7 ms every five
    // seconds, forever, to notice that a shell went back to its prompt.
    //
    // Measured as the WALK over all `PANE_COUNT` panes, because that is what the daemon does; a
    // single-pane row would understate a per-pane term by the count.
    let jobs_read = read_every_foreground_pgid(&state);
    assert_eq!(
        jobs_read, PANE_COUNT,
        "every pane has a live child, or this row is measuring an empty walk",
    );
    let job_sample = paired(
        &format!("sweep: foreground_pgid, {PANE_COUNT} panes"),
        &mut controls,
        None,
        || {
            black_box(read_every_foreground_pgid(black_box(&state)));
        },
    );
    // And what the WATCH does with each reading: one hash insert and one comparison, which is the
    // whole of the establish rule. Measured on a settled pane — the answer every quiet pane gives.
    let watch = JobWatch::new();
    black_box(watch.observe(PaneId(0), Some(4242)));
    let job_observe = paired(
        "sweep: JobWatch::observe, settled",
        &mut controls,
        None,
        || {
            black_box(watch.observe(black_box(PaneId(0)), black_box(Some(4242))));
        },
    );
    budget(
        "what watching every pane's job costs one sweep",
        job_sample.min + job_observe.min * PANE_COUNT as u32,
    );
    println!(
        "  {:<46} {:>9.2}x  cheaper than one /proc pass",
        "so the EVENT against the ANSWER it names",
        processes_fresh.min.as_secs_f64() / job_sample.min.as_secs_f64(),
    );

    // THE DERIVE SITE, at the two ends of a realistic span. It runs after EVERY mutating dispatch,
    // and a keystroke is one (`key`/`text`/`paste`/`mouse` are all invokes) — so this is paid at
    // TYPING rate, which is the cost the H6 design argued about and did not measure. Measured with
    // no change to find: the steady state, and the only one that recurs.
    // The `named` axis is R295's OWN cost, and it is the control this row needed rather than an
    // extra: a pane's name joined the shape, so an unnamed pane clones a `None` (no allocation) and
    // a named one clones a `String`. Measuring only unnamed panes would price the feature at zero by
    // construction — the shape R294 was caught by when its instrument could not move.
    for panes in [1_usize, 64] {
        for named in [false, true] {
            let shaped = Host::new((COLS, ROWS));
            for index in 0..panes {
                let id = shaped
                    .spawn(painted(), format!("q{index}"), COLS, ROWS, None, None)
                    .expect("spawn a quiescent pane");
                if named {
                    shaped
                        .workspace()
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .set_pane_name(
                            id,
                            Some(
                                sprag_terminal::PaneName::parse(&format!("work-pane-{index}"))
                                    .expect("a legal name"),
                            ),
                        );
                }
            }
            let shaped = HostState::new(shaped, Arc::new(ChannelRegistry::default()), None);
            let channels = shaped.channels().clone();
            // Seed the shape, so the row measures the STEADY diff and not the first observation.
            channels.observe(
                &shaped
                    .registry()
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
                "0",
            );
            let which = if named { "all named" } else { "unnamed" };
            paired(
                &format!("events: observe, {panes} panes {which}, no change"),
                &mut controls,
                None,
                || {
                    channels.observe(
                        &shaped
                            .registry()
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner),
                        "0",
                    );
                },
            );
        }
    }

    // THE TERM R292 ADDED TO THAT SITE, measured rather than argued. A session may have filtered
    // waits parked on its journal, and the derive site above now runs beside them — so the question
    // is whether a keystroke pays for a waiter that cannot possibly have news.
    //
    // It must not, and `JournalChannel::take_satisfied` returns immediately when the append landed
    // nothing (which is what a keystroke lands). Without that gate this row walks the ring once per
    // waiter per keystroke, from the OLDEST record — and deleting the gate is the control that moves
    // it, which is how the number below was checked rather than assumed.
    {
        let shaped = Host::new((COLS, ROWS));
        shaped
            .spawn(painted(), "q0".to_owned(), COLS, ROWS, None, None)
            .expect("spawn a quiescent pane");
        let shaped = HostState::new(shaped, Arc::new(ChannelRegistry::default()), None);
        let channels = shaped.channels().clone();
        let journal = channels.journal("0");
        // A FULL ring, so an un-gated evaluation has the whole 256 records to skip past — the worst
        // case, which is also the steady state of a workspace anyone has been using.
        let revision = channels.revision("0");
        for id in 0..u64::try_from(sprag_host::events::JOURNAL_CAPACITY).unwrap_or(u64::MAX) {
            journal.announce(&revision, vec![sprag_host::events::Event::PaneCreated(id)]);
        }
        // Eight parked waits, each caught up, none of which any keystroke can satisfy.
        for _ in 0..8 {
            journal.park_or_answer(
                pinion_rpc::ConnId::allocate(),
                revision.current(),
                sprag_host::events::EventFilter::AnyOf(vec![sprag_host::events::Clause {
                    kind: Some(sprag_host::events::EventKind::PaneJobChanged),
                    subject: Some(sprag_host::events::Subject::Pane(9_999)),
                }]),
                Some(pinion_rpc::RequestId::Num(1)),
                pinion_rpc::RpcReply::new(|_| {}),
            );
        }
        assert_eq!(
            journal.parked_count(),
            8,
            "the row needs its waiters parked"
        );
        channels.observe(
            &shaped
                .registry()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            "0",
        );
        paired(
            "events: observe, 1 pane, 8 waits parked, no change",
            &mut controls,
            None,
            || {
                channels.observe(
                    &shaped
                        .registry()
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner),
                    "0",
                );
            },
        );
    }

    // THE OTHER AXIS, and R269 shipped without it. The rows above hold the pane count constant at
    // ONE WINDOW, and `SessionShape::read` takes a workspace lock PER WINDOW — so the term they do
    // not cover is the one that scales with locks rather than with `u64`s. A session with many
    // windows is ordinary (tmux users live in them), so "no row measures that yet" was a debt, not
    // a boundary.
    for windows in [1_usize, 16] {
        let shaped = Host::new((COLS, ROWS));
        {
            let mut registry = shaped
                .registry()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for _ in 1..windows {
                registry
                    .new_window("0", None)
                    .expect("create a window in the boot session");
            }
        }
        // One pane per window, so each window's lock guards something and the walk is not measuring
        // an empty pool it would never meet.
        for index in 0..windows {
            shaped
                .spawn(painted(), format!("w{index}"), COLS, ROWS, None, None)
                .expect("spawn a pane in the current window");
        }
        let shaped = HostState::new(shaped, Arc::new(ChannelRegistry::default()), None);
        let channels = shaped.channels().clone();
        channels.observe(
            &shaped
                .registry()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            "0",
        );
        paired(
            &format!("events: observe, {windows} windows, no change"),
            &mut controls,
            None,
            || {
                channels.observe(
                    &shaped
                        .registry()
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner),
                    "0",
                );
            },
        );
    }

    let cells = paired(
        "request cells.0 (one pane)",
        &mut controls,
        Some(reply_bytes(&state, CELLS_READ)),
        || {
            black_box(handle_request(black_box(&state), CELLS_READ));
        },
    );
    let snapshot = paired(
        "request scene/snapshot (all panes)",
        &mut controls,
        Some(reply_bytes(&state, SNAPSHOT)),
        || {
            black_box(handle_request(black_box(&state), SNAPSHOT));
        },
    );

    // THE PANE LIST WITH THE DETECTOR IN IT. `with_agents` consumes the state and hands it back, so
    // this is literally the host measured above with a detector installed — the same three panes,
    // the same screens, the same request text. Not a second host, which is the only way a
    // difference this small is attributable to anything.
    let state = state.with_agents(Arc::new(AgentClock::new(Ruleset::default())));
    // Served once OUTSIDE the timed span, which is also every pane's first observation — the one
    // look that has to evaluate. What the span then measures is the steady state a settled
    // workspace is in for the rest of the daemon's life.
    let agent_bytes = reply_bytes(&state, PANES_READ);
    let evaluations_before = sprag_detect::work();
    let panes_with_agents = paired(
        "request panes slot (+ agent eval)",
        &mut controls,
        Some(agent_bytes),
        || {
            black_box(handle_request(black_box(&state), PANES_READ));
        },
    );
    let evaluations_after = sprag_detect::work();

    // THE SOCKET (R262). Every row above is served IN-PROCESS through `handle_request`, the same
    // entry point the transport calls — a bound this tool has stated since it was written and never
    // priced. A client's wall clock is three things stacked: the daemon's own work (measured above),
    // the TRANSPORT (two syscall pairs and the bytes between them), and the client's PARSE of the
    // reply into a value it can use. All three are measured here so the middle one can be had by
    // subtraction rather than asserted.
    //
    // The daemon cannot be this process, so the equivalence between the two hosts is established the
    // only way it can be: the same boot command, the same geometry, the same pane count — and the
    // REPLY BYTES printed for both, because a difference between two hosts is a transport
    // measurement only while they are answering the same question with the same answer.
    let socket_rows: Vec<(&str, Sample, Sample, usize, usize)> =
        if let Some((_host, mut conn)) = socket_host() {
            let mut rows = Vec::new();
            for (name, method, params, in_process, in_process_bytes) in [
                (
                    "scene/revision",
                    "scene/revision",
                    json!({}),
                    revision,
                    result_bytes(&state, REVISION),
                ),
                (
                    "panes slot",
                    "scene/query",
                    json!({"path": "/sprag_mux/external/panes"}),
                    panes,
                    result_bytes(&state, PANES_READ),
                ),
            ] {
                // Served once outside the timer, both to warm the path and to SIZE it: the byte
                // count is the equivalence check between the two hosts.
                let Ok(first) = conn.call(method, params.clone()) else {
                    continue;
                };
                let wire_bytes = serde_json::to_string(&first).map_or(0, |text| text.len());
                let text = serde_json::to_string(&first).unwrap_or_default();
                let over_wire = paired(
                    &format!("socket: {name}"),
                    &mut controls,
                    Some(wire_bytes),
                    || {
                        black_box(conn.call(method, params.clone()).ok());
                    },
                );
                // The client's own parse, on the very bytes that came back — the third term, so the
                // transport is what is left after it and the daemon's work are taken out.
                let parse = paired(
                    "  its reply, parsed client-side",
                    &mut controls,
                    Some(text.len()),
                    || {
                        black_box(serde_json::from_str::<serde_json::Value>(black_box(&text)).ok());
                    },
                );
                rows.push((name, over_wire, parse, wire_bytes, in_process_bytes));
                let _ = in_process;
            }
            rows
        } else {
            Vec::new()
        };
    let socket_in_process = [revision, panes];

    // WHAT THE SWEEP'S LOCKS COST (R261). R260 measured every term of a sweep and refused this one,
    // because a single-threaded instrument acquires a lock uncontended and uncontended is not what a
    // lock costs. What it costs is the WAIT it inflicts on somebody else, so the subject is not the
    // sweep at all — it is a READER's latency while the sweep runs.
    //
    // Three conditions against the same host: no sweeper, a sweeper whose panes are all settled, and
    // a sweeper that reloads the ruleset before every pass so every pane owes an evaluation. The
    // third is what a user's `config.toml` save schedules, and what the first pass after boot does.
    let clock = state.agents().expect("the state has a detector installed");
    // The PRIVATE host the control conditions sweep: built the same way, with the same panes and the
    // same detector, and shared with nothing. A sweeper on it does identical work at an identical
    // rate and touches no lock the reader wants.
    let other = live_host().with_agents(Arc::new(AgentClock::new(Ruleset::default())));
    let other_clock = other.agents().expect("the private host has one too");
    // One job watch per HOST, for the reason `pass` states: the two registries hold different panes,
    // and a watch shared between them would call every pane of each a change on every pass.
    let jobs = JobWatch::new();
    let other_jobs = JobWatch::new();
    // Two rulesets that say the SAME thing, alternated: every pass finds every pane's verdict reached
    // under a revision no longer in force, which is a manifest save sustained. Built once, because
    // `built_ins` compiles patterns and a sweeper in the regex compiler is not a sweeper holding a
    // lock.
    let churn = [Ruleset::new(built_ins()), Ruleset::new(built_ins())];
    // ONE TOGGLE PER CLOCK. A single shared toggle flipped once per clock per iteration, so each
    // clock was handed the SAME ruleset every time and nothing ever went stale — thousands of passes
    // evaluating three panes between them. The `evald` column is what caught it, which is why it is
    // printed: without it both churning conditions would have reported a lock cost of zero, and the
    // zero would have been the harness.
    let mut turns = [0_usize; 2];
    let mut stale = |which: usize, clock: &Arc<AgentClock>| {
        turns[which] ^= 1;
        clock.with(|state| state.reload(churn[turns[which]].clone()));
    };

    // A pass timed on its own, so the differential rows below have a hold time to be read against:
    // whatever a reader can wait for, it cannot be longer than one window's share of this.
    let churn_pass = {
        stale(0, &clock);
        let start = Instant::now();
        let report = sweep_once(
            state.registry(),
            &clock,
            &jobs,
            state.channels(),
            Instant::now(),
            true,
        );
        let elapsed = start.elapsed();
        assert!(report.evaluated > 0, "a churning pass has to evaluate");
        (elapsed, report.evaluated)
    };
    let quiet_pass = {
        let start = Instant::now();
        let report = sweep_once(
            state.registry(),
            &clock,
            &jobs,
            state.channels(),
            Instant::now(),
            true,
        );
        let elapsed = start.elapsed();
        assert_eq!(report.evaluated, 0, "a second pass evaluates nothing");
        elapsed
    };

    let free = reader_latencies(&state);
    let mark = swept_now();
    let quiet_private = under(&state, || pass(&other, &other_clock, &other_jobs));
    let quiet_private_work = swept_since(mark);
    let mark = swept_now();
    let quiet_shared = under(&state, || pass(&state, &clock, &jobs));
    let quiet_shared_work = swept_since(mark);
    // Staleness is forced on the READER's clock in every stale condition, including the controls --
    // that is what makes the reader re-evaluate, and it has to be held constant so the pair differs
    // only in which registry is swept.
    let stale_none = latencies(&state, |_| stale(0, &clock));
    // The pair differs in ONE thing: which registry the sweeper walks. Both churn BOTH clocks, so
    // the background loop runs at the same rate and forces the reader's own re-evaluation at the
    // same rate — the first version churned one clock on one side and two on the other, which let
    // the shared condition iterate faster and charged the difference to the lock.
    let mark = swept_now();
    let stale_private = under(&state, || {
        stale(0, &clock);
        stale(1, &other_clock);
        pass(&other, &other_clock, &other_jobs);
    });
    let stale_private_work = swept_since(mark);
    let mark = swept_now();
    let stale_shared = under(&state, || {
        stale(0, &clock);
        stale(1, &other_clock);
        pass(&state, &clock, &jobs);
    });
    let stale_shared_work = swept_since(mark);

    let quietest = controls.iter().min().copied().unwrap_or_default();
    let noisiest = controls.iter().max().copied().unwrap_or_default();
    println!(
        "\ncontrol (64 KiB copy): {:.2}..{:.2} us over {} pairings, spread {:.1}% — this run's\n\
         disclosure of how quiet the box was while the rows above were taken.",
        micros(quietest),
        micros(noisiest),
        controls.len(),
        (noisiest.as_secs_f64() / quietest.as_secs_f64() - 1.0) * 100.0,
    );

    println!("\nMEASURED — where a one-pane cells fetch, the client's steady-state read, goes:");
    budget("the projection the request is named for", projection.min);
    budget("encoding the reply, straight to text", direct_to_string.min);
    budget("the whole request, in-process", cells.min);
    budget(
        "  of which this instrument cannot yet attribute",
        cells
            .min
            .saturating_sub(projection.min + direct_to_string.min),
    );
    println!(
        "  the projection is {:.2}% of the request that carries it.",
        projection.min.as_secs_f64() / cells.min.as_secs_f64() * 100.0,
    );
    println!(
        "\n  the DOM this request NO LONGER BUILDS (PINION-PR79, consumed R230), priced in this\n  \
         same run: {:.2} us to build + {:.2} us to print = {:.2} us, against {:.2} us straight to\n  \
         text — {:.1}x. That saving is why the projection's share above moved without the\n  \
         projection itself changing, which is the same lesson R222 recorded one layer up.",
        micros(to_value.min),
        micros(dom_to_string.min),
        micros(to_value.min + dom_to_string.min),
        micros(direct_to_string.min),
        (to_value.min + dom_to_string.min).as_secs_f64() / direct_to_string.min.as_secs_f64(),
    );

    // The size is the reason for the time, so the shape that produces the size is worth printing
    // once rather than describing: a reader can see what a cell costs on the wire.
    let cells_in_pane = u64::from(COLS) * u64::from(ROWS);
    println!(
        "  one pane's reply is {} bytes for {cells_in_pane} cells — {:.0} bytes per cell:",
        encoded.len(),
        encoded.len() as f64 / cells_in_pane as f64,
    );
    println!("    {}...", &encoded[..encoded.len().min(150)]);

    println!("\nMEASURED — R222's encoding against the one it replaced, both taken in THIS run:");
    println!(
        "  pre-R222, pinion's derived cell shape: {:>7} bytes, {:>3.0} per cell",
        derived.len(),
        derived.len() as f64 / cells_in_pane as f64,
    );
    println!(
        "  now, the run-length form:              {:>7} bytes, {:>3.0} per cell  -> {:.0}x smaller",
        compact.len(),
        compact.len() as f64 / cells_in_pane as f64,
        derived.len() as f64 / compact.len() as f64,
    );
    println!(
        "  the DOM that size is paid through:     {:>7.1} us -> {:>5.1} us     -> {:.0}x cheaper",
        micros(derived_to_value.min),
        micros(to_value.min),
        derived_to_value.min.as_secs_f64() / to_value.min.as_secs_f64(),
    );
    println!(
        "  printing the cells alone:              {:>7.1} us -> {:>5.1} us     -> {:.0}x cheaper",
        micros(derived_to_string.min),
        micros(direct_to_string.min),
        derived_to_string.min.as_secs_f64() / direct_to_string.min.as_secs_f64(),
    );
    println!(
        "  BYTES ARE COUNTS and repeat to the digit; the two microsecond rows are ratios taken\n  \
         inside one run, which is the only form a duration on this box survives in."
    );

    println!("\nMEASURED — a display client's poll wake over {PANE_COUNT} panes, one changed:");
    budget("now: panes slot + 1 cells fetch", panes.min + cells.min);
    budget(
        "before R220: panes slot + every pane fetched",
        panes.min + cells.min * PANE_COUNT as u32,
    );
    budget(
        "saved by the fetch gate",
        cells.min * (PANE_COUNT as u32 - 1),
    );

    println!("\nMEASURED — what a cluster allocation per cell costs one pane, one frame:");
    budget("clone with clusters it can borrow", clone_borrowed.min);
    budget("clone with clusters it must own", clone_owned.min);
    budget(
        "saved by the borrow",
        clone_owned.min.saturating_sub(clone_borrowed.min),
    );

    println!("\nDERIVED — R218's gate, priced from the projection measured above:");
    budget("a scene/revision, now", revision.min);
    budget(
        "the same read before the gate",
        revision.min + projection.min * PANE_COUNT as u32,
    );
    budget(
        "a snapshot, which must still pay for every pane",
        snapshot.min,
    );
    println!(
        "\nThe fetch a token can skip costs {:.0}x the token that decides to skip it.",
        cells.min.as_secs_f64() / token.min.as_secs_f64(),
    );

    println!(
        "\nMEASURED — what the agent detector (H3) costs the pane list, the debt R253 opened and\n\
         R254 recorded a second reason for. The same host, the same panes, one difference:"
    );
    budget("the pane list, no detector", panes.min);
    budget(
        "the same pane list, detector installed",
        panes_with_agents.min,
    );
    let difference = micros(panes_with_agents.min) - micros(panes.min);
    let band = micros(noisiest) - micros(quietest);
    println!(
        "  the difference is {difference:+.3} us ({:+.1}%), against a control that moved {band:.3} \
         us\n  between its quietest and its noisiest pairing in this same run. DO NOT QUOTE THIS\n  \
         ROW: ten runs of this battery spread it over sixty-fold (this file's docs record the\n  \
         range). It BOUNDS the cost and does not resolve it — the two rows carry three live PTY\n  \
         panes and their reader threads, which is interference the control cannot see and so\n  \
         cannot cancel. The row that resolves it is the registry one below.",
        (panes_with_agents.min.as_secs_f64() / panes.min.as_secs_f64() - 1.0) * 100.0,
    );
    println!(
        "  the two replies are {panes_bytes} and {agent_bytes} bytes: no manifest claims a shell\n  \
         pane, so D8 leaves the key absent and whatever the difference is, it is WORK and not\n  \
         payload."
    );

    // The count, taken across the row above. Every call `measure` makes is accounted for, so the
    // denominator is exact rather than an estimate of how much work the row did.
    let looks = calls(&panes_with_agents) * PANE_COUNT as u64;
    let evaluations = evaluations_after.evaluations_total - evaluations_before.evaluations_total;
    println!(
        "\n  AND THE COUNT, which repeats to the digit where every microsecond above does not:\n  \
         that row took {looks} looks at a settled pane and ran {evaluations} evaluations. The\n  \
         quiescence gate skipped {:.4}% of them, and `sprag_detect::work` is where a TEST can say\n  \
         so — R254 put the rule list in the gate's key, which was right and which left the skip\n  \
         with no behavioural observable at all. `sprag-detect`'s `tests/meter.rs` is that test;\n  \
         this row is what it is worth.",
        (1.0 - evaluations as f64 / looks as f64) * 100.0,
    );

    println!("\nMEASURED — what that skip is worth, with everything but the gate held still:");
    budget("a look at a quiet pane (the gate hits)", gate_hit.min);
    budget("the same look, rules replaced (it misses)", gate_miss.min);
    budget(
        "  saved, per pane per look",
        gate_miss.min.saturating_sub(gate_hit.min),
    );
    budget(
        &format!("  over {PANE_COUNT} panes on one client wake"),
        gate_miss.min.saturating_sub(gate_hit.min) * PANE_COUNT as u32,
    );
    println!(
        "  the gate is {:.0}x cheaper than the evaluation it decides not to run.",
        gate_miss.min.as_secs_f64() / gate_hit.min.as_secs_f64(),
    );
    // The two rows are only worth what their code paths are, so the same meter that gates the skip
    // is read across them here: a row claiming to skip must have skipped, and a row claiming to
    // evaluate must have evaluated once per call. Without this the pair could be measuring the same
    // path twice and reporting the difference as a saving.
    println!(
        "  the rows are what they say: {} evaluations over {} quiet looks, {} over {} reloaded\n  \
         ones — read from `sprag_detect::work` across each row, not asserted about it.",
        quiet_after.evaluations_total - quiet_before.evaluations_total,
        calls(&gate_hit),
        miss_after.evaluations_total - miss_before.evaluations_total,
        calls(&gate_miss),
    );
    println!(
        "  the evaluation alone: {:.1} us for a pane the first manifest claims, {:.1} us for one\n  \
         nobody claims -> {:.2}x. The unclaimed case is every ordinary shell pane in the\n  \
         workspace, and it is the whole list because identification stops only at a CLAIM — which\n  \
         is what slice 4's layering charges when a user's new agent goes to the FRONT.",
        micros(detect_claude.min),
        micros(detect_shell.min),
        detect_shell.min.as_secs_f64() / detect_claude.min.as_secs_f64(),
    );
    println!(
        "  AND THE SHAPE OF IT: one evaluation is {:.1}x the whole pane-list request that would\n  \
         carry it ({:.1} us), where R221 found the projection at half a percent of the fetch it is\n  \
         named for. The gate is not an optimisation on this path; it is what keeps the pane list\n  \
         the cost of a pane list.",
        detect_shell.min.as_secs_f64() / panes.min.as_secs_f64(),
        micros(panes.min),
    );

    println!("\nMEASURED — does one look grow with the SIZE of the registry it is taken from?");
    for (remembered, row) in &scaling {
        let pane = if *remembered == 1 { "pane" } else { "panes" };
        budget(&format!("a look, {remembered} {pane} remembered"), row.min);
    }
    let (smallest, small_row) = scaling.first().expect("the sizes are not empty");
    let (largest, large_row) = scaling.last().expect("the sizes are not empty");
    let slope =
        (large_row.min.as_secs_f64() - small_row.min.as_secs_f64()) / (largest - smallest) as f64;
    println!(
        "  {:.2}x for {}x the registry — {:.2} ns per remembered pane, per look.",
        large_row.min.as_secs_f64() / small_row.min.as_secs_f64(),
        largest / smallest.max(&1),
        slope * 1e9,
    );
    // The middle size was the control that identified the term, and it is kept because it is also
    // what tells a REAPPEARANCE from noise: a walk climbs with the entry count, and nothing else in
    // `observe` does.
    if let Some((middle, middle_row)) = scaling.get(1) {
        let predicted = small_row.min.as_secs_f64() + slope * (middle - smallest) as f64;
        println!(
            "  the middle size is the CONTROL: {middle} panes measured {:.3} us against {:.3} us\n  \
             predicted by a straight line through the other two — {:+.1}%. A cost linear in the\n  \
             entry count is a WALK of the registry; a hash lookup losing its cache in a bigger map\n  \
             would step rather than climb.",
            micros(middle_row.min),
            predicted * 1e6,
            (middle_row.min.as_secs_f64() / predicted - 1.0) * 100.0,
        );
    }
    println!(
        "  R255 measured this row at 3.82x to 4.30x, because `observe` read the registry's nearest\n  \
         deadline before and after every look and the pane list calls it once per pane — 2N^2\n  \
         tracker visits per client wake. R256 asks the O(1) question instead (only the observed\n  \
         pane's tracker can have changed, so only its deadline can have moved the minimum), and\n  \
         this row exists now to say the walk stays gone. `tests/agent_cost.rs` is the half that\n  \
         goes red; a ratio climbing back toward 4x here is the same news arriving as a number."
    );

    println!(
        "\nMEASURED — the settle waker's SWEEP, the cost nobody asks for: one pass every five\n\
         seconds, for the life of the daemon, over a workspace where nothing is happening."
    );
    // What one sweep costs, composed from the terms rather than measured whole: the per-pane
    // question once per pane, the whole-registry read TWICE (once inside the park to choose a
    // sleep, once after it to decide whether anything is due), the prune once, the census once,
    // and the manifest read once. The locks the walk takes are excluded, and why is said above.
    let sweep_pass = |i: usize| {
        per_pane[i].1.min * u32::try_from(per_pane[i].0).expect("a small count")
            + per_park[i].1.min * 2
            + per_prune[i].1.min
            + per_census[i].1.min
            + refresh_present.min
    };
    for (i, &remembered) in REGISTRY_SIZES.iter().enumerate() {
        let pane = if remembered == 1 { "pane" } else { "panes" };
        budget(&format!("one sweep, {remembered} {pane}"), sweep_pass(i));
    }
    println!(
        "  per pane, per sweep: {:.3} us to ask whether this one owes an evaluation ({} kept) —\n  \
         and the answer in a quiet workspace is no, so the screen read behind it never happens.\n  \
         {:+.2} ns per remembered pane across a {}x span, which is the flat this row wants: the\n  \
         question is three hash lookups under one lock and none of them walks anything.",
        micros(per_pane[0].1.min),
        per_pane[0].0,
        (per_pane[2].1.min.as_secs_f64() - per_pane[0].1.min.as_secs_f64())
            / (per_pane[2].0 - per_pane[0].0) as f64
            * 1e9,
        per_pane[2].0 / per_pane[0].0.max(1),
    );
    println!(
        "  per WAKE, twice: `any_due` at {:.3} us over {} panes against {:.3} us over {} — {:.2}x\n  \
         for {}x the registry, which is the WALK it is supposed to be. `park_until_due` performs\n  \
         the same read to choose its sleep, and the answer cannot be carried across the park\n  \
         because a candidate appearing is exactly what cuts the sleep short.",
        micros(per_park[2].1.min),
        per_park[2].0,
        micros(per_park[0].1.min),
        per_park[0].0,
        per_park[2].1.min.as_secs_f64() / per_park[0].1.min.as_secs_f64(),
        per_park[2].0 / per_park[0].0.max(1),
    );
    // The rows above are DURATIONS and this box's durations move 20-30% between runs. The claim
    // underneath them — that `any_due` walks the whole registry and the per-pane question walks
    // nothing — is a COUNT, and a count is exact. So it is checked rather than illustrated: every
    // visit inside this window has to be accounted for by an `any_due` row's calls times its
    // registry size, with nothing left over for `owes_evaluation`, `retain_live` or the census.
    let visits = park_after.deadline_visits_total - park_before.deadline_visits_total;
    let expected: u64 = per_park
        .iter()
        .map(|(remembered, row)| calls(row) * remembered)
        .sum();
    println!(
        "  the walk is METERED, not inferred: {visits} tracker visits over this whole block\n  \
         against {expected} predicted by `any_due` alone ({}) — so the per-pane question, the\n  \
         prune and the census contributed NONE, which is the half a duration cannot show. Read\n  \
         from `sprag_host::agent::work`, the counter `tests/agent_cost.rs` gates the pane list\n  \
         with.",
        if visits == expected {
            "exact"
        } else {
            "MISMATCH — something else is scanning"
        },
    );
    println!(
        "  and the CENSUS is not the free by-product `retain_live`'s docs call it: building the\n  \
         daemon-wide live set costs {:.3} us at {} panes against {:.3} us for the prune it\n  \
         exists to serve — {:.1}x the operation, and the largest single term in a sweep over a\n  \
         big workspace. Free in the sense that matters (it needs no walk of its own) and not in\n  \
         the sense the word suggests.",
        micros(per_census[2].1.min),
        per_census[2].0,
        micros(per_prune[2].1.min),
        per_census[2].1.min.as_secs_f64() / per_prune[2].1.min.as_secs_f64(),
    );
    println!(
        "  THE TERM THE COST ARGUMENT NEVER HAD: the manifest re-read, {:.3} us with a file and\n  \
         {:.3} us with none — the same as asking {:.0} panes whether they owe an evaluation, on a\n  \
         daemon that may have three. R254 put it on this thread and priced the SCHEDULING (no new\n  \
         thread, no new timer, no new wake), which is true and is a different claim; the sweep's\n  \
         own cost paragraph priced the WORK and was written a slice earlier. The durability saver\n  \
         it compares itself against reads no file at all — it writes when the shape moved and is\n  \
         otherwise silent — so this is the one recurring term with no counterpart in the thing the\n  \
         marginal-cost comparison was made against.",
        micros(refresh_present.min),
        micros(refresh_absent.min),
        refresh_present.min.as_secs_f64() / per_pane[0].1.min.as_secs_f64(),
    );
    println!(
        "  SO, AND THE SHAPE IS THE POINT: at {} pane the sweep is {:.2} us and the manifest read\n  \
         is {:.0}% of it; at {} it is {:.2} us and the read is {:.0}%. The term the argument\n  \
         enumerated (a walk over the panes) is the one that scales, and it is not what a sweep\n  \
         costs on the workspaces people actually have — a daemon with three panes spends almost\n  \
         its entire sweep reading a config file. Against the five-second period even the big case\n  \
         is {:.5}% of one core, so nothing here asks to be changed; what asked to be changed was\n  \
         a paragraph that named the small term and omitted the large one.",
        per_pane[0].0,
        micros(sweep_pass(0)),
        refresh_present.min.as_secs_f64() / sweep_pass(0).as_secs_f64() * 100.0,
        per_pane[2].0,
        micros(sweep_pass(2)),
        refresh_present.min.as_secs_f64() / sweep_pass(2).as_secs_f64() * 100.0,
        sweep_pass(2).as_secs_f64() / SWEEP_INTERVAL.as_secs_f64() * 100.0,
    );

    if socket_rows.is_empty() {
        println!("\nSKIPPED — the socket rows: no `sprag-term` was found beside this binary.");
    } else {
        println!(
            "\nMEASURED — THE SOCKET, the round trip every row above excludes. A client's wall\n\
             clock is the daemon's work plus the transport plus the client's own parse; the first\n\
             and third are measured, so the middle is what is left."
        );
        for ((name, wire, parse, wire_bytes, in_process_bytes), served) in
            socket_rows.iter().zip(socket_in_process)
        {
            let transport = wire
                .min
                .saturating_sub(served.min)
                .saturating_sub(parse.min);
            budget(&format!("{name}: over the socket"), wire.min);
            budget("  of which the daemon's own work", served.min);
            budget("  of which the client's parse", parse.min);
            budget("  LEFT OVER: the transport", transport);
            println!(
                "  {:.1}x the in-process row. Replies {} B over the wire against {} B in\n  \
                 process{} — the check that the two hosts are answering the same question.",
                wire.min.as_secs_f64() / served.min.as_secs_f64(),
                wire_bytes,
                in_process_bytes,
                if wire_bytes.abs_diff(*in_process_bytes) * 20 > *in_process_bytes {
                    ", WHICH DIFFER by more than 5%, so this row compares two answers"
                } else {
                    ""
                },
            );
        }
        // The two rows are 14 B and ~900 B, so together they separate the FIXED cost of a round
        // trip from the per-byte one — and the per-byte figure is then what prices the one place
        // the equivalence check is not exact.
        if socket_rows.len() >= 2 {
            let (_, small_wire, small_parse, small_bytes, _) = &socket_rows[0];
            let (_, large_wire, large_parse, large_bytes, large_in_process) = &socket_rows[1];
            let small_transport = small_wire
                .min
                .saturating_sub(socket_in_process[0].min)
                .saturating_sub(small_parse.min);
            let large_transport = large_wire
                .min
                .saturating_sub(socket_in_process[1].min)
                .saturating_sub(large_parse.min);
            let per_byte = (micros(large_transport) - micros(small_transport))
                / (*large_bytes as f64 - *small_bytes as f64);
            println!(
                "  A ROUND TRIP IS MOSTLY FIXED COST: {:.1} us for {} bytes and {:.1} us for {}, so\n  \
                 {:.1} us of it is per-request and the rest scales at about {:.0} ns per byte —\n  \
                 which is some fifty times slower than this box copies memory, so the size term is\n  \
                 per-MESSAGE handling on both ends rather than bandwidth. That slope also prices\n  \
                 the one place the two hosts are not identical: their pane LABELS differ, {} B\n  \
                 against {} B, and {} bytes at that slope is {:.1} us — inside the run-to-run band,\n  \
                 so the divergence flagged above is not what this row is measuring.",
                micros(small_transport),
                small_bytes,
                micros(large_transport),
                large_bytes,
                micros(small_transport),
                per_byte * 1e3,
                large_bytes,
                large_in_process,
                large_bytes.abs_diff(*large_in_process),
                per_byte * large_bytes.abs_diff(*large_in_process) as f64,
            );
        }
        println!(
            "  A CLIENT PAYS ALL THREE. Every other number in this tool is the daemon's share, and\n  \
             that bound is now a figure rather than a disclaimer. The transport is the floor under\n  \
             any latency a person can feel and it is paid once per request however small the\n  \
             request is — which is the argument for the fetch gate (R220) and for `waitFor` over\n  \
             polling: both cut the NUMBER of round trips, and neither could have cut the cost of\n  \
             one. It also puts every daemon-side figure in this tool in proportion: the agent\n  \
             evaluation, the sweep, the projection and the gate are all changes to a term that is\n  \
             a fraction of what the wire under it charges."
        );
    }

    println!(
        "\nMEASURED — what the sweep's LOCKS cost, which is what they make a READER wait. The\n\
         sweeper runs CONTINUOUSLY here, not once every five seconds, so this is the worst case a\n\
         request could ever meet rather than the one it will."
    );
    println!(
        "  {:<42} {:>9} {:>9} {:>9} {:>7} {:>8}",
        "condition (reader = pane-list request)", "median", "p99", "max", "passes", "evald"
    );
    for (label, samples, work) in [
        ("no second thread at all", &free, (0, 0)),
        (
            "  sweeping a PRIVATE registry",
            &quiet_private,
            quiet_private_work,
        ),
        (
            "  sweeping the reader's own",
            &quiet_shared,
            quiet_shared_work,
        ),
        (
            "new rules before EVERY request, no thread",
            &stale_none,
            (0, 0),
        ),
        (
            "rules churning + a PRIVATE registry",
            &stale_private,
            stale_private_work,
        ),
        (
            "rules churning + the reader's own",
            &stale_shared,
            stale_shared_work,
        ),
    ] {
        println!(
            "  {label:<42} {:>8.1}u {:>8.1}u {:>8.1}u {:>7} {:>8}",
            micros(percentile(samples, 0.50)),
            micros(percentile(samples, 0.99)),
            micros(samples.last().copied().unwrap_or_default()),
            work.0,
            work.1,
        );
    }
    println!(
        "  The last two columns are THE CONTROL ON THE PROBE: passes the background thread\n  \
         actually ran, and panes it actually evaluated. A thread that never got scheduled would\n  \
         produce the same reader distribution as a free lock, and the conclusion would be about\n  \
         this harness rather than about the daemon. A quiet condition must show passes and no\n  \
         evaluations; a churning one must show both.",
    );
    println!(
        "  AND THE HOLD ITSELF, timed alone: a churning pass over {} panes is {:.1} us, a quiet\n  \
         one {:.2} us. That is the whole pass across every window; a reader waits at most for the\n  \
         ONE window it wants, so this is an upper bound on the wait, not the wait.",
        churn_pass.1,
        micros(churn_pass.0),
        micros(quiet_pass),
    );
    println!(
        "  READ THE TAIL, NOT THE MINIMUM, and read the PAIRS. Every other row in this tool is\n  \
         estimated by its minimum; here that is the sample that did not collide, which reports no\n  \
         cost whatever the truth is. And each sweeping condition has a control that does the SAME\n  \
         work on a private registry, because the first version of this attributed to locks what was\n  \
         mostly two other things: a churning ruleset makes the READER's own request evaluate every\n  \
         pane, and a second thread burns a core whether or not it shares a lock.",
    );
    println!(
        "  SO, FOR A QUIET PASS — the one that runs every five seconds forever — SHARED minus its\n  \
         PRIVATE control is {:+.1} us on the median and {:+.1} us at p99. And the rate that was\n  \
         measured at: {} passes while the reader ran for {:.0} ms, against ONE pass per {:.0}\n  \
         seconds in a daemon — {:.0} MILLION times the real duty cycle. That is the answer R260\n  \
         declined to give: the locks of the recurring pass cost a concurrent reader nothing this\n  \
         instrument can see, at a rate no daemon will ever produce.",
        micros(percentile(&quiet_shared, 0.50)) - micros(percentile(&quiet_private, 0.50)),
        micros(percentile(&quiet_shared, 0.99)) - micros(percentile(&quiet_private, 0.99)),
        quiet_shared_work.0,
        quiet_shared.iter().sum::<Duration>().as_secs_f64() * 1e3,
        SWEEP_INTERVAL.as_secs_f64(),
        quiet_shared_work.0 as f64
            / (quiet_shared.iter().sum::<Duration>().as_secs_f64() / SWEEP_INTERVAL.as_secs_f64())
            / 1e6,
    );
    println!(
        "  THE CHURNING PAIR CANNOT BE READ THE SAME WAY, and the `evald` column is why: the\n  \
         private sweeper evaluated {} panes over {} passes — three each, every pass — while the\n  \
         shared one evaluated {} over {}. It is not doing less work; the READER is doing that work\n  \
         instead, because a pane-list request runs the same detector under the same clock and\n  \
         whichever thread arrives first pays. Sharing the registry changes WHO evaluates, not only\n  \
         who waits, so the two conditions cannot be matched and their difference is not a lock.\n  \
         This is R255's shape again: the comparison cannot be resolved at the level it was asked.",
        stale_private_work.1, stale_private_work.0, stale_shared_work.1, stale_shared_work.0,
    );
    println!(
        "  SO THE CHURNING CASE IS BOUNDED DIRECTLY INSTEAD, by the pass's own duration above:\n  \
         {:.0} us for three panes against {:.2} us quiet — {:.0}x, which is the evaluation and\n  \
         nothing else. A reader wants ONE window, so what it can wait for is that window's share.\n  \
         The reader's own re-evaluation is the other half and is not a lock at all: {:+.1} us on\n  \
         its median with no second thread in the process, and that is the upper bound of it —\n  \
         every request meets new rules there, where a real reload happens once.",
        micros(churn_pass.0),
        micros(quiet_pass),
        churn_pass.0.as_secs_f64() / quiet_pass.as_secs_f64(),
        micros(percentile(&stale_none, 0.50)) - micros(percentile(&free, 0.50)),
    );
    println!(
        "  WHY A CHURNING PASS IS A DIFFERENT OBJECT: a quiet pass holds each workspace lock only\n  \
         for as long as its panes take to answer a hash-lookup question ({:.3} us each). A pass\n  \
         where every pane owes an evaluation holds it across the whole evaluation of every pane in\n  \
         that window — {:.1} us each — so a reader waiting on that lock waits for all of them. That\n  \
         pass is scheduled by a user saving `config.toml` and by the first pass after boot. It is a\n  \
         one-off, which is why this is documented rather than redesigned; the number is worth\n  \
         knowing because a future slice that makes evaluations dearer inherits it.",
        micros(per_pane[0].1.min),
        micros(detect_shell.min),
    );

    println!(
        "\nONE RUN IS NOT A MEASUREMENT. The same code moves 20-30% between runs of this tool on\n\
         this box, and sampling harder inside a run does not shrink it. Repeat and quote the\n\
         range. Every conclusion above survives that band by two orders of magnitude or more; a\n\
         claim that would not is not a claim this instrument can support."
    );

    ExitCode::SUCCESS
}
