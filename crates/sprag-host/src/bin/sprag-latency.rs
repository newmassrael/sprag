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
//! ## What is deliberately NOT here
//!
//! Hardware counters. Retired instructions would be near-deterministic and gate-able, but this box
//! reports `kernel.perf_event_paranoid = 4`, so `perf_event_open` is unavailable without an
//! operator changing a sysctl — and, more to the point, a count of instructions is a proxy for
//! time, not time. The question this round owes an answer to is a latency budget.
//!
//! The socket is also not here. Every request below is served IN-PROCESS through
//! [`handle_request`], the same entry point the transport calls, so these are the daemon's costs
//! with the wire's cost excluded. That bound is stated rather than hidden: a client's wall-clock
//! adds a round trip nobody has priced.
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

use std::hint::black_box;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use sprag_detect::{DEFAULT_SETTLE, Hysteresis, Ruleset, Tracker, built_ins, detect};
use sprag_grid::{project, projection_token};
use sprag_host::{
    AgentClock, CellFrame, ChannelRegistry, Host, HostState, PaneScrollFacts, handle_request,
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
        "\nONE RUN IS NOT A MEASUREMENT. The same code moves 20-30% between runs of this tool on\n\
         this box, and sampling harder inside a run does not shrink it. Repeat and quote the\n\
         range. Every conclusion above survives that band by two orders of magnitude or more; a\n\
         claim that would not is not a claim this instrument can support."
    );

    ExitCode::SUCCESS
}
