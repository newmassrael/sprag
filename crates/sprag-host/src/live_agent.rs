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
    /// ⚠ Named rather than `_scratch` since R382: the gate that drives a milestone which WRITES A
    /// FILE has to look in it. That look is also the only thing in this module that has ever
    /// checked the premise its whole safety argument rests on — *the agent runs in here* — which
    /// was registered as owed and asserted by nothing.
    scratch: Scratch,
}

impl Live {
    /// Spawn the agent named by [`AGENT_PROGRAM`] in a scratch directory of its own.
    fn start(tag: &str) -> Self {
        let agent = std::env::var(AGENT_PROGRAM).unwrap_or_else(|_| DEFAULT_AGENT.to_owned());
        let scratch = Scratch::new(tag);
        let workspace = Arc::new(Mutex::new(Workspace::new(PANE_SIZE)));

        let mut command = CommandBuilder::new(&agent);
        command.cwd(scratch.path());
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
        // ⚠ The document's own placeholder, which claims nothing: this gate is about arithmetic
        // that raises no dialog, so screening must not be armed or it would be a second variable.
        screen_rules: None,
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
    use sprag_plugin::outer::INNER_SESSION_ENDS;
    use sprag_plugin::{AiLoopState, Brief, Consent, Consents, Turn as TurnContract};

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
        max_turns: LIVE_MAX_TURNS,
        reflect_every: LIVE_MAX_TURNS,
        // ⚠ Unarmed: this gate's claim is about the CONSENT carrying the loop through the dialog,
        // so a standing rule that could also get past it would make the finding ambiguous.
        screen_rules: None,
    };
    let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
    let mut loops = sprag_plugin::AiLoop::new(
        lua,
        live.pane,
        &brief,
        &sprag_plugin::AiLoopSpec {
            turn: TurnContract::lasting(INNER_SESSION_ENDS, Some(TURN_BOUND))
                .expect("a non-zero bound"),
            // ⚠⚠⚠ THE WHOLE POINT. Without this the run stops at the agent's first permission
            // dialog with nothing judged — measured against a stand-in, and the reason every live
            // milestone before this one was arithmetic.
            may_answer: Consents::of(vec![
                Consent::parse("Do you want to".to_string(), "Yes".to_string())
                    .expect("both needles are non-empty"),
            ]),
            ..sprag_plugin::AiLoopSpec::driving(&live.agent)
        },
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
    use sprag_plugin::outer::INNER_SESSION_ENDS;
    use sprag_plugin::{AiLoopState, Brief, ScreenRule, ScreenRules, Turn as TurnContract};

    /// Room for the tool turn, the screening, the redirected turn and the closing report.
    const LIVE_MAX_TURNS: i64 = 4;
    /// A name nothing else would produce — so if it appears, this run allowed it.
    const REFUSED: &str = "SPRAG-LOOP-MUST-NOT-MAKE-THIS.txt";
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
        max_turns: LIVE_MAX_TURNS,
        reflect_every: LIVE_MAX_TURNS,
        // ⚠⚠⚠ THE WHOLE POINT: the caller supplies the AUTHOR's half of the contract, quoting the
        // agent's own words exactly as a consent's `asked` does — the needle R383 measured covering
        // every dialog three tool families raise.
        screen_rules: ScreenRules::of(vec![
            ScreenRule::parse("Do you want to".to_string(), INSTEAD.to_string())
                .expect("both halves are non-empty"),
        ]),
    };
    let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
    let mut loops = sprag_plugin::AiLoop::new(
        lua,
        live.pane,
        &brief,
        &sprag_plugin::AiLoopSpec {
            turn: TurnContract::lasting(INNER_SESSION_ENDS, Some(TURN_BOUND))
                .expect("a non-zero bound"),
            // ⚠⚠⚠ NO CONSENT, and that is the control for the whole gate. If a clause were armed
            // it could take the dialog's own `Yes`, the file would be written, and nothing below
            // would be about `screening` at all.
            may_answer: None,
            ..sprag_plugin::AiLoopSpec::driving(&live.agent)
        },
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
