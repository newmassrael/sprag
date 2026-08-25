//! The per-pane memory: what a single frame cannot know.
//!
//! H3 slice 2. [`detect`](crate::detect) answers from one screen and one title, and that is the
//! whole of what it can do. Three of the measurements this front is built on are about a pane over
//! TIME, and none of them can be honoured without somewhere to remember:
//!
//! * **The working signal is an ANIMATION.** `claude`'s title alternates between braille frames at
//!   about 1 Hz (R249's M2). A detector that publishes every frame publishes a flicker, so a
//!   verdict that rests on NOT seeing that signal has to hold before it is believed. That is
//!   [`Hysteresis`], and M2 is why the design calls it a correctness requirement of the input
//!   rather than a later polish.
//! * **A quiet pane costs nothing, exactly.** An idle agent pane moved ZERO row generations over
//!   eight seconds (M3). Because the rules are a pure function of the screen, the title and the
//!   RULE LIST, a pane where none of the three has moved cannot reach a different verdict — so
//!   skipping the evaluation is a skip with a proof rather than a heuristic that trades accuracy for
//!   cost. The third input is the one slice 4 added and it is named here rather than left implied:
//!   the list is replaced when a user edits `config.toml`, and a gate watching only the first two
//!   would hold a stale verdict on every quiet pane for as long as it stayed quiet. See
//!   [`Ruleset`].
//! * **A modal hides WHO the pane belongs to.** A `codex` dialog covers the composer line and the
//!   footer its fingerprint is made of, so the pane goes unclaimed in the one state this front
//!   exists to report (R251). Nothing on that screen says `codex`, so no better fingerprint fixes
//!   it; only memory does.
//!
//! The clock arrives as a PARAMETER, the shape `Keymap::route` uses and for the same reason: a
//! settle window becomes arithmetic in a test instead of a sleep.

use std::time::{Duration, Instant};

use sprag_vt::Screen;

use crate::{AgentState, Manifest, Ruleset, Verdict};

/// The default settle window.
///
/// Longer than the roughly 1 Hz the working spinner was measured alternating at (R249's M2),
/// because the artifact this window exists to absorb is one frame of that animation: a window
/// shorter than the period it guards against publishes exactly the flicker it was added for. Two
/// seconds rather than one leaves room for a slower box and a slower agent, and it is only ever
/// spent on a pane coming to REST — [`AgentState::is_active`](crate::AgentState::is_active) states
/// are published on sight, so the state a person is waiting for is never delayed by it.
pub const DEFAULT_SETTLE: Duration = Duration::from_secs(2);

/// The rule a verdict names when an UNANSWERED DIALOG overruled a report older than it — register
/// item 524.
///
/// It is published beside the reporter's own `source`, never instead of it: a reader is owed both
/// halves — *who claimed the pane* and *what beat their claim*. ⚠ Spelled once, here, because it
/// travels to the wire and a gate compares it; a string literal at the site would be a second
/// definition of a published word.
pub const DIALOG_OUTRANKS_REPORT: &str = "dialog-outranks-report";

/// How long a candidate state must hold before [`Tracker`] publishes it.
///
/// # One parameter, where the design named two
///
/// H3's design asked for "N consecutive evaluations, with a wall-clock cap so a slow-changing pane
/// still settles". The count is not a second mechanism, and this is the slice that had to find out:
///
/// * A pending candidate is REPLACED by any observation that disagrees with it, so "this has been
///   the answer since `t`" already means "nothing has disagreed since `t`". A count of agreeing
///   observations adds no evidence to that.
/// * Under the OR the design's own sentence implies — the cap exists precisely because a quiet pane
///   never reaches N — the count can only publish EARLIER than the window. On a pane that repaints
///   quickly, which is what an agent printing output IS, N observations arrive in milliseconds and
///   the guard collapses to nothing.
/// * Under an AND it is worse: a pane that goes quiet after one observation never reaches N and
///   freezes in its previous state forever, which is the failure the pending exception below exists
///   to prevent.
///
/// So the window is time, and time alone. The struct stays a struct because the option that will
/// carry it (slice 3, where an evaluation site exists to read it) wants a name, and because a
/// second knob added later must not change [`Tracker::new`]'s shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hysteresis {
    /// How long a candidate that rests on an ABSENCE must hold before it is published.
    pub settle: Duration,
}

impl Default for Hysteresis {
    fn default() -> Self {
        Self {
            settle: DEFAULT_SETTLE,
        }
    }
}

/// A verdict waiting to be believed, and when it was first seen.
#[derive(Debug)]
struct Pending {
    candidate: Verdict,
    since: Instant,
}

/// The inputs the rules read, as of the last evaluation — the quiescence key.
///
/// It is the exactness that earns the skip, so each field is here because the rules can read it
/// and for no other reason.
#[derive(Debug)]
struct Seen {
    /// Per-row damage stamps, which stand in for every row's text: a row whose stamp has not moved
    /// has not been written to. This is the same comparison the projection gate and the wire client
    /// already make (R218, R220), against the same stamps.
    generations: Vec<u64>,
    /// The width, because a row's TEXT is its cells and how many of them fit — and the two can move
    /// independently. `Screen::resized`, the resize path an alternate screen takes, copies every
    /// row's damage stamp verbatim while truncating the cells to the new width, so a narrowing
    /// resize is a content change no stamp records.
    cols: u16,
    /// The title, with an absent one stored as the empty string exactly as
    /// [`detect`](crate::detect) reads it — so two titles this cannot tell apart are two titles the
    /// rules cannot tell apart either.
    title: String,
    /// WHICH rules produced the last verdict ([`Ruleset::revision`](crate::Ruleset::revision)).
    ///
    /// The other three fields are what the rules READ; this one is the rules themselves. It belongs
    /// in the same key for the same reason: the skip claims a re-evaluation would reach the same
    /// answer, and a re-evaluation against a list the user has since edited would not.
    rules: u64,
}

impl Seen {
    fn of(screen: &Screen, title: &str, rules: u64) -> Self {
        let mut seen = Self {
            generations: Vec::new(),
            cols: 0,
            title: String::new(),
            rules,
        };
        seen.refresh(screen, title, rules);
        seen
    }

    /// Whether nothing the rules can read has moved, and the rules are still the same rules.
    fn unchanged(&self, screen: &Screen, title: &str, rules: u64) -> bool {
        self.rules == rules
            && self.cols == screen.cols()
            && self.title == title
            && self.generations.len() == screen.rows() as usize
            && (0..screen.rows())
                .all(|row| screen.row_generation(row) == Some(self.generations[row as usize]))
    }

    /// Take the reading again, reusing the buffers so a steady-state pane allocates nothing.
    fn refresh(&mut self, screen: &Screen, title: &str, rules: u64) {
        self.rules = rules;
        self.generations.clear();
        self.generations
            .extend((0..screen.rows()).map(|row| screen.row_generation(row).unwrap_or_default()));
        self.cols = screen.cols();
        if self.title != title {
            self.title.clear();
            self.title.push_str(title);
        }
    }
}

/// One pane's agent state over time: the quiescence gate, the settle window, and the identity a
/// modal covers.
///
/// One per pane, held by whoever serves the pane's facts. Slice 3 puts it in a host-side registry,
/// NOT on `sprag_terminal::Pane` as this comment said when slice 2 wrote it: `sprag-terminal` is the
/// producer and owns the emulator and the PTY only, while a detector is scene-side — the division
/// `sprag-grid` already sits on the far side of. It owns no clock and no manifests: `now` and the
/// manifest list arrive on every [`observe`](Self::observe) call, so a workspace has one manifest
/// list and a test has arithmetic.
///
/// [`observe`](Self::observe) is meant to be called on EVERY tick, including the ticks where
/// nothing has happened. The quiescence gate lives inside it rather than at the call site, so a
/// caller cannot accidentally hold a pane's pending transition open by deciding for itself that
/// nothing was worth asking about.
///
/// # There is no tick, so a pending transition has to ASK for one
///
/// Slice 2 wrote "every tick" as though a tick existed. Slice 3 read the daemon and found none: the
/// pane list is served when a client asks, and a client asks when the scene revision moves, which
/// pane OUTPUT and user ACTIONS advance. That is enough for a verdict resting on present evidence —
/// the output that paints a dialog is the same event that wakes the reader — and it is not enough for
/// one resting on an absence, because the last thing to move the revision was the output that
/// STOPPED. A pane going quiet is a transition whose confirming observation nothing produces.
///
/// So [`pending_deadline`](Self::pending_deadline) exists: it is the instant at which this tracker
/// would publish if it were asked, and it is `None` when nothing is pending. A caller drives the
/// clock by observing again at that instant, and a caller with nothing pending owes nothing — which
/// keeps M3's "a quiet workspace costs nothing" true of the confirmation as well as of the
/// evaluation.
#[derive(Debug)]
pub struct Tracker {
    policy: Hysteresis,
    /// The verdict on the wire. [`Verdict::default`] until something is published.
    published: Verdict,
    /// Increments on every PUBLISHED change, so a client can tell "still blocked" from "blocked
    /// again" without diffing strings — the treatment `notification_seq` already gets.
    seq: u64,
    /// **HOW MANY QUESTIONS THIS PANE HAS BEEN ASKED** — one per report that STATES an
    /// [`Report::asked`], whether or not the published verdict moved.
    ///
    /// # ⚠⚠⚠⚠⚠ Why [`seq`](Self::seq) could not answer this, measured (register item 441)
    ///
    /// A supervisor's whole question is *did the peer take the question I just asked?*, and until
    /// this counter existed there was **nothing observable to answer it with**. `seq` advances only
    /// inside [`publish`](Self::publish), which runs only when the verdict CHANGES — and a submit
    /// arriving at a pane that is already `working` reports `working` again, so the verdict is
    /// identical, nothing publishes, and `seq` stands still. The peer took a new question and every
    /// reader saw an unchanged pane.
    ///
    /// What that cost, live: a loop typed a prompt at an agent that was still busy, saw an `idle`
    /// belonging to the EARLIER work, called its own turn over, judged an empty window and prompted
    /// again — thirty-three times, 6,604 bytes, while the agent worked throughout and the marker it
    /// printed was never heard.
    ///
    /// ⚠⚠⚠ **A SECOND COUNTER RATHER THAN A RICHER `seq`**, and that is the decision. Folding
    /// `asked` into [`Verdict`] would make every new question a "published change" — waking every
    /// client and conflating *what this pane is doing* with *what it was last asked*, which is one
    /// word covering two worlds and the shape this crate has already paid for. These are two facts
    /// and they move for two reasons.
    ///
    /// ⚠⚠ Counted on the STATEMENT, not on the text: two identical prompts are two questions. A
    /// reader comparing the strings could not tell a re-prompt from an echo, which is exactly the
    /// case that defeats every text-matching rule.
    asked_seq: u64,
    /// **HOW MANY ANSWERS THIS PANE'S AGENT HAS STATED** — one per report that STATES a
    /// [`Report::said`], on the same terms as [`asked_seq`](Self::asked_seq) and for the same
    /// reason it could not be folded into [`seq`](Self::seq).
    ///
    /// ⚠⚠⚠ **IT IS WHAT MAKES A STATEMENT BELONG TO A TURN.** The text alone cannot: an agent that
    /// answers the same words twice states two answers, and a reader comparing strings would call
    /// the second one stale. A supervisor arms on this number when it asks its question and
    /// requires it to have MOVED before reading what came back — the same discipline
    /// [`Verdict`]-arming already uses at both ends of a turn.
    said_seq: u64,
    /// **HOW MANY REPORTS THIS PANE HAS ACCEPTED**, whatever any of them said — register item 458.
    ///
    /// ⚠⚠⚠⚠⚠ **THE ONLY NUMBER HERE THAT MOVES WHILE A TURN IS MERELY WORKING.** [`seq`](Self::seq)
    /// counts published CHANGES, and a turn that calls tool after tool reports `working` each time
    /// and changes nothing; [`asked_seq`](Self::asked_seq) and [`said_seq`](Self::said_seq) count
    /// statements, and a turn in flight has made neither. So *the agent is thinking* and *the agent
    /// was interrupted and will never speak again* were the same three frozen numbers.
    ///
    /// **Measured 2026-08-19**: a turn stopped with Escape emitted no payload of any kind for
    /// fourteen minutes, and the pane read `working seq=6 asked=2 said=0` throughout — exactly what
    /// a long turn reads. A driver polled it and would have done so for `max_seconds`, which the
    /// shipped kind authors at 24 hours.
    ///
    /// ⚠⚠ A FOURTH COUNTER RATHER THAN A RICHER `seq`, on [`asked_seq`](Self::asked_seq)'s own
    /// argument: folding it in would make every tool call a published change, waking every client
    /// and conflating *what this pane is doing* with *has anything spoken for it*.
    ///
    /// ⚠ It says nothing about WHEN. A caller that wants an age compares this across two looks of
    /// its own — the watermark discipline this workspace's hand counts already use, and the reason
    /// this is a count and not an instant: the tracker keeps no reader state, so several waiters can
    /// each ask *since I last looked* without coordinating, and one that never looks costs nothing.
    reports: u64,
    pending: Option<Pending>,
    seen: Option<Seen>,
    /// Which agent this pane was last IDENTIFIED as, independent of what it is doing.
    ///
    /// By name rather than by index into the manifest list, because slice 4 reloads that list from
    /// a file: a name survives a reload and a position does not.
    identity: Option<String>,
    /// The REPORT in force, when a process inside the pane has said what it is doing — the second
    /// kind of evidence this tracker weighs, and the only one that outranks the screen.
    ///
    /// `None` is a pane whose state is inferred, which is every pane before anything reports and
    /// every pane whose reporter has been [`released`](Tracker::release_report).
    reported: Option<Reported>,
    /// Set when the pane's published answer has to be RE-DERIVED from the screen and nothing on the
    /// screen will say so — today, exactly a release.
    ///
    /// It is a flag rather than a recomputation because the recomputation needs a screen, which this
    /// type is never handed except by [`observe`](Self::observe). See
    /// [`owes_look`](Self::owes_look).
    owes_look: bool,
}

