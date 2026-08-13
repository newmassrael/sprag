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
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sprag_plugin::access::WorkspacePaneAccess;
use sprag_plugin::{
    Attended, Completion, DoneWhen, Delivered, Delivery, KeyStroke, Over, PaneAccess, Reached,
    ReadyWhen, Readiness, RunContext, deliver,
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
/// expectation yet. `Turn`'s doc says to size it to the peer; this is a generous reading of *"an
/// agent asked to read a repository is minutes"*.
const TURN_BOUND: Duration = Duration::from_secs(180);

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
        let path = std::env::temp_dir().join(format!(
            "sprag-live-{tag}-{}-{nanos}",
            std::process::id(),
        ));
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
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace))
            .with_agent_state(Some(agent_state_source(
                Arc::clone(&workspace),
                agents,
                shipped_settle,
            )));

        Self {
            workspace,
            access,
            pane,
            agent,
            _scratch: scratch,
        }
    }

    /// What the detector says about the pane right now, as one line.
    fn seen(&self) -> String {
        match self
            .access
            .supervision()
            .and_then(|supervisor| supervisor.pane_agent_state(self.pane))
        {
            Some(seen) => format!(
                "state={:?} agent={:?} seq={} authority={:?} asking={}",
                seen.state,
                seen.agent,
                seen.seq,
                seen.authority,
                seen.asking.map_or("-".to_owned(), |question| question
                    .asked
                    .join(" / ")),
            ),
            None => "no observation (this pane is not an agent's, or nothing is detected)".to_owned(),
        }
    }

    /// The pane's screen, collapsed — what the agent has actually painted.
    fn screen(&self) -> String {
        self.access.pane_collapsed(self.pane).unwrap_or_default()
    }

    /// The last `rows` non-empty rows of the pane, for a walk line that has to stay readable.
    fn tail(&self, rows: usize) -> String {
        let screen = self.screen();
        let lines: Vec<&str> = screen
            .lines()
            .map(str::trim_end)
            .filter(|line| !line.is_empty())
            .collect();
        lines[lines.len().saturating_sub(rows)..].join(" ⏎ ")
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        // The agent is a long-lived process; nothing else in this test reaps it.
        self.workspace
            .lock()
            .expect("the workspace mutex")
            .close(self.pane);
    }
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

    // ── ONE TURN ──
    //
    // A prompt with an answer nobody could produce by accident, so the screen check below cannot
    // be satisfied by the agent's own banner.
    const ASK: &str = "Reply with exactly the word ORTHOGONAL-7 and nothing else.";
    let mut done = Completion::new(DoneWhen::Settles);
    // ⚠ ARMED BEFORE A BYTE GOES IN — `Completion::begin`'s whole guarantee, and the thing this
    // measurement exists to put under a peer that is genuinely at rest beforehand.
    done.begin(&live.access, live.pane);
    let delivered = deliver(
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
    step(began, &format!("delivered: {delivered:?}"));
    assert!(
        !matches!(delivered, Delivered::Unconfirmed { .. }),
        "⚠⚠ a live agent PAINTS what is typed into its composer, which is the premise `deliver` \
         reads the screen back on. An unconfirmed delivery here is a finding about that premise, \
         not about the peer's answer. Screen: {}",
        live.tail(6),
    );

    let asked_at = Instant::now();
    let over = done.wait(&live.access, live.pane, TURN_BOUND, &run);
    let turn = asked_at.elapsed();
    step(began, &format!("turn ended: {over:?} after {turn:?}"));
    step(began, &format!("detector: {}", live.seen()));
    step(began, &format!("screen: {}", live.tail(6)));

    assert_eq!(
        over,
        Over::Yes,
        "⚠⚠⚠ a live turn must end on the contract's own evidence. `NotYet` means the bound \
         ({TURN_BOUND:?}) ran out — the peer never came back to rest, or the detector never said \
         so; `Asking` means it stopped on a dialog, which is a real ending and a different \
         measurement. Screen: {}",
        live.tail(8),
    );
    assert!(
        turn > Duration::from_secs(1),
        "⚠⚠⚠ THE NUMBER THIS WHOLE GATE EXISTS FOR. A live agent's turn cannot be over in \
         {turn:?}: the only thing that is true that fast is the peer's rest from BEFORE the turn, \
         which is what `Completion::begin`'s arming exists to refuse. Every stand-in in this \
         workspace answers inside this window, so this is the first run in which `too fast` and \
         `correct` are distinguishable at all.",
    );
    assert!(
        turn < TURN_BOUND,
        "and it must be the CONTRACT that ended the turn rather than the clock: {turn:?}",
    );
    assert!(
        live.screen().contains("ORTHOGONAL-7"),
        "⚠⚠ and the turn the contract called over is one the agent actually answered — without \
         this the two numbers above are about a peer that went quiet for some other reason. \
         Screen: {}",
        live.tail(8),
    );

    println!(
        "\n== 64c, first half ==\n  agent: {}\n  startup to barrier: {startup:?}\n  one turn: \
         {turn:?}\n",
        live.agent,
    );
}
