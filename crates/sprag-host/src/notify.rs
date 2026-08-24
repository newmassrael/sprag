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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use pinion_core::SceneRevision;
use pinion_rpc::{ConnId, RequestId, RpcEgress, RpcReply, WaiterRegistry};
use sprag_rpc::{PANE_PARAM, PANE_REVISION_FIELD};
use sprag_terminal::PaneId;

use crate::PaneFind;
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
    /// THREE kinds of client park on one session, and the split between this,
    /// [`waiters`](Self::waiters) and [`outputs`](Self::outputs) is the point: a DISPLAY client
    /// parks on the revision, because output makes its projection stale and it must re-read; a
    /// client waiting for a NAMED change parks here, because output is not a change and must not
    /// wake it; a client waiting for named OUTPUT parks in `outputs`, because output IS its
    /// subject but only some of it is its answer.
    journal: Arc<JournalChannel>,
    /// The waits whose subject is a pane's OUTPUT — woken by the revision like a display client,
    /// answered by a predicate like a filtered wait.
    ///
    /// It hangs off the revision rather than off the journal because output is the one thing the
    /// journal deliberately does not record ([`crate::events`]: a record per PTY batch would evict
    /// the ring at output rate), and the revision is the only token output moves.
    outputs: Arc<OutputChannel>,
    /// The waits whose subject is only that a pane MOVED — register item 631, and the FOURTH kind
    /// of client parked on one session.
    ///
    /// It is separate from [`outputs`](Self::outputs) for the reason those two are separate from
    /// [`journal`](Self::journal): the three questions are woken by the same token and answered by
    /// different evidence. A display client re-reads on any bump; an output wait re-reads and
    /// SEARCHES; this one reads a counter and answers it. Folding it into the output channel would
    /// make every revision wait pay a search over a pane's whole retained output — 338 µs at the
    /// default scrollback cap — to be told a number it already had.
    revisions: Arc<RevisionChannel>,
    /// The session NAME this channel currently answers to, shared with the wake observer.
    ///
    /// # Why the observer cannot just capture the name
    ///
    /// It used to, and a [`rename`](ChannelRegistry::rename) is what made that wrong: the whole
    /// point of a rename is that the channel MOVES to a new key, and an observer holding the old
    /// string would go on firing [`OutputSignal`] under a name no session answers to — so every
    /// `pane/waitForOutput` parked on the renamed session would sit through every batch of output
    /// its pane produced. The address is a fact that MOVES, so it is held once and read, not copied
    /// into a closure at mint time.
    ///
    /// **Locked, on the PTY reader thread, and that is affordable for a stated reason**: the read
    /// is inside the `arm()` edge, which is the idle→output TRANSITION and not the per-batch path
    /// (see [`SessionChannel::new`] for the R152 rule this stays on the right side of). The only
    /// writer is a rename, holding it for the length of one string move.
    address: Arc<Mutex<String>>,
}

