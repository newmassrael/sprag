//! The change LOG behind a wake — what moved, not merely that something did.
//!
//! A client does not poll: it parks on `scene/waitFor {since}` and the daemon answers when the
//! scene has moved past `since` ([`crate::notify`] owns *whose* scene). What that answer carries
//! today is a number. So a woken client re-reads the window list, then the pane set, then every
//! pane in it, to rediscover a change the daemon already knew — and a client that is not a display
//! client cannot use the wake at all, because a bare revision names nothing it could act on.
//!
//! This module is the missing half: the daemon records what changed, keyed by the revision it
//! changed at, and a reader asks for everything after the revision it last saw.
//!
//! ## The cursor is the REVISION — this log mints no counter of its own
//!
//! pinion's waiter registry carries a scar directly on this point: an earlier version minted a
//! private counter, which *"forked the scene-version namespace and left the OCC token stale on
//! external arrival — one scene must have one version."*
//!
//! So a [`Record`] carries the revision it was appended at, and that revision IS the cursor. The
//! consequence is worth stating because it removes work rather than adding it: a client already
//! parks on that token, so the blocking read it wants is two calls that already fit together —
//! `scene/waitFor {since: R}` parks and answers `R'`, then a read at cursor `R` returns what
//! happened in `(R, R']`. Nothing here needs a second parking path, and an earlier draft of this
//! design proposed one before noticing it had been built (R1270) for another reason.
//!
//! Several records may share one revision: one mutation can have two structural consequences. They
//! are appended as a GROUP ([`EventLog::record`] takes the whole group) so that a reader can never
//! observe half of one — the torn read R265 met from the other side, made unrepresentable here by
//! giving the caller no way to append an incomplete group under one lock hold.
//!
//! ## An event names WHAT TO RE-READ; it does not carry the new value
//!
//! [`Event::PaneCreated`] carries a pane id and nothing else. It could have carried the pane's new
//! [`PaneInfo`](sprag_terminal::PaneInfo), and that is what a client ultimately wants — but it is
//! also fifteen fields whose meaning is defined on that struct, and copying them here would be a
//! SECOND definition of a pane's public shape, free to drift from the one the `panes` slot already
//! serves. This crate's standing rule is that one shape is spoken by the wire slot, the client's
//! mirror and the in-process arm alike, precisely so none can drift.
//!
//! Naming the subject instead keeps that rule and still delivers the whole point of the log. The
//! cost a wake pays today is N re-reads to find the one thing that moved; an id turns that into
//! one, against a slot that already exists and is already tested. What it does not do is let a
//! client skip the read — and it should not, because a value copied into an event is stale the
//! instant a later mutation lands, while an id stays true.
//!
//! ## What the vocabulary is, and why it is not longer
//!
//! Every variant below is DERIVED from a difference the daemon can actually observe between two
//! reads of state it already publishes — [`PaneInfo`](sprag_terminal::PaneInfo) by id,
//! [`WindowInfo`](sprag_terminal::WindowInfo) and [`SessionInfo`](sprag_terminal::SessionInfo) by
//! name. That derivation PRUNED the list rather than filling it:
//!
//! * There is no `WindowRenamed`. A window's public shape is its name and whether it is current, so
//!   a rename is indistinguishable from a close plus a create by anything reading that shape. A
//!   variant nothing can produce is a promise to a client that the daemon cannot keep.
//! * There is no `PaneOutput`. Output already advances the revision a client is parked on, and a
//!   record per batch of PTY output would evict this ring at output rate — destroying, for the
//!   panes a reader actually cares about, the delivery guarantee the ring exists to give. That is a
//!   deliberate divergence from the rivals that have such an event, priced rather than omitted.
//! * [`Event::AgentStateChanged`] is NOT one of these, and is the exception that shows the rule. It
//!   is not a difference in the state above — it is the settle waker's own verdict transition — so
//!   it is EMITTED by that observer ([`SessionJournal::emit`]) rather than derived, and it was
//!   declared only once something could produce it.
//!
//! ## What the DISPATCH funnel structurally cannot see, and why that pruned the list again
//!
//! [`SessionShape`] is read after a mutating dispatch, which is the one place every mutating wire
//! method passes through — so a method added later produces its events without knowing this module
//! exists, and no handler has to remember to emit.
//!
//! What that site cannot see is anything OUTPUT drives. A pane's title (`OSC 0`/`2`), its
//! notification, its liveness and its exit status all move when the CHILD writes, which reaches the
//! daemon through a pane's `on_dirty` hook and never through a dispatch at all. A `PaneUpdated`
//! variant was declared here before that was read from the code, and it is now gone rather than
//! left standing with nothing able to produce it — the same rule that struck `WindowRenamed`,
//! applied to a variant this module had already shipped. It returns if and when an observer that
//! can see it exists, the way [`Event::AgentStateChanged`] arrived with its own.
//!
//! That sentence has since been taken up twice rather than once: [`Event::PaneJobChanged`] is the
//! second variant to arrive with the settle waker as its observer, and it is the one that reaches
//! the liveness this paragraph lists as unreachable — a pane whose child exits loses its foreground
//! job, and the waker samples that where no dispatch could.
//!
//! The corollary is the reason the funnel is affordable at all: `key`, `text`, `paste` and `mouse`
//! are invokes, so **every keystroke is a mutating dispatch**. A shape that walked every pane here
//! would run at typing rate over an N-lock walk — the cost R265 removed from a different reader.
//! So the shape is deliberately CHEAP ([`SessionShape::read`]): ids and names under one registry
//! lock plus one workspace lock per window, never a `PaneInfo` build. What it is not is gated — the
//! first version gated the pane walk on each window's `layout_revision`, and that was wrong for a
//! reason worth keeping: that number means a client's PROJECTION is stale, not that the pane set
//! moved, and a spawn reconciles the tree lazily so a pane arrives with it unmoved. The test wrote
//! for the gate is what found it.
//!
//! ## What the derive site costs — MEASURED, where the design ARGUED
//!
//! The H6 design priced this paragraph by argument ("mutating dispatches are user actions"), and
//! that argument was already false when it was written: input is an invoke. So it is measured, on
//! `sprag-latency`'s rows (i9-14900HX, `--release`, three runs, minima), with **no change to find**
//! — the steady state, and the only one that recurs:
//!
//! * **1 pane: 0.121-0.142 us.**
//! * **64 panes: 0.651-0.754 us.**
//!
//! About 9-10 ns per pane, which is the sorted id vector plus a workspace lock per window. Against
//! a keystroke path measured in milliseconds (R246: ~5 ms per keystroke in release), the top row is
//! **~0.015% of one keystroke** — and 64 panes is already the wide end of `REGISTRY_SIZES`.
//!
//! The OTHER axis, because the pane rows hold the window count at one and this walk takes a
//! WORKSPACE LOCK PER WINDOW — the term that scales with locks rather than with `u64`s. One pane
//! per window, same conditions:
//!
//! * **1 window: 0.168-0.288 us.**
//! * **16 windows: 1.217-1.313 us.**
//!
//! About **70 ns per window against 10 ns per pane** — a window costs roughly seven panes, which is
//! the lock and is the shape to expect. At sixteen windows it is still **~0.025% of one keystroke**.
//! The two axes agree where they meet: the `1 pane` and `1 window` rows describe the SAME
//! configuration reached by two different constructions, and they land within each other's spread.

use std::collections::VecDeque;

use serde_json::Value;
use sprag_terminal::SessionRegistry;

/// One window's structural fingerprint, as [`SessionShape::read`] takes it under the registry lock.
#[derive(Clone, Debug, PartialEq, Eq)]
struct WindowShape {
    /// The window's display name, which is also its address.
    name: String,
    /// Whether it is the session's current window.
    current: bool,
    /// The window's own arrangement counter — what makes a client's PROJECTION stale.
    ///
    /// It is the source of [`Event::LayoutUpdated`] and NOTHING ELSE. An earlier version of this
    /// struct used it to gate the pane walk below, on the premise that a pane set cannot change
    /// without the arrangement moving. **That premise is false, and a test caught it rather than a
    /// reviewer**: a spawn appends to the window's pool and the tree is reconciled LAZILY, on the
    /// next read, so a pane arrives with this number unmoved. Its own docs say as much — a revision
    /// says a projection is stale, not that the pane set moved — so the gate was reading a real
    /// number for a question it does not answer.
    layout: u64,
    /// The pane this window is ON, or `None` while it holds none — the source of
    /// [`Event::PaneSelected`] and nothing else.
    ///
    /// Read from the window itself rather than from the pane walk below, and the two are different
    /// facts: the pane SET says what exists, this says where the user is. It costs one `Option<u64>`
    /// copy per window on a walk that is already taking that window's lock.
    active: Option<u64>,
    /// The window's pane ids, sorted. Read every time, under the window's workspace lock.
    ///
    /// No carry-forward: see [`layout`](Self::layout) for the one that was tried and was wrong. The
    /// cost this pays instead is bounded and is the reason the walk is affordable at typing rate —
    /// it is O(panes) of `u64` under one uncontended lock per window, against a keystroke path
    /// already measured in milliseconds (R246).
    panes: Vec<u64>,
}

