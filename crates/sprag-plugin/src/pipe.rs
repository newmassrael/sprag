//! The `Pipe` plugin — relay one pane's output into another (plugin #2).
//!
//! The second consumer that validates the extension API. Each step reads the
//! *source* pane and injects the text of whatever rows are newly damaged since
//! its last relay into the *destination* pane. It never self-converges — the
//! [`Driver`]'s guardrails bind it (first-class safety per the README). This
//! proves the substrate generalizes beyond the orchestrator: the same
//! damage-`generation` primitive, but the cursor is "after last relay" rather
//! than "before last stimulus", and it lives in the plugin (not the Driver).
//!
//! Known limitation (for the AI↔AI relay layer, not now): a row is relayed
//! whole whenever its generation bumps, not as a character-level delta — over-
//! relays for in-place redraws, correct for an append/echo source.
//!
//! [`Driver`]: crate::driver::Driver

use std::time::Duration;

use sprag_terminal::PaneId;

use crate::access::{KeyStroke, PaneAccess, PaneError};
use crate::plugin::{Cost, Plugin, Step, Verdict};
use crate::readiness::{Reached, Readiness};
use crate::run::{RunContext, Waited, poll_until};

/// How long a relay waits for its destination to show ANY change before reporting that it showed
/// nothing.
///
/// # ⚠ Why a reaction and not a confirmation of the TEXT
///
/// [`deliver`](crate::deliver::deliver) confirms that specific text appeared, which is what a
/// prompt needs and what a relay cannot use: relayed output is arbitrary and often longer than the
/// destination is wide, so it arrives WRAPPED and no `contains` over the rendered screen can find
/// it whole. A damage-generation bump is wrap-proof, needle-free, and answers the question a relay
/// actually has — *did anything happen over there?*
///
/// Sized like the orchestrator's own observe window: an echo is a round trip through a pty, which
/// is microseconds when the peer is reading.
const REACTION_TIMEOUT: Duration = Duration::from_millis(500);

/// What a relay is pointed at, and what its destination must SHOW before it is typed into.
///
/// A spec rather than two bare ids, matching every other plugin here, because the barrier below is
/// the destination's business and belongs beside the destination.
#[derive(Clone, Debug)]
pub struct PipeSpec {
    /// The pane whose new output is relayed.
    pub src: PaneId,
    /// The pane it is relayed INTO.
    pub dst: PaneId,
    /// What `dst` must show before the first relay — see
    /// [`Readiness`]. `None` relays immediately.
    ///
    /// # ⚠⚠ A relay is MORE exposed to this than a drive loop, not less
    ///
    /// This plugin's destination is a pane **somebody else prepared** — that is the whole shape of
    /// a relay — and it had no barrier at all until R359 measured one being fed. A destination that
    /// was still a shell ate two relayed lines (`SHELL-ATE relayme`) while the peer that came up a
    /// second later saw nothing, and the run reported the same bytes, the same `continue` and the
    /// same `exhausted` a working relay reports.
    pub ready_when: Option<String>,
    /// How long to wait for [`ready_when`](Self::ready_when), or `None` for
    /// [`DEFAULT_READY_TIMEOUT`](crate::readiness::DEFAULT_READY_TIMEOUT).
    pub ready_within: Option<Duration>,
}

impl PipeSpec {
    /// Relay `src` into `dst` with no readiness barrier — for a destination already running what
    /// it is meant to be running.
    #[must_use]
    pub const fn new(src: PaneId, dst: PaneId) -> Self {
        Self {
            src,
            dst,
            ready_when: None,
            ready_within: None,
        }
    }
}

/// Relays the source pane's new output into the destination pane.
pub struct Pipe {
    src: PaneId,
    dst: PaneId,
    /// Last-relayed damage generation per source row.
    consumed: Vec<u64>,
    /// The barrier the DESTINATION must clear before anything is typed into it.
    ready: Readiness,
}

impl Pipe {
    /// Relay according to `spec`.
    #[must_use]
    pub fn new(spec: PipeSpec) -> Self {
        Self {
            src: spec.src,
            dst: spec.dst,
            consumed: Vec::new(),
            ready: Readiness::new(spec.ready_when, spec.ready_within),
        }
    }
}

