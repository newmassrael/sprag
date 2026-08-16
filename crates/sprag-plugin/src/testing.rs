//! Fixture vocabulary shared by the plugin gates — the shell fragments whose CORRECTNESS is not
//! visible in reading them.
//!
//! Test-only ([`cfg(test)`]), and it exists because one of these cost two rebuilds of the same
//! gate before it measured anything. A plugin gate about *readiness* has to build a pane that is
//! genuinely NOT ready, and "not ready" is a stronger condition than it looks: see
//! [`STANDIN_READS_TTY`].

/// Redirection that gives a BACKGROUNDED reader the pane's own input.
///
/// # ⚠⚠ Why a stand-in without this measures nothing
///
/// A gate for a readiness barrier needs a pane that is not ready yet, and the only stand-in that
/// discriminates is one that EATS what it is given — because an un-eaten injection does not
/// vanish. It sits in the pseudoterminal's buffer, and the program that starts next reads it. So a
/// run that injected far too early still converges, off bytes it sent to nobody, and the gate
/// reports that the barrier worked.
///
/// The obvious spelling of a stand-in that steps aside on a timer is a background reader:
///
/// ```sh
/// while read early; do echo "ATE $early"; done &   # ⚠ reads /dev/null, eats NOTHING
/// sleep 2; kill $!; exec the-real-peer
/// ```
///
/// **A background job of a NON-INTERACTIVE shell gets its stdin from `/dev/null`** (POSIX: job
/// control is off, so the shell redirects it). The stand-in therefore reads end-of-file
/// immediately, consumes nothing, and the fixture is back to the un-eaten-bytes case with nothing
/// in its text to show it. Appending this reopens the controlling terminal — the pane's own input —
/// for that job, which is what makes the stand-in a stand-in.
///
/// ⚠ Belongs on the READER, before the `&`: `while read x; do …; done </dev/tty &`.
pub(crate) const STANDIN_READS_TTY: &str = "</dev/tty";

/// Signal the backgrounded stand-in AND REAP IT, so the next thing the fixture does happens with
/// the reader provably gone.
///
/// # ⚠⚠⚠ Why signalling it is not enough
///
/// `kill` DELIVERS a signal and returns; the shell then runs on while the target is still being
/// torn down. A stand-in reading with [`STANDIN_READS_TTY`] is parked in a one-byte `read` on the
/// pane's own terminal, and a shell's `read` builtin reads a byte at a time from a tty precisely so
/// it cannot over-consume — so between the `kill` and the death there is a window in which the
/// stand-in will take **exactly one byte** of whatever arrives.
///
/// What arrives is the next thing the fixture invites: a readiness marker, then a run's prompt. The
/// cost was measured, and it is a CORRECTNESS failure rather than a timing one — a run that asked
/// `"summarise the repo"` was answered `REPLY[ummarise the repo]`, converged, and published it.
/// It surfaced only under whole-suite load, which is what a window looks like from outside.
///
/// Reaping closes it at the cause: the fixture waits for the observable fact (this process is gone)
/// instead of for the act that should cause it. ⚠ Errors are discarded because a stand-in that has
/// ALREADY exited is a perfectly good outcome — the fixture wants it dead, not killed by this line.
pub(crate) const REAP_THE_STANDIN: &str = "kill $! 2>/dev/null; wait $! 2>/dev/null;";

/// **A PANE THAT ENDS THE RUN AT THE KEYSTROKE IT IS ASKED TO WRITE** — the double for a run
/// cancelled with a key already on the pseudoterminal.
///
/// # ⚠⚠⚠ Why the cancel is tied to the KEYSTROKE and not to a clock
///
/// What every act that types is claiming afterwards is *the key went in and then the peer did
/// something*. The dangerous case is the one where the second half never happened, and a fixture
/// that cancels on a thread after a sleep stages it only by luck: on a loaded box the flag can land
/// before the injection, and then the gate is about a run that typed nothing — a different claim,
/// passing under the same name, and one every act here already reports correctly.
///
/// Hanging the flag on the write makes the order a fact of the double rather than of the scheduler.
/// It is [`crate::deliver`]'s `cancel_on_submit` (R393) said about a pane a real pty is behind.
///
/// ⚠ WHICH keystroke is [`StopsWhen`], because the two gates that need one cannot name it the same
/// way: an act called directly counts injections, and a RUN driven through the loop cannot — the
/// prompts its document composes are delivered by the same `inject`, so a number here would be a
/// count of `deliver`'s internals kept in step by nobody.
///
/// ⚠⚠⚠ **AND IT KEEPS THE LEDGER OF WHAT WAS TYPED AFTER THE STOP** — see
/// [`typed_after_the_stop`](StopsAtTheKey::typed_after_the_stop). Every act in this crate claims to
/// type nothing once its run is over, and a claim nobody records is a claim nobody holds.
pub(crate) struct StopsAtTheKey {
    /// The real pane. Everything but [`PaneAccess::inject`] is this, untouched.
    pub(crate) pane: WorkspacePaneAccess,
    /// The run's own cancel flag — hand [`crate::run::RunContext::new`] a clone of it.
    pub(crate) cancel: Arc<std::sync::atomic::AtomicBool>,
    /// WHICH keystroke ends the run.
    when: StopsWhen,
    seen: std::sync::atomic::AtomicUsize,
    after: Mutex<Vec<String>>,
}

/// **WHICH KEYSTROKE ENDS THE RUN** — [`StopsAtTheKey`]'s trigger.
#[derive(Clone, Copy, Debug)]
pub(crate) enum StopsWhen {
    /// The `n`th injection into the pane, counting from one.
    ///
    /// ⚠ A gate can stop a run inside the FIRST wait (the evidence for the key just sent) or inside
    /// a LATER one (the wait after an escalation) — the two places an answering act has to give up,
    /// and they report different things about different keys.
    TheNthKey(usize),
    /// **THE FIRST KEY THIS RUN PRESSES WHILE THE PEER IS SHOWING A DIALOG.**
    ///
    /// ⚠⚠⚠ Asked of the SUPERVISION, at the instant of the press, which is what makes it usable
    /// from outside one act: a whole run reaches its peer's menu through a prompt delivery whose
    /// own injection count is `deliver`'s business. A gate that hard-coded the index would be
    /// asserting a number the product does not pin — and would go on passing, staged at the wrong
    /// keystroke, the first time a delivery grew an attempt.
    TheFirstKeyAtADialog,
}

impl StopsAtTheKey {
    /// `pane`, wired to end its run once `at` keystrokes have been written to it.
    pub(crate) fn nth(pane: WorkspacePaneAccess, at: usize) -> Self {
        Self::stopping(pane, StopsWhen::TheNthKey(at))
    }

    /// `pane`, wired to end its run at the first key pressed into a dialog it is showing.
    pub(crate) fn at_a_dialog(pane: WorkspacePaneAccess) -> Self {
        Self::stopping(pane, StopsWhen::TheFirstKeyAtADialog)
    }

    fn stopping(pane: WorkspacePaneAccess, when: StopsWhen) -> Self {
        Self {
            pane,
            cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            when,
            seen: std::sync::atomic::AtomicUsize::new(0),
            after: Mutex::new(Vec::new()),
        }
    }

    /// A context whose run this pane can end — the other half of the pair.
    pub(crate) fn run(&self) -> crate::run::RunContext {
        crate::run::RunContext::new(Arc::clone(&self.cancel))
    }

    /// **EVERY KEYSTROKE THIS PANE WAS GIVEN AFTER THE RUN WAS ALREADY OVER**, in the order it
    /// arrived — empty for a run that stopped typing when it stopped.
    ///
    /// ⚠⚠ The keys THEMSELVES and not a count, for R377's reason: a gate that fails on this reads
    /// *"a stopped run went on to press `Escape`"*, which names the act, and a number would leave
    /// whoever hit the red to go and find out which one.
    pub(crate) fn typed_after_the_stop(&self) -> Vec<String> {
        self.after.lock().expect("the ledger mutex").clone()
    }

    /// Whether THIS injection is the one that ends the run — asked BEFORE the write, because
    /// [`StopsWhen::TheFirstKeyAtADialog`] is about the screen the key is pressed AT.
    ///
    /// ⚠ The counter is bumped on every injection whichever trigger is armed, so a gate that reads
    /// the ledger is reading a double that never stopped watching.
    fn ends_the_run(&self, id: PaneId) -> bool {
        let seen = self.seen.fetch_add(1, std::sync::atomic::Ordering::AcqRel) + 1;
        match self.when {
            StopsWhen::TheNthKey(at) => seen >= at,
            StopsWhen::TheFirstKeyAtADialog => crate::readiness::peer_asking(&self.pane, id)
                .flatten()
                .is_some(),
        }
    }
}

impl PaneAccess for StopsAtTheKey {
    fn pane_ids(&self) -> Vec<PaneId> {
        self.pane.pane_ids()
    }
    fn pane_collapsed(&self, id: PaneId) -> Option<String> {
        self.pane.pane_collapsed(id)
    }
    fn pane_rows(&self, id: PaneId) -> Option<Vec<crate::access::PaneRow>> {
        self.pane.pane_rows(id)
    }
    fn pane_eof(&self, id: PaneId) -> Option<bool> {
        self.pane.pane_eof(id)
    }
    fn pane_full_text(&self, id: PaneId) -> Option<String> {
        self.pane.pane_full_text(id)
    }
    fn pane_full_lines(&self, id: PaneId) -> Option<Vec<String>> {
        self.pane.pane_full_lines(id)
    }
    /// ⚠ THE KEY GOES OUT UNCONDITIONALLY. A double that stopped writing once its own flag was up
    /// would be withholding the very keystroke the gates above assert reached the peer, so the
    /// flag is a consequence of the write and never a guard on it.
    ///
    /// ⚠⚠⚠ **AND THAT IS WHY THE LEDGER CAN BE TRUSTED.** A key pressed by a run that was already
    /// over reaches the peer here exactly as it would in production, and is written down — so
    /// *"nothing further was typed"* is measured rather than arranged.
    fn inject(
        &self,
        id: PaneId,
        keys: &[crate::access::KeyStroke],
    ) -> Result<crate::access::Written, crate::access::PaneError> {
        let already = self.cancel.load(std::sync::atomic::Ordering::Acquire);
        let ends = self.ends_the_run(id);
        let written = self.pane.inject(id, keys)?;
        if already {
            self.after
                .lock()
                .expect("the ledger mutex")
                .push(keys.iter().map(|key| key.key.as_str()).collect());
        } else if ends {
            self.cancel
                .store(true, std::sync::atomic::Ordering::Release);
        }
        Ok(written)
    }
    fn lifecycle(&self) -> Option<&dyn crate::access::PaneLifecycle> {
        self.pane.lifecycle()
    }
    fn raw_capture(&self) -> Option<&dyn crate::access::PaneRawCapture> {
        self.pane.raw_capture()
    }
    fn supervision(&self) -> Option<&dyn crate::access::PaneSupervision> {
        self.pane.supervision()
    }
    fn input_echo(&self) -> Option<&dyn crate::access::PaneInputEcho> {
        self.pane.input_echo()
    }
    fn terminal_modes(&self) -> Option<&dyn crate::access::PaneTerminalModes> {
        self.pane.terminal_modes()
    }
    fn foreground_job(&self) -> Option<&dyn crate::access::PaneForegroundJob> {
        self.pane.foreground_job()
    }
    fn output_lines(&self) -> Option<&dyn crate::access::PaneOutputLines> {
        self.pane.output_lines()
    }
    fn job_control(&self) -> Option<&dyn crate::access::PaneJobControl> {
        self.pane.job_control()
    }
    fn hands(&self) -> Option<&dyn crate::access::PaneHands> {
        self.pane.hands()
    }
}

/// Wait (bounded) for `marker` to appear on `pane`, so a run below starts against a peer that is
/// already up.
///
/// # ⚠⚠⚠ Why a run's own readiness barrier cannot do this job
///
/// The barrier is bounded by the RUN's clock. A fixture that leaves the wait to it is asking one
/// budget to cover two different things — a loaded box's process startup, and the behaviour the
/// gate exists to measure — and on a busy machine the first eats the second. What comes back is
/// `Exhausted(Duration)` with `Bytes(0)` charged, or a journal whose last step is the readiness
/// wait where the gate demanded the observe wait: **a red about the machine, wearing the shape of a
/// red about the product.**
///
/// Every load-marginal failure this crate has recorded is that one shape. Waiting HERE takes the
/// startup out of the run's budget entirely, so the clock can only be spent on the turn.
///
/// ⚠ **AND THEN THE BARRIER MUST CHANGE KIND, NOT DISAPPEAR.** A gate that drives the product's
/// `ready_when` should go on driving it — but [`ReadyWhen::Prints`] asks for output produced AFTER
/// it begins looking, so against a pre-waited peer it waits for a second announcement that never
/// comes. [`ReadyWhen::Shows`] is the one that reads a marker already on the screen, which is
/// exactly the state this helper leaves the pane in. R359b built that distinction for a caller;
/// this is the same distinction met from the fixture's side.
///
/// [`ReadyWhen::Prints`]: crate::readiness::ReadyWhen::Prints
/// [`ReadyWhen::Shows`]: crate::readiness::ReadyWhen::Shows
pub(crate) fn started(
    panes: &dyn crate::access::PaneAccess,
    pane: sprag_terminal::PaneId,
    marker: &str,
) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        if panes
            .pane_collapsed(pane)
            .is_some_and(|text| text.contains(marker))
        {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!(
        "the peer never printed {marker:?}, so nothing below would be measuring what it says it \
         is: {:?}",
        panes.pane_collapsed(pane),
    );
}

