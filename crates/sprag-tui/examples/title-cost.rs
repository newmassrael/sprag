//! What the per-paint TITLE WALK costs this client — the last path in the project that no
//! instrument reached (R262 registered it; this is the harness it said was missing).
//!
//! ## Why it is here and not in `sprag-latency`
//!
//! `sprag-latency` is a `sprag-host` binary, and the thing under test is
//! `HostClient::pane_agents` over a live [`WireHost`] — which lives in `sprag-client`, on the
//! far side of `sprag-host`'s dependency edge. A tool cannot reach up. So the measurement lives
//! where its subject does, and pays for that with a daemon of its own.
//!
//! ## What is being measured, and why the shape matters more than the number
//!
//! `retitle` runs on every repaint, which for this client is every KEYSTROKE (R246 measured the
//! release paint at about five milliseconds). The equality skip that makes the title cheap is at the
//! OSC — [`sprag_tui::title_change`] — and NOT at the walk, so the walk happens whether or not the
//! answer moved. What that walk costs is a question about magnitudes, which is what this answers —
//! against the paint it rides on rather than against zero.
//!
//! ## What R265 changed under it
//!
//! Until R265 the walk was `pane_ids()` for the id list and then `pane_agent(id)` per id: N+1 cache
//! locks, and each of those a LINEAR SCAN, because the cache was a `Vec` whose own doc justified
//! that with "the small pane set" — the premise R264 had just removed. It is now one
//! `HostClient::pane_agents` call: one lock, one pass over a cache addressed by `PaneId`.
//!
//! The reason was never the microseconds. The poll thread REPLACES that cache wholesale, so a walk
//! holding the lock N+1 times could pair one generation's pane list with another's verdicts —
//! dropping a pane that went away and missing one that arrived. The numbers below are what the
//! correctness fix also bought.
//!
//! It also answered a question nobody asked, because building it was the first time anything in this
//! project put sixty-odd panes in one session: **a client could not attach to a session with more
//! than 62 panes.** R264 removed that ceiling; the count it was found at stays here as the one
//! these numbers were taken at. See [`PANE_COUNTS`].
//!
//! ## Running it
//!
//! ```text
//! cargo build --release -p sprag-host --bins
//! cargo run --release -p sprag-tui --example title-cost
//! ```
//!
//! A debug build is refused: it is not the code that ships, and R246 is the reason this project
//! knows the difference matters here (about 124 ms per keystroke in debug against about 5 in
//! release).

use std::hint::black_box;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pinion_core::QuitSink;
use sprag_client::WireHost;
use sprag_host::HostClient;
use sprag_tui::{agent_window_title, title_change};

/// Pane counts the walk is measured at. One is the floor; sixty-two was the CEILING, and is not a
/// round number by choice — see below.
///
/// # The 62 was a measured limit, and it is now history
///
/// This harness was written with 64 at the top and 64 did not boot: `WireHost::spawn_or_attach`
/// failed with serde_json's `recursion limit exceeded`. The cause was measured rather than
/// guessed — the daemon's layout slot was a NESTED binary tree whose JSON depth tracked the pane
/// count, so 63 panes crossed serde_json's default limit of 128 and the 63rd pane did not degrade
/// a client, it stopped it attaching at all.
///
/// **R264 flattened that wire shape** (`sprag_terminal::MAX_LAYOUT_DEPTH`), so the arrangement's
/// depth is now a constant and no pane count bounds what a client can read. Pass a larger count on
/// the command line and it attaches — which is how the repair was confirmed against a real daemon
/// rather than in a unit test alone.
///
/// 62 stays the top row for one reason only: R262 and R263's figures were taken there, and a
/// number is comparable to the one beside it or to nothing.
const PANE_COUNTS: [usize; 3] = [1, 8, 62];

/// The counts to measure: [`PANE_COUNTS`], or whatever the command line names instead — which is
/// what found the boot ceiling above rather than a guess about where it was, and what confirms it
/// is gone.
fn pane_counts() -> Vec<usize> {
    let named: Vec<usize> = std::env::args()
        .skip(1)
        .filter_map(|a| a.parse().ok())
        .collect();
    if named.is_empty() {
        PANE_COUNTS.to_vec()
    } else {
        named
    }
}

