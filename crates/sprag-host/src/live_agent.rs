//! **DEBT 64c — THE ai-loop PATH, MEASURED AGAINST A LIVE AGENT CLI.**
//!
//! Every number this front carries was taken against a stand-in. R375 drove a shell that sleeps,
//! R377 a supervised fake whose observation a `Mutex` moved by hand, R378 a `/bin/sh` reading lines.
//! All three answer in **milliseconds**, and the thing they stand in for takes **tens of seconds** —
//! so the contracts that decide when a turn is over have never been asked about a peer that behaves
//! like the one they were designed for.
//!
//! # ⚠⚠⚠ Why this lives in `sprag-host` and not beside the code it measures
//!
//! Because the SUPERVISOR does. `DoneWhen::Settles`, `Over` and the readiness barrier all rest on
//! one thing — [`AgentObservation`](sprag_plugin::AgentObservation) — and the only real producer of
//! one in this workspace is [`agent_state_source`](crate::plugins::agent_state_source): the
//! `sprag-detect` ruleset, the per-pane [`Tracker`](sprag_detect::Tracker), the hysteresis and the
//! settle window, reading a live screen and a live title.
//!
//! A measurement written in `sprag-plugin` has to invent that, and an invented one agrees with the
//! product by construction — R350's rule about stand-ins that parse by pattern, met from the other
//! side. **A fixture supervisor is precisely what debt 64c says has never been got past.**
//!
//! # ⚠⚠ What makes this safe to run
//!
//! A live agent is a program that edits files. It is spawned in a **scratch directory of this
//! measurement's own**, removed when the gate ends, so the worst a confused turn can do is write
//! there. The `CLAUDE_CODE_*` variables this process was started with are blanked, so the child is
//! the thing a person gets from a terminal rather than a nested child session.
//!
//! # How to run it
//!
//! Both gates are `#[ignore]`d: they cost real agent turns, need credentials, and take minutes.
//!
//! ```sh
//! cargo test -p sprag-host --lib live_agent -- --ignored --nocapture
//! ```
//!
//! ⚠ `--nocapture` is not decoration. **What these gates are for is the WALK** — R378's lesson,
//! which cost that round five wrong readings behind one green ending — so each prints what it saw,
//! step by step, and asserts only the things a wrong reading could not survive.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sprag_plugin::access::WorkspacePaneAccess;
use sprag_plugin::{
    Attended, Completion, Delivered, Delivery, DoneWhen, KeyStroke, Over, PaneAccess, Reached,
    Readiness, ReadyWhen, RunContext, SubmittedWhen, deliver,
};
use sprag_terminal::{CommandBuilder, Pane, PaneId, Workspace};

use crate::plugins::agent_state_source;

/// The env var naming the agent CLI to drive.
///
/// A knob rather than a constant because the claim these gates make is about *a real interactive
/// agent CLI*, and this workspace's detector ships manifests for two of them. Whoever runs it says
/// which one they have.
const AGENT_PROGRAM: &str = "SPRAG_LIVE_AGENT";

/// What [`AGENT_PROGRAM`] defaults to — the agent this project's built-in manifest was measured
/// against (`sprag_detect::claude`).
const DEFAULT_AGENT: &str = "claude";

/// How wide and tall the live pane is.
///
/// ⚠ NOT 80x24. An agent CLI lays its composer, footer and dialogs out against the terminal it is
/// given, and every rule in the built-in manifest was written off screens captured at a working
/// size. A cramped pane is a different program to look at, and a measurement taken in one would be
/// about the reflow.
const PANE_SIZE: (u16, u16) = (120, 40);

/// The longest a live agent's turn may take before this measurement gives up on it.
///
/// ⚠ It is the BOUND, not the expectation — the whole point of these gates is that nobody knows the
/// expectation yet. `Turn`'s doc tells a caller to size it to the peer, and a real caller's would be
/// minutes; this one is short because **a contract that cannot be satisfied pays it in full every
/// time**, and a measurement is not improved by waiting longer to be told the same thing.
const TURN_BOUND: Duration = Duration::from_secs(20);

/// How long the agent has to come up before the barrier gives up.
const STARTUP_BOUND: Duration = Duration::from_secs(120);

/// A directory the live agent may do whatever it likes in, removed when this drops.
struct Scratch(PathBuf);

impl Scratch {
    /// Make one, named after this process so two concurrent runs cannot collide.
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.subsec_nanos());
        let path =
            std::env::temp_dir().join(format!("sprag-live-{tag}-{}-{nanos}", std::process::id(),));
        std::fs::create_dir_all(&path).expect("a scratch directory for the live agent");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// One live agent session: a pane, the workspace holding it, and pane access carrying **the real
/// detector**.
struct Live {
    workspace: Arc<Mutex<Workspace>>,
    access: WorkspacePaneAccess,
    /// **THE ONE TRACKER**, kept so a second reader can be built that shares it.
    ///
    /// ⚠ Two `AgentClock`s over one pane would be two memories and two `seq` counters, and the
    /// sampler beside the contract would then be reading a different pane's history than the
    /// contract is. The daemon has exactly one per host for the same reason.
    agents: Arc<crate::AgentClock>,
    pane: PaneId,
    agent: String,
    /// Held so the directory outlives the pane that is running in it.
    /// ⚠ Named rather than `_scratch` since R382: the gate that drives a milestone which WRITES A
    /// FILE has to look in it. That look is also the only thing in this module that has ever
    /// checked the premise its whole safety argument rests on — *the agent runs in here* — which
    /// was registered as owed and asserted by nothing.
    scratch: Scratch,
}

impl Live {
    /// Spawn the agent named by [`AGENT_PROGRAM`] in a scratch directory of its own.
    fn start(tag: &str) -> Self {
        Self::start_args(tag, &[])
    }

    /// The same, but with the agent NAMED THE WAY A DAEMON NAMES IT — through a
    /// [`PaneArgsSource`](sprag_terminal::PaneArgsSource) consulted at every birth, rather than
    /// through argv this side wrote.
    ///
    /// Each identity it mints is pushed to the returned log, so a gate can say what the run was
    /// called without the source having to tell it. The source carries **only** the identity
    /// decision — not `crate::pane_args_source`'s hooks — because a hook pointing at a daemon that
    /// is not running would be a second variable in a measurement about naming.
    fn start_minting(tag: &str) -> (Self, Arc<Mutex<Vec<String>>>) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&log);
        let source: sprag_terminal::PaneArgsSource = Arc::new(move |argv: &[String]| {
            let minted = crate::hooks::CLAUDE
                .identity_args(argv, crate::hooks::mint_session_id)
                .unwrap_or_default();
            if let Some(name) = minted.last() {
                recorder.lock().expect("the log").push(name.clone());
            }
            minted
        });
        (Self::start_inner(tag, &[], Some(source)), log)
    }

    /// The same, plus `extra` arguments on the agent's command line.
    ///
    /// ⚠ A seam rather than a convenience: the one gate that uses it is asking whether an argument
    /// CHANGES WHAT THE AGENT WRITES ABOUT ITSELF, so the argument has to reach the same spawn
    /// every other gate here measures. A second `start` with its own command would be measuring a
    /// different program from the one the rest of this module drives.
    fn start_args(tag: &str, extra: &[&str]) -> Self {
        Self::start_inner(tag, extra, None)
    }

    fn start_inner(
        tag: &str,
        extra: &[&str],
        args_source: Option<sprag_terminal::PaneArgsSource>,
    ) -> Self {
        let agent = std::env::var(AGENT_PROGRAM).unwrap_or_else(|_| DEFAULT_AGENT.to_owned());
        let scratch = Scratch::new(tag);
        let workspace = Arc::new(Mutex::new(Workspace::new(PANE_SIZE)));
        if let Some(source) = args_source {
            workspace
                .lock()
                .expect("the workspace mutex")
                .set_pane_args_source(source);
        }

        let mut command = CommandBuilder::new(&agent);
        command.cwd(scratch.path());
        for argument in extra {
            command.arg(argument);
        }
        // ⚠⚠⚠ **THE CHILD MUST NOT INHERIT WHOEVER RAN THIS GATE'S PERMISSION ALLOWLIST**, and
        // this is the same argument as the blanked variables below rather than a new one.
        //
        // MEASURED, which is the only reason it is here: the gate that drives a milestone which
        // WRITES A FILE reported `dialogs answered: 0` — the agent created the file and never
        // asked, because this developer's `~/.claude/settings.json` allows `Write(*)`, `Bash(*)`
        // and `Read(*)`. So the run's permission path was decided by a file nobody in this
        // measurement wrote, and the same gate on another machine would take a different path and
        // report the same green. **A fixture whose behaviour is read off the developer's home
        // directory is a fixture asserting their configuration.**
        //
        // ⚠ `project` and not the empty string: it names a source that exists, and the scratch
        // directory has no project settings in it — so the effective posture is *this agent's own
        // defaults*, which is what a person meets on a machine they have not configured.
        // ⚠ It is about SETTINGS and not about credentials, which are read from elsewhere; the
        // sessions this spawns still authenticate exactly as before.
        command.arg("--setting-sources");
        command.arg("project");
        // A real terminal, because the agent renders a TUI and the detector reads what it renders.
        command.env("TERM", "xterm-256color");
        // ⚠ THE CHILD MUST BE THE THING A PERSON GETS FROM A TERMINAL. This process is itself an
        // agent session, and it exports variables that tell a child it is nested. Blanked rather
        // than inherited, so the measurement is of the program under test rather than of a mode
        // only this harness could produce. (`CommandBuilder` adds to the inherited environment; it
        // has no unset, and an empty value is falsy to everything that reads these.)
        //
        // ⚠⚠⚠ THE LIST IS THE PRODUCT'S NOW ([`NESTED_AGENT_MARKERS`]), and it was this harness's
        // alone for four rounds — which is why every gate here measured a correctly-launched agent
        // while a real user's pane inherited the markers and its agent wrote no transcript. **A
        // barrier only the harness clears is a barrier the product does not have.** Reading the
        // same constant is what stops the two drifting again.
        for nested in crate::NESTED_AGENT_MARKERS {
            command.env(nested, "");
        }

        let pane = workspace
            .lock()
            .expect("the workspace mutex")
            .spawn(command, agent.clone(), PANE_SIZE.0, PANE_SIZE.1)
            .unwrap_or_else(|error| {
                panic!(
                    "the live agent {agent:?} must start — this gate was asked for by name, so an \
                     agent that is not installed is a RED rather than a skip. Set {AGENT_PROGRAM} \
                     to the one you have. {error:?}",
                )
            });

        // ⚠ THE SHIPPED SETTLE WINDOW, not `crate::config::agent_settle`. The real reader consults
        // the user's `config.toml`, and R331's rule is that a gate which does that is asserting
        // about a timing it did not choose — on a machine whose owner had tuned it, these numbers
        // would be somebody's preference. This is the default the product ships.
        let agents = Arc::new(crate::AgentClock::new(sprag_detect::Ruleset::new(
            sprag_detect::built_ins(),
        )));
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace)).with_agent_state(Some(
            agent_state_source(Arc::clone(&workspace), Arc::clone(&agents), shipped_settle),
        ));

        Self {
            workspace,
            access,
            agents,
            pane,
            agent,
            scratch,
        }
    }

    /// A SECOND reader of the same pane, through the SAME tracker — see [`Live::agents`].
    fn second_reader(&self) -> WorkspacePaneAccess {
        WorkspacePaneAccess::new(Arc::clone(&self.workspace)).with_agent_state(Some(
            agent_state_source(
                Arc::clone(&self.workspace),
                Arc::clone(&self.agents),
                shipped_settle,
            ),
        ))
    }

    /// What the detector says about the pane right now, as one line.
    fn seen(&self) -> String {
        verdict_of(&self.access, self.pane)
    }

    /// The detector's `seq` for this pane — the number [`DoneWhen::Settles`] compares against.
    fn seq(&self) -> Option<u64> {
        self.access
            .supervision()
            .and_then(|supervisor| supervisor.pane_agent_state(self.pane))
            .map(|seen| seen.seq)
    }

    /// The pane's screen, collapsed — what the agent has actually painted.
    fn screen(&self) -> String {
        self.access.pane_collapsed(self.pane).unwrap_or_default()
    }

    /// The last `rows` non-empty rows of the pane, for a walk line that has to stay readable.
    fn tail(&self, rows: usize) -> String {
        tail_of(&self.access, self.pane, rows)
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        // The agent is a long-lived process; nothing else in this test reaps it. ⚠ The answer is
        // deliberately dropped rather than asserted: this runs while a failing assertion is
        // unwinding, and a panic here would replace that gate's message with this one's.
        //
        // ⚠⚠⚠ EVERY PANE IN THE WORKSPACE, and not `self.pane`. A loop that REFLECTS closes its inner
        // session and opens a fresh one, so the pane this struct was built around is not the one the
        // run ended on — and a `Drop` that named only it would leave a live agent CLI running with
        // nobody's hand on it, once per gate, for as long as the machine is up.
        let mut guard = self.workspace.lock().expect("the workspace mutex");
        let live: Vec<PaneId> = guard.panes().iter().map(Pane::id).collect();
        for pane in live {
            let _closed = guard.close(pane);
        }
    }
}

/// One reading of the pane: what the detector said, and what the pane was showing when it said it.
#[derive(Clone, PartialEq, Eq)]
struct Reading {
    verdict: String,
    /// **THE PANE'S TITLE**, which is what two of `claude`'s three rules actually read.
    ///
    /// ⚠ Sampled because a verdict alone cannot say WHY it is what it is, and the manifest's
    /// working and idle rules are both `Region::Title`. Without this a reading of `Idle` during a
    /// turn is a mystery; with it, it is a claim about one string.
    title: String,
    tail: String,
}

/// **WHAT THE REAL DETECTOR PUBLISHES ACROSS A REAL TURN** — sampled beside the contract, not
/// instead of it.
///
/// # ⚠⚠⚠ Why the sample runs WHILE the contract waits, and through the same tracker
///
/// The question this instrument answers is *what did the thing the contract is reading actually
/// say?*, and the only honest way to ask it is to read the same tracker at the same time. A
/// sampler with a clock of its own would be a second memory with a second `seq`, and its answer
/// would be about a pane nobody was waiting on.
///
/// ⚠ It samples FASTER than the contract polls (`POLL_INTERVAL` is 10 ms). That is deliberate and
/// it biases the reading in the product's favour: a pull-based detector publishes only when it is
/// pulled, so any state a sampler this dense fails to see is one no caller could have seen either.
struct Watch {
    stop: Arc<AtomicBool>,
    worker: std::thread::JoinHandle<Vec<(Duration, Reading)>>,
}

impl Watch {
    /// How many DISTINCT readings are kept.
    ///
    /// ⚠ A cap, so it is reported rather than silently applied — see [`Watch::walk`].
    const KEEP: usize = 48;