/// A report in force: who said it, the sequence number they said it with, and how long it lasts.
///
/// The state itself is NOT here — it goes straight into `published`, because a report is the
/// published answer rather than a candidate for it. What is kept is only what the NEXT report has
/// to be judged against, plus what decides when THIS one is over.
#[derive(Debug)]
struct Reported {
    /// **WHAT THIS REPORT ITSELF CLAIMED** — kept here since the day a NEWER screen fact could
    /// overrule it (register item 524).
    ///
    /// It used to live only in `published`, and that was true for as long as nothing could publish
    /// over a standing report. An unanswered dialog now can, so the report's own claim has to
    /// survive being overruled — otherwise the moment the dialog is answered there is nothing left
    /// to fall back to and the pane would stay `blocked` until the next report happened to arrive.
    state: AgentState,
    /// The reporter's name, as it will appear on the wire. Compared against the next report's so a
    /// replay from the SAME speaker is refused while a new speaker is heard.
    source: String,
    /// The last sequence number accepted from `source`, if it sent one. `None` for a reporter with
    /// no clock (a person at a command line), which is therefore never refused as stale.
    seq: Option<u64>,
    /// An opaque token naming the thing whose continued existence keeps this report standing, or
    /// `None` for a report that stands until somebody releases it.
    ///
    /// The tracker stores it and never interprets it — like `source` and `seq`, it is the caller's
    /// vocabulary. It lives HERE rather than in a map beside the tracker because a report's lifetime
    /// is part of the report: a second place answering "is there a report on this pane" would drift
    /// from this one silently, every individual answer still looking right.
    ///
    /// It is on the report rather than on the tracker because it belongs to ONE report — the next
    /// report is a new claim by a possibly new speaker, and it brings its own.
    owner: Option<u64>,
    /// **THE LAST PROMPT THIS PANE'S AGENT SAID IT WAS ASKED**, and `None` until one has said so.
    ///
    /// ⚠⚠⚠ CARRIED FORWARD ACROSS REPORTS THAT SAY NOTHING ABOUT IT, which is the opposite of every
    /// other field here and is the whole reason it is a field rather than a copy of the report. Only
    /// the event that OPENS a turn carries a prompt; the events that end one carry none. A field
    /// replaced wholesale would therefore be erased by the very next report — `Stop` arrives seconds
    /// after `UserPromptSubmit` — and the fact would be gone before any reader could use it.
    asked: Option<String>,
    /// **THE LAST ANSWER THIS PANE'S AGENT SAID IT GAVE**, carried forward for
    /// [`asked`](Self::asked)'s reason one end over: only the event that ENDS a turn carries one,
    /// and the events that follow it carry none.
    said: Option<String>,
    /// **WHY THIS PANE'S AGENT SAID IT WANTS A PERSON**, or `None` where the report in force said
    /// nothing about it.
    ///
    /// ⚠⚠⚠⚠⚠ NOT CARRIED, which is the opposite of the two fields above and is the whole meaning of
    /// the field. Those are the two ends of a TURN, stated once and needed afterwards. **A notice is
    /// a request that is either outstanding or dealt with**, and the report that follows it is the
    /// evidence it was dealt with — a peer that went back to `working` is not still asking. Carrying
    /// it would let a supervisor quote, at a pane blocked on something else, a question a person
    /// answered an hour ago; and it would do so in the peer's own voice, which is exactly what makes
    /// such a quote convincing.
    ///
    /// ⚠ This is also why no `noticed_seq` sits beside [`Tracker::said_seq`]: nothing here is ever
    /// older than the report in force, so there is no gap for a counter to date.
    noticed: Option<String>,
    /// **WHERE THIS PANE'S AGENT SAID IT IS WRITING**, carried forward for [`asked`](Self::asked)'s
    /// reason exactly: a transcript path is stated on the turn's first event and on no other, while
    /// the file goes on existing for the whole session.
    transcript: Option<String>,
    /// **WHICH BUILD THE REPORTER IN FORCE SAID IT IS**, or `None` where it did not say.
    ///
    /// ⚠⚠ Unlike its two neighbours above this is NOT carried across reports — see the assignment in
    /// [`Tracker::report`]. Those are events about a turn; this is a level about the current
    /// reporter, and inheriting it would let a replaced reporter answer under its predecessor's
    /// identity.
    build: Option<String>,
}

/// What a reporter said about a pane, as one message.
///
/// The five fields travel together from the wire to the tracker and mean nothing apart: `state` is
/// the claim, `agent` and `source` are who is making it, and `seq` and `owner` are the two things
/// that bound it — one against the reporter's own replays, the other against the reporter ceasing to
/// exist. Passing them separately let a caller reorder two `Option`s of the same width, which no
/// signature could have caught.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// What the reporter says the pane is doing.
    pub state: AgentState,
    /// The agent's name, when the reporter names it. A reporter that does NOT leaves the pane's
    /// identity alone rather than clearing it — see [`Tracker::report`].
    pub agent: Option<String>,
    /// Who is reporting, as it will appear on the wire beside the verdict.
    pub source: String,
    /// The reporter's own monotonic clock, compared only against the last one from `source`.
    /// `None` for a reporter that has none, which is therefore never refused as stale.
    pub seq: Option<u64>,
    /// An opaque token for the thing whose existence keeps this report standing, or `None` for one
    /// that stands until it is released. The tracker stores it and never interprets it.
    pub owner: Option<u64>,
    /// **WHAT THE AGENT SAYS IT ANSWERED**, on the one event that ends a turn, and `None` on every
    /// other.
    ///
    /// ⚠⚠⚠⚠ [`asked`](Self::asked)'s other end, and it exists because the alternative was measured
    /// impossible rather than merely awkward: a full-screen agent's pane was read at every
    /// judgement of a live run with its whole logical-line count frozen at 37, so *what did this
    /// turn print* answered `0` for ever while the agent wrote reply after reply (register item
    /// 441). What the peer SAID is a fact only the peer has.
    ///
    /// ⚠ Carried forward exactly as `asked` is, and for the same reason: a turn's ending states it
    /// and the events that follow state nothing, so a field replaced wholesale would be erased by
    /// the next `working`.
    pub said: Option<String>,
    /// **THE PROMPT THE AGENT SAYS IT WAS ASKED**, on the one event that opens a turn, and `None`
    /// on every other — which is most of them.
    ///
    /// ⚠⚠⚠⚠ It is an ACCOUNT, not a claim this crate evaluates. Whether it matches what somebody
    /// typed is the caller's question, and it is a question no terminal can answer: a screen shows
    /// the same pixels for text a run delivered and text a composer already held. **This is the
    /// only place the two can be told apart**, which is why it travels with the state rather than
    /// being inferred beside it.
    ///
    /// ⚠ Kept as the LAST one reported rather than a history: what a delivery asks is *did my
    /// question arrive*, and the answer to that is about the most recent turn.
    pub asked: Option<String>,
    /// **WHY THE AGENT SAYS IT WANTS A PERSON**, on the one event it raises to ask for attention, and
    /// `None` on every other.
    ///
    /// ⚠⚠⚠⚠ The half a screen cannot supply for the case the screen was BUILT for. A blocked pane's
    /// question is read as a numbered menu; a peer blocked on anything else leaves a supervisor with
    /// *"this host cannot read it — hand the pane to a person"*, while the peer's own sentence was in
    /// the payload that produced the word `blocked` (register item 452).
    ///
    /// ⚠ REPLACED and never carried, unlike [`asked`](Self::asked) and [`said`](Self::said) — see
    /// the field it is stored in for why a request that has been dealt with must not outlive the
    /// report that dealt with it.
    pub noticed: Option<String>,
    /// **WHERE THE AGENT SAYS IT IS WRITING ITS TRANSCRIPT.**
    ///
    /// Stated rather than resolved from an id — see the wire key's own doc for what resolving it
    /// has cost. `None` where the reporter did not say, which is not a fault: an agent that reports
    /// its turn while writing no transcript is a working agent.
    pub transcript: Option<String>,
    /// **WHICH BUILD THE REPORTER IS**, as it stated — `None` where it did not say.
    ///
    /// Stored and never interpreted, exactly like [`owner`](Self::owner): this crate has no build of
    /// its own to compare against, and the comparison belongs to whoever holds the daemon's identity.
    /// What it buys is that the comparison is POSSIBLE at all — a reporter is a separate process
    /// that a rebuild replaces under a running daemon, so *"is this reporter my image?"* had no
    /// answer anywhere.
    ///
    /// ⚠⚠⚠ `None` is *"it did not say"* and never *"it matches"*. Every reporter that predates the
    /// key answers `None`, and so does a person typing `sprag report-agent` by hand — collapsing
    /// that into agreement would make the commonest case look like the safe one.
    pub build: Option<String>,
}

/// What a [`report`](Tracker::report) did.
///
/// Two independent answers, and a caller needs both: a report can be ACCEPTED without CHANGING
/// anything (the agent says `working` twice), and a refused one changes nothing either — so
/// `changed` alone cannot tell a duplicate from a rejection, and only `accepted` can tell a
/// reporter that its clock has gone backwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReportOutcome {
    /// Whether the report was taken as authoritative. `false` for a stale sequence number from the
    /// source that is already speaking — a replayed or out-of-order message.
    pub accepted: bool,
    /// Whether the PUBLISHED verdict moved, which is what wakes a client and advances `seq`.
    pub changed: bool,
}

impl Default for Tracker {
    fn default() -> Self {
        Self::new(Hysteresis::default())
    }
}

impl Tracker {
    /// A tracker for one pane, with nothing published yet.
    #[must_use]
    pub fn new(policy: Hysteresis) -> Self {
        Self {
            policy,
            published: Verdict::default(),
            seq: 0,
            asked_seq: 0,
            said_seq: 0,
            reports: 0,
            pending: None,
            seen: None,
            identity: None,
            reported: None,
            owes_look: false,
        }
    }

    /// The verdict currently published for this pane.
    #[must_use]
    pub const fn verdict(&self) -> &Verdict {
        &self.published
    }

    /// How many times the published verdict has changed.
    #[must_use]
    pub const fn seq(&self) -> u64 {
        self.seq
    }

    /// **HOW MANY QUESTIONS THIS PANE HAS BEEN ASKED** — see [`asked_seq`](Self::asked_seq)'s field
    /// for why [`seq`](Self::seq) cannot answer it.
    ///
    /// A supervisor snapshots this before it types and compares afterwards: a value that has MOVED
    /// is the peer confirming, in its own words, that it took a new question — the one fact a
    /// screen cannot supply and a state counter cannot either.
    #[must_use]
    pub const fn asked_seq(&self) -> u64 {
        self.asked_seq
    }

    /// **HOW MANY ANSWERS THIS PANE'S AGENT HAS STATED** — see [`said_seq`](Self::said_seq)'s field.
    ///
    /// The other end of [`asked_seq`](Self::asked_seq): a supervisor snapshots this when it asks and
    /// requires it to have MOVED before it reads [`reported_said`](Self::reported_said) as an answer
    /// to that question, so a statement left over from an earlier turn cannot be judged as this
    /// one's.
    #[must_use]
    pub const fn said_seq(&self) -> u64 {
        self.said_seq
    }

    /// **HOW MANY REPORTS THIS PANE HAS ACCEPTED** — see [`reports`](Self::reports)' field for why
    /// none of the three counters above it can answer what this does.
    ///
    /// A supervisor snapshots it when a turn begins and compares it after the turn's own bound has
    /// gone by: a number that MOVED is the peer's reporter still speaking, however little the
    /// verdict changed, and a number that did not is a turn nothing will ever end.
    #[must_use]
    pub const fn reports(&self) -> u64 {
        self.reports
    }

    /// When a pending candidate would be published, or `None` when nothing is pending.
    ///
    /// The instant is derived (`since + settle`) rather than stored, so a policy that changes while a
    /// candidate is pending moves the deadline it is already waiting on — which is what a user who
    /// just shortened the window means by shortening it. See [`set_policy`](Self::set_policy).
    ///
    /// A caller uses this to know when to [`observe`](Self::observe) again; see the type's own docs
    /// for why the answer cannot be left to whatever else happens to be moving.
    #[must_use]
    pub fn pending_deadline(&self) -> Option<Instant> {
        self.pending
            .as_ref()
            .map(|pending| pending.since + self.policy.settle)
    }

    /// Replace the settle policy.
    ///
    /// Exists because the window is a user OPTION and this project's options are read from the file
    /// on every call rather than held — the daemon is a reader of the user's config, not an owner of
    /// it, so `set-option` takes effect with nothing to restart. A tracker built once at a pane's
    /// first sighting would otherwise pin the window at whatever the file said that day.
    ///
    /// It does NOT touch a pending candidate's `since`: the window is how long the answer must hold,
    /// and re-dating the evidence because the policy moved would restart a wait the pane has already
    /// served — the same defect as re-starting the window on every re-observation, which
    /// `a_pane_that_keeps_repainting_settles_when_the_candidate_has_held_long_enough` pins.
    pub const fn set_policy(&mut self, policy: Hysteresis) {
        self.policy = policy;
    }