/// Assert that a readiness barrier REFUSED for `wanted`, and that the job it blames is the one the
/// pane was LAUNCHED as.
///
/// # ⚠⚠ Why not `assert_eq!` against the whole error
///
/// That is what these gates did, and it made every one of them assert a PLATFORM's spelling. A pane
/// spawned as `/bin/sh` is led by a process the kernel calls `sh` on Linux and `bash` on macOS, so a
/// gate comparing the error to `Job("sh")` passes on one runner and fails on the other — which is
/// how this workspace found the divergence, one red at a time, after a push.
///
/// The spelling was never the claim. The claim is that **the refusal names the program the caller
/// launched, in the caller's own word** — and [`JobLeader::answers_to`] is what the product itself
/// decides that with, so a gate written this way measures the product's answer rather than a
/// distribution's packaging.
/// ⚠ Takes the FAILURE rather than the outcome, because a barrier's refusal reaches its two callers
/// in two shapes — `Err` from [`Readiness::reached`](crate::Readiness::reached), and an
/// `Outcome::failure` from a run — and a gate for either is asking the same question.
pub(crate) fn refused_naming(
    failure: Option<&crate::access::PaneError>,
    wanted: &crate::ReadyWhen,
    launched_as: &str,
    why: &str,
) {
    let Some(crate::access::PaneError::NeverReady {
        wanted: asked,
        instead,
        ..
    }) = failure
    else {
        panic!(
            "{why} — but the barrier did not refuse for a readiness it never reached: {failure:?}"
        );
    };
    assert_eq!(
        asked, wanted,
        "{why} — and the refusal hands back the WHOLE question, or a caller cannot tell which of \
         the kinds they got wrong",
    );
    let Some(leader) = instead.leader() else {
        panic!("{why} — but nothing was reported as owning the pane's terminal: {instead}");
    };
    assert!(
        leader.answers_to(launched_as),
        "{why} — the refusal blames {leader} on a pane launched as {launched_as:?}, and a \
         correction phrased in a word the caller never wrote is one they cannot act on",
    );
}

// ── THE ANSWERING CONTRACT'S PEER ─────────────────────────────────────────────────────────────
//
// ⚠⚠⚠ EVERY GATE THAT USES THIS DRIVES A REAL PSEUDOTERMINAL RUNNING A REAL MENU, and the
// observation the product reads is derived by the SHIPPING parser (`sprag_detect::question`) from
// that pane's actual screen — the same derivation the daemon's own `agent_state_source` makes. A
// double reporting a hand-built `Question` would have asserted a belief about dialogs; this asserts
// what the product does to one.
//
// ⚠⚠ AND THE PEER SAYS WHICH KEY IT ACTED ON. Every claim here is about which keystrokes a run
// sends, so a fixture that only reported the OUTCOME would pass for a run that typed a digit it did
// not need — the exact over-typing this contract is about. The peer prints
// `TOOK <option> VIA <byte> AFTER <the bytes it ignored>` when it acts, `SAW <byte>` for a key it
// ignores, and `EXTRA <byte>` for anything that arrives after it is done. The pane is the witness.
//
// ⚠ `AFTER` exists because ACTING CLEARS THE SCREEN, so the `SAW` lines that led up to it are wiped
// by the redraw — and the ORDER two keys went in is exactly what an escalation gate has to assert.
// Carried in a variable, it survives the clear.
//
// ⚠⚠ IT LIVES HERE, not beside one caller's gates, because TWO paths reach a blocked peer now: a
// run's readiness barrier (`crate::readiness`) and the one-shot pane answer (`crate::answer`). A
// second copy of this peer would let the two drift, and what they would drift about is what a
// dialog does to a keystroke — which is the one belief this whole contract refuses to hold on its
// own authority.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use sprag_detect::AgentState;
use sprag_terminal::{CommandBuilder, PaneId, Workspace};

use crate::access::{
    AgentObservation, AgentStateSource, Authority, PaneAccess, WorkspacePaneAccess,
};
use crate::driver::Ceiling;

/// A peer that draws a bottom-anchored numbered menu and reacts to single keystrokes, in one of
/// the four dialog behaviours a run has to survive.
///
/// * `numbers` — the DIGIT selects outright and Enter does nothing. The measured behaviour of
///   the agents this reads, and the one where a reflexive trailing Enter lands on whatever the
///   peer shows next.
/// * `marker` — the digit only MOVES the highlight; Enter commits whatever it is on. The
///   behaviour where an Enter is required, and may only be sent once the marker can be SEEN on
///   the authorised option.
/// * `either` — both, which is what a real agent's permission dialog does. The kind that makes
///   *"do not type a key you do not need"* a claim with consequences.
/// * `deaf` — nothing works. The peer that makes [`Refusal::NotTaken`] reachable.
pub(crate) fn menu_peer(kind: &str) -> String {
    format!(
        r#"
stty -icanon -echo 2>/dev/null
kind={kind}
sel=1
seen=''
readbyte() {{ dd bs=1 count=1 2>/dev/null | od -An -tu1 | tr -d ' \n'; }}
draw() {{
  printf '\033[2J\033[H'
  printf 'Bash command\r\n'
  printf 'Do you want to proceed?\r\n'
  i=1
  for label in 'Yes' 'Yes, and do not ask again' 'No, and tell me what to do'; do
if [ "$i" = "$sel" ]; then printf '\342\235\257 '; else printf '  '; fi
printf '%s. %s\r\n' "$i" "$label"
i=$((i+1))
  done
}}
took() {{
  printf '\033[2J\033[H'
  printf 'TOOK %s VIA %s AFTER%s\r\n' "$sel" "$1" "$seen"
  while :; do
e=$(readbyte)
[ -n "$e" ] || exit 0
printf 'EXTRA %s\r\n' "$e"
  done
}}
draw
while :; do
  k=$(readbyte)
  [ -n "$k" ] || exit 0
  case "$k" in
49|50|51)
  case "$kind" in
    numbers|either) sel=$((k-48)); took "$k" ;;
    marker) sel=$((k-48)); draw ;;
    *) seen="$seen $k"; printf 'SAW %s\r\n' "$k" ;;
  esac ;;
13|10)
  case "$kind" in
    marker|either) took "$k" ;;
    *) seen="$seen $k"; printf 'SAW %s\r\n' "$k" ;;
  esac ;;
*) seen="$seen $k"; printf 'SAW %s\r\n' "$k" ;;
  esac
done
"#
    )
}

/// The byte the peer reports for Enter, so a gate names a KEY and not a number nobody can read.
/// `VIA 10` is Enter; `VIA 50` is the digit `2`.
///
/// ⚠ TEN, not thirteen — [`KeyStroke::named("Enter")`](crate::access::KeyStroke::named) encodes
/// LF and not CR, which the fixture MEASURED rather than assumed (the first draft of these
/// gates asserted `13` and the pane said `10`). The peer accepts both, so the gates are about
/// which key the RUN chose to send and not about which byte a terminal calls Enter.
pub(crate) const ENTER_BYTE: &str = "10";

/// A pane running `script` under `/bin/sh`, wrapped in a pane-access whose SUPERVISOR is derived
/// from that pane's own screen by the shipping choice-list parser.
///
/// ⚠ `Blocked` exactly when the screen carries a menu. That is the daemon's own rule for the
/// `asking` field (`agent_state_source`), reproduced here rather than mocked, so a gate cannot pass
/// against a question the product would not have parsed — and *"this pane is not asking"* is that
/// same parser's verdict rather than a `None` a double chose to hand back.
fn peer_running(script: String) -> (WorkspacePaneAccess, PaneId) {
    let workspace = Arc::new(Mutex::new(Workspace::new((60, 12))));
    let pane = {
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(script);
        command.env("TERM", "dumb");
        workspace
            .lock()
            .expect("the workspace mutex")
            .spawn(command, "peer".to_string(), 60, 12)
            .expect("spawn the peer")
    };
    // ⚠⚠⚠ IT SETTLES, like the real one. A supervisor publishes a resting verdict only once a
    // candidate has held for its window, so a pane whose dialog has just been answered goes on
    // reading `Blocked` with NOTHING readable on it for that long. A source derived straight
    // from the screen has no such lag — and its absence hid a live defect from every gate in
    // this file until an end-to-end run through a real daemon met it: the step after a
    // successful answer read the stale verdict as a fresh one and reported that a person was
    // needed. **A double that cannot be wrong in the way the real thing is wrong is a double
    // that asserts your belief.**
    //
    // ⚠ Far shorter than `sprag_detect::DEFAULT_SETTLE`, because what is under test is that the
    // product tolerates a lag AT ALL rather than any particular length of one — and a gate that
    // paid two seconds per answer would be bought with wall-clock nobody gets back.
    const FIXTURE_SETTLE: Duration = Duration::from_millis(300);
    let source = {
        let workspace = Arc::clone(&workspace);
        let last_menu: Mutex<Option<std::time::Instant>> = Mutex::new(None);
        Arc::new(move |id: PaneId| {
            let guard = workspace.lock().expect("the workspace mutex");
            guard.pane(id)?.pty().with_screen(|screen| {
                let asking = sprag_detect::question(screen, sprag_detect::DIALOG_WINDOW);
                let mut seen = last_menu.lock().expect("the settle mutex");
                if asking.is_some() {
                    *seen = Some(std::time::Instant::now());
                }
                let settling = seen.is_some_and(|at| at.elapsed() < FIXTURE_SETTLE);
                Some(AgentObservation {
                    state: if asking.is_some() || settling {
                        AgentState::Blocked
                    } else {
                        AgentState::Idle
                    },
                    agent: Some("claude".to_string()),
                    authority: Authority::Scraped {
                        rule: Some("dialog-choice-list".to_string()),
                    },
                    seq: 1,
                    asking,
                })
            })
        }) as AgentStateSource
    };
    let access = WorkspacePaneAccess::new(workspace).with_agent_state(Some(source));
    (access, pane)
}

/// Wait (bounded) until the shipping parser reads a menu on `pane`, so nothing below is about a
/// pane that was never blocked.
fn awaiting_the_menu(access: &WorkspacePaneAccess, pane: PaneId) {
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(10)
        && crate::readiness::peer_asking(access, pane)
            .flatten()
            .is_none()
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        crate::readiness::peer_asking(access, pane)
            .flatten()
            .is_some(),
        "the fixture's peer must be showing a menu the shipping parser reads, or this gate is \
         about nothing: {:?}",
        access.pane_collapsed(pane),
    );
}

/// A pane running [`menu_peer`], already showing its menu.
pub(crate) fn asking_peer(kind: &str) -> (WorkspacePaneAccess, PaneId) {
    let (access, pane) = peer_running(menu_peer(kind));
    awaiting_the_menu(&access, pane);
    (access, pane)
}

