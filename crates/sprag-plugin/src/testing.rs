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
/// ⚠⚠⚠ **IT PAINTS WHAT IT READS, AND THE SECOND RUN OF THAT GATE IS WHY.** With echo off and
/// nothing painted, [`deliver`](crate::deliver::deliver) can never confirm the prompt arrived, so
/// it RETYPES it — and a peer counting prompts saw two where the driver sent one, converging a
/// turn early. A real agent CLI paints the prompt into its own box, which is the whole reason
/// `deliver` reads the screen back; a stand-in that stayed silent was testing the retry path, not
/// the loop.
pub(crate) fn standin_agent(prompts_before_done: u32) -> (Arc<Mutex<Workspace>>, PaneId) {
    let workspace = Arc::new(Mutex::new(Workspace::new((80, 16))));
    let script = format!(
        "stty -echo; printf 'AGENT-READY\\n'; n=0; \
         while read line; do \
           printf '%s\\n' \"$line\"; \
           case \"$line\" in \
             *exactly:*|*Summarise*) ;; \
             *) continue;; \
           esac; \
           n=$((n+1)); \
           if [ $n -ge {prompts_before_done} ]; then printf 'MILESTONE REACHED\\n'; \
           else printf 'ACK %s\\n' \"$n\"; fi; \
         done"
    );
    let pane = {
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(script);
        command.env("TERM", "dumb");
        workspace
            .lock()
            .unwrap()
            .spawn(command, "sh".to_string(), 80, 16)
            .expect("spawn pane")
    };
    started(
        &WorkspacePaneAccess::new(Arc::clone(&workspace)),
        pane,
        "AGENT-READY",
    );
    (workspace, pane)
}

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
pub(crate) fn standin_agent_asking() -> (Arc<Mutex<Workspace>>, PaneId) {
    let workspace = Arc::new(Mutex::new(Workspace::new((80, 16))));
    let script = "\
stty -echo; printf 'AGENT-READY\\n'; n=0; asked=0; k=''; \
readbyte() { dd bs=1 count=1 2>/dev/null | od -An -tu1 | tr -d ' \\n'; }; \
while read line; do \
  printf '%s\\n' \"$line\"; \
  case \"$line\" in *exactly:*|*Summarise*) ;; *) continue;; esac; \
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
    printf '\\033[2J\\033[H'; n=$((n+1)); printf 'ACK took %s\\n' \"$k\"; \
    continue; \
  fi; \
  n=$((n+1)); printf 'MILESTONE REACHED\\n'; \
done"
        .to_string();
    let pane = {
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(script);
        command.env("TERM", "dumb");
        workspace
            .lock()
            .unwrap()
            .spawn(command, "sh".to_string(), 80, 16)
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
        let high = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let last_menu: Mutex<Option<std::time::Instant>> = Mutex::new(None);
        Arc::new(move |id: PaneId| {
            let text = WorkspacePaneAccess::new(Arc::clone(&workspace))
                .pane_collapsed(id)
                .unwrap_or_default();
            // ⚠ HELD MONOTONIC BY HAND, `supervised`'s measured rule: the count is read off the
            // COLLAPSED screen, so a pane that has scrolled would make it go DOWN — and
            // `seq > began_at` can never be satisfied again by a number that shrank.
            let answers = (text.matches("ACK").count() + text.matches("MILESTONE").count()) as u64;
            let seq = high
                .fetch_max(answers, std::sync::atomic::Ordering::SeqCst)
                .max(answers);
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
/// ⚠⚠⚠ **AND IT IS HELD MONOTONIC BY HAND, WHICH THE SECOND STALL OF THAT GATE PAID FOR.**
/// The count is read off the COLLAPSED SCREEN, so it is a claim about the terminal's SIZE as
/// much as about the peer: once the pane had scrolled, `ACK 1` left the grid and the count went
/// DOWN — and `seq > began_at` can never be satisfied again by a number that shrank. R375
/// recorded exactly this trap about counting from a screen; a real detector's `seq` never
/// decreases while the pane lives, and this is what makes the stand-in honest about that.
pub(crate) fn supervised(workspace: &Arc<Mutex<Workspace>>) -> WorkspacePaneAccess {
    let source = {
        let workspace = Arc::clone(workspace);
        let high = Arc::new(std::sync::atomic::AtomicU64::new(0));
        Arc::new(move |id: PaneId| {
            let screen = WorkspacePaneAccess::new(Arc::clone(&workspace))
                .pane_collapsed(id)
                .unwrap_or_default();
            let answers =
                (screen.matches("ACK").count() + screen.matches("MILESTONE").count()) as u64;
            let seq = high
                .fetch_max(answers, std::sync::atomic::Ordering::SeqCst)
                .max(answers);
            Some(crate::access::AgentObservation {
                state: AgentState::Idle,
                agent: Some("claude".to_string()),
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
