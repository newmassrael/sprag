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
//! * There is no agent-state variant YET. It is not a difference in the state above; it is the
//!   settle waker's own verdict transition, produced by a different thread on a different schedule,
//!   and it lands with that producer rather than being declared here ahead of anything emitting it.
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
//! applied to a variant this module had already shipped. It returns with the observer that can
//! actually see it, which is where the agent verdict lands too.
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

use std::collections::VecDeque;

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
    /// place and its final screen, so a child's death is not this event and not yet any event —
    /// see the module docs on what the funnel structurally cannot see.
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

/// How many records a session's journal retains.
///
/// **This number is not earned yet.** It bounds how far a reader may fall behind before it is told
/// to re-read everything, and the honest way to choose it is to measure how far a real client
/// actually falls behind — which needs the wire read that does not exist yet. It is deliberately
/// generous rather than tuned: a session accumulating 256 structural changes between two reads of a
/// client that is still connected is a client with a bigger problem than a truncated log.
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
}