/// ONE TURN THAT ASKS TWICE — a peer that shows a permission dialog, and on being answered shows a
/// SECOND, DIFFERENT one before it finishes.
///
/// # ⚠⚠⚠ Why the shape of a real turn needed its own peer
///
/// [`menu_peer`] asks ONE question and is done, which is the shape every answering gate was written
/// against — and it is not the shape of an agent's turn. A `claude` turn that edits a file after
/// running a command asks *"Bash command … Do you want to proceed?"* and then *"Edit file … Do you
/// want to make this edit?"*: two questions, in the agent's own different words, inside one turn a
/// caller left unattended.
///
/// A fixture that only ever asks once cannot tell a contract that covers a TURN from one that covers
/// a QUESTION, so every gate over the single-question peer passes either way. This one makes the
/// difference expressible.
///
/// ⚠ It behaves as `either` does — the digit selects outright AND Enter commits the marker — which
/// is the measured behaviour of a real permission dialog and the kind that makes *"do not type a
/// key you do not need"* a claim with consequences.
///
/// ⚠ The trail SURVIVES the clears: acting wipes the screen, so what was taken on the first
/// question would be gone by the time the second is answered. `TURN COMPLETE` carries both, in
/// order, with the byte that took each — so a gate reads which options a run actually chose rather
/// than trusting the outcome's own tally.
pub(crate) const TWO_QUESTION_TURN: &str = r#"
stty -icanon -echo 2>/dev/null
sel=1
stage=1
trail=''
readbyte() { dd bs=1 count=1 2>/dev/null | od -An -tu1 | tr -d ' \n'; }
draw() {
  printf '\033[2J\033[H'
  if [ "$stage" = 1 ]; then
printf 'Bash command\r\n'
printf 'Do you want to proceed?\r\n'
  else
printf 'Edit file src/main.rs\r\n'
printf 'Do you want to make this edit?\r\n'
  fi
  i=1
  for label in 'Yes' 'Yes, and do not ask again' 'No, and tell me what to do'; do
if [ "$i" = "$sel" ]; then printf '\342\235\257 '; else printf '  '; fi
printf '%s. %s\r\n' "$i" "$label"
i=$((i+1))
  done
}
took() {
  trail="$trail TOOK-$stage-$sel-VIA-$1"
  if [ "$stage" = 1 ]; then
stage=2
sel=1
draw
  else
printf '\033[2J\033[H'
printf 'TURN COMPLETE%s\r\n' "$trail"
while :; do
  e=$(readbyte)
  [ -n "$e" ] || exit 0
  printf 'EXTRA %s\r\n' "$e"
done
  fi
}
draw
while :; do
  k=$(readbyte)
  [ -n "$k" ] || exit 0
  case "$k" in
49|50|51) sel=$((k-48)); took "$k" ;;
13|10) took "$k" ;;
*) trail="$trail SAW-$k" ;;
  esac
done
"#;

/// A pane running [`TWO_QUESTION_TURN`], already showing the FIRST of its two questions.
pub(crate) fn two_question_peer() -> (WorkspacePaneAccess, PaneId) {
    let (access, pane) = peer_running(TWO_QUESTION_TURN.to_string());
    awaiting_the_menu(&access, pane);
    (access, pane)
}

/// The script for a peer whose work CANNOT FINISH until a person has reached into the pane
/// themselves.
///
/// # ⚠⚠⚠ Why the fixture is built this way rather than with a turn count
///
/// The claim under test is *a run comes back after the person lets go*, and the obvious fixture —
/// a peer that converges on its Nth turn, with a person typing somewhere in the middle — is a RACE:
/// whether the run reaches N before or after the person's key decides the reading, so a green is a
/// statement about this machine's scheduler. Here the sentinel is unreachable until byte 88 has
/// arrived from a person's hand, and it takes ONE MORE TURN after that. A run that stops on the
/// interruption cannot converge no matter how fast it was, and a run that comes back cannot fail to,
/// no matter how slow.
///
/// ⚠ `HANDED BACK` ends its own row, and the sentinel is printed only on the Enter AFTER it — so a
/// gate cannot pass on the echo of the person's own keystroke. Neither marker is anything a caller
/// types, which is [`ReadyWhen::Prints`](crate::readiness::ReadyWhen::Prints)'s rule applied to a
/// convergence sentinel.
///
/// ⚠⚠⚠ **HOW LONG A TURN TAKES IS THE CALLER'S, AND BOTH ANSWERS ARE LOAD-BEARING.** `slow_turns`
/// puts a whole second on every turn BEFORE the person acts, and nothing after it:
///
/// * **A gate about a run that STOPS wants it.** Without it a run with a budget of forty can burn
///   the whole budget in the time the person's thread takes to notice the run started, and the
///   control reads `exhausted` — green for the wrong reason.
/// * **A gate about what a run does WHILE somebody types must not have it**, and this was measured
///   rather than reasoned about: with second-long turns the run's own step cadence is longer than a
///   person's pauses, so a build that resumed the instant it noticed stillness had NO CHANCE to
///   type into one. The mutation that removes the whole stillness rule passed. A brisk peer gives
///   it ten chances per pause, and the same mutation goes red.
fn work_needing_a_person_script(slow_turns: bool) -> String {
    let pause = if slow_turns { "sleep 1; " } else { "" };
    format!(
        "stty -icanon -echo 2>/dev/null\n\
         printf 'AT REST\\r\\n'\n\
         handed=0\n\
         turns=0\n\
         while :; do\n\
           k=$(dd bs=1 count=1 2>/dev/null | od -An -tu1 | tr -d ' \\n')\n\
           [ -n \"$k\" ] || exit 0\n\
           case \"$k\" in\n\
             88) handed=1; printf 'HANDED BACK\\r\\n' ;;\n\
             13|10)\n\
               turns=$((turns+1))\n\
               if [ \"$handed\" = 1 ]; then printf 'WORK DONE\\r\\n'; else {pause}printf 'TURN %s\\r\\n' \"$turns\"; fi ;;\n\
             *) printf 'SAW %s\\r\\n' \"$k\" ;;\n\
           esac\n\
         done\n"
    )
}

/// A pane running that peer, settled at its readiness marker and asking nothing.
pub(crate) fn work_needing_a_person(slow_turns: bool) -> (WorkspacePaneAccess, PaneId) {
    let (access, pane) = peer_running(work_needing_a_person_script(slow_turns));
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(10)
        && !access
            .pane_collapsed(pane)
            .is_some_and(|text| text.contains("AT REST"))
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        crate::readiness::peer_asking(&access, pane).is_none(),
        "⚠⚠ the peer must read as NOT asking, or a gate about a person TAKING the pane is about a \
         dialog instead: {:?}",
        access.pane_collapsed(pane),
    );
    (access, pane)
}

/// **A PERSON, AT THE KEYBOARD OF THE PANE THEY ARE WATCHING** — bytes put in through the door a
/// display client writes through, which is not the door a run writes through.
///
/// # ⚠⚠⚠ Why a fixture may not spell this `access.inject(…)`
///
/// It did, and every gate that used it was staging the person out of the distinction it claimed to
/// be about. [`PaneAccess::inject`] is what the RUN types with; a person at a pane goes through
/// `HostClient::send_key` → [`PanePtyHandle::write`], and the host's own encoder documents the two
/// as deliberately identical *on the wire* (`sprag_host::pane::send_key`: *"the human keyboard path
/// and the AI `scene/invoke` path encode IDENTICALLY"*). Encoding identically is right. Being
/// INDISTINGUISHABLE AFTERWARDS is the defect, and a fixture that reaches for the run's own door
/// cannot see it.
///
/// ⚠ Bytes rather than a [`KeyStroke`](crate::access::KeyStroke): the encoder is `sprag-input`'s and
/// lives above this crate, so a fixture that took keys here would be re-implementing it. What a
/// person's hand produces is bytes at a device, which is exactly what this writes.
pub(crate) fn person_types(access: &WorkspacePaneAccess, pane: PaneId, bytes: &[u8]) {
    access
        .handle(pane)
        .expect("the pane a person is typing into")
        .write(bytes, sprag_terminal::Hand::APerson)
        .expect("the person's keystroke reaches the device");
}

/// **A HOST THAT CANNOT SAY WHOSE KEYSTROKES THESE WERE** — every surface of the access it wraps,
/// minus [`PaneAccess::hands`].
///
/// # ⚠⚠⚠ Why the absence needs a fixture of its own
///
/// [`PaneAccess::hands`] is `None` by default, and its documentation makes a SAFETY claim about
/// that: an absence must be read as *carry on*, never as *somebody is present*, because the second
/// reading would stop every run on every host that has not implemented the capability. That is a
/// claim about what the product does, and until this existed it was only a sentence.
///
/// ⚠ It delegates rather than reimplementing, so what it measures is the REAL barrier meeting a
/// real pane with one capability withheld — the same construction the supervision fixtures use, and
/// the reason a double is not good enough here: the interruption check is the only thing that may
/// differ.
pub(crate) struct HandlessAccess(pub(crate) WorkspacePaneAccess);

impl PaneAccess for HandlessAccess {
    fn pane_ids(&self) -> Vec<PaneId> {
        self.0.pane_ids()
    }
    fn pane_collapsed(&self, id: PaneId) -> Option<String> {
        self.0.pane_collapsed(id)
    }
    fn pane_rows(&self, id: PaneId) -> Option<Vec<crate::access::PaneRow>> {
        self.0.pane_rows(id)
    }
    fn pane_eof(&self, id: PaneId) -> Option<bool> {
        self.0.pane_eof(id)
    }
    fn pane_full_text(&self, id: PaneId) -> Option<String> {
        self.0.pane_full_text(id)
    }
    fn pane_full_lines(&self, id: PaneId) -> Option<Vec<String>> {
        self.0.pane_full_lines(id)
    }
    fn inject(
        &self,
        id: PaneId,
        keys: &[crate::access::KeyStroke],
    ) -> Result<crate::access::Written, crate::access::PaneError> {
        self.0.inject(id, keys)
    }
    fn lifecycle(&self) -> Option<&dyn crate::access::PaneLifecycle> {
        self.0.lifecycle()
    }
    fn raw_capture(&self) -> Option<&dyn crate::access::PaneRawCapture> {
        self.0.raw_capture()
    }
    fn supervision(&self) -> Option<&dyn crate::access::PaneSupervision> {
        self.0.supervision()
    }
    fn input_echo(&self) -> Option<&dyn crate::access::PaneInputEcho> {
        self.0.input_echo()
    }
    fn terminal_modes(&self) -> Option<&dyn crate::access::PaneTerminalModes> {
        self.0.terminal_modes()
    }
    fn foreground_job(&self) -> Option<&dyn crate::access::PaneForegroundJob> {
        self.0.foreground_job()
    }
    fn output_lines(&self) -> Option<&dyn crate::access::PaneOutputLines> {
        self.0.output_lines()
    }
    fn job_control(&self) -> Option<&dyn crate::access::PaneJobControl> {
        self.0.job_control()
    }
    /// ⚠ THE ONE WITHHELD SURFACE — the whole point of this wrapper.
    fn hands(&self) -> Option<&dyn crate::access::PaneHands> {
        None
    }
}

/// Wait (bounded) for `pane` to show `needle`, then hand back the whole collapsed screen —
/// which is what every assertion below reads, including the ones about what is NOT there.
pub(crate) fn screen_showing(access: &WorkspacePaneAccess, pane: PaneId, needle: &str) -> String {
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(5)
        && !access
            .pane_collapsed(pane)
            .is_some_and(|text| text.contains(needle))
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    access.pane_collapsed(pane).unwrap_or_default()
}

/// A pane whose peer is plainly NOT asking anything — the control every *"it answered"* gate needs
/// beside it, and the state a supervisor's read races.
///
/// # ⚠⚠ Why it runs a program rather than being an empty pane
///
/// A pane with nothing in it is a pane whose screen is blank, and a blank screen is what a peer
/// looks like for the instant before it draws. This one prints a line and then BLOCKS ON ITS
/// INPUT, so a gate that reads it is reading a settled pane running something — and anything the
/// product typed at it would be visible, which is the assertion.
///
/// ⚠ Its supervisor is the same screen-derived one [`asking_peer`] builds, so *"not asking"* here
/// is the shipping parser's verdict about a real screen rather than a `None` a double handed back.
pub(crate) fn silent_peer() -> (WorkspacePaneAccess, PaneId) {
    let (access, pane) = peer_running(
        "stty -icanon -echo 2>/dev/null\n\
         printf 'AT REST\\r\\n'\n\
         while :; do\n\
           k=$(dd bs=1 count=1 2>/dev/null | od -An -tu1 | tr -d ' \\n')\n\
           [ -n \"$k\" ] || exit 0\n\
           printf 'SAW %s\\r\\n' \"$k\"\n\
         done\n"
            .to_string(),
    );
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(10)
        && !access
            .pane_collapsed(pane)
            .is_some_and(|text| text.contains("AT REST"))
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        crate::readiness::peer_asking(&access, pane).is_none(),
        "⚠⚠ the control must be a pane the shipping parser reads as NOT blocked, or a gate about \
         `there was nothing to answer` is about a parse failure instead: {:?}",
        access.pane_collapsed(pane),
    );
    (access, pane)
}

/// The token a stand-in peer publishes ITS OWN reply counter behind — see [`peer_seq`].
///
/// ⚠⚠⚠ **THE FORTY-EIGHT LINES THAT USED TO BE HERE WERE [`standin_agent`]'s DOC**, attached to
/// this three-word constant because a doc comment binds to the NEXT item and the function it
/// described is a hundred lines further down. Every one of those paragraphs records a run that went
/// wrong, and all of them were being read as commentary on a string literal — while the peer they
/// are about carried no doc at all. Found by this round's own sweep.
const SEQ_MARKER: &str = "SEQ";