/// The structural state of one session, as cheaply as the registry can state it.
///
/// Compared against its predecessor to DERIVE what changed. Deriving rather than emitting is what
/// keeps the vocabulary honest under change: a mutating method added later moves this shape, so it
/// produces its events without a line of its own, and a method that moves nothing structural
/// produces none without having to say so.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionShape {
    /// This session's windows, in registry order.
    windows: Vec<WindowShape>,
    /// Every session's name, registry-wide and in registry order.
    ///
    /// Registry-wide because the `sessions` slot a woken client re-reads is registry-wide. It rides
    /// in the SCOPED session's shape because a bump reaches exactly the clients scoped to that
    /// session, so this log's reach is the wake's reach — no wider, and deliberately not narrower.
    sessions: Vec<String>,
}

impl SessionShape {
    /// Read `session`'s fingerprint: one registry lock, plus one workspace lock per window.
    #[must_use]
    pub fn read(registry: &SessionRegistry, session: &str) -> Self {
        let sessions = registry
            .sessions()
            .iter()
            .map(|entry| entry.name().to_owned())
            .collect();
        let windows = registry.session(session).map_or_else(Vec::new, |entry| {
            let current = entry.current_window().name().to_owned();
            entry
                .windows()
                .iter()
                .map(|window| {
                    let name = window.name().to_owned();
                    let layout = window.layout_revision();
                    let panes = {
                        let guard = window
                            .workspace()
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        let mut ids: Vec<u64> =
                            guard.panes().iter().map(|pane| pane.id().0).collect();
                        ids.sort_unstable();
                        ids
                    };
                    WindowShape {
                        current: name == current,
                        name,
                        layout,
                        active: window.active_pane().map(|pane| pane.0),
                        panes,
                    }
                })
                .collect()
        });
        Self { windows, sessions }
    }

    /// What moved between `self` (the older shape) and `next`.
    ///
    /// Order is deliberate and is the order a reader can apply without a contradiction in hand:
    /// sessions, then windows, then the panes and arrangement inside them.
    #[must_use]
    pub fn diff(&self, next: &Self) -> Vec<Event> {
        let mut events = Vec::new();
        for name in &next.sessions {
            if !self.sessions.contains(name) {
                events.push(Event::SessionCreated(name.clone()));
            }
        }
        for name in &self.sessions {
            if !next.sessions.contains(name) {
                events.push(Event::SessionClosed(name.clone()));
            }
        }
        for window in &next.windows {
            let had = self.windows.iter().find(|had| had.name == window.name);
            let Some(had) = had else {
                events.push(Event::WindowCreated(window.name.clone()));
                // A new window's panes are not PaneCreated events: the window itself is the change,
                // and a reader re-reading it gets the panes with it. Reporting both would have the
                // reader apply the same fact twice, in an order this cannot promise.
                continue;
            };
            if window.current && !had.current {
                events.push(Event::WindowSelected(window.name.clone()));
            }
            for id in &window.panes {
                if !had.panes.contains(id) {
                    events.push(Event::PaneCreated(*id));
                }
            }
            for id in &had.panes {
                if !window.panes.contains(id) {
                    events.push(Event::PaneClosed(*id));
                }
            }
            // AFTER the pane events, so a reader applying this batch in order learns a pane exists
            // before it is told the user moved onto it — the same rule that orders sessions before
            // windows before panes.
            //
            // `had.active` must ALSO be set, which is the same rule a new window's panes follow
            // above: a window going from no active pane to its first one is the window ESTABLISHING
            // itself, not the user moving, and a reader that has just been told the pane exists
            // re-reads the list and finds it marked. Reporting it would additionally make the event
            // arrive whenever the daemon happened to reconcile that window — a moment no request
            // asked for, which is how this was found: a `split` in one window recorded a select in
            // another.
            if let (Some(active), Some(_)) = (window.active, had.active)
                && window.active != had.active
            {
                events.push(Event::PaneSelected(active));
            }
            if window.layout != had.layout {
                events.push(Event::LayoutUpdated);
            }
        }
        for had in &self.windows {
            if !next.windows.iter().any(|window| window.name == had.name) {
                events.push(Event::WindowClosed(had.name.clone()));
            }
        }
        events
    }
}

/// One thing that changed, named by its SUBJECT rather than by its new value.
///
/// See the module docs for why an id and not a payload. A reader turns one of these into a targeted
/// re-read of the slot that already serves that subject.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// A pane was born — `id` is [`PaneInfo::id`](sprag_terminal::PaneInfo::id).
    PaneCreated(u64),
    /// A pane is gone from the session's pane set. NOT "its child exited": a dead pane keeps its
    /// place and its final screen, so a child's death is not this event — see the module docs on
    /// what the funnel structurally cannot see.
    ///
    /// A child's death IS reported, by [`Event::PaneJobChanged`]: the pane's foreground job becomes
    /// nothing. That is a sample and not a dispatch, which is why it took an observer with a clock
    /// and why this variant still must not be widened to mean it.
    PaneClosed(u64),
    /// A window (tmux's window, a tab) appeared under this name.
    WindowCreated(String),
    /// A window is gone from the session's window list.
    WindowClosed(String),
    /// The session's CURRENT window moved to this name.
    WindowSelected(String),
    /// A session appeared under this name.
    SessionCreated(String),
    /// A session is gone from the registry.
    SessionClosed(String),
    /// The pane ARRANGEMENT moved — a split, a divider drag, a float, a break or join — without the
    /// pane SET necessarily changing. Carries no subject because the arrangement is one object; a
    /// reader answers it by re-reading the layout slot, which is what it would do for any subject
    /// this could name.
    LayoutUpdated,
    /// The window's ACTIVE pane moved to this one — tmux `select-pane`, and the pane-level twin of
    /// [`Event::WindowSelected`].
    ///
    /// Reported only when a pane BECOMES active. A window whose last pane closes has no active pane
    /// and produces no event here: the fact a reader needs is that the pane is gone, which
    /// [`Event::PaneClosed`] already carries, and "there is now nothing" is not a subject a reader
    /// could re-read.
    PaneSelected(u64),
    /// A pane's published AGENT verdict moved — the event this whole niche is about.
    ///
    /// Not derived at the dispatch funnel, and could not be: a verdict rests on what is on the pane's
    /// SCREEN, which reaches the daemon through output, and a verdict resting on an ABSENCE ("the
    /// agent stopped working, so it is idle and wants you") is confirmed by a clock nothing else
    /// runs. So this one is EMITTED by the settle waker, the only observer that can see it.
    ///
    /// It is emitted on the same condition the waker already uses to decide whether to wake a
    /// session's clients at all — the pane's `seq` moving — so a record here and a wake there cannot
    /// disagree about what a published change is. Like every other variant it names its subject and
    /// not its value: a reader re-reads the pane list, where the state, the agent and the rule are
    /// defined once.
    AgentStateChanged(u64),
    /// The FOREGROUND JOB that owns this pane's terminal changed — the user started something, or
    /// the thing they started ended.
    ///
    /// Like [`Event::AgentStateChanged`] this is not derivable at the dispatch funnel and could not
    /// be: a shell handing its terminal to a job is a `tcsetpgrp` inside the pane's own process
    /// tree, and what reaches the daemon is bytes. So it too is emitted by the settle waker, which
    /// is the only observer with a clock — see [`crate::JobWatch`] for why watching it is
    /// affordable when reading the ANSWER is 2751 us.
    ///
    /// **The pane is the subject and the process group is not on the wire.** A reader answers this
    /// by re-reading `pane_processes`, where the terminal device, the child pid, and every member
    /// of the job with its `comm` and `argv` are defined once. Carrying the group id here would be a
    /// second encoding of a fact that slot already serves.
    ///
    /// **A pane whose CHILD EXITS reports it here**, and this is the only event that does: the job
    /// becomes nothing, which is a change like any other. A dead pane keeps its place and its final
    /// screen, so [`Event::PaneClosed`] does not fire for it — that variant's docs say a child's
    /// death "is not yet any event", and this is the variant that makes that sentence out of date.
    ///
    /// **Not emitted for a pane's FIRST reading.** Going from *nobody has looked* to *this group*
    /// is discovery, not a change, and announcing it would report a job change for every pane on
    /// the first sweep after boot. Same rule [`Event::PaneSelected`] states for a window's first
    /// active pane.
    PaneJobChanged(u64),
}

