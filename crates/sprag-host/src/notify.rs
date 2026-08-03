//! Per-session change notification: which clients a change wakes.
//!
//! A wire client does not poll. It parks on `scene/waitFor {since}`, and the daemon answers when
//! the scene has moved past `since`. The question this module owns is *whose* scene.
//!
//! ## Why the token is per SESSION and not per daemon
//!
//! There used to be one [`SceneRevision`] for the whole registry, so every attached client woke on
//! every other session's output — re-read its own session, found nothing, re-parked. Safe (a wake
//! is a hint to re-read, and the re-read is scoped and exact) but wrong in the way that matters
//! once several sessions are genuinely busy at once: the cost of a change scales with the number
//! of ATTACHED CLIENTS rather than with the number that could possibly care. `scene/waitFor`
//! already accepted and validated a `session` scope; it simply did not honour it.
//!
//! A session is the right grain because it is the unit a client attaches to, and because it is the
//! unit a pane cannot leave. `Session::break_pane` / `Session::join_pane` move a pane between
//! WINDOWS of one session and there is no operation that moves one between sessions — which is
//! what makes it safe for a pane's `on_dirty` bumper to capture its session's token once, at
//! spawn, and never revisit the question. Had panes been able to migrate, a captured token would
//! have gone on announcing the pane's OLD session for the rest of its life, and nothing would have
//! reported the drift; the grain is chosen so that hazard cannot arise rather than so it can be
//! remembered.
//!
//! ## Why a whole token per session, rather than one counter and per-session waiter sets
//!
//! Because the wake must not be a discipline. pinion bumps the [`SceneRevision`] it is handed
//! after every mutating handler returns `Ok`, from inside its own dispatcher — so with one shared
//! counter, sprag would have to remember to wake the right session after every such call, and a
//! forgotten one is a client parked forever with no error anywhere. Handing pinion the SCOPED
//! session's token instead makes the attribution structural: the bump lands on that session's
//! revision, whose observer wakes that session's waiters, and a new mutating method inherits it
//! without knowing this module exists.
//!
//! The cost is that a revision number is only comparable within one session. That is a real
//! contract, and the one client that waits already keeps it: `sprag-gui` re-reads `scene/revision`
//! on the connection it has just re-scoped, both at boot and on a session switch, so the baseline
//! it parks with always came from the session it parks on.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use pinion_core::SceneRevision;
use pinion_rpc::{ConnId, RequestId, RpcReply, WaiterRegistry};

use crate::events::{Batch, Event, EventFilter, SessionJournal};

/// One session's change-notification channel: the scene-version token its changes advance, and the
/// parked `scene/waitFor` replies waiting on it. The token's observer is the ONLY thing that fires
/// those replies, and it is installed when the channel is minted — so no announce site has to
/// remember the wake half.
struct SessionChannel {
    revision: Arc<SceneRevision>,
    waiters: Arc<WaiterRegistry>,
    /// What has CHANGED in this session, keyed by the very token above, **and the filtered waits
    /// parked on it** — the payload half of a wake. It lives here, beside the revision, because the
    /// revision is its cursor vocabulary: a journal kept anywhere else would be keyed by a number
    /// this map owns, which is the fork [`crate::events`] refuses.
    ///
    /// Two kinds of client park on one session, and the split between this and
    /// [`waiters`](Self::waiters) is the point: a DISPLAY client parks on the revision, because
    /// output makes its projection stale and it must re-read; a client waiting for a NAMED change
    /// parks here, because output is not a change and must not wake it.
    journal: Arc<JournalChannel>,
}

impl SessionChannel {
    /// Mint a channel and wire its wake observer.
    ///
    /// `set_observer` is install-once (pinion): the first caller wins and later ones no-op. A fresh
    /// revision per channel makes the install always succeed here; the assert catches a future
    /// refactor that hands in an already-observed one, which would leave every wait on this session
    /// parked forever with nothing to report it.
    fn new() -> Self {
        let revision = Arc::new(SceneRevision::default());
        let waiters = Arc::new(WaiterRegistry::new());
        let wake = Arc::clone(&waiters);
        assert!(
            revision.set_observer(move |n| {
                wake.wake(n);
            }),
            "a session channel requires a fresh SceneRevision: its wake observer must install \
             (an already-observed revision would leave scene/waitFor parked forever)",
        );
        Self {
            revision,
            waiters,
            journal: Arc::new(JournalChannel::new()),
        }
    }
}

