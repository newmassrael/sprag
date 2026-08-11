//! The `Pipe` plugin — relay one pane's output into another (plugin #2).
//!
//! The second consumer that validates the extension API. Each step takes the source pane's
//! COMPLETE LOGICAL LINES since its cursor and injects them into the *destination* pane. It never
//! self-converges — the [`Driver`]'s guardrails bind it (first-class safety per the README).
//!
//! # ⚠⚠ Why the cursor is a LINE NUMBER and not a mark on the rows
//!
//! This relay read the source's GRID for most of its life — first by damage generation, then by
//! row text — and a grid is a RENDERING of output at the width the pane currently has. Every
//! version of that had the same three holes, and two of them were measured as live defects:
//!
//! * a **RESIZE** re-wraps and re-stamps every row, so a client merely ATTACHING to the session
//!   made this relay re-inject the source's entire screen into a peer that ACTS on what it
//!   receives (measured: 16 bytes for a resize that printed nothing);
//! * a **REPAINT** changes no content at all, and a generation-keyed reader called it output;
//! * **SCROLLING** dropped every line the relay did not come back for in time — silently, with
//!   every number the run reported identical to a working relay's.
//!
//! A LOGICAL line is what the child actually wrote, and reflow is defined as preserving it. So the
//! source numbers its lines from birth ([`PaneOutputLines`](crate::access::PaneOutputLines)) and
//! this holds an ADDRESS: each line is delivered EXACTLY ONCE however often its rows are re-wrapped
//! or repainted, a line that scrolled away is still delivered, and a source that outruns the
//! retained history produces a COUNTED gap rather than a silent one.
//!
//! ⚠ Known limitation (for the AI↔AI relay layer, not now): a line is relayed whole once complete,
//! not as a character-level delta — so an in-place redraw is relayed as the lines it settles on,
//! and the line the source's cursor is still on waits until it is finished.
//!
//! [`Driver`]: crate::driver::Driver

use std::time::Duration;

use sprag_terminal::PaneId;

use crate::access::{KeyStroke, PaneAccess, PaneError, RowTrail};
use crate::plugin::{Cost, Plugin, Step, Verdict};
use crate::readiness::{Reached, Readiness, ReadyWhen};
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
    pub ready_when: Option<ReadyWhen>,
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
    /// The absolute line number this relay has delivered up to — see
    /// [`PaneOutputLines`](crate::access::PaneOutputLines).
    ///
    /// ⚠⚠ A LINE NUMBER, not a row mark. It is an ADDRESS that survives a resize, so each of the
    /// source's lines is delivered EXACTLY ONCE however many times its rows are re-wrapped or
    /// repainted underneath.
    cursor: u64,
    /// The fallback for a host with no output stream: what each source row HELD when it was last
    /// relayed. ⚠ A DEGRADATION and not an equivalent — it cannot see a line that scrolled away,
    /// and a genuine re-wrap moves the rows under it. See [`RowTrail`].
    consumed: RowTrail,
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
            cursor: 0,
            consumed: RowTrail::default(),
            ready: Readiness::new(spec.ready_when, spec.ready_within),
        }
    }
}