    /// Take a reading of the pane and return what is published for it now.
    ///
    /// Call it on every tick. Two things can happen without the rules running at all, and both are
    /// deliberate:
    ///
    /// * **Nothing the rules read has moved and nothing is pending** — the answer cannot have
    ///   changed, so it is not recomputed. The skip is exact, not a sampling policy.
    /// * **Nothing has moved but a transition IS pending** — the rules still do not run, because
    ///   with both their inputs unchanged they would reach the candidate they already reached. What
    ///   has moved is the CLOCK, which is a third input while a transition is pending, and asking
    ///   it is what keeps a pane that went quiet mid-transition from freezing in its previous state
    ///   forever.
    /// * **The RULES have moved** — a user edited `config.toml`, so the list is not the list that
    ///   produced the published verdict. The screen and the title are unchanged and the answer can
    ///   still differ, which is why [`Ruleset`] carries its identity into the key rather than
    ///   leaving the caller to remember to invalidate anything.
    pub fn observe(
        &mut self,
        screen: &Screen,
        title: Option<&str>,
        rules: &Ruleset,
        now: Instant,
    ) -> &Verdict {
        let title = title.unwrap_or_default();
        let revision = rules.revision();
        if self
            .seen
            .as_ref()
            .is_some_and(|seen| seen.unchanged(screen, title, revision))
        {
            self.settle(now);
            return &self.published;
        }
        if let Some(seen) = &mut self.seen {
            seen.refresh(screen, title, revision);
        } else {
            self.seen = Some(Seen::of(screen, title, revision));
        }
        // ⚠⚠⚠⚠⚠ A REPORTED PANE IS STILL NOT EVALUATED BY THE RULES — but ONE fact on the screen
        // outranks a report older than it, and that fact is an unanswered dialog. Register item 524.
        //
        // What this rule cost before it existed, measured on a live run: an agent stood at a
        // permission dialog for **five hours and twenty minutes** while every surface said
        // `working`. The daemon HAD the fact three ways — the hook maps `Notification` to `Blocked`,
        // the attention ledger counted the ask, and the screen itself was a menu — and *a report
        // outranks the screen* beat all three, because the rule had **no clock**: it compared
        // authorities and never asked which was NEWER.
        //
        // ⚠⚠⚠ The recency is structural rather than a stored timestamp, and that is why no clock is
        // needed here: this line is past the unchanged-skip, so the screen being read HAS MOVED
        // since the last look, and a report that arrived before it is by construction the older
        // fact. A report that arrives after this look wins the ordinary way — `report` publishes on
        // the spot.
        //
        // ⚠⚠ COST, stated: this is [`question`], which reads the bottom [`DIALOG_WINDOW`] logical
        // lines and looks for a numbered choice run — bounded, and nothing like running every
        // pattern of every manifest, which is what the skip above exists to avoid.
        //
        // ⚠ It goes through [`consider`] like any other screen candidate rather than publishing on
        // the spot, so the settle window still guards it: a dialog glimpsed in one sample between
        // repaints is not yet an answer a person is owed.
        if let Some(held) = &self.reported {
            let asking = crate::question(screen, crate::DIALOG_WINDOW).is_some();
            let candidate = Verdict {
                state: if asking {
                    AgentState::Blocked
                } else {
                    held.state
                },
                agent: self.identity.clone(),
                // ⚠ NAMED, so a reader can tell this apart from both an ordinary scrape and a plain
                // report: the wire then carries `source` (who reported) AND this rule (what
                // overruled them), which is the whole diagnosis in one line.
                rule: asking.then(|| DIALOG_OUTRANKS_REPORT.to_owned()),
            };
            self.consider(candidate, now);
            return &self.published;
        }
        // A look re-derived the answer, so whatever owed one is served.
        self.owes_look = false;
        let candidate = self.evaluate(screen, title, rules.manifests());
        self.consider(candidate, now);
        &self.published
    }

    /// Take a REPORT from a process inside the pane: publish `state` at once and hold the screen off
    /// until the report is released.
    ///
    /// # Why this skips the settle window entirely
    ///
    /// [`Hysteresis`] exists for one reason, and it is a property of SCREENS: a resting verdict is
    /// asserted by the ABSENCE of a working signal, and the working signal is an animation, so an
    /// absence may be an artifact of the instant the sample was taken. A report is not a sample. The
    /// agent is not being observed between spinner frames — it is saying what it is doing, at the
    /// moment it starts or stops doing it. Making it wait would delay the one thing the report is
    /// better at than the scrape.
    ///
    /// Any pending candidate is dropped for the same reason a disagreeing observation drops one:
    /// something authoritative has settled the question the candidate was waiting to answer.
    ///
    /// # Freshness, and why a new speaker is not judged by the old one's clock
    ///
    /// A `seq` is compared only against the last one accepted from the SAME `source`. A reporter's
    /// sequence is its own monotonic clock, so comparing across sources would let one integration's
    /// numbering silence another's; and a pane runs one agent, so a second source appearing is a new
    /// authority rather than a competing sample of the same one. The newest speaker owns the pane,
    /// and a replay from the speaker that is already talking is refused.
    ///
    /// A report with no `seq` is always accepted: a caller with no clock (a person at a command
    /// line, a shell hook with no counter) has nothing to be stale against.
    ///
    /// # The owner, and why a report may carry one
    ///
    /// A report outranks the screen, so it must end for a reason, and that reason is not a clock:
    /// what expires is the REPORTER. [`Report::owner`] is an opaque token for the thing whose
    /// existence keeps this report standing — the caller decides what it means and asks, later,
    /// whether it is still there. `None` is a report that stands until it is released, which is what
    /// a person at a command line means by making one.
    pub fn report(&mut self, report: Report) -> ReportOutcome {
        let Report {
            state,
            agent,
            source,
            seq,
            owner,
            asked,
            said,
            noticed,
            transcript,
            build,
        } = report;
        if let Some(held) = &self.reported
            && held.source == source
            && let (Some(last), Some(incoming)) = (held.seq, seq)
            && incoming <= last
        {
            return ReportOutcome {
                accepted: false,
                changed: false,
            };
        }
        // ⚠⚠⚠⚠⚠ THE QUESTION IS COUNTED HERE, AND THIS PLACE IS THE WHOLE POINT — see `asked_seq`.
        // It is AFTER the staleness refusal (a replayed report is not a new question) and BEFORE
        // everything that decides whether the VERDICT moves, because whether the pane's state
        // changed has nothing to do with whether it was asked something. A submit arriving at an
        // already-`working` pane publishes nothing, and until this line that submit left no trace
        // any reader could find — which is register item 441's whole defect.
        if asked.is_some() {
            self.asked_seq += 1;
        }
        // ⚠⚠⚠⚠ AND THE ANSWER IS COUNTED IN THE SAME BREATH, for the same reason one line up: a
        // rest that states an answer may leave the verdict exactly where it was (a pane already
        // read as `idle` by its screen, a second `Stop` in a settle window), and a statement no
        // reader can date is one a supervisor cannot tell from the PREVIOUS turn's. See `said_seq`.
        if said.is_some() {
            self.said_seq += 1;
        }
        // ⚠⚠⚠⚠⚠ AND THE REPORT ITSELF IS COUNTED, WHATEVER IT SAID — register item 458, and the one
        // number that separates a peer working slowly from a peer that has stopped speaking.
        //
        // Every counter above it is about a STATEMENT (a question, an answer), and `seq` is about
        // the published VERDICT — so a turn calling tools reports `working` over and over while all
        // three stand still. That is the same reading as a turn whose agent was interrupted and will
        // never report again, and there was no fourth number to tell them apart.
        //
        // ⚠⚠ COUNTED AFTER THE STALENESS REFUSAL, so a replayed report is not a heartbeat: this
        // exists to answer *is anything still speaking for this pane*, and a message the tracker
        // already refused is not something speaking.
        self.reports += 1;
        // A reporter that names itself SETS the pane's identity; one that does not leaves it, and the
        // published verdict falls back to whatever the pane already was. That asymmetry is the same
        // one `evaluate` already keeps — the memory answers where the current evidence is silent about
        // WHO, never about what — and getting it wrong is not hypothetical: publishing `agent: None`
        // for a nameless report would run `publish`'s identity-clearing rule, so a `claude` pane whose
        // hook reported `idle` without repeating its name would come back from a release as nobody at
        // all. A test found it.
        if agent.is_some() {
            self.identity.clone_from(&agent);
        }
        let verdict = Verdict {
            state,
            agent: agent.or_else(|| self.identity.clone()),
            // A report names no RULE, and inventing one here would put a rule id on the wire that
            // fired nothing. `source` is what a reader asks instead — see `reported_source`.
            rule: None,
        };
        // Kept even when the state has not moved: the new `seq` is what the NEXT report is judged
        // against, so a duplicate still advances the reporter's clock.
        // ⚠⚠⚠ THE TWO STATED FACTS ARE CARRIED FORWARD, NOT REPLACED — see `Reported::asked`. Only
        // the event that opens a turn states them, so taking the incoming value unconditionally
        // would erase the prompt on the very next report, which arrives when the turn ENDS. `or`
        // rather than `unwrap_or`: a report that states one keeps it, a report that states nothing
        // leaves what stands.
        let carried = self.reported.take();
        self.reported = Some(Reported {
            state,
            source,
            seq,
            owner,
            asked: asked.or_else(|| carried.as_ref().and_then(|held| held.asked.clone())),
            said: said.or_else(|| carried.as_ref().and_then(|held| held.said.clone())),
            // ⚠⚠⚠⚠⚠ REPLACED, LIKE `build` BELOW AND UNLIKE THE TWO ABOVE — and the difference is
            // not stylistic. `asked` and `said` are the two ends of a TURN: each is stated once and
            // read afterwards, so carrying is what keeps them readable at all. A NOTICE is an
            // outstanding request, and the next report is the evidence it is no longer outstanding —
            // a peer that went back to `working` is not still asking for a person. Carried, it would
            // let a supervisor quote a dealt-with question at a pane blocked on something else, in
            // the peer's own voice, which is what would make the quote believed.
            noticed,
            transcript: transcript
                .or_else(|| carried.as_ref().and_then(|held| held.transcript.clone())),
            // ⚠⚠⚠⚠⚠ REPLACED, NEVER CARRIED — the opposite of its two neighbours above, and the
            // difference is what the field MEANS. Those two are EVENTS: only the report that opens a
            // turn states them, so carrying is what keeps them readable afterwards. This is a LEVEL
            // about whoever is reporting RIGHT NOW, stated on every report by the one reporter that
            // has a build to state.
            //
            // Carrying it would be a false claim of exactly the kind the field exists to catch: a
            // NEW reporter that says nothing would inherit the OLD one's identity, so a hook
            // replaced by a foreign one — the whole hazard — would go on answering the build of the
            // reporter it displaced. `None` here means *this reporter did not say*, which is the
            // honest answer and the only one that stays true when the reporter changes.
            build,
        });
        self.owes_look = false;
        let changed = verdict != self.published;
        if changed {
            self.publish(verdict);
        } else {
            // Nothing published, so nothing about the pending candidate has been decided by a
            // `publish` — but the question it was waiting on is answered all the same.
            self.pending = None;
        }
        ReportOutcome {
            accepted: true,
            changed,
        }
    }

    /// Give the pane back to the screen: drop any report in force and owe a fresh look.
    ///
    /// Answers whether a report was actually in force, so a caller can tell "stopped listening to a
    /// reporter" from "there was nobody to stop listening to".
    ///
    /// Two things are cleared, and the second is what makes the release take effect. The report goes,
    /// and so does the quiescence memory: without that, a pane whose screen has not moved since the
    /// report would be skipped by [`observe`](Self::observe)'s own exact-skip and would keep
    /// publishing the reported answer with nothing left to justify it. The published verdict is
    /// deliberately NOT cleared — it stays as the last thing anybody knew until a look replaces it,
    /// which is better than a flicker through `unknown` that no client asked for.
    pub fn release_report(&mut self) -> bool {
        let held = self.reported.take().is_some();
        if held {
            self.seen = None;
            self.owes_look = true;
        }
        held
    }

    /// Who is reporting this pane, or `None` when its state is inferred from the screen.
    ///
    /// Published beside the verdict so a reader can tell an authority from an inference — D7's rule
    /// (a gate that cannot say what it saw cannot be diagnosed) applied to the second kind of
    /// evidence.
    #[must_use]
    pub fn reported_source(&self) -> Option<&str> {
        self.reported.as_ref().map(|held| held.source.as_str())
    }

    /// **WHICH BUILD THE REPORTER IN FORCE SAID IT IS**, or `None` when there is no report or the
    /// reporter did not say.
    ///
    /// ⚠⚠⚠ The two `None`s are deliberately NOT distinguished here, on
    /// [`reported_owner`](Self::reported_owner)'s argument: a caller that needs to tell *no report*
    /// from *a report that said nothing* asks [`reported_source`](Self::reported_source), which is
    /// `Some` exactly when a report is in force. What must never be collapsed is either `None` into
    /// *"the reporter matches"* — see [`Report::build`].
    #[must_use]
    pub fn reported_build(&self) -> Option<&str> {
        self.reported
            .as_ref()
            .and_then(|held| held.build.as_deref())
    }

    /// The token whose continued existence keeps the report in force, when there is a report and it
    /// named one.
    ///
    /// Two `None`s that a caller must NOT collapse: no report at all, and a report that stands until
    /// released. Both mean "this rule has nothing to do here", which is why one answer serves — but
    /// a caller that ever needs to tell them apart asks [`reported_source`](Self::reported_source),
    /// which is `Some` exactly when a report is in force.
    #[must_use]
    pub fn reported_owner(&self) -> Option<u64> {
        self.reported.as_ref().and_then(|held| held.owner)
    }

    /// **THE LAST PROMPT THIS PANE'S AGENT SAID IT WAS ASKED**, or `None` if none ever has.
    ///
    /// ⚠⚠⚠ The answer to *did my question arrive*, from the only party that can give one. A screen
    /// shows the same pixels for text a run delivered and text a composer already held, so no
    /// reading of a terminal can tell them apart; this can.
    #[must_use]
    pub fn reported_asked(&self) -> Option<&str> {
        self.reported
            .as_ref()
            .and_then(|held| held.asked.as_deref())
    }

    /// **THE LAST ANSWER THIS PANE'S AGENT SAID IT GAVE**, or `None` if none ever has.
    ///
    /// ⚠⚠⚠⚠ The answer to *what did the peer just say*, from the only party that can give one —
    /// measured, not assumed: a full-screen agent repaints, so its pane's logical-line addresses
    /// stop advancing and *what did this turn print* answers `0` for the rest of the session while
    /// the agent goes on replying (register item 441). The screen holds the words and cannot be
    /// asked for them.
    ///
    /// ⚠ It says nothing about WHEN — pair it with [`said_seq`](Self::said_seq), which is what dates
    /// a statement to a turn.
    #[must_use]
    pub fn reported_said(&self) -> Option<&str> {
        self.reported.as_ref().and_then(|held| held.said.as_deref())
    }

    /// **WHY THIS PANE'S AGENT SAYS IT WANTS A PERSON**, or `None` where the report in force did not
    /// say — which is every report but the one that asks.
    ///
    /// ⚠⚠⚠ UNDATED ON PURPOSE, and it needs no dating: unlike [`reported_said`](Self::reported_said)
    /// this is not carried across reports, so anything standing here belongs to the report in force
    /// and to no earlier one. A supervisor reads it beside the published state and needs no watermark
    /// to know it is current.
    #[must_use]
    pub fn reported_noticed(&self) -> Option<&str> {
        self.reported
            .as_ref()
            .and_then(|held| held.noticed.as_deref())
    }