/// **THE PEER'S OWN PUBLISHED-CHANGE COUNTER**, read off the screen — what the two supervisors
/// below report as [`AgentObservation::seq`](crate::access::AgentObservation::seq).
///
/// # ⚠⚠⚠ Why this replaced COUNTING THE PEER'S TOKENS, and the number that said so
///
/// Both supervisors used to count occurrences of `ACK` and `MILESTONE` on the collapsed screen and
/// hold the total monotone by hand, with a doc explaining that a scrolled pane would otherwise make
/// it go DOWN. Holding it monotone stops the count SHRINKING; it does not make it GROW. So once the
/// pane had scrolled, the high-water mark was reached and `seq` never moved again — and
/// `DoneWhen::Settles` compares `seq` against the turn's arming, so **no further turn could ever
/// end**.
///
/// Measured, driving the refusing peer for six turns: the walk was
/// `Idle -> Priming -> Working -> Screening -> … -> Judging` three times and then
/// `Working --Null--> Working` **twenty-two times** until the run's wall clock. Three turns is the
/// most any gate in this crate could have driven, because the authored prompts are three or four
/// rows each and the pane is sixteen — nobody had asked for a fourth.
///
/// The peer prints a counter it increments on every reply, so the LARGEST one on screen is the
/// current total whatever has scrolled off. That is also closer to what the field means: a real
/// detector's `seq` is a count of published changes, not of words a screen happens to still hold.
///
/// ⚠ The monotone latch is KEPT anyway, and not as decoration: the refusing peer CLEARS the screen
/// between turns, so there is an instant with no counter on it at all.
///
/// See also [`has_painted`], which is the other half of what a supervisor must not over-claim.
///
/// ⚠⚠⚠ **AND THE LATCH IS PER PANE, which a session REPLACEMENT is what measured.** One `AgentStateSource`
/// answers about every pane it is asked about, and it held ONE high-water mark — so when a loop's
/// `restarting` closed its inner pane and opened a fresh one, the new peer's counter started at 1
/// against a mark of 3 and **could never exceed it**: the run reached `priming` on the replacement
/// session, prompted it, and then sat in `Working --Null--> Working` until its wall clock. A real
/// detector's `seq` is a per-pane count and a new pane starts at its own zero, so the stand-in's has
/// to as well.
///
/// ⚠⚠⚠ **AND IT READS ROWS, NOT [`pane_collapsed`](crate::access::PaneAccess::pane_collapsed)** —
/// R383's rule, paid a second time by the first draft of this very function. That read joins the
/// rows **without separators**, exactly as its doc says, so `str::lines` yields ONE line and a
/// per-row prefix match finds nothing: every turn reported `seq` 0 and the two gates that drive this
/// peer both stalled at the FIRST turn rather than the fourth. The old token count survived a joined
/// string only because `str::matches` does not care where a row ends.
fn peer_seq(rows: &[String]) -> u64 {
    rows.iter()
        .filter_map(|row| row.trim().strip_prefix(SEQ_MARKER))
        .filter_map(|count| count.trim().parse::<u64>().ok())
        .max()
        .unwrap_or(0)
}

/// **HAS THIS PANE'S PROGRAM PAINTED ANYTHING YET?** — what stops the two supervisors below claiming
/// an agent is present on a pane that has drawn nothing.
///
/// # ⚠⚠⚠ Why a blank pane must answer `agent: None`, measured
///
/// [`ReadyWhen::Settles`](crate::readiness::ReadyWhen) is satisfied when the supervisor reports THAT
/// AGENT at rest, and both supervisors used to name it unconditionally. So a pane whose child had not
/// yet been scheduled — a blank grid, microseconds old — read as *the agent is here and idle*, and a
/// barrier over it came down at once.
///
/// It went unnoticed for as long as no run met a pane it had not waited for by hand: every fixture
/// waits for its peer's announcement before the run begins. A loop that REPLACES its own session meets
/// exactly that pane, and the gate for it measured the consequence — the barrier cleared on the blank
/// screen, the loop primed and typed its first prompt, and the menu the replacement was ABOUT to paint
/// arrived afterwards and was met as a blocked TURN instead of as a session that never came up.
///
/// R350 recorded this from the other side: *"the stand-in agent paints no title, no spinner, no
/// footer, so the pane has NO agent key before its first event"* — the property that made an
/// end-to-end proof possible there, over-claimed here.
fn has_painted(rows: &[String]) -> bool {
    rows.iter().any(|row| !row.trim().is_empty())
}

/// The monotone latch [`peer_seq`] is read through — **one high-water mark PER PANE**.
///
/// See `peer_seq` for the run that measured why it cannot be one mark for the source: a pane a loop
/// has REPLACED is a different pane, and its peer's counter starts again at one.
type SeqHighWater = Arc<Mutex<std::collections::HashMap<PaneId, u64>>>;

/// `rows`' counter, never below what this pane has already published.
fn latched(high: &SeqHighWater, pane: PaneId, rows: &[String]) -> u64 {
    let seen = peer_seq(rows);
    let mut marks = high.lock().expect("the high-water mutex");
    let mark = marks.entry(pane).or_default();
    *mark = (*mark).max(seen);
    *mark
}

/// A stand-in AGENT CLI: long-lived, echo off, answers every prompt and says the document's
/// done marker once it has taken `prompts_before_done` turns.
///
/// ⚠ SHARED between the outer driver's gates and the loop PLUGIN's, and that is the point: those
/// two subjects are one pane's behaviour seen from two heights, so a second copy of this peer would
/// be two fixtures a change could drift between. It moved here the round the plugin was built.
///
/// ⚠⚠ **ECHO OFF AND A READINESS MARKER THAT ENDS ITS ROW** — both recorded hazards. With echo
/// on, the line discipline paints the prompt before the program reads a byte and every wait
/// ends on the kernel's work rather than the peer's; with the marker mid-row, the first
/// stimulus merges onto it and reads as the pane's own output.
///
/// ⚠ It is `read`-driven, so it consumes what is typed at it. A stand-in that merely SLEEPS
/// does not stand in for an agent: nothing eats the stimulus, so it waits in the pty buffer
/// and the run converges either way.
///
/// ⚠⚠⚠ **IT ANSWERS ONE PROMPT, NOT ONE LINE, AND THE FIRST RUN OF THAT GATE IS WHY.** The
/// authored prompts are MULTI-LINE — `start_prompt` is four clauses joined with `\n` — so a
/// `read line` stand-in took one delivery as four turns and said the done marker during the
/// first one. The peer therefore keys on each prompt's LAST clause, which is the honest shape:
/// a real agent CLI takes a whole prompt box and answers it once.
///
/// ⚠⚠ **AND THE PRODUCT QUESTION THAT FOUND IS REGISTERED, NOT FIXED HERE**: what a newline
/// INSIDE an authored prompt does to a peer that submits on Enter is a live question about
/// delivery, and this fixture is not the place to answer it.
///
/// ⚠⚠ **AND IT KEYS ON `exactly:` BECAUSE THE DOCUMENT'S LAST CLAUSE MOVED THERE** (R379): the
/// working prompts now end with `done_instruction`, so a peer keying on the OLD last clause
/// (*"Report what you did"*) would count a turn one clause early and answer into the middle of
/// a delivery.
///
/// ⚠⚠⚠ **AND IT ANSWERS BOTH ENDINGS' QUESTIONS, WHICH THE ROUND `stopping` WAS BUILT MEASURED.**
/// `closing` and `stopping` each send a question this peer would otherwise not recognise, and a
/// stand-in that ignores what it is asked never publishes another change — the turn never ends, the
/// loop reports `Stopping --Null--> Stopping` to its wall clock, and **four gates measuring the turn
/// budget came back `exhausted — duration` about a run that had spent exactly the turns it was
/// briefed with**. See [`STOP_QUESTION`].
///
/// ⚠⚠⚠ **IT PAINTS WHAT IT READS, AND THE SECOND RUN OF THAT GATE IS WHY.** With echo off and
/// nothing painted, [`deliver`](crate::deliver::deliver) can never confirm the prompt arrived, so
/// it RETYPES it — and a peer counting prompts saw two where the driver sent one, converging a
/// turn early. A real agent CLI paints the prompt into its own box, which is the whole reason
/// `deliver` reads the screen back; a stand-in that stayed silent was testing the retry path, not
/// the loop.
///
/// ⚠⚠⚠ **AND IT ANSWERS THE REFLECTION PROMPT, NAMING NOTHING** — added the round `reflecting`
/// became a TURN, and the four gates that went red are the argument. A peer that IGNORES a prompt
/// is not standing in for an agent: the turn never ends, the loop reports `Reflecting --Null-->
/// Reflecting` until its wall clock, and four gates measuring something else entirely died of it.
/// **A stand-in must answer whatever it is asked**, even where the gate driving it does not care
/// what the answer is.
///
/// ⚠ It names NO next milestone on purpose, which keeps this peer's axis exactly what it was. The
/// arm where an agent DOES decide one is [`standin_agent_reflecting`]'s, and mixing the two would
/// make every gate here also a gate about the reflection's reader.
pub(crate) fn standin_agent(prompts_before_done: u32) -> (Arc<Mutex<Workspace>>, PaneId) {
    let workspace = Arc::new(Mutex::new(Workspace::new((STANDIN_COLUMNS, 16))));
    let script = format!(
        "stty -echo; printf 'AGENT-READY\\n'; n=0; s=0; \
         while read line; do \
           printf '%s\\n' \"$line\"; \
           case \"$line\" in \
             *'{REFLECT}'*) \
               printf 'ACK nothing to change\\n'; \
               s=$((s+1)); printf '{SEQ} %s\\n' \"$s\"; continue;; \
           esac; \
           case \"$line\" in \
             *exactly:*|*Summarise*|*'{STOP}'*) ;; \
             *) continue;; \
           esac; \
           n=$((n+1)); \
           if [ $n -ge {prompts_before_done} ]; then printf 'MILESTONE REACHED\\n'; \
           else printf 'ACK %s\\n' \"$n\"; fi; \
           s=$((s+1)); printf '{SEQ} %s\\n' \"$s\"; \
         done",
        SEQ = SEQ_MARKER,
        REFLECT = REFLECTION_MILESTONE_LABEL,
        STOP = STOP_QUESTION,
    );
    let pane = {
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(script);
        command.env("TERM", "dumb");
        workspace
            .lock()
            .unwrap()
            .spawn(command, "sh".to_string(), STANDIN_COLUMNS, 16)
            .expect("spawn pane")
    };
    started(
        &WorkspacePaneAccess::new(Arc::clone(&workspace)),
        pane,
        "AGENT-READY",
    );
    (workspace, pane)
}

/// **A STAND-IN AGENT THAT WILL NAME ITS OWN NEXT MILESTONE IF IT IS EVER ASKED** — the peer a
/// reflection TURN is measured against.
///
/// # ⚠⚠⚠ What it stands in for, and why it is a peer rather than a longer script
///
/// [`standin_agent`] answers working prompts and says the done marker. It has no opinion about
/// where the work should go next, because until a reflection could ASK, nothing in the product
/// wanted one. This peer has exactly one axis more: **a prompt carrying the document's milestone
/// LABEL is answered with a next milestone and a next reference**, in the two-line shape the
/// authored `reflect_prompt` asks for.
///
/// ⚠⚠ **IT KEYS ON THE LABEL, WHICH IS THE PRODUCT'S OWN CONTRACT** — the same discipline
/// [`standin_agent`] uses in keying its done reply on `exactly:`. A peer keying on some private
/// word would answer a prompt no real agent could recognise, and the gate would measure the
/// fixture. ⚠ The label is spelled here rather than read from the document for the reason
/// `MILESTONE REACHED` is spelled in [`standin_agent`]: a fixture cannot read a datamodel that has
/// not been initialised. What holds the two in step is that the gate asserts the composed
/// `reflect_prompt` carries this same label.
///
/// ⚠⚠⚠ **AND IT STAGES BOTH OF THE READER'S HAZARDS ON PURPOSE, WHICH MUTATION IS WHY.** The peer
/// answers with FOUR rows and only the last two are its answer:
///
/// 1. **A ROW SHAPED LIKE A WRAPPED ECHO** — the label followed by a VERBATIM slice of the prompt
///    that asked for it ([`REFLECTION_ECHO_SLICE`]). The prompt names the label mid-sentence
///    precisely so its echo does not open a row with it, and a terminal wraps where it likes, so
///    the day a pane is a different width this row is what the screen holds.
/// 2. **A ROW THE AGENT THOUGHT BETTER OF** ([`REFLECTION_PROVISIONAL`]) — an agent asked for two
///    lines that writes a paragraph first, which is what agents do.
///
/// ⚠⚠⚠ **NEITHER WAS REACHABLE BEFORE THEY WERE STAGED, AND THE MUTATIONS SAID SO.** The first
/// draft of this peer answered with its two real lines alone: dropping the echo discount from
/// [`OuterLoop::proposed`](crate::outer::OuterLoop) left the gate GREEN, and so did taking the
/// FIRST match instead of the last — because at 80 columns the prompt's own wrap happened to break
/// the label across two rows. **Both of the reader's rules were untested and its doc claimed both.**
/// R374's rule: when a state is only reached by luck, stage it on purpose.
pub(crate) fn standin_agent_reflecting(
    prompts_before_done: u32,
    next_milestone: &str,
    next_reference: &str,
) -> (Arc<Mutex<Workspace>>, PaneId) {
    standin_agent_reflecting_at(
        STANDIN_COLUMNS,
        prompts_before_done,
        next_milestone,
        next_reference,
    )
}