impl Event {
    /// What kind of change this is — the derivation [`EventKind`] exists for.
    ///
    /// Exhaustive on purpose and not by accident: this match is what makes a new variant impossible
    /// to add without giving it a name a waiter can ask for.
    #[must_use]
    pub const fn kind(&self) -> EventKind {
        match self {
            Self::PaneCreated(_) => EventKind::PaneCreated,
            Self::PaneClosed(_) => EventKind::PaneClosed,
            Self::PaneSelected(_) => EventKind::PaneSelected,
            Self::AgentStateChanged(_) => EventKind::PaneAgentStateChanged,
            Self::PaneJobChanged(_) => EventKind::PaneJobChanged,
            Self::WindowCreated(_) => EventKind::WindowCreated,
            Self::WindowClosed(_) => EventKind::WindowClosed,
            Self::WindowSelected(_) => EventKind::WindowSelected,
            Self::SessionCreated(_) => EventKind::SessionCreated,
            Self::SessionClosed(_) => EventKind::SessionClosed,
            Self::LayoutUpdated => EventKind::LayoutUpdated,
        }
    }

    /// Who this change is about, or `None` for [`Event::LayoutUpdated`] — the one variant whose
    /// subject is the arrangement itself, which is one object and so has nothing to name.
    #[must_use]
    pub fn subject(&self) -> Option<Subject> {
        match self {
            Self::PaneCreated(id)
            | Self::PaneClosed(id)
            | Self::PaneSelected(id)
            | Self::AgentStateChanged(id)
            | Self::PaneJobChanged(id) => Some(Subject::Pane(*id)),
            Self::WindowCreated(name) | Self::WindowClosed(name) | Self::WindowSelected(name) => {
                Some(Subject::Window(name.clone()))
            }
            Self::SessionCreated(name) | Self::SessionClosed(name) => {
                Some(Subject::Session(name.clone()))
            }
            Self::LayoutUpdated => None,
        }
    }

    /// This event as the wire object a client parses: `{type, <subject key>?}`.
    ///
    /// **The ONE place an event becomes JSON**, read by the `events.<since>` slot and by the reply to
    /// a filtered wait alike. It used to be an eleven-arm `match` in the serializer with the type
    /// names written out as literals — which was correct and was also the second spelling of a
    /// vocabulary, free to drift from any other reader of it the moment one existed. A filter is
    /// exactly such a reader.
    ///
    /// The subject key comes from the KIND and the value from the SUBJECT, so the pairing is stated
    /// once and asserted by `a_kinds_subject_key_agrees_with_the_event_that_produced_it`.
    #[must_use]
    pub fn to_wire(&self) -> Value {
        let kind = self.kind();
        let mut object = serde_json::Map::new();
        object.insert("type".to_owned(), Value::from(kind.wire_str()));
        if let (Some(key), Some(subject)) = (kind.subject_key(), self.subject()) {
            object.insert(key.to_owned(), subject.wire_value());
        }
        Value::Object(object)
    }
}

/// WHICH changes a waiter wants to be woken for — a disjunction of [`Clause`]s, or *everything*.
///
/// ## Any-of over clauses, all-of inside one
///
/// `[{kind: pane_job_changed, pane: 3}, {kind: pane_closed, pane: 3}]` is *wake me when pane 3's job
/// changes OR pane 3 disappears* — one wait, one connection, one answer. That disjunction is the
/// shape the job-change event needs and cannot get from a single match: a build finishing and the
/// pane running it dying are the two ways the thing a caller is waiting for can end, and a caller
/// forced to pick one waits forever on the other. The rival's `events.wait` takes exactly one
/// `EventMatch` (`api/schema/events.rs:89` at `9a4ce5e1`), so there it is two waits — and
/// `pane_closed` is one of the eighteen match shapes its handler refuses at runtime.
///
/// ## [`Everything`](Self::Everything) is the default, so the parameter is additive
///
/// A caller that passes no filter gets what it gets today: every change in its session. Nothing
/// existing has to be rewritten to keep working, and the wait is still strictly better for it,
/// because the thing this removes — waking on OUTPUT — was never a change at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EventFilter {
    /// No constraint: every recorded change matches.
    Everything,
    /// At least one clause must match. Never empty — see [`EventFilter::from_wire`], which refuses an
    /// empty list rather than accepting a filter that can match nothing.
    AnyOf(Vec<Clause>),
}

/// One term of an [`EventFilter`]: a kind, a subject, or both.
///
/// Both fields optional, and that is the expressiveness this has over a per-variant match enum:
///
/// * `{kind: pane_closed}` — *any* pane closing. The rival's `PaneClosed { pane_id: String }` makes
///   the subject mandatory, so this question cannot be asked there at all.
/// * `{pane: 3}` — anything about pane 3, whatever the vocabulary grows to.
/// * both — the narrow case, and the common one.
///
/// **Every accepted clause is satisfiable.** A clause with neither field, or one naming a subject the
/// kind cannot carry, is refused by [`EventFilter::from_wire`] rather than accepted as a predicate
/// that is always false — a wait that can never return is the worst answer this surface could give,
/// and it would arrive as silence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clause {
    /// The kind this clause admits, or `None` for any kind.
    pub kind: Option<EventKind>,
    /// The subject this clause admits, or `None` for any subject.
    pub subject: Option<Subject>,
}

impl Clause {
    /// Whether `event` satisfies every constraint this clause states.
    fn matches(&self, event: &Event) -> bool {
        if let Some(kind) = self.kind
            && kind != event.kind()
        {
            return false;
        }
        if let Some(subject) = &self.subject
            && Some(subject) != event.subject().as_ref()
        {
            return false;
        }
        true
    }
}

impl EventFilter {
    /// The wire key the filter rides under in a wait's params.
    pub const WIRE_KEY: &'static str = "match";

    /// Whether `event` is one this filter's holder asked to be woken for.
    #[must_use]
    pub fn matches(&self, event: &Event) -> bool {
        match self {
            Self::Everything => true,
            Self::AnyOf(clauses) => clauses.iter().any(|clause| clause.matches(event)),
        }
    }