/// One session's change journal AND the filtered waits parked on it, behind ONE lock.
///
/// ## Why the two are one type
///
/// A waiter asks *"wake me when a change matching this lands"*, so the park decision is *"is there
/// already a matching record above my cursor?"* — and that question is only sound if no append can
/// land between reading the answer and being parked. pinion carries the scar on exactly this point:
/// [`WaiterRegistry::park_if_current`] reads the live revision **under the lock its `wake` takes**,
/// because anything else is the lost wakeup. Two locks here would admit the same bug; one lock makes
/// it unrepresentable.
///
/// The second reason is the rule R271 paid for: **a new authority must enter where the old one
/// publishes its change.** Every append site in the daemon — [`ChannelRegistry::observe`], which the
/// dispatch funnel calls after every mutating method, and [`ChannelRegistry::announce`], which the
/// settle sweep calls — goes through this type, and this type has no method that appends without
/// waking. A caller cannot forget the wake half because there is nothing to forget.
///
/// ## What it deliberately does NOT do
///
/// It holds no clock and no deadline. A parked wait's lifetime is its CONNECTION's: the client sets
/// its own socket read deadline, and when it gives up and closes, the transport's reader calls
/// `RpcIngress::on_disconnect` and [`release`](Self::release) drops that connection's waits. A
/// deadline the daemon owned would need something to fire it, and the only clock in this daemon is
/// the settle sweep — whose interval is the agent subsystem's scheduling decision, and which would
/// make a timeout arrive up to `SWEEP_INTERVAL` late where the client's own deadline is exact. The
/// same hook releases a wait whose client CRASHED, which a filtered park needs more than an
/// unfiltered one: a filter that never matches would otherwise retain the entry for the daemon's
/// remaining life.
#[derive(Debug, Default)]
pub struct JournalChannel {
    inner: Mutex<Journal>,
}

/// [`JournalChannel`]'s guarded state: what has changed, and who is waiting to hear about it.
#[derive(Debug, Default)]
struct Journal {
    /// The change log — see [`crate::events`].
    log: SessionJournal,
    /// The waits parked on it. Never large: a wait is one client's one outstanding question, and the
    /// clients that ask are orchestrating agents rather than display clients (which park on the scene
    /// revision instead, and should).
    parked: Vec<ParkedWait>,
}

/// One client's outstanding question: *wake me when something matching this lands after `cursor`*.
#[derive(Debug)]
struct ParkedWait {
    /// The connection that asked — the key [`JournalChannel::release`] drops by, and the reason a
    /// crashed client leaks nothing.
    conn: ConnId,
    /// The revision the asker has already accounted for. Exclusive, like every other cursor here.
    cursor: u64,
    /// Which changes it wants. [`EventFilter::Everything`] is a caller that passed none.
    filter: EventFilter,
    /// The JSON-RPC id to answer under. `None` is a NOTIFICATION — parked and then deliberately not
    /// answered, which is pinion's own choice for an id-less `scene/waitFor` and is what JSON-RPC
    /// requires: there is nobody to tell.
    id: Option<RequestId>,
    /// Where the answer goes. Opaque, so it is NEVER run under the lock above — pinion's rule, and
    /// R291's own worst defect was a lock held across I/O.
    ///
    /// Consumed by [`RpcReply::send`], so it fires at most once by construction: a wait cannot be
    /// answered twice, whatever order a drain and an append arrive in.
    reply: RpcReply,
}

impl JournalChannel {
    /// A channel that has observed nothing and has nobody waiting.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Everything recorded after `cursor` — the `events.<since>` slot's answer. Unfiltered: see
    /// [`crate::workspace`]'s `events_value`.
    #[must_use]
    pub fn since(&self, cursor: u64) -> Batch {
        self.lock().log.since(cursor)
    }