    /// **WHERE THIS PANE'S AGENT SAID IT IS WRITING**, or `None` if none ever has.
    #[must_use]
    pub fn reported_transcript(&self) -> Option<&str> {
        self.reported
            .as_ref()
            .and_then(|held| held.transcript.as_deref())
    }

    /// Whether this pane owes a fresh look that no screen event will ask for.
    ///
    /// A released pane is settled, known, and evaluated under the current rules, so every other
    /// question a caller asks about "does this pane need attention" answers `false` — while its
    /// published verdict is a report nobody stands behind any more. This is the third such reason,
    /// beside a due candidate and a replaced ruleset, and it is the same shape: an input moved that
    /// was not on the screen.
    #[must_use]
    pub const fn owes_look(&self) -> bool {
        self.owes_look
    }

    /// Which rules produced the published verdict, or `None` for a pane never observed.
    ///
    /// The waker's test for "this pane's answer was reached under a list that has since been
    /// replaced". A pane can be neither due nor unknown and still owe an evaluation, which is the
    /// third reason to ask about a pane and the one slice 4 added.
    #[must_use]
    pub fn evaluated_under(&self) -> Option<u64> {
        self.seen.as_ref().map(|seen| seen.rules)
    }

    /// The verdict this frame argues for, with the memory consulted where the screen has gone
    /// silent about who the pane belongs to.
    ///
    /// Identification is [`detect`](crate::detect)'s rather than a second walk of the list here,
    /// and the reason is the same one [`Rule::id`](crate::Rule::id) gives for keeping arbitration in
    /// one function: a second matcher is a matcher that can disagree. It also leaves exactly one
    /// place where the rules run, which is what lets [`work`](crate::work) meter the cost the
    /// quiescence gate exists to avoid — see [`DetectWork`](crate::DetectWork). A manifest that
    /// claims a pane always names itself on the verdict, so `agent.is_some()` is precisely "the
    /// list claimed this pane".
    fn evaluate(&mut self, screen: &Screen, title: &str, manifests: &[Manifest]) -> Verdict {
        let claimed = crate::detect(screen, Some(title), manifests);
        if claimed.agent.is_some() {
            if self.identity != claimed.agent {
                self.identity.clone_from(&claimed.agent);
            }
            return claimed;
        }
        // Nothing claims the pane — which is what a `codex` pane looks like the moment a modal
        // covers the composer and the footer (R251). A pane that was an agent a moment ago is
        // still that agent, so the remembered manifest is asked directly.
        //
        // Only for an ACTIVE verdict, and that bound is the same measurement read the other way:
        // at rest a `codex` title is the bare working directory and its screen is a prompt line,
        // which is what a SHELL looks like too. Without the fingerprint there is nothing left to
        // tell an agent at rest from the shell that outlived it, so a resting pane is not asserted
        // to still be anybody — while a dialog or a spinner is evidence the remembered agent is
        // still there, and is exactly the state the memory exists to rescue.
        self.identity
            .as_deref()
            .and_then(|name| manifests.iter().find(|m| m.name == name))
            .map(|manifest| manifest.verdict(screen, title))
            .filter(|verdict| verdict.state.is_active())
            .unwrap_or_default()
    }

    /// Weigh a fresh candidate against what is published.
    fn consider(&mut self, candidate: Verdict, now: Instant) {
        if candidate == self.published {
            // The pane moved without changing what it MEANS — the next spinner frame, another line
            // of output. Whatever transition was pending is off, because something disagreed with
            // it.
            self.pending = None;
            return;
        }
        if candidate.state.is_active() {
            // Positive evidence: a spinner frame in the title, a choice list on the screen. There
            // is no sampling artifact that INVENTS one of those, and the state a person is waiting
            // for is the one it would be perverse to delay.
            self.publish(candidate);
            return;
        }
        let restart = !matches!(&self.pending, Some(pending) if pending.candidate == candidate);
        if restart {
            self.pending = Some(Pending {
                candidate,
                since: now,
            });
        }
        self.settle(now);
    }

    /// Publish a pending candidate once it has held for the settle window.
    fn settle(&mut self, now: Instant) {
        let Some(pending) = &self.pending else {
            return;
        };
        if now.saturating_duration_since(pending.since) >= self.policy.settle {
            let candidate = pending.candidate.clone();
            self.publish(candidate);
        }
    }

    fn publish(&mut self, verdict: Verdict) {
        self.pending = None;
        // Let the identity go exactly when the published answer is that nobody claims this pane.
        // The memory and the wire then agree, and a pane an agent has EXITED cannot go on being
        // reported as that agent the next time something dialog-shaped appears in it.
        if verdict.agent.is_none() {
            self.identity = None;
        }
        self.published = verdict;
        self.seq += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentState, claude, codex, detect};
    use sprag_vt::{Emulator, VtPort};

    /// The measurement the settle window is sized against: R249's M2 sampled `claude`'s working
    /// title alternating between two braille frames at about 1 Hz.
    const MEASURED_SPINNER_PERIOD: Duration = Duration::from_secs(1);

    fn painted(lines: &[&str]) -> Emulator {
        let mut em = Emulator::new(80, 24);
        em.advance(lines.join("\r\n").as_bytes());
        em
    }

    /// Repaint the SAME pane, the way an agent redrawing its screen does — so the row damage
    /// stamps move exactly as they would live, rather than being compared across two emulators
    /// that never shared a generation counter.
    fn repaint(em: &mut Emulator, lines: &[&str]) {
        em.advance(b"\x1b[2J\x1b[H");
        em.advance(lines.join("\r\n").as_bytes());
    }

    /// Enough of a `claude` pane to be claimed by its footer fingerprint with no title at all.
    const CLAUDE_FOOTER: &[&str] = &["❯", "  ⏸ manual mode on · ? for shortcuts"];

    /// A `claude` composer HOLDING an unsubmitted paste — the placeholder, the box, and the hint
    /// that replaces the footer while it holds. Captured from a live 2.1.243 (register item 669);
    /// the rule's fidelity is that crate's own business, and this is the smallest screen it fires
    /// on.
    const HELD_COMPOSER: &[&str] = &[
        "──────────────────────────────",
        "❯ [Pasted text #3 +13 lines]",
        "──────────────────────────────",
        "  paste again to expand",
    ];

    /// The smallest screen that is a choice list. The dialog RULE's fidelity to a real agent is
    /// slice 1's business, proven there against three captured `claude` dialogs and three `codex`
    /// ones; these tests are about time, and need only a screen the rule fires on.
    const DIALOG: &[&str] = &["❯ 1. Yes", "  2. No"];

    /// A `codex` pane at rest: the composer line and the footer shape, which is the conjunction its
    /// fingerprint is made of.
    const CODEX_AT_REST: &[&str] = &[
        "› Write tests for @filename",
        "  gpt-5.6-sol default · /tmp",
    ];

    /// A `codex` modal, which covers both halves of that conjunction — the R251 finding, in the
    /// smallest form that reproduces it. Every test using it asserts that it really is unclaimed
    /// rather than assuming it.
    const CODEX_MODAL: &[&str] = &[
        "  Select Model",
        "› 1. gpt-5.6-sol (current)",
        "  2. gpt-5.5",
    ];

    #[test]
    fn the_default_settle_window_outlasts_the_measured_spinner_period() {
        assert!(
            DEFAULT_SETTLE > MEASURED_SPINNER_PERIOD,
            "a window shorter than the animation it guards against publishes the flicker it exists \
             to absorb",
        );
    }

    /// D9: the deadline is the tracker's whole answer to "when should somebody ask me again", so it
    /// has to be absent exactly when nothing is waiting and exact when something is.
    #[test]
    fn the_pending_deadline_is_the_instant_a_waiting_candidate_would_publish() {
        let rules = Ruleset::new(vec![claude()]);
        let mut tracker = Tracker::default();
        let mut em = painted(CLAUDE_FOOTER);
        let base = Instant::now();

        // A dialog publishes on sight, so nothing is left waiting.
        repaint(&mut em, DIALOG);
        tracker.observe(em.screen(), Some("✳ Claude Code"), &rules, base);
        assert_eq!(tracker.verdict().state, AgentState::Blocked);
        assert_eq!(
            tracker.pending_deadline(),
            None,
            "a verdict published on sight leaves nothing to come back for",
        );

        // The dialog goes away: a return to rest is an ABSENCE, so it waits — and says until when.
        repaint(&mut em, CLAUDE_FOOTER);
        tracker.observe(em.screen(), Some("✳ Claude Code"), &rules, base);
        assert_eq!(
            tracker.pending_deadline(),
            Some(base + DEFAULT_SETTLE),
            "the deadline is when the candidate was first seen plus the window",
        );
        assert_eq!(
            tracker.verdict().state,
            AgentState::Blocked,
            "and it has NOT published yet — otherwise the deadline above is describing nothing",
        );

        // Asked at the deadline, it publishes and the deadline is spent.
        tracker.observe(
            em.screen(),
            Some("✳ Claude Code"),
            &rules,
            base + DEFAULT_SETTLE,
        );
        assert_eq!(tracker.verdict().state, AgentState::Idle);
        assert_eq!(tracker.pending_deadline(), None);
    }

    /// F3: the window is a user option, so it can move while a candidate is already waiting. It moves
    /// the DEADLINE and not the evidence — a pane that has already held for the new window is done
    /// waiting, rather than starting over because the user typed `set-option`.
    #[test]
    fn a_shortened_window_publishes_a_candidate_that_has_already_held_long_enough() {
        let rules = Ruleset::new(vec![claude()]);
        let mut tracker = Tracker::default();
        let mut em = painted(CLAUDE_FOOTER);
        let base = Instant::now();

        repaint(&mut em, DIALOG);
        tracker.observe(em.screen(), Some("✳ Claude Code"), &rules, base);
        repaint(&mut em, CLAUDE_FOOTER);
        tracker.observe(em.screen(), Some("✳ Claude Code"), &rules, base);
        assert_eq!(tracker.pending_deadline(), Some(base + DEFAULT_SETTLE));

        let shorter = Duration::from_millis(250);
        tracker.set_policy(Hysteresis { settle: shorter });
        assert_eq!(
            tracker.pending_deadline(),
            Some(base + shorter),
            "the candidate keeps the instant it was first seen; only the window moved",
        );

        // Half of the OLD window is already twice the new one, so the next reading publishes.
        tracker.observe(
            em.screen(),
            Some("✳ Claude Code"),
            &rules,
            base + DEFAULT_SETTLE / 2,
        );
        assert_eq!(
            tracker.verdict().state,
            AgentState::Idle,
            "time already served counts against the new window",
        );
    }

    /// The list exists so a caller compiles the patterns once; if it ever stops carrying every
    /// built-in, a pane of that agent goes unclaimed on the wire with nothing failing here.
    #[test]
    fn the_built_in_list_carries_every_built_in_manifest() {
        let names: Vec<String> = crate::built_ins().into_iter().map(|m| m.name).collect();
        assert_eq!(names, vec![claude().name, codex().name]);
    }

    /// The gate H3's design named for this slice: one animation is one publication.
    #[test]
    fn a_spinner_animation_publishes_one_working_not_six() {
        let rules = Ruleset::new(vec![claude()]);
        let mut tracker = Tracker::default();
        let mut em = painted(CLAUDE_FOOTER);
        let base = Instant::now();

        for (tick, frame) in ["⠂", "⠐", "⠂", "⠐", "⠂", "⠐"].iter().enumerate() {
            let title = format!("{frame} Run sleep command for 25 seconds");
            let now = base + MEASURED_SPINNER_PERIOD * u32::try_from(tick).expect("six of them");
            let verdict = tracker.observe(em.screen(), Some(&title), &rules, now);
            assert_eq!(verdict.state, AgentState::Working, "frame {frame}");
        }
        assert_eq!(
            tracker.seq(),
            1,
            "six frames of one animation are one publication",
        );

        // Non-vacuity: a tracker that had frozen after the first frame would have passed every
        // assertion above. This one is still reading the pane.
        repaint(&mut em, DIALOG);
        let verdict = tracker.observe(
            em.screen(),
            Some("⠂ Run sleep command for 25 seconds"),
            &rules,
            base + Duration::from_secs(6),
        );
        assert_eq!(verdict.state, AgentState::Blocked);
        assert_eq!(tracker.seq(), 2);
    }

    /// The RULES are the quiescence key's fourth term, and this test replaces the one R252 wrote.
    ///
    /// R252 proved the skip by rewriting a rule underneath the tracker and asserting the verdict did
    /// NOT move: only an evaluation could notice the rewrite, so a frozen verdict meant the rules had
    /// not run. That instrument is the defect slice 4 exists to fix — a user edits `config.toml` to
    /// correct a rule that is misfiring, and the pane they are watching is quiet, which is why the
    /// wrong answer is visible and stuck there. So the assertion is INVERTED here.
    ///
    /// What is lost with it is the only behavioural view of the skip, and that is worth stating
    /// rather than discovering: an exact skip is one whose absence changes no answer, so with the
    /// rules in the key there is nothing left for a test to see. The gate's remaining proof is the
    /// evaluation it does not run, which is a COST, and cost is what H3's open measurement 3
    /// instruments.
    #[test]
    fn a_rewritten_rule_reaches_a_pane_that_has_not_moved() {
        let mut rewritten = claude();
        rewritten
            .rules
            .iter_mut()
            .find(|rule| rule.id == "idle-glyph")
            .expect("the manifest has an idle rule")
            .state = AgentState::Working;

        let mut tracker = Tracker::default();
        let em = painted(CLAUDE_FOOTER);
        let title = Some("✳ Claude Code");
        let base = Instant::now();

        let rules = Ruleset::new(vec![claude()]);
        tracker.observe(em.screen(), title, &rules, base);
        tracker.observe(em.screen(), title, &rules, base + DEFAULT_SETTLE);
        assert_eq!(tracker.verdict().state, AgentState::Idle);
        let generations: Vec<Option<u64>> = (0..em.screen().rows())
            .map(|row| em.screen().row_generation(row))
            .collect();

        let edited = Ruleset::new(vec![rewritten]);
        tracker.observe(em.screen(), title, &edited, base + DEFAULT_SETTLE * 2);
        assert_eq!(
            tracker.verdict().state,
            AgentState::Working,
            "a replaced rule list is a changed input, so the answer is recomputed",
        );

        // The premise, asserted rather than assumed — the same discipline R252's resize test
        // applies. If the pane had repainted, the re-evaluation would be explained by the screen
        // and this test would prove nothing about the rules term.
        assert_eq!(
            generations,
            (0..em.screen().rows())
                .map(|row| em.screen().row_generation(row))
                .collect::<Vec<_>>(),
            "not one row moved between the two verdicts",
        );
    }