impl SessionChannel {
    /// Mint a channel and wire its wake observer.
    ///
    /// `set_observer` is install-once (pinion): the first caller wins and later ones no-op. A fresh
    /// revision per channel makes the install always succeed here; the assert catches a future
    /// refactor that hands in an already-observed one, which would leave every wait on this session
    /// parked forever with nothing to report it.
    ///
    /// ## The observer runs on the PTY READER thread, so it does the least it can
    ///
    /// A pane's `on_dirty` bumps this token once per applied output batch
    /// (`sprag_terminal::PanePty`), and `bump` fires this closure SYNCHRONOUSLY. So everything here
    /// is on that reader thread, and anything expensive would back-pressure the terminal itself —
    /// the R152 rule ("nothing that runs per batch of PTY output may take a shared lock") and the
    /// same argument `crate::rpc::spawn_reaper` records for moving its scan off this thread.
    ///
    /// The output half is therefore two lock-free atomics and, on the false→true edge only, one
    /// non-blocking send: [`OutputChannel::arm`] decides, [`OutputSignal::fire`] delivers, and the
    /// PASS runs on the dispatch owner. See [`OutputChannel`] for why that is the right thread.
    fn new(session: &str, signal: &Arc<OutputSignal>) -> Self {
        let revision = Arc::new(SceneRevision::default());
        let waiters = Arc::new(WaiterRegistry::new());
        let outputs = Arc::new(OutputChannel::new());
        let revisions = Arc::new(RevisionChannel::new());
        let wake = Arc::clone(&waiters);
        let arm = Arc::clone(&outputs);
        let arm_revisions = Arc::clone(&revisions);
        let signal = Arc::clone(signal);
        let address = Arc::new(Mutex::new(session.to_owned()));
        let named = Arc::clone(&address);
        assert!(
            revision.set_observer(move |n| {
                wake.wake(n);
                // ⚠⚠ NON-SHORT-CIRCUITING, and the honest reason is COST rather than correctness:
                // one fire runs BOTH passes, so `||` would still evaluate everything — it would
                // just leave the second channel's `queued` false while a pass was genuinely on its
                // way, and the next bump would fire a redundant second time. `|` keeps the two
                // flags saying the same true thing, which is what `queued` is for.
                if arm.arm() | arm_revisions.arm() {
                    // Read on the EDGE, never per batch — and read rather than captured, because a
                    // rename moves this channel to another name (see `SessionChannel::address`).
                    let session = named.lock().unwrap_or_else(PoisonError::into_inner).clone();
                    signal.fire(&session);
                }
            }),
            "a session channel requires a fresh SceneRevision: its wake observer must install \
             (an already-observed revision would leave scene/waitFor parked forever)",
        );
        Self {
            revision,
            waiters,
            journal: Arc::new(JournalChannel::new()),
            outputs,
            revisions,
            address,
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
    /// The SUBSCRIPTIONS following it — the same question asked once instead of once per batch.
    ///
    /// A second vector rather than a flag on [`ParkedWait`], because the two differ in the one thing
    /// that matters at the derive site: a wait is CONSUMED when it fires (its
    /// [`RpcReply`] is a `FnOnce`) and a subscription ADVANCES. One vector holding both would need a
    /// branch at every touch to decide whether the entry survives, which is the shape a type is for.
    streams: Vec<Subscription>,
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

/// One client's standing interest: *keep telling me what matches this, and remember where I got to*.
///
/// The streaming half of [`ParkedWait`], and the two differ in exactly two fields — which is the
/// whole of what pinion R1552 (PINION-PR83) bought. Where a wait holds an [`RpcReply`] that fires
/// once and a fixed `cursor`, a subscription holds the connection's EGRESS, which fires as often as
/// there is something to say, and a cursor it ADVANCES as it delivers.
struct Subscription {
    /// The opaque, process-unique id every notification carries and `events/unsubscribe` takes.
    id: u64,
    /// The connection that asked — the key [`JournalChannel::release`] drops by, so a client that
    /// crashes without unsubscribing leaks nothing.
    conn: ConnId,
    /// How far this subscriber has been told about. Exclusive, like every other cursor here, and
    /// ADVANCED under the append's own lock — which is what makes a record deliverable exactly once.
    cursor: u64,
    /// Which changes it wants. The same [`EventFilter`] a wait carries, evaluated the same way.
    filter: EventFilter,
    /// Where notifications go. Cloned from the frame that opened the subscription, so a response and
    /// a notification to this client are written through ONE writer and therefore cannot interleave
    /// mid-frame — pinion's own ordering guarantee, inherited rather than re-established.
    egress: Arc<dyn RpcEgress>,
    /// Whether the opening frame's RESPONSE has gone out yet.
    ///
    /// **Registered disarmed, armed by the dispatch site afterwards** — pinion's own shape for the
    /// same hazard: a change landing between the register and the reply would write a notification
    /// naming a subscription id the client has not been told, which it can only discard. The window
    /// is sub-microsecond and closing it with a flag is structural where "register last" would be a
    /// rule to remember at every future call site.
    armed: bool,
    /// How many notifications this subscription has written — answered by `events/unsubscribe` so a
    /// client can reconcile its own count against the daemon's without a second method.
    delivered: u64,
}

impl std::fmt::Debug for Subscription {
    /// Hand-written for pinion's own reason on [`pinion_rpc::RpcFrame`]: [`RpcEgress`] is a trait
    /// object, and deriving would force every transport's writer to be [`Debug`] for the sake of a
    /// diagnostic line. The egress is named by WHAT IT IS instead — a writer's contents are not a
    /// fact about this subscription, but whether it can still reach a peer is.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Subscription")
            .field("id", &self.id)
            .field("conn", &self.conn)
            .field("cursor", &self.cursor)
            .field("filter", &self.filter)
            .field("armed", &self.armed)
            .field("delivered", &self.delivered)
            .field("reaches_a_peer", &self.egress.reaches_a_peer())
            .finish()
    }
}

/// The next subscription id. Process-wide rather than per-session, so an id is unique across the
/// daemon and a client holding several cannot confuse two sessions' streams.
static NEXT_SUBSCRIPTION: AtomicU64 = AtomicU64::new(1);

impl JournalChannel {
    /// A channel that has observed nothing and has nobody waiting.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a subscription for `conn`, DISARMED, and answer its id and the cursor it starts from.
    ///
    /// Disarmed is not an optimisation — a change landing before the client has read its own
    /// subscription id could only be discarded. The caller sends the opening
    /// response and then calls [`Self::arm`], which is the only order that cannot write a
    /// notification for an id the client has not learned.
    ///
    /// The cursor is the caller's `since` verbatim, so nothing between its last read and this call is
    /// skipped — the exact-resume half of [`crate::wire::EVENTS_SUBSCRIBE_METHOD`]'s contract.
    pub fn subscribe(
        &self,
        conn: ConnId,
        cursor: u64,
        filter: EventFilter,
        egress: Arc<dyn RpcEgress>,
    ) -> u64 {
        let id = NEXT_SUBSCRIPTION.fetch_add(1, Ordering::Relaxed);
        self.lock().streams.push(Subscription {
            id,
            conn,
            cursor,
            filter,
            egress,
            armed: false,
            delivered: 0,
        });
        id
    }

    /// Arm the subscription `id` and deliver anything that has landed since it was registered.
    ///
    /// Called after the opening response has gone out. The catch-up is not a special case: the
    /// ordinary delivery pass runs here, so a record that landed inside that sub-microsecond window
    /// is written by the same code every later one is — there is no "check first, then stream" fork
    /// and so no gap between the two for a record to fall into.
    pub fn arm(&self, id: u64) {
        let writes = {
            let mut journal = self.lock();
            if let Some(stream) = journal.streams.iter_mut().find(|s| s.id == id) {
                stream.armed = true;
            }
            Self::take_streamable(&mut journal)
        };
        Self::write_all(writes);
    }

    /// End the subscription `id` if `conn` holds it, answering how many notifications it delivered.
    ///
    /// SCOPED TO THE CONNECTION, and that is the access rule rather than a courtesy: an id is opaque
    /// but it is also guessable (a small integer), so a client that could close another's stream
    /// would be able to silence a peer it cannot otherwise address. [`None`] for an id this
    /// connection does not hold, which is one answer for "already closed", "never opened" and
    /// "somebody else's" — a caller learns only that it has no such stream, which is all any of the
    /// three entitles it to.
    pub fn unsubscribe(&self, conn: ConnId, id: u64) -> Option<u64> {
        let mut journal = self.lock();
        let at = journal
            .streams
            .iter()
            .position(|s| s.id == id && s.conn == conn)?;
        Some(journal.streams.remove(at).delivered)
    }

    /// How many subscriptions are live — for the tests that pin a subscribe subscribing and a
    /// release releasing.
    #[must_use]
    pub fn stream_count(&self) -> usize {
        self.lock().streams.len()
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
        let (fire, writes) = {
            let mut journal = self.lock();
            let at = revision.current();
            let landed = journal.log.observe(registry, session, at);
            let fire = Self::take_satisfied(&mut journal, landed);
            // Gated on `landed` for `take_satisfied`'s own typing-rate reason, and gated a second
            // time on there BEING a stream: a daemon nobody is following must not walk a vector to
            // learn that, and the overwhelmingly common case is an empty one.
            let writes = if landed == 0 || journal.streams.is_empty() {
                Vec::new()
            } else {
                Self::take_streamable(&mut journal)
            };
            (fire, writes)
        };
        Self::answer(fire);
        Self::write_all(writes);
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
        let (at, fire, writes) = {
            let mut journal = self.lock();
            let at = revision.bump();
            let landed = journal.log.emit(at, events);
            let fire = Self::take_satisfied(&mut journal, landed);
            // Both gates, for [`Self::observe`]'s reasons. This site is not typing-rate, but the
            // empty-stream check is what keeps a subscription's cost proportional to the number of
            // subscribers rather than to the number of announcements.
            let writes = if landed == 0 || journal.streams.is_empty() {
                Vec::new()
            } else {
                Self::take_streamable(&mut journal)
            };
            (at, fire, writes)
        };
        Self::answer(fire);
        Self::write_all(writes);
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
    /// Both registries, because a connection going away ends both kinds of interest and the caller
    /// has no reason to know there are two. A subscription needs no farewell for
    /// [`Self::unsubscribe`]'s reason inverted: there is nobody left to tell, and its `delivered`
    /// count was only ever for a client that asked.
    pub fn release(&self, conn: ConnId) -> usize {
        let mut journal = self.lock();
        let before = journal.parked.len() + journal.streams.len();
        journal.parked.retain(|wait| wait.conn != conn);
        journal.streams.retain(|stream| stream.conn != conn);
        before - (journal.parked.len() + journal.streams.len())
    }

    /// Fire every parked wait with whatever its cursor can see — the session is ENDING.
    ///
    /// The same reasoning [`ChannelRegistry::close`] gives for its bump, applied to this half: a
    /// client parked here for a session that has just been killed is waiting on a journal nothing will
    /// ever append to again. Released, it re-reads, meets the scope refusal, and applies its
    /// detach-on-destroy policy — the path any other client's kill puts it on.
    pub fn drain(&self) {
        let (fire, writes) = {
            let mut journal = self.lock();
            let parked = std::mem::take(&mut journal.parked);
            let fire = parked
                .into_iter()
                .map(|wait| {
                    let batch = journal.log.since(wait.cursor);
                    (wait, batch)
                })
                .collect::<Vec<_>>();
            // The subscriptions get their LAST batch and are then dropped, which is the same
            // reasoning one line up applied to a stream: a follower of a session that has just been
            // killed is following a journal nothing will append to again, and it is entitled to
            // whatever landed before the end. It learns the session is gone the way every other
            // client does — its next request meets the scope refusal.
            let writes = Self::take_streamable(&mut journal);
            journal.streams.clear();
            (fire, writes)
        };
        Self::answer(fire);
        Self::write_all(writes);
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
    ///
    /// ## `landed == 0` returns at once, and that is a TYPING-RATE decision
    ///
    /// Every keystroke is a mutating dispatch ([`crate::events`]), so the derive site runs at typing
    /// rate — and most of those dispatches change nothing structural, appending nothing. Evaluating
    /// the parked waits anyway would call `since(cursor)` once per waiter per keystroke, and that scan
    /// starts at the OLDEST record: a full ring is 256 comparisons a waiter, for an answer that cannot
    /// have changed. R265's rule is that nothing at typing rate may walk.
    ///
    /// It is exact rather than a heuristic: the only two things that can satisfy a parked wait are a
    /// new record and an eviction, and an eviction happens only inside an append
    /// ([`crate::events::EventLog::record`]). No append, no possible satisfaction.
    fn take_satisfied(journal: &mut Journal, landed: usize) -> Vec<(ParkedWait, Batch)> {
        if landed == 0 {
            return Vec::new();
        }
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

    /// Collect one notification per ARMED subscription that has something to say, ADVANCING each
    /// one's cursor as it goes.
    ///
    /// ## The cursor advances here, under the caller's lock, and that is the delivery guarantee
    ///
    /// A subscription is not consumed by firing, so "have I already sent this?" is a question this
    /// type has to answer — and the only place it can answer it exactly is where the append happens.
    /// Advancing under the lock makes a record deliverable **exactly once**: a second append cannot
    /// observe the old cursor, because it cannot take the lock until this pass has released it with
    /// the new one written back.
    ///
    /// Advancing BEFORE the write is deliberate, and it is the safer of the two orders. If the write
    /// fails (the peer went away between the append and the flush) the record is not re-offered — but
    /// a peer that cannot be written to has no reader to re-offer it to, and the subscription is
    /// dropped on the same pass. The other order — write, then advance — would re-send every batch to
    /// a client whose socket buffer was merely full, which is a live client being told the same thing
    /// twice.
    ///
    /// **The frames are built here and written by [`Self::write_all`] with no lock held**, which is
    /// the same split [`Self::answer`] makes and for R291's reason: an egress is opaque, and pinion's
    /// own `send_frame` contract only promises not to block on the CLIENT.
    fn take_streamable(journal: &mut Journal) -> Vec<(Arc<dyn RpcEgress>, String)> {
        // Taken out for `take_satisfied`'s reason: the loop reads `journal.log` while deciding, which
        // it cannot do while holding a mutable borrow of `journal.streams`.
        let streams = std::mem::take(&mut journal.streams);
        let mut writes = Vec::new();
        let mut kept = Vec::with_capacity(streams.len());
        for mut stream in streams {
            if !stream.armed {
                kept.push(stream);
                continue;
            }
            let batch = journal.log.since(stream.cursor);
            if satisfied(&batch, &stream.filter) {
                let batch = filter_batch(&batch, &stream.filter);
                stream.cursor = batch.next;
                stream.delivered += 1;
                writes.push((Arc::clone(&stream.egress), notification(stream.id, &batch)));
            }
            kept.push(stream);
        }
        journal.streams = kept;
        writes
    }

    /// Write every collected notification. **Called with no lock held**, for [`Self::answer`]'s
    /// reason.
    ///
    /// A write that fails is not reported anywhere and not retried: `send_frame` answers `false` for
    /// a peer that is gone, and the connection's own disconnect arm is what drops the subscription
    /// ([`Self::release`]). Pruning here as well would be a second authority on when a stream ends,
    /// racing the first — and the honest one is the transport's, because only it knows whether the
    /// connection is closed or merely slow.
    fn write_all(writes: Vec<(Arc<dyn RpcEgress>, String)>) {
        for (egress, frame) in writes {
            let _ = egress.send_frame(frame);
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
/// The JSON-RPC NOTIFICATION one delivery is written as — no `id`, so a client tells it apart from
/// an answer to something it asked ([`crate::wire::EVENTS_CHANGED_METHOD`] argues why).
///
/// The subscription's id first and the batch's own keys flattened beside it, so a client reading
/// `params` has exactly the shape [`EVENTS_WAIT_METHOD`](crate::wire::EVENTS_WAIT_METHOD) answers
/// with plus the one field that says which stream it belongs to — one batch reader for both.
fn notification(subscription: u64, batch: &Batch) -> String {
    let mut params = batch.to_wire();
    if let Some(map) = params.as_object_mut() {
        map.insert(
            crate::wire::SUBSCRIPTION_PARAM.to_owned(),
            serde_json::json!(subscription),
        );
    }
    serde_json::json!({
        "jsonrpc": "2.0",
        "method": crate::wire::EVENTS_CHANGED_METHOD,
        "params": params,
    })
    .to_string()
}

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

/// The daemon-wide "this session's panes produced output" sink, injected once at boot.
///
/// It exists so [`SessionChannel`]'s observer can hand the work to the dispatch owner without this
/// module knowing what a dispatch owner is: the daemon installs a closure that sends its own
/// ingress event, a test installs one that records. The alternative — this module owning the
/// channel type — would put the transport's vocabulary inside the notification layer, and a GUI
/// embedder that dispatches on its event loop has a different one.
///
/// `OnceLock` rather than a mutex because the read is on the PTY reader thread, once per output
/// batch: install-once is the honest semantics (it is a boot wiring decision, not a setting) AND
/// the only shape whose read is lock-free. A daemon that never installs one simply has no output
/// waits — every park still answers its first evaluation, because the PARK signals through the
/// dispatch path directly rather than through here.
/// What [`OutputSignal`] holds: "tell the dispatch owner that this session moved".
///
/// Named rather than spelled inline because the name is the contract — it takes a SESSION and
/// answers nothing, so there is no result for a caller to wait on and nothing for it to block for.
type OutputSink = Box<dyn Fn(&str) + Send + Sync>;

#[derive(Default)]
pub struct OutputSignal {
    sink: OnceLock<OutputSink>,
}

impl std::fmt::Debug for OutputSignal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The boxed closure is not `Debug`; report whether one is installed, which is the only
        // thing a reader of this type ever wants to know.
        f.debug_struct("OutputSignal")
            .field("installed", &self.sink.get().is_some())
            .finish()
    }
}

impl OutputSignal {
    /// Install the sink. `true` if this call installed it, `false` if one already was — the same
    /// install-once contract pinion's `set_observer` has, and for the same reason: two sinks would
    /// mean one bump evaluating twice.
    pub fn install(&self, sink: impl Fn(&str) + Send + Sync + 'static) -> bool {
        self.sink.set(Box::new(sink)).is_ok()
    }

    /// Tell the dispatch owner that `session` has output to evaluate. **On the PTY reader thread**:
    /// the installed sink must not block, which is why the daemon's is a send on an unbounded
    /// channel and nothing else.
    fn fire(&self, session: &str) {
        if let Some(sink) = self.sink.get() {
            sink(session);
        }
    }
}

/// What a parked output wait is looking for — the SAME two search languages the pane's
/// `find.<needle>` and `regex.<pattern>` slots address, carried as an enum so one cannot silently
/// become the other.
///
/// Two variants rather than a string and a `regex: bool`, for the reason
/// [`crate::wire::REGEX_FIELD`] already gives about those slots: a needle and a pattern are
/// separate languages, so one string must not mean both depending on a mode carried beside it.
#[derive(Debug)]
pub enum OutputQuery {
    /// A literal needle, ASCII-case-folded — `find.<needle>`'s language.
    Literal(String),
    /// A regular expression, case-sensitive (`(?i)` is in the language itself) —
    /// `regex.<pattern>`'s language.
    Pattern(String),
}

/// One client's outstanding question about ONE pane — *wake me when this pane does `Q`*.
#[derive(Debug)]
struct ParkedPaneWait<Q> {
    /// The connection that asked — the key [`PaneWaitChannel::release`] drops by, and the reason a
    /// crashed client leaks nothing.
    conn: ConnId,
    /// The pane that is the subject. Resolved to a live pane at PARK time; a pane that dies while
    /// the wait is parked simply stops producing answers, and the client's own read deadline is
    /// what ends the question — the same lifetime every other park here has.
    pane: PaneId,
    /// What would satisfy it: an [`OutputQuery`] for a `pane/waitForOutput`, a REVISION already
    /// accounted for by a `pane/waitForRevision`.
    question: Q,
    /// The JSON-RPC id to answer under. `None` is a NOTIFICATION — parked and then deliberately not
    /// answered, exactly as [`ParkedWait::id`] documents.
    id: Option<RequestId>,
    /// Where the answer goes. Opaque, so it is NEVER run under the lock below.
    reply: RpcReply,
}

/// One session's parked waits ABOUT ONE PANE, and the lock-free flag that bounds how often they are
/// evaluated.
///
/// ## Why the evaluation is on the DISPATCH OWNER and not on the thread that wakes it
///
/// The wake comes from the revision observer, which runs on the PTY reader thread. Searching a
/// pane's whole retained output there would make the terminal slower *because somebody is waiting on
/// it*. So the observer only ARMS this channel, and the pass runs where every other request runs.
///
/// That thread choice buys a property rather than merely avoiding a cost. [`JournalChannel`] needs
/// one lock spanning decide-and-park because a park on the dispatch thread races an append on
/// another; here the park **and every evaluation** are on the one dispatch owner, so "matched
/// between the check and the park" cannot be expressed. There is no initial-check code path either:
/// a park signals, and the ordinary pass answers it.
///
/// ## The flag is what makes a flooding pane cheap
///
/// `arm` returns `true` only on the false→true edge, so at most ONE evaluation pass per
/// session is ever in flight however fast a pane produces. [`evaluate`](Self::evaluate) clears the
/// flag BEFORE it searches, so output arriving during a pass queues exactly one more pass and never
/// zero — the ordering that makes a missed match unrepresentable.
///
/// `parked_any` is read by the observer to skip arming when nobody is waiting, and it is an ATOMIC
/// rather than `parked.len()` because reading the length would take this type's mutex on the PTY
/// reader thread — the one thing this whole arrangement exists to avoid.
///
/// # ⚠⚠⚠⚠⚠ Why it is ONE type over `Q` and not two types that look alike
///
/// Register item 631 added a second question about a pane — *has it MOVED* — and the mechanism it
/// needs is the mechanism above, to the letter: park on the dispatch owner, arm on the false→true
/// edge, evaluate where every other request runs, clear the flag before the pass, release by
/// connection, drain on a session's end. **None of that is about output.**
///
/// A second copy of it would be a second place for the four properties this doc argues for to be
/// got wrong, and they are the kind that fail silently: an `arm` that forgot the edge floods the
/// dispatch owner, an `evaluate` that cleared its flag after the pass loses a match, a `release`
/// that was never wired leaks a park per crashed client for the daemon's life. So the QUESTION is
/// the parameter and the mechanism is written once.
///
/// ⚠⚠ What is deliberately NOT shared is the ANSWER. `evaluate` takes the reply builder as an
/// argument, because a search answers a [`PaneFind`] and a revision answers a number, and one
/// response shape covering both would be a wire key that means two things.
#[derive(Debug)]
pub struct PaneWaitChannel<Q> {
    parked: Mutex<Vec<ParkedPaneWait<Q>>>,
    /// Whether an evaluation pass is already queued for this session.
    queued: AtomicBool,
    /// Whether anything at all is parked — the observer's lock-free "is this worth a message?".
    parked_any: AtomicBool,
}

/// The waits whose subject is what a pane has SAID — `pane/waitForOutput`.
pub type OutputChannel = PaneWaitChannel<OutputQuery>;

/// The waits whose subject is only that a pane has MOVED — `pane/waitForRevision`, register item
/// 631. The question is the revision the caller has already accounted for.
pub type RevisionChannel = PaneWaitChannel<u64>;

/// Written out rather than derived because `derive(Default)` would demand `Q: Default`, and an
/// [`OutputQuery`] has no default — a channel with nobody waiting holds no question at all.
impl<Q> Default for PaneWaitChannel<Q> {
    fn default() -> Self {
        Self {
            parked: Mutex::new(Vec::new()),
            queued: AtomicBool::new(false),
            parked_any: AtomicBool::new(false),
        }
    }
}

impl<Q> PaneWaitChannel<Q> {
    /// A channel with nobody waiting.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The observer's decision, taken with no lock: does this bump need an evaluation pass?
    ///
    /// `true` exactly on the false→true edge of [`queued`](Self::queued), and only when something is
    /// parked — so a bump on a session nobody waits on costs two atomic loads and sends nothing.
    fn arm(&self) -> bool {
        self.parked_any.load(Ordering::Acquire) && !self.queued.swap(true, Ordering::AcqRel)
    }

    /// Park a question. The caller signals a pass afterwards, which is what answers it the first
    /// time — see this type's docs for why there is deliberately no separate initial check.
    pub fn park(
        &self,
        conn: ConnId,
        pane: PaneId,
        question: Q,
        id: Option<RequestId>,
        reply: RpcReply,
    ) {
        let mut parked = self.lock();
        parked.push(ParkedPaneWait {
            conn,
            pane,
            question,
            id,
            reply,
        });
        // Published while the lock is held, so an observer that sees the flag also sees a non-empty
        // vector by the time its pass takes the lock.
        self.parked_any.store(true, Ordering::Release);
    }

    /// Run `look` for every parked wait and answer the ones that are satisfied, leaving the rest
    /// parked.
    ///
    /// `look` is injected rather than performed here because this module owns PARKING, not panes:
    /// the caller (the dispatch owner) is the one that holds the registry and knows how to read a
    /// pane. It answers `None` for "not yet" and `Some(answer)` for an answer — including a REFUSED
    /// pattern, which is an answer (the caller decides that; see `crate::PaneFind::error`).
    ///
    /// `send` is injected for the reason this type's own doc gives: the mechanism is shared and the
    /// ANSWER SHAPE is not.
    ///
    /// The flag is cleared FIRST, so output that lands while this runs queues another pass.
    /// The replies fire with **no lock held**: a reply sink is opaque, and running one under this
    /// mutex is how a convoy starts.
    pub fn evaluate<A>(
        &self,
        look: impl Fn(PaneId, &Q) -> Option<A>,
        send: impl Fn(RpcReply, Option<&RequestId>, PaneId, &A),
    ) {
        self.queued.store(false, Ordering::Release);
        let fire = {
            let mut parked = self.lock();
            // ⚠ THIS LOCK IS HELD ACROSS `search`, and `search` is not cheap: it walks a pane's whole
            // retained output (338 us at the default scrollback cap) and takes the registry and
            // workspace locks on the way. That is R291's "never hold a lock across expensive work"
            // read literally — and it is sound here for a reason that is structural rather than
            // lucky: EVERY mutator of this vector (`park`, `release`, `drain`, and this pass) runs
            // on the dispatch owner, so there is no second thread to keep waiting. Releasing it
            // around the searches would be worse, not better: a `park` landing in the gap would be
            // clobbered by the write-back below.
            //
            // Taken OUT rather than iterated so `look` is not called while the vector is borrowed
            // mutably.
            let waits = std::mem::take(&mut *parked);
            let mut fire = Vec::new();
            let mut kept = Vec::with_capacity(waits.len());
            for wait in waits {
                match look(wait.pane, &wait.question) {
                    Some(found) => fire.push((wait, found)),
                    None => kept.push(wait),
                }
            }
            *parked = kept;
            self.parked_any.store(!parked.is_empty(), Ordering::Release);
            fire
        };
        // **With no lock held.**
        for (wait, found) in fire {
            send(wait.reply, wait.id.as_ref(), wait.pane, &found);
        }
    }

    /// Drop every wait `conn` parked — it closed, or it crashed. Answers how many went.
    ///
    /// Dropping rather than answering, for the reason [`JournalChannel::release`] gives: a
    /// connection that is gone has nobody to tell.
    pub fn release(&self, conn: ConnId) -> usize {
        let mut parked = self.lock();
        let before = parked.len();
        parked.retain(|wait| wait.conn != conn);
        self.parked_any.store(!parked.is_empty(), Ordering::Release);
        before - parked.len()
    }

    /// Forget every parked wait — the session is ENDING.
    ///
    /// Dropped rather than answered with an empty find: an output wait's answer shape is "here is
    /// the match", and there is no match to report. The client's connection is about to meet the
    /// scope refusal on its next call, which is the path [`ChannelRegistry::close`] puts every other
    /// parked client on.
    pub fn drain(&self) {
        let mut parked = self.lock();
        parked.clear();
        self.parked_any.store(false, Ordering::Release);
    }

    /// How many waits are parked — for the tests that pin a park actually parking, and a release
    /// actually releasing.
    #[must_use]
    pub fn parked_count(&self) -> usize {
        self.lock().len()
    }

    /// The guarded state, recovering a poisoned lock the way the rest of the host does.
    fn lock(&self) -> MutexGuard<'_, Vec<ParkedPaneWait<Q>>> {
        self.parked.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The JSON-RPC success response a matched output wait returns: the pane it was watching and the
/// search answer, serialised from the SAME [`PaneFind`] the `find.<needle>` slot serves.
///
/// Reusing that type is the point rather than a convenience: "wait until it says X" and "does it
/// say X" answer one shape, so a caller can hand either to the same reader and the two cannot drift.
///
/// `None` for an id-less request, for the reason [`send`] gives.
pub(crate) fn send_found(reply: RpcReply, id: Option<&RequestId>, pane: PaneId, found: &PaneFind) {
    if let Some(id) = id {
        reply.send(
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "pane": pane.0, "find": found },
            })
            .to_string(),
        );
    }
}

/// The JSON-RPC success response a released REVISION wait returns: the pane it was watching and
/// that pane's revision as it stands — register item 631.
///
/// ⚠⚠⚠ **THE NUMBER IS READ AT THE PASS, NOT AT THE PARK**, which is what makes a caller's compare
/// sound: the answer is *where the pane is now*, so a client that hands it back as the next
/// `since` cannot be woken twice for one move, and cannot miss a second move that landed while this
/// reply was in flight. Answering the park's own `since + 1` would be a number nothing measured.
///
/// `None` for an id-less request, for the reason [`send`] gives.
pub(crate) fn send_revision(reply: RpcReply, id: Option<&RequestId>, pane: PaneId, revision: &u64) {
    if let Some(id) = id {
        reply.send(
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { PANE_PARAM: pane.0, PANE_REVISION_FIELD: revision },
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
    /// Where a session's "my panes produced output" edge is delivered. Handed to every channel this
    /// registry mints, so a sink installed before the first session is wired into all of them and
    /// no mint site has to remember it.
    signal: Arc<OutputSignal>,
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

    /// `session`'s parked OUTPUT waits — what a `pane/waitForOutput` parks into, and what an
    /// evaluation pass runs over.
    #[must_use]
    pub fn outputs(&self, session: &str) -> Arc<OutputChannel> {
        Arc::clone(&self.entry(session).outputs)
    }

    /// `session`'s parked REVISION waits — what a `pane/waitForRevision` parks into, and what an
    /// evaluation pass runs over. Register item 631.
    #[must_use]
    pub fn revisions(&self, session: &str) -> Arc<RevisionChannel> {
        Arc::clone(&self.entry(session).revisions)
    }

    /// The sink every channel this registry mints signals through.
    ///
    /// Every channel captures THIS `Arc`, so installing into it reaches the channels already minted
    /// as well as the ones to come — the ordering hazard a per-channel sink would have had does not
    /// exist. What [`OutputSignal::install`]'s answer is for is the other half: two sinks would mean
    /// one bump evaluating twice, so the second caller is told it did not win.
    #[must_use]
    pub fn output_signal(&self) -> Arc<OutputSignal> {
        Arc::clone(&self.signal)
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
        let parked: Vec<(
            Arc<JournalChannel>,
            Arc<OutputChannel>,
            Arc<RevisionChannel>,
        )> = self
            .lock()
            .values()
            .map(|channel| {
                (
                    Arc::clone(&channel.journal),
                    Arc::clone(&channel.outputs),
                    Arc::clone(&channel.revisions),
                )
            })
            .collect();
        // The map lock is released before the channels are touched: two locks, never held at once,
        // and always in this order.
        //
        // ALL THREE kinds of park are released here, and an output wait needs it at least as much:
        // its predicate may never match, so an entry the disconnect did not drop would be retained
        // for the daemon's remaining life — [`JournalChannel`]'s own argument, one registry over.
        //
        // ⚠⚠⚠ A REVISION wait needs it MORE THAN EITHER, because a driver holds one open by design:
        // `sprag_plugin::run::park_until` gives up on a slice and comes back to the SAME park, so a
        // run that ends while parked leaves one behind on purpose. It is bounded by one per pane
        // per connection and clears itself the next time that pane moves — but a driver whose
        // process died leaves a pane that may never move again, and only the disconnect can answer
        // for that one.
        parked
            .iter()
            .map(|(journal, outputs, revisions)| {
                journal.release(conn) + outputs.release(conn) + revisions.release(conn)
            })
            .sum()
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
        // The same argument for the OTHER kinds of parked client. Neither a filtered wait nor an
        // output wait is released by the bump — that is the whole point of both — so a session
        // ending has to release them here, or the clients that asked a precise question are the
        // ones left hanging.
        let entry = self.entry(session);
        entry.journal.drain();
        entry.outputs.drain();
        // ⚠ AND THE REVISION WAITS. A pane of a session that has ended will never move again, so a
        // park left here is one no bump can ever reach — the exact state the two drains above
        // exist to prevent, one channel over.
        //
        // ⚠⚠⚠ **IT IS NOT REDUNDANT WITH THE `remove` BELOW, AND A GREEN MUTATION IS WHAT SAID SO.**
        // Removing the map entry frees the parked waits only if nothing else holds the channel —
        // and [`revisions`](Self::revisions) hands out `Arc` CLONES, so a dispatch owner mid-pass
        // holds one. A `close` that only removed would leave that holder's copy carrying waits in a
        // channel no bump can reach, for as long as the holder lives. The gate had to be re-aimed
        // at a clone taken BEFORE the close to see the difference at all.
        entry.revisions.drain();
        self.lock().remove(session);
    }

    /// Carry `from`'s channel over to `to` — what a session RENAME does to the wake machinery.
    ///
    /// # Why the channel MOVES rather than being re-minted
    ///
    /// Everything a client is currently waiting on lives in it: the scene-revision TOKEN (which
    /// every pane of the session captured at spawn and bumps from its reader thread — nothing can
    /// re-point those), the change JOURNAL with its cursor vocabulary and its established SHAPE, the
    /// parked `scene/waitFor` replies, the filtered `events/waitFor` waits and their subscriptions,
    /// and the parked output waits. A rename that minted a fresh channel would leave every one of
    /// them parked on a key nothing reaches again — the session would be alive, its panes would be
    /// producing output, and its clients would hear nothing for as long as they lived.
    ///
    /// So the rename is deliberately NOT a close plus a create, which is the same sentence
    /// [`crate::events`] arrived at one layer up for the same reason.
    ///
    /// A stale channel already sitting on `to` (a name whose session is gone) is CLOSED first
    /// rather than dropped, on [`close`](Self::close)'s own argument: dropping it would silently
    /// take its parked replies with it.
    pub fn rename(&self, from: &str, to: &str) {
        if from == to {
            return;
        }
        if self.lock().contains_key(to) {
            self.close(to);
        }
        let mut channels = self.lock();
        if let Some(channel) = channels.remove(from) {
            *channel
                .address
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = to.to_owned();
            channels.insert(to.to_owned(), channel);
        }
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
        let signal = &self.signal;
        let mut channels = self.lock();
        let channel = channels
            .entry(session.to_owned())
            .or_insert_with(|| SessionChannel::new(session, signal));
        SessionChannel {
            revision: Arc::clone(&channel.revision),
            waiters: Arc::clone(&channel.waiters),
            journal: Arc::clone(&channel.journal),
            outputs: Arc::clone(&channel.outputs),
            revisions: Arc::clone(&channel.revisions),
            address: Arc::clone(&channel.address),
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
        // quiet pane returns none. REPRODUCIBLE since R320: `sprag-latency`'s poll-pair row
        // measured 17 152 returns a second on another box, against 542 for the same loop fed the
        // revision it actually waits on. A wait parked HERE sleeps through all of it, because this
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

    /// Subscribe on `session` from cursor 0, ARM it, and answer the notifications it writes.
    ///
    /// Armed here rather than left to the caller because a disarmed subscription is deliberately
    /// silent, so a test that forgot would measure nothing and read as a stream that never fired —
    /// the same trap [`park`]'s own doc records about a wait with no id.
    fn subscribe(
        channels: &ChannelRegistry,
        session: &str,
        filter: EventFilter,
    ) -> Arc<std::sync::Mutex<Vec<String>>> {
        let written = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Arc::clone(&written);
        let journal = channels.journal(session);
        let id = journal.subscribe(
            ConnId::allocate(),
            0,
            filter,
            pinion_rpc::FnEgress::new(move |frame: String| {
                sink.lock().expect("the frame sink").push(frame);
                true
            }),
        );
        journal.arm(id);
        written
    }

    /// **THE SECOND DERIVE SITE.** `announce` pushes to a subscription, and a test that only drove
    /// the OTHER site would not say so.
    ///
    /// ⚠ This test exists because a revert-proof PASSED: deleting the streaming call from `announce`
    /// left `rpc`'s own subscription test green, because a spawn reaches a subscriber through
    /// `observe` (the shape diff) and never through here. `announce` is the sweeper's path — a job
    /// change is recorded when nobody performed anything — so a subscription following
    /// `pane_job_changed` would have been silent for ever with nothing failing.
    ///
    /// **Fifth round running that the largest gap in a round's own tests was found by breaking the
    /// code rather than by reading it.**
    ///
    /// REVERT-PROOF: drop `take_streamable` from `announce` and this fails while `rpc`'s test passes,
    /// which is the whole reason both exist.
    #[test]
    fn the_announce_site_writes_to_a_subscription_too() {
        let channels = ChannelRegistry::default();
        let written = subscribe(&channels, "work", EventFilter::Everything);

        channels.announce("work", vec![Event::PaneJobChanged(2)]);

        let frames = written.lock().expect("the frame sink");
        assert_eq!(
            frames.len(),
            1,
            "the sweeper's own derive site reaches a follower: {frames:?}",
        );
        let frame: serde_json::Value =
            serde_json::from_str(&frames[0]).expect("a written frame is JSON-RPC");
        assert_eq!(frame["method"], crate::wire::EVENTS_CHANGED_METHOD);
        assert!(frame.get("id").is_none(), "a notification carries no id");
        assert_eq!(
            frame["params"]["events"],
            serde_json::json!([{ "type": "pane_job_changed", "pane": 2 }]),
        );
    }

    /// A subscription is SILENT until it is armed, and then catches up.
    ///
    /// The window is sub-microsecond in the daemon and the hazard is exact: a change landing between
    /// the register and the opening response would name a subscription id the client has not read,
    /// which it can only discard — so the record would be lost with the cursor already past it.
    ///
    /// REVERT-PROOF: register armed (`armed: true`) and the first assertion fails; drop the delivery
    /// from `arm` and the second does, because the change made during the window is never told.
    #[test]
    fn a_subscription_says_nothing_before_it_is_armed_and_then_catches_up() {
        let channels = ChannelRegistry::default();
        let written = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Arc::clone(&written);
        let journal = channels.journal("work");
        let id = journal.subscribe(
            ConnId::allocate(),
            0,
            EventFilter::Everything,
            pinion_rpc::FnEgress::new(move |frame: String| {
                sink.lock().expect("the frame sink").push(frame);
                true
            }),
        );

        // The window: a change between the register and the response.
        channels.announce("work", vec![Event::PaneJobChanged(2)]);
        assert!(
            written.lock().expect("the frame sink").is_empty(),
            "a client that has not been told its subscription id must not be written to",
        );

        journal.arm(id);
        let frames = written.lock().expect("the frame sink");
        assert_eq!(
            frames.len(),
            1,
            "and arming DELIVERS what landed in the window rather than losing it: {frames:?}",
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

    /// A RENAME carries the channel over: the revision token every pane already holds, the
    /// journal's contents, and the waits parked on it. A rename that minted a fresh channel would
    /// leave a live session's clients parked on a key nothing reaches again.
    #[test]
    fn a_rename_carries_the_channel_its_journal_and_its_parked_waits() {
        let channels = ChannelRegistry::default();
        // Something to lose: a record in the journal, and a client parked on the revision.
        let revision = channels.revision("work");
        let announced = channels.announce("work", vec![Event::LayoutUpdated]);
        // Parked at the revision the announce LEFT, so it is genuinely waiting rather than
        // answered on the spot by a change it has already accounted for.
        let replies = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Arc::clone(&replies);
        channels.waiters("work").park_if_current(
            &revision,
            announced,
            Some(pinion_rpc::RequestId::Num(1)),
            pinion_rpc::RpcReply::new(move |reply| {
                sink.lock().expect("the reply sink").push(reply);
            }),
        );
        assert_eq!(answered(&replies), 0, "parked, not answered");

        channels.rename("work", "prod");

        assert_eq!(
            channels.len(),
            1,
            "one channel MOVED — not one closed and another minted",
        );
        assert!(
            Arc::ptr_eq(&revision, &channels.revision("prod")),
            "the SAME revision token: every pane of this session captured it at spawn and no              rename can re-point those",
        );
        assert_eq!(
            channels.journal("prod").since(0).events.len(),
            1,
            "the journal came with it — a client resuming at its cursor is not sent back to a              full re-read",
        );
        assert_eq!(
            channels.waiters("prod").parked_count(),
            1,
            "and so did the client parked on it, still waiting rather than dropped",
        );
        // It answers to the new name and only to that: the old one mints a fresh, empty channel.
        assert_eq!(channels.journal("work").since(0).events.len(), 0);
    }

    /// The rename also moves the address the wake OBSERVER fires under. The observer used to
    /// CAPTURE the session name at mint time, so after a rename every batch of output on a live
    /// session signalled a name nothing answered to — and a `pane/waitForOutput` parked on it would
    /// never be evaluated.
    #[test]
    fn output_after_a_rename_signals_the_new_address() {
        let channels = ChannelRegistry::default();
        let fired = record_signal(&channels);
        // The signal only fires when something is WAITING for output — that is `arm`'s whole job.
        let _parked = park_output(&channels, "work", ConnId::allocate());
        let revision = channels.revision("work");

        revision.bump();
        assert_eq!(
            fired.lock().unwrap().as_slice(),
            ["work"],
            "control: before the rename it fires under the name it was minted with",
        );

        channels.rename("work", "prod");
        // Clear the queued latch the way a real pass does, then produce again on the pane's OWN
        // token — captured at spawn, and the point is that nothing anywhere re-points it.
        channels
            .outputs("prod")
            .evaluate(|_, _| None::<crate::PaneFind>, send_found);
        revision.bump();

        assert_eq!(
            fired.lock().unwrap().as_slice(),
            ["work", "prod"],
            "the SAME token now signals the session's CURRENT address",
        );
    }

    #[test]
    fn a_filtered_wait_sleeps_through_another_sessions_identical_change() {
        // The scope half, which nothing else asserted — and this is the property R279 caught
        // `scene/waitFor` checking and then IGNORING for as long as the daemon had one registry-wide
        // revision. A filtered wait parks on the SCOPED session's channel, so a change in another
        // session cannot reach it even when it is the SAME kind about the SAME pane id: pane ids are
        // registry-unique today, and a wait must not depend on that staying true.
        let channels = ChannelRegistry::default();
        let mine = park_filtered(&channels, "play", ConnId::allocate(), job_of(2));

        channels.announce("work", vec![Event::PaneJobChanged(2)]);
        assert_eq!(
            answered(&mine),
            0,
            "an identical change in another session is not this waiter's business",
        );
        assert_eq!(
            channels.journal("play").parked_count(),
            1,
            "and it is still asleep, not woken-and-re-parked",
        );

        channels.announce("play", vec![Event::PaneJobChanged(2)]);
        assert_eq!(answered(&mine), 1, "its own session's change reaches it");
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

    /// Park an OUTPUT wait on `session`'s pane 0, answering the replies it fires.
    fn park_output(
        channels: &ChannelRegistry,
        session: &str,
        conn: ConnId,
    ) -> Arc<std::sync::Mutex<Vec<String>>> {
        let replies = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Arc::clone(&replies);
        channels.outputs(session).park(
            conn,
            PaneId(0),
            OutputQuery::Literal("done".to_owned()),
            Some(RequestId::Num(1)),
            RpcReply::new(move |reply| {
                sink.lock().expect("the reply sink").push(reply);
            }),
        );
        replies
    }

    /// [`park_output`]'s sibling for a REVISION wait — register item 631.
    fn park_revision(
        channels: &ChannelRegistry,
        session: &str,
        conn: ConnId,
    ) -> Arc<std::sync::Mutex<Vec<String>>> {
        let replies = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Arc::clone(&replies);
        channels.revisions(session).park(
            conn,
            PaneId(0),
            7,
            Some(RequestId::Num(1)),
            RpcReply::new(move |reply| {
                sink.lock().expect("the reply sink").push(reply);
            }),
        );
        replies
    }

    /// ⚠⚠⚠⚠ **A REVISION WAIT SURVIVES A RENAME, IS RELEASED BY A DISCONNECT, AND IS DRAINED BY A
    /// KILL** — the three lifecycle facts the OUTPUT channel already had gates for, asked of the
    /// channel register item 631 added.
    ///
    /// Each is carried by construction — `rename` moves the whole [`SessionChannel`], `release` and
    /// `close` walk all three channels — and *by construction* is exactly the claim that rots when
    /// somebody adds a fourth channel and wires two of the three. **The output channel's own gates
    /// exist because this went wrong for it**; a new channel with no gates is the same bet taken
    /// again.
    ///
    /// ⚠ A REVISION wait is the one that needs the disconnect most: a driver holds one open BY
    /// DESIGN across the slices of `park_until`, so a run whose process died leaves one behind on a
    /// pane that may never move again.
    #[test]
    fn a_revision_wait_survives_a_rename_and_is_released_by_a_disconnect() {
        let channels = ChannelRegistry::default();
        let conn = ConnId::allocate();
        let replies = park_revision(&channels, "work", conn);
        assert_eq!(channels.revisions("work").parked_count(), 1, "parked");

        channels.rename("work", "prod");
        assert_eq!(
            channels.revisions("prod").parked_count(),
            1,
            "the rename CARRIED it — a wait left on the old key is one no bump can ever reach, \
             because the panes bump the token that moved with the channel",
        );
        assert_eq!(
            channels.revisions("work").parked_count(),
            0,
            "and the retired name mints a fresh, empty channel",
        );
        assert_eq!(answered(&replies), 0, "carried, not answered");

        assert_eq!(
            channels.release(conn),
            1,
            "the disconnect released it, and COUNTED it — a registry that walked only two of its \
             three channels would answer 0 here and leak the entry for the daemon's life",
        );
        assert_eq!(channels.revisions("prod").parked_count(), 0);
        assert_eq!(
            answered(&replies),
            0,
            "a gone connection is not written to: the release drops, it does not answer",
        );
    }

    /// ...and a session ENDING **drains** it, on [`ChannelRegistry::close`]'s own argument: a pane
    /// of a dead session will never move again, so a park left here is one no bump can reach.
    ///
    /// # ⛔⛔⛔ THE FIRST VERSION OF THIS GATE WAS BLIND, AND A GREEN MUTATION IS WHAT SAID SO
    ///
    /// It asked `channels.revisions("work").parked_count()` AFTER the close — and `close` REMOVES
    /// the map entry, so that lookup **mints a fresh empty channel**. The answer was 0 whether or
    /// not the drain ran, and the mutation that deleted the drain outright stayed green. The
    /// comment written beside it even claimed the two readings were "the same fact"; they are not.
    ///
    /// ⚠⚠ **The clone taken BEFORE the close is what discriminates**, and it is not a contrivance:
    /// [`ChannelRegistry::revisions`] hands out `Arc` clones by design, so a dispatch owner mid-pass
    /// is holding exactly this. Under a `close` that only removed, that holder's copy would keep
    /// its waits parked in a channel nothing can ever bump again.
    #[test]
    fn a_revision_wait_is_drained_when_its_session_ends() {
        let channels = ChannelRegistry::default();
        let replies = park_revision(&channels, "work", ConnId::allocate());
        // HELD ACROSS THE CLOSE, which is the whole discrimination — see this test's own doc.
        let held = channels.revisions("work");
        assert_eq!(held.parked_count(), 1, "parked");

        channels.close("work");

        assert_eq!(
            held.parked_count(),
            0,
            "the channel a holder still has must be DRAINED, not merely unreachable: removing the \
             map entry frees nothing while somebody holds a clone",
        );
        assert_eq!(
            answered(&replies),
            0,
            "drained rather than answered: a revision wait's answer shape is a NUMBER, and there \
             is no number to report about a session that has gone",
        );
        // ⚠ AND THE FRESH LOOKUP IS ALSO EMPTY — kept as the weaker half rather than deleted,
        // because it is what a CLIENT observes, and a repair that drained the holder's copy while
        // leaving the name resolving to the old channel would pass the assertion above alone.
        assert_eq!(channels.revisions("work").parked_count(), 0);
    }

    /// A recording output sink installed on `channels` — every session it was fired for, in order.
    fn record_signal(channels: &ChannelRegistry) -> Arc<std::sync::Mutex<Vec<String>>> {
        let fired = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Arc::clone(&fired);
        assert!(
            channels.output_signal().install(move |session| sink
                .lock()
                .expect("the signal sink")
                .push(session.to_owned())),
            "the first install wins",
        );
        fired
    }

    #[test]
    fn a_bump_signals_nothing_when_no_output_wait_is_parked() {
        // The typing-rate decision, in the same shape `take_satisfied`'s early return has: a pane's
        // output bumps this token constantly, and a session nobody is waiting on must not put a
        // message on the dispatch queue for every batch.
        let channels = ChannelRegistry::default();
        let fired = record_signal(&channels);

        for _ in 0..20 {
            channels.revision("work").bump();
        }

        assert!(
            fired.lock().unwrap().is_empty(),
            "twenty bumps on a session with no output wait signalled nothing: {fired:?}",
        );
    }

    #[test]
    fn a_flood_of_output_signals_one_pass_and_the_pass_re_arms_it() {
        // THE claim that bounds this feature's cost. herdr's rival surface pays a full pane read per
        // waiter every 100 ms whatever the pane does; here a burst of output costs ONE evaluation in
        // flight per session, and the next burst costs one more only after the pass has run.
        let channels = ChannelRegistry::default();
        let fired = record_signal(&channels);
        let _parked = park_output(&channels, "work", ConnId::allocate());

        for _ in 0..20 {
            channels.revision("work").bump();
        }
        assert_eq!(
            fired.lock().unwrap().as_slice(),
            ["work"],
            "twenty bumps, ONE pass queued — the armed flag is what a flooding pane meets",
        );

        // The pass clears the flag BEFORE it searches, so the next output arms it again. A pass that
        // cleared it afterwards would swallow everything that landed while it ran.
        channels
            .outputs("work")
            .evaluate(|_, _| None::<crate::PaneFind>, send_found);
        channels.revision("work").bump();
        assert_eq!(
            fired.lock().unwrap().as_slice(),
            ["work", "work"],
            "and output after a pass queues the next one",
        );
    }

    #[test]
    fn output_that_lands_during_a_pass_queues_the_next_pass() {
        // ⚠ THE ORDERING CLAIM, and it needed a test built for it: the two tests above pass whether
        // the armed flag is cleared before or after the search, because they never overlap the two.
        // The hazard only exists when a pane produces WHILE a pass is running — clearing the flag
        // afterwards would swallow that output, and the wait would sleep through a match that had
        // already landed with nothing left to wake it.
        //
        // So the search itself bumps the revision, which is exactly what a pane doing work looks
        // like from here.
        let channels = ChannelRegistry::default();
        let fired = record_signal(&channels);
        let _parked = park_output(&channels, "work", ConnId::allocate());

        channels.revision("work").bump();
        assert_eq!(fired.lock().unwrap().len(), 1, "the first pass is queued");

        let revision = channels.revision("work");
        channels.outputs("work").evaluate(
            |_, _| {
                revision.bump();
                None::<crate::PaneFind>
            },
            send_found,
        );

        assert_eq!(
            fired.lock().unwrap().as_slice(),
            ["work", "work"],
            "output during the pass queued the NEXT pass — cleared before the search, never after",
        );
    }

    #[test]
    fn an_output_wait_is_answered_by_the_pass_and_only_by_a_match() {
        // The predicate is the whole difference between this and `scene/waitFor`: a pass that finds
        // nothing leaves the wait parked, and the caller is not woken to look for itself.
        let channels = ChannelRegistry::default();
        let replies = park_output(&channels, "work", ConnId::allocate());

        channels
            .outputs("work")
            .evaluate(|_, _| None::<crate::PaneFind>, send_found);
        assert_eq!(answered(&replies), 0, "no match, no answer");
        assert_eq!(channels.outputs("work").parked_count(), 1, "still parked");

        channels
            .outputs("work")
            .evaluate(|_, _| Some(PaneFind::default()), send_found);
        assert_eq!(answered(&replies), 1, "a match answers it");
        assert_eq!(
            channels.outputs("work").parked_count(),
            0,
            "and an answered wait is not also a waiter",
        );
    }

    #[test]
    fn a_closed_connection_takes_its_output_wait_with_it() {
        // An output wait needs this MORE than a filtered one: its predicate may never match, so an
        // entry the disconnect did not drop is retained for the daemon's remaining life.
        let channels = ChannelRegistry::default();
        let gone = ConnId::allocate();
        let staying = ConnId::allocate();
        let _left = park_output(&channels, "work", gone);
        let _kept = park_output(&channels, "work", staying);

        assert_eq!(channels.release(gone), 1, "one wait released");
        assert_eq!(
            channels.outputs("work").parked_count(),
            1,
            "and the OTHER connection's wait is untouched — release is keyed by connection",
        );
    }

    #[test]
    fn closing_a_session_forgets_its_output_waits() {
        // Dropped rather than answered with an empty find: an output wait's answer shape is "here is
        // the match", and there is no match to report. The client meets the scope refusal next call.
        let channels = ChannelRegistry::default();
        let replies = park_output(&channels, "doomed", ConnId::allocate());

        channels.close("doomed");

        assert_eq!(answered(&replies), 0, "no invented answer");
        assert!(channels.is_empty(), "and the channel forgotten");
    }
}