/// The width every stand-in here is spawned at unless a gate is ABOUT the width.
///
/// ⚠ It is eighty because that is what a terminal is, and for most gates it is a number nobody
/// looked at. **One gate has looked**: `done_instruction` is 109 characters with its marker at 92
/// and `reflect_prompt`'s last line is 152 with its marker at 134, so 80 breaks neither of them at
/// a marker — which is exactly why the hazard those two lines carry went unmeasured until it was
/// asked for on purpose. See [`standin_agent_reflecting_at`].
pub(crate) const STANDIN_COLUMNS: u16 = 80;

/// [`standin_agent_reflecting`] on a pane of a chosen width.
///
/// # ⚠⚠⚠ Why a width is a parameter at all
///
/// Every other fixture in this module takes [`STANDIN_COLUMNS`], and that is fine while nothing
/// depends on the width. **The markers depend on it.** `reflect_prompt`'s last
/// line ENDS with `north_star_marker` and is 152 characters with the marker at 134, so a pane 67 or
/// 134 columns wide breaks that sentence exactly there and leaves the marker alone on a row — a row
/// that `stands_alone` accepts, on a screen where the agent has said nothing. A caller does not
/// choose their agent's pane width, so the driver has to survive every one of them, and a gate that
/// says so needs to be able to spawn a hostile one.
pub(crate) fn standin_agent_reflecting_at(
    columns: u16,
    prompts_before_done: u32,
    next_milestone: &str,
    next_reference: &str,
) -> (Arc<Mutex<Workspace>>, PaneId) {
    let workspace = Arc::new(Mutex::new(Workspace::new((columns, 16))));
    let script = "\
stty -echo; printf 'AGENT-READY\\n'; n=0; s=0; \
bump() { s=$((s+1)); printf 'SEQ %s\\n' \"$s\"; }; \
while read line; do \
  printf '%s\\n' \"$line\"; \
  case \"$line\" in \
    *'MILESTONE_LABEL'*) \
      printf '%s\\n' 'MILESTONE_LABEL ECHO_SLICE'; \
      printf '%s\\n' 'MILESTONE_LABEL PROVISIONAL'; \
      printf '%s\\n' 'MILESTONE_LABEL NEXT_MILESTONE'; \
      printf '%s\\n' 'REFERENCE_LABEL NEXT_REFERENCE'; \
      bump; continue;; \
  esac; \
  case \"$line\" in *exactly:*|*Summarise*|*'STOP_QUESTION'*) ;; *) continue;; esac; \
  n=$((n+1)); \
  if [ $n -ge TURNS_BEFORE_DONE ]; then printf 'MILESTONE REACHED\\n'; \
  else printf 'ACK %s\\n' \"$n\"; fi; \
  bump; \
done"
        .replace("STOP_QUESTION", STOP_QUESTION)
        .replace("ECHO_SLICE", REFLECTION_ECHO_SLICE)
        .replace("PROVISIONAL", REFLECTION_PROVISIONAL)
        .replace("MILESTONE_LABEL", REFLECTION_MILESTONE_LABEL)
        .replace("REFERENCE_LABEL", REFLECTION_REFERENCE_LABEL)
        .replace("NEXT_MILESTONE", next_milestone)
        .replace("NEXT_REFERENCE", next_reference)
        .replace("TURNS_BEFORE_DONE", &prompts_before_done.to_string());
    let pane = {
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(script);
        command.env("TERM", "dumb");
        workspace
            .lock()
            .unwrap()
            .spawn(command, "sh".to_string(), columns, 16)
            .expect("spawn pane")
    };
    started(
        &WorkspacePaneAccess::new(Arc::clone(&workspace)),
        pane,
        "AGENT-READY",
    );
    (workspace, pane)
}

/// **A STAND-IN AGENT THAT SAYS THE WHOLE JOB IS FINISHED WHEN A REFLECTION ASKS** — the peer the
/// run's OTHER ending is measured against, and the first thing in this tree ever to say
/// `north_star_marker`.
///
/// # ⚠⚠⚠ What was untested until this existed
///
/// `ai_loop.scxml` calls that marker *"the ONE way a run reaches its closing report"*, and
/// [`OuterLoop::reflect`](crate::outer::OuterLoop) reads it before anything else — yet no fixture in
/// this crate had ever said it. Every `converged` run in the suite got there the OTHER way: a
/// reached milestone whose reflection proposed no successor, which is the livelock guard's exit and
/// **not a claim that the work is done**. So the product's own headline ending had no run behind it,
/// and register item 267 is what made that visible: two causes, one arrow, and only one of them
/// reachable by any gate.
///
/// # ⚠⚠ Why it does NOT stage the wrapped echo its siblings stage
///
/// [`standin_agent_reflecting`] paints a wrapped echo on purpose because
/// [`OuterLoop::proposed`](crate::outer::OuterLoop) has a rule that discounts one. `said_marker`'s
/// rules are different ones — a LINE rather than a row, standing alone, and not the tail of the
/// question broken — so an echo staged here would be testing that reader through a peer whose axis
/// is the reflection's ANSWER, and mixing the two is what this module keeps apart.
///
/// ⚠ **THE HAZARD ITSELF IS STAGED, ELSEWHERE AND ON PURPOSE** — register item 270, paid: the
/// prompt's last line ENDS with the marker, so a pane whose width breaks it exactly there puts the
/// marker alone on a row. `a_reflection_on_a_pane_that_breaks_the_north_star_line_does_not_close_
/// the_run` spawns that pane through [`standin_agent_reflecting_at`], and three gates in
/// `crate::outer` hold the same rules for `done_marker`. The gate that uses THIS peer still carries
/// its control run — the same prompt, the same echo, a peer that never says the marker — because a
/// control on the answer is worth having whatever the driver's rules are.
pub(crate) fn standin_agent_finishing(prompts_before_done: u32) -> (Arc<Mutex<Workspace>>, PaneId) {
    let workspace = Arc::new(Mutex::new(Workspace::new((STANDIN_COLUMNS, 16))));
    let script = format!(
        "stty -echo; printf 'AGENT-READY\\n'; n=0; s=0; \
         while read line; do \
           printf '%s\\n' \"$line\"; \
           case \"$line\" in \
             *'{REFLECT}'*) \
               printf 'there is nothing left to pick up\\n'; \
               printf '{NORTH_STAR}\\n'; \
               s=$((s+1)); printf '{SEQ} %s\\n' \"$s\"; continue;; \
           esac; \
           case \"$line\" in \
             *exactly:*|*Summarise*|*'{STOP}'*) ;; \
             *) continue;; \
           esac; \
           n=$((n+1)); \
           if [ $n -ge {prompts_before_done} ]; then printf 'MILESTONE REACHED\\n'; \
           else printf 'ACK %s\\n' \"$n\"; fi; \
           s=$((s+1)); printf '{SEQ} %s\\n' \"$s\"; \
         done",
        SEQ = SEQ_MARKER,
        REFLECT = REFLECTION_MILESTONE_LABEL,
        NORTH_STAR = NORTH_STAR_SAID,
        STOP = STOP_QUESTION,
    );
    let pane = {
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(script);
        command.env("TERM", "dumb");
        workspace
            .lock()
            .unwrap()
            .spawn(command, "sh".to_string(), STANDIN_COLUMNS, 16)
            .expect("spawn pane")
    };
    started(
        &WorkspacePaneAccess::new(Arc::clone(&workspace)),
        pane,
        "AGENT-READY",
    );
    (workspace, pane)
}

/// **A STAND-IN AGENT THAT ACTUALLY WRITES A REPORT WHEN IT IS ASKED TO CLOSE** — the peer the
/// captured closing report is measured against.
///
/// # ⚠⚠⚠ Why a peer of its own, and what each part of its answer is for
///
/// [`standin_agent`] answers the closing prompt with `ACK 3`, which is enough to end a turn and not
/// enough to test a READER. Every rule the capture applies was measured off a live `claude`
/// (`what_a_live_agents_report_looks_like_to_a_reader`), and a fixture that does not reproduce the
/// hazards proves nothing about any of them. So this peer's closing answer stages all three, and
/// each has a mutation behind it:
///
/// 1. **IT IS TALLER THAN THE PANE.** [`REPORT_LINES`] rows against sixteen, so the opening
///    SCROLLS. That is what separates the two readers: read through a [`RowTrail`] the account
///    comes back beginning in the middle, read through the line address it comes back whole. Without
///    it, both readers agree and the choice between them is untested — R385's rule, *make the two
///    behaviours disagree, then mutate*.
/// 2. **A WRAPPED ECHO** ([`REPORT_ECHO_SLICE`]) — a verbatim FRAGMENT of the closing prompt,
///    printed ahead of the report. The peer already echoes each prompt WHOLE, which the exact-match
///    half of any echo rule catches; this is the half that needs `asked.contains(line)`, and it is
///    what a real composer produces when it re-wraps a prompt to the pane's width.
///    ⚠⚠ **AHEAD OF THE REPORT AND NOT INSIDE IT, because that is where it was MEASURED** — the
///    live probe found the fragment at index 0 with the whole reply behind it. It was staged inside
///    the report first, on the invented ground that a re-wrap could put it anywhere, and the rule
///    that had to be written to satisfy the invention **deleted a line the agent had written**.
/// 3. **AN INTERIOR BLANK LINE**, with furniture on both ends ([`REPORT_RULE`]). The capture trims
///    edges and keeps the middle, so a rule that trimmed blanks everywhere would silently re-flow
///    somebody's report into one paragraph and nothing would say so.
///
/// ⚠ Its first and last report lines are [`REPORT_OPENS`] and [`REPORT_CLOSES`], so a gate can
/// assert the account's ENDS rather than its length — the two places a reader loses text.
///
/// # ⚠⚠⚠ ONE PEER FOR BOTH ENDINGS THAT ASK, and why it is a parameter rather than a second fixture
///
/// A run reaches an account two ways — it got there (`closing`), or it ran out of turns
/// (`stopping`) — and the three hazards above are properties of READING A PANE, identical on both
/// paths. Two peers would be two copies of the staging, and the day one grew a fourth hazard the
/// other ending's reader would silently stop being tested for it. So the hazards live here once and
/// [`Accounts`] says which question triggers them.
///
/// ⚠⚠ **THE ECHO FRAGMENT MOVES WITH THE QUESTION** ([`Accounts::echo_slice`]). It is a verbatim
/// slice of the prompt the peer is answering, and a peer that painted `end_prompt`'s fragment while
/// answering `stop_prompt` would stage no echo at all — the discount would have nothing to find, and
/// the gate would pass through a reader that had stopped discounting.
/// # ⚠⚠⚠ `thinks_for` — A SECOND AXIS, AND THE ONE HAZARD A FAST PEER CANNOT STAGE
///
/// Every stand-in here answers in microseconds, so a run's own wall clock always expires BETWEEN
/// two of the driver's steps. A real agent's turn is tens of seconds, so a real run's clock expires
/// **inside the wait for one** — and that is a different code path with a different answer:
/// `Completion::wait` reports `Over::RunEnded` and the loop has to decide what to tell its machine.
/// Measured with a fast peer at five different deadlines, the document was left in `working` or
/// `judging` every time and the case simply never arose.
///
/// So the peer can be told to take whole seconds over a WORKING prompt. ⚠ It answers the ACCOUNT's
/// question at once whatever this says: the hazard being staged is a clock that runs out mid-work,
/// and a peer that also dawdled over the account would make the gate a measurement of its own
/// `sleep` rather than of the run's window.
pub(crate) fn standin_agent_reporting(
    accounts: Accounts,
    thinks_for: Duration,
) -> (Arc<Mutex<Workspace>>, PaneId) {
    let workspace = Arc::new(Mutex::new(Workspace::new((STANDIN_COLUMNS, 16))));
    let mut report = vec![
        accounts.echo_slice().to_owned(),
        REPORT_RULE.to_owned(),
        REPORT_OPENS.to_owned(),
    ];
    // ⚠ Numbered filler rather than one repeated line: the emulator's line store is what is being
    // read, and identical rows cannot show which one was lost.
    for line in 1..=REPORT_LINES {
        report.push(format!("did {line}"));
    }
    report.push(String::new());
    report.push(REPORT_CLOSES.to_owned());
    report.push(REPORT_RULE.to_owned());
    let prints = report
        .iter()
        .map(|line| format!("printf '%s\\n' '{line}'; "))
        .collect::<String>();
    let script = "\
stty -echo; printf 'AGENT-READY\\n'; n=0; s=0; \
bump() { s=$((s+1)); printf 'SEQ %s\\n' \"$s\"; }; \
while read line; do \
  printf '%s\\n' \"$line\"; \
  case \"$line\" in \
    *'ACCOUNT_QUESTION'*) REPORT bump; continue;; \
  esac; \
  case \"$line\" in *exactly:*) ;; *) continue;; esac; \
  THINK n=$((n+1)); WORK_ANSWER bump; \
done"
        .replace("REPORT ", &prints)
        .replace("ACCOUNT_QUESTION", accounts.question())
        // ⚠ WHOLE SECONDS, because POSIX `sleep` takes an integer and a fixture that relied on
        // GNU's fractional one would be a gate that passes on this machine and not on a `dash`.
        // Nothing at all when the caller asked for nothing, so the fast peer stays exactly as fast.
        .replace(
            "THINK ",
            &match thinks_for.as_secs() {
                0 => String::new(),
                seconds => format!("sleep {seconds}; "),
            },
        )
        .replace("WORK_ANSWER ", accounts.work_answer());
    let pane = {
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(script);
        command.env("TERM", "dumb");
        workspace
            .lock()
            .unwrap()
            .spawn(command, "sh".to_string(), STANDIN_COLUMNS, 16)
            .expect("spawn pane")
    };
    started(
        &WorkspacePaneAccess::new(Arc::clone(&workspace)),
        pane,
        "AGENT-READY",
    );
    (workspace, pane)
}