    /// `codex`'s sign-in picker, which NAMES the product — the screen the title-free fingerprint
    /// was built for.
    const CODEX_SIGNIN: &[&str] = &[
        "  Welcome to Codex, OpenAI's command-line coding agent",
        "> 1. Sign in with ChatGPT",
        "  2. Sign in with Device Code",
    ];

    /// ...and the directory-trust dialog that follows it, which names NOBODY. Any program could ask
    /// this question, so no fingerprint can claim it and only the memory can.
    const CODEX_TRUST: &[&str] = &[
        "  Do you trust the contents of this directory?",
        "› 1. Yes, continue",
        "  2. No, quit",
    ];

    /// The onboarding SEQUENCE, which is worth more than either screen alone.
    ///
    /// Slice 1 shipped `codex`'s trust dialog as an unclaimable screen and it still is one: nothing
    /// on it belongs to `codex` rather than to any program that could ask the same question. What
    /// closes it is not a better fingerprint but the ORDER a user meets it in — sign-in first, and
    /// that screen does name the product. Once the pane has been claimed, the identity memory the
    /// modal case (R251) was built for carries it through.
    ///
    /// So the two mechanisms compose: the fingerprint reaches a screen the memory could not have
    /// been seeded from, and the memory reaches a screen no fingerprint can match. The premise is
    /// asserted first, because a test whose second half passes for the wrong reason would look
    /// exactly like this one.
    #[test]
    fn a_remembered_agent_carries_through_the_dialog_that_names_nobody() {
        let rules = Ruleset::new(vec![codex()]);
        let base = Instant::now();

        // THE PREMISE: on its own, the trust dialog is claimed by nobody.
        let mut cold = Tracker::default();
        let alone = painted(CODEX_TRUST);
        cold.observe(alone.screen(), None, &rules, base);
        assert_eq!(
            cold.verdict().agent,
            None,
            "the screen really is unclaimable, so the sequence below is what does the work",
        );

        // The pane a user actually has: sign-in, then trust.
        let mut tracker = Tracker::default();
        let mut em = painted(CODEX_SIGNIN);
        tracker.observe(em.screen(), None, &rules, base);
        assert_eq!(
            tracker.verdict().agent.as_deref(),
            Some("codex"),
            "the picker names the product, so the fingerprint reaches it with no title at all",
        );

        repaint(&mut em, CODEX_TRUST);
        let after = tracker.observe(em.screen(), None, &rules, base + Duration::from_millis(50));
        assert_eq!(
            after.agent.as_deref(),
            Some("codex"),
            "and the memory carries the identity onto the screen that names nobody",
        );
        assert_eq!(
            after.state,
            AgentState::Blocked,
            "which is the whole point: it is a question waiting on the user",
        );
    }

    /// THE MISS THAT IS NOT CLOSED, asserted at last — and slice 1 said it was.
    ///
    /// The crate docs have recorded a second measured miss since slice 1 (`codex` replaces its
    /// footer with a transient hint for a few seconds after `esc`, and its fingerprint is a
    /// conjunction over that footer) and claimed *"both are recorded here, and asserted by tests"*.
    /// Only the onboarding one ever was. There is no captured screen of the hint in this tree, so
    /// this test is written from the STRUCTURE the miss follows from rather than from a fixture
    /// pretending to be a measurement: the composer is there and the footer is not.
    ///
    /// What it pins is the reason the memory does not rescue this one, which is the part that looks
    /// like a bug and is not. `evaluate` asserts a remembered identity only for an ACTIVE verdict,
    /// and a pane that has just been `esc`-ed is at rest. Widening that filter would trade this miss
    /// for a worse one: at rest a `codex` screen is a prompt line and its title is a bare directory
    /// name, so a pane whose agent EXITED would be reported as that agent for as long as the shell
    /// that outlived it stayed quiet.
    ///
    /// REVERT-PROOF: drop the `is_active` filter in `evaluate` and this fails, together with
    /// `a_remembered_agent_that_shows_nothing_active_is_let_go` and
    /// `a_resize_that_truncates_a_row_is_not_quiescence`. Naming the co-casualties is the point
    /// rather than an aside: the filter already had a test, so what this one adds is not coverage of
    /// the filter but the connection between the filter and a MISS the crate docs describe — the two
    /// were recorded in different places and nothing said they were the same fact.
    #[test]
    fn a_resting_pane_that_loses_its_fingerprint_is_not_asserted_to_still_be_anybody() {
        let rules = Ruleset::new(vec![codex()]);
        let mut tracker = Tracker::default();
        let base = Instant::now();

        // Claimed normally first, so the memory HAS an identity to offer. `idle` rests on an
        // absence, so it takes the window to be published — hence the second observation.
        let mut em = painted(CODEX_AT_REST);
        tracker.observe(em.screen(), Some("codexprobe"), &rules, base);
        let claimed = tracker.observe(
            em.screen(),
            Some("codexprobe"),
            &rules,
            base + DEFAULT_SETTLE,
        );
        assert_eq!(
            claimed.agent.as_deref(),
            Some("codex"),
            "the composer-and-footer conjunction claims a pane at rest",
        );

        // The footer is replaced. Structurally this is the post-`esc` hint: the composer survives,
        // the footer conjunction cannot hold, and nothing about the screen is active.
        repaint(
            &mut em,
            &["› Write tests for @filename", "  press esc again to clear"],
        );
        let began = base + DEFAULT_SETTLE + Duration::from_millis(10);
        tracker.observe(em.screen(), Some("codexprobe"), &rules, began);
        let after = tracker.observe(
            em.screen(),
            Some("codexprobe"),
            &rules,
            began + DEFAULT_SETTLE,
        );
        assert_eq!(
            after.agent, None,
            "the memory declines to assert an identity for a pane that is merely quiet",
        );
    }

    /// A RELOAD DOES NOT RESTART A WAIT THE PANE HAS ALREADY SERVED.
    ///
    /// Every reload test written so far uses a SETTLED pane, which is the easy half: nothing is in
    /// flight, so nothing can be lost. The pane mid-transition is the one with a decision in it,
    /// and the decision is [`consider`](Tracker::consider)'s `restart` guard — a candidate the new
    /// rules still argue for keeps its original `since`.
    ///
    /// That is right because of what the window is FOR. It absorbs a sampling artifact of the
    /// SCREEN — one frame of a spinner — and the screen here has held for the whole window. The
    /// rules moving is not a reason to disbelieve evidence that has not moved. The tempting
    /// implementation, re-dating the candidate on every re-evaluation, would make a user who edits
    /// `config.toml` while a pane is going quiet wait a second window for a verdict the first one
    /// had already earned — and would do it invisibly, because the answer that eventually arrives
    /// is the right one.
    ///
    /// The assertion is the DEADLINE, not the verdict: observing at exactly `since + settle` is
    /// what a re-dated candidate cannot satisfy.
    ///
    /// REVERT-PROOF: make `restart` unconditional and this fails at the deadline — together with
    /// `a_pane_that_keeps_repainting_settles_when_the_candidate_has_held_long_enough`, and that
    /// second casualty is worth naming rather than hiding. The guard was already load-bearing for a
    /// pane whose SCREEN keeps moving; what had never been asked is whether it holds when the thing
    /// that moves is the RULE LIST, which reaches `consider` by a different route and was the one
    /// input the guard was not written against.
    #[test]
    fn a_reload_mid_transition_keeps_the_window_the_candidate_has_already_served() {
        let mut tracker = Tracker::default();
        let mut em = painted(DIALOG);
        let base = Instant::now();

        // Blocked on sight — positive evidence is never delayed, so the pane starts from a
        // published state with nothing pending.
        let rules = Ruleset::new(vec![claude()]);
        tracker.observe(em.screen(), Some("✳ Claude Code"), &rules, base);
        assert_eq!(tracker.verdict().state, AgentState::Blocked);

        // The dialog is answered: the pane now argues for `idle`, which rests on an ABSENCE and so
        // has to hold for the window.
        repaint(&mut em, CLAUDE_FOOTER);
        let began = base + Duration::from_millis(10);
        tracker.observe(em.screen(), Some("✳ Claude Code"), &rules, began);
        assert_eq!(
            tracker.verdict().state,
            AgentState::Blocked,
            "the candidate is pending, not published",
        );
        assert_eq!(tracker.pending_deadline(), Some(began + DEFAULT_SETTLE));

        // HALFWAY THROUGH THE WINDOW the user edits an unrelated part of the file. A new list, a
        // new revision, and the same verdict for this screen.
        let reloaded = Ruleset::new(vec![claude()]);
        tracker.observe(
            em.screen(),
            Some("✳ Claude Code"),
            &reloaded,
            began + DEFAULT_SETTLE / 2,
        );
        assert_eq!(
            tracker.pending_deadline(),
            Some(began + DEFAULT_SETTLE),
            "the deadline is the one the candidate earned, not one re-dated by the edit",
        );

        tracker.observe(
            em.screen(),
            Some("✳ Claude Code"),
            &reloaded,
            began + DEFAULT_SETTLE,
        );
        assert_eq!(
            tracker.verdict().state,
            AgentState::Idle,
            "so it publishes on time",
        );
    }

    /// ...and a reload the pane DISAGREES with does restart it, which is the same rule read from
    /// the other side: the window is per-candidate, so a different answer serves its own wait.
    ///
    /// Written because the guard above is only half a claim. An implementation that never restarted
    /// would satisfy the test above and would publish a brand-new candidate the instant an older
    /// one's window happened to elapse — a verdict with no evidence behind it at all.
    #[test]
    fn a_reload_the_pane_disagrees_with_starts_the_window_again() {
        let mut tracker = Tracker::default();
        let mut em = painted(DIALOG);
        let base = Instant::now();

        let rules = Ruleset::new(vec![claude()]);
        tracker.observe(em.screen(), Some("✳ Claude Code"), &rules, base);
        assert_eq!(tracker.verdict().state, AgentState::Blocked);

        repaint(&mut em, CLAUDE_FOOTER);
        let began = base + Duration::from_millis(10);
        tracker.observe(em.screen(), Some("✳ Claude Code"), &rules, began);
        assert_eq!(tracker.pending_deadline(), Some(began + DEFAULT_SETTLE));

        // The user disables the rule the pending candidate came from. The same screen now argues
        // for a DIFFERENT answer, so the evidence for the old one is gone.
        let mut stripped = claude();
        stripped.rules.retain(|rule| rule.id != "idle-glyph");
        let edited = Ruleset::new(vec![stripped]);
        let moved = began + DEFAULT_SETTLE / 2;
        tracker.observe(em.screen(), Some("✳ Claude Code"), &edited, moved);
        assert_eq!(
            tracker.pending_deadline(),
            Some(moved + DEFAULT_SETTLE),
            "a candidate nothing has argued for before starts its own window",
        );
        assert_eq!(
            tracker.verdict().state,
            AgentState::Blocked,
            "and nothing is published until it has served it",
        );
    }

    /// The same list, offered again, is not a change — the other direction of the term above, and the
    /// one that keeps it from being "re-evaluate always" wearing a key's costume.
    ///
    /// This is also slice 3's two-clients-one-wake property read from underneath: two readers of one
    /// pane observe against the same ruleset, so the second call must publish nothing new.
    #[test]
    fn the_same_ruleset_offered_twice_publishes_nothing_new() {
        let mut tracker = Tracker::default();
        let em = painted(CLAUDE_FOOTER);
        let title = Some("✳ Claude Code");
        let base = Instant::now();
        let rules = Ruleset::new(vec![claude()]);

        tracker.observe(em.screen(), title, &rules, base);
        tracker.observe(em.screen(), title, &rules, base + DEFAULT_SETTLE);
        assert_eq!(tracker.verdict().state, AgentState::Idle);
        let settled = tracker.seq();

        tracker.observe(em.screen(), title, &rules, base + DEFAULT_SETTLE * 2);
        assert_eq!(
            tracker.seq(),
            settled,
            "the same rules on an unmoved pane are the same answer",
        );
    }

    /// A resize is a content change no damage stamp records, and the test asserts that premise
    /// rather than assuming it — if `sprag-vt` ever stamps rows on resize, this says the premise
    /// died instead of quietly becoming vacuous.
    #[test]
    fn a_resize_that_truncates_a_row_is_not_quiescence() {
        let rules = Ruleset::new(vec![claude()]);
        let mut tracker = Tracker::default();
        let mut em = Emulator::new(80, 24);
        // The alternate screen, because that is the resize path that copies the stamps verbatim
        // (`Screen::resized`); the main screen reflows and stamps every row afresh.
        em.advance(b"\x1b[?1049h");
        em.advance(CLAUDE_FOOTER.join("\r\n").as_bytes());
        let base = Instant::now();

        tracker.observe(em.screen(), None, &rules, base);
        let verdict = tracker.observe(em.screen(), None, &rules, base + DEFAULT_SETTLE);
        assert_eq!(
            verdict.agent.as_deref(),
            Some("claude"),
            "claimed by the footer fingerprint, with no title at all",
        );

        let before: Vec<Option<u64>> = (0..em.screen().rows())
            .map(|row| em.screen().row_generation(row))
            .collect();
        em.resize(20, 24);
        let after: Vec<Option<u64>> = (0..em.screen().rows())
            .map(|row| em.screen().row_generation(row))
            .collect();
        assert_eq!(
            before, after,
            "the premise: this resize moved no damage stamp, so only the width can carry it",
        );

        tracker.observe(em.screen(), None, &rules, base + DEFAULT_SETTLE);
        tracker.observe(em.screen(), None, &rules, base + DEFAULT_SETTLE * 2);
        assert_eq!(
            tracker.verdict().agent,
            None,
            "the footer was truncated away, and the rules were asked because the width moved",
        );
    }