impl Pipe {
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
    fn shown(panes: &dyn PaneAccess, pane: PaneId, before: &RowTrail, written: &str) -> Shown {
        let changed = before.fresh(panes, pane);
        if changed.is_empty() {
            return Shown::Nothing;
        }
        if changed
            .iter()
            .all(|line| line.trim().is_empty() || written.contains(line.trim()))
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

        // ⚠⚠ WHAT THE SOURCE PRODUCED, BY LINE NUMBER — not what its grid looks like. A row is
        // where the terminal broke a line at the width it had; a LOGICAL line is what the child
        // wrote. Keyed on the rendering, this relay re-injected the source's whole screen every
        // time a client attached (a resize re-stamps and re-wraps every row) and lost anything
        // that scrolled away between steps.
        //
        // ⚠⚠ A ROW IS A LINE, AND IT HAS TO ARRIVE AS ONE. These were concatenated into a single
        // run-on string and written with no terminator, so a line-oriented destination — `read`,
        // a REPL, anything cooked — never saw a complete line and could not act on the relay at
        // all. Both of this plugin's gates passed anyway, because the pty's echo put the text on
        // the destination's screen and the check was looking at the screen.
        let (lines, lost) = match panes
            .output_lines()
            .and_then(|stream| stream.pane_lines_since(self.src, self.cursor))
        {
            Some(since) => {
                self.cursor = since.next;
                let mut lines = since.lines;
                // ⚠⚠ AN UNTERMINATED LAST LINE IS RELAYED ONLY ONCE THE SOURCE HAS EXITED, and the
                // signal is EOF rather than a quiet period. A line with no newline after it is
                // something the source has not finished saying — while it is alive that is a
                // PROMPT as often as an answer, and relaying furniture into a peer is worse than
                // waiting. Once its child is gone the line is unfinished forever, and dropping it
                // would silently lose the last thing the source said.
                //
                // ⚠ A quiescence timer would answer this too, and would be the scheduling-shaped
                // predicate this crate keeps paying to remove: the same source would relay or not
                // depending on how loaded the box was. EOF is a state, not a wait.
                if !since.partial.is_empty() && panes.pane_eof(self.src) == Some(true) {
                    lines.push(since.partial);
                }
                (lines, since.lost)
            }
            // ⚠ The DEGRADATION, named rather than silent: a host with no output stream is read by
            // comparing its rendering, which cannot see a scrolled-away line.
            None => (self.consumed.take_fresh(panes, self.src), 0),
        };
        let lines: Vec<String> = lines.into_iter().filter(|line| !line.is_empty()).collect();
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
        let before = RowTrail::mark(panes, self.dst);
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

        // ⚠⚠ A GAP IS REPORTED, NEVER SWALLOWED. Retained history is bounded, so a source that
        // outruns it between two steps has lines this relay can never deliver — and a silent gap
        // is indistinguishable from a quiet source, which is the confusion that would make every
        // number this run publishes a lie about completeness.
        let gap = if lost == 0 {
            String::new()
        } else {
            format!("; {lost} EARLIER LINES WERE LOST — the source outran the retained history")
        };
        // The pipe never self-terminates; the Driver's guardrails bind it.
        Ok(Step::new(Cost::Bytes(cost), Verdict::Continue).noting(
            gap + &match reacted {
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
            },
        ))
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

    /// ⚠⚠ **A GAP IN THE SOURCE IS REPORTED, NOT SWALLOWED** — the arm a real pty cannot be made
    /// to build on demand, and the one whose absence would be invisible.
    ///
    /// Retained history is bounded, so a source that outruns it between two steps has lines this
    /// relay can never deliver. **A silent gap is indistinguishable from a quiet source**: every
    /// number the run publishes — bytes, iterations, the verdict — is identical whether the relay
    /// delivered everything or a fraction. The count is the only thing that says which.
    #[test]
    fn a_source_that_outran_its_history_is_reported_as_a_gap() {
        /// A host whose stream reports lines AND a loss.
        struct Lossy;
        impl PaneAccess for Lossy {
            fn pane_ids(&self) -> Vec<PaneId> {
                vec![PaneId(1), PaneId(2)]
            }
            fn pane_collapsed(&self, _id: PaneId) -> Option<String> {
                Some(String::new())
            }
            fn pane_rows(&self, _id: PaneId) -> Option<Vec<crate::access::PaneRow>> {
                Some(Vec::new())
            }
            fn pane_eof(&self, _id: PaneId) -> Option<bool> {
                Some(false)
            }
            fn pane_full_text(&self, _id: PaneId) -> Option<String> {
                Some(String::new())
            }
            fn inject(
                &self,
                _id: PaneId,
                _keys: &[KeyStroke],
            ) -> Result<crate::access::Written, PaneError> {
                Ok(crate::access::Written::of(1))
            }
            fn output_lines(&self) -> Option<&dyn crate::access::PaneOutputLines> {
                Some(self)
            }
        }
        impl crate::access::PaneOutputLines for Lossy {
            fn pane_lines_since(&self, _id: PaneId, _cursor: u64) -> Option<sprag_vt::LinesSince> {
                Some(sprag_vt::LinesSince {
                    lines: vec!["survived".to_string()],
                    next: 10,
                    lost: 3,
                    partial: String::new(),
                })
            }
        }

        let step = Pipe::new(PipeSpec::new(PaneId(1), PaneId(2)))
            .step(&Lossy, &RunContext::uncancellable())
            .expect("the relay");
        let said = step.note.unwrap_or_default();
        assert!(
            said.contains('3') && said.contains("LOST"),
            "the step must say HOW MANY lines the source outran — without it a partial relay \
             reports exactly what a complete one reports: {said:?}",
        );
    }

    /// ⚠⚠ **A HOST WITH NO OUTPUT STREAM STILL RELAYS, BY THE RENDERING** — this plugin's own
    /// degradation arm.
    ///
    /// Gated HERE as well as in [`crate::agent`] because it is a different call site in a different
    /// plugin: *"the same code is tested elsewhere"* is the reasoning R351 caught being wrong when
    /// a shared path stopped being shared. A relay that silently delivered NOTHING on such a host
    /// would report the same `continue` and the same zero bytes a quiet source produces.
    #[test]
    fn a_host_with_no_output_stream_still_relays_by_the_rendering() {
        /// A source that holds one line and a destination that records what it is given, with
        /// every optional capability at its default.
        struct NoStream(Mutex<Vec<String>>);
        impl PaneAccess for NoStream {
            fn pane_ids(&self) -> Vec<PaneId> {
                vec![PaneId(1), PaneId(2)]
            }
            fn pane_collapsed(&self, id: PaneId) -> Option<String> {
                (id == PaneId(1)).then(|| "from-the-source".to_string())
            }
            fn pane_rows(&self, id: PaneId) -> Option<Vec<crate::access::PaneRow>> {
                Some(match id {
                    PaneId(1) => vec![crate::access::PaneRow {
                        generation: 1,
                        text: "from-the-source".to_string(),
                    }],
                    _ => Vec::new(),
                })
            }
            fn pane_eof(&self, _id: PaneId) -> Option<bool> {
                Some(false)
            }
            fn pane_full_text(&self, id: PaneId) -> Option<String> {
                self.pane_collapsed(id)
            }
            fn inject(
                &self,
                id: PaneId,
                keys: &[KeyStroke],
            ) -> Result<crate::access::Written, PaneError> {
                assert_eq!(id, PaneId(2), "only the DESTINATION is typed into");
                self.0
                    .lock()
                    .unwrap()
                    .push(keys.iter().map(|k| k.key.as_str()).collect());
                Ok(crate::access::Written::of(1))
            }
        }

        let access = NoStream(Mutex::new(Vec::new()));
        let mut pipe = Pipe::new(PipeSpec::new(PaneId(1), PaneId(2)));
        let step = pipe
            .step(&access, &RunContext::uncancellable())
            .expect("the relay");
        assert!(
            step.cost.amount() > 0,
            "a host without the stream must still relay, or the degradation is an outage: {step:?}",
        );
        assert_eq!(
            access.0.lock().unwrap().first().map(String::as_str),
            Some("from-the-sourceEnter"),
            "and it delivers the source's line, terminated",
        );
    }

    /// ⚠⚠ **A LINE THAT SCROLLED AWAY IS STILL RELAYED** — what only an output STREAM can do, and
    /// the reason this plugin stopped reading the grid.
    ///
    /// A relay keyed on rows can only ever deliver what is currently VISIBLE. A source that prints
    /// faster than the relay steps — which is every source worth relaying — pushes its earlier
    /// lines off the top, and a row-keyed reader simply never sees them: no error, no gap, no
    /// number that differs from a working relay's. **The peer is silently given a subset.**
    ///
    /// The fixture makes that certain rather than likely: FIVE lines onto a TWO-row pane, so three
    /// of them are gone from the grid before the relay ever looks.
    #[test]
    fn a_line_that_scrolled_off_the_source_is_still_relayed() {
        let workspace = Arc::new(Mutex::new(Workspace::new((20, 2))));
        let spawn = |ws: &Arc<Mutex<Workspace>>, script: &str| {
            let mut command = CommandBuilder::new("/bin/sh");
            command.arg("-c");
            command.arg(script);
            command.env("TERM", "dumb");
            ws.lock()
                .unwrap()
                .spawn(command, "peer".to_string(), 20, 2)
                .expect("spawn")
        };
        let src = spawn(&workspace, "printf 'L1\\nL2\\nL3\\nL4\\nL5\\n'; exec cat");
        let dst = spawn(&workspace, "cat");
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));
        assert!(wait_until(&access, src, "L5"), "the source never printed");
        assert!(
            !access
                .pane_collapsed(src)
                .unwrap_or_default()
                .contains("L1"),
            "⚠ THE CONTROL: `L1` must ALREADY be off the two-row grid, or this gate is about a \
             visible line and measures nothing new",
        );