    /// Whether any of `events` matches — the question the park decision asks.
    #[must_use]
    pub fn matches_any<'a>(&self, events: impl IntoIterator<Item = &'a Event>) -> bool {
        events.into_iter().any(|event| self.matches(event))
    }

    /// Keep only the events this filter admits.
    #[must_use]
    pub fn retain(&self, events: Vec<Event>) -> Vec<Event> {
        match self {
            // The common case pays nothing: no filter, no walk, no reallocation.
            Self::Everything => events,
            Self::AnyOf(_) => events
                .into_iter()
                .filter(|event| self.matches(event))
                .collect(),
        }
    }

    /// Parse the `match` parameter of a wait: absent or `null` is [`Everything`](Self::Everything);
    /// otherwise a non-empty array of clause objects.
    ///
    /// ## Every refusal here is a caller mistake that would otherwise arrive as SILENCE
    ///
    /// This parser is deliberately strict, because the failure mode of a permissive one is a wait
    /// that never returns and never says why:
    ///
    /// * An **unknown `kind`** — a client written against a newer or older vocabulary. Refused
    ///   naming the vocabulary, so the reply says what this daemon can be asked for.
    /// * An **unknown KEY** (`panes`, `pane_id`, `id`) — a typo, or a caller writing herdr's
    ///   vocabulary. Accepting it as an unconstrained clause would turn a narrow wait into
    ///   *everything* and look like it worked.
    /// * An **empty clause** — `{}` means *everything*, which is what omitting the parameter means.
    ///   Two spellings of one thing, and the likelier reading of `{}` is a caller whose intended
    ///   constraint went missing.
    /// * An **empty list** — a disjunction over no clauses is FALSE, so it would park forever.
    /// * A **subject the kind cannot carry** — `{kind: layout_updated, pane: 3}`. Statically
    ///   contradictory, so it is a mistake and not a question.
    /// * **Two subjects in one clause** — a clause names one subject; two is the same contradiction.
    ///
    /// # Errors
    ///
    /// The sentence to show the caller, phrased for an operator or an agent rather than for a
    /// decoder: it names the offending key or word and what would be accepted.
    pub fn from_wire(value: Option<&Value>) -> Result<Self, String> {
        let Some(value) = value.filter(|value| !value.is_null()) else {
            return Ok(Self::Everything);
        };
        let Some(list) = value.as_array() else {
            return Err(format!(
                "{}: must be a list of clauses like [{{\"kind\":\"pane_job_changed\",\"pane\":2}}] \
                 — omit it to be woken by any change",
                Self::WIRE_KEY,
            ));
        };
        if list.is_empty() {
            return Err(format!(
                "{}: is empty, so nothing could ever match it — omit it to be woken by any change",
                Self::WIRE_KEY,
            ));
        }
        let clauses = list
            .iter()
            .map(Self::clause_from_wire)
            .collect::<Result<Vec<Clause>, String>>()?;
        Ok(Self::AnyOf(clauses))
    }

    /// One clause of [`from_wire`](Self::from_wire)'s list.
    fn clause_from_wire(value: &Value) -> Result<Clause, String> {
        let Some(object) = value.as_object() else {
            return Err(format!(
                "{}: each clause must be an object with a \"kind\" and/or a subject \
                 (\"{}\", \"{}\", \"{}\")",
                Self::WIRE_KEY,
                Subject::PANE_KEY,
                Subject::WINDOW_KEY,
                Subject::SESSION_KEY,
            ));
        };
        let mut kind = None;
        let mut subject: Option<Subject> = None;
        for (key, value) in object {
            let found = match key.as_str() {
                "kind" => {
                    let name = value
                        .as_str()
                        .ok_or_else(|| format!("{}: \"kind\" must be a string", Self::WIRE_KEY))?;
                    kind = Some(EventKind::from_wire(name).ok_or_else(|| {
                        format!(
                            "{}: \"{name}\" is not a change this terminal reports — one of: {}",
                            Self::WIRE_KEY,
                            Self::vocabulary(),
                        )
                    })?);
                    continue;
                }
                Subject::PANE_KEY => Subject::Pane(value.as_u64().ok_or_else(|| {
                    format!(
                        "{}: \"{}\" must be a pane id (a whole number)",
                        Self::WIRE_KEY,
                        Subject::PANE_KEY,
                    )
                })?),
                Subject::WINDOW_KEY | Subject::SESSION_KEY => {
                    let name = value.as_str().ok_or_else(|| {
                        format!("{}: \"{key}\" must be a name (a string)", Self::WIRE_KEY)
                    })?;
                    if key == Subject::WINDOW_KEY {
                        Subject::Window(name.to_owned())
                    } else {
                        Subject::Session(name.to_owned())
                    }
                }
                other => {
                    return Err(format!(
                        "{}: \"{other}\" is not part of a clause — use \"kind\" and/or one of \
                         \"{}\", \"{}\", \"{}\"",
                        Self::WIRE_KEY,
                        Subject::PANE_KEY,
                        Subject::WINDOW_KEY,
                        Subject::SESSION_KEY,
                    ));
                }
            };
            if let Some(had) = subject.replace(found) {
                return Err(format!(
                    "{}: a clause names ONE subject, and this one names both \"{}\" and \"{key}\"",
                    Self::WIRE_KEY,
                    had.wire_key(),
                ));
            }
        }
        match (kind, &subject) {
            (None, None) => Err(format!(
                "{}: an empty clause matches everything — omit the whole parameter to say that",
                Self::WIRE_KEY,
            )),
            // The static contradiction: this kind's subject is not of the sort named.
            (Some(kind), Some(subject)) if kind.subject_key() != Some(subject.wire_key()) => {
                Err(match kind.subject_key() {
                    Some(key) => format!(
                        "{}: \"{}\" names a \"{key}\", so it cannot be constrained by \"{}\"",
                        Self::WIRE_KEY,
                        kind.wire_str(),
                        subject.wire_key(),
                    ),
                    None => format!(
                        "{}: \"{}\" is about the whole arrangement, so it names no \"{}\"",
                        Self::WIRE_KEY,
                        kind.wire_str(),
                        subject.wire_key(),
                    ),
                })
            }
            _ => Ok(Clause { kind, subject }),
        }
    }

    /// The WIRE form of a filter narrowed to a pane and/or a set of kind NAMES, or `None` when
    /// neither was given — the shape a client sends as the [`WIRE_KEY`](Self::WIRE_KEY) parameter.
    ///
    /// ## Why this lives here and not in each client
    ///
    /// Both `sprag events -f` and the MCP `wait_for_change` tool offer the same narrowing, and each
    /// grew its own copy of this cross-product first: one clause per kind, each carrying the pane. Two
    /// spellings of one wire shape, in a round whose whole thesis is that a vocabulary must be spelled
    /// once. So it is spelled once, beside the parser that reads it back.
    ///
    /// ## Why the kinds are STRINGS and are not validated here
    ///
    /// The vocabulary belongs to the DAEMON. A client that checked a kind locally would be a second
    /// enforcement point, and an older client would refuse a kind the daemon it is talking to knows
    /// perfectly well. Passing the word through means the answer always comes from the end that
    /// actually has the list — with the whole list in the refusal ([`from_wire`](Self::from_wire)).
    ///
    /// The output is what `from_wire` accepts, which
    /// `the_narrowing_a_client_sends_is_what_the_daemon_parses_back` pins as a round trip rather than
    /// as two shapes that happen to agree today.
    #[must_use]
    pub fn narrowing_wire(pane: Option<u64>, kinds: &[String]) -> Option<Value> {
        match (pane, kinds) {
            (None, []) => None,
            (Some(id), []) => Some(serde_json::json!([{ Subject::PANE_KEY: id }])),
            (pane, kinds) => Some(Value::Array(
                kinds
                    .iter()
                    .map(|kind| match pane {
                        Some(id) => serde_json::json!({ "kind": kind, Subject::PANE_KEY: id }),
                        None => serde_json::json!({ "kind": kind }),
                    })
                    .collect(),
            )),
        }
    }

    /// Every kind's wire name, comma-separated — for the refusal that has to say what IS accepted.
    fn vocabulary() -> String {
        EventKind::ALL
            .iter()
            .map(|kind| kind.wire_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// WHAT KIND of change an [`Event`] is, with no subject attached — the event's own name.
///
/// ## Why this exists, and why it is DERIVED rather than declared beside [`Event`]
///
/// A waiter says which changes it wants to be woken for ([`EventFilter`]), and the thing it names is
/// a kind. So the kind has to be a value that can be parsed off the wire and compared, which a
/// variant of an enum carrying subjects is not.
///
/// The load-bearing word is *derived*: [`Event::kind`] is an exhaustive match, so a new [`Event`]
/// variant **does not compile** until it has a kind, and is therefore filterable by construction.
/// That is the whole difference between this and a parallel match vocabulary. The rival at
/// `9a4ce5e1` declares three enumerations of one vocabulary — 26 `EventKind`, 19 `EventMatch`, 27
/// `Subscription` — with nothing forcing them to agree, and **seven kinds their daemon emits cannot
/// be named to their `events.wait` at all** (`layout.updated` among them). Nothing here can drift
/// that way, because there is only one list and the compiler owns it.
///
/// [`wire_str`](Self::wire_str) is also the ONE place an event's wire name is spelled. Before this
/// type the names lived in the serializer, so the word a client read and the word a filter would
/// have to parse were two independent literals; now [`Event::to_wire`] reads them from here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EventKind {
    /// [`Event::PaneCreated`].
    PaneCreated,
    /// [`Event::PaneClosed`].
    PaneClosed,
    /// [`Event::PaneSelected`].
    PaneSelected,
    /// [`Event::AgentStateChanged`].
    PaneAgentStateChanged,
    /// [`Event::PaneJobChanged`].
    PaneJobChanged,
    /// [`Event::WindowCreated`].
    WindowCreated,
    /// [`Event::WindowClosed`].
    WindowClosed,
    /// [`Event::WindowSelected`].
    WindowSelected,
    /// [`Event::SessionCreated`].
    SessionCreated,
    /// [`Event::SessionClosed`].
    SessionClosed,
    /// [`Event::LayoutUpdated`].
    LayoutUpdated,
}

impl EventKind {
    /// Every kind, so a test can walk the whole vocabulary rather than the subset its author
    /// remembered.
    ///
    /// Hand-listed, and kept honest by `all_lists_every_kind_and_each_round_trips`: that test
    /// asserts this slice's LENGTH against a literal, so a new variant fails it until it is added
    /// here — the same "assert the count, not just the exit code" discipline R275 cost a round.
    pub const ALL: &'static [Self] = &[
        Self::PaneCreated,
        Self::PaneClosed,
        Self::PaneSelected,
        Self::PaneAgentStateChanged,
        Self::PaneJobChanged,
        Self::WindowCreated,
        Self::WindowClosed,
        Self::WindowSelected,
        Self::SessionCreated,
        Self::SessionClosed,
        Self::LayoutUpdated,
    ];

    /// This kind's name on the wire — the `type` field of an event object, and the word a
    /// [`Clause`] names.
    ///
    /// The SSOT for that word. `pane_agent_state_changed` is deliberately not the variant's own
    /// name: the wire vocabulary prefixes a pane's facts with `pane_`, and the variant it comes from
    /// is [`Event::AgentStateChanged`], which is named for the observer that emits it.
    #[must_use]
    pub const fn wire_str(self) -> &'static str {
        match self {
            Self::PaneCreated => "pane_created",
            Self::PaneClosed => "pane_closed",
            Self::PaneSelected => "pane_selected",
            Self::PaneAgentStateChanged => "pane_agent_state_changed",
            Self::PaneJobChanged => "pane_job_changed",
            Self::WindowCreated => "window_created",
            Self::WindowClosed => "window_closed",
            Self::WindowSelected => "window_selected",
            Self::SessionCreated => "session_created",
            Self::SessionClosed => "session_closed",
            Self::LayoutUpdated => "layout_updated",
        }
    }

    /// The kind a wire name denotes, or `None` for a word this build's vocabulary does not contain.
    ///
    /// `None` is what makes an unknown kind in a filter a REFUSAL rather than a clause that can
    /// never match: a caller writing against a vocabulary this daemon does not have would otherwise
    /// park forever and be told nothing, which is the silence this project treats as a bug.
    #[must_use]
    pub fn from_wire(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|kind| kind.wire_str() == name)
    }

    /// The wire KEY this kind's subject rides under (`pane` / `window` / `session`), or `None` for a
    /// kind whose change has no subject to name.
    ///
    /// Keyed by what the subject IS rather than by a generic `id`, which is [`Event::to_wire`]'s own
    /// contract: a reader that has matched on `type` already knows which slot to re-read, and the key
    /// confirms it rather than making it guess.
    ///
    /// This is also what lets a [`Clause`] be REFUSED at parse time instead of matching nothing
    /// forever: `{kind: "pane_closed", window: "0"}` is a contradiction the parser can see, because
    /// this says `pane_closed` names a pane. Pinned against [`Event::subject`] by
    /// `a_kinds_subject_key_agrees_with_the_event_that_produced_it`.
    #[must_use]
    pub const fn subject_key(self) -> Option<&'static str> {
        match self {
            Self::PaneCreated
            | Self::PaneClosed
            | Self::PaneSelected
            | Self::PaneAgentStateChanged
            | Self::PaneJobChanged => Some(Subject::PANE_KEY),
            Self::WindowCreated | Self::WindowClosed | Self::WindowSelected => {
                Some(Subject::WINDOW_KEY)
            }
            Self::SessionCreated | Self::SessionClosed => Some(Subject::SESSION_KEY),
            // The arrangement is ONE object: see `Event::LayoutUpdated`.
            Self::LayoutUpdated => None,
        }
    }
}

