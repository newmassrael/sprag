//! The `Agent` adapter plugin — structured request/response over a pane running
//! a real AI CLI (adapter #1, the north star's first realization).
//!
//! This is the first plugin whose pane peer is a *real* AI tool (e.g.
//! `claude -p`) rather than a `cat`/`printf` fixture: it injects a prompt,
//! waits for the tool to finish replying, and captures the reply as structured
//! output an external peer reads back as scene-as-data. It is one-shot — one
//! prompt, one captured response, then converge; multi-turn conversation and
//! the bidirectional AI↔AI relay layer on top of this later.
//!
//! Completion is detected by the pane child *exiting*
//! ([`PaneAccess::pane_eof`]), not by output quiescence. A one-shot tool
//! (`claude -p`, or a read-until-EOF shell peer) prints its reply and exits,
//! and the producer guarantees every byte is applied to the screen once EOF is
//! observed — so the captured screen is complete and the read never tears. This
//! sidesteps the echo/think-time race a debounce would hit: the injected prompt
//! echoes back (cooked-mode tty) *before* the model has even started replying,
//! so "settle after the first change" would converge on the echo. A `timeout`
//! bounds the wait so a tool that never exits cannot hang the run.
//!
//! ⚠⚠ This paragraph has now been WRONG TWICE, which is what a limitations note left to age looks
//! like. It said *"the projection has no scrollback yet"* years after `sprag-vt` retained history;
//! corrected, it then said the delta was *"still row-keyed ([`RowTrail`]), repaint-proof but not
//! scroll-proof"* while the code beside it already addressed the reply by LINE NUMBER. Both
//! readings survived because nothing drives a doc.
//!
//! What is true, and gated: the reply is the pane's LOGICAL LINES since the prompt's address, with
//! this run's own cooked-mode echo removed by exact match (`without_own_echo`) and with lines the
//! retained history evicted REPORTED as a count rather than dropped. The remaining residue is named
//! on those two items and nowhere else.

use std::time::Duration;

use sprag_input::Modifiers;
use sprag_terminal::{PaneEcho, PaneEndOfInput, PaneId};

use crate::access::{KeyStroke, PaneAccess, PaneError, RowTrail};
use crate::deliver::{Delivered, Delivery, deliver};
use crate::plugin::{Cost, Plugin, Step, Verdict};
use crate::readiness::{Reached, Readiness, ReadyWhen};
use crate::run::{DEFAULT_REPLY_TIMEOUT, RunContext, Waited, poll_until};

/// How much of a prompt has to appear on the pane before the delivery counts it as arrived.
///
/// A prompt longer than this is confirmed on its leading `CONFIRM_WHOLE_UP_TO` characters, because
/// an interactive agent's prompt box WRAPS what is typed into it and draws its own border between
/// the halves — so the pane's collapsed text holds a long prompt in pieces and never as one run.
///
/// Forty, because the number has to be long enough that a match is not a coincidence and short
/// enough to fit inside a narrow pane's box: at this length the fragment is a sentence, and the
/// narrowest pane this project spawns in a test is 40 columns.
const CONFIRM_WHOLE_UP_TO: usize = 40;

/// What the agent asks and how long it waits for the answer.
#[derive(Clone, Debug)]
pub struct AgentSpec {
    /// The prompt injected into the pane (followed by Enter).
    pub prompt: String,
    /// Send Ctrl-D (EOF) after the prompt, so a tool that reads stdin until
    /// end-of-input (`claude -p`, `cat`) sees EOF and replies. Default `true`;
    /// set `false` for a peer that reads line-by-line and stays alive.
    pub eof: bool,
    /// Overall bound on the reply wait. On timeout the agent converges with
    /// whatever it captured (possibly nothing) rather than hanging.
    pub timeout: Duration,
    /// What the pane must SHOW before the prompt is injected — see [`Readiness`].
    /// `None` prompts immediately, which is right for a pane already running the
    /// tool.
    ///
    /// # ⚠⚠ Why this adapter needs it MOST
    ///
    /// The pane is the CALLER'S, and this plugin types a prompt into it and then
    /// hands whatever came back to a peer as *the agent's reply*. Prompted while
    /// the pane is still a shell, the shell runs the prompt as a command AND the
    /// trailing Ctrl-D ([`eof`](Self::eof)) makes it EXIT — which is exactly the
    /// completion signal this adapter waits for. So the run CONVERGES, reports
    /// success, and publishes the shell's error as the model's answer. Measured:
    /// a prompt of *"summarise the repo"* came back as
    /// `"summarise the repo\n$ sh: 1: summarise: not found\n$"`, with nothing in
    /// the outcome, the cost or the note to say it was not a reply.
    pub ready_when: Option<ReadyWhen>,
    /// How long to wait for [`ready_when`](Self::ready_when), or `None` for
    /// [`DEFAULT_READY_TIMEOUT`](crate::readiness::DEFAULT_READY_TIMEOUT).
    pub ready_within: Option<Duration>,
    /// Whether the peer SHOWS a prompt typed at it before it is submitted — and so whether the
    /// prompt can be DELIVERED rather than merely written.
    ///
    /// # ⚠⚠⚠ What the two paths are, and why the difference is not cosmetic
    ///
    /// `true` — the prompt is injected, the pane is read back, it is re-injected until it appears,
    /// and Enter is pressed **only then**. This is [`mod@crate::deliver`], written from a measured
    /// failure against a rival multiplexer: an agent that is up, has a tty and reports itself idle
    /// still discards what you type while its own input layer finishes starting, and an Enter sent
    /// beside a swallowed prompt submits an EMPTY one — which an agent answers.
    ///
    /// `false` (the default) — the prompt, its Enter and its optional Ctrl-D go in ONE injection.
    /// That is what this adapter has always done and it is right for a peer that renders nothing:
    /// there is no read-back that could succeed, so a retry is not a repair but a SECOND PROMPT,
    /// and splitting the write is not a safeguard but a weld — on a cooked pane the prompt's echo
    /// stops being a whole line and fuses onto whatever the program prints next.
    ///
    /// # ⚠⚠ Why this is declared and not derived
    ///
    /// Two adjacent facts look like they answer it and neither does. [`eof`](Self::eof) is close —
    /// a peer read to end-of-input is usually a one-shot filter that renders nothing — and the
    /// pane's own echo setting is closer still, but a program can have its terminal off echo and
    /// paint nothing until EOF (`stty -echo; in=$(cat); echo "> $in"`), which is a peer that would
    /// be injected into three times by a barrier reading either fact. **That is R358's shape: a
    /// plausible predicate measuring an ADJACENT fact, which passes right up until it does not.**
    /// The pane is the caller's, they know what is in it, and this is the same reason
    /// [`ready_when`](Self::ready_when) is asked for rather than guessed.
    ///
    /// ⚠ Whichever path is taken, what the delivery established is REPORTED: a step whose prompt
    /// could not be confirmed says so in its own note, so a caller reading the capture as a
    /// model's answer is never left to assume the model was asked.
    pub shows_the_prompt: bool,
}

impl AgentSpec {
    /// A spec with the default one-shot behaviour (send EOF, generous timeout).
    #[must_use]
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            eof: true,
            timeout: DEFAULT_REPLY_TIMEOUT,
            ready_when: None,
            ready_within: None,
            // The write, not the delivery: a one-shot peer renders nothing, and this default is
            // what keeps a caller who says nothing on the path their peer can survive.
            shows_the_prompt: false,
        }
    }
}

/// What a step established about its prompt reaching the peer.
///
/// Not [`Delivered`] itself, because that type answers a question only one of this adapter's two
/// paths can ask: it can WITHHOLD the submit, and a peer that renders nothing would never be
/// submitted to at all. So the write path answers in its own terms and both are read in one place.
enum Prompted {
    /// One injection carried the prompt and its submit, with what the pane's terminal was doing
    /// when it landed.
    Written {
        written: u64,
        echo: Option<PaneEcho>,
    },
    /// The prompt was delivered — read back, retried if need be, and submitted only once it was on
    /// the pane.
    Delivered(Delivered),
}

impl Prompted {
    /// How many bytes reached the pseudoterminal — the step's [`Cost`].
    const fn written(&self) -> u64 {
        match self {
            Self::Written { written, .. } => *written,
            Self::Delivered(delivered) => delivered.written().bytes(),
        }
    }

    /// Whether the pane's own TERMINAL painted the prompt back — so whether the capture opens with
    /// an echo this run is responsible for, or with the peer's first line.
    ///
    /// Taken at the moment the prompt was injected, which is the only moment that decides what
    /// happened to those bytes. See [`without_own_echo`], whose named residue this closes.
    ///
    /// ⚠ `None` — a host that offers no such capability, or a platform whose device will not say —
    /// answers **`true`**, which is what this adapter did before the fact existed. It is the
    /// unchanged behaviour rather than the safer-sounding one on purpose: where nothing can be
    /// established, changing the answer would trade a named residue for an unmeasured one.
    const fn echoed_by_the_terminal(&self) -> bool {
        let echo = match self {
            // A confirmed delivery is confirmed BECAUSE the echo was off — the program painted the
            // prompt itself, which is the whole distinction `Delivered::Confirmed` carries.
            Self::Delivered(Delivered::Confirmed { .. }) => return false,
            Self::Delivered(Delivered::OnScreenOnly { echo, .. }) | Self::Written { echo, .. } => {
                *echo
            }
            Self::Delivered(Delivered::Unconfirmed { .. } | Delivered::Stopped { .. }) => {
                // Neither reaches a capture: the step returns before one is taken.
                return true;
            }
        };
        !matches!(echo, Some(PaneEcho::ByTheProgram))
    }

    /// The one line a caller reading a capture AS A MODEL'S ANSWER is owed about whether anything
    /// established that the model was asked — or `None` where the program's own paint proved it.
    ///
    /// ⚠ Constant for a given peer, and deliberately: an unverifiable delivery does not become
    /// verified by being reported often, and a caller publishing an answer is entitled to the
    /// caveat on every one rather than on the interesting ones.
    fn caveat(&self) -> Option<&'static str> {
        let echo = match self {
            Self::Delivered(Delivered::Confirmed { .. }) => return None,
            Self::Delivered(Delivered::OnScreenOnly { echo, .. }) => *echo,
            Self::Written { echo, .. } => *echo,
            // Neither reaches here: the step returns before building a note for both.
            Self::Delivered(Delivered::Unconfirmed { .. } | Delivered::Stopped { .. }) => {
                return None;
            }
        };
        Some(match echo {
            Some(PaneEcho::ByTheTerminal) => {
                "; ⚠ THE PROMPT'S DELIVERY WAS NOT CONFIRMED — this pane's own terminal echoes, so \
                 the prompt appearing on it is the line discipline's doing and not evidence the \
                 peer read it"
            }
            Some(PaneEcho::ByTheProgram) => {
                "; ⚠ THE PROMPT'S DELIVERY WAS NOT CONFIRMED — this peer's terminal is off echo \
                 and it was not asked to show what it was given, so nothing here saw the prompt \
                 arrive"
            }
            None => {
                "; ⚠ THE PROMPT'S DELIVERY WAS NOT CONFIRMED — nothing here can say whether this \
                 pane's terminal or its program puts typed text on the screen"
            }
        })
    }
}