        let mut pipe = Pipe::new(PipeSpec::new(src, dst));
        let _relayed = pipe
            .step(&access, &RunContext::uncancellable())
            .expect("the relay");

        // ⚠ The destination's FULL retained text, not its two-row screen — it scrolls too, and
        // reading the screen would measure the fixture's height rather than the relay's delivery.
        let seen = access.pane_full_text(dst).unwrap_or_default();
        for line in ["L1", "L2", "L3", "L4", "L5"] {
            assert!(
                seen.contains(line),
                "{line} was produced by the source and must reach the destination — a relay that \
                 delivers only what is still on screen hands its peer a SUBSET and reports the \
                 same numbers a working one does: {seen:?}",
            );
        }
    }

    /// ⚠⚠ **A RESIZE RE-RELAYS THE WHOLE SCREEN** — the same paint-vs-content category error R361
    /// removed from the readiness barrier, in the plugin where it is worst.
    ///
    /// This relay chooses WHICH ROWS TO RE-INJECT by damage generation, and a resize
    /// (`Screen::reflowed`) stamps every row with a fresh one while the source produces nothing. So
    /// a client ATTACHING to the session — which is what resizes panes, and the ordinary thing a
    /// person does — makes the relay type the source's entire visible screen into the destination
    /// a second time.
    ///
    /// **It is a delivery defect, not a cosmetic one**: the destination is a peer that ACTS on what
    /// it receives, so the duplicate is re-executed, re-prompted or re-answered.
    ///
    /// Both halves: the step relays NOTHING (its own note says so) and the destination's screen
    /// gained no second copy.
    #[test]
    fn a_resize_of_the_source_does_not_relay_its_whole_screen_again() {
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
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));

        let mut seed = KeyStroke::text("relayme");
        seed.push(KeyStroke::named("Enter"));
        let _seeded = access.inject(src, &seed).expect("seed src");
        assert!(wait_until(&access, src, "relayme"), "source never echoed");

        let mut pipe = Pipe::new(PipeSpec::new(src, dst));
        let run = RunContext::uncancellable();
        let _first = pipe.step(&access, &run).expect("the first relay");
        assert!(
            wait_until(&access, dst, "relayme"),
            "the fixture must actually deliver once, or the second half measures nothing",
        );

        // ⚠ The destination's screen BEFORE the event. Asserted as "unchanged" rather than as a
        // count, because how many copies ONE delivery leaves is the fixture's business (a `cat`
        // destination echoes AND copies) and this gate is about what the RESIZE adds.
        let before = access.pane_collapsed(dst).unwrap_or_default();

        // THE EVENT: a client attaches, so the pane is re-laid out. Nothing is produced.
        workspace
            .lock()
            .unwrap()
            .resize(src, 18, 4, (0, 0))
            .expect("resize the source");
        let after = pipe.step(&access, &run).expect("the step after the resize");

        assert_eq!(
            after.note.as_deref(),
            Some("nothing new on the source"),
            "a RESIZE is not the source producing output — every row's damage generation moved and \
             not one byte was printed, so the relay must send nothing: {after:?}",
        );
        sleep(Duration::from_millis(200));
        assert_eq!(
            access.pane_collapsed(dst).unwrap_or_default(),
            before,
            "and the destination gained NOTHING — it is a peer that ACTS on what it receives, so a \
             duplicate line is re-executed, re-prompted or re-answered",
        );
    }

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
            ready_when: Some(ReadyWhen::Prints("PEER-UP".to_string())),
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
            ready_when: Some(ReadyWhen::Prints(
                "A MARKER THIS PANE NEVER PRINTS".to_string(),
            )),
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
            ready_when: Some(ReadyWhen::Prints("NEVER-PRINTED".to_string())),
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
        crate::testing::refused_naming(
            outcome.failure.as_ref(),
            &ReadyWhen::Prints("NEVER-PRINTED".to_string()),
            // ⚠ And what the destination WAS running — `exec cat`, so this is the pane's own
            // program and not a shell that had not started it yet. The diagnostic reaches the
            // relay's failure too, which is the half a fix applied to one consumer would miss.
            "cat",
            "and the cause names the question the caller got wrong, and what the pane was doing",
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