    /// The pending exception: a pane that goes quiet mid-transition still settles, because while a
    /// transition is pending the clock is an input.
    #[test]
    fn a_pending_transition_settles_on_the_clock_although_nothing_moved() {
        let rules = Ruleset::new(vec![claude()]);
        let mut tracker = Tracker::default();
        let em = painted(CLAUDE_FOOTER);
        let base = Instant::now();

        tracker.observe(em.screen(), Some("⠂ Compacting"), &rules, base);
        assert_eq!(tracker.verdict().state, AgentState::Working);

        let rested = Some("✳ Compacting");
        let began = base + Duration::from_millis(100);
        tracker.observe(em.screen(), rested, &rules, began);
        assert_eq!(
            tracker.verdict().state,
            AgentState::Working,
            "an absence has to hold before it is believed",
        );

        // From here NOTHING moves: same screen, same title. Only the clock advances.
        tracker.observe(em.screen(), rested, &rules, began + DEFAULT_SETTLE / 2);
        assert_eq!(
            tracker.verdict().state,
            AgentState::Working,
            "and it has not held long enough yet",
        );

        tracker.observe(em.screen(), rested, &rules, began + DEFAULT_SETTLE);
        assert_eq!(tracker.verdict().state, AgentState::Idle);
        assert_eq!(tracker.seq(), 2, "one transition, one publication");
    }

    /// The window is measured from when a candidate was FIRST seen, not from the last time it was
    /// seen again. A pane that keeps repainting — an agent printing its transcript — re-reaches the
    /// same candidate on every tick, and a window restarted by each of those never expires at all:
    /// the pane would be stuck in its previous state for as long as it stayed busy, which is the
    /// same freeze the pending exception exists to prevent, arriving by the other door.
    /// ⚠⚠⚠⚠⚠ **A QUESTION IS COUNTED EVEN WHEN THE VERDICT DOES NOT MOVE** — register item 441,
    /// and the arm the whole counter exists for.
    ///
    /// # Why `seq` could not have carried this, staged rather than argued
    ///
    /// A supervisor's question is *did the peer take what I just typed?* The submit that would
    /// answer it arrives at a pane that is ALREADY `working` — an agent mid-turn — and reports
    /// `working` again. The verdict is identical, so nothing publishes and [`seq`](Tracker::seq)
    /// stands still. Every reader saw an unchanged pane while a new question had just been asked.
    ///
    /// What that cost, live: a loop typed into a busy agent, read the `idle` it owed to the PREVIOUS
    /// question as this turn's ending, judged a window the peer had not written in, and prompted
    /// again — thirty-three times, deaf to a marker the agent had printed.
    ///
    /// ⚠⚠⚠ **THE THIRD ARM IS THE ONE THAT KEEPS THEM TWO FACTS**: a report that states NO prompt
    /// must not advance it, or the counter becomes a second spelling of *something happened* and
    /// stops answering the only question it was added for.
    #[test]
    fn a_question_is_counted_even_when_the_pane_looks_unchanged() {
        let mut tracker = Tracker::new(Hysteresis::default());
        let asking = |asked: Option<&str>, seq: u64| Report {
            state: AgentState::Working,
            agent: Some("claude".to_owned()),
            source: "hook:claude".to_owned(),
            seq: Some(seq),
            owner: None,
            asked: asked.map(str::to_owned),
            said: None,
            noticed: None,
            transcript: None,
            build: None,
        };

        tracker.report(asking(Some("the first question"), 1));
        let (after_first, published) = (tracker.asked_seq(), tracker.seq());
        assert_eq!(after_first, 1, "the first question is counted");

        // ── THE ARM THAT MATTERS: the pane is already `working`, so this changes no verdict. ──
        let outcome = tracker.report(asking(Some("the second question"), 2));
        assert!(
            !outcome.changed,
            "⚠ THE STAGING: this report must NOT move the published verdict, or the gate is about \
             an easier case than the live one",
        );
        assert_eq!(
            tracker.seq(),
            published,
            "and `seq` must stand still with it — that standing still IS the defect",
        );
        assert_eq!(
            tracker.asked_seq(),
            after_first + 1,
            "⚠⚠⚠⚠⚠ BUT THE QUESTION IS COUNTED. This is the only observable difference between a \
             peer that took a new question and one that is still chewing the last, and without it a \
             contract waiting on the peer's rest cannot tell whose rest it is looking at. Deleting \
             the count in `report` leaves every other gate in this workspace green",
        );

        // ── AND A REPORT THAT STATES NOTHING DOES NOT COUNT, or the two facts collapse into one ──
        tracker.report(asking(None, 3));
        assert_eq!(
            tracker.asked_seq(),
            after_first + 1,
            "a report with no prompt in it is not a question — only the turn's opening event states \
             one, and every other report of that turn would otherwise inflate the count",
        );
    }

    /// ⚠⚠⚠⚠⚠ **A REQUEST THAT HAS BEEN DEALT WITH MUST NOT OUTLIVE THE REPORT THAT DEALT WITH IT** —
    /// register item 452, and the one design decision `noticed` makes that its neighbours do not.
    ///
    /// # Why this is the sharp end of the field and not a detail
    ///
    /// `asked` and `said` are CARRIED across reports that say nothing about them, because each is
    /// stated once — at a turn's two ends — and read afterwards. Carrying `noticed` on the same
    /// reflex would be a silent defect of a nastier kind: a supervisor reads it to tell a person WHY
    /// their peer wants them, **in the peer's own voice**, and a quotation is exactly the sort of
    /// evidence nobody re-checks. A notice a person answered an hour ago, re-quoted at a pane blocked
    /// on something else, would send them to a question that no longer exists and would sound
    /// authoritative doing it.
    ///
    /// ⚠⚠⚠ **AND THIS IS WHY NO `noticed_seq` SITS BESIDE [`said_seq`](Tracker::said_seq).** That
    /// counter exists because a carried statement cannot be dated. Nothing standing in `noticed` is
    /// ever older than the report in force, so there is no gap for a counter to close — the retirement
    /// asserted here is what makes the missing counter honest rather than an omission.
    ///
    /// ⚠ The two halves are asserted against each other in ONE tracker, so a change that made every
    /// stated fact behave alike goes red whichever direction it moved them in.
    #[test]
    fn a_notice_is_retired_by_the_next_report_although_the_turns_words_are_kept() {
        let mut tracker = Tracker::new(Hysteresis::default());
        let speaking =
            |state: AgentState, said: Option<&str>, noticed: Option<&str>, seq: u64| Report {
                state,
                agent: Some("claude".to_owned()),
                source: "hook:claude".to_owned(),
                seq: Some(seq),
                owner: None,
                asked: None,
                said: said.map(str::to_owned),
                noticed: noticed.map(str::to_owned),
                transcript: None,
                build: None,
            };

        // The turn ends with an answer, and then the peer asks for a person.
        tracker.report(speaking(
            AgentState::Idle,
            Some("the turn's answer"),
            None,
            1,
        ));
        tracker.report(speaking(
            AgentState::Blocked,
            None,
            Some("Claude needs your permission to use Bash"),
            2,
        ));
        assert_eq!(
            tracker.reported_noticed(),
            Some("Claude needs your permission to use Bash"),
            "⚠ THE STAGING: the notice has to be readable at all before its retirement means \
             anything",
        );
        assert_eq!(
            tracker.reported_said(),
            Some("the turn's answer"),
            "and the answer is still standing, carried past a report that stated none",
        );

        // ── THE ARM THAT MATTERS: somebody dealt with it, and the peer went back to work. ──
        tracker.report(speaking(AgentState::Working, None, None, 3));
        assert_eq!(
            tracker.reported_noticed(),
            None,
            "⚠⚠⚠⚠⚠ THE REQUEST IS GONE WITH THE REPORT THAT ANSWERED IT. Carrying it — the `or_else` \
             its two neighbours use — leaves a supervisor able to quote, at a pane blocked on \
             something else entirely, a question a person has already dealt with, in the peer's own \
             words. Every other gate in this workspace stays green under that one-line change",
        );
        assert_eq!(
            tracker.reported_said(),
            Some("the turn's answer"),
            "⚠⚠⚠⚠ AND THE ANSWER IS STILL CARRIED, which is what keeps this a decision about MEANING \
             rather than a rule about stated facts: a turn's words are read after the turn, a \
             request is read only while it is outstanding. A change that made the two behave alike \
             fails here whichever way it moved them",
        );
    }

    /// ⚠⚠⚠⚠⚠ **A PEER STILL SPEAKING AND A PEER THAT HAS STOPPED READ THE SAME ON EVERY OTHER
    /// COUNTER** — register item 458, and the arm this fourth number exists for.
    ///
    /// # What it costs to have no such number, measured
    ///
    /// A turn calling tool after tool reports `working` each time: the verdict never moves, so
    /// [`seq`](Tracker::seq) stands still; no prompt and no answer are stated, so
    /// [`asked_seq`](Tracker::asked_seq) and [`said_seq`](Tracker::said_seq) stand still too. **A
    /// turn a person stopped with Escape reads exactly the same** — and it reads that way for ever,
    /// because the agent restores the prompt into its composer and its idle nag is suppressed while
    /// the composer holds text, so nothing will ever speak for that pane again.
    ///
    /// Live, 2026-08-19: `working seq=6 asked=2 said=0`, unchanged across **fourteen minutes**,
    /// while the driver polled *"looked, nothing had happened"* toward a `max_seconds` the shipped
    /// kind authors at 24 hours. A person typed `exit` to end it.
    ///
    /// # ⚠⚠⚠ The second arm is what keeps it a HEARTBEAT rather than a message count
    ///
    /// A replayed report — the same speaker, a clock that went backwards — is refused, and a
    /// refusal must not tick this. What the number answers is *is anything still speaking for this
    /// pane*, and a message the tracker just threw away is not something speaking. Without that arm
    /// a stuck reporter retrying could look exactly like a live one.
    #[test]
    fn a_report_that_changes_nothing_still_says_something_is_speaking() {
        let mut tracker = Tracker::new(Hysteresis::default());
        let working = |seq: u64| Report {
            state: AgentState::Working,
            agent: Some("claude".to_owned()),
            source: "hook:claude".to_owned(),
            seq: Some(seq),
            owner: None,
            asked: None,
            said: None,
            noticed: None,
            transcript: None,
            build: None,
        };

        tracker.report(working(1));
        let (published, asked, said, beat) = (
            tracker.seq(),
            tracker.asked_seq(),
            tracker.said_seq(),
            tracker.reports(),
        );

        // ── THE ARM: a tool call on a pane that is already working. Nothing it states moves. ──
        let outcome = tracker.report(working(2));
        assert!(
            outcome.accepted && !outcome.changed,
            "⚠ THE STAGING: this report must be TAKEN and must move no verdict, or the gate is \
             about an easier case than the live one — {outcome:?}",
        );
        assert_eq!(
            (tracker.seq(), tracker.asked_seq(), tracker.said_seq()),
            (published, asked, said),
            "⚠⚠⚠ AND ALL THREE OF THE OLDER COUNTERS MUST STAND STILL. Their standing still IS the \
             defect: it is what makes a turn in flight and a turn nothing will ever end the same \
             three numbers",
        );
        assert_eq!(
            tracker.reports(),
            beat + 1,
            "⚠⚠⚠⚠⚠ BUT SOMETHING SPOKE. This is the only observable difference between a peer \
             working and a peer that was interrupted and will never report again — and a wait that \
             cannot draw it polls a dead turn toward a 24-hour clock. Deleting the count in \
             `report` leaves every other gate in this workspace green",
        );

        // ── AND A REFUSED REPLAY IS NOT A HEARTBEAT ──
        let replay = tracker.report(working(2));
        assert!(!replay.accepted, "the staleness rule still refuses it");
        assert_eq!(
            tracker.reports(),
            beat + 1,
            "⚠⚠⚠⚠ a message this tracker THREW AWAY must not read as something speaking, or a \
             reporter stuck retrying one report looks exactly like a live one — which is the \
             confusion this number was added to end",
        );
    }

    /// A REPORT publishes the moment it arrives, where the same answer from the screen has to hold
    /// for the settle window first.
    ///
    /// The two halves are the test: hysteresis exists because a resting verdict rests on the absence
    /// of an ANIMATED signal, and that reasoning does not apply to a process saying what it is doing.
    /// Without the control this would only show that reports are published, not that they skip a wait
    /// the screen cannot.
    #[test]
    fn a_report_publishes_at_once_where_the_screen_would_have_to_wait() {
        let rules = Ruleset::new(vec![claude()]);
        let base = Instant::now();

        // The control: the SCREEN going quiet. The working title stops, and the resting verdict is
        // still not published, because it has not held for the window.
        let mut scraped = Tracker::default();
        let em = painted(CLAUDE_FOOTER);
        scraped.observe(em.screen(), Some("⠂ Reading files"), &rules, base);
        assert_eq!(scraped.verdict().state, AgentState::Working);
        scraped.observe(em.screen(), Some("✳ Reading files"), &rules, base);
        assert_eq!(
            scraped.verdict().state,
            AgentState::Working,
            "the screen's resting answer waits for the window",
        );

        // The report: the same resting state, published on arrival.
        let mut reported = Tracker::default();
        reported.observe(em.screen(), Some("⠂ Reading files"), &rules, base);
        assert_eq!(reported.verdict().state, AgentState::Working);
        let outcome = reported.report(Report {
            state: AgentState::Idle,
            agent: Some("claude".to_owned()),
            source: "hook".to_owned(),
            seq: Some(1),
            owner: None,
            asked: None,
            said: None,
            noticed: None,
            transcript: None,
            build: None,
        });
        assert_eq!(
            outcome,
            ReportOutcome {
                accepted: true,
                changed: true
            },
        );
        assert_eq!(reported.verdict().state, AgentState::Idle);
        assert_eq!(reported.reported_source(), Some("hook"));
        assert_eq!(
            reported.verdict().rule,
            None,
            "a report names no rule, because none fired",
        );
    }