impl Pipe {
    /// Every row's damage generation on `pane`, or empty for a pane that is gone.
    fn generations(panes: &dyn PaneAccess, pane: PaneId) -> Vec<u64> {
        panes
            .pane_rows(pane)
            .map(|rows| rows.iter().map(|row| row.generation).collect())
            .unwrap_or_default()
    }

    /// What the destination has shown since `before`, given what was just written to it.
    ///
    /// # ⚠⚠ Why "a row changed" is not delivery
    ///
    /// **A pty echoes what is written to it whether or not any program ever reads a byte.** So a
    /// destination running `sleep` — nothing reading, nothing consuming — showed the relayed text
    /// straight back and this plugin called it *"the destination reacted"*. That is a delivery
    /// claim with the KERNEL behind it rather than the peer, in the one plugin whose entire job is
    /// delivery: R357 taught it to look at where it delivered, and this is what it was looking at.
    ///
    /// The screen cannot prove a program consumed anything — nothing visible distinguishes `cat`
    /// writing the text back from the pty echoing it. What it CAN say is whether the destination
    /// produced anything OF ITS OWN, and the three answers are three different findings.
    fn shown(panes: &dyn PaneAccess, pane: PaneId, before: &[u64], written: &str) -> Shown {
        let Some(rows) = panes.pane_rows(pane) else {
            return Shown::Nothing;
        };
        let changed: Vec<&str> = rows
            .iter()
            .enumerate()
            .filter(|(i, row)| row.generation > before.get(*i).copied().unwrap_or(0))
            .map(|(_, row)| row.text.trim())
            .collect();
        if changed.is_empty() {
            return Shown::Nothing;
        }
        if changed
            .iter()
            .all(|line| line.is_empty() || written.contains(line))
        {
            return Shown::OwnBytesBack;
        }
        Shown::Output
    }
}

/// What a destination showed after a relay — see [`Pipe::shown`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Shown {
    /// Nothing changed on it at all.
    Nothing,
    /// Only the relayed text came back, which the pty does on its own.
    OwnBytesBack,
    /// Something the destination produced.
    Output,
}