/// Where a turn's reply starts — an ADDRESS when the host can number its lines, and a mark on the
/// rendering when it cannot. See [`Agent::capture`] for what the difference costs.
enum Baseline {
    /// The absolute line number the reply begins at.
    Line(u64),
    /// What the rows held before the prompt — the degradation.
    Rows(RowTrail),
}

/// A one-shot AI-tool adapter over one pane.
pub struct Agent {
    pane: PaneId,
    spec: AgentSpec,
    /// The reply captured this run, surfaced through [`Plugin::captured`].
    response: Option<String>,
    /// The barrier the pane must clear before it is prompted — see [`Readiness`].
    ready: Readiness,
}

impl Agent {
    /// Drive `spec` against `pane`.
    #[must_use]
    pub fn new(pane: PaneId, spec: AgentSpec) -> Self {
        Self {
            ready: Readiness::new(spec.ready_when.clone(), spec.ready_within),
            pane,
            spec,
            response: None,
        }
    }

    /// The keystrokes that SUBMIT a prompt: Enter, then optionally Ctrl-D (EOF) for a
    /// read-until-EOF peer.
    ///
    /// ⚠ On the delivery path these are held back until the prompt is on the pane
    /// ([`Delivery::then_press`](crate::deliver::Delivery::then_press)), which is the ordering that
    /// stops an unread prompt being submitted — and end-of-input'd — as though it had been asked.
    /// The Ctrl-D is the sharper half: a peer reading to end-of-input answers the empty question
    /// and EXITS, which is the very signal [`await_reply`](Self::await_reply) treats as *the reply
    /// is complete*.
    fn submit_keys(&self) -> Vec<KeyStroke> {
        let mut keys = vec![KeyStroke::named("Enter")];
        if self.spec.eof {
            keys.push(KeyStroke {
                key: "d".to_string(),
                mods: Modifiers {
                    ctrl: true,
                    ..Modifiers::default()
                },
            });
        }
        keys
    }

    /// The prompt and its submit as ONE injection — the write path's keystrokes.
    fn prompt_keys(&self) -> Vec<KeyStroke> {
        let mut keys = KeyStroke::text(&self.spec.prompt);
        keys.extend(self.submit_keys());
        keys
    }

    /// Put the prompt in the pane and find out whether it landed, before anything is submitted.
    ///
    /// # ⚠⚠⚠ Why this adapter of all of them owes a DELIVERY rather than a write
    ///
    /// This was `panes.inject(…)` for as long as the adapter has existed — one write, no read-back,
    /// straight into the reply wait — while [`mod@crate::deliver`] sat one module over, written from a
    /// measured failure against a rival multiplexer *for exactly this*: a long-lived agent that is
    /// up, has a tty, reports itself idle, and discards what you type because its own input layer
    /// has not finished starting. **It had no production caller at all.**
    ///
    /// What that cost, measured on a pane that clears its readiness barrier and then consumes the
    /// prompt without acting on it: the run reported `Converged`, charged its bytes, and published
    /// `"REPLY[]"` to its caller **as the model's answer to a question the peer never received**.
    /// Nothing in the outcome, the cost or the note said so.
    ///
    /// # What each answer means here
    ///
    /// * [`Delivered::Confirmed`] — the program painted the prompt, so it has it.
    /// * [`Delivered::OnScreenOnly`] — it is on the screen and the pane's own terminal may be what
    ///   put it there. **Not a failure**: a cooked one-shot peer (`claude -p`) never takes its
    ///   terminal off echo, so this is the strongest claim any reader of a screen can make about
    ///   it, and the run proceeds — saying which it had.
    /// * [`Delivered::Unconfirmed`] — every injection was written and none ever appeared. Nothing
    ///   was submitted, so there is no turn to wait for.
    ///
    /// ⚠ The submit moved INTO the delivery ([`Delivery::then_press`]) and that is the ordering
    /// this buys: `Enter` and the optional `Ctrl-D` are sent only once the prompt is on the pane.
    /// Sent beside a swallowed prompt they submit an EMPTY one — and the `Ctrl-D` is worse than
    /// that, because a peer reading to end-of-input answers the empty question and EXITS, which is
    /// the very signal [`await_reply`](Self::await_reply) treats as *the reply is complete*.
    fn deliver_prompt(
        &self,
        panes: &dyn PaneAccess,
        run: &RunContext,
    ) -> Result<Prompted, PaneError> {
        if !self.spec.shows_the_prompt {
            // The write path, unchanged: one injection carrying the prompt and its submit. What is
            // new is that the pane is ASKED who paints it, so the step can say what its capture is
            // worth instead of publishing it as though the question had been confirmed.
            let written = panes.inject(self.pane, &self.prompt_keys())?.bytes();
            return Ok(Prompted::Written {
                written,
                echo: panes
                    .terminal_modes()
                    .and_then(|modes| modes.pane_echo(self.pane)),
            });
        }
        deliver(
            panes,
            run,
            self.pane,
            &self.spec.prompt,
            &Delivery {
                // ⚠ CONFIRMED ON A LEADING FRAGMENT when the prompt is long, because an agent's
                // prompt box is a BOX: the text wraps inside it and the border lands between the
                // halves, so the pane's collapsed text holds the prompt in pieces. `Delivery`
                // documents this; the number is this adapter's, and it is the point at which a
                // fragment stops being a coincidence.
                confirm: (self.spec.prompt.chars().count() > CONFIRM_WHOLE_UP_TO).then(|| {
                    self.spec
                        .prompt
                        .chars()
                        .take(CONFIRM_WHOLE_UP_TO)
                        .collect::<String>()
                }),
                then_press: self.submit_keys(),
                ..Delivery::new()
            },
        )
        .map(Prompted::Delivered)
    }

    /// Wait (bounded by `timeout`, cancellable) for the pane child to exit —
    /// once it has, its full reply is on screen ([`PaneAccess::pane_eof`]'s
    /// contract). An unknown pane (`None`) counts as done.
    fn await_reply(&self, panes: &dyn PaneAccess, run: &RunContext) -> Waited {
        poll_until(run, self.spec.timeout, || {
            panes.pane_eof(self.pane).unwrap_or(true)
        })
    }

    /// Capture what the pane has produced since `baseline` — the reply region — joined as the
    /// response text.
    ///
    /// # ⚠⚠ Why the reply is addressed by LINE NUMBER
    ///
    /// What this returns is published to the caller AS THE MODEL'S ANSWER, so every way of
    /// mis-reading the pane becomes a lie about what a model said. Two were measured:
    ///
    /// * keyed on each row's DAMAGE GENERATION, a resize stamped every row, so a client merely
    ///   ATTACHING mid-turn made the whole screen — banner, shell prompt and all — come back as
    ///   the reply;
    /// * keyed on row TEXT, that is fixed, but a reply longer than the pane is tall SCROLLS, and
    ///   the rows that left were never in the answer at all. **A truncated reply is worse than a
    ///   missing one, because nothing in it says it is truncated.**
    ///
    /// A LOGICAL LINE is what the tool actually wrote, and numbering those from the pane's birth
    /// makes the baseline an ADDRESS: everything after it is the reply, whether it is still on the
    /// grid or long scrolled into history.
    ///
    /// ⚠ **The unfinished last line is taken here and nowhere else**, because this adapter waits
    /// for the child to EXIT — an unterminated line at EOF is unterminated forever, and a reply
    /// need not end in a newline. On the timeout path it is taken too, which is exactly what that
    /// path's own `PARTIAL` marking already tells the caller.
    ///
    /// ⚠ [`RowTrail`] remains the fallback for a host with no output stream — repaint-proof, not
    /// scroll-proof, and named as a degradation rather than an equivalent.
    fn capture(&self, panes: &dyn PaneAccess, baseline: &Baseline, echoed: bool) -> Captured {
        match baseline {
            Baseline::Line(cursor) => {
                let Some(since) = panes
                    .output_lines()
                    .and_then(|stream| stream.pane_lines_since(self.pane, *cursor))
                else {
                    return Captured::default();
                };
                let mut lines = since.lines;
                if !since.partial.is_empty() {
                    lines.push(since.partial);
                }
                Captured {
                    text: without_own_echo(lines, &self.spec.prompt, echoed).join("\n"),
                    lost: since.lost,
                }
            }
            Baseline::Rows(trail) => Captured {
                text: without_own_echo(trail.fresh(panes, self.pane), &self.spec.prompt, echoed)
                    .join("\n"),
                // ⚠ A rendering comparison cannot report a loss it cannot see — a scrolled-away
                // row is simply not there to be counted. `0` here means UNKNOWN, and it is the
                // degradation this fallback is already named as, not a claim of completeness.
                lost: 0,
            },
        }
    }

    /// Where a turn's reply begins.
    ///
    /// Two shapes because the precise one is a CAPABILITY: a host that can number its lines gives
    /// an address that survives a resize and a scroll, and one that cannot is read by comparing its
    /// rendering. See [`Agent::capture`].
    fn mark(&self, panes: &dyn PaneAccess) -> Baseline {
        panes
            .output_lines()
            // ⚠ `u64::MAX` MARKS WITHOUT TAKING: it is past every line, so nothing is yielded and
            // `next` is the address the reply will start at.
            .and_then(|stream| stream.pane_lines_since(self.pane, u64::MAX))
            .map_or_else(
                || Baseline::Rows(RowTrail::mark(panes, self.pane)),
                |since| Baseline::Line(since.next),
            )
    }
}

/// The reply, and what could not be in it.
///
/// Two fields because a run that answers `converged` with an *"n-character reply"* says the same
/// thing whether the pane's retained history held every line or evicted the first half of the
/// model's answer. **A truncated reply is worse than a missing one, because nothing in it says it
/// is truncated** — this adapter's own doc argued exactly that about the scrolling case and then
/// discarded the field that reports it, while [`crate::pipe`], reading the same stream, put its
/// loss in every note. One reader of a hazard is not a reader of it.
#[derive(Default)]
struct Captured {
    /// The reply as it is published to the caller.
    text: String,
    /// Complete lines the retained history evicted before this capture read them — `0` in the
    /// ordinary case. See [`sprag_vt::LinesSince::lost`].
    lost: u64,
}