/// **WHICH ENDING'S QUESTION [`standin_agent_reporting`] WRITES ITS ACCOUNT FOR.**
///
/// The two are the same act at opposite outcomes — a run says what it did — and they are separate
/// variants because the DOCUMENT asks them in different words, on different paths, and each word is
/// what the reader has to discount as this run's own echo.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Accounts {
    /// The peer reaches its milestone at once and tells the reflection there is nothing left, so the
    /// run reaches `closing` and is asked `end_prompt`.
    ForARunThatGotThere,
    /// The peer never says the marker, so the run spends every turn it was briefed with and is asked
    /// `stop_prompt` on its way to `exhausted`.
    ForARunThatRanOutOfTurns,
}

impl Accounts {
    /// **A VERBATIM SLICE OF THE PROMPT THIS PEER ANSWERS**, which is what it keys on.
    ///
    /// ⚠ Both are claims about `ai_loop.scxml`'s wording, held in step by the gates that assert the
    /// authored prompt carries them — the discipline every other fixture constant here follows.
    pub(crate) const fn question(self) -> &'static str {
        match self {
            Self::ForARunThatGotThere => "Summarise",
            Self::ForARunThatRanOutOfTurns => STOP_QUESTION,
        }
    }

    /// **A SECOND VERBATIM SLICE OF THE SAME PROMPT**, painted ahead of the account as the wrapped
    /// echo — see [`standin_agent_reporting`], and [`REPORT_ECHO_SLICE`] for what it stages.
    pub(crate) const fn echo_slice(self) -> &'static str {
        match self {
            Self::ForARunThatGotThere => REPORT_ECHO_SLICE,
            Self::ForARunThatRanOutOfTurns => STOP_ECHO_SLICE,
        }
    }

    /// What the peer answers a WORKING prompt with, which is what decides the run's ending: saying
    /// the marker converges it, and counting instead spends the budget.
    const fn work_answer(self) -> &'static str {
        match self {
            Self::ForARunThatGotThere => "printf 'MILESTONE REACHED\\n'; ",
            Self::ForARunThatRanOutOfTurns => "printf 'ACK %s\\n' \"$n\"; ",
        }
    }
}

/// **WHICH TURN [`standin_agent_asking`] RAISES ITS DIALOG IN.**
#[derive(Clone, Copy, Debug)]
pub(crate) enum Asks {
    /// The first prompt it recognises, whatever that prompt is — the shape every gate before
    /// `stopping` existed drove, and the one a working turn's dialog has.
    OnItsFirstPrompt,
    /// Only when asked where the run got to, answering every working prompt plainly — so the run
    /// spends its budget, reaches `stopping`, and its ACCOUNT is the turn that gets blocked.
    WhenTheRunStopsShort,
}

impl Asks {
    /// The `case` pattern that decides it: everything, or the stopping question alone.
    const fn pattern(self) -> &'static str {
        match self {
            Self::OnItsFirstPrompt => "*",
            Self::WhenTheRunStopsShort => "*'STOP_QUESTION'*",
        }
    }
}

/// **A VERBATIM SLICE OF `stop_prompt`** — the question `ai_loop.scxml` asks a run that is ending
/// without having got there, as every peer in this file keys its answer on it.
///
/// ⚠⚠⚠ A STAND-IN MUST ANSWER WHATEVER IT IS ASKED, and this constant is what four red gates bought.
/// When `stopping` was built, every peer here ignored its question: the turn never ended, the loop
/// walked `Stopping --Null--> Stopping` to its wall clock, and four gates about the DOCUMENT's turn
/// budget came back `exhausted — duration`. ⚠ It is a claim about the document's wording, and
/// `the_whole_authored_surface_crosses_into_the_datamodel` is what holds the two in step.
pub(crate) const STOP_QUESTION: &str = "where you got to";

/// [`STOP_QUESTION`]'s prompt, sliced a second time — the wrapped echo the reporting peer paints
/// ahead of an account it wrote for `stopping`. See [`REPORT_ECHO_SLICE`], which is this for
/// `closing`.
pub(crate) const STOP_ECHO_SLICE: &str = "what you left half-done";

/// **A VERBATIM SLICE OF THE CLAUSE `ai_loop.scxml` COMPOSES INTO `stop_prompt`** for the ceiling
/// that ended the run — one per [`Ceiling`], and the needle register item 264 is measured with.
///
/// # ⚠⚠⚠ Why a slice per ceiling rather than one needle
///
/// `stopping` is reached by FOUR ceilings and used to ask ONE authored sentence: *"This run has
/// spent its whole turn budget"*. For three of them that is false, and it is not a journal line
/// somebody can weigh — it is typed into a live agent's pane in the one turn that asks that agent
/// what a run picking this up should do first. A gate that only asserted *some* clause is there
/// would pass a document that told every run the same lie, so the gate asserts the ceiling's OWN
/// clause is present AND that no other ceiling's is.
///
/// ⚠⚠ THE SLICES MUST THEREFORE BE MUTUALLY EXCLUSIVE — no one of them a substring of another
/// ceiling's clause — or the second half of that assertion tests nothing. `the_question_a_stopped_run_
/// is_asked_names_the_ceiling_that_stopped_it` checks it of the four before it uses them.
///
/// ⚠ Claims about the document's wording, exactly as [`STOP_QUESTION`] is, and held in step by the
/// same discipline: edited apart from `ai_loop.scxml`, the gate that reads them goes red rather than
/// quietly stopping being about anything.
pub(crate) const fn stop_said(ceiling: Ceiling) -> &'static str {
    match ceiling {
        Ceiling::Turns => "every turn its document budgeted",
        Ceiling::Iterations => "every step its run was allowed",
        Ceiling::Cost => "allowed to spend",
        Ceiling::Duration => "wall-clock time",
    }
}

/// How many filler lines the reporting peer's account carries between its ends.
///
/// ⚠ It has to exceed the pane's sixteen rows with the report's own frame on top, or the account
/// does not scroll and the gate is not measuring a reader at all. The gate asserts the scroll
/// happened rather than trusting this number.
pub(crate) const REPORT_LINES: usize = 24;
/// The first line of the reporting peer's account — the one a rendering reader loses.
pub(crate) const REPORT_OPENS: &str = "what changed: the file was created";
/// The last line of its account.
pub(crate) const REPORT_CLOSES: &str = "what is left: nothing";
/// Furniture the peer wraps its account in, carrying no word a reader wants — the box rule a real
/// agent CLI draws, measured at both ends of a live reply.
pub(crate) const REPORT_RULE: &str = "--------------------------------";
/// **A VERBATIM SLICE OF `end_prompt`**, printed ahead of the report — the wrapped echo, staged
/// where a live composer was measured putting it. ⚠ It is a claim about `ai_loop.scxml`'s wording,
/// and the gate asserts the account does not carry it. Edited apart, this stops being an echo and
/// the rule it tests stops being tested.
pub(crate) const REPORT_ECHO_SLICE: &str = "what was verified";

/// The label `ai_loop.scxml` authors for the milestone half of a reflection's answer, as the peer
/// above obeys it. ⚠ The document is the authority; this is a fixture's copy, held in step by the
/// gate that asserts the composed prompt carries it.
pub(crate) const REFLECTION_MILESTONE_LABEL: &str = "NEXT MILESTONE:";

/// **WHAT AN AGENT SAYS WHEN THE WHOLE JOB IS FINISHED** — `ai_loop.scxml`'s `north_star_marker`, as
/// [`standin_agent_finishing`] obeys it.
///
/// ⚠ The document is the authority and this is a fixture's copy, held in step exactly as
/// `MILESTONE REACHED` is: a peer cannot read a datamodel that has not been initialised, and what
/// keeps the two from drifting is that a run spelling it wrong never converges — loudly, in the one
/// gate that asks for this ending.
pub(crate) const NORTH_STAR_SAID: &str = "NORTH STAR REACHED";
/// See [`REFLECTION_MILESTONE_LABEL`].
pub(crate) const REFLECTION_REFERENCE_LABEL: &str = "NEXT REFERENCE:";

/// **A VERBATIM SLICE OF THE PROMPT**, which the peer above paints behind the label — the shape a
/// wrapped echo has, staged rather than hoped for. See [`standin_agent_reflecting`].
///
/// ⚠ It is a claim about `ai_loop.scxml`'s wording and the gate asserts the composed prompt still
/// contains it. Edited apart, this stops being an echo and the rule it tests stops being tested.
pub(crate) const REFLECTION_ECHO_SLICE: &str =
    "and then the next checkpoint in one line. Open the second with the label";
/// What the peer says behind the label before it settles on its real answer — see
/// [`standin_agent_reflecting`]. ⚠ It must NOT appear in the prompt, or the echo rule would reject
/// it and the last-match rule would go untested again.
pub(crate) const REFLECTION_PROVISIONAL: &str = "a checkpoint it thought better of";

// ── ⚠⚠⚠ THREE DIALOGS CAPTURED FROM A LIVE `claude` 2.1.232 **WHILE IT WAS WORKING** (R383's
//    probes), by `sprag_host::live_agent::what_a_live_agent_asks_while_it_works`. Not composed.
//
//    `sprag-detect` holds six captured dialogs from two real agents and asserts the parse of every
//    one. Five of those six are shown BEFORE an agent works — trust prompts, model pickers, a
//    sign-in — and exactly one is a tool permission (`Fetch`). **An outer loop only ever meets the
//    second kind**, so every consent needle in this tree was read off that single screen. These
//    three are the write, edit and shell families, measured.
//
//    ⚠⚠ THEY LIVE HERE AND NOT BESIDE THE OTHER SIX, and the reason is the claim they exist for:
//    it is about `Consents::covers`, which `sprag-detect` cannot see (it is the crate BELOW this
//    one). Keeping one copy where both claims can be made beat keeping the family together with a
//    second copy of the data — see `consent`'s gate. Registered as the wart it is.

/// The frame rule an agent draws across its dialog, at the 120 columns these were captured in.
///
/// ⚠ A `const` rather than 120 characters typed into six places: it is the ONE part of a capture
/// that is a property of the PANE's width rather than of what the agent said, and spelling it out
/// six times would invite somebody to shorten one and call the fixture still-captured.
const RULE: &str = "────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────";
/// The dotted rule an agent draws around a file preview, at the same width.
const DOTS: &str = "╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌";

/// The `Write` tool's permission dialog.
pub(crate) const CLAUDE_WRITE_DIALOG: &[&str] = &[
    "● Write(PROBE.txt)",
    RULE,
    " Create file",
    " PROBE.txt",
    DOTS,
    "  1 ready",
    DOTS,
    " Do you want to create PROBE.txt?",
    " ❯ 1. Yes",
    "   2. Yes, allow all edits during this session (shift+tab)",
    "   3. No",
    " Esc to cancel · Tab to amend",
];

/// The `Edit` tool's permission dialog — a DIFFERENT sentence and the same option set.
pub(crate) const CLAUDE_EDIT_DIALOG: &[&str] = &[
    "● Update(SEED.txt)",
    RULE,
    " Edit file",
    " SEED.txt",
    DOTS,
    " 1 -ready",
    " 1 +steady",
    DOTS,
    " Do you want to make this edit to SEED.txt?",
    " ❯ 1. Yes",
    "   2. Yes, allow all edits during this session (shift+tab)",
    "   3. No",
    " Esc to cancel · Tab to amend",
];