    /// Read `session`'s shape, record what moved since the last observation, and wake whoever asked
    /// for one of those changes.
    ///
    /// Reads the revision INSIDE the lock rather than taking it as an argument, so the number a
    /// record is keyed by and the number a woken reader is answered with cannot be two numbers.
    pub fn observe(
        &self,
        revision: &SceneRevision,
        registry: &sprag_terminal::SessionRegistry,
        session: &str,
    ) {
        let fire = {
            let mut journal = self.lock();
            let at = revision.current();
            journal.log.observe(registry, session, at);
            Self::take_satisfied(&mut journal)
        };
        Self::answer(fire);
    }

    /// Announce a change, record what it was, and wake whoever asked for it — answering the revision
    /// the scene advanced to.
    ///
    /// ## The journal lock spans the bump, and that is the whole of this function
    ///
    /// The bump fires pinion's wake observer SYNCHRONOUSLY: a parked `scene/waitFor` reply is sent
    /// from inside the call. So a record appended after the bump returns is a record the woken client
    /// can race — it is told `R'`, asks for `(R, R']`, and is answered before the writer has said what
    /// happened. The client would see an empty batch, conclude nothing structural moved, and never be
    /// offered the record again: its cursor has passed that revision.
    ///
    /// Appending BEFORE the bump does not work either, because the record's key must be the revision
    /// the bump PRODUCES, and this thread cannot predict it — a pane's `on_dirty` may bump the same
    /// token concurrently.
    ///
    /// So the barrier is this lock: held across the bump and released only once the record is in. The
    /// woken client gets its reply during the bump, spends a socket round trip coming back, and then
    /// blocks on this lock for as long as it takes to append — after which the answer is complete.
    pub fn announce(&self, revision: &SceneRevision, events: Vec<Event>) -> u64 {
        let (at, fire) = {
            let mut journal = self.lock();
            let at = revision.bump();
            journal.log.emit(at, events);
            (at, Self::take_satisfied(&mut journal))
        };
        Self::answer(fire);
        at
    }

    /// Answer `reply` now if this cursor already has something to read, or park it until it does.
    ///
    /// "Something to read" is `lost` OR a matching event, and the `lost` half is not defensive
    /// tidiness: eviction may have taken **the very record this filter was waiting for**, and a
    /// filter cannot be applied to a record that is gone. So a reader with a hole is woken whatever
    /// its filter says, and answered with the flag that sends it to a full re-read.
    ///
    /// The decision and the park happen under one lock hold, which is the whole reason this type
    /// exists; the reply is sent after it is released.
    pub fn park_or_answer(
        &self,
        conn: ConnId,
        cursor: u64,
        filter: EventFilter,
        id: Option<RequestId>,
        reply: RpcReply,
    ) {
        // The reply and the id come back OUT of the lock hold when the answer is immediate, rather
        // than being cloned into the park: one of the two branches consumes them, and handing them
        // back is how that stays a move instead of a copy of a sink nothing should hold twice.
        let answer = {
            let mut journal = self.lock();
            let batch = journal.log.since(cursor);
            if satisfied(&batch, &filter) {
                Some((reply, id, filter_batch(&batch, &filter)))
            } else {
                journal.parked.push(ParkedWait {
                    conn,
                    cursor,
                    filter,
                    id,
                    reply,
                });
                None
            }
        };
        if let Some((reply, id, batch)) = answer {
            send(reply, id.as_ref(), &batch);
        }
    }

    /// Drop every wait `conn` parked — it closed, or it crashed. Answers how many went.
    ///
    /// Dropping rather than answering: a connection that is gone has nobody to tell, and writing into
    /// its sink is the one thing that could still fail. The client's own read deadline is what turned
    /// its wait into an answer ("nothing changed in the time you gave me"); this is only the daemon
    /// forgetting.
    pub fn release(&self, conn: ConnId) -> usize {
        let mut journal = self.lock();
        let before = journal.parked.len();
        journal.parked.retain(|wait| wait.conn != conn);
        before - journal.parked.len()
    }

    /// Fire every parked wait with whatever its cursor can see — the session is ENDING.
    ///
    /// The same reasoning [`ChannelRegistry::close`] gives for its bump, applied to this half: a
    /// client parked here for a session that has just been killed is waiting on a journal nothing will
    /// ever append to again. Released, it re-reads, meets the scope refusal, and applies its
    /// detach-on-destroy policy — the path any other client's kill puts it on.
    pub fn drain(&self) {
        let fire = {
            let mut journal = self.lock();
            let parked = std::mem::take(&mut journal.parked);
            parked
                .into_iter()
                .map(|wait| {
                    let batch = journal.log.since(wait.cursor);
                    (wait, batch)
                })
                .collect::<Vec<_>>()
        };
        Self::answer(fire);
    }