/// Drop the leading lines that are exactly the prompt THIS run typed.
///
/// # ⚠⚠ Why the caller's own words came back as the model's
///
/// A pty in cooked mode echoes what is injected, and on the grid that echo is ordinary output — so
/// the first logical line after the prompt's address is the prompt itself. Measured: a run that
/// asked `"summarise the repo"` published `"summarise the repo\nREPLY[summarise the repo]"` to its
/// caller **as the model's answer**. A peer that acts on what it receives acts on a sentence sprag
/// typed.
///
/// # ⚠⚠ EXACT and LEADING, and it stops at the first line that is neither
///
/// The alternative — waiting for the echo and marking after it — is the scheduling-shaped predicate
/// R359c paid to remove: a pty echo is asynchronous, so the same call would strip it or not
/// depending on how loaded the box was. What this run TYPED is known exactly, so the comparison is
/// exact, and the failure direction is chosen: a program that renders input its own way (`> ping`,
/// a REPL's re-draw) matches nothing and keeps every line. **Deleting a line of an answer is worse
/// than leaving a line that was not one**, because only the first is unrecoverable.
///
/// # ⚠⚠⚠ `echoed` — and the residue this used to carry is CLOSED by it
///
/// The filing read: *"a program with its echo OFF whose reply's first line is byte-identical to the
/// prompt loses that line … **the safe reading of the pair does not exist** — one of them has to
/// lose."* That was true of everything this function could see. It is not true of the pane: a
/// terminal publishes whether it echoes, and **if it does not, there is no own-echo on that screen
/// to strip.** Measured before the fix, on a peer that quotes the question back
/// (`stty -echo; …; echo "$in"; echo "REPLY[$in]"`): the run captured `REPLY[ping]` and the model's
/// first line was gone.
///
/// So the strip is now conditional on the fact rather than on a guess:
///
/// * `false` — the pane's program is what paints, so every line on that screen is the peer's and
///   none of them is this run's echo. Nothing is stripped.
/// * `true` — the terminal echoes, which is the case this function was written for.
///
/// ⚠ The reading is the one taken **when the prompt was injected**, carried on [`Prompted`], not
/// one taken at capture time: a program that turns its echo off during startup would otherwise
/// have its own-echo kept, because by the time the reply is read the pty is no longer the thing
/// that painted it. **Measured — the first fixture for this raced its own `stty` and put the
/// prompt on the pane TWICE**, once by each.
fn without_own_echo(lines: Vec<String>, prompt: &str, echoed: bool) -> Vec<String> {
    if !echoed {
        return lines;
    }
    let mut lines = lines.into_iter();
    let mut kept: Vec<String> = Vec::new();
    let mut echo = prompt.split('\n').peekable();
    for line in lines.by_ref() {
        if echo.peek() == Some(&line.as_str()) {
            echo.next();
            continue;
        }
        kept.push(line);
        break;
    }
    kept.extend(lines);
    kept
}

impl Plugin for Agent {
    fn step(&mut self, panes: &dyn PaneAccess, run: &RunContext) -> Result<Step, PaneError> {
        // ⚠⚠ NOT ONE BYTE UNTIL THE PANE IS THE TOOL — see [`AgentSpec::ready_when`], which is
        // where the measured failure is written down. Latched, so it costs nothing after the first
        // step (and this adapter is one-shot anyway).
        if self.ready.reached(panes, self.pane, run)? == Reached::RunEnded {
            return Ok(Step::new(Cost::Bytes(0), Verdict::Continue).noting(
                "the run ended while waiting for the pane to be ready; nothing was asked",
            ));
        }

        // Baseline before acting, so `capture` isolates this prompt's reply (and its cooked-mode
        // echo) from prior content.
        let baseline = self.mark(panes);

        // ⚠⚠⚠ DELIVERED, NOT WRITTEN — see [`Agent::deliver_prompt`]. This was a bare `inject` for
        // as long as this adapter has existed, which meant the one plugin whose output is published
        // AS A MODEL'S ANSWER was the one that never checked its question arrived.
        let prompted = self.deliver_prompt(panes, run)?;
        let cost = prompted.written();
        match prompted {
            // The prompt is in the pane, by whichever route, and something was submitted. What each
            // route established is carried into the note below.
            Prompted::Written { .. }
            | Prompted::Delivered(Delivered::Confirmed { .. } | Delivered::OnScreenOnly { .. }) => {
            }
            // Nothing was submitted (`deliver` withholds the press when the text is demonstrably
            // absent), so there is no turn to wait for and nothing on that screen is this run's.
            // A REFUSAL rather than a converged empty capture: the latter tells a caller the model
            // said nothing, which is the one reading that is both actionable and false.
            Prompted::Delivered(Delivered::Unconfirmed { attempts, written }) => {
                return Err(PaneError::NeverTook {
                    attempts,
                    written: written.bytes(),
                });
            }
            // The run ended under the delivery. Continue, so the Driver's loop top decides whether
            // that was a cancel or the deadline — the same hand-off the reply wait makes below.
            Prompted::Delivered(Delivered::Stopped { .. }) => {
                return Ok(Step::new(Cost::Bytes(cost), Verdict::Continue)
                    .noting("the run ended while delivering the prompt; nothing was asked"));
            }
        }

        let waited = self.await_reply(panes, run);
        // If the RUN ended mid-wait — cancelled, or out of time — don't converge
        // or record a partial reply. Return Continue so the Driver's loop top
        // decides the terminal state, which is the only place that knows whether
        // it was a cancel or the duration ceiling.
        if waited == Waited::Stopped {
            return Ok(Step::new(Cost::Bytes(cost), Verdict::Continue)
                .noting("the run ended while waiting for the reply; nothing captured"));
        }
        let reply = self.capture(panes, &baseline, prompted.echoed_by_the_terminal());
        let text = reply.text;
        // ⚠ THE LENGTH IS THE DIAGNOSTIC. A peer that never answered and one that answered are the
        // same `converged` with the same cost, and an EMPTY capture is what a prompt the peer
        // swallowed looks like from out here.
        //
        // ⚠⚠ AND SO IS WHETHER THE PEER FINISHED. This adapter converges on the child EXITING,
        // which is what makes a capture complete; when the per-turn timeout runs out instead, the
        // text is whatever happened to be on screen mid-reply. Both were reported with the same
        // sentence, so a truncated capture was indistinguishable from a whole one.
        let characters = text.chars().count();
        let mut note = if waited == Waited::TimedOut {
            format!(
                "the peer had not finished after {:?}; captured the {characters} characters on \
                 screen, which may be a PARTIAL reply",
                self.spec.timeout,
            )
        } else {
            format!("captured a {characters}-character reply")
        };
        // ⚠⚠⚠ AND WHEN THE END-OF-INPUT COULD NOT ARRIVE, SAY THAT INSTEAD OF BLAMING THE PEER.
        // This adapter converges on the child EXITING, which is what a peer reading to end-of-input
        // does when it is told the question is over. `Ctrl-D` only tells it that in CANONICAL mode;
        // on a raw terminal it is an ordinary byte, so the wait was for something never asked for.
        // Measured on `stty raw -echo; exec cat` with the default `eof`: the whole reply timeout
        // spent, the peer's echo of the prompt published as the model's answer, and the only
        // explanation offered was *"the peer had not finished"* — a sentence about the PEER's speed
        // for a cause that is the TERMINAL's mode and was knowable before the wait began.
        if waited == Waited::TimedOut && self.spec.eof {
            if let Some(PaneEndOfInput::IsJustAByte) = panes
                .terminal_modes()
                .and_then(|modes| modes.pane_end_of_input(self.pane))
            {
                note.push_str(
                    "; ⚠ AND THE END-OF-INPUT NEVER ARRIVED — this pane's terminal is not in \
                     canonical mode, so the Ctrl-D this run sent is an ordinary byte and a peer \
                     reading until end-of-input was never told the question was over",
                );
            }
        }
        // ⚠⚠ A HOLE IN THE ANSWER IS REPORTED, NEVER SWALLOWED. The pane's retained history is
        // bounded, so a reply that outran it between the prompt and the read has lines nothing can
        // recover — and a silent gap is indistinguishable from a model that said less. This is the
        // half [`crate::pipe`] already reported and this adapter, whose text is published AS THE
        // MODEL'S ANSWER, dropped.
        if reply.lost > 0 {
            note.push_str(&format!(
                "; {} EARLIER LINES ARE MISSING FROM IT — the reply outran the pane's retained \
                 history",
                reply.lost,
            ));
        }
        // ⚠⚠ AND WHAT THE QUESTION'S DELIVERY WAS WORTH, on every step that produced a reply. A
        // caller reading a capture as a model's answer is entitled to know whether anything
        // established that the model was asked: on a pane whose own terminal echoes, the prompt
        // appearing there is the line discipline's doing and not the peer's. Silent, this is the
        // shape R363 paid for twice — a fact that reaches the wire and dies at the mouth.
        if let Some(caveat) = prompted.caveat() {
            note.push_str(caveat);
        }
        self.response = Some(text);

        // One-shot: one prompt, one captured reply, then converge. The Driver's
        // guardrails still bound it; `timeout` (above) bounds a non-exiting peer.
        Ok(Step::new(Cost::Bytes(cost), Verdict::Converged).noting(note))
    }

    fn captured(&self) -> Option<String> {
        self.response.clone()
    }

    /// THE PANE THIS PROMPTED, so a run cut short takes the model's turn down with it.
    ///
    /// The case this whole mechanism exists for: a step types a prompt and then blocks for up to
    /// [`DEFAULT_REPLY_TIMEOUT`] waiting for a model to think. A
    /// cancel or a passed deadline lands INSIDE that wait, and before the Driver could act on this
    /// the run ended while the model went on answering a question nobody was listening to — still
    /// billed, still holding the pane.
    ///
    /// ⚠ Answered unconditionally rather than only while a reply is outstanding, because a
    /// `SIGINT` to a peer that has already finished is what its own idle prompt absorbs, and the
    /// alternative — tracking in-flight-ness here — would put a second copy of *"is this turn
    /// over?"* beside the one [`step`](Self::step) already keeps, to be got wrong exactly when it
    /// matters.
    fn driving(&self) -> Option<PaneId> {
        Some(self.pane)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::WorkspacePaneAccess;
    use crate::driver::{Ceiling, Driver, Guardrails, Outcome, OutcomeState};
    use crate::testing::{REAP_THE_STANDIN, STANDIN_READS_TTY, started};
    use sprag_terminal::{CommandBuilder, Workspace};
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    /// A workspace with one pane running `script`, wrapped as pane-access.
    fn sh_access(script: &str, cols: u16, rows: u16) -> (WorkspacePaneAccess, PaneId) {
        let workspace = Arc::new(Mutex::new(Workspace::new((cols, rows))));
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(script);
        command.env("TERM", "dumb");
        let id = workspace
            .lock()
            .unwrap()
            .spawn(command, "sh".to_string(), cols, rows)
            .expect("spawn pane");
        (WorkspacePaneAccess::new(workspace), id)
    }

    fn run(access: &WorkspacePaneAccess, agent: &mut Agent) -> Outcome {
        Driver::new(Guardrails {
            max_iterations: 4,
            max_cost: None,
            max_duration: None,
        })
        .run(agent, access, &RunContext::uncancellable())
    }

    #[test]
    fn converges_and_captures_a_reply() {
        // A one-shot fake AI: read the prompt (until EOF), reply deterministically.
        let (access, pane) = sh_access("in=$(cat); echo \"REPLY[$in]\"", 40, 6);
        let mut agent = Agent::new(pane, AgentSpec::new("ping"));

        let outcome = run(&access, &mut agent);

        assert_eq!(outcome.state, OutcomeState::Converged);
        // ⚠ EQUALITY. This read `contains("REPLY[ping]")` and passed for as long as the capture
        // carried the prompt's own echo welded to its front: a containment check cannot see what a
        // capture has TOO MUCH of, and too much is the shape that publishes sprag's words as a
        // model's.
        assert_eq!(agent.captured().expect("a captured reply"), "REPLY[ping]",);
    }

    /// ⚠⚠⚠ **WHAT THIS RUN TYPED IS NOT WHAT THE MODEL SAID**, and it was published as if it were.
    ///
    /// A pty in cooked mode echoes an injection, and on the grid that echo is ordinary output — so
    /// the first logical line after the prompt's address is the prompt. Measured before the fix:
    /// `"summarise the repo\nREPLY[summarise the repo]"` reached the caller as the agent's answer.
    /// A relay hands that to a peer that ACTS on what it receives, so sprag's own words become an
    /// instruction somebody follows.
    ///
    /// ⚠ EQUALITY, not `contains`. The gate beside this one asserted `contains("REPLY[ping]")` and
    /// passed throughout — a capture with the prompt welded to its front contains the reply too.
    /// **A containment check cannot see what a capture has too much of.**
    #[test]
    fn a_reply_is_what_the_peer_said_and_not_the_prompt_this_run_typed() {
        let (access, pane) = sh_access("in=$(cat); echo \"REPLY[$in]\"", 40, 6);
        let mut agent = Agent::new(pane, AgentSpec::new("summarise the repo"));

        let outcome = run(&access, &mut agent);

        assert_eq!(outcome.state, OutcomeState::Converged);
        assert_eq!(
            agent.captured().expect("a captured reply"),
            "REPLY[summarise the repo]",
            "the whole capture is the peer's answer — the prompt's own echo is this run's, and \
             publishing it makes sprag's words a model's",
        );
    }

    /// ⚠⚠ **AND A LINE THAT IS NOT THE ECHO IS KEPT, however much it looks like one.**
    ///
    /// The other direction of the same rule, and the one that decides which way the fix fails. A
    /// program with its echo OFF that RENDERS the input its own way — `> ping`, every REPL — must
    /// keep that line: it is the peer's output, and deleting a line of an answer is unrecoverable
    /// while leaving one that was not an answer is merely noise.
    ///
    /// The fixture turns the pty's echo off, so the only text on the pane is the program's.
    #[test]
    fn a_program_that_renders_the_prompt_its_own_way_keeps_that_line() {
        let (access, pane) = sh_access(
            "stty -echo; in=$(cat); echo \"> $in\"; echo \"REPLY[$in]\"",
            40,
            6,
        );
        let mut agent = Agent::new(pane, AgentSpec::new("ping"));

        let outcome = run(&access, &mut agent);

        assert_eq!(outcome.state, OutcomeState::Converged);
        assert_eq!(
            agent.captured().expect("a captured reply"),
            "> ping\nREPLY[ping]",
            "an EXACT leading match is the echo and nothing else is — a program's own rendering \
             of the prompt is output, and stripping it would delete an answer's first line",
        );
    }

    /// ⚠⚠ **A HOLE IN THE ANSWER IS REPORTED** — the field [`crate::pipe`] reads and this adapter
    /// dropped.
    ///
    /// The pane's retained history is bounded, so a reply that outran it has lines nothing can
    /// recover. Both cases answered `converged` with the same *"captured an n-character reply"*,
    /// which makes a truncated model answer indistinguishable from a short one — the exact
    /// confusion this adapter's own `capture` doc argues against, two paragraphs above the code
    /// that discarded `lost`.
    ///
    /// Driven through a stream that REPORTS a loss, because a bounded history cannot be overrun on
    /// demand without making the gate a scrolling fixture rather than a claim about the report.
    #[test]
    fn a_reply_that_outran_the_pane_s_history_says_how_much_is_missing() {
        struct Lossy;
        impl PaneAccess for Lossy {
            fn pane_ids(&self) -> Vec<PaneId> {
                vec![PaneId(1)]
            }
            fn pane_collapsed(&self, _id: PaneId) -> Option<String> {
                Some(String::new())
            }
            fn pane_rows(&self, _id: PaneId) -> Option<Vec<crate::access::PaneRow>> {
                Some(Vec::new())
            }
            fn pane_eof(&self, _id: PaneId) -> Option<bool> {
                Some(true)
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
                    lines: vec!["the tail of the answer".to_string()],
                    next: 10,
                    lost: 7,
                    partial: String::new(),
                })
            }
        }

