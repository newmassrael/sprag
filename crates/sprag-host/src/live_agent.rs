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
    Readiness, ReadyWhen, RunContext, deliver,
};
use sprag_terminal::{CommandBuilder, PaneId, Workspace};

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
    _scratch: Scratch,
}

impl Live {
    /// Spawn the agent named by [`AGENT_PROGRAM`] in a scratch directory of its own.
    fn start(tag: &str) -> Self {
        let agent = std::env::var(AGENT_PROGRAM).unwrap_or_else(|_| DEFAULT_AGENT.to_owned());
        let scratch = Scratch::new(tag);
        let workspace = Arc::new(Mutex::new(Workspace::new(PANE_SIZE)));

        let mut command = CommandBuilder::new(&agent);
        command.cwd(scratch.path());
        // A real terminal, because the agent renders a TUI and the detector reads what it renders.
        command.env("TERM", "xterm-256color");
        // ⚠ THE CHILD MUST BE THE THING A PERSON GETS FROM A TERMINAL. This process is itself an
        // agent session, and it exports variables that tell a child it is nested. Blanked rather
        // than inherited, so the measurement is of the program under test rather than of a mode
        // only this harness could produce. (`CommandBuilder` adds to the inherited environment; it
        // has no unset, and an empty value is falsy to everything that reads these.)
        for nested in [
            "CLAUDECODE",
            "CLAUDE_CODE_ENTRYPOINT",
            "CLAUDE_CODE_SESSION_ID",
            "CLAUDE_CODE_CHILD_SESSION",
            "CLAUDE_CODE_MESSAGING_SOCKET",
            "CLAUDE_CODE_MESSAGING_TOKEN",
            "CLAUDE_CODE_EXECPATH",
            "CLAUDE_PID",
            "AI_AGENT",
        ] {
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
            _scratch: scratch,
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
        let _closed = self
            .workspace
            .lock()
            .expect("the workspace mutex")
            .close(self.pane);
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
    use sprag_plugin::outer::INNER_SESSION_ENDS;
    use sprag_plugin::{AiLoopState, OuterLoop, Pumped, Turn as TurnContract};

    let live = Live::start("loop");
    let run = RunContext::uncancellable();
    let began = Instant::now();

    let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
    let mut loops = OuterLoop::new(
        lua,
        live.pane,
        // ⚠ `driving` fixes the two knobs that are true of every agent CLI — the barrier is
        // `settles` on its own name, and it paints the prompt box it is typed into, which is the
        // premise `deliver`'s read-back rests on. Only the per-turn bound is this gate's own.
        &sprag_plugin::AiLoopSpec {
            turn: TurnContract::lasting(INNER_SESSION_ENDS, Some(TURN_BOUND))
                .expect("a non-zero bound"),
            ..sprag_plugin::AiLoopSpec::driving(&live.agent)
        },
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
                from, raised, to, ..
            } => {
                step(began, &format!("{from:?} --{raised:?}--> {to:?}"));
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
                        !loops.said_done(&live.access),
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
#[test]
#[ignore = "drives a LIVE agent CLI: needs credentials, costs real turns, takes minutes"]
fn a_briefed_loop_converges_against_a_live_agent() {
    use sce_rust_runtime::IScriptEngine;
    use sprag_plugin::outer::INNER_SESSION_ENDS;
    use sprag_plugin::{AiLoopState, Brief, Turn as TurnContract};

    /// Small enough that a run which merely ran out of budget is cheap, and large enough that one
    /// answered turn plus the closing report fits with room to spare.
    const LIVE_MAX_TURNS: i64 = 3;

    let live = Live::start("converge");
    let began = Instant::now();

    let brief = Brief {
        north_star: "prove an outer loop can be driven to convergence by a real agent".to_string(),
        milestone: "state the product of 17 and 23 as a single number".to_string(),
        reference: "no tools and no files are needed; answer from arithmetic alone".to_string(),
        max_turns: LIVE_MAX_TURNS,
        // ⚠ EQUAL to the budget, which is what keeps `reflecting` — an unbuilt state — off the
        // path. `AiLoop::new` refuses anything smaller, so this is the door's own rule rather than
        // a number chosen here.
        reflect_every: LIVE_MAX_TURNS,
    };
    let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
    let mut loops = sprag_plugin::AiLoop::new(
        lua,
        live.pane,
        &brief,
        &sprag_plugin::AiLoopSpec {
            turn: TurnContract::lasting(INNER_SESSION_ENDS, Some(TURN_BOUND))
                .expect("a non-zero bound"),
            ..sprag_plugin::AiLoopSpec::driving(&live.agent)
        },
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