impl Plugin for Pipe {
    fn step(&mut self, panes: &dyn PaneAccess, run: &RunContext) -> Result<Step, PaneError> {
        // ⚠⚠ NOT ONE BYTE INTO A DESTINATION THAT IS NOT READY — see [`PipeSpec::ready_when`]. The
        // source is only READ, so it needs no barrier; the destination is typed into, and it is the
        // one this plugin does not own. Latched, so it costs nothing after the first step.
        //
        // ⚠ Before the source is READ, deliberately. Whatever the source produces while this waits
        // is still newly-damaged when the wait ends, so a relay that had to wait for its
        // destination delivers what arrived meanwhile rather than losing it — reading first would
        // consume those rows against a destination not ready to be given them.
        if self.ready.reached(panes, self.dst, run)? == Reached::RunEnded {
            return Ok(Step::new(Cost::Bytes(0), Verdict::Continue)
                .noting("the run ended while waiting for the destination to be ready"));
        }

        let rows = panes.pane_rows(self.src).unwrap_or_default();
        if self.consumed.len() < rows.len() {
            self.consumed.resize(rows.len(), 0);
        }
        // Collect the text of rows newly damaged since the last relay, AS LINES.
        //
        // ⚠⚠ A ROW IS A LINE, AND IT HAS TO ARRIVE AS ONE. These were concatenated into a single
        // run-on string and written with no terminator, so a line-oriented destination — `read`,
        // a REPL, anything cooked — never saw a complete line and could not act on the relay at
        // all. Both of this plugin's gates passed anyway, because the pty's echo put the text on
        // the destination's screen and the check was looking at the screen.
        let mut lines: Vec<String> = Vec::new();
        for (i, row) in rows.iter().enumerate() {
            if row.generation > self.consumed[i] {
                let text = row.text.trim_end();
                if !text.is_empty() {
                    lines.push(text.to_string());
                }
                self.consumed[i] = row.generation;
            }
        }
        if lines.is_empty() {
            return Ok(
                Step::new(Cost::Bytes(0), Verdict::Continue).noting("nothing new on the source")
            );
        }
        let relayed = lines.join("\n");

        // ⚠⚠ WATCH THE DESTINATION REACT. This plugin's whole job is delivery and it was the one
        // that never looked at where it delivered: it read the source, wrote the destination,
        // charged the bytes, and answered `continue` forever. A pipe relaying into a pane that
        // swallows its input is indistinguishable, in every number a run reports, from one that is
        // working — so the failure this plugin exists to have is the failure it could not report.
        let before = Self::generations(panes, self.dst);
        // Each line followed by Enter — the terminator that makes it a line the destination's
        // reader can complete, exactly as the orchestrator terminates its stimulus.
        let mut keys = Vec::new();
        for line in &lines {
            keys.extend(KeyStroke::text(line));
            keys.push(KeyStroke::named("Enter"));
        }
        let cost = panes.inject(self.dst, &keys)?.bytes();
        let reacted = poll_until(run, REACTION_TIMEOUT, || {
            Self::shown(panes, self.dst, &before, &relayed) == Shown::Output
        });

        // The pipe never self-terminates; the Driver's guardrails bind it.
        Ok(
            Step::new(Cost::Bytes(cost), Verdict::Continue).noting(match reacted {
                Waited::Ready => format!("relayed {cost} bytes; the destination answered"),
                // ⚠ The wait ran out with no output of the destination's own — and WHICH of the
                // two silences it was is the difference between a pane nobody is reading and one
                // whose reader said nothing back.
                Waited::TimedOut => match Self::shown(panes, self.dst, &before, &relayed) {
                    Shown::Output => format!("relayed {cost} bytes; the destination answered late"),
                    Shown::OwnBytesBack => format!(
                        "relayed {cost} bytes and ONLY THOSE BYTES CAME BACK — the pty echoes \
                         them whether or not anything read them"
                    ),
                    Shown::Nothing => {
                        format!("relayed {cost} bytes and THE DESTINATION SHOWED NOTHING")
                    }
                },
                Waited::Stopped => format!("relayed {cost} bytes; the run ended"),
            }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::WorkspacePaneAccess;
    use crate::driver::{Ceiling, Driver, Guardrails, OutcomeState};
    use crate::testing::STANDIN_READS_TTY;
    use sprag_terminal::{CommandBuilder, Workspace};
    use std::sync::{Arc, Mutex};
    use std::thread::sleep;
    use std::time::{Duration, Instant};

    /// A workspace with two live `cat` panes, wrapped as pane-access.
    fn two_cat_panes() -> (WorkspacePaneAccess, PaneId, PaneId) {
        let workspace = Arc::new(Mutex::new(Workspace::new((20, 4))));
        let spawn = |ws: &Arc<Mutex<Workspace>>| {
            let mut command = CommandBuilder::new("/bin/sh");
            command.arg("-c");
            command.arg("cat");
            command.env("TERM", "dumb");
            ws.lock()
                .unwrap()
                .spawn(command, "cat".to_string(), 20, 4)
                .expect("spawn")
        };
        let src = spawn(&workspace);
        let dst = spawn(&workspace);
        (WorkspacePaneAccess::new(workspace), src, dst)
    }

    fn wait_until(access: &WorkspacePaneAccess, pane: PaneId, needle: &str) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if access
                .pane_collapsed(pane)
                .is_some_and(|t| t.contains(needle))
            {
                return true;
            }
            sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn relays_source_output_into_destination() {
        let (access, src, dst) = two_cat_panes();

        // Seed the source out-of-band and wait for it to echo, so the pipe's
        // first step has real output to relay.
        let mut seed = KeyStroke::text("relayme");
        seed.push(KeyStroke::named("Enter"));
        let _seeded = access.inject(src, &seed).expect("seed src");
        assert!(
            wait_until(&access, src, "relayme"),
            "source never echoed the seed"
        );

        // The pipe never converges; the iteration budget binds it.
        let mut pipe = Pipe::new(PipeSpec::new(src, dst));
        let outcome = Driver::new(Guardrails {
            max_iterations: 5,
            max_cost: None,
            max_duration: None,
        })
        .run(&mut pipe, &access, &RunContext::uncancellable());
        assert_eq!(outcome.state, OutcomeState::Exhausted(Ceiling::Iterations));

        // The destination received the relayed text (its echo is async).
        assert!(
            wait_until(&access, dst, "relayme"),
            "destination never received the relay"
        );
    }

    /// ⚠⚠ **A RELAY INTO A PANE THAT SWALLOWS IT SAYS SO** — the one plugin whose whole job is
    /// delivery was the one that never looked at where it delivered.
    ///
    /// Both halves against the same fixture, because every NUMBER a run reports is identical
    /// either way: same bytes charged, same `continue`, same `exhausted`. The only thing that can
    /// tell the two apart is the journal, which is exactly why this defect survived.
    ///
    /// The deaf destination is a child that reads its input and prints nothing (`cat >/dev/null`
    /// with echo off) — a real program doing a reasonable thing, not a broken pane. The pane is
    /// alive, the write succeeds, the bytes are charged, and nothing appears.
    #[test]
    fn a_relay_into_a_destination_that_shows_nothing_reports_that_it_showed_nothing() {
        let relay_into = |dst_script: &str| {
            let workspace = Arc::new(Mutex::new(Workspace::new((20, 4))));
            let spawn = |script: &str| {
                let mut command = CommandBuilder::new("/bin/sh");
                command.arg("-c");
                command.arg(script);
                command.env("TERM", "dumb");
                workspace
                    .lock()
                    .unwrap()
                    .spawn(command, "peer".to_string(), 20, 4)
                    .expect("spawn")
            };
            let src = spawn("cat");
            let dst = spawn(dst_script);
            let access = WorkspacePaneAccess::new(Arc::clone(&workspace));

            let mut seed = KeyStroke::text("relayme");
            seed.push(KeyStroke::named("Enter"));
            let _seeded = access.inject(src, &seed).expect("seed src");
            assert!(wait_until(&access, src, "relayme"), "source never echoed");

            let journal = Arc::new(Mutex::new(sprag_plugin_progress()));
            let outcome = Driver::new(Guardrails {
                max_iterations: 1,
                max_cost: None,
                max_duration: None,
            })
            .reporting_to(Arc::clone(&journal))
            .run(
                &mut Pipe::new(PipeSpec::new(src, dst)),
                &access,
                &RunContext::uncancellable(),
            );
            assert_eq!(outcome.state, OutcomeState::Exhausted(Ceiling::Iterations));
            let notes: Vec<String> = journal
                .lock()
                .unwrap()
                .journal
                .iter()
                .filter_map(|step| step.note.clone())
                .collect();
            notes.join(" | ")
        };

        // THE CONTROL FIRST — a destination that answers with something of ITS OWN. If this did
        // not say so, the subjects below would be measuring a check that never passes for anyone.
        //
        // ⚠ It used to be plain `cat`, asserted to read as "reacted". That was the false
        // confidence itself: nothing on screen distinguishes `cat` writing the text back from the
        // pty echoing it, so the control was passing on the kernel's work. A destination that
        // PREFIXES what it reads produces a line that is not the relayed text, which is the only
        // evidence a screen can carry that a program read anything.
        let heard = relay_into("while read line; do echo \"DST-SAW $line\"; done");
        assert!(
            heard.contains("the destination answered"),
            "a destination that produces output of its own must read as answering: {heard}",
        );

        // THE SUBJECT — a destination that consumes its input and prints nothing.
        let deaf = relay_into("stty -echo; cat >/dev/null");
        assert!(
            deaf.contains("THE DESTINATION SHOWED NOTHING"),
            "a relay into a pane that shows nothing must say so — no number in the outcome can: \
             {deaf}",
        );

        // ⚠⚠ THE THIRD CASE, AND THE ONE THAT MATTERS MOST: echo ON, and NOTHING READING. The
        // pty echoes what is written to it whether or not a program ever reads a byte, so a
        // destination running `sleep` shows the relayed text back exactly as a working one does.
        // Reading that as delivery is the same blindness R357 removed, one layer in — the check
        // was watching the kernel rather than the peer.
        let unread = relay_into("sleep 5");
        assert!(
            !unread.contains("the destination reacted"),
            "nothing in this pane has read a byte — the text on screen is the pty's own echo, and \
             calling it a reaction is a delivery claim with nothing behind it: {unread}",
        );
    }

    /// ⚠⚠ **A RELAY CUT OFF BY THE RUN'S CLOCK SAYS SO, RATHER THAN BLAMING THE DESTINATION** —
    /// the branch a mutation proved no test in this workspace built.
    ///
    /// A step whose reaction-wait is ended by the deadline has learned NOTHING about where it
    /// delivered: the destination was given no chance to answer. Reporting that as
    /// `THE DESTINATION SHOWED NOTHING` would put a real finding — the one this plugin exists to
    /// report — on a pane that was never asked. The two are told apart, and only the clock's
    /// answer is honest about who ran out.
    #[test]
    fn a_relay_the_clock_cut_short_does_not_blame_the_destination() {
        let workspace = Arc::new(Mutex::new(Workspace::new((20, 4))));
        let spawn = |script: &str| {
            let mut command = CommandBuilder::new("/bin/sh");
            command.arg("-c");
            command.arg(script);
            command.env("TERM", "dumb");
            workspace
                .lock()
                .unwrap()
                .spawn(command, "peer".to_string(), 20, 4)
                .expect("spawn")
        };
        let src = spawn("cat");
        // Deaf, so the wait can only end by a bound — and the run's is the shorter of the two.
        let dst = spawn("stty -echo; cat >/dev/null");
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));

        let mut seed = KeyStroke::text("relayme");
        seed.push(KeyStroke::named("Enter"));
        let _seeded = access.inject(src, &seed).expect("seed src");
        assert!(wait_until(&access, src, "relayme"), "source never echoed");

        let journal = Arc::new(Mutex::new(sprag_plugin_progress()));
        let outcome = Driver::new(Guardrails {
            max_iterations: 100,
            max_cost: None,
            // Shorter than REACTION_TIMEOUT, so the run's clock is provably what ends the wait.
            max_duration: Some(Duration::from_millis(150)),
        })
        .reporting_to(Arc::clone(&journal))
        .run(
            &mut Pipe::new(PipeSpec::new(src, dst)),
            &access,
            &RunContext::uncancellable(),
        );

        assert_eq!(outcome.state, OutcomeState::Exhausted(Ceiling::Duration));
        let notes = journal
            .lock()
            .unwrap()
            .journal
            .iter()
            .filter_map(|step| step.note.clone())
            .collect::<Vec<_>>()
            .join(" | ");
        assert!(
            notes.contains("the run ended"),
            "the step must say the RUN ended, not that the destination was silent — it was never \
             given the time to speak: {notes}",
        );
        assert!(
            !notes.contains("SHOWED NOTHING") && !notes.contains("ONLY THOSE BYTES"),
            "and it must not report a finding about a destination it cut off: {notes}",
        );
    }

    /// ⚠⚠ **A RELAY DOES NOT TYPE INTO A DESTINATION WHOSE PEER DOES NOT EXIST YET**, and this
    /// plugin had no barrier at all until it was measured being fed.
    ///
    /// A relay's destination is by construction a pane SOMEBODY ELSE prepared, which makes it more
    /// exposed to this than the drive loop that got the barrier first — and the failure is silent
    /// in every number a run reports: same bytes charged, same `continue`, same `exhausted`. Run
    /// against this fixture without the barrier, the destination's stand-in shell ate the relay
    /// twice (`"relaymerelaymeSHELL-ATE relaymeSHELL-ATE relayme"`) and the peer that came up a
    /// second later never saw a word of it.
    ///
    /// Both halves, because either alone is weak: the stand-in must not have been fed, AND the peer
    /// must have received the relay — a barrier that simply never let go would satisfy the first.
    #[test]
    fn a_relay_does_not_feed_a_destination_that_is_still_a_shell() {
        let workspace = Arc::new(Mutex::new(Workspace::new((40, 8))));
        let spawn = |script: &str| {
            let mut command = CommandBuilder::new("/bin/sh");
            command.arg("-c");
            command.arg(script);
            command.env("TERM", "dumb");
            workspace
                .lock()
                .unwrap()
                .spawn(command, "peer".to_string(), 40, 8)
                .expect("spawn")
        };
        let src = spawn("cat");
        // The destination is a stand-in shell that EATS and NAMES anything typed at it, then
        // becomes the peer — the ordinary shape of "open a pane, start the tool in it, relay".
        let dst = spawn(&format!(
            "while read early; do echo \"SHELL-ATE $early\"; done {STANDIN_READS_TTY} & \
             sleep 2; kill $! 2>/dev/null; printf 'PEER-UP\\n'; \
             exec sh -c 'while read l; do echo \"PEER-SAW $l\"; done'"
        ));
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));