        let step = Agent::new(PaneId(1), AgentSpec::new("ping"))
            .step(&Lossy, &RunContext::uncancellable())
            .expect("the turn");
        let said = step.note.unwrap_or_default();
        assert!(
            said.contains('7') && said.contains("MISSING"),
            "the turn must say HOW MANY lines of the answer it never saw, or a truncated reply is \
             published as a whole one: {said:?}",
        );
    }

    /// The cases [`without_own_echo`] decides, without a pty in the way.
    #[test]
    fn an_echo_is_dropped_only_where_it_leads_and_only_where_it_matches() {
        let lines = |all: &[&str]| all.iter().map(|l| (*l).to_string()).collect::<Vec<_>>();

        // ⚠⚠⚠ THE FIRST CASE IS THE PANE'S ANSWER, not a line's shape: with the program painting,
        // there is no echo on that screen at all, so the strongest-looking match is still the
        // model's. Every case below it assumes a terminal that echoes.
        assert_eq!(
            without_own_echo(lines(&["ask", "answer"]), "ask", false),
            lines(&["ask", "answer"]),
            "a pane whose PROGRAM paints has nothing of this run's on it to strip",
        );

        assert_eq!(
            without_own_echo(lines(&["ask", "answer"]), "ask", true),
            lines(&["answer"]),
            "the leading echo of a one-line prompt",
        );
        assert_eq!(
            without_own_echo(lines(&["one", "two", "answer"]), "one\ntwo", true),
            lines(&["answer"]),
            "and of a prompt with newlines in it, line for line",
        );
        assert_eq!(
            without_own_echo(lines(&["answer", "ask"]), "ask", true),
            lines(&["answer", "ask"]),
            "⚠⚠ NOT A LINE THAT MERELY EQUALS THE PROMPT — only the LEADING one is the echo, and \
             a model that quotes the question mid-answer is quoting it",
        );
        assert_eq!(
            without_own_echo(lines(&["ask"]), "ask", true),
            Vec::<String>::new(),
            "a peer that answered nothing leaves an EMPTY capture, which is the diagnostic the \
             step's character count exists to publish — not a capture of this run's own prompt",
        );
        assert_eq!(
            without_own_echo(lines(&["answer"]), "", true),
            lines(&["answer"]),
            "and a prompt with nothing in it consumes nothing",
        );
    }

    /// ⚠⚠ **AN AGENT RUN AGAINST A PANE THAT IS STILL A SHELL MUST NOT REPORT THE SHELL'S OUTPUT
    /// AS THE MODEL'S REPLY** — the worst shape this defect takes, because it is a WRONG ANSWER
    /// rather than a missing one.
    ///
    /// The subject half is what a caller gets today with no barrier, and it is not a hang or a
    /// failure: the shell runs the prompt as a command, the trailing Ctrl-D makes it EXIT, and
    /// exiting is precisely the completion signal this adapter converges on. So the run reports
    /// SUCCESS and hands back `"summarise the repo\n$ sh: 1: summarise: not found\n$"` as the
    /// agent's answer. Nothing in the state, the cost or the note says otherwise.
    ///
    /// With the barrier the run waits for the tool to announce itself and asks the TOOL, so the
    /// captured text is the tool's. Both halves against the same pane script, because the claim is
    /// about the barrier and not about the fixture.
    #[test]
    fn an_agent_waits_for_the_tool_rather_than_prompting_the_shell_that_is_still_there() {
        // A pane that is a shell for a moment and then becomes the "tool": it announces itself and
        // execs a one-shot that reads until EOF and answers. The stand-in shell EATS what it is
        // given (see `STANDIN_READS_TTY`) — an un-eaten prompt would sit in the pty and be read by
        // the tool anyway, and this gate would pass without a barrier.
        // ⚠⚠⚠ `kill` AND THEN REAP, and the reap is the whole fix. `kill` only DELIVERS a signal;
        // the shell runs on without the reader being gone, so `TOOL-UP` could be printed while the
        // stand-in was still parked in a one-byte `read` on this pane's tty. Under whole-suite load
        // it was: the barrier cleared, the prompt was injected, and the dying reader took its FIRST
        // BYTE — `REPLY[ummarise the repo]`, a whole run answering a question nobody asked. Forced
        // deterministically in `a_reader_that_outlives_the_barrier_steals_the_prompts_first_byte`.
        let script = format!(
            "while read early; do echo \"SHELL-ATE $early\"; done {STANDIN_READS_TTY} & \
             sleep 1; {REAP_THE_STANDIN} printf 'TOOL-UP\\n'; \
             exec sh -c 'in=$(cat); echo \"REPLY[$in]\"'"
        );
        let (access, pane) = sh_access(&script, 40, 8);
        let mut agent = Agent::new(
            pane,
            AgentSpec {
                ready_when: Some(ReadyWhen::Prints("TOOL-UP".to_string())),
                ..AgentSpec::new("summarise the repo")
            },
        );

        let outcome = Driver::new(Guardrails {
            max_iterations: 4,
            max_cost: None,
            max_duration: Some(Duration::from_secs(20)),
        })
        .run(&mut agent, &access, &RunContext::uncancellable());

        assert_eq!(outcome.state, OutcomeState::Converged);
        let captured = agent.captured().expect("a captured reply");
        assert!(
            captured.contains("REPLY[summarise the repo]"),
            "the reply must be the TOOL's: {captured:?}",
        );
        assert!(
            !captured.contains("not found") && !captured.contains("SHELL-ATE"),
            "and it must carry no trace of the shell that was there first — a caller reading this \
             as the model's answer would be reading a shell error: {captured:?}",
        );
    }

    /// A peer whose terminal is RAW with echo off — an interactive agent's shape — that discards
    /// the first four bytes it is given and paints everything after them.
    ///
    /// `dd` is what makes the swallow exact rather than something to wait for: the first injection
    /// of `ping` is ALWAYS lost and everything after it is ALWAYS painted. The announcement comes
    /// after the `stty`, so nothing below races the peer's own configuration (R347).
    const SWALLOWS_THE_FIRST_PROMPT: &str =
        "stty raw -echo; printf 'UP\\r\\n'; dd bs=1 count=4 of=/dev/null 2>/dev/null; exec cat";

    /// ⚠⚠⚠ **A PROMPT THE PEER SWALLOWED IS RE-ASKED — AND ON THE WRITE PATH IT IS SIMPLY GONE.**
    ///
    /// This is the failure [`mod@crate::deliver`] was written from, measured against a rival
    /// multiplexer, and for as long as this adapter has existed it did not use it: the one plugin
    /// whose capture is published AS A MODEL'S ANSWER wrote its question once and never looked.
    ///
    /// Both halves against ONE fixture, which is what makes it a measurement rather than two
    /// stories. The peer eats exactly four bytes — one `ping` — and paints the rest:
    ///
    /// * on the WRITE path the whole injection is `ping` + Enter, the peer eats the prompt and is
    ///   left holding a bare carriage return, and **the word never reaches the pane at all**;
    /// * on the DELIVERY path the first injection is eaten, the read-back finds nothing, and the
    ///   re-injection lands — so the peer ends up holding the question it was asked.
    ///
    /// ⚠ The control is the write path, and it is the half that can fail: if the peer painted the
    /// prompt either way, the retry would be measuring nothing.
    #[test]
    fn a_peer_that_swallows_the_first_prompt_is_asked_again_and_a_bare_write_is_not() {
        /// The pane's own text once the turn is over, for a peer that paints what it is given.
        fn asked(shows_the_prompt: bool) -> String {
            let (access, pane) = sh_access(SWALLOWS_THE_FIRST_PROMPT, 40, 8);
            let mut agent = Agent::new(
                pane,
                AgentSpec {
                    // `cat` never exits, so the turn ends on its own clock; what this gate reads is
                    // what the PEER was given, not what it answered.
                    eof: false,
                    timeout: Duration::from_millis(400),
                    ready_when: Some(ReadyWhen::Prints("UP".to_string())),
                    shows_the_prompt,
                    ..AgentSpec::new("ping")
                },
            );
            let outcome = Driver::new(Guardrails {
                max_iterations: 1,
                max_cost: None,
                max_duration: Some(Duration::from_secs(30)),
            })
            .run(&mut agent, &access, &RunContext::uncancellable());
            assert_eq!(outcome.state, OutcomeState::Converged, "{outcome:?}");
            let screen = access.pane_collapsed(pane).expect("a live pane");
            access.lifecycle().expect("lifecycle").close(pane);
            screen
        }

        // THE CONTROL. One injection, the peer eats it, and nothing downstream can tell.
        let written = asked(false);
        assert!(
            !written.contains("ping"),
            "the control must really lose the prompt, or the delivery below proves nothing: \
             {written:?}",
        );

        // THE SUBJECT. Same peer, same swallow, and the question arrives.
        let delivered = asked(true);
        assert!(
            delivered.contains("ping"),
            "a prompt the peer swallowed has to be asked again — this is the whole reason a \
             delivery is not a write: {delivered:?}",
        );
    }

    /// ⚠⚠⚠ **A MODEL THAT OPENS BY QUOTING THE QUESTION KEEPS THAT LINE** — the residue
    /// [`without_own_echo`] carried as unclosable, closed by asking the pane who echoes.
    ///
    /// The filing said *"the safe reading of the pair does not exist — one of them has to lose"*,
    /// and it was right about everything the STRIP could see. It was wrong about the pane: a
    /// terminal publishes whether it echoes, so *"is this line mine or the peer's?"* stopped being
    /// a guess.
    ///
    /// Both peers say the same words and differ only in who painted them, which is what makes this
    /// a measurement of the fact rather than of the fixture:
    ///
    /// * ECHO OFF — the peer prints the question back itself, and the only `ping` on that screen is
    ///   the model's. Before the fix this run captured `REPLY[ping]` and **the model's first line
    ///   was deleted**; deleting a line of an answer is the unrecoverable direction.
    /// * ECHO ON — nothing but the line discipline puts the prompt there, and it must still be
    ///   stripped. That is R362's defect (sprag publishing its own words as a model's) and this
    ///   round must not trade one for the other.
    ///
    /// ⚠ Each peer announces AFTER configuring its terminal, so neither races its own `stty` — the
    /// first draft of the echo-off arm did, and put `ping` on the pane twice, once by each painter.
    #[test]
    fn a_reply_that_opens_by_quoting_the_question_keeps_that_line() {
        /// What one turn publishes as the model's answer, against a peer that quotes the question
        /// back before answering it.
        fn captured(configure: &str) -> String {
            let script =
                format!("{configure} printf 'UP\\n'; in=$(cat); echo \"$in\"; echo \"REPLY[$in]\"");
            let (access, pane) = sh_access(&script, 40, 8);
            let mut agent = Agent::new(
                pane,
                AgentSpec {
                    ready_when: Some(ReadyWhen::Prints("UP".to_string())),
                    ..AgentSpec::new("ping")
                },
            );
            let outcome = Driver::new(Guardrails {
                max_iterations: 1,
                max_cost: None,
                max_duration: Some(Duration::from_secs(30)),
            })
            .run(&mut agent, &access, &RunContext::uncancellable());
            assert_eq!(outcome.state, OutcomeState::Converged, "{outcome:?}");
            let text = agent.captured().expect("a captured reply");
            access.lifecycle().expect("lifecycle").close(pane);
            text
        }

        assert_eq!(
            captured("stty -echo;"),
            "ping\nREPLY[ping]",
            "the peer painted that `ping` — its terminal echoes nothing — so it is the model's \
             first line, and a run that deletes it hands its caller a truncated answer with \
             nothing in it saying so",
        );
        // THE OTHER DIRECTION, and it is the one this fix could have broken: with the terminal
        // echoing, the identical `ping` is sprag's own and republishing it is R362's defect.
        assert_eq!(
            captured(""),
            "ping\nREPLY[ping]",
            "with the terminal echoing there are TWO `ping`s — the line discipline's and the \
             peer's — and exactly one of them may survive",
        );
    }

    /// ⚠⚠⚠ **A RUN THAT SENT AN END-OF-INPUT THE TERMINAL CANNOT DELIVER SAYS SO** — instead of
    /// spending its whole wait and then blaming the peer's speed for it.
    ///
    /// This adapter converges on the pane's child EXITING, which is what a peer reading to
    /// end-of-input does once it is told the question is over. `Ctrl-D` tells it that **only in
    /// canonical mode**; a program that took its terminal raw — every full-screen agent — receives
    /// an ordinary byte. So the run waits for something it never asked for, and the sentence it
    /// offered was *"the peer had not finished after 120s"*: about the PEER's speed, for a cause
    /// that is the TERMINAL's mode and is knowable before the wait begins.
    ///
    /// Both arms against peers that differ ONLY in that mode, because the sentence is worth
    /// nothing if it fires for everyone:
    ///
    /// * RAW — the byte is just a byte, the turn ends on its clock, and the run names the cause.
    /// * CANONICAL — the same `Ctrl-D` is an end-of-input, the peer really does finish, and there
    ///   is no such sentence to say.
    #[test]
    fn a_run_says_when_its_end_of_input_could_not_reach_the_peer() {
        /// The note of one turn against `script`, with the adapter's default `eof`.
        fn note_for(script: &str) -> String {
            let (access, pane) = sh_access(script, 40, 8);
            // ⚠ The peer is UP before the run starts, so the run's clock is spent on the turn and
            // not on a loaded box's process startup — see `started`.
            started(&access, pane, "UP");
            let cell = crate::driver::ProgressCell::default();
            // ⚠ NO `ready_when`: `started` above already established the peer is up, and
            // `ReadyWhen::Prints` asks for output printed AFTER the barrier begins looking — so a
            // barrier here would wait for a second announcement that never comes. One wait, on the
            // observable fact, in the fixture (R359b's distinction, met from the other side).
            let mut agent = Agent::new(
                pane,
                AgentSpec {
                    timeout: Duration::from_millis(400),
                    ..AgentSpec::new("ping")
                },
            );
            let outcome = Driver::new(Guardrails {
                max_iterations: 1,
                max_cost: None,
                max_duration: Some(Duration::from_secs(30)),
            })
            .reporting_to(Arc::clone(&cell))
            .run(&mut agent, &access, &RunContext::uncancellable());
            assert_eq!(outcome.state, OutcomeState::Converged, "{outcome:?}");
            let note = cell
                .lock()
                .expect("the progress cell")
                .journal
                .last()
                .and_then(|step| step.note.clone())
                .unwrap_or_default();
            access.lifecycle().expect("lifecycle").close(pane);
            note
        }

        // RAW: `cat` never sees an end-of-input, so the turn can only end on the clock.
        let raw = note_for("stty raw -echo; printf 'UP\\r\\n'; exec cat");
        assert!(
            raw.contains("THE END-OF-INPUT NEVER ARRIVED") && raw.contains("canonical"),
            "a run whose Ctrl-D could not be an end-of-input must name that, or its caller reads a \
             wrong diagnosis of a knowable fact: {raw:?}",
        );

        // CANONICAL: the same keystroke IS an end-of-input, so the peer finishes and answers.
        let cooked = note_for("printf 'UP\\n'; in=$(cat); echo \"REPLY[$in]\"");
        assert!(
            !cooked.contains("END-OF-INPUT"),
            "and it must NOT be said where the terminal delivers it — a caveat that fires for \
             every peer is not a diagnosis: {cooked:?}",
        );
        assert!(
            cooked.contains("captured a"),
            "the control has to be a turn that really completed, or its silence proves nothing: \
             {cooked:?}",
        );
    }

    /// ⚠⚠⚠ **A READER THAT OUTLIVES THE READINESS BARRIER STEALS THE PROMPT'S FIRST BYTE** — the
    /// mechanism behind a load-marginal gate, forced into one deterministic run.
    ///
    /// `an_agent_waits_for_the_tool_…` failed under whole-suite load with a capture of
    /// `REPLY[ummarise the repo]` — a CORRECTNESS symptom, not a timing one, filed with its cause
    /// open. The cause is that signalling a process is not the same as it being gone, so a stand-in
    /// parked in a one-byte `read` on the pane's tty is still there when the barrier clears, and
    /// the very next thing that happens is the injection.
    ///
    /// Both arms against one shape, because the claim is about the REAP and not about `dd`:
    ///
    /// * THE CONTROL — a reader nobody reaps takes exactly one byte, and the peer answers a
    ///   question this run did not ask. ⚠ It must carry [`STANDIN_READS_TTY`]: a background job of
    ///   a non-interactive shell reads `/dev/null`, and without the redirect this arm eats nothing
    ///   and passes for the wrong reason. **Measured — the first draft of this test did exactly
    ///   that and reported a whole capture.**
    /// * THE SUBJECT — the same reader, reaped ([`REAP_THE_STANDIN`]), cannot be holding a read
    ///   when the prompt arrives, so the question reaches the peer whole.
    #[test]
    fn a_reader_that_outlives_the_barrier_steals_the_prompts_first_byte() {
        /// One turn against a pane whose stand-in reader is dealt with by `how`, answering what the
        /// peer was actually asked.
        fn asked(how: &str) -> String {
            let script = format!(
                "dd bs=1 count=1 of=/dev/null 2>/dev/null {STANDIN_READS_TTY} & \
                 sleep 0.3; {how} printf 'TOOL-UP\\n'; \
                 exec sh -c 'in=$(cat); echo \"REPLY[$in]\"'"
            );
            let (access, pane) = sh_access(&script, 40, 8);
            let mut agent = Agent::new(
                pane,
                AgentSpec {
                    ready_when: Some(ReadyWhen::Prints("TOOL-UP".to_string())),
                    ..AgentSpec::new("summarise the repo")
                },
            );
            let outcome = Driver::new(Guardrails {
                max_iterations: 1,
                max_cost: None,
                max_duration: Some(Duration::from_secs(30)),
            })
            .run(&mut agent, &access, &RunContext::uncancellable());
            assert_eq!(outcome.state, OutcomeState::Converged, "{outcome:?}");
            let captured = agent.captured().expect("a captured reply");
            access.lifecycle().expect("lifecycle").close(pane);
            captured
        }

        assert_eq!(
            asked(""),
            "REPLY[ummarise the repo]",
            "the control must really lose the first byte, or the subject below proves nothing — \
             and this is the shape a whole-suite run hit for real",
        );
        assert_eq!(
            asked(REAP_THE_STANDIN),
            "REPLY[summarise the repo]",
            "a barrier may only clear once the reader that was there before it is GONE — \
             signalling it is an act, and what a fixture must wait for is the fact",
        );
    }

    /// ⚠⚠⚠ **A CAPTURE FROM A COOKED PANE SAYS THAT NOTHING CONFIRMED THE QUESTION WAS ASKED.**
    ///
    /// The measurement this whole round started from. The pane clears its readiness barrier, then
    /// consumes exactly the prompt and its Enter without acting on them, and the peer behind it
    /// answers the empty question it was left with. The run reported `Converged`, charged its
    /// bytes, and published **`"REPLY[]"` to its caller as the model's answer** — with nothing in
    /// the outcome, the cost or the note to say the peer had never been asked.
    ///
    /// ⚠⚠ AND IT STILL PUBLISHES IT, because on a pane whose own terminal echoes there is no
    /// observation that would say otherwise: the line discipline paints the prompt on receipt, so
    /// the screen shows the question whether or not anything read it, and a peer that answers an
    /// empty question is byte-for-byte a peer that answered a real one. **What changed is that the
    /// caller is told which of those they have.** That is the honest fix and the whole of it; a
    /// gate claiming detection here would be claiming an observation nothing can make.
    ///
    /// ⚠ Both halves. The caveat has to name the TERMINAL — a caveat that fired for every peer
    /// would be a constant, and the same run against a peer whose program paints its own prompt
    /// carries no caveat at all (`a_peer_that_swallows_…` drives that side).
    #[test]
    fn a_reply_from_a_pane_whose_terminal_echoes_is_published_with_that_said() {
        // `dd` discards exactly `summarise the repo` and its Enter — 19 bytes — and the tool behind
        // it then reads what is left, which is the end-of-input the same injection carried.
        let script = "printf 'TOOL-UP\\n'; dd bs=1 count=19 of=/dev/null 2>/dev/null; \
                      exec sh -c 'in=$(cat); echo \"REPLY[$in]\"'";
        let (access, pane) = sh_access(script, 40, 8);
        let mut agent = Agent::new(
            pane,
            AgentSpec {
                ready_when: Some(ReadyWhen::Prints("TOOL-UP".to_string())),
                ..AgentSpec::new("summarise the repo")
            },
        );
        // ⚠ REPORTED TO, because a note nobody publishes is not a note: `Step::note` reaches a
        // caller only through the run's journal, and this is the seam the host reads it from.
        let cell = crate::driver::ProgressCell::default();
        let outcome = Driver::new(Guardrails {
            max_iterations: 1,
            max_cost: None,
            max_duration: Some(Duration::from_secs(30)),
        })
        .reporting_to(Arc::clone(&cell))
        .run(&mut agent, &access, &RunContext::uncancellable());

        assert_eq!(outcome.state, OutcomeState::Converged, "{outcome:?}");
        // THE CONTROL, and it is what makes the caveat worth anything: the peer really did answer a
        // question it was never asked, and this run really did publish that answer.
        assert_eq!(
            agent.captured().as_deref(),
            Some("REPLY[]"),
            "the fixture must reproduce the silent loss, or the caveat below is decoration",
        );
        let said = cell
            .lock()
            .expect("the progress cell")
            .journal
            .last()
            .and_then(|step| step.note.clone())
            .unwrap_or_default();
        assert!(
            said.contains("NOT CONFIRMED") && said.contains("terminal echoes"),
            "a caller reading that capture as a model's answer has to be told that nothing \
             established the model was asked, and WHY nothing could: {said:?}",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠⚠ **A PANE THAT NEVER TAKES THE PROMPT IS A REFUSAL, NOT AN EMPTY ANSWER.**
    ///
    /// The peer reads its input into `/dev/null` and paints nothing, so no number of injections
    /// will ever show up. What must NOT happen is the thing that happened before: converge, charge
    /// the bytes, and hand the caller an empty capture — because an empty capture is a sentence
    /// about the MODEL (*it said nothing*) and this is a sentence about the PANE.
    ///
    /// ⚠ The second half is the one that matters more: the submit is WITHHELD. An Enter beside a
    /// prompt that is not there submits an empty one, and the `Ctrl-D` that would follow it makes a
    /// read-to-end-of-input peer answer that empty question AND EXIT — which is the exact signal
    /// this adapter treats as *the reply is complete*.
    #[test]
    fn a_pane_that_never_shows_the_prompt_refuses_instead_of_publishing_an_empty_reply() {
        let (access, pane) = sh_access(
            "stty raw -echo; printf 'UP\\r\\n'; exec cat >/dev/null",
            40,
            8,
        );
        let mut agent = Agent::new(
            pane,
            AgentSpec {
                eof: false,
                timeout: Duration::from_millis(200),
                ready_when: Some(ReadyWhen::Prints("UP".to_string())),
                shows_the_prompt: true,
                ..AgentSpec::new("ping")
            },
        );
        let outcome = Driver::new(Guardrails {
            max_iterations: 1,
            max_cost: None,
            max_duration: Some(Duration::from_secs(60)),
        })
        .run(&mut agent, &access, &RunContext::uncancellable());

        assert_eq!(outcome.state, OutcomeState::Failed, "{outcome:?}");
        // ⚠ THE SENTENCE, not the variant: what a caller reads is this type's `Display`, and that
        // is the only rendering standing between an agent and a debug dump.
        let said = outcome
            .failure
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default();
        assert!(
            said.contains("never took the prompt") && said.contains("nothing was submitted"),
            "the failure has to name the pane rather than the model, in the sentence a caller \
             reads: {said:?}",
        );
        assert!(
            agent.captured().is_none(),
            "and nothing may be published as a reply to a question that was never asked: {:?}",
            agent.captured(),
        );
        // THE WITHHELD SUBMIT. Everything written into the pane is in its trail, and a run that had
        // pressed Enter would have put a carriage return there.
        let trail = access
            .input_echo()
            .expect("this host records a trail")
            .pane_recent_input(pane)
            .expect("a live pane");
        assert!(
            !trail.contains('\r'),
            "the submit must be withheld when the prompt is not there — a `\\r` in the trail is an \
             empty prompt submitted: {trail:?}",
        );
        access.lifecycle().expect("lifecycle").close(pane);
    }

    /// ⚠⚠ **A HOST WITH NO OUTPUT STREAM STILL CAPTURES A REPLY** — the degradation arm, which no
    /// gate built.
    ///
    /// [`PaneAccess::output_lines`] is optional, so a build without it falls back to comparing the
    /// RENDERING ([`RowTrail`]). That fallback is named as a degradation — it cannot see a line
    /// that scrolled away — and a degradation that returned NOTHING would not be one: this adapter
    /// publishes what it captures as the model's answer, so an empty capture is a silent failure
    /// wearing the shape of a reply.
    ///
    /// ⚠ Both halves: the reply comes back, and the text that was on the pane BEFORE the prompt
    /// does not — a fallback that returned the whole screen would pass the first alone.
    #[test]
    fn a_host_with_no_output_stream_still_captures_by_the_rendering() {
        /// A pane that answers when typed at, with every optional capability at its default.
        struct NoStream(Mutex<Vec<String>>);
        impl PaneAccess for NoStream {
            fn pane_ids(&self) -> Vec<PaneId> {
                vec![PaneId(1)]
            }
            fn pane_collapsed(&self, _id: PaneId) -> Option<String> {
                Some(self.0.lock().unwrap().join(""))
            }
            fn pane_rows(&self, _id: PaneId) -> Option<Vec<crate::access::PaneRow>> {
                Some(
                    self.0
                        .lock()
                        .unwrap()
                        .iter()
                        .map(|text| crate::access::PaneRow {
                            generation: 1,
                            text: text.clone(),
                        })
                        .collect(),
                )
            }
            fn pane_eof(&self, _id: PaneId) -> Option<bool> {
                Some(true)
            }
            fn pane_full_text(&self, id: PaneId) -> Option<String> {
                self.pane_collapsed(id)
            }
            fn inject(
                &self,
                _id: PaneId,
                _keys: &[KeyStroke],
            ) -> Result<crate::access::Written, PaneError> {
                // ⚠ THE ECHO FIRST, because a pty in cooked mode puts it there before the program
                // has read a byte. A fake that skips it makes the degradation arm look cleaner
                // than the hosts that actually take it — and left `without_own_echo` on this path
                // built by nothing, which is how the same defect comes back through the fallback.
                let mut screen = self.0.lock().unwrap();
                screen.push("ask".to_string());
                screen.push("REPLY-BY-ROWS".to_string());
                Ok(crate::access::Written::of(4))
            }
        }

        let access = NoStream(Mutex::new(vec!["banner".to_string()]));
        let mut agent = Agent::new(PaneId(1), AgentSpec::new("ask"));
        let outcome = Driver::new(Guardrails {
            max_iterations: 1,
            max_cost: None,
            max_duration: Some(Duration::from_secs(5)),
        })
        .run(&mut agent, &access, &RunContext::uncancellable());
        assert_eq!(outcome.state, OutcomeState::Converged, "{outcome:?}");
        assert_eq!(
            agent.captured().as_deref(),
            Some("REPLY-BY-ROWS"),
            "the fallback must return the reply — and only the reply: `banner` was on the pane \
             before the prompt and `ask` is this run's own echo, and neither is what the model \
             said",
        );
    }

    /// ⚠⚠ **A REPLY LONGER THAN THE PANE IS TALL IS CAPTURED WHOLE** — what only an addressed
    /// reply region can do, and the residue the row-keyed capture carried.
    ///
    /// A capture that compares ROWS can only ever return what is still on the grid. A model whose
    /// answer is longer than the pane pushes its own opening off the top, and the caller is handed
    /// the tail as though it were the whole reply — **a truncated answer, with nothing in it saying
    /// so**. That is worse than a missing one: it reads as complete.
    ///
    /// The fixture makes it certain rather than likely — a TEN-line reply into a FOUR-row pane —
    /// and the last line deliberately ends WITHOUT a newline, because a reply need not end in one
    /// and dropping it would lose the model's last word.
    #[test]
    fn a_reply_that_scrolled_past_the_pane_is_captured_whole() {
        let (access, pane) = sh_access(
            "exec sh -c 'in=$(cat); i=1; while [ $i -le 9 ]; do echo \"R$i[$in]\"; \
             i=$((i+1)); done; printf \"R10[$in]\"'",
            40,
            4,
        );
        let mut agent = Agent::new(pane, AgentSpec::new("ask"));
        let outcome = Driver::new(Guardrails {
            max_iterations: 2,
            max_cost: None,
            max_duration: Some(Duration::from_secs(30)),
        })
        .run(&mut agent, &access, &RunContext::uncancellable());
        assert_eq!(outcome.state, OutcomeState::Converged, "{outcome:?}");

        let captured = agent.captured().expect("a captured reply");
        assert!(
            !access
                .pane_collapsed(pane)
                .unwrap_or_default()
                .contains("R1[ask]"),
            "⚠ THE CONTROL: the reply's opening must ALREADY be off the four-row grid, or this \
             gate is about a visible reply and measures nothing new",
        );
        for i in 1..=9 {
            assert!(
                captured.contains(&format!("R{i}[ask]")),
                "line {i} of the model's answer must reach the caller — a capture that returns \
                 only what is still on screen hands back a TRUNCATED reply that reads as a \
                 complete one: {captured:?}",
            );
        }
        assert!(
            captured.contains("R10[ask]"),
            "⚠⚠ INCLUDING THE LAST LINE, WHICH HAS NO NEWLINE AFTER IT. A reply need not end in \
             one, and for a one-shot tool that unterminated line is the end of its answer — the \
             child has EXITED, so it is unfinished forever: {captured:?}",
        );
    }

    /// ⚠⚠ **A RESIZE MID-TURN IS NOT THE MODEL SPEAKING** — the worst instance of the
    /// paint-vs-content error, because what this plugin captures is published AS THE MODEL'S REPLY.
    ///
    /// The reply region was *"the rows whose DAMAGE GENERATION moved since the prompt"*, and a
    /// resize (`Screen::reflowed`) stamps every row. A client ATTACHING to the session mid-turn —
    /// the ordinary thing a person does — therefore made **the entire screen, banner and shell
    /// prompt and all, come back to the caller as what the model said**.
    ///
    /// The fixture puts text on screen that the model demonstrably did not produce (`OLD-BANNER`,
    /// printed before the prompt was ever sent), makes the peer slow enough that the resize lands
    /// inside the turn, and asserts the capture is the REPLY and not the screen.
    ///
    /// ⚠ Both halves: the reply is there (so the capture still works at all) and the banner is not
    /// (so it is a reply and not a screenshot).
    #[test]
    fn a_resize_during_a_turn_does_not_become_the_models_reply() {
        let workspace = Arc::new(Mutex::new(Workspace::new((40, 8))));
        let pane = {
            let mut command = CommandBuilder::new("/bin/sh");
            command.arg("-c");
            // OLD-BANNER is on screen BEFORE the prompt, and the peer waits a second before
            // answering — so the resize below lands between the baseline and the capture.
            command.arg(
                "printf 'OLD-BANNER\\n'; exec sh -c 'in=$(cat); sleep 1; echo \"REPLY[$in]\"'",
            );
            command.env("TERM", "dumb");
            workspace
                .lock()
                .unwrap()
                .spawn(command, "sh".to_string(), 40, 8)
                .expect("spawn pane")
        };
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5)
            && !access
                .pane_collapsed(pane)
                .is_some_and(|text| text.contains("OLD-BANNER"))
        {
            std::thread::sleep(Duration::from_millis(10));
        }

        let mut agent = Agent::new(pane, AgentSpec::new("ask"));
        std::thread::scope(|scope| {
            scope.spawn(|| {
                // Inside the turn: after the prompt's baseline, before the reply lands.
                std::thread::sleep(Duration::from_millis(300));
                workspace
                    .lock()
                    .unwrap()
                    .resize(pane, 34, 8, (0, 0))
                    .expect("a client attaches, so the pane is re-laid out");
            });
            let outcome = Driver::new(Guardrails {
                max_iterations: 2,
                max_cost: None,
                max_duration: Some(Duration::from_secs(30)),
            })
            .run(&mut agent, &access, &RunContext::uncancellable());
            assert_eq!(outcome.state, OutcomeState::Converged, "{outcome:?}");
        });

        let captured = agent.captured().expect("a captured reply");
        assert!(
            captured.contains("REPLY[ask]"),
            "the reply must still be captured — a fix that captured NOTHING would pass the other \
             half for the wrong reason: {captured:?}",
        );
        assert!(
            !captured.contains("OLD-BANNER"),
            "⚠⚠ AND TEXT THE MODEL NEVER PRODUCED MUST NOT BE PUBLISHED AS ITS REPLY — this was \
             on screen before the prompt was sent, and only a REPAINT could have put it in the \
             reply region: {captured:?}",
        );
    }

    /// ⚠⚠ **A PANE THAT NEVER COMES UP FAILS THE ASK AND NAMES WHAT WAS THERE** — this plugin's own
    /// `NeverReady` arm, which had no gate of its own.
    ///
    /// It was registered rather than built, on the reasoning that the other two injecting plugins
    /// build the identical `Readiness` path — which is exactly the argument R351 caught being wrong
    /// when a shared path stopped being shared. **And this plugin is the one where the arm matters
    /// most**: its failure mode is a WRONG ANSWER, not a missing one. Without the barrier it hands
    /// a shell's `command not found` back to a peer AS THE MODEL'S REPLY, so *"the pane never came
    /// up"* must reach the caller as a failure rather than as a captured reply.
    ///
    /// Three halves, and the last is the one the other plugins' gates cannot make: the run FAILED,
    /// the cause is typed and names both the question and what was running instead, and **nothing
    /// was captured** — because a captured anything here would be published as what the model said.
    #[test]
    fn an_agent_whose_pane_never_becomes_ready_fails_and_captures_nothing() {
        let (access, pane) = sh_access("exec cat", 40, 8);
        let mut agent = Agent::new(
            pane,
            AgentSpec {
                ready_when: Some(ReadyWhen::Runs("claude".to_string())),
                ready_within: Some(Duration::from_millis(200)),
                ..AgentSpec::new("summarise the repo")
            },
        );
        let outcome = Driver::new(Guardrails {
            max_iterations: 5,
            max_cost: None,
            // ⚠ FAR LONGER than the readiness bound, so the run's own clock provably cannot be
            // what ends this — that is the neighbouring gate, and it reaches a different arm.
            max_duration: Some(Duration::from_secs(30)),
        })
        .run(&mut agent, &access, &RunContext::uncancellable());

        assert_eq!(
            outcome.state,
            OutcomeState::Failed,
            "a pane that never became the tool is a FAILURE of the ask: {outcome:?}",
        );
        crate::testing::refused_naming(
            outcome.failure.as_ref(),
            &ReadyWhen::Runs("claude".to_string()),
            "cat",
            "and it names the question AND what the pane was running instead",
        );
        assert_eq!(
            agent.captured(),
            None,
            "⚠⚠ AND NOTHING WAS CAPTURED — anything here is published to the caller as THE \
             MODEL'S REPLY, which is this plugin's whole reason for taking the barrier",
        );
    }

    /// ⚠⚠ **A RUN THAT ENDS WHILE WAITING TO BE LET IN ASKS NOTHING AND CHARGES NOTHING** — and
    /// says which of the two it was doing, because "nothing was asked" and "asked, no reply" are
    /// opposite instructions to whoever reads the journal.
    #[test]
    fn an_agent_whose_run_ends_before_the_pane_is_ready_asks_nothing() {
        let (access, pane) = sh_access("exec cat", 40, 8);
        let mut agent = Agent::new(
            pane,
            AgentSpec {
                ready_when: Some(ReadyWhen::Prints(
                    "A MARKER THIS PANE NEVER PRINTS".to_string(),
                )),
                // ⚠ FAR ABOVE the run's clock, so the RUN's deadline is provably what ends the
                // wait rather than the barrier's own bound — that ending is the other arm.
                ready_within: Some(Duration::from_secs(300)),
                ..AgentSpec::new("summarise the repo")
            },
        );
        let cell = crate::driver::ProgressCell::default();
        let outcome = Driver::new(Guardrails {
            max_iterations: 100,
            max_cost: None,
            max_duration: Some(Duration::from_millis(200)),
        })
        .reporting_to(Arc::clone(&cell))
        .run(&mut agent, &access, &RunContext::uncancellable());

        assert_eq!(outcome.state, OutcomeState::Exhausted(Ceiling::Duration));
        let said = cell
            .lock()
            .expect("the progress cell")
            .journal
            .iter()
            .filter_map(|step| step.note.clone())
            .collect::<Vec<_>>()
            .join(" | ");
        assert!(
            said.contains("nothing was asked"),
            "the step must say the prompt was never sent, not that a reply never came: {said}",
        );
        assert_eq!(
            outcome.cost,
            Some(Cost::Bytes(0)),
            "nothing was injected, so nothing is charged: {outcome:?}",
        );
        assert!(
            agent.captured().is_none(),
            "and a run that asked nothing has captured no reply: {:?}",
            agent.captured(),
        );
        assert_eq!(
            access.pane_collapsed(pane).unwrap_or_default().trim(),
            "",
            "and the pane is untouched",
        );
    }

    /// ⚠⚠ **A CAPTURE TAKEN BECAUSE TIME RAN OUT SAYS SO** — it may be half a sentence.
    ///
    /// This adapter converges on the child EXITING, which is what makes a capture complete: every
    /// byte the peer produced is on the screen by then. When the per-turn timeout ends the wait
    /// instead, the text is whatever was on screen mid-reply — and both were reported with the
    /// same sentence, so a truncated capture was indistinguishable from a whole one by anything a
    /// run publishes.
    #[test]
    fn a_capture_taken_because_the_turn_ran_out_of_time_is_marked_as_possibly_partial() {
        // Announces, then answers when spoken to, then stays alive — so the reply IS on screen
        // but EOF never comes.
        //
        // ⚠⚠ IT USED TO PRINT ITS REPLY UNCONDITIONALLY and this test waited for nothing, which
        // made it one of the recorded load-marginal gates: under whole-suite load the peer got its
        // line out BEFORE the run took its baseline, so the reply was not "since" the prompt and
        // the capture came back without it. The reply now waits to be asked for, and the fixture
        // waits for the peer — the wait is on the observable fact, not on the ACT that causes it.
        let (access, pane) =
            sh_access("printf 'UP\n'; read _; echo PARTIAL-REPLY; exec cat", 40, 8);
        started(&access, pane, "UP");
        let mut agent = Agent::new(
            pane,
            AgentSpec {
                eof: false,
                timeout: Duration::from_millis(300),
                ..AgentSpec::new("x")
            },
        );
        let cell = crate::driver::ProgressCell::default();
        let outcome = Driver::new(Guardrails {
            max_iterations: 2,
            max_cost: None,
            max_duration: Some(Duration::from_secs(20)),
        })
        .reporting_to(Arc::clone(&cell))
        .run(&mut agent, &access, &RunContext::uncancellable());

        assert_eq!(
            outcome.state,
            OutcomeState::Converged,
            "a per-turn timeout still converges with what it has — that behaviour is deliberate \
             and unchanged; what was missing is saying so",
        );
        let notes: Vec<String> = cell
            .lock()
            .expect("the progress cell")
            .journal
            .iter()
            .filter_map(|step| step.note.clone())
            .collect();
        let said = notes.join(" | ");
        assert!(
            said.contains("PARTIAL"),
            "a capture the clock cut short must be marked as possibly partial: {said}",
        );
        assert!(
            agent
                .captured()
                .is_some_and(|reply| reply.contains("PARTIAL-REPLY")),
            "and it still captures what the peer did say",
        );
    }

    #[test]
    fn captures_a_complete_multiline_reply() {
        // Two reply lines. Converging on child-exit (not first damage) captures
        // BOTH — a first-damage observe would stop at the prompt echo and miss
        // the reply. Pane is tall enough that the reply does not scroll.
        let (access, pane) = sh_access(
            "in=$(cat); printf 'one:%s\\ntwo:%s\\n' \"$in\" \"$in\"",
            40,
            8,
        );
        let mut agent = Agent::new(pane, AgentSpec::new("x"));

        let outcome = run(&access, &mut agent);

        assert_eq!(outcome.state, OutcomeState::Converged);
        let captured = agent.captured().expect("a captured reply");
        assert!(captured.contains("one:x"), "captured: {captured:?}");
        assert!(captured.contains("two:x"), "captured: {captured:?}");
    }

    /// ⚠⚠ **THE RUN'S DEADLINE REACHES INSIDE A STEP**, which is the only thing that makes it a
    /// bound at all.
    ///
    /// The peer never exits, so `await_reply` waits out `spec.timeout` in full. Both halves drive
    /// that same peer and differ only in whether the run is timed:
    ///
    /// * The CONTROL has no deadline. Its step runs its own four-second timeout to the end, the
    ///   agent captures whatever is on screen and converges — which is the behaviour a per-turn
    ///   timeout is FOR, and it is unchanged.
    /// * The SUBJECT is given three hundred milliseconds. It must come back an order of magnitude
    ///   sooner, and it must come back `Exhausted(Duration)`.
    ///
    /// ⚠ A deadline enforced only at the Driver's loop top would make both halves take four
    /// seconds and both assertions about ELAPSED time fail — which is why the timing is asserted
    /// and not just the outcome. The two are the same claim read two ways: the run stopped because
    /// of the clock, and it stopped WHEN the clock said.
    #[test]
    fn a_runs_deadline_cuts_a_step_that_is_still_inside_its_own_timeout() {
        // `exec cat` holds its pty open forever, and `eof: false` means no Ctrl-D is sent to end
        // it — so nothing but a bound can end this wait.
        let timed = |deadline: Option<Duration>| {
            let (access, pane) = sh_access("exec cat", 40, 6);
            let mut spec = AgentSpec::new("ping");
            spec.eof = false;
            spec.timeout = Duration::from_secs(4);
            let mut agent = Agent::new(pane, spec);
            let start = std::time::Instant::now();
            let outcome = Driver::new(Guardrails {
                max_iterations: 100,
                max_cost: None,
                max_duration: deadline,
            })
            .run(&mut agent, &access, &RunContext::uncancellable());
            (outcome, start.elapsed())
        };

        let (control, control_took) = timed(None);
        assert_eq!(
            control.state,
            OutcomeState::Converged,
            "an untimed run rides its own per-turn timeout out and converges on what it saw",
        );
        assert!(
            control_took >= Duration::from_secs(3),
            "the control must actually have waited its step out, or the subject below is being \
             compared against nothing; it took {control_took:?}",
        );

        let (subject, subject_took) = timed(Some(Duration::from_millis(300)));
        assert_eq!(
            subject.state,
            OutcomeState::Exhausted(Ceiling::Duration),
            "a run out of time is exhausted by the DURATION ceiling, and says so",
        );
        assert!(
            subject_took < Duration::from_secs(2),
            "the deadline must end the wait that is in flight, not merely stop the next step \
             being taken — this run took {subject_took:?} against a step timeout of 4s",
        );
    }

    /// The genuine AI↔AI proof: drive a real `claude -p` pane and capture its
    /// answer. Ignored by default — it needs the `claude` CLI, network, and
    /// auth, and is non-deterministic. Run manually:
    /// `cargo test -p sprag-plugin drives_real_claude -- --ignored --nocapture`.
    #[test]
    #[ignore = "needs the claude CLI + network + auth; run manually with --ignored"]
    fn drives_real_claude() {
        let mut command = CommandBuilder::new("claude");
        command.arg("-p");
        command.env("TERM", "dumb");
        let workspace = Arc::new(Mutex::new(Workspace::new((80, 24))));
        let pane = workspace
            .lock()
            .unwrap()
            .spawn(command, "claude".to_string(), 80, 24)
            .expect("spawn claude pane");
        let access = WorkspacePaneAccess::new(workspace);

        let mut agent = Agent::new(
            pane,
            AgentSpec::new("Reply with exactly the single word: PONG"),
        );
        let outcome = Driver::new(Guardrails {
            max_iterations: 2,
            max_cost: None,
            max_duration: None,
        })
        .run(&mut agent, &access, &RunContext::uncancellable());

        assert_eq!(outcome.state, OutcomeState::Converged);
        let captured = agent.captured().unwrap_or_default();
        assert!(captured.contains("PONG"), "captured: {captured:?}");
    }

    /// ⚠⚠⚠ **A RUN THAT IS STOPPED TAKES THE PEER'S TURN WITH IT** — end to end, on a real shell
    /// pane, against a job that would otherwise outlive the run by five minutes.
    ///
    /// This is the whole defect in one fixture. The pane is an interactive `bash`, the prompt is
    /// `sleep 300`, and the agent's own reply wait is two minutes — so when the cancel lands, the
    /// step is BLOCKED and a job the run set going owns the pane's terminal. Before
    /// [`Plugin::driving`](crate::plugin::Plugin::driving), `Driver::run` returned `cancelled` here
    /// and the `sleep` ran on: **the run's bookkeeping ended and its work did not.**
    ///
    /// ⚠ The cancel is raised by a watcher thread THE MOMENT the job is observed running, not on a
    /// timer. A timer would make the gate a race — cancel too early and there is no job yet, so it
    /// would pass while measuring nothing.
    ///
    /// ⚠ The claim asserted last is about the WORLD, not about the report: no `sleep` owns the
    /// pane's terminal afterwards. A report can be made to say anything; the process table cannot.
    ///
    /// ⚠ REVERT-PROOF, MEASURED: with the `stop_the_work` call in `Driver::run` disabled, this
    /// fails at *"a cancelled run with a job going must report stopping it: None"* — the REPORT
    /// assertion, which comes first. The world assertion below it is the stronger claim and is
    /// never reached in that state, so the two are not redundant: the first says the run knows what
    /// it did, the second says the process table agrees.
    // Linux AND macOS: the substrate underneath is `procfs`, which answers on both.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_cancelled_turn_does_not_leave_the_peer_working() {
        use crate::access::PaneAccess;
        use std::sync::atomic::{AtomicBool, Ordering};

        let workspace = Arc::new(Mutex::new(Workspace::new((60, 8))));
        // ⚠ `bash -i` and not `/bin/sh`: JOB CONTROL is what puts `sleep` in its own process group,
        // which is what makes this a gate about stopping the pane's JOB rather than its shell. A
        // non-interactive shell runs the job in its own group and the two questions collapse.
        let mut command = CommandBuilder::new("/bin/bash");
        command.arg("--norc");
        command.arg("-i");
        command.env("TERM", "dumb");
        command.env("PS1", "$ ");
        let pane = workspace
            .lock()
            .unwrap()
            .spawn(command, "bash".to_string(), 60, 8)
            .expect("spawn pane");
        let child = workspace
            .lock()
            .unwrap()
            .pane(pane)
            .and_then(|held| held.pty().pid())
            .expect("a live child");
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));

        let leader_named = |want: &str| {
            (&access as &dyn PaneAccess)
                .foreground_job()
                .and_then(|jobs| jobs.pane_foreground_leader(pane))
                .is_some_and(|job| job.name == want)
        };
        let until = |within: Duration, mut ready: Box<dyn FnMut() -> bool>| {
            let start = Instant::now();
            while start.elapsed() < within {
                if ready() {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            false
        };
        assert!(
            until(
                Duration::from_secs(15),
                Box::new(|| sprag_terminal::foreground_leader_of(child)
                    .is_some_and(|job| job.pid == child)),
            ),
            "the shell must reach its own prompt first, or the prompt below is typed at nothing",
        );

        let cancel = Arc::new(AtomicBool::new(false));
        let watcher = {
            let cancel = Arc::clone(&cancel);
            std::thread::spawn(move || {
                let start = Instant::now();
                while start.elapsed() < Duration::from_secs(20) {
                    if sprag_terminal::foreground_leader_of(child)
                        .is_some_and(|job| job.name == "sleep")
                    {
                        cancel.store(true, Ordering::Release);
                        return true;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                // ⚠ Cancel anyway so the run cannot hang the suite for two minutes; the assertion
                // below is what reports that the job was never seen.
                cancel.store(true, Ordering::Release);
                false
            })
        };

        let mut agent = Agent::new(
            pane,
            AgentSpec {
                // ⚠ `eof: false`: a trailing Ctrl-D would make the shell EXIT, which ends the job
                // by killing the pane and would let this pass with the mechanism removed.
                eof: false,
                ..AgentSpec::new("sleep 300")
            },
        );
        let outcome = Driver::new(Guardrails {
            max_iterations: 1,
            max_cost: None,
            max_duration: None,
        })
        .run(&mut agent, &access, &RunContext::new(Arc::clone(&cancel)));

        assert!(
            watcher.join().expect("the watcher thread"),
            "the job never started, so this measured nothing — the cancel was raised on the \
             fallback rather than on the observation",
        );
        assert_eq!(
            outcome.state,
            OutcomeState::Cancelled,
            "the run ended because somebody stopped it",
        );
        match &outcome.stopped {
            Some(crate::driver::Stopped::Job(signalled)) => assert!(
                signalled
                    .leader
                    .as_ref()
                    .is_some_and(|leader| leader.answers_to("sleep")),
                "the outcome names the job it stopped, which is the answer a caller acts on: \
                 {signalled}",
            ),
            other => panic!("a cancelled run with a job going must report stopping it: {other:?}"),
        }
        assert!(
            until(Duration::from_secs(15), Box::new(|| !leader_named("sleep"))),
            "⚠⚠ AND THE WORLD AGREES: no `sleep` owns the pane's terminal after the run, which is \
             the claim a report cannot fake",
        );
    }

    /// ⚠⚠⚠ **A RUN CUT SHORT AGAINST A PANE WHOSE OWN PROGRAM IS THE PEER LEAVES THE PANE STANDING,
    /// AND SAYS THE WORK IS STILL RUNNING.**
    ///
    /// The other half of the gate above, and it exists because the first version of this mechanism
    /// did not have it: a run cut short signalled whatever owned the pane's terminal, and on a pane
    /// whose own program was the peer that ENDED THE PANE. Measured then — the pane went, it was a
    /// session's last, and the daemon exited behind it.
    ///
    /// A person typing a stop at one named pane may choose that. A clock running out may not, so
    /// [`Reach::UnderTheProgram`] refuses and the outcome carries the refusal — which is a better
    /// answer than either alternative, because *your work is still running and here is why* is
    /// something a caller can act on.
    ///
    /// ⚠ The pane's OWN program is the peer, with no shell in between: with one, the peer would be
    /// a JOB one level down and no reach would refuse — the case that would make this gate vacuous.
    ///
    /// ⚠⚠ AND THE GATE WAITS FOR THE `exec`, BY NAME. The disposition is read at stop time, so a
    /// shell that has not yet replaced itself with its peer reads as THE SHELL — and a shell
    /// catches `SIGINT`, so the stop would be delivered and this would measure the opposite of what
    /// it claims. Found by this gate failing exactly that way.
    ///
    /// [`Reach::UnderTheProgram`]: sprag_terminal::Reach::UnderTheProgram
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_cancelled_run_does_not_end_the_pane_whose_own_program_is_the_peer() {
        use crate::access::PaneAccess;
        use std::sync::atomic::AtomicBool;

        // `sleep` and not `cat`, because what is discriminated is a program the signal would KILL
        // — and `sleep` is one whose arrival can be waited for by name.
        let (access, pane) = sh_access("exec sleep 300", 40, 6);
        let panes: &dyn PaneAccess = &access;
        let jobs = panes
            .foreground_job()
            .expect("this host reads the job table");
        let became_the_peer = || {
            jobs.pane_foreground_leader(pane)
                .is_some_and(|job| job.name == "sleep")
        };
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline && !became_the_peer() {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            became_the_peer(),
            "the shell must have BECOME the peer, or the disposition read below is the shell's",
        );

        let mut agent = Agent::new(
            pane,
            AgentSpec {
                eof: false,
                ..AgentSpec::new("anything")
            },
        );
        let outcome = Driver::new(Guardrails {
            max_iterations: 1,
            max_cost: None,
            max_duration: None,
        })
        .run(
            &mut agent,
            &access,
            &RunContext::new(Arc::new(AtomicBool::new(true))),
        );

        assert_eq!(outcome.state, OutcomeState::Cancelled);
        let said = outcome
            .stopped
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default();
        assert!(
            said.contains("still running"),
            "⚠ the outcome must TELL the caller their peer was left going, which is the whole \
             value of refusing rather than reaching: {said:?}",
        );
        assert_eq!(
            panes.pane_eof(pane),
            Some(false),
            "⚠⚠ AND THE PANE IS STILL THERE — a run that merely ran out of time must not be able \
             to close somebody's pane",
        );
    }
}
