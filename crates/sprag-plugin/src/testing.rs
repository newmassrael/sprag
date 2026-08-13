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

/// A pane running [`menu_peer`], already showing its menu.
pub(crate) fn asking_peer(kind: &str) -> (WorkspacePaneAccess, PaneId) {
    let (access, pane) = peer_running(menu_peer(kind));
    // The menu must be UP before anything asks the barrier, or the gate is about a pane that
    // was never blocked.
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(10)
        && crate::readiness::peer_asking(&access, pane)
            .flatten()
            .is_none()
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        crate::readiness::peer_asking(&access, pane)
            .flatten()
            .is_some(),
        "the fixture's peer must be showing a menu the shipping parser reads, or this gate is \
         about nothing: {:?}",
        access.pane_collapsed(pane),
    );
    (access, pane)
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
            }),
            &ReadyWhen::Runs("claude".to_string()),
            "sh",
            "a barrier that gave up on a pane still running its shell",
        );
    }
}