    /// Start sampling `live`'s pane every 5 ms.
    fn start(live: &Live) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let access = live.second_reader();
        let workspace = Arc::clone(&live.workspace);
        let pane = live.pane;
        let flag = Arc::clone(&stop);
        let worker = std::thread::spawn(move || {
            let began = Instant::now();
            let mut seen: Vec<(Duration, Reading)> = Vec::new();
            while !flag.load(Ordering::Relaxed) {
                let reading = Reading {
                    verdict: verdict_of(&access, pane),
                    title: title_of(&workspace, pane),
                    tail: tail_of(&access, pane, 2),
                };
                if seen.last().map(|(_, last)| last) != Some(&reading) && seen.len() < Self::KEEP {
                    seen.push((began.elapsed(), reading));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            seen
        });
        Self { stop, worker }
    }

    /// Stop sampling and return the walk, newest last.
    fn walk(self) -> Vec<(Duration, Reading)> {
        self.stop.store(true, Ordering::Relaxed);
        self.worker.join().expect("the sampler thread")
    }
}

/// What the detector says about `pane` right now, as one line.
fn verdict_of(access: &WorkspacePaneAccess, pane: PaneId) -> String {
    match access
        .supervision()
        .and_then(|supervisor| supervisor.pane_agent_state(pane))
    {
        Some(seen) => format!(
            "state={:?} agent={:?} seq={} authority={:?} asking={}",
            seen.state,
            seen.agent,
            seen.seq,
            seen.authority,
            seen.asking
                .map_or_else(|| "-".to_owned(), |question| question.asked.join(" / ")),
        ),
        None => "no observation".to_owned(),
    }
}

/// The pane's title — **the string `claude`'s working and idle rules are both written against**,
/// read the way [`agent_state_source`] reads it (the CHILD's own title, never the pane's name).
///
/// ⚠ Each character is reported with its codepoint. The whole question this measurement turned out
/// to be about is which glyph family an agent animates its title with, and `✳` against `✢` is one
/// pixel of difference in a log and two different codepoints in a regex.
fn title_of(workspace: &Arc<Mutex<Workspace>>, pane: PaneId) -> String {
    let title = workspace
        .lock()
        .expect("the workspace mutex")
        .pane(pane)
        .and_then(sprag_terminal::Pane::title);
    match title {
        Some(title) => {
            let codepoints: String = title
                .chars()
                .take(3)
                .map(|ch| format!("U+{:04X} ", u32::from(ch)))
                .collect();
            format!("{title:?} [{}]", codepoints.trim_end())
        }
        None => "<no title>".to_owned(),
    }
}

/// The last `rows` non-empty rows of `pane`, for a walk line that has to stay readable.
fn tail_of(access: &WorkspacePaneAccess, pane: PaneId, rows: usize) -> String {
    let screen = access.pane_collapsed(pane).unwrap_or_default();
    let lines: Vec<&str> = screen
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .collect();
    lines[lines.len().saturating_sub(rows)..].join(" ⏎ ")
}

/// The settle window the product ships, as the `fn` pointer the source takes.
fn shipped_settle() -> sprag_detect::Hysteresis {
    sprag_detect::Hysteresis {
        settle: sprag_detect::DEFAULT_SETTLE,
    }
}

/// Print one line of the walk, timestamped from `began`.
fn step(began: Instant, what: &str) {
    println!("[{:>7.2}s] {what}", began.elapsed().as_secs_f64());
}

/// ⚠⚠⚠ **THE TWO CONTRACTS, ASKED ABOUT A PEER THAT TAKES SECONDS** — debt 64c's first half.
///
/// The barrier at the START of a turn ([`ReadyWhen::Settles`]) and the contract at its END
/// ([`DoneWhen::Settles`]) are the same observation asked twice, and every existing measurement of
/// them is against a peer whose whole turn is shorter than one poll interval. This asks them about
/// a live agent, and the assertions are the ones a wrong answer could not survive:
///
/// * the barrier clears at all — the real detector names the program in the pane, so `Settles`'
///   agent-name check is satisfied by a name nobody typed into a fixture;
/// * the turn's end costs **more than a moment**, which is the whole reason `Completion` is armed:
///   a contract that answered instantly here would be reporting the peer's PRE-TURN rest as its
///   answer, and would publish the screen from before the model wrote a word;
/// * and it costs **less than the bound**, which is what says the contract fired on the peer's own
///   evidence rather than on the clock.
///
/// ⚠ The middle assertion is the one this workspace has never been able to make. Against every
/// stand-in in `sprag-plugin` a turn is over in milliseconds, so `too fast` and `correct` are the
/// same reading there.
#[test]
#[ignore = "drives a LIVE agent CLI: needs credentials, costs real turns, takes minutes"]
fn a_live_agents_turn_is_ended_by_the_contract_rather_than_by_the_clock() {
    let live = Live::start("turn");
    let run = RunContext::uncancellable();
    let began = Instant::now();
    step(began, &format!("spawned {:?}", live.agent));

    // ── THE BARRIER ──
    let mut barrier = Readiness::new(
        Some(ReadyWhen::Settles(live.agent.clone())),
        Some(STARTUP_BOUND),
        None,
        Attended::NoOne,
    );
    let reached = barrier
        .reached(&live.access, live.pane, &run)
        .expect("the pane must stay readable");
    let startup = began.elapsed();
    step(began, &format!("barrier: {reached:?}"));
    step(began, &format!("detector: {}", live.seen()));
    step(began, &format!("screen: {}", live.tail(3)));
    assert_eq!(
        reached,
        Reached::Yes,
        "⚠⚠⚠ the barrier never cleared for a LIVE {:?}. This is the first thing debt 64c asks and \
         the first thing no stand-in could answer: a fixture's observation is whatever the fixture \
         wrote, and this one comes from `sprag-detect` reading the program's own screen and title. \
         What the pane was showing: {}",
        live.agent,
        live.tail(6),
    );

    // ── THE TURNS ──
    //
    // ⚠⚠⚠ FOUR OF THEM, ALTERNATING WHETHER ANYBODY ELSE IS LOOKING — because the first two runs
    // of this gate disagreed, and the only thing that differed between them was the SAMPLER. A
    // second reader pulling the same tracker is the control: same session, same peer, same prompt
    // shape, and the one variable is whether the detector is being asked more often than the
    // contract asks it. If the two halves disagree, the contract's answer is a function of who
    // else was watching, which is a fact about the product and not about the harness.
    let mut turns: Vec<Turn> = Vec::new();
    for index in 0..4 {
        let sampled = index % 2 == 1;
        turns.push(one_turn(&live, &run, index, sampled, began));
    }

    println!("\n== 64c, first half: {} ==", live.agent);
    println!("  startup to barrier: {startup:?}");
    for turn in &turns {
        println!(
            "  turn {} sampled={:<5} {:>8.2}s  over={:?}  seq {:?} -> {:?}  answered={}",
            turn.index,
            turn.sampled,
            turn.elapsed.as_secs_f64(),
            turn.over,
            turn.seq_before,
            turn.seq_after,
            turn.answered,
        );
    }
    println!();

    for turn in &turns {
        assert_eq!(
            turn.over,
            Over::Yes,
            "⚠⚠⚠ a live turn must end on the contract's own evidence. `NotYet` means the bound \
             ({TURN_BOUND:?}) ran out, and the walk printed above says why: `DoneWhen::Settles` \
             needs the detector to publish a state change (`seq > began_at`), and what a working \
             `claude` gets published as decides whether it ever can. Turn {} (sampled={}), \
             answered={}, seq {:?} -> {:?}",
            turn.index,
            turn.sampled,
            turn.answered,
            turn.seq_before,
            turn.seq_after,
        );
        assert!(
            turn.answered,
            "⚠⚠ and the turn the contract called over is one the agent actually answered — \
             without this every number here is about a peer that went quiet for some other \
             reason. Turn {}",
            turn.index,
        );
        assert!(
            turn.elapsed > Duration::from_millis(500),
            "⚠⚠⚠ THE NUMBER THIS WHOLE GATE EXISTS FOR. A live agent's turn cannot be over in \
             {:?}: the only thing true that fast is the peer's rest from BEFORE the turn, which is \
             what `Completion::begin`'s arming exists to refuse. Every stand-in in this workspace \
             answers inside this window, so this is the first measurement in which `too fast` and \
             `correct` are distinguishable at all. Turn {}",
            turn.elapsed,
            turn.index,
        );
        assert!(
            turn.elapsed < TURN_BOUND,
            "and the CONTRACT must have ended the turn rather than the clock: turn {} took {:?}",
            turn.index,
            turn.elapsed,
        );
    }

    // ⚠⚠⚠ AND THE TWO HALVES MUST AGREE. This is the assertion the first two runs of this gate
    // would have failed, and it is the one no fixture could ever have made: a contract whose
    // verdict depends on whether a SECOND reader happened to be pulling the same detector is not a
    // contract, it is a race — and the outer loop it serves would converge or hang by luck.
    let unsampled: Vec<&Turn> = turns.iter().filter(|turn| !turn.sampled).collect();
    let sampled: Vec<&Turn> = turns.iter().filter(|turn| turn.sampled).collect();
    assert_eq!(
        unsampled
            .iter()
            .filter(|turn| turn.over == Over::Yes)
            .count(),
        unsampled.len(),
        "⚠⚠⚠ THE CONTROL, AND THE POINT: the turns nobody else was watching must end exactly as \
         the watched ones did. {} of {} unwatched turns ended `Yes` against {} of {} watched — a \
         contract that needs a second reader to be satisfied cannot be relied on by the one \
         caller it has.",
        unsampled
            .iter()
            .filter(|turn| turn.over == Over::Yes)
            .count(),
        unsampled.len(),
        sampled.iter().filter(|turn| turn.over == Over::Yes).count(),
        sampled.len(),
    );
}

/// ⚠⚠⚠ **THE OUTER LOOP, PUMPED AGAINST A LIVE AGENT** — debt 64c's second half.
///
/// R378 built the driver and drove it against a `/bin/sh` stand-in. What a stand-in cannot supply
/// is the thing this gate exists for: **a live agent PAINTS the prompt it was given, decorated,
/// into its own transcript** — so every word the loop says to it comes back onto the screen the
/// loop then judges.
///
/// That is not hypothetical. R379 made the document ask the agent for its own `done_marker` (it
/// never had, so no live run could converge at all), and the driver's `said_done` — a whole-screen
/// `contains` — immediately started reading the loop's own instruction as the agent's answer. The
/// unit gate caught it against an undecorated stand-in. **This is the same claim against the echo a
/// real agent CLI actually produces**, which is boxed, prefixed with `❯ ` and re-wrapped, and which
/// this crate's existing echo rule (*"the stimulus contains the row"*) would not have discounted.
///
/// # ⚠⚠ What it deliberately does NOT assert, and where that moved to
///
/// That a live loop CONVERGES. When this gate was written nothing in the product could fill the
/// template in, so the milestone a live agent read was `(edit me) the next checkpoint on the way
/// there` and driving it to convergence would have meant asserting what a model says about a
/// placeholder. **That gap is now closed** — [`OuterLoop::brief`] exists — and the convergence
/// claim is its own gate, [`a_briefed_loop_converges_against_a_live_agent`]. This one stays as it
/// is: it is the ECHO claim, and keeping it on the SHIPPED template is what makes it a claim about
/// the document a person actually gets.
#[test]
#[ignore = "drives a LIVE agent CLI: needs credentials, costs real turns, takes minutes"]
fn the_outer_loop_does_not_converge_on_the_prompt_a_live_agent_paints_back() {
    use sce_rust_runtime::IScriptEngine;
    use sprag_plugin::{AiLoopState, OuterLoop, Pumped};

    let live = Live::start("loop");
    // ⚠⚠⚠ THE RUN'S OWN CLOCK IS THIS GATE'S ONLY BOUND, and that is a consequence of register item
    // 300 rather than a preference. The per-turn bound used to be an `AiLoopSpec` field this gate
    // set to twenty seconds; it is the DOCUMENT's now, this gate deliberately drives the SHIPPED
    // document (see the doc above), and the shipped number is half an hour — a person's allowance
    // for a live session doing real work. A gate cannot sit on that, and it must not answer by
    // re-authoring the file, because then it would no longer be driving what a person gets.
    // ⚠ So the bound moves to the thing every run already has. A stalled agent ends the run and the
    // assertions below say what was missing, where before it would have been half an hour of
    // silence.
    let run = RunContext::uncancellable().deadline_in(Some(TURN_BOUND * 6));
    let began = Instant::now();

    let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
    let mut loops = OuterLoop::new(
        lua,
        live.pane,
        // ⚠ `driving` fixes the two knobs that are true of every agent CLI — the barrier is
        // `settles` on its own name, and it paints the prompt box it is typed into, which is the
        // premise `deliver`'s read-back rests on. Nothing here is this gate's own any more: every
        // field of the spec is a predicate about the peer.
        &sprag_plugin::AiLoopSpec::driving(&live.agent),
    )
    .expect("the document's datamodel must carry its authored strings");

    // ⚠ The marker is a plain `<data>` literal, so it is readable before anything is primed. The
    // PROMPT that carries it is composed on entry to `priming`, which is why its control sits
    // below, at the first move rather than here.
    let marker = loops
        .authored()
        .expect("the document's datamodel must carry its authored strings")
        .done_marker;

    let mut walked = Vec::new();
    let mut turn_began = Instant::now();
    let mut first_turn = None;
    while walked.len() < 8 {
        match loops
            .pump(&live.access, &run)
            .expect("the pane stays readable")
        {
            Pumped::Moved {
                from,
                raised,
                to,
                found,
                because,
                unreadable,
                checked,
                witnessed,
                spent: _,
            } => {
                // ⚠⚠ THE REFUSAL A PASS ARRIVED AT GOES INTO THE LINE, and so does WHY IT TOOK THE
                // EDGE, and this is the harness whose reader is a PERSON watching a live agent.
                // Register items 240 and 261 are about the plugin's journal; this composes its own
                // sentence one layer down, so the same edges would have read
                // `Working --TurnBlocked--> Screening` and `Judging --Judge--> Reflecting` and
                // nothing more the moment a real dialog or a real reflection came up.
                //
                // ⚠⚠⚠ THE WILDCARD IS GONE, which is half of register item 263: this pattern used
                // to end in `..`, so 240's new field reached it in SILENCE while the compiler
                // stopped every exhaustive site in the workspace. A wildcard pattern is a list
                // with no glob one layer down (R376/R381's rule, in a `match` rather than in a
                // `const`), and `spent: _` is the same fact declared where a reader meets it. ⚠ The
                // other half — that this is a SECOND composer of `AiLoop::walked`'s sentence, kept
                // in step by nobody — is still owed.
                let arrived = found
                    .map(|unanswered| format!(" — {}", unanswered.noted()))
                    .unwrap_or_default();
                let cause = because
                    .map(|reason| format!(" — {}", reason.noted()))
                    .unwrap_or_default();
                // ⚠⚠⚠ AND A RECORD THIS RUN COULD NOT READ — register item 431(a), and THIS reader is
                // the one it was written for: a person watching a live agent is who would otherwise
                // read three zeros as a session that has spent nothing. It is a LIVE agent's own
                // stated path, so a break here is a real deployment fact rather than a fixture's.
                let unread = unreadable
                    .map(|record| {
                        format!(
                            " — could not read the record it states: {}",
                            record.display()
                        )
                    })
                    .unwrap_or_default();
                // ⚠⚠⚠⚠ AND WHAT CHECKED A CLAIMED MILESTONE — register item 428, and this harness is
                // where it matters most: the reader is a PERSON watching a live agent claim it has
                // finished. Without this clause, *the agent said the milestone was reached* is the
                // whole of what they are told, and the party that did the work is the party that
                // certified it.
                let verdict = checked
                    .map(|verdict| format!(" — {}", verdict.describe()))
                    .unwrap_or_default();
                // ⚠⚠⚠⚠⚠ AND WHAT PROVED THE PROMPT ARRIVED — register item 434, and **this harness
                // is the one place it can be measured against a real `claude` rather than a
                // double**. Item 421's claim is that a prompt an agent's composer FOLDED AWAY is
                // delivered on that agent's own account, and item 433 says only a live run can
                // retire it: `Witnessed::Account` on this line is what that looks like when it
                // happens, and `Painted` is what says the peer's composer did not fold this one.
                //
                // ⚠⚠ It is a CHANGE that is published (see `Told`), so a run whose prompts all take
                // one road says so once — which is what a person watching wants, and what stops
                // this line repeating down a live walk.
                let evidence = witnessed
                    .map(|proof| format!(" — {}", proof.noted()))
                    .unwrap_or_default();
                step(
                    began,
                    &format!(
                        "{from:?} --{raised:?}--> {to:?}{cause}{verdict}{evidence}{arrived}{unread}"
                    ),
                );
                walked.push((from, raised, to));

                if to == AiLoopState::Priming {
                    let composed = loops
                        .authored()
                        .expect("a primed machine answers with its strings")
                        .start;
                    assert!(
                        composed.contains(&marker),
                        "⚠ THE CONTROL, and the whole reason this gate exists: the start prompt \
                         must NAME the marker, or the screen below cannot contain the loop's own \
                         instruction and the assertion after it is about nothing. Composed: \
                         {composed:?}",
                    );
                }

                // ⚠⚠⚠ THE MOMENT THIS GATE IS FOR. The start prompt has just been delivered and
                // the agent has not answered anything: the ONLY thing on that screen carrying the
                // marker is the loop's own instruction, painted back by the agent's composer.
                if to == AiLoopState::Working && first_turn.is_none() {
                    let shows = live.screen().contains(&marker);
                    step(began, &format!("screen carries the marker: {shows}"));
                    assert!(
                        shows,
                        "⚠ THE SECOND CONTROL: the live agent must have painted the prompt back, \
                         or `said_done` has nothing to be fooled BY and the next assertion passes \
                         for the wrong reason. Screen: {}",
                        live.tail(6),
                    );
                    assert!(
                        !loops.said_done(&live.access).said(),
                        "⚠⚠⚠ the loop read ITS OWN INSTRUCTION as the agent saying it was done. \
                         The agent has answered nothing yet; a judge satisfied here converges a \
                         run in which no work happened, and it is what this driver did before the \
                         marker had to stand alone on a row of its own. Screen: {}",
                        live.tail(8),
                    );
                    turn_began = Instant::now();
                    first_turn = Some(());
                }
                if to == AiLoopState::Judging {
                    break;
                }
            }
            Pumped::Unbuilt(state) => panic!(
                "a live run reached {state:?}, which no driver serves yet. Walked: {walked:?}, \
                 screen: {}",
                live.tail(8),
            ),
            // ⚠⚠ THE ARM R379 EXISTS FOR, and it must be TRANSIENT rather than fatal: this gate
            // spawns the agent and starts pumping immediately, which is exactly the race that used
            // to put the first prompt into a booting program. Pumping again is the whole remedy.
            Pumped::NotReady(seen) => {
                step(began, &format!("not ready yet: {seen:?}"));
                continue;
            }
            Pumped::Ended(state) => panic!(
                "⚠⚠⚠ the loop ENDED at {state:?} without ever taking a turn. Walked: {walked:?}, \
                 screen: {}",
                live.tail(8),
            ),
        }
    }
    let turn_cost = turn_began.elapsed();

    assert!(
        walked.iter().any(|(_, _, to)| *to == AiLoopState::Judging),
        "the loop must reach `judging` — one real turn, ended by the contract. Walked: {walked:?}",
    );
    assert_eq!(
        loops.turns(),
        Some(1),
        "⚠⚠ the machine's own counter, after exactly one live turn. Walked: {walked:?}",
    );
    assert!(
        turn_cost > Duration::from_millis(500),
        "⚠⚠ and it was a REAL turn rather than a contract satisfied by the peer's rest from before \
         it: {turn_cost:?}",
    );

    println!(
        "\n== 64c, second half: {} ==\n  walk: {walked:?}\n  one live turn through the loop: \
         {turn_cost:?}\n",
        live.agent,
    );
}

/// ⚠⚠⚠ **A BRIEFED LOOP IS DRIVEN TO CONVERGENCE BY A LIVE AGENT** — the thing that had never
/// happened, and the gate debt A-1 was blocking.
///
/// # What could not be asked before
///
/// Two separate things had to be true for a live loop to converge, and until R379 neither was.
/// R379 made the document ASK the agent for the word the loop stops on. This round made it
/// possible to tell the agent what the run is FOR: the shipped template says `(edit me)`, the
/// prompts were composed from it at `<datamodel>` init, and nothing could reach them. So every
/// live run to date could only spend its budget and report `exhausted`.
///
/// # ⚠⚠ What this asserts that no stand-in can
///
/// * **that a real model, reading `done_instruction`, ends a reply with the marker on a row of its
///   own.** Every unit gate in the tree has a stand-in *written* to print it — R358's rule, which
///   R379 paid for — so the instruction being followable by an actual reader has never been tested
///   by anything. This is the first time the sentence is read by something that could ignore it.
/// * **that `stands_alone` recognises what that model actually paints.** The predicate requires
///   the row to END with the marker and carry no other alphanumerics; a model that writes
///   `**MILESTONE REACHED**` or `MILESTONE REACHED.` is not recognised and the loop fails SAFE —
///   one more turn. Registered debt says nobody had ever watched a real agent try. This watches,
///   and PRINTS the row either way, so the answer is on the record whichever it is.
/// * **that convergence is the machine's, not the clock's** — `Converged` is a distinct final
///   state from `Exhausted`, and the budget below is small enough that a run which merely ran out
///   would say so rather than look like success.
///
/// # ⚠ Why the milestone is arithmetic
///
/// It needs NO TOOL. A milestone that writes a file raises a permission dialog, which sends the
/// machine to `screening` — an unbuilt state — and the gate would be measuring debt 60 instead of
/// this. Arithmetic is answerable in one turn from the model alone, which keeps the claim on the
/// LOOP rather than on what an agent is allowed to do.
/// ⚠⚠⚠⚠⚠ **THE MILESTONE'S WORDING IS NOT WHY A LIVE JUDGE WENT DEAF** — register item 441's first
/// control, and it RULES A CAUSE OUT rather than reproducing one.
///
/// # ⚠⚠⚠⚠ The measurement this exists to explain
///
/// Item 433's own proof run (2026-08-18, HEAD daemon, `claude` 2.1.234) judged NINE turns and wrote
/// *"the agent had not declared"* on every one of them — `Heard::NotSaid`, so `lost` was 0 and the
/// driver believed it had read the whole turn — while the pane plainly showed `MILESTONE REACHED`
/// on a row of its own. **Both of that run's reflections came by the BUDGET road; the `milestone`
/// road was never taken.** The owner's own long-running loop reports the same thing on the same
/// day, four judgements running, so it is not one run's luck.
///
/// ⚠⚠⚠ **AND ITS NEIGHBOUR CONVERGES, which is what makes this a fixture rather than a theory.**
/// [`a_briefed_loop_converges_against_a_live_agent`] drives the same document against the same
/// program and reaches `Judging --Judge--> Reflecting — milestone: the agent said the milestone was
/// reached`. So the marker CAN be heard, the alternate screen is not the suspect, and the
/// difference is in what the agent was asked. **This gate is that neighbour with ONE thing changed
/// — the brief — so whatever it answers is about the brief and nothing else.**
///
/// # ⚠⚠⚠⚠⚠ WHAT IT ANSWERED, FIRST RUN: heard = TRUE, deaf judgements = 0
///
/// The live run's own trivial milestone (*"say the word one"*), driven here, is heard on the FIRST
/// judgement and reaches `Judging --Judge--> Reflecting — milestone`. ⇒ **The brief is exonerated,
/// and item 441's cause is somewhere in what still differs**: the live run passed no `max_turns` and
/// no `reflect_every` (so the document's own defaults stood, where this sets 3 and 3), it went
/// through the CLI's `orchestrate` rather than an in-process `Brief`, and its pane was born from
/// `split-window` on a daemon rather than from `Live::start`. **Those three are the next controls,
/// one at a time.**
///
/// ⚠⚠ **A GATE THAT RULES A CAUSE OUT IS WORTH KEEPING**, and this one is cheap to re-point: change
/// the brief back and it becomes the reproduction if the brief ever turns out to matter after all.
///
/// # ⚠⚠ It asserts the READING, not the convergence
///
/// What is owed here is *did the judge hear it*, and a run can fail to converge for reasons that
/// have nothing to do with hearing. Measured on the first run: the ladder works so well that the
/// agent climbs it — one, two, three — until the SUBSTRATE's 24-iteration guardrail stops the run
/// at `Exhausted(Iterations)`. That is the brief doing what it says, so the ending asserted here is
/// *not a failure*, never *converged*.
#[test]
#[ignore = "drives a LIVE agent CLI: needs credentials, costs real turns, takes minutes"]
fn a_live_judge_hears_the_marker_whatever_the_milestone_asked_for() {
    use sce_rust_runtime::IScriptEngine;
    use sprag_plugin::{AiLoopState, Brief};

    /// The same budget as the neighbour, and equal to `reflect_every` for its reason: with the two
    /// equal the BUDGET road cannot open, so a run that reaches `reflecting` at all reached it
    /// because the agent was HEARD.
    const LIVE_MAX_TURNS: i64 = 3;

    let live = Live::start("unheard");

    let brief = Brief {
        // ⚠ THE ONE THING CHANGED, and it is the brief item 433's proof run used. A trivial
        // milestone is not a lesser test of the loop: it is the one an agent answers in a sentence,
        // and every live run that went unheard had one.
        north_star: "count from one to four in English words, one number per milestone; say the \
                     north star is reached only after you have said the word four"
            .to_string(),
        milestone: "say the word one".to_string(),
        reference: "answer in one short line and use no tools".to_string(),
        closing_rules: None,
        max_turns: Some(sprag_plugin::Counted::Of(LIVE_MAX_TURNS)),
        reflect_every: Some(LIVE_MAX_TURNS),
        screen_rules: None,
        may_answer: None,
        await_person_ms: Some(0),
        handback_still_ms: None,
        ready_timeout_ms: None,
        turn_within_ms: Some(TURN_BOUND.as_millis() as i64),
    };
    let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
    let mut loops = sprag_plugin::AiLoop::new(
        lua,
        live.pane,
        &brief,
        &sprag_plugin::AiLoopSpec::driving(&live.agent),
    )
    .expect("a well-briefed loop over a live agent's pane starts");
    let marker = loops
        .authored()
        .expect("the document's datamodel must carry its authored strings")
        .done_marker;

    let progress = sprag_plugin::ProgressCell::default();
    let outcome = sprag_plugin::Driver::new(sprag_plugin::Guardrails {
        max_iterations: 24,
        max_cost: None,
        max_duration: Some(Duration::from_secs(300)),
    })
    .reporting_to(Arc::clone(&progress))
    .run(&mut loops, &live.access, &RunContext::uncancellable());
    let walked: Vec<String> = progress
        .lock()
        .expect("the progress cell")
        .journal
        .iter()
        .filter_map(|entry| entry.note.clone())
        .collect();

    let screen = live.screen();
    let marker_rows: Vec<&str> = screen
        .lines()
        .filter(|row| row.contains(marker.as_str()))
        .collect();
    let heard = walked
        .iter()
        .any(|note| note.contains("milestone: the agent said the milestone was reached"));
    let deaf = walked
        .iter()
        .filter(|note| note.contains("the agent had not declared"))
        .count();
    println!(
        "\n== item 441: what a live agent said and what the judge heard ==\n  agent: {}\n  \
         milestone: {:?}\n  heard the marker: {heard}\n  judgements that said NOT declared: \
         {deaf}\n  ended: {:?} after {} iterations\n  rows on the pane carrying the marker:\n{}\n  \
         walk: {walked:?}\n  the pane:\n{}\n",
        live.agent,
        brief.milestone,
        outcome.state,
        outcome.iterations,
        marker_rows
            .iter()
            .map(|row| format!("    {row:?}"))
            .collect::<Vec<_>>()
            .join("\n"),
        live.tail(14),
    );

    // ⚠⚠⚠⚠ **THE SCREEN IS NOT THE PREMISE HERE, AND THE FIRST BUILD OF THIS GATE LEARNED IT THE
    // expensive way.** A run that HEARS its marker reflects, and a reflection REPLACES the session —
    // so `live.pane` is a pane that no longer exists by the time this reads it, and `live.screen()`
    // answers the empty string. The premise assertion written against it failed on a run whose
    // measurement had already succeeded. `Live::drop` says the same thing about the pane it closes.
    //
    // ⚠⚠ So the walk is both the measurement AND the premise: `heard` can only be true if the agent
    // put the marker somewhere the judge read, and if it is false the rows below say whether the
    // model said it at all — diagnostically, from whichever pane is current.
    assert!(
        heard,
        "⚠⚠⚠⚠⚠ ITEM 441 REPRODUCED: the judge answered `not declared` on {deaf} judgement(s) and \
         the `milestone` road was never opened. Its neighbour \
         `a_briefed_loop_converges_against_a_live_agent` reaches that road with the SAME document \
         and the SAME program, so what differs is this brief. ⚠ {} row(s) of the CURRENT pane carry \
         the marker — {marker_rows:?} — which is diagnostic only: a run that never reflected still \
         has its original pane, so an empty list here means the model said nothing rather than that \
         the pane was replaced. Walk: {walked:?}",
        marker_rows.len(),
    );
    // ⚠⚠⚠ AND IT DID NOT BREAK, which is the only thing the ENDING is asked here. A heard marker
    // sends this run up its own ladder — one, two, three — so the substrate's iteration guardrail
    // is the ordinary ending and `Converged` is the unusual one. What may NOT happen is `Failed`:
    // that is a delivery or a datamodel breaking, and it would make every reading above suspect.
    assert_ne!(
        loops.state(),
        AiLoopState::Failed,
        "⚠⚠ a run that FAILED was not measuring what its judge can hear. Walk: {walked:?}",
    );
}

/// ⚠⚠⚠⚠⚠ **ITEM 441's SECOND CONTROL: THE BUDGET EVERY DEAF RUN SHARED.**
///
/// # ⚠⚠⚠ What this is one thing away from
///
/// [`a_live_judge_hears_the_marker_whatever_the_milestone_asked_for`] is the FIRST control and it
/// answered `heard = true`, `deaf judgements = 0` — so the deaf run's own brief is exonerated and
/// the cause is in what still differs. Three things did: the budget pair, the door the run came
/// through (the CLI's `orchestrate` rather than an in-process [`Brief`](sprag_plugin::Brief)), and
/// how the pane was born (`split-window` on a daemon rather than `Live::start`). **This gate is
/// that control with the FIRST of the three changed and nothing else**, which is the only shape
/// that answers about one of them: a fixture differing in five things answers about none.
///
/// # ⚠⚠⚠⚠ The pair is MEASURED off the door the deaf run used, not copied from the register
///
/// Item 441 recorded the suspect as *"a `reflect_every` of 8 with an unbounded `max_turns`"*. Half
/// of that is this workspace's `ai_loop.scxml` default and **is not what the deaf run ran under**.
/// `sprag orchestrate --plugin ai_loop` resolves an absent budget through
/// [`LoopKind::debt`](sprag_plugin::kind::LoopKind::debt) — `debt_loop.scxml`, which authors
/// `max_turns = 'never'` and `reflect_every = 5` — and the skill that starts the owner's loop
/// passes neither key on purpose. **So the deaf pair is [`Counted::Never`] and 5**, and that is
/// what is set below. ⚠ The register is corrected rather than followed: a control that changed the
/// variable to a value nobody ran would answer about a third configuration.
///
/// # ⚠⚠ Why this pair is the suspect at all, stated so a green result means something
///
/// The two prompts that carry the marker are composed in `priming` and neither names a turn count,
/// so there is **no path from these numbers into the text the judge reads**. What they do change is
/// the run's SHAPE: `max_turns = 'never'` makes `judging`'s budget guard unreachable, so the only
/// road out of a judged turn other than the milestone is `reflect_every`, and at 5 the run reaches
/// `reflecting` on turns 5 and 10 — which is exactly the twice-by-the-budget-road the deaf run
/// reported over nine turns. The gate above sets the two EQUAL, which closes the budget road
/// entirely. ⚠ So if the marker goes unheard here, what differs from a heard run is a session that
/// gets replaced under it; if it is heard, the pair is out and the next control is the door.
///
/// # ⚠⚠ What it asserts, and what it will mean when it goes red
///
/// The same claim as the first control — *a live judge hears a live agent's marker* — because that
/// is the property item 441 says is broken and a control that asserted something weaker could not
/// reproduce it. **A red here is the reproduction**, and it localises item 441 to the budget pair;
/// a green rules the pair out and the gate stays as the record of that, exactly as its neighbour
/// stays as the record for the brief.
///
/// # ⚠⚠⚠⚠⚠ WHAT IT ANSWERED, FIRST RUN: heard = TRUE, deaf judgements = 0, budget road = 0
///
/// Run 2026-08-18 against `claude` 2.1.234, 61 seconds. The marker was heard on the FIRST
/// judgement and on every judgement after it — `Judging --Judge--> Reflecting — milestone` **three
/// times**, each followed by a real replacement (`Reflecting --ReflectApplied--> Reviewing`,
/// `Reviewing --ReviewNone--> Restarting`, `Restarting --SessionReplaced--> Resuming`,
/// `Resuming --SessionReady--> Priming`) — and `reflecting` was reached by the budget road **not
/// once**, though the road was wide open. It ended `Exhausted(Iterations)` at the substrate's
/// ceiling, which is the neighbour's ordinary ending for the same reason: a heard ladder gets
/// climbed. ⇒ **The budget pair is OUT.** Item 441's cause is in the two differences left, and they
/// are ENTANGLED: `orchestrate` only ever drives a pane a daemon owns, and an in-process
/// [`Brief`](sprag_plugin::Brief) can only reach a pane this process built — so neither can be
/// changed alone and the next control has to be both at once.
///
/// ⚠⚠⚠⚠ **AND A FOURTH DIFFERENCE THE REGISTER NEVER LISTED IS NOW THE MOST SUSPICIOUS**: what the
/// agent DOES. Both controls' agents answer in one short line and touch no tool; the deaf run's
/// agent reads files, runs builds and writes a long report before the marker — and its turn bound
/// is the document's shipped half-hour where these two are cut off at 20 seconds. **A reply's SIZE
/// and a turn's LENGTH are what neither control has varied.**
#[test]
#[ignore = "drives a LIVE agent CLI: needs credentials, costs real turns, takes minutes"]
fn a_live_judge_hears_the_marker_on_the_budget_the_deaf_runs_shared() {
    use sce_rust_runtime::IScriptEngine;
    use sprag_plugin::{AiLoopState, Brief};

    /// How often `debt_loop.scxml` says a run of this repository's kind stops to improve itself,
    /// and the number the deaf run reflected on — twice, both times by this road.
    const DEAF_REFLECT_EVERY: i64 = 5;

    let live = Live::start("unheard-budget");

    let brief = Brief {
        // ⚠ THE BRIEF IS THE FIRST CONTROL'S — the deaf run's own — and it is held FIXED here
        // because that control proved it innocent. Changing it back would put two variables in one
        // fixture again.
        north_star: "count from one to four in English words, one number per milestone; say the \
                     north star is reached only after you have said the word four"
            .to_string(),
        milestone: "say the word one".to_string(),
        reference: "answer in one short line and use no tools".to_string(),
        closing_rules: None,
        // ⚠⚠ THE ONE THING CHANGED, AND IT IS A PAIR because the two are one decision: the
        // template's default for reflection IS the budget, so a run that declines the budget must
        // name a cadence or be refused at the door. This is the pair `orchestrate` composes for
        // this repository when a caller names neither.
        max_turns: Some(sprag_plugin::Counted::Never),
        reflect_every: Some(DEAF_REFLECT_EVERY),
        screen_rules: None,
        may_answer: None,
        await_person_ms: Some(0),
        handback_still_ms: None,
        ready_timeout_ms: None,
        turn_within_ms: Some(TURN_BOUND.as_millis() as i64),
    };
    let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
    let mut loops = sprag_plugin::AiLoop::new(
        lua,
        live.pane,
        &brief,
        &sprag_plugin::AiLoopSpec::driving(&live.agent),
    )
    .expect("a loop that declines its budget and names a cadence starts");
    let marker = loops
        .authored()
        .expect("the document's datamodel must carry its authored strings")
        .done_marker;

    // ⚠⚠⚠ THE SUBSTRATE'S BOUNDS ARE THE NEIGHBOUR'S, UNCHANGED, and here they are the only thing
    // that ends an unheard run at all: with `max_turns` declined this document has no ending of its
    // own short of the marker, so a judge that cannot hear one leaves the iteration ceiling to stop
    // it. **That is the deaf run's shape, which is the point.**
    let progress = sprag_plugin::ProgressCell::default();
    let outcome = sprag_plugin::Driver::new(sprag_plugin::Guardrails {
        max_iterations: 24,
        max_cost: None,
        max_duration: Some(Duration::from_secs(300)),
    })
    .reporting_to(Arc::clone(&progress))
    .run(&mut loops, &live.access, &RunContext::uncancellable());
    let walked: Vec<String> = progress
        .lock()
        .expect("the progress cell")
        .journal
        .iter()
        .filter_map(|entry| entry.note.clone())
        .collect();

    let screen = live.screen();
    let marker_rows: Vec<&str> = screen
        .lines()
        .filter(|row| row.contains(marker.as_str()))
        .collect();
    let heard = walked
        .iter()
        .any(|note| note.contains("milestone: the agent said the milestone was reached"));
    let deaf = walked
        .iter()
        .filter(|note| note.contains("the agent had not declared"))
        .count();
    // ⚠⚠ THE OTHER ROAD IS COUNTED TOO, because this control's whole subject is that it is now
    // OPEN: the deaf run reached `reflecting` twice and both times by the budget. A run that is
    // heard should reach it by the milestone FIRST, and the two counts side by side say which road
    // this run took rather than leaving a reader to infer it from the ending.
    let by_budget = walked
        .iter()
        .filter(|note| note.contains("the reflection budget came round"))
        .count();
    println!(
        "\n== item 441, control 2: the budget pair every deaf run shared ==\n  agent: {}\n  budget: \
         {:?} every {DEAF_REFLECT_EVERY}\n  heard the marker: {heard}\n  judgements that said NOT \
         declared: {deaf}\n  reflections by the BUDGET road: {by_budget}\n  ended: {:?} after {} \
         iterations\n  rows on the pane carrying the marker:\n{}\n  walk: {walked:?}\n  the \
         pane:\n{}\n",
        live.agent,
        brief.max_turns,
        outcome.state,
        outcome.iterations,
        marker_rows
            .iter()
            .map(|row| format!("    {row:?}"))
            .collect::<Vec<_>>()
            .join("\n"),
        live.tail(14),
    );

    // ⚠⚠⚠⚠ THE WALK IS THE PREMISE AND THE MEASUREMENT BOTH, for the neighbour's reason: a run that
    // reflects REPLACES its session, so `live.pane` may not exist by the time this reads it and
    // `live.screen()` answers the empty string. The rows above are diagnostic only.
    assert!(
        heard,
        "⚠⚠⚠⚠⚠ ITEM 441 REPRODUCED, AND THE BUDGET PAIR IS THE CAUSE: the judge answered `not \
         declared` on {deaf} judgement(s) and reached `reflecting` by the budget road {by_budget} \
         time(s), which is the deaf run's own reading. This gate is \
         `a_live_judge_hears_the_marker_whatever_the_milestone_asked_for` with ONLY `max_turns` and \
         `reflect_every` changed — to `never` and {DEAF_REFLECT_EVERY}, the pair `orchestrate` \
         composes for this repository — and that control is green, so nothing else differs. ⚠ {} \
         row(s) of the CURRENT pane carry the marker — {marker_rows:?} — diagnostic only. Walk: \
         {walked:?}",
        marker_rows.len(),
    );
    // ⚠⚠⚠ AND IT DID NOT BREAK. `Failed` is a delivery or a datamodel giving out, which would make
    // every reading above suspect; with the budget declined the ordinary ending is the substrate's
    // iteration ceiling rather than `Converged`.
    assert_ne!(
        loops.state(),
        AiLoopState::Failed,
        "⚠⚠ a run that FAILED was not measuring what its judge can hear. Walk: {walked:?}",
    );
}

#[test]
#[ignore = "drives a LIVE agent CLI: needs credentials, costs real turns, takes minutes"]
fn a_briefed_loop_converges_against_a_live_agent() {
    use sce_rust_runtime::IScriptEngine;
    use sprag_plugin::{AiLoopState, Brief};

    /// Small enough that a run which merely ran out of budget is cheap, and large enough that one
    /// answered turn plus the closing report fits with room to spare.
    const LIVE_MAX_TURNS: i64 = 3;

    let live = Live::start("converge");
    let began = Instant::now();

    let brief = Brief {
        north_star: "prove an outer loop can be driven to convergence by a real agent".to_string(),
        milestone: "state the product of 17 and 23 as a single number".to_string(),
        reference: "no tools and no files are needed; answer from arithmetic alone".to_string(),
        closing_rules: None,
        max_turns: Some(sprag_plugin::Counted::Of(LIVE_MAX_TURNS)),
        // ⚠ EQUAL to the budget, which is what keeps `reflecting` — an unbuilt state — off the
        // path. `AiLoop::new` refuses anything smaller, so this is the door's own rule rather than
        // a number chosen here.
        reflect_every: Some(LIVE_MAX_TURNS),
        // ⚠ The document's own placeholder, which claims nothing: this gate is about arithmetic
        // that raises no dialog, so screening must not be armed or it would be a second variable.
        screen_rules: None,
        // ⚠ NOBODY IS WATCHING, said rather than inherited: the patience is the document's since
        // the round that moved it, and these gates were written against `Attended::NoOne` — a run
        // that ends at the first dialog it cannot answer rather than waiting out an hour.
        may_answer: None,
        await_person_ms: Some(0),
        handback_still_ms: None,
        // ⚠ THE BARRIER'S BOUND IS THE DOCUMENT'S THREE MINUTES, inherited on purpose: a live
        // `claude` cold-starting is exactly what that number was authored for, so a gate naming its
        // own here would be measuring something no real run gets.
        ready_timeout_ms: None,
        // ⚠⚠ THE TURN'S BOUND IS SAID, because the shipped one is half an hour — a person's
        // allowance for a session doing real work, and far past what a gate may sit on. It used to
        // be an `AiLoopSpec` field; register item 300 moved it here.
        turn_within_ms: Some(TURN_BOUND.as_millis() as i64),
    };
    let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
    let mut loops = sprag_plugin::AiLoop::new(
        lua,
        live.pane,
        &brief,
        &sprag_plugin::AiLoopSpec::driving(&live.agent),
    )
    .expect("a well-briefed loop over a live agent's pane starts");
    let marker = loops
        .authored()
        .expect("the document's datamodel must carry its authored strings")
        .done_marker;

    // ⚠⚠⚠ THE BOUNDS ARE THE SUBSTRATE'S, AND THAT IS WHAT THIS GATE NOW MEASURES THAT IT COULD
    // NOT BEFORE. It used to pump `OuterLoop` by hand under `walked.len() < 24` and a five-minute
    // wall clock written here — which is register item 66 exactly: *"nothing bounds the pump; the
    // CALLER loops"*. Those two numbers are the same two numbers, moved into the `Guardrails`
    // every other run on this daemon is bounded by. A live loop that stalls is now stopped by the
    // product rather than by a `panic!` in a test.
    let progress = sprag_plugin::ProgressCell::default();
    let outcome = sprag_plugin::Driver::new(sprag_plugin::Guardrails {
        max_iterations: 24,
        max_cost: None,
        max_duration: Some(Duration::from_secs(300)),
    })
    .reporting_to(Arc::clone(&progress))
    .run(&mut loops, &live.access, &RunContext::uncancellable());
    let walked: Vec<String> = progress
        .lock()
        .expect("the progress cell")
        .journal
        .iter()
        .filter_map(|entry| entry.note.clone())
        .collect();

    // ⚠ THE CONTROL, read off the datamodel the agent was actually prompted from. If the model
    // were reading the shipped template rather than this brief, everything below would still be a
    // live measurement — of the wrong document.
    let composed = loops
        .authored()
        .expect("a primed machine answers with its strings")
        .start;
    assert!(
        composed.contains(&brief.milestone) && !composed.contains("(edit me)"),
        "the live agent must be asked what this run was BRIEFED with. Composed: {composed:?}",
    );

    // ⚠⚠⚠ WHAT THE MODEL ACTUALLY PAINTED, on the record whichever way this went — debt 92 is a
    // question about a real agent's decoration and this is the only place it is ever answered.
    let screen = live.screen();
    let marker_rows: Vec<&str> = screen
        .lines()
        .filter(|row| row.contains(marker.as_str()))
        .collect();
    println!(
        "\n== a briefed live loop, under the substrate's own guardrails ==\n  agent: {}\n  \
         walk: {walked:?}\n  turns: {:?}\n  iterations: {} of 24\n  spent: {:?}\n  \
         through the loop: {:?}\n  rows carrying the marker:\n{}\n  \
         composed start prompt:\n{composed}\n",
        live.agent,
        loops.turns(),
        outcome.iterations,
        outcome.cost,
        began.elapsed(),
        marker_rows
            .iter()
            .map(|row| format!("    {row:?}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );

    assert_eq!(
        outcome.state,
        sprag_plugin::OutcomeState::Converged,
        "⚠⚠⚠ a briefed loop must CONVERGE against a live agent, and `exhausted — turns` here would \
         mean the model never put the marker on a row `stands_alone` accepts — which is debt 92, \
         not a budget. ⚠ `exhausted — iterations` or `duration` would mean the GUARDRAILS stopped \
         it, which is a different finding again. Walked: {walked:?}, marker rows: {marker_rows:?}, \
         screen: {}",
        live.tail(12),
    );
    assert_eq!(
        loops.state(),
        AiLoopState::Converged,
        "and the DOCUMENT agrees with the run's word, or the two are counting different things",
    );
    assert!(
        loops.turns().is_some_and(|turns| turns < LIVE_MAX_TURNS),
        "⚠⚠ and it must have converged INSIDE its budget rather than at it — a run that reached \
         the ceiling on the same pass cannot be told from one that ran out. Turns: {:?}",
        loops.turns(),
    );

    // ⚠⚠⚠ **AND THE RUN HANDS BACK WHAT ITS AGENT WROTE** — register item 121, asserted here
    // because this is the only gate in the tree where the report is a REAL agent's prose rather
    // than a shell script's. Every rule the capture applies was measured off a live `claude`
    // (`what_a_live_agents_report_looks_like_to_a_reader`), and a fixture cannot say whether they
    // still hold against the program they were read from.
    //
    // ⚠ The assertion is the ARITHMETIC and the SHAPE, not a wording: what the model chooses to
    // say in a summary is its own, and a gate demanding a phrase would be asserting a model's
    // style. What it may not do is come back empty, come back as the loop's own question, or come
    // back wrapped in the terminal's furniture.
    let report = sprag_plugin::Plugin::captured(&loops).unwrap_or_else(|| {
        panic!(
            "⚠⚠⚠ A CONVERGED RUN MUST HAND BACK THE ACCOUNT ITS AGENT WROTE. `closing` asked for \
             one and the agent answered it on this pane; a caller who started the run gets the \
             word `converged` and nothing else without this. Screen: {}",
            live.tail(12),
        )
    });
    step(
        began,
        &format!("the account, {} chars", report.chars().count()),
    );
    for line in report.lines() {
        println!("    | {line}");
    }
    assert!(
        report.contains("391"),
        "⚠⚠ the account must be about the work — this run's whole milestone was one product, so a \
         summary that does not carry it is not a summary of this run: {report:?}",
    );
    assert!(
        !report.contains("Summarise what changed"),
        "⚠⚠⚠ THE CALLER'S OWN CLOSING PROMPT CAME BACK AS THE AGENT'S REPORT. A live agent paints \
         the prompt into its composer and the composer's rows reach the line store: {report:?}",
    );
    assert!(
        report.chars().next().is_some_and(char::is_alphanumeric) || report.starts_with(['●', '⏺']),
        "⚠⚠ and an account must open on something a person would read rather than on a box rule: \
         {report:?}",
    );
}

/// **A VERBATIM SLICE OF THE CLAUSE `ai_loop.scxml` COMPOSES INTO `stop_prompt`** when the
/// DOCUMENT's own turn budget ended the run.
///
/// ⚠⚠⚠ The two live endings below reach `stopping` by the two different doors, and register item 264
/// is that they used to be asked the SAME sentence — one that named the turn budget, whatever had
/// actually bound. So each gate asserts its own clause is there AND the other's is not, and these
/// are the needles.
///
/// ⚠ Claims about the document's wording, exactly as *"where you got to"* already is here, and held
/// in step by nothing but these gates going red when somebody edits one and not the other.
const STOP_SAID_TURNS: &str = "every turn its document budgeted";
/// [`STOP_SAID_TURNS`]'s counterpart for the run a WALL CLOCK stopped.
const STOP_SAID_DURATION: &str = "wall-clock time";

/// ⚠⚠⚠ **A RUN THAT RAN OUT OF TURNS SAYS WHERE IT GOT TO, IN A REAL AGENT'S WORDS** — register
/// item 201, against the program the whole feature exists for.
///
/// # ⚠⚠⚠ Why the ending that stops short is the one that needed this most
///
/// The gate above proves a CONVERGED run hands back its agent's account. That is the ending a
/// person can already read from its own word: `converged` means it got there. **`exhausted` means
/// it did not, and says nothing about how far it came** — so of the two endings, the one that was
/// explained was the one needing no explanation. A person who briefed a run and got back
/// `exhausted — turns` learned only that their budget was too small.
///
/// # What makes this a live question rather than a second fixture
///
/// The stand-in gate (`a_run_that_ran_out_of_turns_hands_back_the_account_it_was_asked_for`) proves
/// the READER: the mark, the echo discount, the address. What only a real agent can answer is
/// whether `stop_prompt` READS AS A QUESTION to the model — it tells the agent the run is over,
/// which is a thing no other prompt in this document does, and a model that took it as a statement
/// would answer nothing and the loop would report a wall clock. The wording is a product decision
/// and this is where it meets its subject.
///
/// ⚠⚠ **THE BRIEF IS UNREACHABLE ON PURPOSE AND IT IS NOT SELF-CONTRADICTORY.** The milestone asks
/// for five replies, one number each, and the budget allows two — every instruction is consistent,
/// there is simply not enough budget, which is exactly the situation being measured. A brief that
/// asked for something impossible would be measuring the model's confusion instead. ⚠ If the agent
/// says the marker anyway the run reports `converged` and this gate fails, which is the honest
/// reading: it would mean the peer claimed a milestone it had not reached.
///
/// ⚠ The assertion is the ARITHMETIC and the SHAPE, never a wording — the convergence gate's rule,
/// and for its reason.
#[test]
#[ignore = "drives a LIVE agent CLI: needs credentials, costs real turns, takes minutes"]
fn a_run_that_runs_out_of_turns_says_where_it_got_to_against_a_live_agent() {
    use sce_rust_runtime::IScriptEngine;
    use sprag_plugin::{AiLoopState, Brief};

    /// Two, against a milestone that needs five replies — so the budget is what ends this run, and
    /// the account is asked for on the way out.
    const LIVE_MAX_TURNS: i64 = 2;

    let live = Live::start("stopshort");
    let began = Instant::now();

    let brief = Brief {
        north_star: "prove a run that stops short can still say where it got to".to_string(),
        milestone: "count upward from 1, exactly ONE number per reply, until you have said 5"
            .to_string(),
        reference: "no tools and no files are needed; the numbers are the whole task".to_string(),
        closing_rules: None,
        max_turns: Some(sprag_plugin::Counted::Of(LIVE_MAX_TURNS)),
        // ⚠ EQUAL to the budget, so `judging` takes the turn-budget edge rather than the reflect
        // one — the document tests them in that order, and this gate is about the budget's ending.
        reflect_every: Some(LIVE_MAX_TURNS),
        screen_rules: None,
        // ⚠ NOBODY IS WATCHING, said rather than inherited: the patience is the document's since
        // the round that moved it, and these gates were written against `Attended::NoOne` — a run
        // that ends at the first dialog it cannot answer rather than waiting out an hour.
        may_answer: None,
        await_person_ms: Some(0),
        handback_still_ms: None,
        // ⚠ THE BARRIER'S BOUND IS THE DOCUMENT'S THREE MINUTES, inherited on purpose: a live
        // `claude` cold-starting is exactly what that number was authored for, so a gate naming its
        // own here would be measuring something no real run gets.
        ready_timeout_ms: None,
        // ⚠⚠ THE TURN'S BOUND IS SAID, because the shipped one is half an hour — a person's
        // allowance for a session doing real work, and far past what a gate may sit on. It used to
        // be an `AiLoopSpec` field; register item 300 moved it here.
        turn_within_ms: Some(TURN_BOUND.as_millis() as i64),
    };
    let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
    let mut loops = sprag_plugin::AiLoop::new(
        lua,
        live.pane,
        &brief,
        &sprag_plugin::AiLoopSpec::driving(&live.agent),
    )
    .expect("a well-briefed loop over a live agent's pane starts");

    // ⚠ THE CONTROL FOR THE WHOLE GATE, read off the datamodel the agent will be prompted from: the
    // question this run's ending asks has to be the document's, not a string retyped here.
    let asked = loops
        .authored()
        .expect("the document's datamodel must carry its authored strings")
        .stop;
    assert!(
        asked.contains("where you got to"),
        "⚠ the stopping question must be the DOCUMENT's, or what follows measures nothing: \
         {asked:?}",
    );
    // ⚠⚠ AND BEFORE THE RUN IT NAMES NO CEILING, which is the premise of the assertion after it:
    // `stopping` composes that clause in, so reading it here would be reading the preview.
    assert!(
        !asked.contains(STOP_SAID_TURNS) && !asked.contains(STOP_SAID_DURATION),
        "⚠⚠ the shipped question must carry no ceiling clause — which ceiling ends a run is not \
         knowable before one does, and a preview that named one would be register item 264 a layer \
         out: {asked:?}",
    );

    let progress = sprag_plugin::ProgressCell::default();
    let outcome = sprag_plugin::Driver::new(sprag_plugin::Guardrails {
        max_iterations: 24,
        max_cost: None,
        max_duration: Some(Duration::from_secs(300)),
    })
    .reporting_to(Arc::clone(&progress))
    .run(&mut loops, &live.access, &RunContext::uncancellable());
    let walked: Vec<String> = progress
        .lock()
        .expect("the progress cell")
        .journal
        .iter()
        .filter_map(|entry| entry.note.clone())
        .collect();

    println!(
        "\n== a live loop that ran out of turns ==\n  agent: {}\n  walk: {walked:?}\n  \
         turns: {:?}\n  iterations: {} of 24\n  spent: {:?}\n  through the loop: {:?}\n",
        live.agent,
        loops.turns(),
        outcome.iterations,
        outcome.cost,
        began.elapsed(),
    );

    assert_eq!(
        outcome.state,
        sprag_plugin::OutcomeState::Exhausted(sprag_plugin::Ceiling::Turns),
        "⚠⚠⚠ THE CONTROL, AND IT IS THREE CLAIMS AT ONCE. `converged` would mean the agent said the \
         marker without having counted to five; `exhausted — duration` or `— iterations` would mean \
         a GUARDRAIL stopped this run, and in particular that the loop sat in `stopping` waiting \
         for an answer the model never gave — which is the finding this gate exists to catch. \
         Walked: {walked:?}, screen: {}",
        live.tail(12),
    );
    assert_eq!(
        loops.state(),
        AiLoopState::Exhausted,
        "and the DOCUMENT agrees with the run's word, or the two are counting different things",
    );

    // ⚠⚠⚠ WHAT THE MODEL WAS ACTUALLY TOLD — register item 264, on the ONE door a live agent can
    // reach through this document's own arithmetic. The sentence typed into a real pane has to name
    // the ceiling that ended the run, and this is where that is a claim about a MODEL's input rather
    // than about a fixture's.
    let typed = loops
        .authored()
        .expect("the document's datamodel must carry its authored strings")
        .stop;
    assert!(
        typed.contains(STOP_SAID_TURNS) && !typed.contains(STOP_SAID_DURATION),
        "⚠⚠⚠ REGISTER ITEM 264: this run spent the DOCUMENT's own turn budget and the question put \
         to the live agent was {typed:?}. It must name that ceiling and no other — the agent cannot \
         check, and whatever it says about what a run picking this up should do first is reasoned \
         from this sentence",
    );

    let report = sprag_plugin::Plugin::captured(&loops).unwrap_or_else(|| {
        panic!(
            "⚠⚠⚠ A RUN THAT RAN OUT OF TURNS MUST HAND BACK THE ACCOUNT IT WAS ASKED FOR. This is \
             register item 201: `stopping` asked a real agent where it got to, and a caller who \
             briefed the run gets the word `exhausted` and nothing else without this. ⚠ It is also \
             what says the RECOMPOSED question still reads as a question to a model — the wording \
             changed for item 264, and a model that took it as a statement would answer nothing. \
             Screen: {}",
            live.tail(12),
        )
    });
    step(
        began,
        &format!("the account, {} chars", report.chars().count()),
    );
    for line in report.lines() {
        println!("    | {line}");
    }
    assert!(
        report.contains('1'),
        "⚠⚠ the account must be about the work — this run's whole milestone was counting, so an \
         account carrying none of what it counted is not an account of this run: {report:?}",
    );
    assert!(
        !report.contains(asked.as_str()) && !report.contains("what you left half-done"),
        "⚠⚠⚠ THE CALLER'S OWN STOPPING QUESTION CAME BACK AS THE AGENT'S ACCOUNT — whole, or as the \
         wrapped fragment a live composer paints. The echo discounted has to be the question THIS \
         state asked: {report:?}",
    );
    assert!(
        report.chars().next().is_some_and(char::is_alphanumeric) || report.starts_with(['●', '⏺']),
        "⚠⚠ and an account must open on something a person would read rather than on a box rule: \
         {report:?}",
    );
}

/// ⚠⚠⚠ **A RUN THE WALL CLOCK STOPPED SAYS WHERE IT GOT TO, TOO** — register item 208, against
/// the program the feature exists for and on the path only a real agent takes.
///
/// # ⚠⚠⚠ Why the stand-in cannot answer this and a live agent can
///
/// The gate above proves the account for the DOCUMENT's own budget. `max_turns` is arithmetic the
/// loop can see coming, so it routes itself into `stopping` between two turns. A
/// [`Guardrails`](sprag_plugin::Guardrails) ceiling is counted OUTSIDE the plugin and can fall due
/// at any instant — **including inside the wait for a turn**, which is where a real agent spends
/// almost all of a run. Measured against a stand-in at five different wall clocks, the deadline
/// landed BETWEEN two steps every single time, because that peer answers in microseconds. The path
/// this gate takes is the one a real caller's run takes on every timeout, and no fixture in the
/// tree reaches it.
///
/// # ⚠⚠ What the numbers are chosen to make certain
///
/// * `max_turns` is far out of reach, so the ending cannot be the document's own budget — the
///   ceiling asserted below is the one this gate is about;
/// * the milestone needs more replies than the clock allows, so the run genuinely stops short;
/// * and `max_duration` is a few of this agent's turns, so the clock expires while the model is
///   mid-reply rather than while the loop is between them.
///
/// ⚠ A run that came back `converged` would mean the agent claimed a milestone it had not reached;
/// one that came back `exhausted — turns` would mean the budget bit first and this gate measured
/// the one next door. Both are asserted against.
///
/// ⚠ The assertion is the ARITHMETIC and the SHAPE, never a wording — the convergence gate's rule.
#[test]
#[ignore = "drives a LIVE agent CLI: needs credentials, costs real turns, takes minutes"]
fn a_run_that_runs_out_of_time_says_where_it_got_to_against_a_live_agent() {
    use sce_rust_runtime::IScriptEngine;
    use sprag_plugin::{AiLoopState, Brief};

    /// Out of reach on purpose: this run must end on the CLOCK, and a budget the agent could spend
    /// would make the ending ambiguous between two ceilings.
    const UNSPENDABLE_TURNS: i64 = 1_000;
    /// A few of this agent's turns — long enough that at least one lands, short enough that the
    /// clock runs out while the model is mid-reply.
    const CLOCK: Duration = Duration::from_secs(45);
    /// ⚠⚠ LONGER THAN [`TURN_BOUND`], and this gate is the one place that difference is a claim
    /// rather than a cost. That constant is short because *a contract that cannot be satisfied pays
    /// it in full every time*; here the same number ALSO SIZES THE ACCOUNT'S WINDOW — the plugin
    /// asks for TWO of it, one for the turn in flight to end and one for the answer — and a live
    /// account is a real reply a model is genuinely writing, which R390 measured at 19.7 s for a
    /// whole stopping run. Sized at 20 s this gate would be asserting that a model writes faster
    /// than a number this module chose for stalls.
    const ACCOUNT_BOUND: Duration = Duration::from_secs(60);

    let live = Live::start("outoftime");
    let began = Instant::now();

    let brief = Brief {
        north_star: "prove a run stopped by its own clock can still say where it got to"
            .to_string(),
        milestone: "count upward from 1, exactly ONE number per reply, until you have said 50"
            .to_string(),
        reference: "no tools and no files are needed; the numbers are the whole task".to_string(),
        closing_rules: None,
        max_turns: Some(sprag_plugin::Counted::Of(UNSPENDABLE_TURNS)),
        reflect_every: Some(UNSPENDABLE_TURNS),
        screen_rules: None,
        // ⚠ NOBODY IS WATCHING, said rather than inherited: the patience is the document's since
        // the round that moved it, and these gates were written against `Attended::NoOne` — a run
        // that ends at the first dialog it cannot answer rather than waiting out an hour.
        may_answer: None,
        await_person_ms: Some(0),
        handback_still_ms: None,
        ready_timeout_ms: None,
        // ⚠⚠ THIS IS ALSO THE ACCOUNT'S WINDOW. The plugin sizes the turn it is granted from the
        // bound declared for a turn — see `Accounting::Within` — so a gate that declared none
        // would be measuring the substrate's default instead of this contract.
        // ⚠⚠⚠ AND IT IS DECLARED HERE, ON THE BRIEF, since register item 300 made the bound the
        // document's: `Accounting::Within` reads it back out of the datamodel through
        // `OuterLoop::turn_within`, so this line is what that read has to find.
        turn_within_ms: Some(ACCOUNT_BOUND.as_millis() as i64),
    };
    let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
    let mut loops = sprag_plugin::AiLoop::new(
        lua,
        live.pane,
        &brief,
        &sprag_plugin::AiLoopSpec::driving(&live.agent),
    )
    .expect("a well-briefed loop over a live agent's pane starts");

    let asked = loops
        .authored()
        .expect("the document's datamodel must carry its authored strings")
        .stop;

    let progress = sprag_plugin::ProgressCell::default();
    let outcome = sprag_plugin::Driver::new(sprag_plugin::Guardrails {
        max_iterations: 4_000,
        max_cost: None,
        max_duration: Some(CLOCK),
    })
    .reporting_to(Arc::clone(&progress))
    .run(&mut loops, &live.access, &RunContext::uncancellable());
    let walked: Vec<String> = progress
        .lock()
        .expect("the progress cell")
        .journal
        .iter()
        .filter_map(|entry| entry.note.clone())
        .collect();

    // ⚠⚠ THE PANE IS PRINTED WHATEVER HAPPENS, and the first live run of this gate is why: it
    // failed with the account never asked, and the walk alone could not say whether the agent was
    // thinking, asking, or gone. A live measurement that has to be re-run to be diagnosed costs a
    // real agent session to learn what one `println!` would have said.
    println!(
        "\n== a live loop that ran out of TIME ==\n  agent: {}\n  walk: {walked:?}\n  \
         turns: {:?}\n  iterations: {}\n  spent: {:?}\n  through the loop: {:?} (clock {CLOCK:?})\
         \n  the pane:\n{}\n",
        live.agent,
        loops.turns(),
        outcome.iterations,
        outcome.cost,
        began.elapsed(),
        live.tail(16),
    );

    assert_eq!(
        outcome.state,
        sprag_plugin::OutcomeState::Exhausted(sprag_plugin::Ceiling::Duration),
        "⚠⚠⚠ THE CONTROL, AND IT IS THREE CLAIMS AT ONCE. `converged` would mean the agent claimed \
         a milestone it never reached; `exhausted — turns` would mean the DOCUMENT's budget bit \
         first, so this gate measured the one next door; and `cancelled` would mean the loop read \
         its own clock running out as somebody's stop — which puts the machine in a final state \
         where no account can ever be asked for. Walked: {walked:?}, screen: {}",
        live.tail(12),
    );
    assert!(
        began.elapsed() > CLOCK,
        "⚠ the clock must be what ended it: the run took {:?} against a ceiling of {CLOCK:?}",
        began.elapsed(),
    );
    assert!(
        walked
            .iter()
            .any(|note| note.contains("own duration ceiling fell due")),
        "⚠⚠ the run must have been ASKED — the walk's own line is the only place that says which \
         budget sent a loop to its account, because both budgets reach `stopping` by one edge: \
         {walked:?}",
    );

    // ⚠⚠⚠ **THE INVARIANT, AND IT IS THE WHOLE OF ITEM 208 SAID IN ONE SENTENCE: a run stopped by
    // its own ceiling either hands back its agent's account or SAYS WHY IT HAS NONE.** Before this
    // round it did neither — `exhausted` and silence, indistinguishable from a build that captures
    // nothing at all.
    //
    // ⚠⚠ It is asserted as an alternation because which branch a LIVE run takes is not this gate's
    // to fix: a live delivery is a large fraction of a short turn, so the clock lands INSIDE one
    // often, and a prompt cut short between its text and its Enter is a turn that never started —
    // measured twice here, and registered. **Both branches are correct answers; neither is
    // silence**, and a build that lost the account would have to lose the reason too to pass.
    let report = sprag_plugin::Plugin::captured(&loops);
    let excuse = walked
        .last()
        .is_some_and(|note| note.starts_with("no account:"));
    assert!(
        report.is_some() != excuse,
        "⚠⚠⚠ A RUN STOPPED BY ITS OWN WALL CLOCK MUST EITHER HAND BACK ITS AGENT'S ACCOUNT OR SAY \
         WHY IT HAS NONE — and exactly one of those, or a reader cannot tell an empty report from a \
         build that captures none. Account: {report:?}. Walk: {walked:?}",
    );

    let Some(report) = report else {
        // ⚠ The other branch is a measurement, not a failure: it is register item 222, and the
        // sentence the run gives is what a person acts on.
        println!(
            "    | no account, and the run said why: {:?}",
            walked.last(),
        );
        return;
    };
    step(
        began,
        &format!("the account, {} chars", report.chars().count()),
    );
    for line in report.lines() {
        println!("    | {line}");
    }
    assert_eq!(
        loops.state(),
        AiLoopState::Exhausted,
        "and the DOCUMENT agrees with the run's word, or the two are counting different things",
    );

    // ⚠⚠⚠ **THIS IS THE RUN REGISTER ITEM 264 WAS LYING TO, AND HERE IT IS MEASURED AGAINST THE
    // MODEL.** Until this round `stop_prompt` was one authored sentence opening *"This run has spent
    // its whole turn budget"*, and this run's `max_turns` is a thousand and untouched: a real
    // `claude`, in the one turn that asks it what a run picking this up should do first, was told it
    // had run out of turns by a run its WALL CLOCK had stopped. The stand-in gates prove the
    // composition; only this proves the sentence a model actually read.
    let typed = loops
        .authored()
        .expect("the document's datamodel must carry its authored strings")
        .stop;
    assert!(
        typed.contains(STOP_SAID_DURATION) && !typed.contains(STOP_SAID_TURNS),
        "⚠⚠⚠ REGISTER ITEM 264: a live agent whose run the WALL CLOCK stopped was asked {typed:?}. \
         It must name the clock and must not name the turn budget — this run never came near \
         `max_turns` and the agent has no way to check. Walked: {walked:?}",
    );
    // ⚠⚠ AND THE WALK SAYS IT TOO (register item 265). The Driver's own `note_to_itself` line is a
    // SEPARATE entry, so before this the only way to tell the two doors apart in a journal was by
    // whether that line preceded the arrow — reading the absence of a key as a guarantee.
    assert!(
        walked
            .iter()
            .filter(|note| note.contains("--> Stopping"))
            .all(|note| note.contains("— duration:")),
        "⚠⚠⚠ REGISTER ITEM 265: the arrow into `stopping` must name the ceiling that took it there, \
         and this run's is the clock: {walked:?}",
    );

    assert!(
        report.contains('1'),
        "⚠⚠ the account must be about the work — this run's whole milestone was counting, so an \
         account carrying none of what it counted is not an account of this run: {report:?}",
    );
    assert!(
        !report.contains(asked.as_str()) && !report.contains("what you left half-done"),
        "⚠⚠⚠ THE CALLER'S OWN STOPPING QUESTION CAME BACK AS THE AGENT'S ACCOUNT — whole, or as the \
         wrapped fragment a live composer paints: {report:?}",
    );
    // ⚠ `asked` above is the PREVIEW, taken before the run; the discount the driver applies is over
    // the COMPOSED question, so the ceiling clause has to be absent from the account too.
    assert!(
        !report.contains(STOP_SAID_DURATION),
        "⚠⚠ the ceiling clause `stopping` composed in came back inside the agent's account, so the \
         echo discount is reading a stale copy of the question: {report:?}",
    );
}

/// ⚠⚠⚠ **A LIVE LOOP THAT DOES WORK WHICH CHANGES SOMETHING** — register item 112, and the one
/// claim every other measurement of this loop has deliberately avoided making.
///
/// # ⚠⚠⚠ What every live gate before this one refused to touch, and said so
///
/// The convergence gate above picks an ARITHMETIC milestone, in its own words *"so no permission
/// dialog can fire"*: a milestone that writes a file raises one, a dialog sends the machine to
/// `screening`, and `screening` is unbuilt. So the whole live record of this loop is of an agent
/// **answering from its own head and touching nothing**. The model wrote that limitation back in
/// its own closing report, twice, unprompted: *"the next step is a milestone with real work in it
/// — something requiring tool use, multiple steps, or a failure to recover from."*
///
/// This is that milestone. It asks a real `claude` to CREATE A FILE, which means:
///
/// * it uses a tool, so the agent stops and asks for permission — the path no live run has taken;
/// * the run answers on the caller's own [`Consents`](sprag_plugin::Consents), which is the
///   argument this round put on the `ai_loop` form and which nothing but a stand-in has driven;
/// * and **the assertion is the FILE**, not a word on a screen. A run that reported `converged`
///   having written nothing would fail here, which is the difference between measuring the loop's
///   bookkeeping and measuring its effect.
///
/// # ⚠⚠ And it settles a premise this module's safety argument rests on
///
/// *"A live agent is a program that edits files. It is spawned in a scratch directory of this
/// measurement's own"* — registered as owed because **nothing asserted it**. If the file appears in
/// the scratch directory, the agent's working directory really is the sandbox; if it appears
/// nowhere, this gate says so before anybody trusts the sentence again.
///
/// ⚠ THE CONSENT QUOTES THE AGENT'S OWN WORDS and authorises exactly one option. `Yes` is matched
/// as a WHOLE LABEL, so *"Yes, and don't ask again"* — the option that turns off every future
/// question — is NOT what this run agrees to. That is [`Consent::covers`]'s exact-before-substring
/// rule doing the work, on a real dialog rather than a fixture's.
#[test]
#[ignore = "drives a LIVE agent CLI: needs credentials, costs real turns, takes minutes"]
fn a_live_loop_does_work_that_changes_something_on_the_callers_consent() {
    use sce_rust_runtime::IScriptEngine;
    use sprag_plugin::{AiLoopState, Brief, Consent, Consents};

    /// Room for the tool turn, the dialog and the closing report, and no more.
    const LIVE_MAX_TURNS: i64 = 4;
    /// A name nothing else would produce, so its presence is this run's doing.
    const MADE: &str = "SPRAG-LOOP-MADE-THIS.txt";

    let live = Live::start("does-work");
    let began = Instant::now();
    let expected = live.scratch.path().join(MADE);
    assert!(
        !expected.exists(),
        "⚠ THE CONTROL: the file must not be there before the run, or its presence afterwards \
         says nothing: {expected:?}",
    );

    let brief = Brief {
        north_star: "prove an outer loop can drive a real agent through work that changes a file"
            .to_string(),
        milestone: format!(
            "create a file named {MADE} in the current directory whose only contents are the \
             single word: ready"
        ),
        reference: "you are in an empty scratch directory of your own; use your file tools"
            .to_string(),
        closing_rules: None,
        max_turns: Some(sprag_plugin::Counted::Of(LIVE_MAX_TURNS)),
        reflect_every: Some(LIVE_MAX_TURNS),
        // ⚠ Unarmed: this gate's claim is about the CONSENT carrying the loop through the dialog,
        // so a standing rule that could also get past it would make the finding ambiguous.
        screen_rules: None,
        // ⚠⚠⚠ THE WHOLE POINT. Without this the run stops at the agent's first permission dialog
        // with nothing judged — measured against a stand-in, and the reason every live milestone
        // before this one was arithmetic.
        may_answer: Consents::of(vec![
            Consent::parse("Do you want to".to_string(), "Yes".to_string())
                .expect("both needles are non-empty"),
        ]),
        // ⚠ NOBODY IS WATCHING, said rather than inherited: the patience is the document's since
        // the round that moved it, and these gates were written against `Attended::NoOne` — a run
        // that ends at the first dialog it cannot answer rather than waiting out an hour.
        await_person_ms: Some(0),
        handback_still_ms: None,
        // ⚠ THE BARRIER'S BOUND IS THE DOCUMENT'S THREE MINUTES, inherited on purpose: a live
        // `claude` cold-starting is exactly what that number was authored for, so a gate naming its
        // own here would be measuring something no real run gets.
        ready_timeout_ms: None,
        // ⚠⚠ THE TURN'S BOUND IS SAID, because the shipped one is half an hour — a person's
        // allowance for a session doing real work, and far past what a gate may sit on. It used to
        // be an `AiLoopSpec` field; register item 300 moved it here.
        turn_within_ms: Some(TURN_BOUND.as_millis() as i64),
    };
    let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
    let mut loops = sprag_plugin::AiLoop::new(
        lua,
        live.pane,
        &brief,
        // ⚠ THE CONSENT AND BOTH DURATIONS ARE ON THE BRIEF NOW — they are the document's data,
        // not bindings. What is left here is which agent, in which pane.
        &sprag_plugin::AiLoopSpec::driving(&live.agent),
    )
    .expect("a well-briefed loop over a live agent's pane starts");

    let progress = sprag_plugin::ProgressCell::default();
    let outcome = sprag_plugin::Driver::new(sprag_plugin::Guardrails {
        // The substrate's bounds, as the convergence gate beside this one establishes. A tool turn
        // is slower than an arithmetic one, so the wall clock is the looser of the two.
        max_iterations: 40,
        max_cost: None,
        max_duration: Some(Duration::from_secs(420)),
    })
    .reporting_to(Arc::clone(&progress))
    .run(&mut loops, &live.access, &RunContext::uncancellable());
    let held = progress.lock().expect("the progress cell").clone();
    let walked: Vec<String> = held
        .journal
        .iter()
        .filter_map(|entry| entry.note.clone())
        .collect();

    let made = std::fs::read_to_string(&expected);
    println!(
        "\n== a live loop doing work that changes something ==\n  agent: {}\n  walk: {walked:?}\n  \
         turns: {:?}\n  iterations: {}\n  dialogs answered: {}\n  spent: {:?}\n  \
         through the loop: {:?}\n  {MADE}: {made:?}\n",
        live.agent,
        loops.turns(),
        outcome.iterations,
        held.answered,
        outcome.cost,
        began.elapsed(),
    );

    // ⚠⚠⚠ THE FILE IS THE ASSERTION. Everything else this run reports is bookkeeping about a
    // screen; this is the world being different because a loop ran.
    let made = made.unwrap_or_else(|_| {
        panic!(
            "⚠⚠⚠ the loop must have driven the agent to actually CREATE {expected:?}. It is not \
             there, so either the agent never got permission (walk: {walked:?}) or it wrote \
             somewhere else — which would also mean this module's sandbox claim is false. Outcome: \
             {:?}, screen: {}",
            outcome.state,
            live.tail(14),
        )
    });
    assert!(
        made.to_lowercase().contains("ready"),
        "⚠⚠ and its contents must be what the milestone asked for, or the loop drove the agent to \
         do something adjacent to the work: {made:?}",
    );
    assert_eq!(
        outcome.state,
        sprag_plugin::OutcomeState::Converged,
        "⚠⚠ and the run must AGREE that it reached the milestone. A file on disk beside an \
         `exhausted` or `blocked` outcome would mean the work happened and the loop could not tell. \
         Walked: {walked:?}, screen: {}",
        live.tail(14),
    );
    assert_eq!(
        loops.state(),
        AiLoopState::Converged,
        "and the document agrees with the run's word",
    );
    assert!(
        held.answered >= 1,
        "⚠⚠⚠ AND THE RUN MUST HAVE ANSWERED AT LEAST ONE DIALOG ON THE CALLER'S BEHALF. If it \
         answered none, this agent was configured to ask for nothing and the gate measured the \
         arithmetic case with extra steps — which is exactly the hole it was built to close. \
         Journal: {walked:?}",
    );
}

/// ⚠⚠⚠ **A LIVE LOOP IS CARRIED PAST A DIALOG BY ITS AUTHOR'S STANDING INSTRUCTION** — register
/// items 119, 5 and 142, end to end against a real agent.
///
/// # ⚠⚠⚠ Why this is the OPPOSITE gate to the one above it, and needs both to mean anything
///
/// `a_live_loop_does_work_that_changes_something_on_the_callers_consent` proves a consent carries a
/// loop THROUGH a permission dialog and the file gets written. This one arms **no consent at all**
/// and a standing rule instead, and its assertion is that **the file is never written** — the agent
/// asked, the run refused on the author's behalf, and redirected it to something else, which it
/// then did.
///
/// So the two gates use the same agent, the same tool and the same dialog, and the world ends up in
/// opposite states. That is what makes each of them a claim about the CONTRACT rather than about
/// the agent's mood.
///
/// # ⚠⚠⚠ The file's ABSENCE is the assertion, and it is the strongest one this module can make
///
/// Everything else a run reports is bookkeeping about a screen. `Escape` refusing the tool call was
/// measured by `what_a_key_does_to_a_live_agents_permission_dialog` reading the agent's own words
/// (`User rejected write to …`), and words are what an agent chose to print. **A file that does not
/// exist is not a rendering.**
///
/// # ⚠⚠ And the redirect is KOREAN, deliberately
///
/// The loop document's shipped reply is in its author's language, so *"a standing instruction
/// reaches a live agent's composer"* is a claim about non-ASCII by construction. PR-87 was a round
/// in which non-ASCII reached this datamodel by one route mangled and the other whole; this is the
/// third route — datamodel to PANE — and nothing had measured it.
#[test]
#[ignore = "drives a LIVE agent CLI: needs credentials, costs real turns, takes minutes"]
fn a_live_loop_is_carried_past_a_dialog_by_its_authors_standing_instruction() {
    use sce_rust_runtime::IScriptEngine;
    use sprag_plugin::{AiLoopState, Brief, ScreenRule, ScreenRules};

    /// Room for the tool turn, the screening, the redirected turn and the closing report.
    const LIVE_MAX_TURNS: i64 = 4;
    /// A name nothing else would produce — so if it appears, this run allowed it.
    ///
    /// ⚠⚠⚠ **AND IT MUST NOT ARGUE WITH THE AGENT**, which the session-replacement gate below measured
    /// the hard way. This constant used to read `SPRAG-LOOP-MUST-NOT-MAKE-THIS.txt`, and a live claude
    /// declined to attempt it AT ALL — not by asking permission, but by reasoning about the NAME:
    ///
    /// > *"If this arrived via a /loop or scheduled agent carrying instructions from a prior session,
    /// > that's worth checking — that's the scenario the filename appears designed to catch. No files
    /// > were created or modified."*
    ///
    /// So the run raised no dialog, nothing was screened, and the gate measured nothing — while its
    /// green twin had measured a dialog the day before. **A fixture whose NAME is an argument is a
    /// fixture that decides its own result**, and the name that reads as a tripwire is the one an agent
    /// resisting prompt injection is right to refuse. Neutral, distinctive, and it says nothing.
    const REFUSED: &str = "LOOP-OUTPUT.txt";
    /// The author's standing instruction, in the author's own language.
    ///
    /// ⚠ It names the done marker because the loop's own `done_instruction` is in the PROMPT, and a
    /// redirect is not a prompt — `screen.matched` owes none, deliberately, since the agent has
    /// just been spoken to. An agent redirected without being told how to finish would work on and
    /// spend the run's turns.
    ///
    /// ⚠⚠⚠ **AND IT SAYS IT OUTRANKS THE MILESTONE, WHICH THE FIRST RUN OF THIS GATE IS WHY.** The
    /// redirect said only *"do not make that file"* against a milestone that said *make it*, and
    /// the live agent refused to finish — correctly, and it wrote the reason out: *"the loop is
    /// re-issuing the original milestone, but your last direct instruction was to not create that
    /// file. That standing instruction wins, so I'm not making it … I will not write the completion
    /// token in this state."* **The product did everything right and the BRIEF was
    /// self-contradictory.** A standing instruction that forbids the only route to the milestone
    /// has to give another one, which is what a person writing one actually does.
    const INSTEAD: &str = "그 파일은 만들지 마라. 이 지시가 마일스톤보다 우선한다. 대신 그 파일에 \
                           들어갔을 내용을 답으로만 말하고, 답의 마지막 줄을 정확히 이렇게만 \
                           써라: MILESTONE REACHED";
    /// A word of that instruction that is unmistakably non-ASCII, for the crossing claim.
    const KOREAN: &str = "만들지";

    let live = Live::start("screens");
    let began = Instant::now();
    let forbidden = live.scratch.path().join(REFUSED);
    assert!(
        !forbidden.exists(),
        "⚠ THE CONTROL: the file must not be there before the run, or its ABSENCE afterwards says \
         nothing: {forbidden:?}",
    );

    let brief = Brief {
        north_star: "prove a loop can refuse its agent's tool call on a standing instruction"
            .to_string(),
        milestone: format!(
            "create a file named {REFUSED} in the current directory whose only contents are the \
             single word: ready"
        ),
        reference: "you are in an empty scratch directory of your own; use your file tools"
            .to_string(),
        closing_rules: None,
        max_turns: Some(sprag_plugin::Counted::Of(LIVE_MAX_TURNS)),
        reflect_every: Some(LIVE_MAX_TURNS),
        // ⚠⚠⚠ THE WHOLE POINT: the caller supplies the AUTHOR's half of the contract, quoting the
        // agent's own words exactly as a consent's `asked` does — the needle R383 measured covering
        // every dialog three tool families raise.
        screen_rules: ScreenRules::of(vec![
            ScreenRule::parse("Do you want to".to_string(), INSTEAD.to_string())
                .expect("both halves are non-empty"),
        ]),
        // ⚠ Nobody is watching — see the note on the gates above.
        may_answer: None,
        await_person_ms: Some(0),
        handback_still_ms: None,
        // ⚠ THE BARRIER'S BOUND IS THE DOCUMENT'S THREE MINUTES, inherited on purpose: a live
        // `claude` cold-starting is exactly what that number was authored for, so a gate naming its
        // own here would be measuring something no real run gets.
        ready_timeout_ms: None,
        // ⚠⚠ THE TURN'S BOUND IS SAID, because the shipped one is half an hour — a person's
        // allowance for a session doing real work, and far past what a gate may sit on. It used to
        // be an `AiLoopSpec` field; register item 300 moved it here.
        turn_within_ms: Some(TURN_BOUND.as_millis() as i64),
    };
    let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
    let mut loops = sprag_plugin::AiLoop::new(
        lua,
        live.pane,
        &brief,
        // ⚠⚠⚠ NO CONSENT, and that is the control for the whole gate — said on the BRIEF, which is
        // where the clauses live now. If one were armed it could take the dialog's own `Yes`, the
        // file would be written, and nothing below would be about `screening`.
        &sprag_plugin::AiLoopSpec::driving(&live.agent),
    )
    .expect("a well-briefed loop over a live agent's pane starts");

    let progress = sprag_plugin::ProgressCell::default();
    let outcome = sprag_plugin::Driver::new(sprag_plugin::Guardrails {
        max_iterations: 40,
        max_cost: None,
        max_duration: Some(Duration::from_secs(420)),
    })
    .reporting_to(Arc::clone(&progress))
    .run(&mut loops, &live.access, &RunContext::uncancellable());
    let held = progress.lock().expect("the progress cell").clone();
    let walked: Vec<String> = held
        .journal
        .iter()
        .filter_map(|entry| entry.note.clone())
        .collect();
    let typed = live
        .access
        .input_echo()
        .and_then(|echo| echo.pane_recent_input(live.pane))
        .unwrap_or_default();

    // ⚠⚠ THE SCREEN IS PRINTED UNCONDITIONALLY, and the first run of this gate is why: it stopped
    // on `calls refused: 0` and the assertion that fired carried the WALK but not the PANE — so
    // *why the agent never asked* was the one thing the measurement could not say. R378's lesson
    // applies to the failure message AND to the print.
    println!(
        "\n== a live loop refusing its agent's call on a standing instruction ==\n  agent: {}\n  \
         walk: {walked:?}\n  turns: {:?}\n  iterations: {}\n  calls refused: {}\n  dialogs \
         answered: {}\n  spent: {:?}\n  through the loop: {:?}\n  {REFUSED} exists: {}\n  \
         detector: {}\n  screen:\n{}\n",
        live.agent,
        loops.turns(),
        outcome.iterations,
        held.screened,
        held.answered,
        outcome.cost,
        began.elapsed(),
        forbidden.exists(),
        live.seen(),
        live.tail(20),
    );

    // ⚠⚠⚠ THE ABSENCE IS THE ASSERTION.
    assert!(
        !forbidden.exists(),
        "⚠⚠⚠ THE AGENT'S TOOL CALL MUST NOT HAVE HAPPENED. The file is there, so the run either \
         approved a dialog nobody consented to or typed into one that was still up — which is the \
         exact hazard `Tab` demonstrated. Walked: {walked:?}, screen: {}",
        live.tail(14),
    );
    assert!(
        held.screened >= 1,
        "⚠⚠⚠ AND THE RUN MUST HAVE REFUSED AT LEAST ONE CALL ON THE AUTHOR'S BEHALF. Zero means \
         this agent asked for nothing — so the file's absence is about a milestone it never \
         attempted, and this gate measured nothing. Walked: {walked:?}",
    );
    assert_eq!(
        held.answered, 0,
        "⚠⚠ and it must have APPROVED nothing. A run whose two tallies were one number could not \
         tell a person what their agent was allowed to do: {walked:?}",
    );
    assert!(
        typed.contains(KOREAN),
        "⚠⚠⚠ AND THE AUTHOR'S OWN LANGUAGE MUST HAVE REACHED THE AGENT'S COMPOSER. This is the \
         third route non-ASCII takes into this loop — the datamodel to the PANE — and the two \
         before it were the round PR-87 came from. Typed: {typed:?}",
    );
    assert_eq!(
        outcome.state,
        sprag_plugin::OutcomeState::Converged,
        "⚠⚠ and the redirected agent must have reached the end the instruction gave it. Anything \
         else means the refusal landed and the agent was left with nothing to do — the failure \
         `Malformed::SaysNothing` refuses at construction, met at the other end. Walked: \
         {walked:?}, screen: {}",
        live.tail(14),
    );
    assert_eq!(
        loops.state(),
        AiLoopState::Converged,
        "and the document agrees with the run's word",
    );
    assert!(
        walked
            .iter()
            .any(|note| note.contains("refused the peer's call")),
        "⚠⚠ and the run's JOURNAL must carry the refusal in its own words, or a person auditing \
         this run finds a converged loop and no record of what it turned down: {walked:?}",
    );
}

/// ⚠⚠⚠ **A LIVE LOOP REPLACES ITS AGENT'S SESSION AND TELLS THE REPLACEMENT WHAT IT LEARNED** —
/// register items 6 and 148, against the real thing.
///
/// # ⚠⚠⚠ What no gate in this workspace could say before this one
///
/// `reflecting` and `restarting` were the last two states of `ai_loop.scxml` that no driver served.
/// What they are for is the only thing that lets ONE RUN outlive ONE AGENT'S CONTEXT: the parts a loop
/// would want to improve about its inner session — the agent's base context, which MCP servers load,
/// `CLAUDE.md`, a memory index — are all read when a session STARTS, so a live session cannot be asked
/// to re-read them. The loop closes it and opens a fresh one.
///
/// Offline gates prove the walk, the pane, the argv, the directory and the prompts. What they cannot
/// prove is the part that matters most: **that a real agent CLI comes back up in the replacement and
/// carries on toward the same milestone.** A `/bin/sh` stand-in restarts in milliseconds and has no
/// context to lose.
///
/// # The shape, and why the trigger is a standing instruction rather than the budget
///
/// A reflection is triggered by `turns_since_reflect >= reflect_every` OR by a standing instruction
/// having fired since the last one. The budget trigger would reflect and find nothing to change, which
/// costs no restart — correctly. So this gate uses the other one, which is also register item 148's
/// own case:
///
/// 1. the milestone asks for a FILE, so the agent reaches for a tool and asks permission;
/// 2. no consent is armed, so the author's `screen_rules` claim the dialog: the call is REFUSED and
///    the agent is told, in the author's own language, to do something else instead;
/// 3. that instruction is remembered, the next judgement reflects on it, and the session is REPLACED;
/// 4. ⚠⚠⚠ **the fresh agent — which never saw the refusal — is greeted with a `start_prompt` carrying
///    it**, and finishes the work the instruction described.
///
/// Step 4 is the whole claim. Before this round the redirect reached the pane once and `turn_prompt`
/// asked for the original milestone on every turn after it; the live agent of R384 reported the
/// deadlock in words — *"루프가 매 턴 같은 요청을 반복하고 저는 매 턴 같은 이유로 거절 … 진전이
/// 없습니다"*.
///
/// # ⚠⚠⚠ WHY THIS RUN ENDS `exhausted` AND NOT `converged`, which the FIRST run of this gate is why
///
/// The first attempt reused R384's standing instruction, which tells the agent to say the file's
/// contents in its reply **and to write the completion token**. Measured: the live agent did both IN
/// THE SAME TURN, so `judging`'s first guard — *goal met* — took the run to `closing` and there was
/// nothing left to reflect on. The whole walk was eight steps with no reflection in it.
///
/// That is `judging` working exactly as authored, and it is a fact worth writing down: **a standing
/// instruction that COMPLETES the milestone leaves nothing to carry across a session.** A reflection
/// is only ever needed by a run that has further to go.
///
/// So this gate's instruction deliberately leaves the milestone unmet — it forbids the file and says
/// not to write the token, which is the conclusion R384's own live agent reached unprompted about a
/// brief of this shape (*"이 상태에서 완료 토큰은 쓰지 않습니다"*). The run therefore spends its turn
/// budget, and what is asserted is the REPLACEMENT rather than the ending. ⚠ Being explicit about it
/// is the difference between an instrument and a self-contradictory brief nobody noticed.
#[test]
#[ignore = "drives a LIVE agent CLI: needs credentials, costs real turns, takes minutes"]
fn a_live_loop_replaces_its_session_and_tells_the_replacement_what_it_learned() {
    use sce_rust_runtime::IScriptEngine;
    use sprag_plugin::{AiLoopState, Brief, ScreenRule, ScreenRules};

    /// The tool turn, the redirected turn on the fresh session, and room for two more. ⚠ A reflection
    /// turn is not among them: this build's reflection speaks to nobody.
    const LIVE_MAX_TURNS: i64 = 4;
    /// A name nothing else would produce — so if it appears, this run allowed it. ⚠⚠⚠ NEUTRAL, and the
    /// first two runs of this gate are why: see the same constant in the screening gate above, where a
    /// live agent read the old name as a tripwire and declined the whole milestone on its own
    /// judgement, so nothing was ever screened and nothing was measured.
    const REFUSED: &str = "LOOP-OUTPUT.txt";
    /// The author's standing instruction, in the author's own language.
    ///
    /// ⚠⚠⚠ **IT LEAVES THE MILESTONE UNMET ON PURPOSE** — see the doc above. R384's version told the
    /// agent how to FINISH, and the live agent finished in the same turn, so `judging` went straight
    /// to `closing` and no reflection ever happened. An instruction that completes the milestone is
    /// one a run has nothing to carry across a session.
    const INSTEAD: &str = "그 파일은 만들지 마라. 이 지시가 마일스톤보다 우선한다. 파일을 만드는 \
                           대신, 그 파일에 들어갔을 내용을 답으로만 말해라. 마일스톤이 파일을 \
                           요구하므로 완료 토큰은 쓰지 마라.";
    /// A word of that instruction that is unmistakably non-ASCII, for the crossing claim.
    const KOREAN: &str = "만들지";

    let live = Live::start("replaces");
    let began = Instant::now();
    let forbidden = live.scratch.path().join(REFUSED);
    let started_on = live.pane;
    // ⚠ Read BEFORE the run: these are what the replacement has to match, off the pane that is about
    // to be closed.
    let (argv, cwd) = {
        let guard = live.workspace.lock().expect("the workspace mutex");
        let pane = guard.pane(started_on).expect("the pane the loop is given");
        (pane.argv().to_vec(), pane.pty().cwd())
    };

    let brief = Brief {
        north_star:
            "prove a loop can replace its agent's session and carry its instructions across"
                .to_string(),
        milestone: format!(
            "create a file named {REFUSED} in the current directory whose only contents are the \
             single word: ready"
        ),
        reference: "you are in an empty scratch directory of your own; use your file tools"
            .to_string(),
        closing_rules: None,
        max_turns: Some(sprag_plugin::Counted::Of(LIVE_MAX_TURNS)),
        // ⚠⚠⚠ THE BUDGET TRIGGER IS OFF, so the reflection below is caused by the STANDING
        // INSTRUCTION and by nothing else. Equal is what makes `turns_since_reflect >= reflect_every`
        // unreachable — `judging` tests the turn budget first.
        reflect_every: Some(LIVE_MAX_TURNS),
        screen_rules: ScreenRules::of(vec![
            ScreenRule::parse("Do you want to".to_string(), INSTEAD.to_string())
                .expect("both halves are non-empty"),
        ]),
        // ⚠ Nobody is watching — see the note on the gates above.
        may_answer: None,
        await_person_ms: Some(0),
        handback_still_ms: None,
        // ⚠ THE BARRIER'S BOUND IS THE DOCUMENT'S THREE MINUTES, inherited on purpose: a live
        // `claude` cold-starting is exactly what that number was authored for, so a gate naming its
        // own here would be measuring something no real run gets.
        ready_timeout_ms: None,
        // ⚠⚠ THE TURN'S BOUND IS SAID, because the shipped one is half an hour — a person's
        // allowance for a session doing real work, and far past what a gate may sit on. It used to
        // be an `AiLoopSpec` field; register item 300 moved it here.
        turn_within_ms: Some(TURN_BOUND.as_millis() as i64),
    };
    let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
    let mut loops = sprag_plugin::AiLoop::new(
        lua,
        live.pane,
        &brief,
        // ⚠ NO CONSENT, on the brief: a clause could take the dialog's own `Yes`, the file would be
        // written, and nothing below would be about screening — or about the reflection screening
        // triggers.
        &sprag_plugin::AiLoopSpec::driving(&live.agent),
    )
    .expect("a well-briefed loop over a live agent's pane starts");

    let progress = sprag_plugin::ProgressCell::default();
    let outcome = sprag_plugin::Driver::new(sprag_plugin::Guardrails {
        max_iterations: 60,
        max_cost: None,
        // ⚠ Longer than the other loop gates by a whole agent STARTUP: this run pays for a second
        // session coming up, which R379 measured at tens of seconds on a cold start. ⚠⚠ And not
        // longer than that: a peer that goes quiet costs this whole number and says only `Null`
        // (item 175), so every second here is a second the WORST case takes. Measured: the walk this
        // gate wants is done inside ninety.
        max_duration: Some(Duration::from_secs(300)),
    })
    .reporting_to(Arc::clone(&progress))
    .run(&mut loops, &live.access, &RunContext::uncancellable());
    let held = progress.lock().expect("the progress cell").clone();
    let walked: Vec<String> = held
        .journal
        .iter()
        .filter_map(|entry| entry.note.clone())
        .collect();

    // ⚠⚠⚠ THE PANE THE RUN ENDED ON, found by asking the WORKSPACE rather than the loop: `driving()`
    // answers `None` once a run has converged, which is correct (its peer is at rest) and useless
    // here. Exactly one pane should be left — the replacement.
    let live_panes = live.access.pane_ids();
    let ended_on = live_panes.first().copied().unwrap_or(started_on);
    let typed_fresh = live
        .access
        .input_echo()
        .and_then(|echo| echo.pane_recent_input(ended_on))
        .unwrap_or_default();
    let (fresh_argv, fresh_cwd) = {
        let guard = live.workspace.lock().expect("the workspace mutex");
        guard.pane(ended_on).map_or((Vec::new(), None), |pane| {
            (pane.argv().to_vec(), pane.pty().cwd())
        })
    };

    // ⚠⚠ PRINTED UNCONDITIONALLY, R378's lesson: what this gate is for is the walk, and a failure
    // whose message carries the ending but not the screen cannot say why.
    println!(
        "\n== a live loop replacing its own inner session ==\n  agent: {}\n  walk: {walked:?}\n  \
         turns: {:?}\n  iterations: {}\n  calls refused: {}\n  dialogs answered: {}\n  spent: {:?}\n  \
         through the loop: {:?}\n  started on pane {} and ended on pane {} ({} live)\n  \
         {REFUSED} exists: {}\n  detector: {}\n  screen of the pane it ended on:\n{}\n",
        live.agent,
        loops.turns(),
        outcome.iterations,
        held.screened,
        held.answered,
        outcome.cost,
        began.elapsed(),
        started_on.0,
        ended_on.0,
        live_panes.len(),
        forbidden.exists(),
        verdict_of(&live.access, ended_on),
        tail_of(&live.access, ended_on, 24),
    );

    // ── ⚠⚠⚠ 1. THE SESSION WAS REALLY REPLACED ──
    //
    // ⚠⚠⚠⚠⚠ **FOUR ACTS, AND FOR A ROUND THIS LIST ASKED FOR A FIFTH THAT NOTHING CAN PRODUCE** —
    // register item 439's second defect, and the one its own diagnosis missed. The list opened with
    // `Reflecting --ReflectApplied--> Restarting`, and the shipped document has no such edge:
    // `reflect.applied` targets `reviewing` and `restarting` is reached only from THERE, by
    // `review.done` or `review.none`. So this gate could not go green on any run whatever, and the
    // register read its red as *the standing instruction never fired* — one cause found, one cause
    // hidden underneath it.
    //
    // ⚠⚠⚠⚠ **MEASURED RATHER THAN READ OFF THE DOCUMENT**: item 441's second control
    // (`a_live_judge_hears_the_marker_on_the_budget_the_deaf_runs_shared`) drove a live `claude`
    // through THREE replacements in 61 seconds, and every one of them walked exactly these four
    // lines and never the old first one. **A live walk is what says what the acts are.**
    //
    // ⚠⚠⚠ AND THE SECOND ACT IS NAMED BY ITS STATE AND NOT BY ITS WORD, deliberately. `reviewing`
    // reaches `restarting` by `review.done` AND by `review.none` — *there were records to read* and
    // *there were none* — and which one a run takes is a fact about what its closed sessions left
    // behind, not about whether the replacement happened. The control above answered `ReviewNone`
    // three times because its agent wrote nothing to review; the agent HERE does real work and may
    // well answer the other. Pinning the word would make this gate red for a run that did
    // everything it claims. ⚠ Only those two edges leave that state for `restarting` (the third is
    // `cancel`), so the prefix is not loose about where the run went.
    for edge in [
        "Reflecting --ReflectApplied--> Reviewing",
        "Reviewing --Review",
        "Restarting --SessionReplaced--> Resuming",
        "Resuming --SessionReady--> Priming",
    ] {
        // ⚠⚠ THE ARROW IS THE CLAIM, AND THE CLAUSES AFTER IT ARE NOT. `==` was right while a walk
        // line was only ever its arrow; the last of these edges DELIVERS a prompt, so register item
        // 434's evidence lands on it whenever the road it took differs from the last one a reader
        // was told about — and this assertion is about *the replacement happened as four acts*,
        // which no sentence after the arrow can make more or less true. A gate that failed on it
        // would fail for something it does not claim, in the one live gate that proves item 433.
        assert!(
            walked.iter().any(|note| note.starts_with(edge)),
            "⚠⚠⚠ the run must have gone through the replacement as four separate acts — `{edge}` is \
             missing. Walked {walked:?}",
        );
    }
    assert_ne!(
        ended_on, started_on,
        "⚠⚠⚠ and it must have ended on a DIFFERENT pane. The same one back means nothing was replaced",
    );
    assert_eq!(
        live_panes.len(),
        1,
        "⚠⚠⚠ and exactly ONE pane may be left. Two means a live agent CLI is still running in the \
         session this run was supposed to have closed — two models on one milestone, both spending \
         somebody's tokens. Live: {live_panes:?}",
    );
    assert_eq!(
        (fresh_argv, fresh_cwd),
        (argv, cwd),
        "⚠⚠⚠ and the replacement must be the SAME agent in the SAME directory. `respawn` reads both \
         off the pane it replaces, which is why it takes no argv — a loop that named the agent \
         instead would restart the program without the flags its launcher chose",
    );

    // ── ⚠⚠⚠ 2. THE REPLACEMENT WAS TOLD WHAT THE RUN HAD LEARNED ──
    assert!(
        typed_fresh.contains(KOREAN),
        "⚠⚠⚠ THE WHOLE POINT. The fresh session never saw the refusal — it is a new process with an \
         empty context — so the author's standing instruction must be in the FIRST prompt it is \
         greeted with. Before `reflecting` existed the redirect reached one pane once and every later \
         prompt asked for the original milestone; the live agent of R384 reported that deadlock in \
         words. Typed into the pane it ended on: {typed_fresh:?}",
    );
    assert!(
        typed_fresh.contains("North star: "),
        "⚠⚠ and it must be the START prompt that carried it, which is the half a `turn_prompt`-only \
         answer would miss: a replacement is greeted, not continued. Typed: {typed_fresh:?}",
    );

    // ── ⚠⚠⚠ 3. AND THE RUN DID WHAT IT WAS FOR ──
    assert!(
        !forbidden.exists(),
        "⚠⚠⚠ THE AGENT'S TOOL CALL MUST NOT HAVE HAPPENED, in either session. The file is there, so \
         some turn approved a dialog nobody consented to. Walked: {walked:?}",
    );
    assert_eq!(
        held.screened, 1,
        "⚠⚠⚠ **ONE REFUSAL ACROSS BOTH SESSIONS**, and this is the sharpest thing this gate measures.
         \n\
         ZERO would mean the agent asked for nothing, so there was no instruction to carry and \
         nothing here was measured. TWO would mean the REPLACEMENT reached for the forbidden tool \
         again — the instruction would have reached its composer (asserted above) and changed nothing \
         about what it did, which is register item 148 surviving the fix. One means the fresh agent, \
         told in its first prompt what its predecessor had been told, did not repeat the attempt. \
         Walked: {walked:?}",
    );
    assert_eq!(
        held.answered, 0,
        "⚠⚠ and nothing was APPROVED — the two tallies are separate words for opposite decisions",
    );
    assert!(
        loops.turns().is_some_and(|turns| turns >= 2),
        "⚠⚠⚠ and the REPLACEMENT must have taken a turn. One turn is the first session's; two says the \
         session this run opened was prompted and answered, which is the difference between replacing \
         a session and merely opening one. Turns: {:?}, walked {walked:?}",
        loops.turns(),
    );
    assert!(
        matches!(outcome.state, sprag_plugin::OutcomeState::Exhausted(_)),
        "⚠⚠⚠ and it must end EXHAUSTED, which is this instrument's shape rather than a disappointment \
         — see the gate's own doc. The standing instruction forbids the only route to the milestone and \
         says not to write the token, so the run has further to go on every turn, which is the only \
         condition under which a reflection is ever needed. `Converged` would mean the instruction \
         finished the work and the reflection above happened for some other reason.\n\
         ⚠⚠ WHICH ceiling is deliberately not asserted, and register item 175 is why: an agent told \
         repeatedly to do something it has already done stops answering, and a loop cannot say *my peer \
         went quiet* — measured here as `turns` and then twenty-nine `Working --Null--> Working` until \
         the wall clock. Pinning `Ceiling::Turns` made this gate a claim about how long a model keeps \
         replying. Walked: {walked:?}",
    );
    assert!(
        matches!(
            loops.state(),
            AiLoopState::Exhausted | AiLoopState::Cancelled
        ),
        "⚠⚠ and the DOCUMENT must have ended too — on its own turn budget, or on `cancelled`, which is \
         its word for *the run ended underneath me*. The two vocabularies differ here deliberately: a \
         run out of TIME reaches the machine as `cancel`, because telling a person's stop from a clock \
         running out is a distinction only the Driver can make and this document must not guess at. \
         Got {:?}",
        loops.state(),
    );
}

/// ⚠⚠⚠ **WHAT A REAL AGENT ASKS WHILE IT IS WORKING** — the instrument register item 137 is about,
/// and the evidence `screening`'s shape has to be decided from.
///
/// # ⚠⚠⚠ What this crate's captured dialogs already cover, and what they do not
///
/// `sprag-detect` holds SIX dialogs captured from two real agents, and the parse of every one is
/// asserted against what a person reading the fixture sees. That is real coverage and this gate
/// does not duplicate it. What it does is name the hole in it:
///
/// | captured | kind |
/// |---|---|
/// | claude trust, codex trust, claude model picker, codex model picker, codex sign-in | **before the agent works** |
/// | claude permission (one `Fetch`) | **while it works** |
///
/// **An outer loop only ever meets the second kind**, and there is one of them, from one tool, from
/// one version. Every consent needle in this tree — including the `"Do you want to"` the live loop
/// gate answers with — was read off that single screen. So the question `screening` turns on is
/// unmeasured: *can a caller quoting the agent's own words cover the dialogs a working loop meets,
/// or does something have to classify them by KIND first?*
///
/// # What it does
///
/// One FRESH agent session per probe, because a capture taken on a screen the previous probe left
/// behind is a capture of two dialogs. Each probe asks for work that needs a different TOOL, waits
/// for the shipping parser to read a menu, and records the rendered rows plus what the parser made
/// of them — in the exact `&[&str]` shape `sprag-detect`'s own fixtures use, so a capture can be
/// pasted in and replayed offline for ever after.
///
/// ⚠⚠ **NOTHING IS ANSWERED.** The probes stop at the question; the session is dropped with the
/// dialog still up. A gate that answered would be measuring the answer path, which the loop gates
/// already do — and it would let the agent do the work, which this one has no reason to.
///
/// # ⚠⚠⚠ The assertion, and why it is the point rather than the print-out
///
/// It builds the ONE clause a caller would plausibly write — *"Do you want to"* → *"Yes"* — and
/// asks [`Consents::covers`] about every dialog it captured. If one clause covers all of them, a
/// caller can arm a loop for real work without enumerating anything, and the authored `screening`'s
/// dialog-KIND matching is answering a question nobody has. If it does not, the failure names the
/// tool whose dialog broke it, which is the same finding from the other side.
#[test]
#[ignore = "drives a LIVE agent CLI: needs credentials, costs real turns, takes minutes"]
fn what_a_live_agent_asks_while_it_works() {
    use sprag_plugin::{Consent, Consents};

    /// How long to wait for the agent to reach its permission question.
    const ASKS_WITHIN: Duration = Duration::from_secs(90);
    /// The rows of a capture, matching `sprag-detect`'s tallest fixture with room over.
    const CAPTURE_ROWS: usize = 14;

    /// One probe: what to call it, the smallest work that needs its tool, and **whether a dialog
    /// is expected at all**.
    ///
    /// ⚠⚠⚠ THE `false` ROW IS A MEASUREMENT AND NOT A CONTROL FOR ITS OWN SAKE. The first run of
    /// this gate asked a live `claude` to run `date -u +%Y` and it **ran it without asking** — the
    /// agent judges some tool calls safe and gates others. That means the dialog population a loop
    /// actually meets is NARROWER than *"every tool call"*, which is a fact about how much of a
    /// run needs covering and which nothing in this tree had ever established. Asserted rather
    /// than noted, so the day an agent starts gating `date` this says the population moved.
    const PROBES: &[(&str, &str, bool)] = &[
        (
            "write",
            "Create a file called PROBE.txt whose only contents are the word ready.",
            true,
        ),
        (
            "edit",
            "In the file SEED.txt, change the word ready to steady. Do not create any new file.",
            true,
        ),
        (
            "bash-writes",
            "Run the shell command `touch MADE-BY-BASH.txt` and tell me when it is done.",
            true,
        ),
        (
            "bash-reads",
            "Run the shell command `date -u +%Y` and tell me the single line it prints.",
            false,
        ),
    ];

    let mut captured: Vec<(&str, Vec<String>, sprag_detect::Question)> = Vec::new();
    for (label, ask, must_ask) in PROBES {
        let live = Live::start(&format!("asks-{label}"));
        let began = Instant::now();
        // ⚠ The `edit` probe needs something to edit. Seeded HERE rather than by an earlier probe,
        // because probes must not depend on each other's side effects — the whole reason each gets
        // its own session.
        std::fs::write(live.scratch.path().join("SEED.txt"), "ready\n")
            .expect("the scratch directory is this measurement's own");

        let run = RunContext::uncancellable();
        // ⚠⚠⚠ THE BARRIER, AND THE FIRST RUN OF THIS GATE IS WHY IT IS HERE. It was written
        // without one, and the capture came back showing the agent's WELCOME BANNER with the
        // prompt sitting unsubmitted in its composer: the text was typed into a program that was
        // still booting, `deliver` read its own echo back and called the delivery confirmed, and
        // the Enter went nowhere. **That is R379's finding, reproduced by a gate written three
        // rounds after it was recorded** — which is the measurement this barrier exists for, met
        // from the other side.
        let reached = Readiness::new(
            Some(ReadyWhen::Settles(live.agent.clone())),
            Some(STARTUP_BOUND),
            None,
            Attended::NoOne,
        )
        .reached(&live.access, live.pane, &run)
        .expect("the pane must stay readable");
        assert_eq!(
            reached,
            Reached::Yes,
            "{label}: the agent must be up and at rest before it is spoken to: {}",
            live.tail(3),
        );

        // ⚠⚠ ARMED BEFORE A BYTE GOES IN — `Completion::begin`'s guarantee, and what lets this
        // gate tell *the agent asked* from *the agent finished* at all.
        let mut done = Completion::new(DoneWhen::Settles);
        done.begin(&live.access, live.pane);
        let delivered = deliver(
            &live.access,
            &run,
            live.pane,
            ask,
            &Delivery {
                confirm: Some(ask.chars().take(40).collect()),
                then_press: vec![KeyStroke::named("Enter")],
                ..Delivery::new()
            },
        )
        .expect("the pane must take the prompt");
        assert!(
            !matches!(delivered, Delivered::Unconfirmed { .. }),
            "{label}: a live agent PAINTS what is typed into its composer: {delivered:?}",
        );

        // ⚠⚠⚠ THE PRODUCT'S OWN TURN-ENDING VOCABULARY DOES THE MEASURING. `Over` is exactly the
        // distinction this gate is about — *it answered* versus *it stopped to ask* — and it is
        // what a loop's `watch` reads, so what is captured here is what a run would have met. A
        // hand-rolled poll for a menu would be a second reader of one screen, which this crate has
        // paid for before.
        let over = done.wait(&live.access, live.pane, ASKS_WITHIN, &run);
        // ⚠⚠ ROWS, NOT THE COLLAPSED SCREEN, and the first run of this gate is why: it captured
        // `pane_collapsed`, which joins the whole pane into ONE string, and produced a "fixture"
        // of a single two-thousand-character line. `sprag_detect` reads `row_text` per row, so a
        // capture that is not per-row is not the parser's input.
        let rows: Vec<String> = live
            .access
            .pane_rows(live.pane)
            .unwrap_or_default()
            .into_iter()
            .map(|row| row.text.trim_end().to_owned())
            .filter(|row| !row.trim().is_empty())
            .rev()
            .take(CAPTURE_ROWS)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        println!(
            "\n== {label}: {}, after {:?} — {} ==",
            if *must_ask {
                "expected to ASK"
            } else {
                "expected to just DO IT"
            },
            began.elapsed(),
            match &over {
                Over::Yes => "finished WITHOUT asking".to_owned(),
                Over::Asking(Some(_)) => "ASKED, and the parser read it".to_owned(),
                Over::Asking(None) => "BLOCKED and the parser could NOT read it".to_owned(),
                other => format!("{other:?}"),
            },
        );

        let Over::Asking(asked) = over else {
            assert!(
                !must_ask && matches!(over, Over::Yes),
                "⚠⚠⚠ {label}: this probe expects a permission dialog and the turn ended {over:?}. \
                 A `Yes` means the agent did the work WITHOUT asking — so either this session \
                 inherited an allowlist (see `Live::start`'s `--setting-sources`) or the agent \
                 stopped gating that tool. Screen: {}",
                live.tail(6),
            );
            println!(
                "  ⚠ MEASURED: this agent does not gate that call. Screen: {}",
                live.tail(3),
            );
            continue;
        };
        let question = asked.unwrap_or_else(|| {
            panic!(
                "⚠⚠⚠ {label}: the pane is BLOCKED and `sprag_detect::question` could not read what \
                 it is asking. That is a finding about the PARSER rather than about the agent, and \
                 it is the one this gate exists to catch. Rows:\n{}",
                rows.join("\n"),
            )
        });
        assert!(
            must_ask,
            "⚠⚠ {label}: this probe was declared as one the agent does NOT gate, and it asked. \
             The dialog population moved, which is the fact that row exists to notice",
        );
        println!(
            "    /// Captured from a live `{}` ({label}).\n    const {}_DIALOG: &[&str] = &[\n{}\n    ];",
            live.agent,
            label.to_uppercase().replace('-', "_"),
            rows.iter()
                .map(|row| format!("        {row:?},"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        println!(
            "  parsed: asked={:?}\n          choices={:?}\n          enter lands on {:?}",
            question.asked,
            question
                .choices
                .iter()
                .map(|c| (c.number, c.label.as_str()))
                .collect::<Vec<_>>(),
            question.selected().map(|c| c.number),
        );
        captured.push((label, rows, question));
    }

    // ⚠⚠⚠ THE MEASUREMENT. One clause, quoting the agent's own words, against every tool's dialog.
    let one_clause = Consents::of(vec![
        Consent::parse("Do you want to".to_string(), "Yes".to_string())
            .expect("both needles are non-empty"),
    ])
    .expect("a non-empty consent list");
    let verdicts: Vec<(&str, String)> = captured
        .iter()
        .map(|(label, _, question)| {
            (
                *label,
                match one_clause.covers(question) {
                    Ok(chose) => format!("Ok({}. {:?})", chose.number, chose.label),
                    Err(why) => format!("Err({:?})", why),
                },
            )
        })
        .collect();
    println!("\n== one clause, every tool ==\n  {verdicts:?}\n");

    for (label, _, question) in &captured {
        one_clause.covers(question).unwrap_or_else(|why| {
            panic!(
                "⚠⚠⚠ {label}: ONE clause quoting the agent's own words did not cover this tool's \
                 dialog ({why:?}). That is the finding: a caller cannot arm a loop for real work \
                 without enumerating dialogs, and whatever `screening` becomes has to carry the \
                 difference. asked={:?} choices={:?}",
                question.asked,
                question
                    .choices
                    .iter()
                    .map(|c| (c.number, c.label.as_str()))
                    .collect::<Vec<_>>(),
            )
        });
    }
    assert_eq!(
        captured.len(),
        PROBES.iter().filter(|(_, _, must_ask)| *must_ask).count(),
        "every probe that expects a dialog must have reached one, or the claim above is about the \
         ones that did",
    );
}

/// ⚠⚠⚠ **WHAT A KEY ACTUALLY DOES TO A LIVE AGENT'S PERMISSION DIALOG** — the measurement
/// `screening` has to be built on, taken before anything is built.
///
/// # ⚠⚠⚠ The question, and why reading the screen cannot answer it
///
/// R383 captured three real dialogs and every one of them ends with the same footer:
///
/// ```text
///  ❯ 1. Yes
///    2. Yes, allow all edits during this session (shift+tab)
///    3. No
///  Esc to cancel · Tab to amend
/// ```
///
/// From that text alone, **the one thing a screening act needs is unknown**: *cancel* does not say
/// whether Escape merely closes the dialog or REFUSES THE TOOL CALL, and those are different acts
/// with different reporting duties. If Escape only dismisses, a rule that presses it and then types
/// is *"get past the dialog and redirect"*. If Escape refuses the call, it is the same decision as
/// option `3. No` — a decision taken on somebody's behalf, which this crate's own rule says must be
/// reported in the run's vocabulary rather than logged as housekeeping.
///
/// And `Tab to amend` may already be this agent's *"tell it to do it differently"* door, in which
/// case building on Escape is building on the wrong one.
///
/// # ⚠⚠ Why each key gets its own session
///
/// [`what_a_live_agent_asks_while_it_works`]'s rule: a probe that presses a key CHANGES the session,
/// so a second probe on the same screen measures the first one's aftermath. ⚠ **And unlike that
/// gate, this one presses keys at a live agent** — which is why it does its pressing inside the
/// scratch directory whose existence is what makes any of this safe, and why the file it watches for
/// is created there or nowhere.
///
/// # ⚠⚠⚠ WHAT IT MEASURED (`claude` 2.1.232, 2026-08-14), and what each answer licenses
///
/// | key | the dialog | the tool call | typing afterwards |
/// |---|---|---|---|
/// | `Escape` | gone in **25 ms** | ⚠ **`User rejected write to PROBE.txt`** | reached the composer; the agent answered it |
/// | `3` | gone in **23 ms** | **rejected, identically** | the same |
/// | `Tab` | ⚠ **still up after 10 s** | — | ⚠⚠⚠ **`Wrote 1 line to PROBE.txt`** |
///
/// * **ESCAPE IS A DECISION, NOT A DISMISSAL.** It refuses the tool call — the same outcome as the
///   offered `3. No`, which is an option [`Consents`](sprag_plugin::Consents) can already take. So a
///   standing rule that presses it is **answering on somebody's behalf**, and this crate's rule is
///   that such an act gets a word in the run's own vocabulary rather than a note. That is why
///   `screening` reports [`Verdict::Screened`](sprag_plugin::Verdict::Screened) and not a
///   `Continue`. ⚠ What Escape buys over `3` is that it needs no matching OPTION to exist, which is
///   exactly the case a consent cannot reach.
/// * **`Tab to amend` IS NOT A TEXT BOX.** It leaves the dialog up and rewrites option 1 into
///   *"Yes, and tell Claude what to do next"* — so the amend flow is an APPROVAL that carries an
///   instruction. ⚠⚠⚠ **The probe typed into that and the write went through.** Building the
///   redirect on Tab would have been building a standing rule that grants permissions.
/// * ⚠⚠⚠ **AND `deliver`'s CONFIRMATION IS NOT PROOF OF A COMPOSER.** In the `Tab` arm the text was
///   read back off the screen — `Confirmed { attempts: 1 }` — and the Enter behind it approved a
///   file write. **The two proofs a screening act needs are ORDERED and neither is sufficient
///   alone**: first the question must be gone, and only then does a read-back mean what it looks
///   like it means.
/// * ⚠ A Tab-amended dialog also renames option 1, so a consent quoting `Yes` exactly stops
///   matching it and falls to a substring that reaches two options — [`Refusal::Ambiguous`], which
///   is the safe direction.
///
/// [`Refusal::Ambiguous`]: sprag_plugin::Refusal::Ambiguous
#[test]
#[ignore = "drives a LIVE agent CLI: needs credentials, costs real turns, takes minutes"]
fn what_a_key_does_to_a_live_agents_permission_dialog() {
    /// How long to wait for the agent to reach its permission question.
    const ASKS_WITHIN: Duration = Duration::from_secs(90);
    /// How long a key gets to take the dialog off the screen.
    ///
    /// ⚠ Generous next to a repaint, and sized for the reason the answering bound in
    /// `sprag_plugin::readiness` is: the slowest party is not the pane but the SUPERVISOR, whose
    /// verdict only settles after `sprag_detect::DEFAULT_SETTLE`.
    const DISMISSED_WITHIN: Duration = Duration::from_secs(10);
    /// The rows of a capture — [`what_a_live_agent_asks_while_it_works`]'s number.
    const CAPTURE_ROWS: usize = 14;
    /// The work that raises a dialog, and the file whose existence says whether it went through.
    const ASK: &str = "Create a file called PROBE.txt whose only contents are the word ready.";
    const MADE: &str = "PROBE.txt";
    /// What is typed once the dialog is off the screen. ⚠ It asks for NO tool, deliberately: what
    /// is being measured is *where does typing go*, and work that raises a second dialog would
    /// answer a different question on top of it.
    const REDIRECT: &str = "Say only the word REDIRECTED-OK and do nothing else.";
    const REDIRECTED: &str = "REDIRECTED-OK";

    /// The keys worth asking about, each named as the agent's own footer names it, **with what
    /// this gate measured them to do**: whether the key takes the dialog off the screen, and
    /// whether the tool call has happened by the time the probe is finished with the session.
    ///
    /// ⚠ `3` is here as the CONTROL, and it is the one this product can already press: selecting an
    /// offered option is exactly what [`Consents`](sprag_plugin::Consents) does. Measured, Escape
    /// behaves like it — which is the finding that decides how `screening` REPORTS.
    ///
    /// ⚠⚠⚠ **THE `Tab` ROW IS AN ASSERTION THAT A KEY IS DANGEROUS**, and it is deliberately the
    /// only row whose call goes through: the probe types into a dialog that is still up, and the
    /// file gets written. That is the hazard stated as a fact rather than as a warning, and if
    /// upstream ever makes Tab open a real text box this row goes red and whoever meets it should
    /// read this table before changing anything.
    const KEYS: &[(&str, bool, bool)] = &[
        ("Escape", true, false),
        ("Tab", false, true),
        ("3", true, false),
    ];

    for (key, dismisses, call_goes_through) in KEYS {
        let live = Live::start(&format!("keys-{}", key.to_lowercase()));
        let began = Instant::now();
        let made = live.scratch.path().join(MADE);
        let run = RunContext::uncancellable();

        let reached = Readiness::new(
            Some(ReadyWhen::Settles(live.agent.clone())),
            Some(STARTUP_BOUND),
            None,
            Attended::NoOne,
        )
        .reached(&live.access, live.pane, &run)
        .expect("the pane must stay readable");
        assert_eq!(
            reached,
            Reached::Yes,
            "{key}: the agent must be up and at rest before it is spoken to: {}",
            live.tail(3),
        );
        assert!(
            !made.exists(),
            "⚠ {key}: THE CONTROL — {MADE} must not exist before the agent is asked to make it, or \
             what this measures about the tool call is about somebody else's file",
        );

        let mut done = Completion::new(DoneWhen::Settles);
        done.begin(&live.access, live.pane);
        deliver(
            &live.access,
            &run,
            live.pane,
            ASK,
            &Delivery {
                confirm: Some(ASK.chars().take(40).collect()),
                then_press: vec![KeyStroke::named("Enter")],
                ..Delivery::new()
            },
        )
        .expect("the pane must take the prompt");
        let Over::Asking(Some(question)) = done.wait(&live.access, live.pane, ASKS_WITHIN, &run)
        else {
            panic!(
                "⚠⚠⚠ {key}: this measurement needs a real dialog on the screen and did not get \
                 one. Nothing below is about anything. Screen: {}",
                live.tail(8),
            );
        };
        step(
            began,
            &format!("{key}: dialog up — asked={:?}", question.asked),
        );

        // ── THE KEY ──
        let stroke = if sprag_input::is_key_name(key) {
            vec![KeyStroke::named(key)]
        } else {
            KeyStroke::text(key)
        };
        let pressed_at = Instant::now();
        let spent = live
            .access
            .inject(live.pane, &stroke)
            .expect("the pane must take the key");
        // ⚠⚠ THE QUESTION, NOT THE STATE — `Arrival::LeftTheQuestion`'s measured rule. A detector's
        // verdict settles, so a pane goes on reading `Blocked` for its hysteresis window after the
        // menu has left the screen, and a probe keyed on the STATE would time out on a key that
        // plainly worked.
        let dismissed = sprag_plugin::poll_until(&run, DISMISSED_WITHIN, || {
            live.access
                .supervision()
                .and_then(|supervisor| supervisor.pane_agent_state(live.pane))
                .is_none_or(|seen| seen.asking.is_none_or(|now| now.asked != question.asked))
        });
        let dismissed_in = pressed_at.elapsed();
        let rows: Vec<String> = live
            .access
            .pane_rows(live.pane)
            .unwrap_or_default()
            .into_iter()
            .map(|row| row.text.trim_end().to_owned())
            .filter(|row| !row.trim().is_empty())
            .rev()
            .take(CAPTURE_ROWS)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        println!(
            "\n== {key} ==\n  {} after {dismissed_in:?} ({spent:?})\n  the tool call: {MADE} {}\n  \
             detector: {}\n  screen:\n{}",
            match dismissed {
                sprag_plugin::Waited::Ready => "THE DIALOG WENT AWAY",
                sprag_plugin::Waited::TimedOut => "⚠ THE DIALOG IS STILL UP",
                sprag_plugin::Waited::Stopped => "the run ended underneath",
            },
            if made.exists() {
                "EXISTS — the call went through"
            } else {
                "is absent — the call did not happen (yet)"
            },
            live.seen(),
            rows.iter()
                .map(|row| format!("    {row}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );

        // ── AND WHERE DOES TYPING GO NOW? ──
        //
        // ⚠ Through `deliver` rather than a bare inject, because the question is exactly the one
        // `deliver` answers: is the text READ BACK OFF THE SCREEN before Enter. A composer that
        // paints it is a peer ready to be spoken to; anything else is a run about to submit into
        // something it cannot see.
        let mut after = Completion::new(DoneWhen::Settles);
        after.begin(&live.access, live.pane);
        let typed = deliver(
            &live.access,
            &run,
            live.pane,
            REDIRECT,
            &Delivery {
                confirm: Some(REDIRECT.chars().take(40).collect()),
                then_press: vec![KeyStroke::named("Enter")],
                ..Delivery::new()
            },
        );
        let landed = match &typed {
            Ok(Delivered::Unconfirmed { attempts, written }) => {
                format!("⚠ NEVER APPEARED on screen ({attempts} attempts, {written:?})")
            }
            Ok(other) => format!("was confirmed on screen ({other:?})"),
            Err(error) => format!("⚠ could not be typed at all ({error:?})"),
        };
        let over = after.wait(&live.access, live.pane, ASKS_WITHIN, &run);
        println!(
            "  the redirect {landed}\n  the turn then ended {}\n  the agent said {REDIRECTED}: \
             {}\n  {MADE} at the end: {}\n  final screen:\n{}",
            match &over {
                Over::Yes => "NORMALLY".to_owned(),
                Over::Asking(Some(q)) => format!("ASKING AGAIN: {:?}", q.asked),
                Over::Asking(None) => "BLOCKED, unreadable".to_owned(),
                other => format!("{other:?}"),
            },
            live.screen().contains(REDIRECTED),
            if made.exists() { "EXISTS" } else { "absent" },
            live.tail(8),
        );

        // ── ⚠⚠⚠ WHAT THE PRODUCT IS BUILT ON, ASSERTED ──
        assert_eq!(
            dismissed == sprag_plugin::Waited::Ready,
            *dismisses,
            "⚠⚠⚠ {key}: whether this key takes the dialog off the screen is the FIRST of the two \
             proofs `screening` needs, and it moved. `Escape` dismissing is what the act rests on; \
             `Tab` NOT dismissing is why the act refuses to type when the question is still up. \
             Waited {dismissed:?} after {dismissed_in:?}. Screen: {}",
            live.tail(8),
        );
        assert_eq!(
            made.exists(),
            *call_goes_through,
            "⚠⚠⚠ {key}: whether the TOOL CALL happened is the fact that decides how a standing \
             rule must REPORT. A key that refuses the call is a decision taken on somebody's \
             behalf; one that lets it through is a standing permission nobody wrote. Screen: {}",
            live.tail(8),
        );
        assert!(
            live.screen().contains(REDIRECTED),
            "⚠⚠ {key}: and the free text must have reached the agent AS AN INSTRUCTION — the \
             second half of the feature. Screen: {}",
            live.tail(8),
        );
        step(began, &format!("{key}: done"));
    }
}

/// What one live turn cost and how it ended.
struct Turn {
    index: usize,
    /// Whether a second reader was pulling the same tracker throughout — see the gate.
    sampled: bool,
    over: Over,
    elapsed: Duration,
    /// The detector's `seq` immediately before and after, which is what `Settles` compares.
    seq_before: Option<u64>,
    seq_after: Option<u64>,
    /// Whether the agent actually put this turn's token on the screen.
    answered: bool,
}

/// Drive one turn of `live` and report it — the unit the gate above repeats.
fn one_turn(live: &Live, run: &RunContext, index: usize, sampled: bool, began: Instant) -> Turn {
    // A token nobody could produce by accident, DIFFERENT PER TURN, so a screen check cannot be
    // satisfied by the previous turn's answer still being on the scrollback.
    let token = format!("ORTHOGONAL-{index}7");
    let ask = format!("Reply with exactly the word {token} and nothing else.");

    let seq_before = live.seq();
    let mut done = Completion::new(DoneWhen::Settles);
    // ⚠ ARMED BEFORE A BYTE GOES IN — `Completion::begin`'s whole guarantee, and the thing this
    // measurement exists to put under a peer that is genuinely at rest beforehand.
    done.begin(&live.access, live.pane);
    let delivered = deliver(
        &live.access,
        run,
        live.pane,
        &ask,
        &Delivery {
            confirm: Some(ask.chars().take(40).collect()),
            then_press: vec![KeyStroke::named("Enter")],
            ..Delivery::new()
        },
    )
    .expect("the pane must take the prompt");
    step(began, &format!("turn {index}: delivered {delivered:?}"));
    assert!(
        !matches!(delivered, Delivered::Unconfirmed { .. }),
        "⚠⚠ a live agent PAINTS what is typed into its composer, which is the premise `deliver` \
         reads the screen back on. An unconfirmed delivery is a finding about that premise, not \
         about the peer's answer. Screen: {}",
        live.tail(6),
    );

    let watch = sampled.then(|| Watch::start(live));
    let asked_at = Instant::now();
    let over = done.wait(&live.access, live.pane, TURN_BOUND, run);
    let elapsed = asked_at.elapsed();
    let walk = watch.map(Watch::walk).unwrap_or_default();
    let seq_after = live.seq();
    let answered = live.screen().contains(&token);
    step(
        began,
        &format!("turn {index}: {over:?} after {elapsed:?} (sampled={sampled})"),
    );
    if sampled {
        println!("  -- what the detector published across turn {index} --");
        for (at, reading) in &walk {
            println!("    [{:>7.2}s] {}", at.as_secs_f64(), reading.verdict);
            println!("               title: {}", reading.title);
            println!("               screen: {}", reading.tail);
        }
        if walk.len() >= Watch::KEEP {
            println!("    ⚠ TRUNCATED at {} distinct readings", Watch::KEEP);
        }
        println!("  -- {} distinct readings --", walk.len());
    }
    Turn {
        index,
        sampled,
        over,
        elapsed,
        seq_before,
        seq_after,
        answered,
    }
}

/// The identity these gates hand an agent — **the product's own minter**,
/// [`crate::hooks::mint_session_id`], and not one of this module's.
///
/// ⚠⚠⚠ A GATE WITH ITS OWN GENERATOR PROVES THAT ITS OWN IDS REACH A RECORD, which is not the claim.
/// R383's rule, met from the naming side: the fixture's reader must be the product's reader, and so
/// must its writer.
fn minted_uuid() -> String {
    crate::hooks::mint_session_id()
}

/// The record the agent wrote about the session called `session`, found **by the identity**.
///
/// # ⚠⚠⚠ Why this scans rather than deriving a directory
///
/// The agent files its record under a directory named for the cwd it was started in, and every
/// route to that name is a guess this workspace should not be making: the live cwd drifts the
/// moment the agent works in a subdirectory, the spawn cwd is stored nowhere, and picking the
/// newest file in the directory races any other session in the same repository. **All three fail
/// by silently reading somebody else's record rather than by failing.**
///
/// Minting removes the question. The file is named for the identity, so the identity finds it —
/// no directory, no recency, no slug. That this scan is possible at all is the measurement's own
/// argument for minting.
fn agent_record(session: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let projects = PathBuf::from(home).join(".claude").join("projects");
    let wanted = format!("{session}.jsonl");
    std::fs::read_dir(projects)
        .ok()?
        .flatten()
        .map(|project| project.path().join(&wanted))
        .find(|candidate| candidate.is_file())
}

/// What a record says its session was charged to read.
#[derive(Debug, Default, PartialEq, Eq)]
struct Billed {
    /// Distinct billed requests — deduplicated by `message.id`, because a streamed message appears
    /// many times and every fragment repeats the same usage.
    requests: usize,
    /// The accumulated context on the LAST request: everything the model was charged to read, cache
    /// included. This is the quantity `ai_loop` has no access to today.
    context: u64,
    /// Of that, the part served from cache.
    cached: u64,
}

fn billed(record: &Path) -> Billed {
    let text = std::fs::read_to_string(record).unwrap_or_default();
    let mut seen: Vec<String> = Vec::new();
    let mut billed = Billed::default();
    for line in text.lines() {
        let Ok(row) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if row.get("type").and_then(serde_json::Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(usage) = row.pointer("/message/usage") else {
            continue;
        };
        let Some(cached) = usage
            .get("cache_read_input_tokens")
            .and_then(serde_json::Value::as_u64)
        else {
            continue;
        };
        let id = row
            .pointer("/message/id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if !id.is_empty() && seen.contains(&id) {
            continue;
        }
        seen.push(id);
        let field = |name: &str| {
            usage
                .get(name)
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0)
        };
        billed.requests += 1;
        billed.cached = cached;
        billed.context = field("input_tokens") + cached + field("cache_creation_input_tokens");
    }
    billed
}

/// **THE PREMISE THE WHOLE COST SIGNAL RESTS ON: a run can NAME the session it starts, and the
/// agent files its own record under that name.**
///
/// # ⚠⚠⚠ What this replaces, and why the replaced thing was wrong
///
/// `ai_loop` cannot say what it spends. `Cost::Bytes` counts prompt bytes, which measurement showed
/// is the component that *falls* as a session grows, and `max_turns` counts turns, which span 861
/// to 633,749 tokens of context each. The number that matters — accumulated context — is written by
/// the agent on every request and never read.
///
/// The first attempt at reading it went looking for the file: pane cwd, to a directory name, to the
/// newest transcript in it. Three inferences, each failing by reading the WRONG session rather than
/// by failing. `claude --session-id <uuid>` removes all three by letting this side choose the name
/// first — the same rule `claudedocs/INSIGHT-LOOP-SCORING-AND-COST-SIGNALS.md` states one level up,
/// an identity must be minted rather than recovered.
///
/// ⚠⚠ Measured in `--print` mode before this gate was written, and that is exactly why the gate
/// exists: a piped shell probe could not submit a prompt to the TUI (the closed stdin read as
/// Ctrl-D), so the interactive case — **the only one `ai_loop` uses**, since sprag reads agent state
/// from a rendered screen — stayed unverified. A real pty driver is what settles it, and sprag is
/// one.
#[test]
#[ignore = "drives a LIVE agent CLI: needs credentials, costs real turns, takes minutes"]
fn a_minted_session_identity_names_the_record_a_live_agent_writes() {
    let session = minted_uuid();
    let live = Live::start_args("session", &["--session-id", session.as_str()]);
    let run = RunContext::uncancellable();
    let began = Instant::now();
    step(began, &format!("minted {session}"));
    step(began, &format!("spawned {:?}", live.agent));

    let mut barrier = Readiness::new(
        Some(ReadyWhen::Settles(live.agent.clone())),
        Some(STARTUP_BOUND),
        None,
        Attended::NoOne,
    );
    let reached = barrier
        .reached(&live.access, live.pane, &run)
        .expect("the pane must stay readable");
    step(began, &format!("barrier: {reached:?}"));
    assert_eq!(
        reached,
        Reached::Yes,
        "⚠⚠⚠ the agent did not come up with `--session-id` on its command line. That is the FIRST \
         thing this gate asks — an identity the run chose must not cost it a session — and it is a \
         finding about the flag rather than about the record. Screen: {}",
        live.tail(6),
    );

    let turn = one_turn(&live, &run, 0, false, began);
    assert!(
        turn.answered,
        "⚠⚠ the turn produced no answer, so there is nothing the agent would have billed and this \
         gate cannot tell a missing record from a missing turn. Over: {:?}. Screen: {}",
        turn.over,
        live.tail(6),
    );

    // ⚠ THE RECORD IS WRITTEN BY SOMEBODY ELSE'S PROCESS, so it is polled rather than asserted on
    // the first look. What is NOT allowed to be slow is the identity: a record that appears under a
    // different name would never appear under this one however long the wait.
    let mut record = None;
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        record = agent_record(&session);
        if record.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    let record = record.unwrap_or_else(|| {
        panic!(
            "⚠⚠⚠ NO RECORD IS NAMED {session}.jsonl anywhere under ~/.claude/projects. The run \
             minted an identity, handed it to the agent on the command line, and drove a turn the \
             agent answered — so the agent wrote its record somewhere this side cannot name. \
             Everything the cost signal was going to be built on assumed it could.",
        )
    });
    step(began, &format!("record: {}", record.display()));

    let billed = billed(&record);
    println!("\n== a minted identity, and what it reaches ==");
    println!("  minted:   {session}");
    println!("  record:   {}", record.display());
    println!("  requests: {}", billed.requests);
    println!("  context:  {} tokens on the last request", billed.context);
    println!(
        "  cached:   {} of them ({:.1}%)",
        billed.cached,
        if billed.context == 0 {
            0.0
        } else {
            100.0 * billed.cached as f64 / billed.context as f64
        },
    );

    assert!(
        billed.requests > 0,
        "⚠⚠⚠ the record exists under the minted name but carries no billed request. The identity \
         reaches a FILE and not a NUMBER, which is half of what this gate claims. Record: {}",
        record.display(),
    );
    assert!(
        billed.context > 0,
        "⚠⚠ {} requests are recorded and the accumulated context reads zero. The field this whole \
         signal is denominated in is `cache_read_input_tokens` + `input_tokens` + \
         `cache_creation_input_tokens`; a zero means the shape moved and every number downstream \
         of it is about to be wrong.",
        billed.requests,
    );
}

/// **A REPLACEMENT CANNOT REUSE AN ARGUMENT THAT NAMES ONE INSTANCE — and today it does, and it is
/// told it succeeded.**
///
/// # ⚠⚠⚠ What this measures, and why it is not about the feature that found it
///
/// [`PaneLifecycle::respawn`](sprag_plugin::access::PaneLifecycle::respawn) promises *"the same
/// argv, the same environment, the same working directory"*, and argues — correctly — that the loop
/// must not be the authority on what its pane runs. That argument is about WHAT RUNS. It does not
/// hold for an argument that names WHICH INSTANCE is running, because such an argument is unique by
/// construction and a second use of it is refused:
///
/// ```text
/// Error: Session ID e7eddfb2-… is already in use.
/// ```
///
/// So a pane opened with an explicit `--session-id` cannot be replaced. **This is reachable today
/// by a person** who launches one that way; nothing in sprag passes the flag yet, which is the only
/// reason it is latent rather than live.
///
/// # ⚠⚠ The shape of the failure is the finding, not the failure
///
/// The pseudoterminal spawn SUCCEEDS — the program is there and execs fine — and the agent then
/// refuses itself and exits. So `respawn` answers `Ok(new_pane)` and the caller is holding a pane
/// whose agent is already gone. A loop's `restarting` would take `session.ready` on it and wait out
/// its whole startup bound against a corpse. **A replacement that fails by reporting success is the
/// class this workspace pays most for.**
#[test]
#[ignore = "drives a LIVE agent CLI: needs credentials, costs real turns, takes minutes"]
fn a_replacement_reuses_the_argument_that_named_the_session_it_replaces() {
    let session = minted_uuid();
    let live = Live::start_args("respawn-id", &["--session-id", session.as_str()]);
    let run = RunContext::uncancellable();
    let began = Instant::now();
    step(began, &format!("minted {session}"));

    let mut barrier = Readiness::new(
        Some(ReadyWhen::Settles(live.agent.clone())),
        Some(STARTUP_BOUND),
        None,
        Attended::NoOne,
    );
    let reached = barrier
        .reached(&live.access, live.pane, &run)
        .expect("the pane must stay readable");
    step(began, &format!("first session barrier: {reached:?}"));
    assert_eq!(
        reached,
        Reached::Yes,
        "⚠⚠ the FIRST session must come up, or this gate is measuring a broken launch rather than a \
         broken replacement. Screen: {}",
        live.tail(6),
    );

    // ⚠⚠⚠ THE FIRST SESSION MUST ACTUALLY USE ITS IDENTITY, and the first run of this gate did not
    // — which is why it reported the replacement coming up healthy and refuted its own premise.
    // A `claude` that has only been STARTED has written nothing: the earlier probe that launched
    // one and killed it left no record at all. The identity is claimed by the session doing
    // something, so a gate that skips the turn hands the replacement a free name and measures
    // nothing. **A fixture that manufactures a non-answer costs the same as one that manufactures
    // an answer.**
    let first = one_turn(&live, &run, 0, false, began);
    assert!(
        first.answered,
        "⚠⚠ the first session did not answer, so it may not have claimed its identity and the \
         replacement below would be measuring an unclaimed name. Over: {:?}",
        first.over,
    );
    step(began, "first session has used its identity");

    // ── THE REPLACEMENT, exactly as `restarting` performs it ──
    let replaced = live
        .access
        .lifecycle()
        .expect("this host has the lifecycle capability")
        .respawn(live.pane);
    step(began, &format!("respawn answered: {replaced:?}"));

    let fresh = replaced.expect(
        "⚠ respawn REFUSING is a different (and better) finding than the one this gate expects — \
         it would mean the failure is visible to the caller. Record it and rewrite this gate.",
    );

    // Give the replacement's child the time it needs to refuse itself and go.
    let mut ended = false;
    let mut screen = String::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        screen = live.access.pane_full_text(fresh).unwrap_or_default();
        ended = live.access.pane_eof(fresh).unwrap_or(false);
        if ended || screen.contains("already in use") {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    let tail: String = screen.lines().rev().take(8).collect::<Vec<_>>().join(" | ");
    step(began, &format!("replacement eof={ended} screen: {tail}"));

    println!("\n== what a replacement did with an argument that named one instance ==");
    println!("  minted:              {session}");
    println!("  respawn answered:    Ok({})", fresh.0);
    println!("  replacement at eof:  {ended}");
    println!("  replacement screen:  {tail}");

    assert!(
        ended || screen.contains("already in use"),
        "⚠⚠⚠ THE REPLACEMENT CAME UP. That refutes this gate's premise — either the agent stopped \
         refusing a reused session id, or respawn stopped reusing argv. Both change the design \
         resting on this measurement, so find out which before deleting anything. Screen: {tail}",
    );
    assert!(
        screen.contains("already in use"),
        "⚠⚠ the replacement's child is gone but its screen does not say why, so this gate cannot \
         attribute the death to the reused identity rather than to anything else that kills a \
         startup. Screen: {tail}",
    );
    println!(
        "\n  ⚠⚠⚠ respawn reported Ok on a pane whose agent refused itself at startup. The caller \
         has no way to tell this from a healthy replacement."
    );
}

/// **A RUN AND ITS REPLACEMENT ARE TWO NAMED SESSIONS, AND BOTH RECORDS CAN BE FOUND** — the thing
/// the two gates above were separately unable to have.
///
/// # ⚠⚠⚠ What this proves that the others cannot
///
/// The gate above measures a launch that carries its identity in **argv the caller wrote**, and that
/// one cannot be replaced: `respawn` promises the same argv, the agent refuses the reused name, and
/// the replacement dies reporting success. The fix is not to weaken that promise. It is that sprag's
/// own naming does not live in argv at all — a pane's argv is captured BEFORE instrumentation, and
/// [`sprag_terminal::PaneArgsSource`] is consulted at EVERY BIRTH, so a replacement re-enters the
/// decision and is named afresh without `respawn` knowing anything about identities.
///
/// The same mechanism already carries the hooks instrumentation for the same reason, stated in
/// `workspace.rs`: *"a stored instrumentation would point a fresh agent at a dead socket."* A stored
/// identity goes stale in exactly that way — measured, `Error: Session ID … is already in use.`
///
/// # ⚠⚠ And the trajectory is the point, not a side effect
///
/// Two sessions, two names, two records. `claudedocs/INSIGHT-LOOP-SCORING-AND-COST-SIGNALS.md` asks
/// for an identity that outlives the iteration so *"did we do this twice"* can be asked; this is
/// that question's session-level half, and the list of names a run accumulates IS the answer.
#[test]
#[ignore = "drives a LIVE agent CLI: needs credentials, costs real turns, takes minutes"]
fn a_replacement_is_named_afresh_and_both_records_can_be_found() {
    let (live, minted) = Live::start_minting("minting");
    let run = RunContext::uncancellable();
    let began = Instant::now();

    let mut barrier = Readiness::new(
        Some(ReadyWhen::Settles(live.agent.clone())),
        Some(STARTUP_BOUND),
        None,
        Attended::NoOne,
    );
    assert_eq!(
        barrier
            .reached(&live.access, live.pane, &run)
            .expect("the pane must stay readable"),
        Reached::Yes,
        "⚠⚠ the FIRST session must come up. Screen: {}",
        live.tail(6),
    );
    let first = one_turn(&live, &run, 0, false, began);
    assert!(first.answered, "the first session must claim its identity");
    step(began, "first session has used its identity");

    let fresh = live
        .access
        .lifecycle()
        .expect("this host has the lifecycle capability")
        .respawn(live.pane)
        .expect("the replacement must spawn");
    step(began, &format!("respawn answered: Ok({})", fresh.0));

    // ⚠ The barrier is built anew for the new pane, exactly as `Session::replacing` re-arms one:
    // a latched barrier would report *already ready* about a program that has existed for
    // milliseconds, which is R379's measured defect.
    let mut replaced_barrier = Readiness::new(
        Some(ReadyWhen::Settles(live.agent.clone())),
        Some(STARTUP_BOUND),
        None,
        Attended::NoOne,
    );
    let came_up = replaced_barrier
        .reached(&live.access, fresh, &run)
        .expect("the replacement pane must stay readable");
    let screen = live.access.pane_full_text(fresh).unwrap_or_default();
    let tail: String = screen.lines().rev().take(6).collect::<Vec<_>>().join(" | ");
    step(began, &format!("replacement barrier: {came_up:?}"));

    let names = minted.lock().expect("the log").clone();
    println!("\n== a run and its replacement, each named at its own birth ==");
    for (index, name) in names.iter().enumerate() {
        println!("  birth {index}: {name}");
    }

    assert!(
        !screen.contains("already in use"),
        "⚠⚠⚠ THE REPLACEMENT WAS HANDED THE NAME ITS PREDECESSOR IS USING. The whole point of \
         minting at each birth is that this cannot happen — so either the args source was not \
         consulted on respawn, or the identity reached the pane's RECORDED argv and was replayed. \
         Screen: {tail}",
    );
    assert_eq!(
        came_up,
        Reached::Yes,
        "⚠⚠ the replacement did not come up, and not because of a reused name. Screen: {tail}",
    );
    assert_eq!(
        names.len(),
        2,
        "⚠⚠⚠ two births, two names — got {names:?}. A respawn that did not re-enter the naming \
         decision is the failure this gate exists for.",
    );
    assert_ne!(
        names[0], names[1],
        "⚠⚠⚠ both births were given the SAME name. Minting per birth is what makes replacement \
         possible at all; a repeated one is the stored-identity bug wearing the fix's clothes.",
    );

    // The replacement must use its own identity before it has a record to find.
    let second = one_turn_on(&live, fresh, &run, 1, began);
    assert!(
        second.answered,
        "the replacement must answer, or its record is missing for a reason this gate is not about",
    );

    let mut found = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        found = names.iter().filter_map(|name| agent_record(name)).collect();
        if found.len() == names.len() {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    for (name, record) in names.iter().zip(&found) {
        let billed = billed(record);
        println!(
            "  {name} -> {} requests, {} tokens of context",
            billed.requests, billed.context,
        );
    }
    assert_eq!(
        found.len(),
        names.len(),
        "⚠⚠⚠ {} sessions were named and {} records can be found. A trajectory with a hole in it \
         cannot answer what a run spent, which is the whole reason for naming. Names: {names:?}",
        names.len(),
        found.len(),
    );
    println!(
        "\n  the run's trajectory is {} named sessions, every one of them findable",
        names.len(),
    );
}

/// **A LOOP KNOWS WHAT ITS AGENT IS BEING CHARGED TO READ** — the number reaches the document, and
/// it is the same number the agent wrote about itself.
///
/// # ⚠⚠⚠ The last link, and the only one no unit test can close
///
/// Everything under this claim is separately fixed: that a minted name reaches a record, that a
/// replacement is named afresh, that the reader counts a streamed reply once. What none of them can
/// say is whether the number **crosses into the machine** — the driver puts it on `turn.done`,
/// `judging`'s entry assigns it, and a stand-in agent would prove only that a fixture's number
/// survives a datamodel.
///
/// So this asserts the two ends against each other: what `OuterLoop::context` reads out of the
/// document, and what the record on disk says, for the same live session. **Equal, or the loop is
/// holding a number about something else.**
///
/// ⚠ The pane is opened through a naming source — [`Live::start_minting`] — because a loop over an
/// UNNAMED session is the degradation, not the feature: it reads `0` and carries on, which is what
/// every other gate in this module has been driving all along.
#[test]
#[ignore = "drives a LIVE agent CLI: needs credentials, costs real turns, takes minutes"]
fn a_loop_holds_what_its_live_agent_has_been_charged_to_read() {
    use sce_rust_runtime::IScriptEngine;
    use sprag_plugin::Brief;

    const LIVE_MAX_TURNS: i64 = 3;

    let (live, minted) = Live::start_minting("spend");
    let began = Instant::now();

    let brief = Brief {
        north_star: "prove a loop can read what its own agent session is spending".to_string(),
        milestone: "state the product of 17 and 23 as a single number".to_string(),
        reference: "no tools and no files are needed; answer from arithmetic alone".to_string(),
        closing_rules: None,
        max_turns: Some(sprag_plugin::Counted::Of(LIVE_MAX_TURNS)),
        reflect_every: Some(LIVE_MAX_TURNS),
        screen_rules: None,
        // ⚠ NOBODY IS WATCHING, said rather than inherited: the patience is the document's since
        // the round that moved it, and these gates were written against `Attended::NoOne` — a run
        // that ends at the first dialog it cannot answer rather than waiting out an hour.
        may_answer: None,
        await_person_ms: Some(0),
        handback_still_ms: None,
        // ⚠ THE BARRIER'S BOUND IS THE DOCUMENT'S THREE MINUTES, inherited on purpose: a live
        // `claude` cold-starting is exactly what that number was authored for, so a gate naming its
        // own here would be measuring something no real run gets.
        ready_timeout_ms: None,
        // ⚠⚠ THE TURN'S BOUND IS SAID, because the shipped one is half an hour — a person's
        // allowance for a session doing real work, and far past what a gate may sit on. It used to
        // be an `AiLoopSpec` field; register item 300 moved it here.
        turn_within_ms: Some(TURN_BOUND.as_millis() as i64),
    };
    let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
    let mut loops = sprag_plugin::AiLoop::new(
        lua,
        live.pane,
        &brief,
        &sprag_plugin::AiLoopSpec::driving(&live.agent),
    )
    .expect("a briefed loop over a named pane starts");

    let outcome = sprag_plugin::Driver::new(sprag_plugin::Guardrails {
        max_iterations: 24,
        max_cost: None,
        max_duration: Some(Duration::from_secs(300)),
    })
    .run(&mut loops, &live.access, &RunContext::uncancellable());
    step(began, &format!("run ended: {outcome:?}"));

    let turns = loops.turns();
    let held = loops.context();
    let names = minted.lock().expect("the log").clone();
    // ⚠⚠⚠⚠ AND THIS IS NOW AN INDEPENDENT ORACLE RATHER THAN THE SAME READ TWICE. The loop reads
    // the transcript its agent STATED on its submit hook; this reads the record filed under the
    // name the HARNESS minted. Register item 431 is exactly the case where those two come apart —
    // a pane born as one session reported another, and no record of the first was ever written —
    // so a red on the size comparison below is that divergence showing up, not a flake.
    let recorded = names.first().and_then(|name| sprag_plugin::spend_of(name));

    println!("\n== what the loop knew about its own session ==");
    println!("  session:  {names:?}");
    println!("  turns:    {turns:?}");
    println!("  context held by the document: {held:?}");
    println!("  spend read from the record:   {recorded:?}");

    assert_eq!(
        turns,
        Some(1),
        "⚠⚠ the loop must have judged exactly one turn for this to be about a number rather than \
         about a run that never got going",
    );
    let recorded = recorded.expect(
        "⚠⚠⚠ the session was named and the loop drove a turn, but no record can be found under \
         that name. Everything above this claim assumed it could be.",
    );
    let held = held.expect("a machine that judged a turn holds its own `context`");
    assert!(
        held > 0,
        "⚠⚠⚠ THE DOCUMENT HOLDS ZERO after a judged turn against a NAMED session. Zero is the \
         degradation this deliberately cannot distinguish from a real level — so reaching it here, \
         where the name was minted by the harness and the record exists, means the number never \
         crossed. Recorded: {recorded:?}",
    );
    // ⚠⚠⚠ NOT EQUALITY, AND THE FIRST RUN OF THIS GATE IS WHY. It asserted the two were the same
    // number and went red at 31,754 against 31,964 — with the record showing **two** requests where
    // the document had judged **one**. Nothing was wrong: `closing` sends the report prompt after
    // the judged turn, so the session bills again while this test is still reading. The document
    // holds A LEVEL AT A MOMENT and the record keeps growing past it; an assertion that they are
    // equal is an assertion that nothing happened in between, which is false by construction here.
    //
    // So what is pinned is what is actually true of the pair: the level the loop holds is one this
    // session really reached, and it is the same size rather than merely non-zero. The ordering is
    // the sharp half — a document holding MORE than the record has ever reached would mean the
    // number came from somewhere else.
    let held = u64::try_from(held).expect("a context is not negative");
    assert!(
        held <= recorded.context,
        "⚠⚠⚠ the loop holds {held} and its agent's record has never exceeded {}. A level the \
         session never reached did not come from the session.",
        recorded.context,
    );
    assert!(
        held * 2 > recorded.context,
        "⚠⚠ the loop holds {held} against a record at {}. Ordered correctly but not the same size, \
         which is what reading the WRONG session's record would look like — the check that a bare \
         `> 0` would pass straight through.",
        recorded.context,
    );
    println!(
        "\n  the loop holds {held} tokens as of the turn it judged; the record has since reached \
         {} over {} requests ({} cached)",
        recorded.context, recorded.requests, recorded.cached,
    );
}

/// **DOES A DESIGN QUESTION REACH `screening` AT ALL, AND IF IT DOES, WHAT WORDS DOES IT CARRY?**
///
/// # ⚠⚠⚠ The premise `screening` has been blocked on since it was written
///
/// `ai_loop.scxml` ships a `screen_rules` placeholder that claims nothing, and says why in its own
/// words: *"the dialogs a loop meets are MEASURED for tool permissions and unmeasured for design
/// questions, and quoting words nobody has seen is the mistake this file made the last time."*
///
/// R383 measured the permission population — write, edit and bash all carry `Do you want to` and
/// offer `1. Yes`. Nothing has ever measured the other kind. So an author who wants a standing
/// instruction for *"stop asking me which approach to take"* has no words to quote, and
/// `screening`'s whole authored half is guesswork until this runs.
///
/// # ⚠⚠ What can and cannot be asserted here
///
/// Only the CONTROL asserts. A design probe that does not produce a dialog is a **measurement** —
/// it says the population is narrower than *"every decision"* — and asserting one would be
/// asserting a behaviour of somebody else's model. R383's `bash-reads` row is the same shape.
///
/// The control is what makes the silence readable: if a known-gating call also failed to raise a
/// dialog, the finding would be about this harness rather than about design questions.
///
/// # ⚠⚠⚠ And the sharpest thing it can find is a COLLISION
///
/// If a design dialog also carries `Do you want to`, then the consent clause every loop is told to
/// arm — `asked: "Do you want to" → "1. Yes"` — would **auto-approve design decisions**, silently,
/// with the agent's first option. That would make the recommended arming actively wrong, and it is
/// exactly what the coverage check at the end asks.
#[test]
#[ignore = "drives a LIVE agent CLI: needs credentials, costs real turns, takes minutes"]
fn what_a_live_agent_asks_when_the_decision_is_a_design_one() {
    use sprag_plugin::{Consent, Consents};

    const ASKS_WITHIN: Duration = Duration::from_secs(120);
    const CAPTURE_ROWS: usize = 16;

    /// One probe: a label, what to seed the directory with, the ask, and whether a dialog is a
    /// CONTROL (must appear) or a measurement (may or may not).
    ///
    /// ⚠⚠⚠ NOT ONE OF THESE ASKS THE AGENT TO ASK. A prompt containing *"ask me"* would
    /// manufacture the dialog this gate exists to find out about — R379's fixture lesson, met at
    /// the prompt rather than at the assertion. Each is a genuine fork with the preference left
    /// out, which is the situation a loop's milestone puts an agent in.
    ///
    /// ⚠ NAMED, because a four-place tuple of borrowed slices is what `clippy::type_complexity`
    /// refuses — and the alternative it offers, an `allow`, would hide the next one too.
    type Probe = (
        &'static str,
        &'static [(&'static str, &'static str)],
        &'static str,
        bool,
    );
    const PROBES: &[Probe] = &[
        (
            "control-permission",
            &[],
            "Create a file called PROBE.txt whose only contents are the word ready.",
            true,
        ),
        (
            "design-two-approaches",
            &[],
            "This directory needs a settings store that survives a restart. A single JSON file \
             and a small SQLite database both work here and I have no constraint either way. \
             Write the design you land on to DESIGN.txt.",
            false,
        ),
        (
            "design-underspecified",
            &[
                ("notes.txt", "kept\n"),
                ("old-draft.txt", "superseded\n"),
                ("scratch.tmp", "temporary\n"),
            ],
            "Clean up this directory.",
            false,
        ),
        (
            "design-irreversible",
            &[
                ("report-final.txt", "the delivered report\n"),
                ("report-v1.txt", "an earlier draft\n"),
                ("report-v2.txt", "a later draft\n"),
            ],
            "Remove the report files that are no longer needed here.",
            false,
        ),
    ];

    let mut captured: Vec<(&str, Vec<String>, sprag_detect::Question)> = Vec::new();
    let mut silent: Vec<(&str, String)> = Vec::new();

    for (label, seed, ask, is_control) in PROBES {
        let live = Live::start(&format!("design-{label}"));
        let began = Instant::now();
        for (name, body) in *seed {
            std::fs::write(live.scratch.path().join(name), body)
                .expect("the scratch directory is this measurement's own");
        }

        let run = RunContext::uncancellable();
        let reached = Readiness::new(
            Some(ReadyWhen::Settles(live.agent.clone())),
            Some(STARTUP_BOUND),
            None,
            Attended::NoOne,
        )
        .reached(&live.access, live.pane, &run)
        .expect("the pane must stay readable");
        assert_eq!(
            reached,
            Reached::Yes,
            "{label}: the agent must be up and at rest before it is spoken to: {}",
            live.tail(3),
        );

        let mut done = Completion::new(DoneWhen::Settles);
        done.begin(&live.access, live.pane);
        let delivered = deliver(
            &live.access,
            &run,
            live.pane,
            ask,
            &Delivery {
                confirm: Some(ask.chars().take(40).collect()),
                then_press: vec![KeyStroke::named("Enter")],
                ..Delivery::new()
            },
        )
        .expect("the pane must take the prompt");
        assert!(
            !matches!(delivered, Delivered::Unconfirmed { .. }),
            "{label}: a live agent PAINTS what is typed into its composer: {delivered:?}",
        );

        // The product's own turn-ending vocabulary does the measuring, for R383's reason: `Over` is
        // what a loop's `watch` reads, so what is seen here is what a run would have met.
        let over = done.wait(&live.access, live.pane, ASKS_WITHIN, &run);
        let rows: Vec<String> = live
            .access
            .pane_rows(live.pane)
            .unwrap_or_default()
            .into_iter()
            .map(|row| row.text.trim_end().to_owned())
            .filter(|row| !row.trim().is_empty())
            .rev()
            .take(CAPTURE_ROWS)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();

        println!(
            "\n== {label} ({}) after {:?} — {} ==",
            if *is_control {
                "CONTROL"
            } else {
                "measurement"
            },
            began.elapsed(),
            match &over {
                Over::Yes => "decided WITHOUT asking".to_owned(),
                Over::Asking(Some(_)) => "ASKED, and the parser read it".to_owned(),
                Over::Asking(None) => "BLOCKED and the parser could NOT read it".to_owned(),
                other => format!("{other:?}"),
            },
        );

        let Over::Asking(asked) = over else {
            assert!(
                !is_control,
                "⚠⚠⚠ {label}: THE CONTROL DID NOT ASK ({over:?}). Every silence below is now \
                 uninterpretable — it would say as much about this harness as about design \
                 questions. Check `Live::start`'s `--setting-sources` before reading anything \
                 else. Screen: {}",
                live.tail(6),
            );
            let tail = rows.iter().rev().take(4).rev().cloned().collect::<Vec<_>>();
            println!("  ⚠ MEASURED: no dialog. What it did instead:");
            for row in &tail {
                println!("      {row}");
            }
            silent.push((label, tail.join(" | ")));
            continue;
        };
        let question = asked.unwrap_or_else(|| {
            panic!(
                "⚠⚠⚠ {label}: the pane is BLOCKED and `sprag_detect::question` could not read what \
                 it is asking. For a DESIGN dialog that is its own finding — the detector's rules \
                 were written against permission dialogs. Rows:\n{}",
                rows.join("\n"),
            )
        });
        println!(
            "    /// Captured from a live `{}` ({label}).\n    const {}_DIALOG: &[&str] = &[\n{}\n    ];",
            live.agent,
            label.to_uppercase().replace('-', "_"),
            rows.iter()
                .map(|row| format!("        {row:?},"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        println!(
            "  parsed: asked={:?}\n          choices={:?}",
            question.asked,
            question
                .choices
                .iter()
                .map(|c| (c.number, c.label.as_str()))
                .collect::<Vec<_>>(),
        );
        captured.push((label, rows, question));
    }

    // ══ WHAT THE ARMED CONSENT WOULD DO TO EACH OF THEM ══
    let armed = Consents::of(vec![
        Consent::parse("Do you want to".to_string(), "Yes".to_string())
            .expect("both needles are non-empty"),
    ])
    .expect("a non-empty consent list");

    // ⚠⚠⚠ THE ARMED CLAUSE IS ALSO THE CLASSIFIER, and that is not a shortcut. R383 established
    // that every PERMISSION dialog carries `Do you want to`; a dialog this clause cannot cover is
    // therefore one of the other kind, sorted by the product's own reader rather than by a taxonomy
    // this gate invented.
    println!("\n== the clause every loop is told to arm, against each dialog ==");
    let (mut permission, mut decision) = (Vec::new(), Vec::new());
    for (label, _, question) in &captured {
        match armed.covers(question) {
            Ok(chose) => {
                println!(
                    "  {label:<24} PERMISSION -> covered, would take {}. {:?}",
                    chose.number, chose.label,
                );
                permission.push(*label);
            }
            Err(why) => {
                println!("  {label:<24} DECISION   -> not covered ({why:?})");
                decision.push((*label, question));
            }
        }
    }

    println!("\n== what this establishes ==");
    println!("  permission-shaped: {permission:?}");
    println!(
        "  decision-shaped:   {:?}",
        decision.iter().map(|d| d.0).collect::<Vec<_>>()
    );
    println!(
        "  no dialog at all:  {:?}",
        silent.iter().map(|s| s.0).collect::<Vec<_>>()
    );
    for (label, question) in &decision {
        println!("\n  a DECISION dialog's `asked`, which is what a `when` needle must sit inside:");
        println!("    {label}: {:?}", question.asked);
    }

    assert!(
        permission.contains(&"control-permission"),
        "⚠⚠⚠ THE CONTROL WAS NOT COVERED by the clause R383 measured against every permission \
         dialog. Nothing below is readable — the finding would be about this clause rather than \
         about design questions.",
    );

    // ⚠⚠⚠ THE SAFETY PROPERTY, and the reason this gate checks coverage rather than only
    // capturing: a DECISION dialog must reach a person or an authored rule. A consent that covered
    // one would answer *which of these files should I delete* with the agent's first option,
    // having consulted nobody — the exact act `screen.rs` removed the `keys` field to prevent,
    // arriving by the other door.
    for (label, question) in &decision {
        assert!(
            armed.covers(question).is_err(),
            "{label}: a decision dialog must not be answerable by the permission clause",
        );
    }
    assert!(
        !decision.is_empty(),
        "⚠⚠ NOT ONE probe produced a decision dialog, so this gate cannot say what words one \
         carries and `screen_rules` stays unauthorable. That is a finding about the probes rather \
         than about the agent — write a sharper fork.",
    );

    // ⚠⚠ AND THE ASYMMETRY IS THE HEADLINE. A design FORK did not produce a decision dialog: the
    // agent chose, wrote the reasoning, and asked only for permission to create the file. What did
    // produce one is an IRREVERSIBLE act over things it did not create. So the population a
    // standing instruction is for is much narrower than *"design decisions"*.
    println!(
        "\n  ⚠⚠⚠ {} of {} probes were answered by the agent DECIDING and asking only for \
         permission to act. A standing stance therefore belongs in the BRIEF — `priming` composes \
         it into every prompt — and `screen_rules` reaches only the {} that actually asked.",
        permission.len() - 1,
        captured.len() - 1,
        decision.len(),
    );
}

/// **THE JUDGE, THROUGH THE PRODUCT'S OWN PATH, AGAINST THE DIALOGS IT HAS TO SEPARATE.**
///
/// # ⚠⚠⚠ Why this exists when a shell probe already answered
///
/// A probe answered the PREMISE — a cheap model can tell a design-bearing dialog from a routine
/// one — by asking `claude -p` for `DESIGN` or `ROUTINE`. Then the product was built, and it asks a
/// different question in different words: the author's own criterion, answered `YES` or `NO`,
/// rendered by [`judge::render`](sprag_plugin::judge) with the dialog's CHOICES included.
///
/// **The measured thing and the shipped thing are not the same thing.** This drives the shipped
/// one. A gate that trusted the probe would be trusting a prompt nobody ships.
///
/// # ⚠⚠ The pair is the whole test
///
/// Two of these four dialogs ask the identical question — `Do you want to create <file>.txt?` —
/// and differ only in what the diff behind them commits. No quote separates them. If the judge
/// does not, `cond="_event.data.design"` has nothing to decide on and the feature is dead.
#[test]
#[ignore = "drives a LIVE agent CLI: needs credentials, costs real turns, takes minutes"]
fn the_judge_separates_a_design_dialog_from_a_routine_one() {
    use sprag_detect::{Choice, Question};
    use sprag_plugin::judge::{JudgeSpec, judges};

    /// ⚠⚠⚠ THE OWNER'S OWN SENTENCE, arrived at by being shown the captured dialogs and saying
    /// which of them they would want turned down.
    ///
    /// **What separates them is not the subject, it is who is deciding.** Three of the captured
    /// dialogs offer yes-or-no on ONE act the agent has already chosen; one hands the person a set
    /// of substantively different outcomes and waits. The last is the agent's own setup prompt,
    /// about nothing the loop asked for.
    ///
    /// ⚠ The `design-write` fixture is the sharp one and this gate labelled it wrongly once: the
    /// ARCHITECTURE was decided there — JSON over SQLite — but it was decided silently, BEFORE the
    /// dialog, and the dialog asks only whether a file may be written. Nothing is being put to the
    /// person, so it is a NO. An earlier version scored the judge against the opposite label,
    /// which was this file's author's opinion rather than the owner's criterion.
    const CRITERION: &str = "the agent is asking the person to CHOOSE between substantively \
                             different outcomes for the work it is doing. Asking permission to \
                             carry out one step it has already decided on is NOT this, however \
                             consequential that step is. Neither is a prompt about the tool's own \
                             setup rather than about the work.";
    const ROUNDS: usize = 3;

    fn asked(lines: &[&str], choices: &[(u32, &str)]) -> Question {
        Question {
            asked: lines.iter().map(|l| (*l).to_owned()).collect(),
            choices: choices
                .iter()
                .map(|(number, label)| Choice {
                    number: *number,
                    label: (*label).to_owned(),
                    selected: *number == 1,
                })
                .collect(),
        }
    }

    // ⚠⚠⚠ VERBATIM from `what_a_live_agent_asks_when_the_decision_is_a_design_one`. Fixtures a
    // person wrote would be fixtures asserting what a person imagines a dialog looks like — the
    // failure R383 recorded and this module keeps paying to avoid.
    let dialogs: Vec<(&str, Question, bool)> = vec![
        (
            "routine-write",
            asked(
                &[
                    "● Write(PROBE.txt)",
                    "Create file",
                    "PROBE.txt",
                    "1 ready",
                    "Do you want to create PROBE.txt?",
                ],
                &[
                    (1, "Yes"),
                    (2, "Yes, allow all edits during this session (shift+tab)"),
                    (3, "No"),
                ],
            ),
            false,
        ),
        (
            "design-write",
            asked(
                &[
                    "226 The migration is contained by design: the API above is the only thing",
                    "227 callers touch, so the backing store swaps behind it. A one-shot importer",
                    "228 reads the JSON, writes the table, and renames the file to",
                    "229 settings.json.migrated. Do not pre-build for this — an abstraction layer",
                    "230 added now to support a database we may never need costs more than the",
                    "231 migration would.",
                    "Do you want to create DESIGN.txt?",
                ],
                &[
                    (1, "Yes"),
                    (2, "Yes, allow all edits during this session (shift+tab)"),
                    (3, "No"),
                ],
            ),
            // ⚠⚠⚠ THE OWNER SAYS NO, and this is the label this gate got wrong before. The
            // ARCHITECTURE was decided — JSON over SQLite — but it was decided BEFORE the dialog,
            // silently, and the dialog asks only whether a file may be written. Nothing is being
            // put to the person. An earlier version of this gate scored the judge against the
            // opposite label, which was its author's opinion rather than a measurement.
            false,
        ),
        (
            "routine-bash",
            asked(
                &[
                    "● Bash(for f in notes.txt old-draft.txt scratch.tmp; do cat \"$f\"; done)",
                    "Bash command",
                    "Show contents of the three files",
                    "Do you want to proceed?",
                ],
                &[(1, "Yes"), (2, "No")],
            ),
            false,
        ),
        (
            "design-delete",
            asked(
                &["☐ Delete scope", "Which report files should I delete?"],
                &[
                    (1, "Both drafts (Recommended)"),
                    (2, "Only report-v1.txt"),
                    (3, "All three"),
                ],
            ),
            true,
        ),
        // ⚠⚠⚠ THE DIALOG THAT IS NOT ABOUT THE WORK AT ALL, and it is here because it walked into
        // a measurement and was counted. `does_an_agent_ask_the_person_about_an_architecture_
        // decision` read `Over::Asking` on three tasks and every one of them was THIS — the
        // agent's own onboarding prompt, arriving after the turn, about nothing the loop asked
        // for. That gate went green on it.
        //
        // A loop meets these too, and a judge that said YES here would refuse the person's own
        // setup prompt and tell the agent to reconsider its architecture. Being able to turn this
        // down is not a bonus: it is the difference between a judge and a dialog detector.
        (
            "unrelated-onboarding",
            asked(
                &[
                    "Set up auto mode for your environment?",
                    "Auto mode lets Claude act without asking first. Telling it which repos you \
                     trust and what data is sensitive gives it clearer guardrails on what's safe \
                     to run. Claude will explore your repo and recent sessions, then review the \
                     settings it suggests with you.",
                ],
                &[(1, "Set it up"), (2, "Not now"), (3, "Don't show again")],
            ),
            false,
        ),
    ];

    let live = Live::start("judge");
    let run = RunContext::uncancellable();
    let began = Instant::now();
    let spec = JudgeSpec {
        argv: vec![
            live.agent.clone(),
            "-p".to_owned(),
            "--model".to_owned(),
            "haiku".to_owned(),
            "--setting-sources".to_owned(),
            "project".to_owned(),
        ],
        within: Duration::from_secs(90),
    };

    // ⚠⚠ THE A/B THIS GATE USED TO RUN IS SETTLED AND GONE. It compared rendering the dialog's
    // options against withholding them; withholding them removed both false YES and `render` now
    // does that unconditionally, so passing a question with or without options produces the same
    // prompt and comparing them would compare nothing. The finding lives in `render`'s own doc.
    let mut report: Vec<(String, bool, Vec<Option<bool>>)> = Vec::new();
    let mut slowest = Duration::ZERO;

    println!("\n== the shipped judge, against the owner's labels ==");
    for (label, question, expected) in &dialogs {
        let mut holds = Vec::new();
        for _ in 0..ROUNDS {
            let judged = judges(&live.access, &run, CRITERION, question, &spec);
            if let Some(judged) = &judged {
                slowest = slowest.max(judged.took);
            }
            holds.push(judged.map(|j| j.holds));
        }
        println!(
            "  {label:<22} want {:<4}  got {holds:?}",
            if *expected { "YES" } else { "NO" },
        );
        report.push(((*label).to_owned(), *expected, holds));
    }
    step(began, &format!("slowest judgement: {slowest:?}"));

    let agreed: usize = report
        .iter()
        .map(|(_, want, got)| got.iter().filter(|g| **g == Some(*want)).count())
        .sum();
    let out_of = report.len() * ROUNDS;

    // ⚠⚠⚠ THE TWO DIRECTIONS ARE NOT WORTH THE SAME, so they are asserted apart.
    //
    // A FALSE YES refuses a tool call the person never wanted refused — the loop presses the
    // refusing key on a permission, or on the agent's own setup prompt, and tells it to reconsider
    // an architecture nobody was choosing. That is this mechanism doing harm.
    //
    // A FALSE NO leaves the dialog to `screening` and then to the person: exactly what happens
    // today, for a run with no criterion at all. The feature did not fire; nothing broke.
    let (mut false_yes, mut false_no) = (Vec::new(), Vec::new());
    for (label, want, holds) in &report {
        for got in holds {
            match (got, want) {
                (Some(true), false) => false_yes.push(label.clone()),
                (Some(false), true) => false_no.push(label.clone()),
                _ => {}
            }
        }
    }
    println!("\n== the two directions, which are not worth the same ==");
    println!("  false YES (refuses what nobody wanted refused): {false_yes:?}");
    println!("  false NO  (degrades to today's behaviour):      {false_no:?}");

    // ⚠⚠⚠ WHAT FIVE DIALOGS TIMES THREE ROUNDS CAN AND CANNOT SUPPORT, and this gate asked for
    // more than that twice before settling here.
    //
    // It cannot support **perfection**. The difference between 14/15 and 15/15 is one judgement,
    // and this judge is not deterministic — the same dialog has come back both ways across runs.
    // A gate that demanded zero errors from this sample would go red on noise, and the way to make
    // it green would be to keep editing the prompt until these five pass, which is fitting the
    // fixtures rather than measuring the product.
    //
    // It CAN support two things, and they are the two the feature stands or falls on:
    //
    // * the population a loop meets constantly — PERMISSIONS — must not be refused. Nine
    //   judgements over three dialogs, and a leak here fires on nearly every turn;
    // * the case the feature exists for must actually fire, or there is no feature.
    //
    // Everything else is REPORTED. The onboarding prompt's leak is real and is left visible rather
    // than asserted away: what it costs is one refusal of the agent's own setup prompt, which
    // closes it and lets the run continue.
    let permissions: Vec<&(String, bool, Vec<Option<bool>>)> = report
        .iter()
        .filter(|(label, _, _)| label.starts_with("routine") || label == "design-write")
        .collect();
    let refused_a_permission: Vec<&String> = permissions
        .iter()
        .filter(|(_, _, holds)| holds.contains(&Some(true)))
        .map(|(label, _, _)| label)
        .collect();
    assert!(
        refused_a_permission.is_empty(),
        "⚠⚠⚠ THE JUDGE WOULD REFUSE A PERMISSION: {refused_a_permission:?}. This is the population \
         a loop meets on nearly every turn — write, edit, run — so a leak here does not degrade \
         the run, it stops it. Rows: {report:?}",
    );

    let target = report
        .iter()
        .find(|(label, _, _)| label == "design-delete")
        .expect("the one dialog the owner said is theirs to intercept");
    let caught = target.2.iter().filter(|got| **got == Some(true)).count();
    assert!(
        caught >= 2,
        "⚠⚠ THE FEATURE DID NOT FIRE: the judge caught the owner's own example {caught} time(s) \
         in {ROUNDS}. Below two this is not a rate, it is a mechanism that does not work.",
    );

    assert!(
        slowest < Duration::from_secs(30),
        "⚠⚠ the slowest judgement took {slowest:?} and the agent stands blocked for every second \
         of it. The premise was probed at 4-6 s; this far out means the cost of judging moved.",
    );
    println!(
        "\n  agreed {agreed}/{out_of}; permissions never refused; the owner's example caught \
         {caught}/{ROUNDS}; slowest {slowest:?}"
    );
    if !false_yes.is_empty() {
        println!(
            "  ⚠ REPORTED, NOT ASSERTED: {false_yes:?} would be refused. Worth watching, not worth \
             fitting a five-dialog sample to."
        );
    }
}

/// **DOES A LIVE AGENT ASK THE USER ABOUT AN ARCHITECTURE DECISION, OR DECIDE ALONE?**
///
/// # ⚠⚠⚠ Why the earlier probes measured the wrong population
///
/// `what_a_live_agent_asks_when_the_decision_is_a_design_one` captured four dialogs and all four
/// were about FILE OPERATIONS — write this, run that, delete which. The thing a standing
/// instruction is actually wanted for is narrower and different: **a decision about how software
/// is built**, and specifically the moment the agent puts that decision to the person.
///
/// One of those probes came close and settled nothing in the right direction: told that a settings
/// store could be JSON or SQLite and that the asker had no preference, the agent **chose alone**,
/// wrote 256 lines of reasoning, and asked only for permission to create the file. Pressing the
/// refusing key at that dialog interrupts a file write, not a decision.
///
/// # ⚠⚠ So this measures ONE thing, and it labels nothing
///
/// **Did the agent ask the person?** No expected column, no accuracy, no ground truth invented by
/// whoever wrote this test — the last gate's scoring was agreement with its author's opinion, and
/// that is not a measurement. A dialog appearing is the fact; what it says is the capture.
///
/// ⚠ Each task ships REAL FILES rather than a hypothetical, and none of them says *"I have no
/// preference"* — that sentence is what handed the last probe's decision to the agent.
#[test]
#[ignore = "drives a LIVE agent CLI: needs credentials, costs real turns, takes minutes"]
fn does_an_agent_ask_the_person_about_an_architecture_decision() {
    const ASKS_WITHIN: Duration = Duration::from_secs(150);
    const CAPTURE_ROWS: usize = 18;

    /// A label, the files the task is about, and the task.
    ///
    /// ⚠ NAMED for `PROBES`' reason, one gate above: `clippy::type_complexity` refuses the tuple
    /// inline, and an `allow` would silence the next one too.
    type Task = (
        &'static str,
        &'static [(&'static str, &'static str)],
        &'static str,
    );
    const TASKS: &[Task] = &[
        (
            "caching",
            &[(
                "store.py",
                "import json\n\n\
                 def read_profile(user_id):\n\
                 \x20   with open(f'profiles/{user_id}.json') as f:\n\
                 \x20       return json.load(f)\n\n\
                 def list_active():\n\
                 \x20   return [read_profile(u) for u in open('active.txt').read().split()]\n",
            )],
            "read_profile in store.py is called many times per request and hits the disk every \
             time. Add caching. Write what you land on to store.py.",
        ),
        (
            "concurrency",
            &[(
                "counter.py",
                "import json\n\n\
                 def bump(name):\n\
                 \x20   counts = json.load(open('counts.json'))\n\
                 \x20   counts[name] = counts.get(name, 0) + 1\n\
                 \x20   json.dump(counts, open('counts.json', 'w'))\n",
            )],
            "bump() in counter.py loses updates when two workers call it at the same time. Make it \
             correct under concurrent callers.",
        ),
        (
            "errors",
            &[(
                "parse.py",
                "def parse_port(text):\n\
                 \x20   return int(text)\n\n\
                 def parse_config(lines):\n\
                 \x20   return {k: parse_port(v) for k, v in (l.split('=') for l in lines)}\n",
            )],
            "parse.py raises on bad input and callers cannot tell which line was wrong. Change how \
             it reports failure.",
        ),
        (
            "auth",
            &[(
                "handler.py",
                "ROUTES = {}\n\n\
                 def handle(request):\n\
                 \x20   return ROUTES[request['path']](request)\n",
            )],
            "handler.py serves every request to anyone. Add authentication.",
        ),
    ];

    let mut asked_the_person: Vec<&str> = Vec::new();
    let mut decided_alone: Vec<(&str, String)> = Vec::new();

    for (label, seed, task) in TASKS {
        let live = Live::start(&format!("arch-{label}"));
        let began = Instant::now();
        for (name, body) in *seed {
            std::fs::write(live.scratch.path().join(name), body)
                .expect("the scratch directory is this measurement's own");
        }

        let run = RunContext::uncancellable();
        let reached = Readiness::new(
            Some(ReadyWhen::Settles(live.agent.clone())),
            Some(STARTUP_BOUND),
            None,
            Attended::NoOne,
        )
        .reached(&live.access, live.pane, &run)
        .expect("the pane must stay readable");
        assert_eq!(reached, Reached::Yes, "{label}: {}", live.tail(3));

        let mut done = Completion::new(DoneWhen::Settles);
        done.begin(&live.access, live.pane);
        let delivered = deliver(
            &live.access,
            &run,
            live.pane,
            task,
            &Delivery {
                confirm: Some(task.chars().take(40).collect()),
                then_press: vec![KeyStroke::named("Enter")],
                ..Delivery::new()
            },
        )
        .expect("the pane must take the prompt");
        assert!(
            !matches!(delivered, Delivered::Unconfirmed { .. }),
            "{label}: {delivered:?}",
        );

        let over = done.wait(&live.access, live.pane, ASKS_WITHIN, &run);
        let rows: Vec<String> = live
            .access
            .pane_rows(live.pane)
            .unwrap_or_default()
            .into_iter()
            .map(|row| row.text.trim_end().to_owned())
            .filter(|row| !row.trim().is_empty())
            .rev()
            .take(CAPTURE_ROWS)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();

        match &over {
            Over::Asking(question) => {
                println!(
                    "\n== {label}: ASKED THE PERSON, after {:?} ==",
                    began.elapsed()
                );
                for row in &rows {
                    println!("    {row}");
                }
                if let Some(question) = question {
                    println!(
                        "  parsed: asked={:?}\n          choices={:?}",
                        question.asked,
                        question
                            .choices
                            .iter()
                            .map(|c| (c.number, c.label.as_str()))
                            .collect::<Vec<_>>(),
                    );
                } else {
                    println!("  ⚠ blocked, and the parser could not read the question");
                }
                asked_the_person.push(label);
            }
            other => {
                let tail = rows.iter().rev().take(6).rev().cloned().collect::<Vec<_>>();
                println!(
                    "\n== {label}: DECIDED ALONE ({other:?}), after {:?} ==",
                    began.elapsed(),
                );
                for row in &tail {
                    println!("    {row}");
                }
                decided_alone.push((label, tail.join(" | ")));
            }
        }
    }

    println!("\n== the only thing this gate measures ==");
    println!("  asked the person: {asked_the_person:?}");
    println!(
        "  decided alone:    {:?}",
        decided_alone.iter().map(|d| d.0).collect::<Vec<_>>()
    );

    // ⚠⚠⚠ THE ASSERTION IS ABOUT REACHABILITY AND NOTHING ELSE. `redirecting` is entered from a
    // dialog. If an agent never puts an architecture decision to the person, there is no dialog to
    // judge and the whole mechanism cannot reach the case it was built for — which is a finding
    // about the design, not a failing test, and has to be impossible to read past.
    assert!(
        !asked_the_person.is_empty(),
        "⚠⚠⚠ NOT ONE ARCHITECTURE TASK PRODUCED A QUESTION TO THE PERSON. The agent decided all \
         {} of them alone. A dialog-triggered mechanism cannot intercept a decision that raises no \
         dialog, so `cond=\"_event.data.design\"` would never fire on the population it was built \
         for. What each did instead: {decided_alone:?}",
        decided_alone.len(),
    );
}

/// ⚠⚠⚠ **WHICH READER CAN STILL SEE A REPORT THAT SCROLLED** — the premise register item 121's
/// answer rests on, measured before the answer was built.
///
/// # The question, and why it had to be asked of a live agent
///
/// A loop's closing report is the one piece of a run that outlives its sessions (item 121), and
/// capturing it means choosing where to read it from. This workspace has two readers for *what a
/// pane produced since a mark*, and their difference is stated in [`Agent::capture`]'s own doc:
///
/// * [`RowTrail`] compares the RENDERING — repaint-proof, and **the rows that scrolled off were
///   never in it at all**. Every reader in the outer driver used it (`said_done`, `judged`,
///   `proposed`), so it is the one a new reader would reach for by habit. ⚠ `said_done` has since
///   moved to the address for the same reason this measurement gives, and register item 270 is
///   where the cost of the habit was paid a second time: a marker whose sentence had scrolled past
///   the top of the grid converged a run on its own instruction.
/// * [`PaneOutputLines::pane_lines_since`] is an ADDRESS into the pane's logical lines, so it
///   survives a scroll and reports what the retained history evicted.
///
/// The doc calls the trail *"the degradation"* — but that sentence was written about a one-shot CLI
/// printing to a cooked-mode pty, and a loop's peer is **a full-screen TUI that repaints in place**.
/// Whether a program like that commits anything to the line store at all is a fact about the
/// program, not about the reader, and nothing in this tree had asked it. A closing report is
/// precisely the long output where the difference decides whether a person gets the whole account
/// or its last page.
///
/// # What it does
///
/// One session, one turn, and the turn asks for a reply **taller than the pane** — sixty labelled
/// lines against forty rows, so the top of it is guaranteed to have gone by the time anyone reads.
/// Both readers are marked before the prompt goes in and both are read after the turn ends, and the
/// assertion is the discriminator rather than the print-out: **the first line must be readable
/// through the address and must be gone from the rendering.**
///
/// ⚠ Deterministic content on purpose — `LINE-1 … LINE-60`, no prose. What is being measured is a
/// READER, so the reply has to be something whose loss is unambiguous; asking a model to write an
/// essay would make the gate's own subject matter a variable.
///
/// ⚠⚠ **A FAILURE HERE IS A FINDING, NOT A BROKEN TEST.** If the address loses the line too, then
/// no reader in this workspace can carry a long report off a TUI pane, and the capture built on it
/// must say what it cannot promise. That is the sentence the assertion is written to force.
#[test]
#[ignore = "drives a LIVE agent CLI: needs credentials, costs real turns, takes minutes"]
fn what_a_live_agents_report_looks_like_to_a_reader() {
    /// How many lines the agent is asked for — comfortably past [`PANE_SIZE`]'s forty rows, so the
    /// opening of the reply cannot still be on the grid.
    const REPORT_LINES: usize = 60;
    /// A reply this long is many turns' worth of painting; the bound is generous because what is
    /// being measured is the reader and not the speed.
    const REPORT_WITHIN: Duration = Duration::from_secs(180);

    let live = Live::start("report-reader");
    let began = Instant::now();
    let run = RunContext::uncancellable();
    let reached = Readiness::new(
        Some(ReadyWhen::Settles(live.agent.clone())),
        Some(STARTUP_BOUND),
        None,
        Attended::NoOne,
    )
    .reached(&live.access, live.pane, &run)
    .expect("the pane must stay readable");
    assert_eq!(
        reached,
        Reached::Yes,
        "the agent must be up and at rest before it is spoken to: {}",
        live.tail(3),
    );

    // ⚠⚠ BOTH MARKS BEFORE A BYTE GOES IN, and for `Completion::begin`'s reason: a baseline taken
    // after the injection cannot tell the reply from the prompt's own echo.
    let trail = sprag_plugin::RowTrail::mark(&live.access, live.pane);
    let address = live
        .access
        .output_lines()
        .and_then(|stream| stream.pane_lines_since(live.pane, u64::MAX))
        .map(|since| since.next);
    step(began, &format!("marked: address={address:?}"));

    let ask = format!(
        "Print exactly {REPORT_LINES} lines and nothing else. Line n must be LINE-n, so the first \
         is LINE-1 and the last is LINE-{REPORT_LINES}. Do not number them any other way and do \
         not add commentary."
    );
    let mut done = Completion::new(DoneWhen::Settles);
    done.begin(&live.access, live.pane);
    let delivered = deliver(
        &live.access,
        &run,
        live.pane,
        &ask,
        &Delivery {
            confirm: Some(ask.chars().take(40).collect()),
            then_press: vec![KeyStroke::named("Enter")],
            ..Delivery::new()
        },
    )
    .expect("the pane must take the prompt");
    assert!(
        !matches!(delivered, Delivered::Unconfirmed { .. }),
        "a live agent PAINTS what is typed into its composer: {delivered:?}",
    );
    let over = done.wait(&live.access, live.pane, REPORT_WITHIN, &run);
    step(began, &format!("the report turn ended {over:?}"));

    let rendered = trail.fresh(&live.access, live.pane);
    let since = live
        .access
        .output_lines()
        .zip(address)
        .and_then(|(stream, mark)| stream.pane_lines_since(live.pane, mark));
    let addressed: Vec<String> = since
        .as_ref()
        .map(|since| since.lines.clone())
        .unwrap_or_default();
    let lost = since.as_ref().map_or(0, |since| since.lost);

    println!(
        "  the rendering  : {} row(s) changed, first={:?} last={:?}",
        rendered.len(),
        rendered.first(),
        rendered.last(),
    );
    println!(
        "  the address    : {} logical line(s), {lost} lost, first={:?} last={:?}",
        addressed.len(),
        addressed.first(),
        addressed.last(),
    );
    // ⚠⚠⚠ EVERY ADDRESSED LINE, VERBATIM, and this is the half of the probe a capture is written
    // from. Knowing that the address survives a scroll says nothing about WHAT ELSE is in it — the
    // prompt this run typed comes back re-wrapped by the agent's own composer, and the TUI's box
    // rules and footer are output like any other. Anything a capture strips has to be chosen off
    // this list rather than imagined, because the direction that cannot be undone is deleting a
    // line the agent meant.
    for (at, line) in addressed.iter().enumerate() {
        println!("    [{at:>3}] {line:?}");
    }

    let opening = "LINE-1 ";
    let carries = |lines: &[String]| lines.iter().any(|line| line.trim().starts_with("LINE-1"));
    let closing = format!("LINE-{REPORT_LINES}");
    assert!(
        addressed.iter().any(|line| line.contains(&closing)),
        "⚠⚠⚠ the agent's reply must have reached the pane at all before any reader can be judged \
         — {over:?}, {} addressed line(s): {addressed:?}",
        addressed.len(),
    );
    assert!(
        carries(&addressed),
        "⚠⚠⚠ THE ADDRESS LOST THE OPENING OF A REPLY IT WAS SUPPOSED TO SURVIVE ({lost} line(s) \
         reported evicted). If neither reader can carry a long report off a TUI pane, a captured \
         closing report is a LAST PAGE and must say so. Addressed: {addressed:?}",
    );
    assert!(
        !carries(&rendered),
        "⚠⚠⚠ THE RENDERING STILL HOLDS {opening:?} AFTER A {REPORT_LINES}-LINE REPLY ON A {} ROW \
         PANE, so this gate did not measure a scroll and its verdict about the two readers is \
         worthless. Either the agent clipped its own output or it wrote fewer lines than it was \
         asked for: {rendered:?}",
        PANE_SIZE.1,
    );
}

/// ⚠⚠⚠ **WHAT BECOMES OF A PROMPT WHOSE CONFIRMATION WAS ALREADY ON THE SCREEN** — register item
/// 222, asked of a live agent rather than reasoned about.
///
/// # ⚠⚠⚠ The evidence this was written from
///
/// Three live runs of the ceiling gates ended with the turn prompt sitting INSIDE `claude`'s
/// composer box, the agent idle underneath it, and the earlier prompts shown without that box —
/// i.e. submitted. `deliver` had returned success, so the text had been read back off the screen and
/// Enter had been injected behind it. **The turn never started**, so nothing could be asked for an
/// account, and the run spent its whole window on `Working --Null--> Working`.
///
/// # What this stages, and why it is the ordinary case
///
/// An outer loop's `turn_prompt` is a FIXED sentence — `'Continue toward: ' + milestone + …` — and
/// the confirmation needle is its leading run of columns. So from the second turn on, **the needle
/// is a string the agent's own transcript is still showing**, and *"is the needle on the screen?"*
/// is answered YES by the previous turn before a byte of this one has been read. The submit then
/// goes in on top of unread text, which the pty hands the program as one read of `…prompt…\r`
/// rather than as a prompt and then a keystroke.
///
/// Two turns, one session, and the two prompts DIFFER ONLY PAST COLUMN 40:
///
/// * they share a confirmation needle, so the second delivery meets the first one's echo;
/// * they have different ANSWERS, so what says the second turn ran is a word that cannot have come
///   from the first turn or from either prompt's own echo.
///
/// # ⚠⚠ The staging is asserted, not assumed
///
/// Between the turns this demands that the needle really is still on the screen. A run where the
/// agent's reply had scrolled the first prompt away would be measuring the ordinary case and
/// calling it the hazard — R388's rule, met here as *ask the pane before drawing the conclusion*.
///
/// ⚠ It asserts the CONSEQUENCE (the turn ran and the agent answered it) rather than a timing. How
/// long a delivery waits is a fact about how fast a model repaints, and this project has paid for
/// gates that asserted a model's speed; the durations are printed instead.
///
/// # ⚠⚠⚠ THE ANSWER, MEASURED — AND IT NEEDED BOTH SHAPES TO BE READABLE
///
/// The single-line shape was written first, on the reading that the stale confirmation was the
/// whole cause. **Alone it is not, and the first live run said so**: against `claude` 2.1.233 the
/// second single-line delivery confirmed off the previous turn's echo in **269.95 µs** — no program
/// had read a byte, the Enter went in behind it — **and the agent submitted it anyway**. A gate
/// that had stopped there would have cleared the delivery path and left item 222 open.
///
/// So the shape was made the variable, because what those three failing runs delivered and that
/// probe did not is a **multi-line prompt**: the outer loop's `start_prompt` is five clauses joined
/// with `\n`, and [`KeyStroke::text`] encodes each one as a bare 0x0A into a raw-mode TUI (register
/// item 8). The same 324-byte five-line prompt, in one session, measured against the OLD rule:
///
/// | turn | the delivery | the turn |
/// |---|---|---|
/// | 1, a needle nobody had seen | `Confirmed` after **10.6 ms** — a real repaint | `Over::Yes` |
/// | 2, the needle already on screen | `Confirmed` after **308 µs** — nothing had happened | **`Over::NotYet`** |
///
/// and turn 2's pane is register item 222's evidence exactly: the prompt inside the composer's box
/// rule, the agent idle under it, the run's whole window spent. ⚠ The submitted copy above it reads
/// `❯ North star:` and the unsent one reads `❯\u{a0}North star:` — **a no-break space where the
/// submitted one has a plain one**, which is the agent's own rendering saying it took the second
/// one as a block of pasted text rather than as something typed.
///
/// **So the cause is a CONJUNCTION, and only one half of it is ours.** A confirmation satisfied by
/// the previous turn's echo puts the submit into the same unread run of pty bytes as the text; a
/// program handed `…five lines…\r` in one read takes the whole thing as a paste and the trailing
/// carriage return with it. Neither half alone leaves a prompt unsent — turn 1 is multi-line and
/// runs, and the single-line turn 2 is stale-confirmed and runs. The half this workspace owns is
/// the first, and [`deliver`]'s baseline removes it: turn 2 now costs one poll interval, which is a
/// round trip through the program, and both shapes run.
///
/// ⚠ Each shape gets a session of its own, because a transcript is an input to what an agent's
/// composer does next — see the suggestion this gate met on the way.
#[test]
#[ignore = "drives a LIVE agent CLI: needs credentials, costs real turns, takes minutes"]
fn a_prompt_whose_confirmation_was_already_on_the_screen_still_starts_a_turn() {
    /// How long one of these one-word turns may take.
    const REPLY_WITHIN: Duration = Duration::from_secs(60);

    /// **THE SHAPES A LOOP ACTUALLY DELIVERS**: a label, the two prompts, and the word only the
    /// SECOND of them can put on the pane.
    ///
    /// ⚠ The two prompts of a pair are identical for their first 40 columns and differ after it,
    /// which is what makes the second delivery meet the first one's echo. That is not contrived:
    /// `turn_prompt` is `'Continue toward: ' + milestone`, fixed for the life of a run.
    ///
    /// ⚠⚠ The multi-line pair is the document's own `start_prompt` composition — `North star:` /
    /// `Milestone:` / `What to carry:` / a report clause / a marker clause, joined with `\n` and
    /// ending with one — with a milestone cheap enough to answer without a tool.
    const SHAPES: &[(&str, &str, &str, &str)] = &[
        (
            "one line",
            "Reply with one word and no tool: what is 1 plus 1 in English?",
            "Reply with one word and no tool: what is 2 plus 2 in English?",
            "four",
        ),
        (
            "many lines",
            "North star: answer from what you already know and use no tool\n\
             Milestone: say what 1 plus 1 is, in English, in one word\n\
             What to carry: nothing; the arithmetic is the whole task\n\
             Report what you did and what is left.\n\
             When the milestone is fully reached AND verified, make the last line of your reply \
             exactly: MILESTONE REACHED\n",
            "North star: answer from what you already know and use no tool\n\
             Milestone: say what 2 plus 2 is, in English, in one word\n\
             What to carry: nothing; the arithmetic is the whole task\n\
             Report what you did and what is left.\n\
             When the milestone is fully reached AND verified, make the last line of your reply \
             exactly: MILESTONE REACHED\n",
            "four",
        ),
    ];

    let mut refused: Vec<String> = Vec::new();
    for (shape, first, second, only_the_second_answers) in SHAPES {
        // ⚠ The needle the way the OUTER LOOP builds it — a leading run of 40 COLUMNS. These
        // prompts are ASCII, where a column is a character, so `chars().take(40)` is
        // `outer::confirmable`'s answer for them; a wide-glyph prompt would need that function,
        // which is private for the reason register item 27 records.
        let needle: String = first.chars().take(40).collect();
        assert_eq!(
            needle,
            second.chars().take(40).collect::<String>(),
            "⚠⚠⚠ {shape}: THE INSTRUMENT. The two prompts must share their confirmation needle, or \
             the second delivery never meets the first one's echo and this pair measures nothing",
        );
        assert!(
            !first.contains(only_the_second_answers) && !second.contains(only_the_second_answers),
            "⚠⚠ {shape}: and neither prompt may carry the second turn's answer, or the pane would \
             show it whether or not a turn ever ran",
        );

        // ⚠⚠⚠ A SESSION OF ITS OWN PER SHAPE, and the first run of this gate is why. After two
        // prompts differing only in a digit, `claude` left `what is 3 plus 3 in English?` sitting
        // in its composer — **a prompt nobody typed**, offered from the two before it. A second
        // shape driven onto that transcript would be delivered into a composer already holding
        // something, and a supervisor reading the screen cannot tell an agent's own suggestion from
        // text a run put there. Registered rather than handled here.
        let live = Live::start(&format!("already-showing-{}", shape.replace(' ', "-")));
        let began = Instant::now();
        let run = RunContext::uncancellable();

        let reached = Readiness::new(
            Some(ReadyWhen::Settles(live.agent.clone())),
            Some(STARTUP_BOUND),
            None,
            Attended::NoOne,
        )
        .reached(&live.access, live.pane, &run)
        .expect("the pane must stay readable");
        assert_eq!(
            reached,
            Reached::Yes,
            "{shape}: the agent must be up and at rest before it is spoken to: {}",
            live.tail(3),
        );
        step(began, &format!("{shape}: the agent is up"));

        // ⚠ ONE closure for both turns, so the second differs from the first ONLY in its text and
        // in what the screen was carrying when it began. Two hand-written turns would be two
        // instruments.
        let turn = |ask: &str, label: &str| {
            let mut done = Completion::new(DoneWhen::Settles);
            done.begin(&live.access, live.pane);
            let began_delivery = Instant::now();
            let delivered = deliver(
                &live.access,
                &run,
                live.pane,
                ask,
                &Delivery {
                    confirm: Some(needle.clone()),
                    then_press: vec![KeyStroke::named("Enter")],
                    ..Delivery::new()
                },
            )
            .expect("the pane must take the prompt");
            let delivery_took = began_delivery.elapsed();
            let over = done.wait(&live.access, live.pane, REPLY_WITHIN, &run);
            step(
                began,
                &format!(
                    "{shape} / {label}: {} byte(s), {} line(s) -> delivered {delivered:?} in \
                     {delivery_took:?}, turn ended {over:?}",
                    ask.len(),
                    ask.lines().count(),
                ),
            );
            (delivered, delivery_took, over)
        };

        let (first_delivered, first_took, first_over) = turn(first, "turn 1, a needle nobody saw");
        assert_eq!(
            first_over,
            Over::Yes,
            "⚠⚠⚠ {shape}: THE CONTROL. The FIRST prompt's needle is on nobody's screen, so every \
             rule agrees about it — if this turn does not run, the measurement below is compared \
             against nothing and the fault is not the one being staged. Delivered \
             {first_delivered:?} in {first_took:?}. Pane: {}",
            live.tail(8),
        );

        // ⚠⚠⚠ THE STAGING, MEASURED. Everything below is about a needle the screen is already
        // carrying; a screen that has scrolled it away is a different experiment.
        let between = live.screen();
        assert!(
            between.contains(&needle),
            "⚠⚠⚠ {shape}: THE HAZARD IS NOT STAGED — the first turn's prompt has left the screen, \
             so the second delivery would meet a pane that never showed the needle and this pair \
             would pass for the defect. Needle {needle:?} is not in: {between:?}",
        );
        step(
            began,
            &format!("{shape}: the needle is STILL on the screen — the hazard is staged"),
        );

        let (second_delivered, second_took, second_over) =
            turn(second, "turn 2, the needle is already there");
        let answered = live
            .access
            .pane_full_text(live.pane)
            .unwrap_or_default()
            .to_lowercase()
            .contains(*only_the_second_answers);

        println!(
            "\n== item 222 / {shape} ==\n  agent      : {}\n  \
             turn 1     : {first_delivered:?} in {first_took:?} -> {first_over:?}\n  \
             turn 2     : {second_delivered:?} in {second_took:?} -> {second_over:?}\n  \
             answered   : {answered} ({only_the_second_answers:?} on the pane)\n  \
             the pane   :\n{}\n",
            live.agent,
            live.tail(16),
        );

        // ⚠⚠⚠ THE MEASUREMENT, COLLECTED RATHER THAN ASSERTED PER SHAPE. Which shapes fail is the
        // finding; a gate that panicked on the first would never report the second, and the
        // difference BETWEEN them is the whole diagnosis.
        //
        // ⚠ THE `Unconfirmed` CASE IS COLLECTED HERE TOO rather than asserted on the spot, for the
        // same reason: it is the OPPOSITE fault — a rule that refused a repeat delivery would
        // refuse every turn an outer loop takes after its first — and a gate that panicked on it
        // would still have measured only one shape.
        let unconfirmed = matches!(second_delivered, Delivered::Unconfirmed { .. });
        if second_over != Over::Yes || !answered || unconfirmed {
            refused.push(format!(
                "{shape}: {} byte(s) over {} line(s) -> {second_over:?}, answered={answered}, \
                 delivered {second_delivered:?} in {second_took:?}{}; pane: {}",
                second.len(),
                second.lines().count(),
                if unconfirmed {
                    " — ⚠ UNCONFIRMED, the opposite fault: a repeat delivery was REFUSED"
                } else {
                    ""
                },
                live.tail(6),
            ));
        }
    }

    assert!(
        refused.is_empty(),
        "⚠⚠⚠ A PROMPT WAS DELIVERED, REPORTED AS SUCCESS, AND STARTED NO TURN — register item \
         222's live symptom, reproduced. The shape is the finding: {refused:#?}",
    );
}

/// ⚠⚠⚠ **WHAT A LIVE AGENT DOES WITH A SUBMIT, AND WHAT A DELIVERY CAN SEE OF IT** — register
/// item 225, asked of the peer it was written for.
///
/// # What is staged, and why it is not item 222's own failure
///
/// 222's symptom was a coalesced `…prompt…\r` that `claude` read as a PASTE: the text landed in the
/// composer, the carriage return with it, and no turn began. That coalescing is fixed — a delivery
/// is confirmed only against a screen it changed, so the submit is now its own pty read — which
/// means the failing shape cannot be re-created here on purpose any more.
///
/// So what is staged is the OBSERVABLE rather than the mechanism: a delivery that presses **a
/// printable key instead of Enter**. The composer takes it, repaints for it, and starts nothing —
/// which is what item 222's pane looked like for sixty seconds, and what a future agent release
/// could produce again with this workspace's half correct.
///
/// # The three readings
///
/// * **CONTROL** — Enter, [`SubmittedWhen::Stirs`]. The turn must run, and the delivery must be
///   `Confirmed`. Without it the two below are compared against nothing.
/// * **SUBJECT** — the stray key, `Stirs`. Must be [`Delivered::Unsubmitted`], and the agent must
///   still be at rest.
/// * **THE WEAKER CONTRACT** — the stray key, [`SubmittedWhen::Repaints`]. Expected to be
///   CONFIRMED, and that is the point: it is the residue that kind's own doc declares, measured on
///   the peer it matters for, and the reason the contract is the caller's to choose rather than
///   this module's to pick.
///
/// ⚠ Faults are COLLECTED, not asserted where they are found: which readings disagree is the
/// finding, and a panic on the first would hide the rest.
#[test]
#[ignore = "drives a LIVE agent CLI: needs credentials, costs real turns, takes minutes"]
fn what_a_live_agent_does_with_a_submit_it_was_never_given() {
    /// How long one of these one-word turns may take.
    const REPLY_WITHIN: Duration = Duration::from_secs(60);
    /// A key a composer takes and does nothing with — a submit that is not one.
    const NOT_A_SUBMIT: &str = "k";

    let live = Live::start("never-submitted");
    let began = Instant::now();
    let run = RunContext::uncancellable();
    let grace = sprag_plugin::DEFAULT_SUBMIT_GRACE;

    let reached = Readiness::new(
        Some(ReadyWhen::Settles(live.agent.clone())),
        Some(STARTUP_BOUND),
        None,
        Attended::NoOne,
    )
    .reached(&live.access, live.pane, &run)
    .expect("the pane must stay readable");
    assert_eq!(
        reached,
        Reached::Yes,
        "the agent must be up and at rest before it is spoken to: {}",
        live.tail(3),
    );
    step(began, "the agent is up");

    // ⚠ ONE closure for all three readings, so they differ ONLY in what is pressed and what is
    // watched for. Three hand-written deliveries would be three instruments.
    let delivery = |ask: &str, press: &str, submitted_when: SubmittedWhen, label: &str| {
        let seq_before = live.seq();
        let screen_before = live.screen();
        let began_delivery = Instant::now();
        let delivered = deliver(
            &live.access,
            &run,
            live.pane,
            ask,
            &Delivery {
                confirm: Some(ask.chars().take(40).collect()),
                // ⚠⚠⚠ `named`, NOT `text`. The first run of this gate pressed
                // `KeyStroke::text("Enter")` — the five CHARACTERS — and the control's own pane
                // shows the word `Enter` sitting in the composer behind the prompt. It failed for
                // the reason it was written to detect, which is what a fixture that types a key
                // name instead of pressing a key looks like from the inside.
                then_press: vec![KeyStroke::named(press)],
                submitted_when,
                ..Delivery::new()
            },
        )
        .expect("the pane must take the prompt");
        let took = began_delivery.elapsed();
        let seq_after = live.seq();
        step(
            began,
            &format!(
                "{label}: pressed {press:?} watching {submitted_when:?} -> {delivered:?} in \
                 {took:?}; seq {seq_before:?} -> {seq_after:?}; the screen {} while it waited",
                if live.screen() == screen_before {
                    "never moved"
                } else {
                    "moved"
                },
            ),
        );
        (delivered, took)
    };

    let mut faults: Vec<String> = Vec::new();

    // ── THE CONTROL ────────────────────────────────────────────────────────────────────────────
    let mut done = Completion::new(DoneWhen::Settles);
    done.begin(&live.access, live.pane);
    let (control, control_took) = delivery(
        "Reply with one word and no tool: what is 1 plus 1 in English?",
        "Enter",
        SubmittedWhen::Stirs { within: grace },
        "control",
    );
    let over = done.wait(&live.access, live.pane, REPLY_WITHIN, &run);
    let answered = live
        .access
        .pane_full_text(live.pane)
        .unwrap_or_default()
        .to_lowercase()
        .contains("two");
    step(
        began,
        &format!("control: turn ended {over:?}, answered={answered}"),
    );
    if !control.is_confirmed() || over != Over::Yes || !answered {
        faults.push(format!(
            "⚠⚠⚠ THE CONTROL FAILED, so nothing below is measured against a working delivery: \
             {control:?} in {control_took:?}, turn {over:?}, answered={answered}. Pane: {}",
            live.tail(8),
        ));
    }

    // ── THE SUBJECT: a submit that is not one, watched by the supervisor ───────────────────────
    let (subject, subject_took) = delivery(
        "Reply with one word and no tool: what is 2 plus 2 in English?",
        NOT_A_SUBMIT,
        SubmittedWhen::Stirs { within: grace },
        "subject",
    );
    let at_rest = live.seen();
    if !matches!(subject, Delivered::Unsubmitted { .. }) {
        faults.push(format!(
            "⚠⚠⚠ A PROMPT NOBODY SUBMITTED CAME BACK AS {subject:?} in {subject_took:?} — this is \
             register item 225's whole question, and the answer here is the one it had before the \
             contract existed. The detector says {at_rest}. Pane: {}",
            live.tail(8),
        ));
    }

    // ── THE WEAKER CONTRACT, over the same peer and the same non-submit ────────────────────────
    let (weaker, weaker_took) = delivery(
        "Reply with one word and no tool: what is 3 plus 3 in English?",
        NOT_A_SUBMIT,
        SubmittedWhen::Repaints { within: grace },
        "weaker",
    );
    if !weaker.is_confirmed() {
        faults.push(format!(
            "⚠⚠ `Repaints` DID NOT accept a keystroke the composer merely absorbed: {weaker:?} in \
             {weaker_took:?}. That is not a failure of the product — it is this measurement's \
             premise being wrong about the peer, and the doc that calls it a residue would need \
             re-writing. Pane: {}",
            live.tail(8),
        ));
    }

    println!(
        "\n== item 225 / a live {} ==\n  \
         control (Enter, stirs)     : {control:?} in {control_took:?} -> {over:?}, answered \
         {answered}\n  \
         subject ({NOT_A_SUBMIT:?}, stirs)       : {subject:?} in {subject_took:?}\n  \
         weaker  ({NOT_A_SUBMIT:?}, repaints)    : {weaker:?} in {weaker_took:?}\n  \
         detector after the subject : {at_rest}\n  the pane:\n{}\n",
        live.agent,
        live.tail(16),
    );

    assert!(
        faults.is_empty(),
        "⚠⚠⚠ WHAT A DELIVERY CAN SEE OF A LIVE AGENT'S SUBMIT — the readings that disagreed: \
         {faults:#?}",
    );
}

/// ⚠⚠⚠⚠⚠ **WHAT MAKES A LIVE AGENT'S COMPOSER FOLD THE PROMPT AWAY** — register item 433's
/// blocker, asked of the peer instead of guessed at.
///
/// # ⚠⚠⚠⚠ Why this had to be measured before another run was budgeted for it
///
/// Item 421's fix says a prompt a composer FOLDED away is delivered on the agent's own account, and
/// item 433 says only a live run retires that claim. The obvious plan — run the loop until a
/// reflection folds — rests on the fold being a property of the TEXT, and **the register's own
/// numbers say it is not**:
///
/// | run | the reflection prompt | what the screen showed |
/// |---|---|---|
/// | 10 | 3 × **1,334** bytes, 6 lines | `[Pasted text #2 +5 lines]` — folded, run died |
/// | 13 | 2 × **1,314** bytes, 6 lines | the prompt, painted; delivery confirmed |
///
/// **The same prompt shape, twenty bytes apart, opposite outcomes.** So *"measure the limit and
/// refuse above it"* — which is what item 421's own entry first asked for — is answering a question
/// about length that the evidence has already ruled out. What varies has to be something about the
/// WRITE or about the peer's state when it arrives, and this asks which.
///
/// # ⚠⚠ One live agent per reading, deliberately
///
/// A composer that has taken a delivery cannot be emptied — `C-u` does not clear it (items
/// 223/224), so a second delivery into the same pane CONCATENATES and the reading after the first
/// is about a composer holding two prompts. Three agents cost three cold starts and buy three
/// independent readings.
///
/// ⚠ **NOTHING IS SUBMITTED.** The question is what the composer SHOWS, so no `Enter` is pressed
/// and no turn is spent; each pane is dropped holding its prompt.
#[test]
#[ignore = "drives a LIVE agent CLI: needs credentials, spawns several, takes minutes"]
fn what_makes_a_live_agents_composer_fold_the_prompt_away() {
    /// The placeholder a folded paste paints — matched on its stable head, since the `#N` and the
    /// line count are the peer's to choose.
    const FOLD: &str = "[Pasted text";
    /// How long to let the composer paint before deciding what it shows.
    const PAINT_WITHIN: Duration = Duration::from_secs(3);

    // The loop's own reflection prompt, in shape and in size: six lines, ~1,320 bytes — the
    // population both readings in the table above were taken from.
    let prompt = format!(
        "North star: {}\nYou have been working toward: {}\nWhat this session has cost: {}\nStop \
         and decide what comes next, from what you have just done.\nReply with exactly two lines \
         and nothing else, the first opening NEXT MILESTONE: and the second NEXT REFERENCE:.\nIf \
         the north star itself is fully reached, make the last line exactly: NORTH STAR REACHED",
        "x".repeat(300),
        "y".repeat(300),
        "z".repeat(300),
    );

    let began = Instant::now();
    let run = RunContext::uncancellable();

    // ⚠ ONE closure for every reading, so they differ ONLY in HOW the bytes are written. Two
    // hand-written arms would be two instruments, which is this module's own rule.
    let reading = |label: &str, chunk: Option<usize>| -> (String, String) {
        let live = Live::start(label);
        let reached = Readiness::new(
            Some(ReadyWhen::Settles(live.agent.clone())),
            Some(STARTUP_BOUND),
            None,
            Attended::NoOne,
        )
        .reached(&live.access, live.pane, &run)
        .expect("the pane must stay readable");
        assert_eq!(
            reached,
            Reached::Yes,
            "⚠ THE PREMISE OF EVERY READING: the composer must be up and at rest before anything is \
             typed into it, or this measures a swallowed write rather than a fold. {}",
            live.tail(3),
        );

        let wrote = match chunk {
            // One write, which is what `PaneAccess::inject` does for a whole prompt today.
            None => live
                .access
                .inject(live.pane, &KeyStroke::text(&prompt))
                .expect("the pane takes the bytes")
                .bytes(),
            // ⚠ The same bytes, handed over the way a person's keyboard hands them over: small
            // pieces with time between them. If the fold is a burst heuristic this is what does
            // not trigger it, and the difference IS the answer.
            Some(size) => {
                let mut wrote = 0;
                let chars: Vec<char> = prompt.chars().collect();
                for piece in chars.chunks(size) {
                    let text: String = piece.iter().collect();
                    wrote += live
                        .access
                        .inject(live.pane, &KeyStroke::text(&text))
                        .expect("the pane takes the bytes")
                        .bytes();
                    std::thread::sleep(Duration::from_millis(20));
                }
                wrote
            }
        };

        // Let it paint. ⚠ Bounded by a clock rather than by a predicate: BOTH outcomes are things
        // this is trying to tell apart, so waiting for either would decide the answer.
        std::thread::sleep(PAINT_WITHIN);
        let screen = live.screen();
        let saw = if screen.contains(FOLD) {
            "FOLDED"
        } else if screen.contains(&prompt.chars().take(40).collect::<String>()) {
            "painted (its head is readable back)"
        } else {
            "neither — the prompt's head is not on that screen and no placeholder is either"
        };
        step(
            began,
            &format!("{label}: wrote {wrote} bytes in {chunk:?}-sized pieces -> {saw}"),
        );
        (saw.to_owned(), live.tail(6))
    };

    let (bulk, bulk_pane) = reading("fold-bulk", None);
    let (typed, typed_pane) = reading("fold-typed", Some(16));

    println!(
        "\n== what makes a live composer fold ==\n  \
         one write of {} bytes : {bulk}\n{bulk_pane}\n  \
         the same in 16-char pieces : {typed}\n{typed_pane}\n",
        prompt.len(),
    );

    // ⚠⚠⚠⚠⚠ THE CLAIM: the fold is something the WRITE does, not something the text is. If both
    // readings agree, this measurement has ruled that out too — and the register's next step is a
    // different question, which is why the message says what was actually seen rather than only
    // that a comparison failed.
    assert_ne!(
        bulk,
        typed,
        "⚠⚠⚠⚠⚠ ITEM 433: the same {} bytes reached the same composer two ways and it showed the \
         SAME thing for both. So the fold is not a property of how the bytes are written either, \
         and neither is it the text (run 10's 1,334 bytes folded where run 13's 1,314 painted) — \
         which leaves the peer's own state, and no run of this loop can be budgeted to produce a \
         fold until that is known.",
        prompt.len(),
    );

    // ── ⚠⚠⚠⚠⚠ AND WHAT A DELIVERY MAKES OF IT — item 421's live death, reproduced on HEAD ──
    //
    // Everything above is about what the composer SHOWS. This is what `deliver` does when it is
    // shown that: a peer whose supervisor cannot publish an account keeps the screen predicate, so
    // the read-back can never match, the submit is withheld and the run gets a REFUSAL. That is the
    // control half of item 421's claim — the half that does NOT need a hook-instrumented peer — and
    // until now it was held only by a `/bin/sh` double printing a placeholder.
    //
    // ⚠⚠⚠ **THE PREMISE IS ASSERTED, NOT ASSUMED**: this harness's own detector reports
    // `Authority::Scraped` (measured, `rule: Some("idle-glyph")`), which is exactly the peer
    // `submit_lands_when` refuses to escalate for. A run of this against a HOOKED peer would take
    // the other road and prove the opposite thing, so the reading is recorded beside the answer.
    let live = Live::start("fold-refused");
    assert_eq!(
        Readiness::new(
            Some(ReadyWhen::Settles(live.agent.clone())),
            Some(STARTUP_BOUND),
            None,
            Attended::NoOne,
        )
        .reached(&live.access, live.pane, &run)
        .expect("the pane must stay readable"),
        Reached::Yes,
        "the composer must be up before it is delivered into: {}",
        live.tail(3),
    );
    let authority = live
        .access
        .supervision()
        .and_then(|supervisor| supervisor.pane_agent_state(live.pane))
        .map(|seen| format!("{:?}", seen.authority));
    let delivered = deliver(
        &live.access,
        &run,
        live.pane,
        &prompt,
        &Delivery {
            confirm: Some(prompt.chars().take(40).collect()),
            then_press: vec![KeyStroke::named("Enter")],
            // What a peer whose verdict is SCRAPED gets — see `OuterLoop::submit_lands_when`.
            submitted_when: SubmittedWhen::Stirs {
                within: sprag_plugin::DEFAULT_SUBMIT_GRACE,
            },
            // ⚠⚠⚠⚠⚠ **ONE, AND THE FIRST RUN OF THIS ARM IS WHY** — measured 2026-08-18. With the
            // default three it came back `Confirmed { attempts: 2, written: 2477 }` = 2 × 1,238 + 1,
            // and the pane's final screen carried the WHOLE prompt with the agent already working.
            // **The peer expands its own fold when the identical paste arrives again**, and it says
            // so on the screen: the placeholder is followed by *"paste again to expand"*. So
            // `deliver`'s retry does not merely re-send — on this peer it UN-FOLDS, which is the
            // complete explanation of run 13's `2 × 1,314 + 1` and of why that run survived on a
            // daemon without item 421's fix at all.
            //
            // ⚠⚠⚠ This arm is about what a delivery sees of a FOLDED screen, so it must not be
            // allowed to heal it first. One attempt is also exactly the shape the register quotes
            // from the live deaths — *"the live `Unconfirmed { attempts: 1 }` verbatim"*.
            attempts: 1,
            ..Delivery::new()
        },
    )
    .expect("the pane takes the bytes");
    let refused_screen = live.screen();
    step(
        began,
        &format!("fold-refused: authority {authority:?} -> {delivered:?}"),
    );
    println!(
        "\n== what a delivery makes of a folded paste, live ==\n  authority: {authority:?}\n  \
         answer   : {delivered:?}\n{}\n",
        live.tail(6),
    );
    assert!(
        refused_screen.contains(FOLD),
        "⚠ THE PREMISE AGAIN: this arm is only about a folded paste if the composer folded it. \
         Screen: {}",
        live.tail(6),
    );
    assert!(
        matches!(delivered, Delivered::Unconfirmed { attempts: 1, .. })
            && !delivered.is_confirmed(),
        "⚠⚠⚠⚠⚠ ITEM 421, LIVE ON HEAD: a real composer folded the prompt away and this peer's \
         supervisor publishes no account, so the screen is all there is and it cannot carry the \
         text. The delivery MUST refuse rather than press over it — that refusal is what ended \
         three live runs at 0 iterations, and it is correct here. Got {delivered:?}",
    );
}

/// [`one_turn`] against a pane that is not [`Live::pane`] — what a replacement needs.
fn one_turn_on(live: &Live, pane: PaneId, run: &RunContext, index: usize, began: Instant) -> Turn {
    let token = format!("ORTHOGONAL-{index}7");
    let ask = format!("Reply with exactly the word {token} and nothing else.");
    let mut done = Completion::new(DoneWhen::Settles);
    done.begin(&live.access, pane);
    let delivered = deliver(
        &live.access,
        run,
        pane,
        &ask,
        &Delivery {
            confirm: Some(ask.chars().take(40).collect()),
            then_press: vec![KeyStroke::named("Enter")],
            ..Delivery::new()
        },
    )
    .expect("the pane must take the prompt");
    step(began, &format!("turn {index}: delivered {delivered:?}"));
    let asked_at = Instant::now();
    let over = done.wait(&live.access, pane, TURN_BOUND, run);
    let elapsed = asked_at.elapsed();
    let answered = live
        .access
        .pane_full_text(pane)
        .unwrap_or_default()
        .contains(&token);
    step(began, &format!("turn {index}: {over:?} after {elapsed:?}"));
    Turn {
        index,
        sampled: false,
        over,
        elapsed,
        seq_before: None,
        seq_after: None,
        answered,
    }
}