/// WHO an [`Event`] is about — the thing a reader re-reads to learn the new value.
///
/// Owns its name rather than borrowing it, so that ONE type serves both sides of a match: an event's
/// subject and the subject a [`Clause`] constrains it to. Two types (a borrowed one for events, an
/// owned one for filters) would be two definitions of "which pane" free to disagree about what
/// equality means, for an allocation of a window name — `"0"`, in the workspaces this daemon builds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Subject {
    /// A pane, by [`PaneInfo::id`](sprag_terminal::PaneInfo::id).
    Pane(u64),
    /// A window, by name — a window's name IS its address in a session.
    Window(String),
    /// A session, by name.
    Session(String),
}

impl Subject {
    /// The wire key a pane subject rides under. See [`EventKind::subject_key`].
    pub const PANE_KEY: &'static str = "pane";
    /// The wire key a window subject rides under.
    pub const WINDOW_KEY: &'static str = "window";
    /// The wire key a session subject rides under.
    pub const SESSION_KEY: &'static str = "session";

    /// This subject's value on the wire — an integer id, or a name.
    #[must_use]
    pub fn wire_value(&self) -> Value {
        match self {
            Self::Pane(id) => Value::from(*id),
            Self::Window(name) | Self::Session(name) => Value::from(name.clone()),
        }
    }

    /// The wire key this subject would ride under, for the parse that has a subject but no kind.
    #[must_use]
    pub const fn wire_key(&self) -> &'static str {
        match self {
            Self::Pane(_) => Self::PANE_KEY,
            Self::Window(_) => Self::WINDOW_KEY,
            Self::Session(_) => Self::SESSION_KEY,
        }
    }
}

/// One [`Event`] with the revision it was appended at — the log's stored unit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    /// The scene revision this change landed at. The cursor vocabulary; see the module docs for why
    /// it is the scene's own token and not a counter this log owns.
    pub revision: u64,
    /// What changed.
    pub event: Event,
}

/// What a reader gets back for a cursor: the changes after it, where to read from next, and whether
/// anything was dropped before it could be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Batch {
    /// The events after the requested cursor, oldest first.
    pub events: Vec<Event>,
    /// The cursor to ask with next time.
    ///
    /// Not simply "the revision of the last event returned": a reader that asked from ahead of the
    /// log keeps its own cursor, so this never moves BACKWARDS, which a caller that stores it would
    /// otherwise have to defend against itself.
    pub next: u64,
    /// **Records were evicted that this cursor had not yet seen.** The reader's picture has a hole
    /// in it that no future read can fill.
    ///
    /// The [`events`](Self::events) beside this flag are still real and still worth applying; what
    /// the flag says is that something BEFORE them is missing. The answer to it is the re-read of
    /// everything that a client does on every wake today — so a `lost` batch degrades exactly to
    /// the behaviour this log is an optimization over, which is why the log can be adopted before
    /// it is complete without a client ever being wrong.
    pub lost: bool,
}

impl Batch {
    /// This batch as the wire object both readers of it answer with: `{events, next, lost}`.
    ///
    /// **`lost` travels even when it is false**, unlike the `skip_serializing_if` shapes elsewhere in
    /// this tree. Those omit a field to keep an addition wire-compatible with an older peer; this one
    /// is a SAFETY answer, and a peer that cannot see the key would read a hole as a clean read.
    /// Absent must not be able to mean "fine".
    ///
    /// One shape for the `events.<since>` slot and for a filtered wait's reply, so the two cannot
    /// answer one question differently — the rule that put [`Event::to_wire`] in this module.
    #[must_use]
    pub fn to_wire(&self) -> Value {
        serde_json::json!({
            "events": self.events.iter().map(Event::to_wire).collect::<Vec<Value>>(),
            "next": self.next,
            "lost": self.lost,
        })
    }
}

/// A bounded, revision-keyed ring of recent changes.
///
/// Bounded because a daemon runs for weeks and nothing acknowledges a read: an unbounded log is a
/// leak whose size is set by uptime. The bound is what makes [`Batch::lost`] necessary, and the two
/// are the whole contract — a reader is never handed a silently truncated list.
///
/// Not internally synchronised. The caller holds it under whatever lock guards the session it
/// belongs to, which is also what makes [`record`](Self::record)'s group atomic.
#[derive(Debug)]
pub struct EventLog {
    /// Oldest first. Revisions are non-decreasing along this queue — [`record`](Self::record)
    /// enforces it, and [`since`](Self::since) relies on it to stop scanning.
    records: VecDeque<Record>,
    /// The most records retained. See [`new`](Self::new) for why a group larger than this is the
    /// caller's problem and not a case this type tries to survive.
    capacity: usize,
    /// The highest revision any EVICTED record carried, or `None` while nothing has been evicted.
    ///
    /// This single value is the whole `lost` decision, and it stays correct even when eviction
    /// splits a revision GROUP in half: a cursor at or above it cannot have wanted anything
    /// evicted, because every evicted record's revision is at most this and a cursor is asking for
    /// revisions strictly greater than itself.
    evicted_through: Option<u64>,
}