/// Samples per subject; odd, so the median is a sample the machine produced.
const SAMPLES: usize = 101;

/// Calls inside one timed span, so the span is large against the clock's own floor.
const REPEATS: u32 = 200;

/// The paint this walk rides on, from R246 — the number every figure below is read against. A
/// duration measured elsewhere and quoted here is a citation, not a measurement, and is labelled
/// as one in the output.
const R246_RELEASE_PAINT: Duration = Duration::from_millis(5);

/// A client that never quits — this harness owns its own lifetime.
struct NeverQuits;
impl QuitSink for NeverQuits {
    fn request_quit(&self) {}
}

/// The daemon this example spawns, killed and unlinked on the way out.
///
/// `WireHost` deliberately does NOT own the daemon's lifetime ("the session survives this GUI"), so
/// a harness that spawns one has to reap it itself or leave a `sprag-term` and a socket behind on
/// every run.
struct Daemon(PathBuf);
impl Drop for Daemon {
    fn drop(&mut self) {
        // The daemon self-daemonized, so there is no child handle to wait on. End it the way
        // `sprag kill-server` does: kill every session, which takes its last pane with it, and a
        // daemon with nothing left to serve exits. The last kill severs the connection — that is
        // the success case, not an error.
        if let Ok(mut conn) = sprag_rpc::HostConn::connect(&self.0, Duration::from_millis(500)) {
            let names: Vec<String> = conn
                .call(
                    "scene/query",
                    serde_json::json!({
                        "path": sprag_host::mux_action_path(sprag_host::wire::SESSIONS_SLOT)
                    }),
                )
                .ok()
                .into_iter()
                .flat_map(|value| {
                    value
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|s| s["name"].as_str().map(str::to_owned))
                        .collect::<Vec<_>>()
                })
                .collect();
            for name in names {
                let _ = conn.call(
                    "scene/invoke",
                    serde_json::json!({
                        "path": sprag_host::mux_action_path(
                            sprag_host::wire::KILL_SESSION_ACTION
                        ),
                        "args": {"name": name},
                    }),
                );
            }
        }
        std::thread::sleep(Duration::from_millis(100));
        let _ = std::fs::remove_file(&self.0);
    }
}

fn micros(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1e6
}

/// The least-interfered sample of `body`, for the reason `sprag-latency`'s module doc gives: this
/// box is bimodal (P and E cores under a ranging governor), and the minimum converges on one
/// well-defined operating point where an average does not.
fn measure(mut body: impl FnMut()) -> Duration {
    for _ in 0..16 {
        body();
    }
    let mut samples = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let start = Instant::now();
        for _ in 0..REPEATS {
            body();
        }
        samples.push(start.elapsed() / REPEATS);
    }
    samples.sort_unstable();
    samples[0]
}