        let mut seed = KeyStroke::text("relayme");
        seed.push(KeyStroke::named("Enter"));
        let _seeded = access.inject(src, &seed).expect("seed src");
        assert!(wait_until(&access, src, "relayme"), "source never echoed");

        let spec = PipeSpec {
            ready_when: Some("PEER-UP".to_string()),
            ..PipeSpec::new(src, dst)
        };
        let _outcome = Driver::new(Guardrails {
            max_iterations: 6,
            max_cost: None,
            max_duration: Some(Duration::from_secs(10)),
        })
        .run(&mut Pipe::new(spec), &access, &RunContext::uncancellable());

        assert!(
            wait_until(&access, dst, "PEER-SAW relayme"),
            "the peer must have received the relay once it was up: {:?}",
            access.pane_collapsed(dst),
        );
        // ⚠ WAIT FOR THE EVIDENCE. The stand-in shell's `echo` is asynchronous, so reading the
        // screen the instant the run returns races it and reports "clean" over a pane that was fed
        // — the first form of this gate passed for exactly that reason.
        let screen = access.pane_collapsed(dst).unwrap_or_default();
        assert!(
            !screen.contains("SHELL-ATE"),
            "the relay fed a pane whose peer did not exist yet — every SHELL-ATE is output this \
             relay handed to a program that was about to be killed: {screen:?}",
        );
    }

    /// ⚠⚠ **A RELAY THE CLOCK CUT OFF WHILE WAITING TO BE LET IN SAYS THAT, AND CHARGES NOTHING.**
    ///
    /// The other ending of the barrier, and a different finding from both of its neighbours: not
    /// *the destination never came up* (that is a failure naming the marker) and not *the
    /// destination said nothing* (that is about a pane which was actually given something). Here
    /// the relay never wrote a byte, so a note blaming the destination would be a report about a
    /// pane this run never spoke to, and any cost charged would be for bytes that do not exist.
    #[test]
    fn a_relay_whose_run_ends_while_waiting_to_be_let_in_charges_nothing_and_blames_nobody() {
        let workspace = Arc::new(Mutex::new(Workspace::new((20, 4))));
        let spawn = |script: &str| {
            let mut command = CommandBuilder::new("/bin/sh");
            command.arg("-c");
            command.arg(script);
            command.env("TERM", "dumb");
            workspace
                .lock()
                .unwrap()
                .spawn(command, "peer".to_string(), 20, 4)
                .expect("spawn")
        };
        let src = spawn("cat");
        let dst = spawn("exec cat");
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));

        let mut seed = KeyStroke::text("relayme");
        seed.push(KeyStroke::named("Enter"));
        let _seeded = access.inject(src, &seed).expect("seed src");
        assert!(wait_until(&access, src, "relayme"), "source never echoed");

        let spec = PipeSpec {
            ready_when: Some("A MARKER THIS PANE NEVER PRINTS".to_string()),
            // ⚠ FAR ABOVE the run's clock, so the run's deadline is provably what ends the wait
            // rather than the barrier's own bound — that ending is the OTHER arm.
            ready_within: Some(Duration::from_secs(300)),
            ..PipeSpec::new(src, dst)
        };
        let journal = Arc::new(Mutex::new(sprag_plugin_progress()));
        let outcome = Driver::new(Guardrails {
            max_iterations: 100,
            max_cost: None,
            max_duration: Some(Duration::from_millis(200)),
        })
        .reporting_to(Arc::clone(&journal))
        .run(&mut Pipe::new(spec), &access, &RunContext::uncancellable());

        assert_eq!(outcome.state, OutcomeState::Exhausted(Ceiling::Duration));
        let notes = journal
            .lock()
            .unwrap()
            .journal
            .iter()
            .filter_map(|step| step.note.clone())
            .collect::<Vec<_>>()
            .join(" | ");
        assert!(
            notes.contains("waiting for the destination to be ready"),
            "the step must say it never got in, not anything about a relay it did not make: \
             {notes}",
        );
        assert!(
            !notes.contains("SHOWED NOTHING") && !notes.contains("ONLY THOSE BYTES"),
            "and it must not report a finding about a destination it never wrote to: {notes}",
        );
        assert_eq!(
            outcome.cost,
            Some(Cost::Bytes(0)),
            "nothing was injected, so nothing is charged: {outcome:?}",
        );
        assert_eq!(
            access.pane_collapsed(dst).unwrap_or_default().trim(),
            "",
            "and the destination is untouched",
        );
    }

    /// ⚠⚠ **A DESTINATION THAT NEVER COMES UP FAILS THE RUN AND NAMES THE MARKER**, rather than
    /// relaying into whatever is there.
    ///
    /// The relay's counterpart of the orchestrator's arm, and a different answer from its
    /// neighbour above: *the destination never came up* is about the PANE and names what the
    /// caller got wrong, while *the run ran out of time* is about the run and says nothing about
    /// the pane. A relay that guessed between them would send the caller after the wrong thing.
    #[test]
    fn a_destination_that_never_becomes_ready_fails_the_relay_and_names_the_marker() {
        let workspace = Arc::new(Mutex::new(Workspace::new((20, 4))));
        let spawn = |script: &str| {
            let mut command = CommandBuilder::new("/bin/sh");
            command.arg("-c");
            command.arg(script);
            command.env("TERM", "dumb");
            workspace
                .lock()
                .unwrap()
                .spawn(command, "peer".to_string(), 20, 4)
                .expect("spawn")
        };
        let src = spawn("cat");
        let dst = spawn("exec cat");
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));

        let mut seed = KeyStroke::text("relayme");
        seed.push(KeyStroke::named("Enter"));
        let _seeded = access.inject(src, &seed).expect("seed src");
        assert!(wait_until(&access, src, "relayme"), "source never echoed");

        let spec = PipeSpec {
            ready_when: Some("NEVER-PRINTED".to_string()),
            ready_within: Some(Duration::from_millis(200)),
            ..PipeSpec::new(src, dst)
        };
        let outcome = Driver::new(Guardrails {
            max_iterations: 100,
            max_cost: None,
            // Far above the barrier's bound, so the run's clock provably is not what ended this.
            max_duration: Some(Duration::from_secs(30)),
        })
        .run(&mut Pipe::new(spec), &access, &RunContext::uncancellable());

        assert_eq!(
            outcome.state,
            OutcomeState::Failed,
            "a destination that never came up FAILS the relay: {outcome:?}",
        );
        assert_eq!(
            outcome.failure,
            Some(PaneError::NeverReady("NEVER-PRINTED".to_string())),
            "and the cause names the marker the caller got wrong",
        );
        assert_eq!(
            access.pane_collapsed(dst).unwrap_or_default().trim(),
            "",
            "and not one byte was relayed into it",
        );
    }

    /// The journal cell a `Driver` reports into, spelled once for the test above.
    fn sprag_plugin_progress() -> crate::driver::Progress {
        crate::driver::Progress::default()
    }
}