/// A SHELL command's permission dialog — the shortest question of the three, and the one whose
/// second option is worded differently again.
///
/// ⚠⚠ ITS SECOND OPTION NAMES THE WORKING DIRECTORY, which for the probe that took this was a
/// scratch path carrying a pid and a nanosecond count. **Kept verbatim** rather than generalised:
/// the point of a captured fixture is that nobody edited it, and a reader who cannot tell which
/// parts were touched cannot trust any of it. What the volatile name proves is its own small
/// thing — that this option's text is BUILT from the session, so a consent needle aimed at it
/// would be aimed at a string that differs every run.
pub(crate) const CLAUDE_BASH_DIALOG: &[&str] = &[
    "● I'll run that now.",
    "● Bash(touch MADE-BY-BASH.txt && ls -la MADE-BY-BASH.txt)",
    "  ⎿ \u{a0}Waiting…",
    RULE,
    " Bash command",
    "   touch MADE-BY-BASH.txt && ls -la MADE-BY-BASH.txt",
    "   Create MADE-BY-BASH.txt",
    " Do you want to proceed?",
    " ❯ 1. Yes",
    "   2. Yes, and always allow access to sprag-live-asks-bash-writes-3241854-485793172/ from this project",
    "   3. No",
    " Esc to cancel · Tab to amend · ctrl+e to explain",
];

/// Every working-agent dialog this crate has captured, with the label the probe that took it used.
pub(crate) const CLAUDE_WORKING_DIALOGS: &[(&str, &[&str])] = &[
    ("write", CLAUDE_WRITE_DIALOG),
    ("edit", CLAUDE_EDIT_DIALOG),
    ("bash", CLAUDE_BASH_DIALOG),
];

/// Replay a captured screen through the SHIPPING parser and hand back what it read.
///
/// ⚠ Through a real [`Emulator`](sprag_vt::Emulator) rather than by handing the parser a
/// hand-built screen: `sprag_detect::question` reads `row_text`, and how a row BECOMES text — the
/// wrapping, the trailing blanks, the wide glyphs in these captures — is the emulator's answer and
/// not a test's.
pub(crate) fn parsed_dialog(rows: &[&str]) -> Option<sprag_detect::Question> {
    use sprag_vt::VtPort;
    let mut emulator = sprag_vt::Emulator::new(120, 40);
    emulator.advance(rows.join("\r\n").as_bytes());
    sprag_detect::question(emulator.screen(), sprag_detect::DIALOG_WINDOW)
}

/// A stand-in AGENT CLI **THAT STOPS TO ASK PERMISSION**, exactly once, on its first turn.
///
/// # ⚠⚠⚠ Why the outer loop has never met one of these
///
/// Every live measurement of the loop picked an ARITHMETIC milestone, deliberately, *"so no
/// permission dialog can fire"* — because a dialog sends the machine to `screening`, which nothing
/// drives. That choice is registered debt (112) and the model itself wrote it back in its own
/// closing report: *"the next step is a milestone with real work in it — something requiring tool
/// use."* **Every kind of work a real loop does raises one of these**, and until this fixture the
/// path was measured by nothing.
///
/// ⚠⚠⚠ **IT READS THE DIALOG AS A KEYPRESS AND THE PROMPT AS A LINE, AND THE FIRST RUN OF THIS
/// FIXTURE IS WHY.** The first draft read everything with `read line`, which looks harmless and
/// silently tests the wrong product: what a run sends a menu whose marker is ALREADY on the
/// authorised option is a bare **Enter** ([`Taken::Selected`](crate::consent::Taken)), and — if
/// that is ignored — the digit **alone**, with no newline behind it. A line-reading peer never sees
/// either, so the run typed both keys, watched the menu sit there and reported `not_taken`: a
/// perfect measurement of a peer no agent CLI resembles. A real one is in raw mode and acts on the
/// key. So `icanon` is turned OFF for exactly as long as the menu is up.
///
/// ⚠⚠ **IT CLEARS THE SCREEN WHEN THE DIALOG IS ANSWERED**, which is not decoration: the shipping
/// parser reads a menu off the pane's bottom window, so a menu left painted keeps the pane
/// `Blocked` for ever and the answer would look like it had not been taken. A real agent CLI
/// repaints the same way.
///
/// ⚠ After the dialog it answers one more prompt with the marker, so a run that gets PAST the
/// question can still converge — which is what makes *"it stopped"* and *"it went on"* two
/// distinguishable endings rather than one.
///
/// ⚠⚠ **AND IT KEYS ON `Summarise` AS WELL AS ON `exactly:`**, [`standin_agent`]'s rule and for its
/// reason: `closing` sends the END prompt, which carries neither the done instruction nor anything
/// else this peer would recognise. A peer that ignores it never publishes another change, the
/// turn never ends, and a run that had already reached its milestone burns its whole wall clock in
/// `closing` — measured here, as `exhausted — duration` on a run that had converged in every sense
/// but the last one.
///
/// # ⚠⚠⚠ [`Asks`] — WHICH turn the question lands in, and why that is a parameter
///
/// A dialog raised in a WORKING turn is a run that can still be helped: `screening` looks for a
/// rule, and failing that a person is woken. A dialog raised in the turn that asks for the run's
/// ACCOUNT is a different situation entirely — the ending is already decided, so there is nothing
/// to unblock and nobody is woken, and what is left behind is **a question on somebody's pane that
/// outlives the run**. The peer is the same program; only the moment differs, which is exactly what
/// makes it a parameter rather than a second fixture.
pub(crate) fn standin_agent_asking(asks: Asks) -> (Arc<Mutex<Workspace>>, PaneId) {
    let workspace = Arc::new(Mutex::new(Workspace::new((STANDIN_COLUMNS, 16))));
    let script = "\
stty -echo; printf 'AGENT-READY\\n'; n=0; asked=0; s=0; k=''; \
readbyte() { dd bs=1 count=1 2>/dev/null | od -An -tu1 | tr -d ' \\n'; }; \
bump() { s=$((s+1)); printf 'SEQ %s\\n' \"$s\"; }; \
while read line; do \
  printf '%s\\n' \"$line\"; \
  case \"$line\" in *exactly:*|*Summarise*|*'STOP_QUESTION'*) ;; *) continue;; esac; \
  case \"$line\" in ASKS_AT) ;; *) n=$((n+1)); printf 'ACK %s\\n' \"$n\"; bump; continue;; esac; \
  if [ $asked -eq 0 ]; then \
    asked=1; \
    printf 'Bash command\\n'; \
    printf 'Do you want to proceed?\\n'; \
    printf '\\342\\235\\257 1. Yes\\n'; \
    printf '  2. Yes, and do not ask again\\n'; \
    printf '  3. No, and tell me what to do\\n'; \
    stty -icanon; \
    while :; do \
      k=$(readbyte); \
      [ -n \"$k\" ] || exit 0; \
      case \"$k\" in 49|50|51|10|13) break;; esac; \
    done; \
    stty icanon; \
    printf '\\033[2J\\033[H'; n=$((n+1)); printf 'ACK took %s\\n' \"$k\"; bump; \
    continue; \
  fi; \
  n=$((n+1)); printf 'MILESTONE REACHED\\n'; bump; \
done"
        // ⚠ THE PATTERN FIRST: it CONTAINS the placeholder for one of the two variants, so
        // substituting the question first would leave the pattern's own copy unreplaced.
        .replace("ASKS_AT", asks.pattern())
        .replace("STOP_QUESTION", STOP_QUESTION);
    let pane = {
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(script);
        command.env("TERM", "dumb");
        workspace
            .lock()
            .unwrap()
            .spawn(command, "sh".to_string(), STANDIN_COLUMNS, 16)
            .expect("spawn pane")
    };
    started(
        &WorkspacePaneAccess::new(Arc::clone(&workspace)),
        pane,
        "AGENT-READY",
    );
    (workspace, pane)
}

/// A stand-in AGENT CLI **THAT ASKS SOMETHING NO CONSENT CAN TAKE, AND TAKES THE REFUSING KEY** —
/// the peer `screening` exists for.
///
/// # ⚠⚠⚠ How it differs from [`standin_agent_asking`], and why each difference is a product claim
///
/// That one raises a dialog whose menu carries the answer a caller would write (`Yes`), so a consent
/// gets the run past it. This one is the case a consent **cannot** reach, which is the whole reason
/// the loop document has a `screening` state:
///
/// * **It reads [`REFUSES`](crate::screen::REFUSES) as a keypress**, in raw mode, exactly as
///   [`standin_agent_asking`] reads a digit — and it is the ONLY key it acts on, so a run that
///   pressed anything else sits there and the gate says so rather than passing.
/// * ⚠⚠⚠ **AND ITS DIALOG STAYS UP UNTIL THAT KEY ARRIVES.** The fidelity claim R384 measured is
///   that a live agent's dialog is dismissed by this key in ~25 ms and by `Tab` not at all — so a
///   fixture that cleared its menu on any input would make the product's *"prove the question is
///   gone"* step pass for free, which is precisely the step whose absence approved a file write.
/// * **What it does after the refusal is what a real agent did**: it CLEARS the menu, reports the
///   call turned down, and comes back for the next prompt. The redirect then arrives as ordinary
///   text.
///
/// ⚠ It ACKNOWLEDGES the redirect — which ENDS that turn — and then says the done marker once it has
/// been prompted `turns_after_redirect` more times. At 1 that makes *"the standing instruction
/// worked"* an ending rather than a note: a run that gets past the dialog CONVERGES and one that does
/// not cannot.
///
/// ⚠⚠⚠ **AND EVERY REPLY BUMPS ITS PUBLISHED COUNTER, WHICH IS NOT DECORATION — R375's TRAP, MET
/// TWICE.** [`supervised_asking`] derives `seq` from [`peer_seq`] and latches it monotone, because
/// this peer does something worse than scroll: it CLEARS, so for an instant the screen carries no
/// counter at all. The first draft printed a refusal line that published nothing and the count was
/// pinned at its old high for the rest of the run — measured, as a run whose screening plainly
/// worked (`Working --TurnBlocked--> Screening`, the refusal in its journal) and then sat in
/// `Working --Null--> Working` until its wall clock. **A fixture's counting has to survive the
/// fixture's own repaint** — and, as `peer_seq` records, its own SCROLL.
/// ⚠⚠⚠ **`takes_the_key` IS THE HAZARD, PARAMETERISED.** `false` builds the peer the live probe's
/// `Tab` arm measured: a dialog that stays up whatever arrives. What the product must do there is
/// **type nothing at all**, and a fixture that could not be that peer would leave the one assertion
/// that matters — *the redirect never reached the pane* — with nothing to make it fail.
///
/// ⚠⚠⚠ **AND `turns_after_redirect` IS THE SECOND HAZARD, PARAMETERISED FOR THE SAME REASON.** One
/// is the shape that CONVERGES on the redirect, which is what *"the standing instruction worked"*
/// needs. A LARGE one is the shape a live agent actually took: it does what it was redirected to,
/// comes back, and is asked for the original milestone again — R384's live agent wrote that out in
/// words (*"루프가 매 턴 같은 요청을 반복하고 저는 매 턴 같은 이유로 거절 … 진전이 없습니다"*), and
/// with a peer that converges immediately nothing can count it.
/// ⚠⚠⚠ **AND `asks_on_its_second_life` IS THE THIRD, which is the only way to be a peer a REPLACEMENT
/// session cannot be driven through.**
///
/// A loop's `resuming` waits for the pane its `restarting` opened, and the barrier there can answer
/// something other than *ready*: a fresh agent CLI showing a trust prompt is the real case, and it is
/// the one a person meeting this feature for the first time is likeliest to hit. That path ends the run
/// with a sentence naming what the replacement came up asking.
///
/// A peer cannot know which of its lives it is in — a replacement runs the SAME argv, deliberately —
/// so this one asks the FILESYSTEM. Given a path, the first life creates it and behaves normally; any
/// later life finds it and comes up showing a menu instead of announcing itself. **The world changed,
/// not the command**, which is exactly what distinguishes a second session from a first.
///
/// ⚠⚠ **AND IT PRINTS SIX LINES BEFORE THE MENU**, which the first run of that gate is why:
/// `sprag_detect::question` reads the pane's BOTTOM [`DIALOG_WINDOW`](sprag_detect::DIALOG_WINDOW)
/// rows — twelve — and a menu at the top of an otherwise-blank sixteen-row screen sits ABOVE them, so
/// the barrier saw nothing and the run waited out its whole clock. ⚠ It is a fixture artifact and not
/// a product defect: a real agent CLI paints a full-screen TUI, so its dialog is never the only thing
/// on the screen. Said out loud because a reader meeting the filler lines would otherwise delete them.
pub(crate) fn standin_agent_refusing(
    takes_the_key: bool,
    turns_after_redirect: u32,
    asks_on_its_second_life: Option<&std::path::Path>,
) -> (Arc<Mutex<Workspace>>, PaneId) {
    let workspace = Arc::new(Mutex::new(Workspace::new((STANDIN_COLUMNS, 16))));
    // ⚠ `27` is Escape's byte, and `0` is a byte no key sends — so the un-dismissable peer is the
    // SAME program waiting for something that never arrives, rather than a different fixture whose
    // difference a reader has to take on trust.
    let breaks_on = if takes_the_key { "27" } else { "0" };
    // ⚠ EMPTY means *there is no second life to behave differently in*, so the guard below is false
    // for every existing caller and the peer is exactly what it was.
    let once = asks_on_its_second_life
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    let script = "\
LIVES='ONCE_MARKER'; \
if [ -n \"$LIVES\" ] && [ -e \"$LIVES\" ]; then \
  stty -echo; \
  i=0; while [ $i -lt 6 ]; do printf 'starting up\\n'; i=$((i+1)); done; \
  printf 'Choose an approach\\n'; \
  printf 'Which way should I build this?\\n'; \
  printf '\\342\\235\\257 1. The quick one\\n'; \
  printf '  2. The thorough one\\n'; \
  exec cat; \
fi; \
[ -z \"$LIVES\" ] || : > \"$LIVES\"; \
stty -echo; printf 'AGENT-READY\\n'; asked=0; n=0; s=0; k=''; \
readbyte() { dd bs=1 count=1 2>/dev/null | od -An -tu1 | tr -d ' \\n'; }; \
bump() { s=$((s+1)); printf 'SEQ %s\\n' \"$s\"; }; \
while read line; do \
  printf '%s\\n' \"$line\"; \
  case \"$line\" in \
    *'REFLECT_LABEL'*) printf 'ACK nothing to change\\n'; bump; continue;; \
  esac; \
  if [ $asked -eq 1 ]; then \
    asked=2; printf 'ACK took the redirect\\n'; bump; continue; \
  fi; \
  case \"$line\" in *exactly:*|*Summarise*|*'STOP_QUESTION'*) ;; *) continue;; esac; \
  if [ $asked -eq 0 ]; then \
    printf 'Choose an approach\\n'; \
    printf 'Which way should I build this?\\n'; \
    printf '\\342\\235\\257 1. The quick one\\n'; \
    printf '  2. The thorough one\\n'; \
    stty -icanon; \
    while :; do \
      k=$(readbyte); \
      [ -n \"$k\" ] || exit 0; \
      case \"$k\" in BREAKS_ON) break;; esac; \
    done; \
    stty icanon; \
    asked=1; \
    printf '\\033[2J\\033[H'; printf 'ACK rejected the choice\\n'; bump; \
    continue; \
  fi; \
  n=$((n+1)); \
  if [ $n -ge TURNS_AFTER ]; then printf 'MILESTONE REACHED\\n'; \
  else printf 'ACK %s\\n' \"$n\"; fi; \
  bump; \
done"
        .replace("BREAKS_ON", breaks_on)
        .replace("STOP_QUESTION", STOP_QUESTION)
        .replace("TURNS_AFTER", &turns_after_redirect.to_string())
        // ⚠ A REFLECTION IS A TURN AND A PEER MUST ANSWER IT — see [`standin_agent`], whose doc
        // carries the four gates that measured what a silent one costs. This one names nothing
        // either: what it is a stand-in FOR is a dialog and a redirect.
        .replace("REFLECT_LABEL", REFLECTION_MILESTONE_LABEL)
        .replace("ONCE_MARKER", &once);
    let pane = {
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(script);
        command.env("TERM", "dumb");
        // ⚠⚠⚠ A WORKING DIRECTORY THAT IS NOT THE TEST PROCESS'S, and it is load-bearing rather than
        // tidy. This peer is the one a session REPLACEMENT is measured on, and the claim there is
        // that the fresh pane runs in the SAME directory — which `PaneLifecycle::respawn` reads off
        // the pane it is replacing. Spawned in the runner's own directory, that claim is satisfied by
        // a `respawn` that passes NO directory at all, because an unset cwd is inherited and the two
        // answers are the same string. **Measured: the mutation that dropped the cwd left the gate
        // green.** R349's rule — a fixture that fits proves nothing about an anchor; make the two
        // behaviours disagree.
        //
        // ⚠ `/` because it exists on every host this suite runs on and is never a build directory.
        command.cwd("/");
        workspace
            .lock()
            .unwrap()
            .spawn(command, "sh".to_string(), STANDIN_COLUMNS, 16)
            .expect("spawn pane")
    };
    started(
        &WorkspacePaneAccess::new(Arc::clone(&workspace)),
        pane,
        "AGENT-READY",
    );
    (workspace, pane)
}