    /// While a report stands an ORDINARY screen cannot overrule it; a release gives the pane back.
    ///
    /// # ⚠⚠⚠⚠⚠ This test used to paint a DIALOG here, and that was the defect (register item 524)
    ///
    /// Its screen was the loudest thing the scrape has — a choice list — and it asserted the report
    /// won anyway, in the words *"the screen argued blocked and the report still owns the pane"*.
    /// **That is the state a live run sat in for five hours and twenty minutes**: an agent at a
    /// permission dialog, published `working`, with the daemon holding the fact three ways.
    ///
    /// The general claim is still true and is what this keeps: a report outranks an INFERENCE. What
    /// it may not outrank is a newer dialog, which is
    /// [`a_dialog_outranks_a_report_older_than_it`](fn@a_dialog_outranks_a_report_older_than_it)'s
    /// subject — so this one now argues with a screen the rules read as `idle`, which is an
    /// inference and exactly the thing a report exists to beat.
    #[test]
    fn a_report_outranks_the_screen_until_it_is_released() {
        let rules = Ruleset::new(vec![claude()]);
        let mut tracker = Tracker::default();
        let mut em = painted(CLAUDE_FOOTER);
        let base = Instant::now();

        // Claimed by its footer first, then asking: a real agent's modal covers the fingerprint, so
        // the dialog is read through the identity the footer established.
        tracker.observe(em.screen(), Some("⠂ x"), &rules, base);
        repaint(&mut em, DIALOG);
        tracker.observe(
            em.screen(),
            Some("✳ x"),
            &rules,
            base + Duration::from_millis(100),
        );
        assert_eq!(
            tracker.verdict().state,
            AgentState::Blocked,
            "the fixture really is a screen the rules read as blocked",
        );

        tracker.report(Report {
            state: AgentState::Idle,
            agent: None,
            source: "hook".to_owned(),
            seq: Some(1),
            owner: None,
            asked: None,
            said: None,
            noticed: None,
            transcript: None,
            build: None,
        });
        assert_eq!(
            tracker.verdict().agent.as_deref(),
            Some("claude"),
            "a report that does not repeat the name leaves the pane the agent it already was",
        );
        // Repaint so the quiescence gate cannot be what holds the answer still: the screen HAS moved,
        // and the report still wins. ⚠ Back to the FOOTER — an at-rest screen the rules infer from —
        // because a dialog is the one screen a report may not outrank (item 524).
        repaint(&mut em, CLAUDE_FOOTER);
        tracker.observe(
            em.screen(),
            Some("⠂ x"),
            &rules,
            base + Duration::from_secs(10),
        );
        assert_eq!(
            tracker.verdict().state,
            AgentState::Idle,
            "the screen inferred something and the report still owns the pane",
        );

        // And the dialog is back for the release half: what the pane goes back to being is the
        // screen's answer, which here is the loud one.
        repaint(&mut em, DIALOG);
        assert!(tracker.release_report(), "a report was in force");
        assert!(
            !tracker.release_report(),
            "and only the first release drops it"
        );
        assert_eq!(tracker.reported_source(), None);
        tracker.observe(
            em.screen(),
            Some("✳ x"),
            &rules,
            base + Duration::from_secs(11),
        );
        assert_eq!(
            tracker.verdict().state,
            AgentState::Blocked,
            "released, the pane is the screen's again",
        );
    }

    /// ⚠⚠⚠⚠⚠ **AND A COMPOSER HOLDING AN UNSUBMITTED PROMPT DOES NOT** — the exact opposite of
    /// [`a_dialog_outranks_a_report_older_than_it`] below, pinned because a caller is now resting a
    /// contract on it.
    ///
    /// # What this measures, and why it is a gate rather than a comment
    ///
    /// [`Tracker::observe`] does not run the manifest's rules on a pane a hook is reporting: one
    /// screen fact is carved out (register item 524's dialog) and everything else the rules could
    /// see is not consulted. So [`AgentState::Holding`] — register item 669's fourth state, which
    /// is a rule reading the composer — **can never be published for a reported pane**, and
    /// `sprag_plugin`'s [`SubmittedWhen::Released`] therefore refuses on exactly the population a
    /// supervisor drives.
    ///
    /// ⚠⚠ **That is a LIMIT, not a defect, and the difference is that somebody chose it.** Lifting
    /// it means letting a second screen fact overrule a report, which is the change 524 already
    /// made once — and it would decide, unmeasured, what a pane says when its agent is genuinely
    /// working AND its composer holds a queued paste. The staging below asserts the same screen IS
    /// read as `Holding` with no report in force, so what this pins is the ARBITRATION and not a
    /// manifest that failed to match.
    #[test]
    fn a_reported_pane_holding_a_paste_is_not_read_as_holding() {
        let rules = Ruleset::new(vec![claude()]);
        let base = Instant::now();

        // ⚠ THE PREMISE, ASSERTED: with nobody reporting, this very screen reads `Holding`. Without
        // it a green gate below could mean the manifest simply does not match this fixture, which
        // is a different fact and would make the arbitration claim vacuous.
        let mut scraped = Tracker::default();
        let mut em = painted(HELD_COMPOSER);
        scraped.observe(em.screen(), Some("✳ Claude Code"), &rules, base);
        assert_eq!(
            scraped.verdict().state,
            AgentState::Holding,
            "the staging: unreported, this screen's composer is read for what it is holding",
        );

        // ── THE SAME SCREEN, UNDER A REPORT ──
        let mut reported = Tracker::default();
        reported.observe(em.screen(), Some("✳ Claude Code"), &rules, base);
        reported.report(Report {
            state: AgentState::Idle,
            agent: Some("claude".to_owned()),
            source: "hook:claude".to_owned(),
            seq: Some(4),
            owner: None,
            asked: None,
            said: None,
            noticed: None,
            transcript: None,
            build: None,
        });
        repaint(&mut em, HELD_COMPOSER);
        reported.observe(
            em.screen(),
            Some("✳ Claude Code"),
            &rules,
            base + Duration::from_millis(100),
        );
        assert_eq!(
            reported.verdict().state,
            AgentState::Idle,
            "⚠⚠⚠⚠ THE REPORT STANDS AND THE COMPOSER IS NOT LOOKED AT. This is the bound on \
             register item 669's second stage: a supervisor's own agents all report, so the \
             contract that reads this state refuses there rather than answering. Change this \
             deliberately or not at all",
        );
        assert_eq!(
            reported.verdict().rule,
            None,
            "⚠ and NO rule is named, because none ran — unlike the dialog case one door down, \
             which names what overruled the reporter",
        );
    }

    /// ⚠⚠⚠⚠⚠ **AN UNANSWERED DIALOG OUTRANKS A REPORT OLDER THAN IT** — register item 524, and the
    /// five hours and twenty minutes that bought it.
    ///
    /// # What was measured, and why the old rule could not see it
    ///
    /// A live `ai_loop` run stood at *"Do you want to make this edit to spec.rs?"* from 23:12 to
    /// 04:33 while every surface said `working` and the run's cost never moved. The daemon HELD the
    /// fact three ways — the hook maps `Notification` to `Blocked`, the attention ledger counted the
    /// ask, and the screen itself was a menu — and *a report outranks the screen* beat all three,
    /// **because that rule had no clock**. It compared authorities and never asked which was newer.
    ///
    /// # The three claims here, and each fails on its own
    ///
    /// * a dialog painted AFTER a `working` report publishes `blocked` — the run's five hours;
    /// * the verdict NAMES what overruled the reporter ([`DIALOG_OUTRANKS_REPORT`]) while
    ///   [`Tracker::reported_source`] still names the reporter, so a reader gets both halves;
    /// * and when the dialog is answered the pane goes back to the REPORT's own claim rather than
    ///   sticking on `blocked` — which is the whole reason `Reported` had to start keeping `state`.
    #[test]
    fn a_dialog_outranks_a_report_older_than_it() {
        let rules = Ruleset::new(vec![claude()]);
        let mut tracker = Tracker::default();
        let mut em = painted(CLAUDE_FOOTER);
        let base = Instant::now();

        tracker.observe(em.screen(), Some("⠂ x"), &rules, base);
        tracker.report(Report {
            state: AgentState::Working,
            agent: Some("claude".to_owned()),
            source: "hook:claude".to_owned(),
            seq: Some(4),
            owner: None,
            asked: None,
            said: None,
            noticed: None,
            transcript: None,
            build: None,
        });
        assert_eq!(
            tracker.verdict().state,
            AgentState::Working,
            "the control: the report is in force and says the agent is working",
        );

        // ── THE DIALOG ARRIVES AFTER THE REPORT ──
        repaint(&mut em, DIALOG);
        tracker.observe(
            em.screen(),
            Some("✳ x"),
            &rules,
            base + Duration::from_millis(100),
        );
        assert_eq!(
            tracker.verdict().state,
            AgentState::Blocked,
            "⚠⚠⚠⚠⚠ A QUESTION IS ON THE SCREEN AND NOBODY HAS ANSWERED IT. `working` here is the \
             five-hour reading: the run goes on looking, the cost never moves, and no surface says \
             a person is needed. A report is an authority about the PAST — this screen is now",
        );
        assert_eq!(
            tracker.verdict().rule.as_deref(),
            Some(DIALOG_OUTRANKS_REPORT),
            "⚠⚠⚠ and the verdict must say WHAT beat the reporter, or a reader sees `blocked` from a \
             pane that is also `source=hook:claude` and cannot tell which authority spoke",
        );
        assert_eq!(
            tracker.reported_source(),
            Some("hook:claude"),
            "⚠⚠ the report is OVERRULED, not released: the reporter still owns everything else it \
             states, and a release is a decision somebody takes on purpose",
        );

        // ── AND THE ANSWER GIVES THE PANE BACK TO THE REPORTER ──
        repaint(&mut em, CLAUDE_FOOTER);
        tracker.observe(
            em.screen(),
            Some("⠂ x"),
            &rules,
            base + Duration::from_secs(30),
        );
        assert_eq!(
            tracker.verdict().state,
            AgentState::Working,
            "⚠⚠⚠⚠ THE DIALOG IS GONE, SO THE REPORT IS THE BEST FACT AGAIN. Sticking on `blocked` \
             would trade a five-hour silence for a permanent false alarm — and the screen alone \
             would say `idle` here, which is the inference the report exists to beat",
        );
        assert_eq!(
            tracker.verdict().rule,
            None,
            "and nothing overruled anybody this time, so no rule is named",
        );
    }

    /// A release is served by the next LOOK, and it is the release that clears the quiescence memory —
    /// without which the screen it must be re-read from would be skipped as unchanged.
    #[test]
    fn a_release_owes_a_look_and_the_look_serves_it() {
        let rules = Ruleset::new(vec![claude()]);
        let mut tracker = Tracker::default();
        let mut em = painted(CLAUDE_FOOTER);
        let base = Instant::now();

        tracker.observe(em.screen(), Some("⠂ x"), &rules, base);
        repaint(&mut em, DIALOG);
        tracker.observe(
            em.screen(),
            Some("✳ x"),
            &rules,
            base + Duration::from_millis(100),
        );
        assert_eq!(tracker.verdict().state, AgentState::Blocked);
        tracker.report(Report {
            state: AgentState::Idle,
            agent: None,
            source: "hook".to_owned(),
            seq: None,
            owner: None,
            asked: None,
            said: None,
            noticed: None,
            transcript: None,
            build: None,
        });
        assert!(!tracker.owes_look(), "a report is its own answer");

        tracker.release_report();
        assert!(
            tracker.owes_look(),
            "released, and nothing on the screen will say so"
        );
        // The screen has NOT moved since the report — the exact case the cleared memory exists for.
        tracker.observe(
            em.screen(),
            Some("✳ x"),
            &rules,
            base + Duration::from_millis(200),
        );
        assert!(!tracker.owes_look(), "the look served it");
        assert_eq!(tracker.verdict().state, AgentState::Blocked);
    }

    /// A replay from the source already speaking is refused; a NEW speaker is heard whatever its clock
    /// says; and a reporter with no clock is never stale.
    #[test]
    fn a_replay_is_refused_but_a_new_speaker_is_heard() {
        let mut tracker = Tracker::default();

        assert!(
            tracker
                .report(Report {
                    state: AgentState::Working,
                    agent: None,
                    source: "hook".to_owned(),
                    seq: Some(5),
                    owner: None,
                    asked: None,
                    said: None,
                    noticed: None,
                    transcript: None,
                    build: None,
                })
                .accepted,
        );
        assert_eq!(
            tracker.report(Report {
                state: AgentState::Idle,
                agent: None,
                source: "hook".to_owned(),
                seq: Some(5),
                owner: None,
                asked: None,
                said: None,
                noticed: None,
                transcript: None,
                build: None,
            }),
            ReportOutcome {
                accepted: false,
                changed: false
            },
            "the same sequence number twice is a replay",
        );
        assert_eq!(
            tracker.verdict().state,
            AgentState::Working,
            "and a refused report changes nothing",
        );
        assert_eq!(
            tracker.report(Report {
                state: AgentState::Idle,
                agent: None,
                source: "hook".to_owned(),
                seq: Some(4),
                owner: None,
                asked: None,
                said: None,
                noticed: None,
                transcript: None,
                build: None,
            }),
            ReportOutcome {
                accepted: false,
                changed: false
            },
            "nor does one that goes backwards",
        );
        assert!(
            tracker
                .report(Report {
                    state: AgentState::Idle,
                    agent: None,
                    source: "hook".to_owned(),
                    seq: Some(6),
                    owner: None,
                    asked: None,
                    said: None,
                    noticed: None,
                    transcript: None,
                    build: None,
                })
                .accepted,
            "forwards is heard",
        );

        // A different integration, whose clock starts wherever it starts. Judging it by the first
        // source's numbering would silence it entirely.
        assert!(
            tracker
                .report(Report {
                    state: AgentState::Working,
                    agent: None,
                    source: "other".to_owned(),
                    seq: Some(1),
                    owner: None,
                    asked: None,
                    said: None,
                    noticed: None,
                    transcript: None,
                    build: None,
                })
                .accepted,
            "a new speaker is not judged by the old one's clock",
        );
        assert_eq!(tracker.reported_source(), Some("other"));
        // And a reporter with no clock at all (a person at a command line).
        assert!(
            tracker
                .report(Report {
                    state: AgentState::Blocked,
                    agent: None,
                    source: "cli".to_owned(),
                    seq: None,
                    owner: None,
                    asked: None,
                    said: None,
                    noticed: None,
                    transcript: None,
                    build: None,
                })
                .accepted,
        );
        assert!(
            tracker
                .report(Report {
                    state: AgentState::Blocked,
                    agent: None,
                    source: "cli".to_owned(),
                    seq: None,
                    owner: None,
                    asked: None,
                    said: None,
                    noticed: None,
                    transcript: None,
                    build: None,
                })
                .accepted,
            "with nothing to be stale against, nothing is refused",
        );
    }