    /// How many waits are parked — for the tests that pin a park actually parking, and a release
    /// actually releasing.
    #[must_use]
    pub fn parked_count(&self) -> usize {
        self.lock().parked.len()
    }

    /// Take the waits whose question is now answerable, leaving the rest parked.
    ///
    /// Runs under the caller's lock hold, immediately after an append, so a wait cannot be evaluated
    /// against a journal that has moved since the record landed.
    fn take_satisfied(journal: &mut Journal) -> Vec<(ParkedWait, Batch)> {
        // Taken out first: the loop reads `journal.log` while deciding, which it could not do while
        // holding a mutable borrow of `journal.parked`.
        let parked = std::mem::take(&mut journal.parked);
        let mut fire = Vec::new();
        let mut kept = Vec::with_capacity(parked.len());
        for wait in parked {
            let batch = journal.log.since(wait.cursor);
            if satisfied(&batch, &wait.filter) {
                fire.push((wait, batch));
            } else {
                kept.push(wait);
            }
        }
        journal.parked = kept;
        fire
    }

    /// Send each satisfied wait its batch, filtered to what it asked for. **Called with no lock
    /// held**: a reply sink is opaque, and running one under this type's mutex is how a convoy starts.
    fn answer(fire: Vec<(ParkedWait, Batch)>) {
        for (wait, batch) in fire {
            let batch = filter_batch(&batch, &wait.filter);
            send(wait.reply, wait.id.as_ref(), &batch);
        }
    }

    /// The guarded state, recovering a poisoned lock the way the rest of the host does: a panic
    /// elsewhere must not make change notification unavailable for the daemon's remaining life.
    fn lock(&self) -> MutexGuard<'_, Journal> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Whether a cursor's [`Batch`] is something its holder asked to be woken for.
///
/// The `lost` term is first because it does not depend on the filter at all — see
/// [`JournalChannel::park_or_answer`].
fn satisfied(batch: &Batch, filter: &EventFilter) -> bool {
    batch.lost || filter.matches_any(&batch.events)
}

/// `batch` with only the events `filter` admits.
///
/// The reply carries what was ASKED FOR and not everything that happened, because `next` advances
/// past both: a caller handed events outside its filter would be reading noise now and would have no
/// way to re-read it later, which is the worst of the two. A caller that wants the whole history
/// re-reads the `events.<since>` slot, which is unfiltered by design.
fn filter_batch(batch: &Batch, filter: &EventFilter) -> Batch {
    Batch {
        events: filter.retain(batch.events.clone()),
        next: batch.next,
        lost: batch.lost,
    }
}

/// The JSON-RPC success response a satisfied filtered wait returns: the batch, exactly as the
/// `events.<since>` slot serves it ([`Batch::to_wire`]).
///
/// `None` for an id-less request: a NOTIFICATION has nobody to answer, and inventing a reply for one
/// would break JSON-RPC. pinion's own waiter makes the identical choice at the identical point.
fn send(reply: RpcReply, id: Option<&RequestId>, batch: &Batch) {
    if let Some(id) = id {
        reply.send(
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": batch.to_wire(),
            })
            .to_string(),
        );
    }
}

/// Every session's change channel — its scene-version token and its parked `scene/waitFor`
/// replies — keyed by session NAME.
///
/// Minted on FIRST USE rather than on session creation, which is what keeps this free of a
/// lifecycle to keep in step with the registry: a session nothing has announced on and nobody waits
/// on has no channel, and needs none. The one lifecycle event that does matter is a session ENDING
/// — see [`close`](Self::close) — because a client parked on a session that is gone has nothing
/// left to wake it.
#[derive(Default)]
pub struct ChannelRegistry {
    /// Keyed by session name. A `Mutex` rather than a `RwLock`: every operation here either mints
    /// or looks up, both of which are a few instructions under an uncontended lock, and the
    /// hot-path caller (a pane's `on_dirty`) holds an `Arc<SceneRevision>` captured at spawn and
    /// never comes back through this map at all.
    channels: Mutex<HashMap<String, SessionChannel>>,
}