/// The supervision a real host would provide for [`standin_agent_asking`] — **the shipping dialog
/// parser for what it is asking, and the peer's own output for how far it has got**.
///
/// # ⚠⚠⚠ Why neither half alone would do
///
/// [`supervised`] answers `Idle` always and derives `seq` from the screen, which is honest for a
/// peer that never blocks and useless here: a run has to be able to SEE the question. The dialog
/// fixtures' supervisor ([`asking_peer`]) reads the question with the real parser and pins `seq` to
/// 1, which is honest for a one-shot answer and useless here: `DoneWhen::Settles` compares `seq`
/// against the turn's arming, so a frozen one means no turn ever ends.
///
/// A loop needs both at once, so this is both — and the `asking` half is the SHIPPING parser
/// (`sprag_detect::question`) rather than a hand-written menu reader, so a gate cannot pass against
/// a question the product would not have parsed.
///
/// ⚠⚠ **IT SETTLES**, for [`asking_peer`]'s measured reason: a real supervisor publishes a resting
/// verdict only once a candidate has held for its window, so a pane whose dialog was just answered
/// goes on reading `Blocked` for that long. A source with no lag hid a live defect from every gate
/// in this file until an end-to-end run met it.
pub(crate) fn supervised_asking(workspace: &Arc<Mutex<Workspace>>) -> WorkspacePaneAccess {
    /// Far shorter than `sprag_detect::DEFAULT_SETTLE`: what is under test is that the product
    /// tolerates a lag AT ALL, not any particular length of one.
    const FIXTURE_SETTLE: Duration = Duration::from_millis(300);
    let source = {
        let workspace = Arc::clone(workspace);
        let high: SeqHighWater = Arc::default();
        let last_menu: Mutex<Option<std::time::Instant>> = Mutex::new(None);
        Arc::new(move |id: PaneId| {
            let rows = WorkspacePaneAccess::new(Arc::clone(&workspace))
                .pane_full_lines(id)
                .unwrap_or_default();
            // ⚠⚠ THE PEER'S OWN COUNTER, not a count of its words, latched per PANE — see
            // [`peer_seq`] for the two walks that measured both halves of that.
            let seq = latched(&high, id, &rows);
            let guard = workspace.lock().expect("the workspace mutex");
            guard.pane(id)?.pty().with_screen(|screen| {
                let asking = sprag_detect::question(screen, sprag_detect::DIALOG_WINDOW);
                let mut seen = last_menu.lock().expect("the settle mutex");
                if asking.is_some() {
                    *seen = Some(std::time::Instant::now());
                }
                let settling = seen.is_some_and(|at| at.elapsed() < FIXTURE_SETTLE);
                Some(AgentObservation {
                    state: if asking.is_some() || settling {
                        AgentState::Blocked
                    } else {
                        AgentState::Idle
                    },
                    // ⚠⚠⚠ ONLY ONCE THE PANE HAS PAINTED — see [`has_painted`]. A blank pane naming
                    // an agent is a barrier that comes down before the program exists.
                    agent: has_painted(&rows).then(|| "claude".to_string()),
                    authority: Authority::Scraped {
                        rule: Some("dialog-choice-list".to_string()),
                    },
                    seq,
                    asking,
                })
            })
        }) as AgentStateSource
    };
    WorkspacePaneAccess::new(Arc::clone(workspace)).with_agent_state(Some(source))
}

/// The supervision a real host would provide for [`standin_agent`], derived from the peer's OWN
/// output.
///
/// # ⚠⚠⚠ Why `seq` carries the whole signal and the STATE is always at rest
///
/// A shell peer with echo off paints nothing between reading a prompt and answering it, so
/// there is no moment at which a screen-derived detector could honestly call it *working* —
/// and a fixture that claimed otherwise would be inventing evidence the pane does not carry.
///
/// What it can say truthfully is HOW MANY answers the peer has produced, which is exactly what
/// [`AgentObservation::seq`](crate::access::AgentObservation::seq) means: published changes.
/// So this reports `Idle` always and lets the count do the work — **which puts the whole weight
/// on [`DoneWhen::Settles`](crate::completion::DoneWhen)'s arming**, the discipline that stops a
/// peer's rest from BEFORE a turn reading as its answer. A driver that dropped the arming would
/// end every turn instantly against this fixture, and the gate would say so.
///
/// ⚠⚠⚠ **AND IT IS THE PEER'S OWN COUNTER, WHICH THE THIRD STALL OF THAT GATE PAID FOR** — see
/// [`peer_seq`], which carries the walk. Counting the peer's WORDS on the collapsed screen was a
/// claim about the terminal's SIZE as much as about the peer, and holding that count monotone (which
/// this fixture did, for two rounds, with a doc explaining why) stops it shrinking without making it
/// GROW: past the first scroll the high-water mark was final and **no further turn could end**.
/// R375 recorded the shrinking half of this trap; the half that cost a round is the ceiling.
pub(crate) fn supervised(workspace: &Arc<Mutex<Workspace>>) -> WorkspacePaneAccess {
    let source = {
        let workspace = Arc::clone(workspace);
        let high: SeqHighWater = Arc::default();
        Arc::new(move |id: PaneId| {
            let rows = WorkspacePaneAccess::new(Arc::clone(&workspace))
                .pane_full_lines(id)
                .unwrap_or_default();
            let seq = latched(&high, id, &rows);
            Some(crate::access::AgentObservation {
                state: AgentState::Idle,
                // ⚠⚠ ONLY ONCE THE PANE HAS PAINTED — see [`has_painted`], and the same reason as its
                // sibling above: `Settles` names an agent, and a blank pane must not.
                agent: has_painted(&rows).then(|| "claude".to_string()),
                authority: crate::access::Authority::Reported {
                    source: "test".to_string(),
                },
                seq,
                asking: None,
            })
        })
    };
    WorkspacePaneAccess::new(Arc::clone(workspace)).with_agent_state(Some(source))
}

#[cfg(test)]
mod tests {
    use super::refused_naming;
    use crate::access::{JobLeader, PaneDoing, PaneError};
    use crate::readiness::ReadyWhen;

    /// ⚠⚠ **THE HELPER'S OWN REFUSALS FIRE** — the two paths that were registered as *"built by
    /// nothing"* rather than written, which took a `#[should_panic]` each.
    ///
    /// It matters because this helper is what NINE gates decide a readiness refusal by. A shape
    /// check that silently accepted the wrong shape would make all nine pass over a barrier that
    /// refused for some other reason entirely, or over one that blamed nobody — and a gate that
    /// cannot fail is worse than no gate, because its green is read as evidence.
    #[test]
    #[should_panic(expected = "did not refuse for a readiness it never reached")]
    fn a_failure_that_is_not_a_readiness_refusal_is_rejected() {
        refused_naming(
            Some(&PaneError::Write("broken pipe".to_string())),
            &ReadyWhen::Runs("claude".to_string()),
            "claude",
            "a write failure is not a barrier giving up",
        );
    }

    /// The other half: a refusal that names no program at all. `PaneDoing::Unknown` is the honest
    /// answer from a host with no process table — and a gate asserting *the refusal names what the
    /// caller launched* must not read it as a pass.
    #[test]
    #[should_panic(expected = "nothing was reported as owning the pane's terminal")]
    fn a_refusal_that_blames_nobody_is_rejected() {
        refused_naming(
            Some(&PaneError::NeverReady {
                wanted: ReadyWhen::Runs("claude".to_string()),
                instead: PaneDoing::Unknown,
                already_showing: false,
            }),
            &ReadyWhen::Runs("claude".to_string()),
            "claude",
            "a host that cannot see the process table blames nobody",
        );
    }

    /// ⚠ AND THE CONTROL: the shape it is FOR passes. Without this the two above are satisfied by a
    /// helper that panics unconditionally.
    #[test]
    fn the_shape_it_exists_for_passes() {
        refused_naming(
            Some(&PaneError::NeverReady {
                wanted: ReadyWhen::Runs("claude".to_string()),
                instead: PaneDoing::Job(JobLeader::known_as("sh".to_string())),
                already_showing: false,
            }),
            &ReadyWhen::Runs("claude".to_string()),
            "sh",
            "a barrier that gave up on a pane still running its shell",
        );
    }
}