fn main() -> ExitCode {
    if cfg!(debug_assertions) {
        eprintln!("title-cost: refusing to measure a debug build.");
        eprintln!("Run: cargo run --release -p sprag-tui --example title-cost");
        return ExitCode::from(2);
    }
    // The daemon binary sits one directory up from an example's own — `target/<profile>/examples/`.
    // Named explicitly rather than left to `WireHost`'s sibling lookup, which would miss it from
    // here and silently fall back to whatever `sprag-term` is on PATH.
    let Ok(exe) = std::env::current_exe() else {
        eprintln!("title-cost: cannot locate this binary");
        return ExitCode::FAILURE;
    };
    let Some(daemon_bin) = exe
        .parent()
        .and_then(|d| d.parent())
        .map(|d| d.join("sprag-term"))
    else {
        eprintln!("title-cost: cannot locate a target directory");
        return ExitCode::FAILURE;
    };
    if !daemon_bin.is_file() {
        eprintln!("title-cost: no sprag-term at {}", daemon_bin.display());
        eprintln!("Build it first: cargo build --release -p sprag-host --bins");
        return ExitCode::FAILURE;
    }

    println!("title-cost — the per-paint title walk, against a real WireHost and a real daemon");
    println!(
        "{:<44} {:>10} {:>10} {:>12}",
        "subject", "min", "vs 1 pane", "% of a paint"
    );

    let mut walks: Vec<(usize, Duration)> = Vec::new();
    let mut builds: Vec<(usize, Duration)> = Vec::new();
    let mut wholes: Vec<(usize, Duration)> = Vec::new();
    let mut claimed_wholes: Vec<(usize, Duration)> = Vec::new();
    for (panes, claimed) in pane_counts()
        .into_iter()
        .flat_map(|n| [(n, false), (n, true)])
    {
        let sock = std::env::temp_dir().join(format!(
            "sprag-title-cost-{}-{panes}-{claimed}.sock",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&sock);
        // SAFETY-adjacent note: this process is single-threaded until `WireHost` starts its poll
        // thread, and these two are read by `WireHost` on the line below.
        unsafe {
            std::env::set_var("SPRAG_GUI_HOST_SOCK", &sock);
            std::env::set_var("SPRAG_GUI_HOST_BIN", &daemon_bin);
        }
        let _daemon = Daemon(sock.clone());
        // EVERY PANE AN AGENT, or none. A `cat` pane is claimed by no manifest, so `pane_agent`
        // answers `None` for all of them: no string is cloned and the title never leaves its
        // baseline. That is the EMPTY branch, and measuring only it would price the plumbing and
        // not the work — R259's lesson, at a different surface. The other configuration paints
        // `claude`'s footer fingerprint into every pane, so the daemon's detector claims all of
        // them and the walk pays what it actually costs when there is something to report.
        // Both halves of a real `claude` pane, because the manifest needs both: the FOOTER is
        // what fingerprints the pane, and the TITLE's resting glyph is what any RULE reads. A pane
        // with the footer alone is identified and has no STATE, so `pane_agent` answers `None` and
        // the walk quietly measures the empty branch again — which is what the first version of
        // this did, and what the guard below caught.
        let painter = if claimed {
            "printf '\u{1b}]0;\u{2733} Claude Code\u{7}'; \
             printf '%s' '  \u{23f8} manual mode on \u{b7} ? for shortcuts'; exec cat"
        } else {
            "exec cat"
        };
        let host = match WireHost::spawn_or_attach(
            Some(vec!["/bin/sh".into(), "-c".into(), painter.into()]),
            80,
            24,
            panes,
            Arc::new(|| {}),
            Arc::new(NeverQuits),
        ) {
            Ok(host) => host,
            Err(error) => {
                eprintln!("title-cost: could not bring up a host for {panes} panes: {error}");
                return ExitCode::FAILURE;
            }
        };
        // The walk is only as long as the cache, so a measurement taken before the panes are all
        // there is a measurement of a smaller N wearing this one's label.
        let deadline = Instant::now() + Duration::from_secs(10);
        while host.pane_ids().len() < panes && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let seen = host.pane_ids().len();
        if seen < panes {
            eprintln!("title-cost: only {seen} of {panes} panes came up; skipping that row");
            continue;
        }

        // A verdict resting on an ABSENCE is not published until its settle window closes, and the
        // daemon's sweep is what closes it — so a measurement taken too early prices the EMPTY walk
        // wearing the populated one's label. Waited for rather than slept through.
        if claimed {
            let deadline = Instant::now() + Duration::from_secs(20);
            while Instant::now() < deadline && host.pane_agents().len() < panes {
                std::thread::sleep(Duration::from_millis(100));
            }
            let claimed_now = host.pane_agents().len();
            if claimed_now < panes {
                eprintln!(
                    "title-cost: only {claimed_now} of {panes} panes were claimed; \
                     that row would be the empty branch in disguise"
                );
                continue;
            }
        }
        let walk = measure(|| {
            black_box(black_box(&host).pane_agents());
        });
        let agents = host.pane_agents();
        let session = host.current_session();
        let build = measure(|| {
            black_box(agent_window_title(black_box(&session), black_box(&agents)));
        });
        // The whole of what a paint pays: the walk, the build, and the equality check that
        // discards the answer. `held` starts at the title the build produces, so every measured
        // call takes the SKIP branch — which is the branch every paint but the first takes.
        let mut held = Some(agent_window_title(&session, &agents));
        let whole = measure(|| {
            let agents = black_box(&host).pane_agents();
            let wanted = agent_window_title(&session, &agents);
            black_box(title_change(&mut held, wanted));
        });

        if claimed {
            claimed_wholes.push((panes, whole));
        } else {
            walks.push((panes, walk));
            builds.push((panes, build));
            wholes.push((panes, whole));
        }
        let branch = if claimed {
            "all claimed"
        } else {
            "none claimed"
        };
        for (label, value) in [
            ("walk", walk),
            ("title build", build),
            ("both + the skipped OSC", whole),
        ] {
            println!(
                "{:<44} {:>9.3}u {:>9.2}x {:>11.4}%",
                format!("{panes} pane(s), {branch}: {label}"),
                micros(value),
                value.as_secs_f64() / walks[0].1.as_secs_f64(),
                value.as_secs_f64() / R246_RELEASE_PAINT.as_secs_f64() * 100.0,
            );
        }
    }

    if walks.len() >= 2 && !claimed_wholes.is_empty() {
        let (small_n, small) = walks[0];
        let (large_n, large) = walks[walks.len() - 1];
        let (top_n, top_claimed) = claimed_wholes[claimed_wholes.len() - 1];
        let (_, top_empty) = wholes[wholes.len() - 1];
        println!(
            "\n  READ THE CLAIMED ROWS. A workspace of `cat` panes is the EMPTY branch: no manifest\n  \
             claims them, so nothing is cloned and the title never leaves its baseline. At {top_n}\n  \
             panes that branch is {:.2} us and the populated one is {:.1} us — {:.0}x. Measuring\n  \
             only the cheap branch would have understated this whole path by an order of magnitude,\n  \
             which is the shape R259 recorded at a different surface.",
            micros(top_empty),
            micros(top_claimed),
            top_claimed.as_secs_f64() / top_empty.as_secs_f64(),
        );
        println!(
            "  AND THE WALK IS NOT THE LARGER HALF, which is the opposite of what reading the code\n  \
             suggested, and is still true after R265 removed the quadratic term: the empty-branch\n  \
             walk now grows {:.1}x for {}x the panes ({:.3} us at {small_n} against {:.3} us at\n  \
             {large_n}) — LINEAR — while on a populated workspace the DIGEST STRING BUILD is still\n  \
             comparable to or larger than the walk it feeds. What is left in the walk itself is the\n  \
             per-agent String clone, which is what handing owned data out from behind a lock COSTS.",
            large.as_secs_f64() / small.as_secs_f64(),
            large_n / small_n.max(1),
            micros(small),
            micros(large),
        );
        println!(
            "  AGAINST THE PAINT IT RIDES ON: {:.1} us at {top_n} panes with every pane an agent is\n  \
             {:.3}% of R246's ~5 ms release paint (a CITED number, taken in another round on this\n  \
             box, not re-measured here). The equality skip is at the OSC and not at the walk, so all\n  \
             of this happens on every keystroke and is then discarded — and at well under a percent\n  \
             of a keystroke, that is a fact to record rather than a thing to fix. A gate in front of\n  \
             the walk would buy that percent and cost a second piece of state that has to stay true.\n  \
             R264 removed the wire's 62-pane cap, so nothing bounds this count any more; what makes\n  \
             the conclusion safe at a larger one is that R265 made the growth LINEAR, not that the\n  \
             number here is small.",
            micros(top_claimed),
            top_claimed.as_secs_f64() / R246_RELEASE_PAINT.as_secs_f64() * 100.0,
        );
    }
    ExitCode::SUCCESS
}