    /// A duplicate report is ACCEPTED and changes nothing — and the two answers are independent, which
    /// is why `ReportOutcome` carries both.
    #[test]
    fn a_duplicate_report_is_accepted_and_publishes_nothing() {
        let mut tracker = Tracker::default();
        tracker.report(Report {
            state: AgentState::Working,
            agent: None,
            source: "hook".to_owned(),
            seq: Some(1),
            owner: None,
            asked: None,
            said: None,
            noticed: None,
            transcript: None,
            build: None,
        });
        let published = tracker.seq();

        let outcome = tracker.report(Report {
            state: AgentState::Working,
            agent: None,
            source: "hook".to_owned(),
            seq: Some(2),
            owner: None,
            asked: None,
            said: None,
            noticed: None,
            transcript: None,
            build: None,
        });
        assert_eq!(
            outcome,
            ReportOutcome {
                accepted: true,
                changed: false
            },
        );
        assert_eq!(
            tracker.seq(),
            published,
            "the published generation does not move for an answer that did not",
        );
        // The reporter's clock DID advance, so its next message is judged against the newer number.
        assert!(
            !tracker
                .report(Report {
                    state: AgentState::Idle,
                    agent: None,
                    source: "hook".to_owned(),
                    seq: Some(2),
                    owner: None,
                    asked: None,
                    said: None,
                    noticed: None,
                    transcript: None,
                    build: None,
                })
                .accepted,
            "a duplicate still advances the source's sequence",
        );
    }

    /// The owner belongs to ONE report, so a later report brings its own — including none.
    ///
    /// This is what keeps the lifetime from outliving the claim it was attached to. A pane runs one
    /// agent, and a new speaker owns the pane (see the sequence tests above); inheriting the previous
    /// speaker's owner would tie a fresh report to a process that has nothing to do with it, and
    /// carrying it past a release would let a released pane be re-released.
    ///
    /// A release clears it, which is asserted here rather than assumed: `reported_owner` answering
    /// after a release would be a lifetime with no report on the other end of it.
    #[test]
    fn an_owner_belongs_to_one_report_and_the_next_one_brings_its_own() {
        let mut tracker = Tracker::default();

        tracker.report(Report {
            state: AgentState::Working,
            agent: None,
            source: "hook".to_owned(),
            seq: None,
            owner: Some(4242),
            asked: None,
            said: None,
            noticed: None,
            transcript: None,
            build: None,
        });
        assert_eq!(tracker.reported_owner(), Some(4242));

        // The same speaker, saying something new: its own owner, not the one before.
        tracker.report(Report {
            state: AgentState::Blocked,
            agent: None,
            source: "hook".to_owned(),
            seq: None,
            owner: Some(99),
            asked: None,
            said: None,
            noticed: None,
            transcript: None,
            build: None,
        });
        assert_eq!(tracker.reported_owner(), Some(99));

        // A speaker that binds nothing REPLACES the binding rather than inheriting it — otherwise a
        // person's report would be retired by whatever the previous hook was speaking for.
        tracker.report(Report {
            state: AgentState::Idle,
            agent: None,
            source: "cli".to_owned(),
            seq: None,
            owner: None,
            asked: None,
            said: None,
            noticed: None,
            transcript: None,
            build: None,
        });
        assert_eq!(tracker.reported_owner(), None);
        assert_eq!(
            tracker.reported_source(),
            Some("cli"),
            "and there IS a report — the two `None`s are not the same answer",
        );

        tracker.report(Report {
            state: AgentState::Working,
            agent: None,
            source: "hook".to_owned(),
            seq: None,
            owner: Some(7),
            asked: None,
            said: None,
            noticed: None,
            transcript: None,
            build: None,
        });
        assert!(tracker.release_report(), "a report was in force");
        assert_eq!(
            tracker.reported_owner(),
            None,
            "a released pane has no lifetime left to run out",
        );
    }

    #[test]
    fn a_pane_that_keeps_repainting_settles_when_the_candidate_has_held_long_enough() {
        let rules = Ruleset::new(vec![claude()]);
        let mut tracker = Tracker::default();
        let mut em = painted(CLAUDE_FOOTER);
        let base = Instant::now();

        tracker.observe(em.screen(), Some("⠂ Reading files"), &rules, base);
        assert_eq!(tracker.verdict().state, AgentState::Working);

        let rested = Some("✳ Reading files");
        let began = base + Duration::from_millis(100);
        tracker.observe(em.screen(), rested, &rules, began);

        // The spinner has stopped but the transcript has not: every tick moves a row.
        let mut printed = CLAUDE_FOOTER.to_vec();
        for tick in 1..=3 {
            printed.insert(0, "● and then it said something else");
            repaint(&mut em, &printed);
            tracker.observe(
                em.screen(),
                rested,
                &rules,
                began + Duration::from_millis(500) * tick,
            );
            assert_eq!(
                tracker.verdict().state,
                AgentState::Working,
                "tick {tick}: the candidate has not held long enough yet",
            );
        }

        printed.insert(0, "● one more");
        repaint(&mut em, &printed);
        tracker.observe(em.screen(), rested, &rules, began + DEFAULT_SETTLE);
        assert_eq!(
            tracker.verdict().state,
            AgentState::Idle,
            "it has been the answer for the whole window, busy pane or not",
        );
    }

    /// A pause shorter than the window is absorbed entirely — the flicker M2 made hysteresis a
    /// correctness requirement for.
    #[test]
    fn a_pause_in_the_animation_does_not_publish_a_return_to_rest() {
        let rules = Ruleset::new(vec![claude()]);
        let mut tracker = Tracker::default();
        let em = painted(CLAUDE_FOOTER);
        let base = Instant::now();

        tracker.observe(em.screen(), Some("⠂ Working"), &rules, base);
        tracker.observe(
            em.screen(),
            Some("✳ Working"),
            &rules,
            base + MEASURED_SPINNER_PERIOD,
        );
        // A tick DURING the pause, or this test would pass with a window of one nanosecond: it is
        // the window that has to be asked, not merely the pane.
        tracker.observe(
            em.screen(),
            Some("✳ Working"),
            &rules,
            base + MEASURED_SPINNER_PERIOD + MEASURED_SPINNER_PERIOD / 2,
        );
        assert_eq!(tracker.verdict().state, AgentState::Working);
        tracker.observe(
            em.screen(),
            Some("⠐ Working"),
            &rules,
            base + MEASURED_SPINNER_PERIOD * 2,
        );

        assert_eq!(tracker.verdict().state, AgentState::Working);
        assert_eq!(tracker.seq(), 1, "the pause never reached the wire");
    }

    /// The asymmetry, with both sides driven at the SAME instants so the difference is the policy
    /// and not the timings: evidence that is PRESENT publishes on sight, evidence that is an
    /// absence has to hold.
    #[test]
    fn a_dialog_publishes_at_once_where_a_return_to_rest_waits() {
        let rules = Ruleset::new(vec![claude()]);
        let base = Instant::now();
        let mut asking = Tracker::default();
        let mut resting = Tracker::default();
        let mut em_asking = painted(CLAUDE_FOOTER);
        let em_resting = painted(CLAUDE_FOOTER);

        asking.observe(em_asking.screen(), Some("⠂ x"), &rules, base);
        resting.observe(em_resting.screen(), Some("⠂ x"), &rules, base);

        let moment = base + Duration::from_millis(100);
        repaint(&mut em_asking, DIALOG);
        asking.observe(em_asking.screen(), Some("✳ x"), &rules, moment);
        resting.observe(em_resting.screen(), Some("✳ x"), &rules, moment);

        assert_eq!(asking.verdict().state, AgentState::Blocked);
        assert_eq!(
            asking.seq(),
            2,
            "the state a person is waiting for is not delayed"
        );
        assert_eq!(
            resting.verdict().state,
            AgentState::Working,
            "the same instant, and this one still has to hold",
        );
        assert_eq!(resting.seq(), 1);
    }

    /// R251's finding, which is what grew this slice: a modal covers the two things `codex`'s
    /// fingerprint is made of, so the pane the front exists to report is the pane no fingerprint
    /// claims. The memory supplies the half the screen has hidden.
    #[test]
    fn a_modal_that_covers_the_fingerprint_keeps_the_agent_the_pane_already_was() {
        let rules = Ruleset::new(vec![codex()]);
        let mut tracker = Tracker::default();
        let mut em = painted(CODEX_AT_REST);
        let base = Instant::now();

        tracker.observe(em.screen(), Some("codexprobe"), &rules, base);
        let verdict = tracker.observe(
            em.screen(),
            Some("codexprobe"),
            &rules,
            base + DEFAULT_SETTLE,
        );
        assert_eq!(verdict.agent.as_deref(), Some("codex"));
        assert_eq!(verdict.state, AgentState::Idle);

        repaint(&mut em, CODEX_MODAL);
        // The premise, asserted rather than assumed: from one frame this pane is nobody's.
        assert_eq!(
            detect(em.screen(), Some("codexprobe"), rules.manifests()),
            Verdict::default(),
            "if this screen were still claimed the test would prove nothing about memory",
        );

        let verdict = tracker.observe(
            em.screen(),
            Some("codexprobe"),
            &rules,
            base + DEFAULT_SETTLE + Duration::from_millis(100),
        );
        assert_eq!(verdict.state, AgentState::Blocked);
        assert_eq!(
            verdict.agent.as_deref(),
            Some("codex"),
            "the memory answered what the screen would not",
        );
    }

    /// The OTHER measured miss this slice inherited, and the settle window is what absorbs it:
    /// `codex` replaces its footer with a transient hint for a few seconds, so the fingerprint's
    /// conjunction fails and the pane briefly belongs to nobody. Because a resting verdict has to
    /// hold, that flicker never reaches the wire at all — which is why the window applies to a
    /// FIRST publication too, and not only to a state that is being left.
    #[test]
    fn a_fingerprint_covered_for_less_than_the_window_never_reaches_the_wire() {
        let rules = Ruleset::new(vec![codex()]);
        let mut tracker = Tracker::default();
        let mut em = painted(CODEX_AT_REST);
        let base = Instant::now();

        tracker.observe(em.screen(), Some("codexprobe"), &rules, base);
        let settled = base + DEFAULT_SETTLE;
        tracker.observe(em.screen(), Some("codexprobe"), &rules, settled);
        assert_eq!(tracker.verdict().agent.as_deref(), Some("codex"));
        assert_eq!(tracker.seq(), 1);

        let hint = &[
            "› Write tests for @filename",
            "  press esc again to interrupt",
        ];
        repaint(&mut em, hint);
        assert_eq!(
            detect(em.screen(), Some("codexprobe"), rules.manifests()),
            Verdict::default(),
            "the premise: with the footer replaced, one frame claims nobody",
        );
        tracker.observe(
            em.screen(),
            Some("codexprobe"),
            &rules,
            settled + Duration::from_millis(100),
        );

        repaint(&mut em, CODEX_AT_REST);
        tracker.observe(
            em.screen(),
            Some("codexprobe"),
            &rules,
            settled + Duration::from_millis(900),
        );
        assert_eq!(tracker.verdict().agent.as_deref(), Some("codex"));
        assert_eq!(
            tracker.seq(),
            1,
            "the pane never stopped being codex as far as anybody downstream can tell",
        );
    }

    /// The bound on that memory, and the other half of the same measurement: an unclaimed pane
    /// showing nothing active is not asserted to still be anybody. Otherwise the shell that
    /// outlives an agent goes on being reported as the agent.
    #[test]
    fn a_remembered_agent_that_shows_nothing_active_is_let_go() {
        let rules = Ruleset::new(vec![codex()]);
        let mut tracker = Tracker::default();
        let mut em = painted(CODEX_AT_REST);
        let base = Instant::now();

        tracker.observe(em.screen(), Some("codexprobe"), &rules, base);
        let settled = base + DEFAULT_SETTLE;
        tracker.observe(em.screen(), Some("codexprobe"), &rules, settled);
        assert_eq!(tracker.verdict().agent.as_deref(), Some("codex"));

        // The agent exits and the pane is a shell again.
        repaint(&mut em, &["coin@box:~$ "]);
        tracker.observe(em.screen(), Some("coin@box: ~"), &rules, settled);
        assert_eq!(
            tracker.verdict().agent.as_deref(),
            Some("codex"),
            "an absence, so it has to hold like any other",
        );
        tracker.observe(
            em.screen(),
            Some("coin@box: ~"),
            &rules,
            settled + DEFAULT_SETTLE,
        );
        assert_eq!(tracker.verdict(), &Verdict::default());

        // And the identity is really gone: the very screen that rode on memory in the test above
        // is nobody's dialog now.
        repaint(&mut em, CODEX_MODAL);
        let verdict = tracker.observe(
            em.screen(),
            Some("coin@box: ~"),
            &rules,
            settled + DEFAULT_SETTLE * 2,
        );
        assert_eq!(
            verdict,
            &Verdict::default(),
            "a dialog in a shell is not the agent that used to live here",
        );
    }

    /// A pane nobody ever claimed publishes nothing at all, so a workspace of ordinary shells keeps
    /// the pre-H3 wire shape — the additive discipline, checked at the only place that can decide
    /// it before slice 3 exists.
    #[test]
    fn a_pane_that_was_never_an_agent_never_publishes() {
        let rules = Ruleset::new(vec![claude(), codex()]);
        let mut tracker = Tracker::default();
        let mut em = painted(&["coin@box:~$ cargo build"]);
        let base = Instant::now();

        tracker.observe(em.screen(), Some("coin@box: ~"), &rules, base);
        repaint(
            &mut em,
            &["coin@box:~$ cargo build", "   Compiling sprag-vt"],
        );
        tracker.observe(
            em.screen(),
            Some("coin@box: ~"),
            &rules,
            base + DEFAULT_SETTLE * 2,
        );

        assert_eq!(tracker.verdict(), &Verdict::default());
        assert_eq!(tracker.seq(), 0, "nothing was ever published");
    }
}