impl ChannelRegistry {
    /// The scene-version token `session`'s changes advance — bumping it wakes exactly the clients
    /// parked on that session.
    ///
    /// Cloned out rather than borrowed, so a pane's `on_dirty` can capture it at spawn and announce
    /// its output without ever taking this lock (the R152 rule: nothing that runs per batch of PTY
    /// output may take a shared lock).
    #[must_use]
    pub fn revision(&self, session: &str) -> Arc<SceneRevision> {
        Arc::clone(&self.entry(session).revision)
    }

    /// The parked `scene/waitFor` replies waiting on `session` — what the dispatch parks into.
    #[must_use]
    pub fn waiters(&self, session: &str) -> Arc<WaiterRegistry> {
        Arc::clone(&self.entry(session).waiters)
    }

    /// `session`'s change journal — the payload a wake on this channel is about, and the waits parked
    /// for it.
    #[must_use]
    pub fn journal(&self, session: &str) -> Arc<JournalChannel> {
        Arc::clone(&self.entry(session).journal)
    }

    /// Record what moved in `session` since the last observation, at the revision it is now at, and
    /// wake whoever asked for one of those changes.
    ///
    /// Hands the channel its own revision token rather than a number: the number a record is keyed by
    /// and the number a parked waiter is released with must be the one token, and a caller free to
    /// pass a different one is a caller who eventually will.
    pub fn observe(&self, registry: &sprag_terminal::SessionRegistry, session: &str) {
        let entry = self.entry(session);
        entry.journal.observe(&entry.revision, registry, session);
    }

    /// Announce a change in `session`, answering the revision it advanced to.
    ///
    /// The convenience form of `revision(session).bump()`, for the announce sites that hold a name
    /// rather than a token.
    pub fn bump(&self, session: &str) -> u64 {
        self.revision(session).bump()
    }

    /// Announce a change in `session` AND record what it was, so a client woken by the bump can
    /// read the reason it was woken — and so a client that asked for exactly this change is woken by
    /// the record rather than by the bump.
    ///
    /// The lock discipline that makes the two orders safe lives in [`JournalChannel::announce`],
    /// which owns both halves.
    pub fn announce(&self, session: &str, events: Vec<Event>) -> u64 {
        let entry = self.entry(session);
        entry.journal.announce(&entry.revision, events)
    }

    /// Drop every filtered wait `conn` parked, wherever it parked one. Answers how many went.
    ///
    /// Walks every session's channel because a connection is not scoped to one: a client may park on
    /// the session it is attached to and then be re-scoped by `client/attach`. The map is the size of
    /// the LIVE session set ([`close`](Self::close) forgets the rest), so this is a walk over sessions
    /// rather than over history.
    pub fn release(&self, conn: ConnId) -> usize {
        let journals: Vec<Arc<JournalChannel>> = self
            .lock()
            .values()
            .map(|channel| Arc::clone(&channel.journal))
            .collect();
        // The map lock is released before the channels are touched: two locks, never held at once,
        // and always in this order.
        journals.iter().map(|journal| journal.release(conn)).sum()
    }

    /// A session ENDED: wake everything parked on it, then forget its channel.
    ///
    /// The wake is not a courtesy. A client parked on `scene/waitFor` for a session that has just
    /// been killed would otherwise wait on a token nothing can ever advance again — no pane of that
    /// session is left to produce output, and no request will be scoped to it. Bumping first is
    /// what fires the observer and releases them; each then re-reads, meets the scope refusal, and
    /// applies its `detach-on-destroy` policy, which is the path they take when any other client
    /// kills their session.
    ///
    /// Forgetting the channel afterwards is what keeps this map the size of the LIVE session set
    /// rather than of every session the daemon has ever held. A session re-created under the same
    /// name mints a fresh channel starting from zero, which is sound because the only client that
    /// could hold a stale baseline for that name was detached by this very call.
    pub fn close(&self, session: &str) {
        // Bump through the entry (so the observer fires and the waiters drain) BEFORE removing it:
        // a channel removed first is a channel whose parked replies nothing owns.
        self.bump(session);
        // The same argument for the OTHER kind of parked client. A filtered wait is not released by
        // the bump — that is the whole point of it — so a session ending has to release it here, or
        // the one client that asked a precise question is the one left hanging.
        self.entry(session).journal.drain();
        self.lock().remove(session);
    }

