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
use std::sync::{Arc, Mutex, PoisonError};

use pinion_core::SceneRevision;
use pinion_rpc::WaiterRegistry;

use crate::events::SessionJournal;

/// One session's change-notification channel: the scene-version token its changes advance, and the
/// parked `scene/waitFor` replies waiting on it. The token's observer is the ONLY thing that fires
/// those replies, and it is installed when the channel is minted — so no announce site has to
/// remember the wake half.
struct SessionChannel {
    revision: Arc<SceneRevision>,
    waiters: Arc<WaiterRegistry>,
    /// What has CHANGED in this session, keyed by the very token above — the payload half of a
    /// wake. It lives here, beside the revision, because the revision is its cursor vocabulary: a
    /// journal kept anywhere else would be keyed by a number this map owns, which is the fork
    /// [`crate::events`] refuses.
    journal: Arc<Mutex<SessionJournal>>,
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
            journal: Arc::new(Mutex::new(SessionJournal::new())),
        }
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

    /// `session`'s change journal — the payload a wake on this channel is about.
    #[must_use]
    pub fn journal(&self, session: &str) -> Arc<Mutex<SessionJournal>> {
        Arc::clone(&self.entry(session).journal)
    }

    /// Record what moved in `session` since the last observation, at the revision it is now at.
    ///
    /// Reads the revision through this same channel rather than taking it as an argument: the
    /// number a record is keyed by and the number a parked waiter is released with must be the one
    /// token, and a caller free to pass a different one is a caller who eventually will.
    pub fn observe(&self, registry: &sprag_terminal::SessionRegistry, session: &str) {
        let entry = self.entry(session);
        let revision = entry.revision.current();
        entry
            .journal
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .observe(registry, session, revision);
    }

    /// Announce a change in `session`, answering the revision it advanced to.
    ///
    /// The convenience form of `revision(session).bump()`, for the announce sites that hold a name
    /// rather than a token.
    pub fn bump(&self, session: &str) -> u64 {
        self.revision(session).bump()
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
}