impl EventLog {
    /// A log retaining at most `capacity` records.
    ///
    /// # Panics
    ///
    /// If `capacity` is zero — a log that retains nothing answers every read `lost`, which is a
    /// working configuration for no one and a silent one to debug.
    ///
    /// A group larger than `capacity` evicts its own head as it is appended, so the group is never
    /// readable whole. That is not defended against here: the caller is a mutation's structural
    /// consequences, which are a handful, and a capacity below that is a misconfiguration to fix
    /// rather than a runtime case to survive.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        assert!(
            capacity > 0,
            "an event log retaining nothing answers every read `lost`"
        );
        Self {
            records: VecDeque::new(),
            capacity,
            evicted_through: None,
        }
    }

    /// Append everything one mutation changed, at the revision it changed at.
    ///
    /// The GROUP is the unit deliberately. A reader takes the same lock this write does, so a
    /// caller with no way to append half a group has no way to expose one — the torn read is
    /// removed by the signature rather than by a rule the next caller has to remember.
    ///
    /// An empty group is a no-op, which is the common case: most dispatches change nothing
    /// structural.
    ///
    /// # Panics
    ///
    /// If `revision` is below the last one recorded. The scene revision is monotonic and this log
    /// is ordered by it; a caller that went backwards has paired a change with the wrong token, and
    /// every cursor comparison below would quietly answer from the wrong place.
    pub fn record(&mut self, revision: u64, events: impl IntoIterator<Item = Event>) {
        for event in events {
            if let Some(last) = self.records.back() {
                assert!(
                    revision >= last.revision,
                    "an event log is ordered by the scene revision, and this record went backwards \
                     ({revision} after {}) — the change was paired with the wrong token",
                    last.revision,
                );
            }
            self.records.push_back(Record { revision, event });
            while self.records.len() > self.capacity {
                let evicted = self
                    .records
                    .pop_front()
                    .expect("a queue longer than a non-zero capacity has a front");
                self.evicted_through = Some(evicted.revision);
            }
        }
    }

    /// Everything recorded after `cursor`, and whether anything before it was already lost.
    ///
    /// `cursor` is a revision the reader has already accounted for, so the answer is the records
    /// STRICTLY above it — the same half-open convention `scene/waitFor {since}` uses, so a reader
    /// can hand the number it got from one straight to the other.
    #[must_use]
    pub fn since(&self, cursor: u64) -> Batch {
        let events: Vec<Event> = self
            .records
            .iter()
            .skip_while(|record| record.revision <= cursor)
            .map(|record| record.event.clone())
            .collect();
        // The head, not the last event RETURNED: a reader ahead of the log keeps its own cursor
        // rather than being wound back to a revision the log happens to stop at.
        let head = self.records.back().map_or(cursor, |record| record.revision);
        Batch {
            events,
            next: head.max(cursor),
            lost: self.evicted_through.is_some_and(|through| cursor < through),
        }
    }

    /// How many records are retained — for the tests that pin eviction actually evicting.
    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether nothing has been recorded (or everything has been evicted, which cannot happen while
    /// `capacity > 0`).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

/// One session's change journal: the shape it was last seen in, and the log of what has moved.
///
/// The two are ONE type behind ONE lock deliberately. Deriving a diff means reading the old shape,
/// comparing, appending, and storing the new one; splitting those across two locks would let two
/// observers diff against the same predecessor and record the change twice, which no reader could
/// tell from the change having happened twice. Today's dispatch is single-threaded and would not
/// expose it — which is exactly why it is closed structurally now rather than after a second
/// producer arrives.
#[derive(Debug)]
pub struct SessionJournal {
    log: EventLog,
    /// `None` until the first observation. The first read of a session therefore records NOTHING —
    /// there is no predecessor to have changed from, and inventing one would report a whole
    /// workspace as freshly created to the client that merely arrived first.
    shape: Option<SessionShape>,
}

/// How many records a session's journal retains — **derived, and here is the derivation.**
///
/// It bounds how far a reader may fall behind before [`Batch::lost`] sends it back to a full
/// re-read. R269 shipped this as a round number with "not earned yet" written above it; what earns
/// it is two measurements and one choice.
///
/// ## What a record costs
///
/// `size_of::<Record>()` is **40 bytes** (`size_of::<Event>()` is 32). The `String` variants add a
/// small heap allocation each, but they carry window and session NAMES — `"0"`, `"1"` — so that
/// term is a byte or two. At this capacity a journal is **~10 KB per session**, and a session gets
/// one only once something has been announced on it.
///
/// ## What an operation costs, in records
///
/// Measured, and pinned by `rpc::tests::the_records_per_operation_ratio_the_ring_is_sized_against`
/// so the derivation cannot rot under a later change:
///
/// | operation | records |
/// |---|---|
/// | spawn | 1 |
/// | close | 1 (2 when it closes the ACTIVE pane, which hands off) |
/// | new window | 1 |
/// | select window | 1 |
/// | select pane | 1 |
/// | **split** | **3** (the pane, the ARRANGEMENT, and the active pane it moves to) |
///
/// The split's third record arrived with H7 and is the reason this table is a test rather than a
/// paragraph: a split makes its new pane active (tmux's rule, applied in the daemon so every caller
/// gets it), and that is a real change a reader must be told about. The number moved from 2 to 3
/// under a change that had nothing to do with this ring, which is exactly what pinning it catches.
///
/// ## The choice
///
/// Building the widest workspace this project measures — 64 panes, the top of `sprag-latency`'s
/// `REGISTRY_SIZES` — costs **188 records** by split, the three-record shape
/// (`rpc::tests::a_workspace_scale_burst_fits_the_ring_with_room`). So this capacity still holds a
/// full workspace-scale reconstruction with a third to spare, for 10 KB.
///
/// Erring large is the right direction rather than a hedge: an undersized ring costs a client the
/// re-read it already does today, while the memory is 40 bytes a record. What the number buys is
/// bounded and stateable — **85 worst-case operations between two reads by one client** — which is
/// hours at a human's rate and seconds under a script hammering the mux, and the script is exactly
/// the case `lost` exists to answer honestly.
///
/// ## What does NOT consume it
///
/// A daemon restart does not: a restore rebuilds the registry before the socket mounts, and a
/// session's FIRST observation has no predecessor and records nothing
/// ([`SessionJournal::observe`]). Pane output does not either — it is deliberately not a record
/// (see the module docs), which is the decision that keeps this ring measured in operations rather
/// than in bytes-per-second.
pub const JOURNAL_CAPACITY: usize = 256;

impl SessionJournal {
    /// A journal that has observed nothing.
    #[must_use]
    pub fn new() -> Self {
        Self {
            log: EventLog::new(JOURNAL_CAPACITY),
            shape: None,
        }
    }

    /// Read `session`'s shape, record what moved since the last observation at `revision`, and keep
    /// the new shape.
    ///
    /// Called after a mutating dispatch, with the revision that dispatch advanced the scene to — so
    /// a client woken by that very bump reads a record keyed at the number it was woken with.
    pub fn observe(&mut self, registry: &SessionRegistry, session: &str, revision: u64) {
        let next = SessionShape::read(registry, session);
        if let Some(previous) = self.shape.replace(next) {
            // `self.shape` now holds the new one, so diff against what it replaced.
            let events = previous.diff(self.shape.as_ref().expect("just replaced"));
            self.log.record(revision, events);
        }
    }

    /// Record changes an OBSERVER saw, which no shape comparison could have derived.
    ///
    /// Separate from [`observe`](Self::observe) rather than folded into it, and the split is the
    /// same one [`Event::AgentStateChanged`] describes: [`observe`](Self::observe) DERIVES from
    /// state the daemon publishes, so a new mutating method inherits it; this EMITS what only the
    /// producer knows. Folding them would put a discipline back — an emitter would have to be
    /// invoked from the derive site, which does not run on the thread that sees these.
    ///
    /// A caller that has nothing to say passes an empty group and records nothing.
    pub fn emit(&mut self, revision: u64, events: impl IntoIterator<Item = Event>) {
        self.log.record(revision, events);
    }

    /// Everything recorded after `cursor`. See [`EventLog::since`].
    #[must_use]
    pub fn since(&self, cursor: u64) -> Batch {
        self.log.since(cursor)
    }
}

impl Default for SessionJournal {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A log big enough that nothing in the test evicts unless the test means it to.
    fn log() -> EventLog {
        EventLog::new(8)
    }

    #[test]
    fn a_reader_gets_what_happened_after_its_cursor_and_nothing_it_has_seen() {
        let mut log = log();
        log.record(1, [Event::PaneCreated(7)]);
        log.record(2, [Event::WindowSelected("two".to_owned())]);

        let batch = log.since(0);
        assert_eq!(
            batch.events,
            vec![
                Event::PaneCreated(7),
                Event::WindowSelected("two".to_owned())
            ],
            "a reader that has seen nothing gets everything, oldest first",
        );
        assert_eq!(batch.next, 2, "and is told where to resume");
        assert!(!batch.lost, "nothing was dropped");

        assert_eq!(
            log.since(1).events,
            vec![Event::WindowSelected("two".to_owned())],
            "a cursor is EXCLUSIVE — revision 1 is what this reader already accounted for",
        );
        assert!(
            log.since(2).events.is_empty(),
            "and a reader level with the head is told nothing happened, not told again",
        );
    }

    #[test]
    fn a_cursor_ahead_of_the_log_is_not_wound_backwards() {
        // A reader's cursor comes from `scene/waitFor`, which advances on bumps this log never
        // records (pane output, most of all). So being ahead of the head is the NORMAL state, not
        // an error, and answering it with the head would hand back a cursor that re-delivers
        // everything between the two on the next read.
        let mut log = log();
        log.record(3, [Event::LayoutUpdated]);

        let batch = log.since(9);
        assert!(batch.events.is_empty(), "nothing has happened above 9");
        assert_eq!(batch.next, 9, "and the reader keeps its own, higher cursor");
    }

    #[test]
    fn one_mutations_consequences_land_together_or_not_at_all() {
        // THE reason `record` takes a group. Two events at one revision must not be separable by a
        // reader, or a client applies half a change and the half it missed is never re-offered:
        // its cursor has already passed that revision.
        let mut log = log();
        log.record(4, [Event::PaneCreated(2), Event::LayoutUpdated]);

        let batch = log.since(3);
        assert_eq!(
            batch.events,
            vec![Event::PaneCreated(2), Event::LayoutUpdated],
            "both consequences of the one mutation are in the one batch",
        );
        assert_eq!(
            log.since(4).events,
            Vec::new(),
            "and a cursor past that revision gets neither — which is why it had to get both",
        );
    }

    #[test]
    fn an_empty_group_records_nothing() {
        // The common case: most dispatches change nothing structural, and a log that grew an entry
        // per dispatch would evict at dispatch rate rather than at change rate.
        let mut log = log();
        log.record(1, []);
        assert!(
            log.is_empty(),
            "a mutation with no consequences leaves none"
        );
        assert_eq!(log.since(0).next, 0, "and moves no cursor");
    }

    #[test]
    fn a_reader_that_fell_behind_is_told_so_rather_than_handed_a_hole() {
        // The whole point of the bound. Silently returning the surviving suffix would leave the
        // client confident and wrong; `lost` is what sends it back to a full re-read.
        let mut log = EventLog::new(2);
        log.record(1, [Event::PaneCreated(1)]);
        log.record(2, [Event::PaneCreated(2)]);
        log.record(3, [Event::PaneCreated(3)]);

        assert_eq!(log.len(), 2, "the ring is bounded — revision 1 was evicted");

        let stale = log.since(0);
        assert!(
            stale.lost,
            "a cursor below the evicted revision missed something no future read can supply",
        );
        assert_eq!(
            stale.events,
            vec![Event::PaneCreated(2), Event::PaneCreated(3)],
            "and is still given what survived — the flag says something is MISSING, not that \
             nothing is valid",
        );

        let caught_up = log.since(1);
        assert!(
            !caught_up.lost,
            "a cursor level with the highest evicted revision had already accounted for it",
        );
    }

    #[test]
    fn eviction_that_splits_a_revision_group_still_reports_the_loss() {
        // The boundary the single `evicted_through` value exists to survive. A group of two at
        // revision 2 is half-evicted, so a reader at cursor 1 would otherwise be handed the
        // surviving half of a change it has never seen the start of.
        let mut log = EventLog::new(2);
        log.record(1, [Event::PaneCreated(1)]);
        log.record(2, [Event::PaneCreated(2), Event::LayoutUpdated]);
        // Pushing the second event of revision 2 evicted revision 1; pushing a third evicts the
        // FIRST HALF of revision 2.
        log.record(3, [Event::PaneClosed(2)]);

        assert!(
            log.since(1).lost,
            "revision 2 is now half gone, so a cursor below it has a hole",
        );
        assert!(
            !log.since(2).lost,
            "while a cursor that had already accounted for revision 2 lost nothing it wanted",
        );
    }

    #[test]
    #[should_panic(expected = "went backwards")]
    fn a_change_paired_with_an_older_revision_is_a_bug_and_says_so() {
        // Not defensive tidiness: the cursor comparisons in `since` are ordering-dependent, so a
        // record out of order would answer readers from the wrong place with no symptom at the
        // site that caused it.
        let mut log = log();
        log.record(5, [Event::LayoutUpdated]);
        log.record(4, [Event::LayoutUpdated]);
    }

    #[test]
    #[should_panic(expected = "retaining nothing")]
    fn a_log_that_could_retain_nothing_is_refused_at_construction() {
        let _ = EventLog::new(0);
    }

    /// One event of every kind — the RATCHET for the tests below.
    ///
    /// The match is on the KIND and is exhaustive, so a new [`EventKind`] does not compile until it
    /// has a sample here, and every test that walks [`EventKind::ALL`] then covers it.
    fn sample(kind: EventKind) -> Event {
        match kind {
            EventKind::PaneCreated => Event::PaneCreated(7),
            EventKind::PaneClosed => Event::PaneClosed(7),
            EventKind::PaneSelected => Event::PaneSelected(7),
            EventKind::PaneAgentStateChanged => Event::AgentStateChanged(7),
            EventKind::PaneJobChanged => Event::PaneJobChanged(7),
            EventKind::WindowCreated => Event::WindowCreated("two".to_owned()),
            EventKind::WindowClosed => Event::WindowClosed("two".to_owned()),
            EventKind::WindowSelected => Event::WindowSelected("two".to_owned()),
            EventKind::SessionCreated => Event::SessionCreated("work".to_owned()),
            EventKind::SessionClosed => Event::SessionClosed("work".to_owned()),
            EventKind::LayoutUpdated => Event::LayoutUpdated,
        }
    }

    #[test]
    fn all_lists_every_kind_and_each_round_trips_its_wire_name() {
        // The COUNT is the ratchet on `ALL` itself: the exhaustive match in `sample` forces a new
        // variant to be handled, but nothing forces it into a hand-written slice — so a walk over
        // `ALL` would silently not cover it. R275 cost a round to exactly this shape of silence.
        assert_eq!(
            EventKind::ALL.len(),
            11,
            "a kind was added or removed — update `ALL` and this count together",
        );
        for kind in EventKind::ALL {
            assert_eq!(
                EventKind::from_wire(kind.wire_str()),
                Some(*kind),
                "{} must parse back to the kind it prints",
                kind.wire_str(),
            );
        }
        assert_eq!(
            EventKind::from_wire("pane_output"),
            None,
            "a word this vocabulary does not contain is None, which is what makes a filter naming \
             it a refusal rather than a wait that can never return",
        );
    }

    #[test]
    fn a_kinds_subject_key_agrees_with_the_event_that_produced_it() {
        // The pairing `Event::to_wire` relies on: it takes the KEY from the kind and the VALUE from
        // the event, so a kind that disagreed with its own events would emit a key with no value or
        // a value with no key.
        for kind in EventKind::ALL {
            let event = sample(*kind);
            match (kind.subject_key(), event.subject()) {
                (Some(key), Some(subject)) => assert_eq!(
                    key,
                    subject.wire_key(),
                    "{} says its subject rides under {key}, but its event carries a {}",
                    kind.wire_str(),
                    subject.wire_key(),
                ),
                (None, None) => {}
                (key, subject) => panic!(
                    "{} disagrees with its own event about having a subject: {key:?} vs {subject:?}",
                    kind.wire_str(),
                ),
            }
        }
    }

    #[test]
    fn the_wire_form_of_every_event_is_exactly_what_the_serializer_used_to_write() {
        // The whole point of the refactor is that the wire did NOT move. These eleven objects are
        // the literals the eleven-arm match in `workspace::events_value` wrote, transcribed from it
        // — so if `kind()`, `subject()` or `to_wire()` renames or re-keys anything, this fails
        // rather than a client failing later.
        let expected = [
            (
                EventKind::PaneCreated,
                json!({ "type": "pane_created", "pane": 7 }),
            ),
            (
                EventKind::PaneClosed,
                json!({ "type": "pane_closed", "pane": 7 }),
            ),
            (
                EventKind::PaneSelected,
                json!({ "type": "pane_selected", "pane": 7 }),
            ),
            (
                EventKind::PaneAgentStateChanged,
                json!({ "type": "pane_agent_state_changed", "pane": 7 }),
            ),
            (
                EventKind::PaneJobChanged,
                json!({ "type": "pane_job_changed", "pane": 7 }),
            ),
            (
                EventKind::WindowCreated,
                json!({ "type": "window_created", "window": "two" }),
            ),
            (
                EventKind::WindowClosed,
                json!({ "type": "window_closed", "window": "two" }),
            ),
            (
                EventKind::WindowSelected,
                json!({ "type": "window_selected", "window": "two" }),
            ),
            (
                EventKind::SessionCreated,
                json!({ "type": "session_created", "session": "work" }),
            ),
            (
                EventKind::SessionClosed,
                json!({ "type": "session_closed", "session": "work" }),
            ),
            (
                EventKind::LayoutUpdated,
                json!({ "type": "layout_updated" }),
            ),
        ];
        assert_eq!(
            expected.len(),
            EventKind::ALL.len(),
            "every kind's wire form must be pinned, not most of them",
        );
        for (kind, want) in expected {
            assert_eq!(
                sample(kind).to_wire(),
                want,
                "the wire form of {} moved",
                kind.wire_str(),
            );
        }
    }

    #[test]
    fn a_batchs_wire_form_carries_lost_even_when_it_is_false() {
        // Absent must not be able to mean "fine": a peer that cannot see the key would read a hole
        // as a clean read. The rest of this tree omits absent fields for compatibility; this one is
        // a safety answer and does not.
        let mut log = log();
        log.record(3, [Event::LayoutUpdated]);
        let wire = log.since(0).to_wire();

        assert_eq!(wire["events"], json!([{ "type": "layout_updated" }]));
        assert_eq!(wire["next"], json!(3));
        assert_eq!(wire["lost"], json!(false), "false TRAVELS");
        assert!(
            wire.get("lost").is_some(),
            "and is present as a key, not merely falsy",
        );
    }

    /// Parse a filter from the wire form a caller would send.
    fn filter(value: Value) -> Result<EventFilter, String> {
        EventFilter::from_wire(Some(&value))
    }

    #[test]
    fn no_filter_at_all_is_everything_so_the_parameter_is_additive() {
        // A caller that passes nothing keeps today's contract exactly, which is what lets the
        // parameter be added without rewriting the callers that already wait.
        assert_eq!(EventFilter::from_wire(None), Ok(EventFilter::Everything));
        assert_eq!(
            EventFilter::from_wire(Some(&Value::Null)),
            Ok(EventFilter::Everything),
            "an explicit null is the same statement as omitting it",
        );
        assert!(
            EventFilter::Everything.matches(&Event::LayoutUpdated),
            "and it admits every kind, including the one with no subject",
        );
    }

    #[test]
    fn a_clause_constrains_the_kind_the_subject_or_both() {
        let job_of_three = filter(json!([{ "kind": "pane_job_changed", "pane": 3 }]))
            .expect("a kind and a subject");
        assert!(job_of_three.matches(&Event::PaneJobChanged(3)));
        assert!(
            !job_of_three.matches(&Event::PaneJobChanged(5)),
            "another pane's job is not what this waiter asked about — THE case that was \
             impossible before",
        );
        assert!(
            !job_of_three.matches(&Event::PaneClosed(3)),
            "nor another kind of news about the right pane",
        );

        let any_close = filter(json!([{ "kind": "pane_closed" }])).expect("a bare kind");
        assert!(any_close.matches(&Event::PaneClosed(9)));
        assert!(
            any_close.matches(&Event::PaneClosed(1)),
            "ANY pane closing — a question herdr's mandatory pane_id cannot ask at all",
        );

        let anything_about_three = filter(json!([{ "pane": 3 }])).expect("a bare subject");
        assert!(anything_about_three.matches(&Event::AgentStateChanged(3)));
        assert!(anything_about_three.matches(&Event::PaneClosed(3)));
        assert!(!anything_about_three.matches(&Event::PaneClosed(4)));
        assert!(
            !anything_about_three.matches(&Event::LayoutUpdated),
            "an event with no subject cannot satisfy a subject constraint",
        );
    }

    #[test]
    fn clauses_are_a_disjunction_so_one_wait_covers_two_endings() {
        // The shape the job event needs: a build finishing and the pane running it dying are the two
        // ways the thing a caller waits for can end, and a caller forced to name one waits forever
        // on the other. The rival's `events.wait` takes ONE match, so this is two waits there.
        let done_or_gone = filter(json!([
            { "kind": "pane_job_changed", "pane": 2 },
            { "kind": "pane_closed", "pane": 2 },
        ]))
        .expect("two clauses");

        assert!(
            done_or_gone.matches(&Event::PaneJobChanged(2)),
            "the build ended"
        );
        assert!(
            done_or_gone.matches(&Event::PaneClosed(2)),
            "or the pane did"
        );
        assert!(
            !done_or_gone.matches(&Event::PaneJobChanged(7)),
            "and nothing else"
        );
    }

    #[test]
    fn a_filter_keeps_only_what_was_asked_for() {
        let mine = filter(json!([{ "pane": 2 }])).expect("a subject clause");
        let batch = vec![
            Event::PaneJobChanged(2),
            Event::PaneJobChanged(5),
            Event::LayoutUpdated,
            Event::PaneClosed(2),
        ];

        assert!(mine.matches_any(&batch), "the park decision sees a match");
        assert_eq!(
            mine.retain(batch.clone()),
            vec![Event::PaneJobChanged(2), Event::PaneClosed(2)],
            "and the answer carries the caller's two, in order, not the session's four",
        );
        assert_eq!(
            EventFilter::Everything.retain(batch.clone()),
            batch,
            "while no filter is not a walk: the vector comes back as it went in",
        );
    }

    #[test]
    fn every_way_a_filter_can_be_a_mistake_is_refused_by_a_sentence() {
        // Each of these would otherwise be a wait that never returns and never says why — the
        // silence this project treats as a bug. The MESSAGE is asserted, not just the Err: a
        // refusal an agent cannot act on is barely better than the silence.
        let refusals = [
            (
                json!("pane_job_changed"),
                "must be a list of clauses",
                "a bare string instead of a list",
            ),
            (
                json!([]),
                "nothing could ever match",
                "an empty list is FALSE, not everything",
            ),
            (
                json!([{}]),
                "an empty clause matches everything",
                "an empty clause",
            ),
            (
                json!([{ "kind": "pane_output" }]),
                "is not a change this terminal reports",
                "a kind from another vocabulary",
            ),
            (
                json!([{ "pane_id": 3 }]),
                "is not part of a clause",
                "herdr's key name, which would otherwise widen the wait to everything",
            ),
            (
                json!([{ "pane": "3" }]),
                "must be a pane id (a whole number)",
                "a pane id as a string",
            ),
            (
                json!([{ "kind": "pane_closed", "window": "0" }]),
                "cannot be constrained by",
                "a subject of the wrong SORT for the kind",
            ),
            (
                json!([{ "kind": "layout_updated", "pane": 1 }]),
                "is about the whole arrangement",
                "a subject on the kind that has none",
            ),
            (
                json!([{ "pane": 1, "window": "0" }]),
                "names ONE subject",
                "two subjects in one clause",
            ),
        ];

        for (wire, expected, why) in refusals {
            let error = filter(wire.clone()).expect_err(&format!("{why} must be refused: {wire}"));
            assert!(
                error.contains(expected),
                "{why}: expected a sentence containing {expected:?}, got {error:?}",
            );
            assert!(
                error.starts_with(EventFilter::WIRE_KEY),
                "and every refusal names the parameter it is about: {error:?}",
            );
        }
    }

    #[test]
    fn the_narrowing_a_client_sends_is_what_the_daemon_parses_back() {
        // The two ends of ONE shape: `narrowing_wire` is what `sprag events -f` and the MCP wait tool
        // build, `from_wire` is what the daemon reads. Pinned as a round trip rather than as two
        // shapes that happen to agree today — which is the whole failure mode this round is about.
        let kinds = ["pane_job_changed".to_owned(), "pane_closed".to_owned()];

        assert_eq!(
            EventFilter::narrowing_wire(None, &[]),
            None,
            "a caller that narrowed nothing sends no parameter at all",
        );

        let pane_only = EventFilter::narrowing_wire(Some(3), &[]).expect("a pane clause");
        assert_eq!(
            EventFilter::from_wire(Some(&pane_only)),
            Ok(EventFilter::AnyOf(vec![Clause {
                kind: None,
                subject: Some(Subject::Pane(3)),
            }])),
            "a pane alone is anything about that pane",
        );

        let both = EventFilter::narrowing_wire(Some(3), &kinds).expect("two clauses");
        let parsed = EventFilter::from_wire(Some(&both)).expect("the daemon parses its own shape");
        assert_eq!(
            parsed,
            EventFilter::AnyOf(vec![
                Clause {
                    kind: Some(EventKind::PaneJobChanged),
                    subject: Some(Subject::Pane(3)),
                },
                Clause {
                    kind: Some(EventKind::PaneClosed),
                    subject: Some(Subject::Pane(3)),
                },
            ]),
            "a pane and two kinds is the cross product, one clause each",
        );
        assert!(parsed.matches(&Event::PaneJobChanged(3)));
        assert!(parsed.matches(&Event::PaneClosed(3)));
        assert!(!parsed.matches(&Event::PaneJobChanged(4)));

        let kinds_only = EventFilter::narrowing_wire(None, &kinds).expect("two bare kinds");
        let parsed = EventFilter::from_wire(Some(&kinds_only)).expect("parses");
        assert!(
            parsed.matches(&Event::PaneClosed(99)),
            "kinds without a pane are those kinds for any subject",
        );
    }

    #[test]
    fn the_unknown_kind_refusal_names_the_whole_vocabulary_it_could_have_asked_for() {
        // A client written against a newer or older daemon needs to be told what THIS one reports —
        // otherwise the fix is a guess. Derived from `ALL`, so it cannot list a stale set.
        let error = filter(json!([{ "kind": "pane_output_matched" }])).expect_err("unknown kind");
        for kind in EventKind::ALL {
            assert!(
                error.contains(kind.wire_str()),
                "the refusal must offer {}, and says: {error}",
                kind.wire_str(),
            );
        }
    }
}