    /// How many sessions currently have a channel — the live size of this map, for the test that
    /// pins [`close`](Self::close) actually forgetting one.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Whether no session has a channel yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// `session`'s channel, minting it on first use. Returns the pieces by clone because the map
    /// lock must not be held while a caller uses them.
    fn entry(&self, session: &str) -> SessionChannel {
        let mut channels = self.lock();
        let channel = channels
            .entry(session.to_owned())
            .or_insert_with(SessionChannel::new);
        SessionChannel {
            revision: Arc::clone(&channel.revision),
            waiters: Arc::clone(&channel.waiters),
            journal: Arc::clone(&channel.journal),
        }
    }

    /// The map, recovering a poisoned lock the way the rest of the host does: a panic elsewhere
    /// must not make change notification unavailable for the daemon's remaining life.
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, SessionChannel>> {
        self.channels.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_session_gets_its_own_token_and_the_same_one_twice() {
        let channels = ChannelRegistry::default();
        let a = channels.revision("work");
        let b = channels.revision("work");
        let other = channels.revision("play");

        a.bump();
        assert_eq!(
            b.current(),
            1,
            "one session, one token — asking twice is asking once"
        );
        assert_eq!(
            other.current(),
            0,
            "another session's token is untouched by this one's change",
        );
    }

    /// Park a `scene/waitFor` on `session` from revision 0, answering the replies it fires.
    ///
    /// The request id is REAL and not `None`: a parked waiter with no id is a JSON-RPC
    /// notification, which pinion wakes and then deliberately does not answer — so a test that
    /// parks without one measures nothing and reads as a wake that never happened.
    fn park(channels: &ChannelRegistry, session: &str) -> Arc<std::sync::Mutex<Vec<String>>> {
        let replies = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Arc::clone(&replies);
        channels.waiters(session).park_if_current(
            &channels.revision(session),
            0,
            Some(pinion_rpc::RequestId::Num(1)),
            pinion_rpc::RpcReply::new(move |reply| {
                sink.lock().expect("the reply sink").push(reply);
            }),
        );
        replies
    }

    /// How many replies a parked wait has fired.
    fn answered(replies: &Arc<std::sync::Mutex<Vec<String>>>) -> usize {
        replies.lock().expect("the reply sink").len()
    }

    #[test]
    fn a_change_wakes_only_the_session_it_happened_in() {
        // THE claim. Before this grain existed, every attached client woke on every session's
        // output; a `waitFor` parked on `play` had no way to sleep through `work` being busy.
        let channels = ChannelRegistry::default();
        let replies = park(&channels, "play");
        assert_eq!(
            channels.waiters("play").parked_count(),
            1,
            "the wait is parked, not answered — `play` has not moved",
        );

        channels.bump("work");
        assert_eq!(
            answered(&replies),
            0,
            "a change in `work` is not this client's business",
        );
        assert_eq!(
            channels.waiters("play").parked_count(),
            1,
            "and it is still asleep, not woken-and-re-parked",
        );

        channels.bump("play");
        assert_eq!(answered(&replies), 1, "its own session's change reaches it");
    }

    #[test]
    fn closing_a_session_releases_whoever_was_waiting_on_it() {
        // A killed session's waiters are parked on a token no pane can advance again. Left alone
        // they would hang until the connection died; released, each re-reads, meets the scope
        // refusal, and detaches — the same path any other client's kill puts them on.
        let channels = ChannelRegistry::default();
        let replies = park(&channels, "doomed");

        channels.close("doomed");
        assert_eq!(answered(&replies), 1, "the parked wait was released");
        assert!(
            channels.is_empty(),
            "and the channel is forgotten, so the map tracks the LIVE sessions",
        );
    }

    /// Park a FILTERED wait on `session`'s journal from cursor 0, on connection `conn`, answering
    /// the replies it fires.
    ///
    /// The id is REAL for the reason [`park`] gives: an id-less wait is a notification, which is
    /// parked and then deliberately never answered, so a test using one measures nothing.
    fn park_filtered(
        channels: &ChannelRegistry,
        session: &str,
        conn: ConnId,
        filter: EventFilter,
    ) -> Arc<std::sync::Mutex<Vec<String>>> {
        let replies = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Arc::clone(&replies);
        channels.journal(session).park_or_answer(
            conn,
            0,
            filter,
            Some(RequestId::Num(1)),
            RpcReply::new(move |reply| {
                sink.lock().expect("the reply sink").push(reply);
            }),
        );
        replies
    }

    /// A filter matching one pane's job — the "wait until the build in that pane finishes" question.
    fn job_of(pane: u64) -> EventFilter {
        EventFilter::AnyOf(vec![crate::events::Clause {
            kind: Some(crate::events::EventKind::PaneJobChanged),
            subject: Some(crate::events::Subject::Pane(pane)),
        }])
    }

    /// The events a fired reply carried.
    fn carried(replies: &Arc<std::sync::Mutex<Vec<String>>>) -> Vec<serde_json::Value> {
        let replies = replies.lock().expect("the reply sink");
        let reply: serde_json::Value =
            serde_json::from_str(replies.first().expect("a reply")).expect("valid JSON-RPC");
        reply["result"]["events"]
            .as_array()
            .expect("the batch carries an events array")
            .clone()
    }

    #[test]
    fn output_alone_does_not_wake_a_filtered_wait() {
        // ⚠ THE DEFECT, at unit scale. A pane's output bumps the session's revision and appends
        // NOTHING, so a `scene/waitFor` is released by it and answers with an empty batch — measured
        // at 22 431 returns per second against a build-rate pane, every one of them empty, where a
        // quiet pane returns none. A wait parked HERE sleeps through all of it, because this
        // journal's wake condition is a RECORD and output is not one.
        let channels = ChannelRegistry::default();
        let replies = park_filtered(&channels, "work", ConnId::allocate(), job_of(2));
        assert_eq!(
            channels.journal("work").parked_count(),
            1,
            "parked, not answered"
        );

        for _ in 0..1_000 {
            channels.bump("work");
        }

        assert_eq!(
            answered(&replies),
            0,
            "a thousand batches of output are not a change this waiter asked about",
        );
        assert_eq!(
            channels.journal("work").parked_count(),
            1,
            "and it is still asleep, not woken-and-re-parked a thousand times",
        );

        channels.announce("work", vec![Event::PaneJobChanged(2)]);
        assert_eq!(
            answered(&replies),
            1,
            "the change it DID ask about reaches it"
        );
    }

    #[test]
    fn a_record_wakes_only_the_waiters_that_asked_for_it() {
        // The register's own statement of the gap: "an agent waiting on pane 2 is woken by pane 5's
        // build too and must re-call". Three waiters, one record, and only the two whose question it
        // answers are woken.
        let channels = ChannelRegistry::default();
        let two = park_filtered(&channels, "work", ConnId::allocate(), job_of(2));
        let five = park_filtered(&channels, "work", ConnId::allocate(), job_of(5));
        let anything = park_filtered(
            &channels,
            "work",
            ConnId::allocate(),
            EventFilter::Everything,
        );

        channels.announce("work", vec![Event::PaneJobChanged(2)]);

        assert_eq!(answered(&two), 1, "the waiter that named pane 2");
        assert_eq!(answered(&anything), 1, "and the one that named nothing");
        assert_eq!(
            answered(&five),
            0,
            "but NOT the one waiting on pane 5 — the whole point of a server-side filter",
        );
        assert_eq!(
            channels.journal("work").parked_count(),
            1,
            "and that one is still parked, having spent no round trip",
        );
    }

    #[test]
    fn a_woken_wait_carries_what_it_asked_for_and_not_the_rest() {
        // `next` advances past everything, so handing over events outside the filter would be noise
        // now AND no way to re-read it later. A caller that wants the whole history reads the
        // unfiltered slot.
        let channels = ChannelRegistry::default();
        let replies = park_filtered(&channels, "work", ConnId::allocate(), job_of(2));

        channels.announce(
            "work",
            vec![
                Event::PaneJobChanged(5),
                Event::PaneJobChanged(2),
                Event::LayoutUpdated,
            ],
        );

        assert_eq!(
            carried(&replies),
            vec![serde_json::json!({ "type": "pane_job_changed", "pane": 2 })],
            "one of the three, in the vocabulary the slot serves",
        );
    }

    #[test]
    fn a_matching_record_already_in_the_log_answers_without_parking() {
        // The `park_if_current` shape: the decision is made under the same lock an append takes, so
        // a caller whose answer is already there never parks — and a bump that lands between the two
        // cannot be lost, because there is no gap for it to land in.
        let channels = ChannelRegistry::default();
        channels.announce("work", vec![Event::PaneJobChanged(2)]);

        let replies = park_filtered(&channels, "work", ConnId::allocate(), job_of(2));

        assert_eq!(answered(&replies), 1, "answered immediately");
        assert_eq!(
            channels.journal("work").parked_count(),
            0,
            "and never parked at all",
        );
    }

    #[test]
    fn a_reader_that_lost_records_is_woken_whatever_its_filter_says() {
        // Eviction may have taken the very record this filter was waiting for, and a filter cannot be
        // applied to a record that is gone. So `lost` wakes regardless — and the answer says so, which
        // is what sends the client to the full re-read `Batch::lost` already means.
        let channels = ChannelRegistry::default();
        let journal = channels.journal("work");
        let revision = channels.revision("work");
        // Fill past the ring so the cursor-0 reader's history is evicted, with NOTHING matching the
        // filter: the only reason to wake this waiter is the hole.
        for _ in 0..=crate::events::JOURNAL_CAPACITY {
            journal.announce(&revision, vec![Event::PaneJobChanged(5)]);
        }

        let replies = park_filtered(&channels, "work", ConnId::allocate(), job_of(2));

        assert_eq!(answered(&replies), 1, "woken by the hole, not by a match");
        let reply: serde_json::Value =
            serde_json::from_str(&replies.lock().expect("sink")[0]).expect("valid JSON-RPC");
        assert_eq!(
            reply["result"]["lost"],
            serde_json::json!(true),
            "and told why"
        );
        assert!(
            carried(&replies).is_empty(),
            "with no events, because none of the survivors matched — the flag is the answer",
        );
    }

    #[test]
    fn a_connection_that_goes_away_takes_its_waits_with_it() {
        // A filtered park needs this more than an unfiltered one: an entry whose filter never matches
        // would otherwise be retained for the daemon's remaining life. `RpcIngress::on_disconnect`
        // fires however the client goes away, including a crash.
        let channels = ChannelRegistry::default();
        let doomed = ConnId::allocate();
        let mine = park_filtered(&channels, "work", doomed, job_of(2));
        let theirs = park_filtered(&channels, "other", doomed, job_of(2));
        let survivor = park_filtered(&channels, "work", ConnId::allocate(), job_of(2));
        assert_eq!(channels.journal("work").parked_count(), 2);

        assert_eq!(
            channels.release(doomed),
            2,
            "released across EVERY session it had parked on, not just the one it attached to",
        );

        assert_eq!(channels.journal("work").parked_count(), 1);
        assert_eq!(channels.journal("other").parked_count(), 0);
        assert_eq!(answered(&mine), 0, "a gone connection is not written to");
        assert_eq!(answered(&theirs), 0);
        assert_eq!(
            answered(&survivor),
            0,
            "and another connection's wait is untouched"
        );

        channels.announce("work", vec![Event::PaneJobChanged(2)]);
        assert_eq!(
            answered(&survivor),
            1,
            "which still wakes on its own change"
        );
    }

    #[test]
    fn closing_a_session_releases_a_filtered_wait_too() {
        // The bump releases `scene/waitFor`; it does NOT release a filtered wait, which is the whole
        // point of it. So the one client that asked a precise question would be the one left hanging
        // on a journal nothing will ever append to again.
        let channels = ChannelRegistry::default();
        let replies = park_filtered(&channels, "doomed", ConnId::allocate(), job_of(2));

        channels.close("doomed");

        assert_eq!(answered(&replies), 1, "released by the close");
        assert!(channels.is_empty(), "and the channel forgotten");
    }
}
