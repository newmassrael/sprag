//! Per-client session attachment tracking — the daemon's answer to "who is attached to which
//! session" (R-PR67 Stage 1, the tmux `list-clients` / cmux "N viewing this workspace" root).
//!
//! ## Why the daemon holds this, when it deliberately holds no per-request scope state
//!
//! A request's SCOPE (which session it acts on) stays CLIENT-carried, per-request
//! ([`crate::wire::SESSION_PARAM`]) — the host stays free of that bookkeeping on purpose.
//! ATTACHMENT is a DIFFERENT fact: which session a client is currently VIEWING, independent of
//! what any one request targets (tmux keeps the two apart too — a command may `-t` any session
//! regardless of the client's attached one). "How many clients view session S" cannot be answered
//! without per-client state that outlives a single request, so this registry is the one, scoped
//! exception, and it never touches request routing.
//!
//! ## Connection vs. client — why both layers
//!
//! pinion gives a per-CONNECTION [`ConnId`] with a crash-safe close signal (`on_disconnect`). A
//! logical client (one `sprag-gui` window) opens SEVERAL connections (a request stream and a
//! long-poll) to avoid head-of-line blocking, so counting connections would count one window
//! twice. Each connection announces its client id ([`crate::wire::CLIENT_HELLO_METHOD`]); the
//! registry maps `conn -> client`, attaches per CLIENT, and releases a client only when its LAST
//! connection closes. That is the faithful mapping of sprag's two-connection client onto tmux's
//! one `struct client`: the connections are transport, the client is the unit that attaches.
//!
//! ## Crash-safety
//!
//! Attachment is dropped by connection CLOSE ([`disconnect`](AttachmentRegistry::disconnect)),
//! never by an explicit "detach" message, so however a client dies — clean exit, reset, crash —
//! its connections' readers end and the count falls. An explicit detach message would leak a
//! session as "attached" forever when the sender crashes before sending it; the close signal
//! cannot be skipped. All mutation runs on the single dispatch thread (the close signal is routed
//! onto the same FIFO as frames), so no lock orders against pinion's transport threads.

use pinion_rpc::ConnId;
use serde::{Deserialize, Serialize};
use sprag_terminal::{SessionId, WindowId};
use std::collections::HashMap;

use crate::report::Announcement;

/// An opaque, client-minted lifecycle token shared by every connection of one logical client.
/// Not identity (says nothing about who the peer is), only "these connections are one client".
pub type ClientId = String;

/// The cell area one client can give a window, as that client REPORTED it
/// ([`crate::wire::CLIENT_SIZE_METHOD`]).
///
/// Reported rather than measured, because the daemon owns neither surface: a `sprag-tui`'s area is
/// its terminal's winsize, a `sprag-gui`'s is its window's pixels divided by its font metric, and
/// the only process that can turn either into cells is the one holding it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ClientSize {
    /// How many columns the client has.
    pub cols: u16,
    /// How many rows the client has.
    pub rows: u16,
}

/// One attached client, for the `clients` wire slot (tmux `list-clients`): the opaque client id,
/// the session it is currently viewing, and the cell area it reported.
///
/// The daemon's honest analog of tmux's `struct client` row. The size used to be absent with a
/// reason attached — "a `sprag-gui` window is not a terminal the daemon owns" — and that reason
/// survives, which is why the field is a REPORT rather than a measurement: the daemon still owns no
/// tty, it is now TOLD. `None` is a real state (a client that attached before reporting), not a
/// placeholder, so a reader never mistakes an unreported size for a zero-cell one.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ClientInfo {
    /// The opaque, client-minted id (e.g. a `sprag-gui` window's `gui-{pid}-{nanos}`).
    pub client: ClientId,
    /// The session this client is attached to (tmux `client -> session`).
    pub session: String,
    /// The cell area it reported, or `None` if it has not reported one yet.
    #[serde(default)]
    pub size: Option<ClientSize>,
    /// The NAME of the window this client is looking at, or `None` off a listing that could not
    /// resolve one (a client whose window has just gone).
    ///
    /// # Why a listing of clients owes this
    ///
    /// It did not, while every client of a session saw the same window: the session answered it and
    /// the row would have repeated itself. R346 made a view a fact about the CLIENT, and the first
    /// question a person asks when their panes are the wrong size is *who else is on this window* —
    /// which this listing was the natural place to answer and could not.
    ///
    /// A NAME and not an id, because this is a surface a person reads and an id appears on none of
    /// them; resolved by the caller, which is the only side that holds the registry.
    #[serde(default)]
    pub window: Option<String>,
    /// **WHICH BUILD THIS CLIENT SAID IT IS** ([`sprag_rpc::CLIENT_BUILD_PARAM`]), or `None` from
    /// one that did not say.
    ///
    /// # ⚠⚠⚠⚠⚠ `None` is *"it did not say"*, and a reader that renders it as agreement breaks the
    /// key's licence
    ///
    /// The daemon is the only party holding every window's answer AND its own, so this is the row
    /// that lets a person ask *is the window I am looking at running this daemon's code*. Register
    /// item 463: a `sprag-gui` is started by hand from wherever somebody points, and this
    /// repository's own promotion copies the daemon to one directory while the GUI is run out of
    /// `target/debug` — so the skew is ordinary, not exotic.
    ///
    /// Additive, like the two fields above: a reader predating it sees no key, which is exactly the
    /// answer it would get from a daemon that has one and a client that stayed quiet.
    #[serde(default)]
    pub build: Option<String>,
}

/// What an [`attach`](AttachmentRegistry::attach) did, so the caller knows whether the per-session
/// counts moved (and the scene must be bumped so other clients' long-polls re-read the badge).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum AttachOutcome {
    /// The connection never sent [`crate::wire::CLIENT_HELLO_METHOD`] — a protocol error; the
    /// caller refuses the request rather than inventing a client.
    NoClient,
    /// The client's attached session changed (a first attach, or a switch): the counts moved.
    ///
    /// `previous` names the session it LEFT, `None` on a first attach. Carried rather than
    /// discarded because a switch moves TWO badges, and each session announces its own changes: a
    /// client watching the session being left has to be told its viewer count fell, and only this
    /// value can say which session that was.
    Changed { previous: Option<String> },
    /// The client was already attached to that session: an idempotent re-send, counts unmoved.
    Unchanged,
}

/// What a [`size`](AttachmentRegistry::size) report did, so the caller knows whether the session's
/// arbitrated window could have moved (and the scene must be bumped so every other client's
/// long-poll re-reads it).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SizeOutcome {
    /// The connection never sent [`crate::wire::CLIENT_HELLO_METHOD`] — a protocol error; the
    /// caller refuses the request rather than inventing a client.
    NoClient,
    /// The client's area is new or different: the arbitration's inputs moved.
    Changed,
    /// The client re-reported the size it already had: nothing to announce.
    Unchanged,
}

/// Record `id` as the session this client viewed MOST recently: drop any entry it already has, then
/// push to the front.
///
/// Most-recent-first and deduplicated, so the head is always where the client is now and the next
/// live entry is where it would go back to. The dedup is also the bound: at most one entry per
/// session this client has ever visited.
fn push_visit(history: &mut Vec<SessionId>, id: SessionId) {
    history.retain(|visited| *visited != id);
    history.insert(0, id);
}

/// One client's reported area together with WHEN it was reported, relative to every other report.
///
/// The ordinal exists for one policy: `window-size latest` has to name the most recent client, and
/// "most recent" is not a property of a `HashMap`. It counts reports and attachments rather than
/// keystrokes, which is a deliberate narrowing of tmux's "most recently USED client" — a keystroke
/// arrives on a pane's input path carrying no client identity, so tracking it would mean threading
/// a client id through the whole input wire to serve one option. Attach-or-resize is what this
/// daemon can observe honestly, and it is also what a user changing their window means by "latest".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Reported {
    /// The area the client last reported.
    size: ClientSize,
    /// A monotone stamp: higher is more recent.
    ordinal: u64,
}

/// The daemon's `conn -> client -> attached session` map, and each client's reported area (see the
/// module docs).
#[derive(Default, Debug)]
pub struct AttachmentRegistry {
    /// Which client each LIVE connection belongs to (from `client/hello`). A connection is in
    /// here from its hello until its close; a client is PRESENT while any of its connections is.
    conn_client: HashMap<ConnId, ClientId>,
    /// Which session each present, ATTACHED client is viewing (from `client/attach`). A client
    /// appears only after it attaches; every entry is a present client (removed when its last
    /// connection closes), so the values ARE the live attachments.
    client_session: HashMap<ClientId, String>,
    /// Which sessions each present client HAS VIEWED, most-recent-first and deduplicated — its
    /// visit history, recorded by [`attach`](Self::attach) beside the attachment above.
    ///
    /// # Why this one is keyed by IDENTITY when the attachment beside it is keyed by name
    ///
    /// The attachment is a fact about the PRESENT, and a fact about the present can be kept true by
    /// a hook where the change is published: [`rename_session`](Self::rename_session) moves it and
    /// [`session_ended`](Self::session_ended) ends it, both inside the dispatch that does the thing.
    ///
    /// A history is a fact about the PAST, and no hook can keep one true, because its subject may
    /// no longer exist to be updated. A remembered NAME is then the worst of both: after a rename
    /// it resolves to nothing (the visit is silently lost) and, once a new session takes the freed
    /// name, to A STRANGER. R304 measured a real client walking through that second door — asked to
    /// go back where it was, it attached to a session it had never seen.
    ///
    /// A [`SessionId`] cannot: it is minted once per session per run of the daemon and never
    /// reissued, so an id that resolves IS that session under whatever it is called now, and one
    /// that does not is a session that is gone. Nothing has to maintain it.
    ///
    /// Bounded by the number of distinct sessions a client has visited (the dedup keeps at most one
    /// entry per session), and shrunk further by [`last_viewed`](Self::last_viewed), which drops
    /// entries that no longer resolve as it walks.
    client_history: HashMap<ClientId, Vec<SessionId>>,
    /// Which WINDOW of its attached session each client is LOOKING AT.
    ///
    /// # The half a session used to hold for everybody
    ///
    /// `Session::current_window` was the one answer to "what is on screen", so two clients of one
    /// session could not look at different things — and, because the size arbitration folds every
    /// client attached to the SESSION, a phone attaching beside a desktop resized the desktop's
    /// panes to the phone. R346 split the question: a session's current window is now where a client
    /// LANDS when it attaches, and this is where each client went afterwards.
    ///
    /// # Keyed by IDENTITY, like the history and unlike the attachment
    ///
    /// A window NAME is an address a person types and `rename-window` moves. What this answers is
    /// *which clients does this window's size come from*, and a retired-then-reissued name would
    /// fold in a client that is looking at something else — the same capture
    /// [`client_history`](Self::client_history) records in full, one level down. A [`WindowId`] is
    /// minted once per window per run and never reissued, so it cannot.
    ///
    /// Every entry is a present, ATTACHED client: written by [`attach`](Self::attach) (which lands
    /// it on the session's current window) and by [`watch`](Self::watch) (which is that client's own
    /// `select-window`), and dropped with the client in [`disconnect`](Self::disconnect) and with
    /// its session in [`session_ended`](Self::session_ended).
    client_window: HashMap<ClientId, WindowId>,
    /// Each present client's reported area (from `client/size`), stamped with its recency.
    ///
    /// Keyed by CLIENT rather than by connection because a client's several connections describe
    /// one surface. Independent of `client_session`: a client may report a size before it attaches
    /// (which is the order both frontends use, so the first arbitration already counts it) and an
    /// attached client may never report one.
    client_size: HashMap<ClientId, Reported>,
    /// The one message waiting for each present client, put there by
    /// [`deliver`](AttachmentRegistry::deliver) and taken by [`collect`](AttachmentRegistry::collect).
    ///
    /// Keyed by CLIENT and not by connection, for the reason the size beside it is: a client's
    /// several connections are one surface with one status row, and a message queued per connection
    /// would be shown twice or — worse — once on whichever connection happened to ask first.
    ///
    /// Bounded by construction: one entry per present client, one message per entry. It is dropped
    /// with the client in [`disconnect`](AttachmentRegistry::disconnect), so a message nobody
    /// collected cannot outlive the client it was addressed to and become a sentence a LATER client
    /// with the same id is shown — the capture the history beside it already refuses.
    client_mail: HashMap<ClientId, Announcement>,
    /// **WHICH BUILD each present client SAID IT IS** ([`sprag_rpc::CLIENT_BUILD_PARAM`], stated at
    /// `client/hello`), or no entry at all for one that did not say.
    ///
    /// # ⚠⚠⚠⚠ An absent entry is *"it did not say"* and never *"it matches"*
    ///
    /// This registry only RECORDS; the comparison and its four answers live in
    /// [`crate::wire::reporter_image`], because a client that stated nothing and a client that
    /// stated this daemon's own build are different facts and a `bool` here would fold them. Every
    /// client older than the key sends exactly that silence, so the fold would make the commonest
    /// case read as the safe one.
    ///
    /// Keyed by CLIENT and not by connection, for the reason the size and the mailbox beside it
    /// are: a client's several connections are one surface, and they are one PROCESS — so they
    /// carry one build, and the last hello of a client re-states what its first one did.
    ///
    /// Dropped with the client in [`disconnect`](AttachmentRegistry::disconnect), for the reason
    /// the mailbox is: a client id is a lifecycle token, and a departed window's build left behind
    /// would be reported as the build of whoever next holds that id.
    client_build: HashMap<ClientId, String>,
    /// The stamp the next report takes. Monotone for the life of the daemon — it orders reports,
    /// it does not count them, so wrapping is not a concern at one per window change.
    next_ordinal: u64,
}

impl AttachmentRegistry {
    /// Associate `conn` with the `client` it belongs to (`client/hello`). Idempotent; every
    /// connection of a client calls this once so the client stays present while any is live.
    ///
    /// `build` is what that client SAID it is ([`sprag_rpc::CLIENT_BUILD_PARAM`]) — `None` from a
    /// client that did not say, which is not a client that matches. A `None` never ERASES a build
    /// already held for this client: the two connections of one window are one process, so the
    /// answer belongs to the client rather than to whichever connection spoke last.
    pub fn hello(&mut self, conn: ConnId, client: ClientId, build: Option<String>) {
        if let Some(build) = build {
            self.client_build.insert(client.clone(), build);
        }
        self.conn_client.insert(conn, client);
    }

    /// Attach (or switch — tmux `switch-client`) the client owning `conn` to `session`, whose
    /// identity is `id`. The connection must have said hello first; otherwise
    /// [`AttachOutcome::NoClient`].
    ///
    /// `id` is not a second spelling of `session`: it is what the VISIT is recorded under (this
    /// registry's per-client history, read back by [`last_viewed`](Self::last_viewed)), because a
    /// history entry has to outlive the name it was made under. Both come off one resolved [`SessionScope`](crate::SessionScope), read
    /// off the same session at the same instant, so they cannot describe two sessions.
    ///
    /// The visit is recorded even when the attachment does not move: re-declaring the session you
    /// are already on is idempotent for the attachment (nothing to announce) and for the history
    /// too (the dedup lifts the entry it already holds back to the front, where it already was).
    /// `landing` is the window this client arrives on — the session's CURRENT window, which is what
    /// that field means now that each client carries a view of its own.
    /// Recorded only on a real change, for the same reason the restamp below is: an idempotent
    /// re-send of `client/attach` must not drag a client back off the window it has since selected.
    pub fn attach(
        &mut self,
        conn: ConnId,
        session: String,
        id: SessionId,
        landing: WindowId,
    ) -> AttachOutcome {
        let Some(client) = self.conn_client.get(&conn) else {
            return AttachOutcome::NoClient;
        };
        let client = client.clone();
        // The visit is a fact about this client whatever the attachment does, so it is recorded
        // before the outcome is decided.
        push_visit(self.client_history.entry(client.clone()).or_default(), id);
        match self.client_session.get(&client) {
            Some(prev) if *prev == session => AttachOutcome::Unchanged,
            _ => {
                // An attach makes this client the most recent one, which is what `window-size
                // latest` reads. Only on a real change: an idempotent re-send says nothing new, and
                // reordering on it would let a client that merely re-declared its session take the
                // window from one the user had just resized.
                self.restamp(&client);
                self.client_window.insert(client.clone(), landing);
                let previous = self.client_session.insert(client, session);
                AttachOutcome::Changed { previous }
            }
        }
    }

    /// Record that the client owning `conn` is now looking at `window` — its own `select-window`.
    ///
    /// Answers whether that MOVED it, so the caller can skip a scene bump for a client re-selecting
    /// the window it is already on. A connection that never said hello, or a client that has not
    /// attached, is not watching anything and is left alone.
    pub fn watch(&mut self, conn: ConnId, window: WindowId) -> bool {
        let Some(client) = self.conn_client.get(&conn) else {
            return false;
        };
        let client = client.clone();
        if !self.client_session.contains_key(&client) {
            return false;
        }
        self.client_window.insert(client, window) != Some(window)
    }

    /// Put every client of `session` whose view names a window that is no longer in `live` back on
    /// `landing` — what a window's DEATH owes its viewers.
    ///
    /// A seat naming a window that is gone is not just stale: the arbitration folds a client into
    /// the window it is watching ([`sizes_for`](Self::sizes_for)), so those clients would size
    /// nothing at all and the window that survived them would arbitrate from an empty list. Driven
    /// off the registry's own live list rather than from each of the several places a window can
    /// die, so a new way to end one cannot be forgotten here.
    pub fn reseat(&mut self, session: &str, live: &[WindowId], landing: WindowId) -> bool {
        let stranded: Vec<ClientId> = self
            .client_session
            .iter()
            .filter(|(client, viewing)| {
                viewing.as_str() == session
                    && !self
                        .client_window
                        .get(*client)
                        .is_some_and(|window| live.contains(window))
            })
            .map(|(client, _)| client.clone())
            .collect();
        for client in &stranded {
            self.client_window.insert(client.clone(), landing);
        }
        !stranded.is_empty()
    }

    /// The window the client owning `conn` is looking at — how a request resolves its scope to the
    /// window THIS client is on rather than to the one its session last landed somebody on.
    #[must_use]
    pub fn window_of(&self, conn: ConnId) -> Option<WindowId> {
        let client = self.conn_client.get(&conn)?;
        self.client_window.get(client).copied()
    }

    /// The session the client owning `conn` was viewing BEFORE its current one and is still there
    /// to go back to — tmux `switch-client -l`'s target, resolved by IDENTITY and answered as the
    /// name that session carries NOW. `None` when this client has no such session: it never
    /// switched, or everything else it visited is gone.
    ///
    /// `name_of` resolves an id — `SessionRegistry::name_of`, which answers liveness AND the current
    /// name in one lookup, which is the whole reason the history is kept as ids. It is passed in
    /// rather than held so this registry keeps knowing nothing about the session registry (the same
    /// lazy-resolver shape [`SessionScope::resolve`](crate::SessionScope::resolve) takes for the
    /// attached scope, and it keeps the lock order attachments→registry rather than the reverse).
    ///
    /// `unattached` narrows the answer to a session NO OTHER client is viewing — tmux
    /// `detach-on-destroy no-detached`'s "most recently used detached session". It is exact here,
    /// because this registry IS the attachment map; the client-side filter it replaces read a
    /// mirror its own poll refreshed.
    ///
    /// It takes `&mut self` because it PRUNES: an id `name_of` no longer resolves is a session that
    /// will never come back (ids are never reissued), so it is dropped as the walk passes it. That
    /// is the whole of the history's garbage collection, and it runs on a user gesture rather than
    /// on any hot path.
    pub fn last_viewed(
        &mut self,
        conn: ConnId,
        name_of: impl Fn(SessionId) -> Option<String>,
        unattached: bool,
    ) -> Option<(SessionId, String)> {
        let client = self.conn_client.get(&conn)?.clone();
        // The session this client is on now is not somewhere to go BACK to. Compared by name
        // because that is what the attachment holds; the candidate is compared as the name it
        // carries now, so a renamed current session still excludes itself.
        let current = self.client_session.get(&client).cloned();
        // What every OTHER client is viewing, read before the history is borrowed and only when the
        // narrowing is asked for — the exact answer `no-detached` needs.
        let occupied: Vec<String> = if unattached {
            self.client_session
                .iter()
                .filter(|(other, _)| **other != client)
                .map(|(_, viewing)| viewing.clone())
                .collect()
        } else {
            Vec::new()
        };
        let history = self.client_history.get_mut(&client)?;
        let mut answer = None;
        history.retain(|id| {
            let Some(name) = name_of(*id) else {
                return false; // gone for good — an id is never reissued.
            };
            if answer.is_none() && Some(&name) != current.as_ref() && !occupied.contains(&name) {
                answer = Some((*id, name));
            }
            true
        });
        answer
    }

    /// Record the cell area the client owning `conn` can give a window
    /// ([`crate::wire::CLIENT_SIZE_METHOD`]). The connection must have said hello first.
    ///
    /// A report of a DIFFERENT area makes this client the most recent one, which is what
    /// `window-size latest` reads. Re-reporting the same area is [`SizeOutcome::Unchanged`] and
    /// leaves the order alone: a client that re-sent its own numbers has moved nothing a policy can
    /// see, and announcing it would wake every other client to re-read a window that did not move.
    pub fn size(&mut self, conn: ConnId, size: ClientSize) -> SizeOutcome {
        let Some(client) = self.conn_client.get(&conn) else {
            return SizeOutcome::NoClient;
        };
        let client = client.clone();
        if self.client_size.get(&client).map(|held| held.size) == Some(size) {
            return SizeOutcome::Unchanged;
        }
        let ordinal = self.take_ordinal();
        self.client_size.insert(client, Reported { size, ordinal });
        SizeOutcome::Changed
    }

    /// The session the client owning `conn` is attached to, if it has one.
    ///
    /// The caller's use is announcing: a fact about a CLIENT (its area) changes what a SESSION's
    /// window is, and only the registry knows which session that is.
    #[must_use]
    pub fn session_of(&self, conn: ConnId) -> Option<&str> {
        let client = self.conn_client.get(&conn)?;
        self.client_session.get(client).map(String::as_str)
    }

    /// Move `client` to the front of the recency order, if it has a size to order.
    ///
    /// A client with no reported area is left alone: there is nothing for `latest` to name, and
    /// inventing a stamp for an absent size would make it the "most recent" answer to a question
    /// it cannot answer.
    fn restamp(&mut self, client: &ClientId) {
        if let Some(held) = self.client_size.get(client).copied() {
            let ordinal = self.take_ordinal();
            self.client_size
                .insert(client.clone(), Reported { ordinal, ..held });
        }
    }

    /// The next recency stamp.
    fn take_ordinal(&mut self) -> u64 {
        self.next_ordinal = self.next_ordinal.saturating_add(1);
        self.next_ordinal
    }

    /// Every area reported by a client attached to `session` AND LOOKING AT `window`, oldest first
    /// — the arbitration's real input.
    ///
    /// # Why the window and not just the session
    ///
    /// "How big is this window" is a question about the people who can SEE it. Folding every client
    /// of the session in was right only while they all saw the same thing, and R346 made them not:
    /// a phone on window 1 has nothing to say about window 0's size, and saying it anyway is the
    /// defect this whole arc is about — a small client attaching and shrinking a big client's panes.
    ///
    /// A client that has not reported an area is absent rather than present with a zero: a policy
    /// taking the smallest client must not be handed a size nobody has.
    #[must_use]
    pub fn sizes_for(&self, session: &str, window: WindowId) -> Vec<ClientSize> {
        self.reported(|client, viewing| {
            viewing == session && self.client_window.get(client) == Some(&window)
        })
    }

    /// The walk behind [`sizes_for`](Self::sizes_for): every reported area whose client `keep`
    /// admits, OLDEST FIRST — so the last element is the most recent report, which is what
    /// `window-size latest` names.
    fn reported(&self, keep: impl Fn(&ClientId, &str) -> bool) -> Vec<ClientSize> {
        let mut reported: Vec<Reported> = self
            .client_session
            .iter()
            .filter(|(client, viewing)| keep(client, viewing.as_str()))
            .filter_map(|(client, _)| self.client_size.get(client).copied())
            .collect();
        reported.sort_by_key(|held| held.ordinal);
        reported.into_iter().map(|held| held.size).collect()
    }

    /// Release `conn` on close. When it is the LAST live connection of its client, that client is
    /// gone: its attachment is dropped (the crash-safe `-1`). Returns the SESSION the released
    /// client was attached to — `Some` exactly when a per-session count fell (so the caller bumps
    /// the scene and can log which session lost a viewer), `None` for a stray conn (never said
    /// hello), a client with other live connections, or one that never attached.
    pub fn disconnect(&mut self, conn: ConnId) -> Option<String> {
        let client = self.conn_client.remove(&conn)?;
        if self.conn_client.values().any(|c| *c == client) {
            // The client still has another connection open; it stays present and attached.
            return None;
        }
        // The area goes with the client. A departed client's size left behind would keep
        // arbitrating a window nobody is looking at — the smallest attached client would stay
        // smallest forever after it closed.
        self.client_size.remove(&client);
        // ...and the window it was looking at, for the same reason: a departed client's view would
        // keep arbitrating a window nobody is watching.
        self.client_window.remove(&client);
        // ...and so does where it had been. A visit history belongs to the client that made it, and
        // a client id is a lifecycle token: the next client to hold one is a different client, and
        // inheriting somebody else's "go back" is the same capture in a different key.
        self.client_history.remove(&client);
        // ...and so does anything still waiting to be said to it. A client id is a lifecycle token,
        // so leaving the mailbox would hand a stranger a sentence meant for somebody who has gone.
        self.client_mail.remove(&client);
        // ...and so does what it said it was built from. A client id is a lifecycle token, so a
        // departed window's build left behind would be reported as the build of whoever takes that
        // id next — and the whole value of this field is that a reader can trust which window it
        // describes.
        self.client_build.remove(&client);
        self.client_session.remove(&client)
    }

    /// Move every client attached to `from` over to `to` — what a session RENAME does here.
    ///
    /// An attachment is *which session this client is VIEWING*, and a rename does not change what
    /// anyone is looking at: it changes what that thing is called. Leaving the old string in place
    /// would make [`clients`](Self::clients) (the `sprag list-clients` listing) name a session the
    /// registry no longer holds, and [`attached_count`](Self::attached_count) report the renamed
    /// session as having no viewers while people are typing into it — the badge every other client
    /// draws.
    ///
    /// Returns how many attachments moved, so the caller can tell an actual rename of a viewed
    /// session from one nobody was watching.
    pub fn rename_session(&mut self, from: &str, to: &str) -> usize {
        let mut moved = 0;
        for session in self.client_session.values_mut() {
            if session == from {
                *session = to.to_owned();
                moved += 1;
            }
        }
        moved
    }

    /// Release every client attached to `session` — what a session's DESTRUCTION does here.
    ///
    /// The twin of [`rename_session`](Self::rename_session), and the reason it exists is the same
    /// one written there: an attachment names a session that must exist. A rename moves the name;
    /// a kill leaves no name to move to, so the attachment ENDS.
    ///
    /// **Leaving it was a shipping defect, measured at R303**: after `kill-session alpha`,
    /// `sprag list-clients` went on reporting a viewer of `alpha` — and when a NEW session took the
    /// freed name, that session INHERITED the viewer, so `sprag ls` showed `alpha … (1 attached)`
    /// for a session no client had ever seen. Two facts the daemon publishes, both wrong, with no
    /// client at fault.
    ///
    /// It matters more now than it read then, because an attachment is an ADDRESS a client can
    /// scope to ([`ScopeAsk::Attached`](sprag_rpc::ScopeAsk::Attached)): a stale one would hand the
    /// impostor's panes to the client that was viewing the dead session — the very capture the
    /// attached scope exists to make impossible. Releasing here is what keeps that promise total
    /// rather than true only of renames.
    ///
    /// The client's reported SIZE is deliberately kept: it is a fact about that client's own
    /// surface, not about the session that died, and a client that switches to another session must
    /// arbitrate with it at once rather than after re-reporting.
    ///
    /// Returns how many attachments were released, so the caller can tell the death of a session
    /// somebody was watching from one nobody was.
    pub fn session_ended(&mut self, session: &str) -> usize {
        let before = self.client_session.len();
        // The views go with the attachments: a window of a destroyed session is not somewhere a
        // client can still be looking, and an entry left behind would arbitrate for a window that
        // no longer exists (and, once ids are re-read, for whatever the caller asks about).
        self.client_window.retain(|client, _| {
            self.client_session
                .get(client)
                .is_some_and(|v| v != session)
        });
        self.client_session.retain(|_, viewing| viewing != session);
        before - self.client_session.len()
    }

    /// How many DISTINCT clients are currently attached to `session` — the wire
    /// [`SessionInfo::attached`](sprag_terminal::SessionInfo::attached) badge.
    #[must_use]
    pub fn attached_count(&self, session: &str) -> usize {
        self.client_session
            .values()
            .filter(|s| s.as_str() == session)
            .count()
    }

    /// Every currently-attached client and the session it views — the `clients` wire slot behind
    /// the `sprag list-clients` CLI (tmux `list-clients`). Only clients that have ATTACHED appear
    /// (a hello-only connection is present but views nothing), which is exactly tmux's rule that a
    /// client is listed once it is attached to a session. Sorted by (client, session) so the wire
    /// order is deterministic — a `HashMap`'s iteration order is not, and a CLI listing that
    /// reshuffles between reads would be noise.
    #[must_use]
    pub fn clients(&self, name_of: impl Fn(&str, WindowId) -> Option<String>) -> Vec<ClientInfo> {
        let mut clients: Vec<ClientInfo> = self
            .client_session
            .iter()
            .map(|(client, session)| ClientInfo {
                client: client.clone(),
                session: session.clone(),
                size: self.client_size.get(client).map(|held| held.size),
                // Cloned rather than defaulted: the ABSENCE is a fact this row must be able to
                // carry, because a client that did not say is not a client that matches.
                build: self.client_build.get(client).cloned(),
                // Resolved by the CALLER, for the reason `last_viewed` takes its resolver: this
                // registry holds no session tree, and the lock order is attachments THEN registry.
                window: self
                    .client_window
                    .get(client)
                    .and_then(|window| name_of(session, *window)),
            })
            .collect();
        clients.sort_by(|a, b| {
            a.client
                .cmp(&b.client)
                .then_with(|| a.session.cmp(&b.session))
        });
        clients
    }

    /// Put `announcement` in front of everyone `audience` names, and answer WHO — see [`Delivery`].
    ///
    /// **A message goes to ATTACHED clients only, and that is the whole reason this lives here.** A
    /// client that has said hello but attached to nothing is viewing no session and painting no row,
    /// so it has nowhere to put a sentence; queueing for it would let the answer claim a delivery
    /// that could never be shown. The set this walks is [`clients`](Self::clients)'s own, so the
    /// listing a caller reads to choose a `-c` target and the set a message reaches are one map.
    ///
    /// Each client keeps ONE waiting message, resolved by [`Announcement::over`] — see there for why
    /// a slot rather than a queue.
    pub fn deliver(&mut self, audience: &Audience, announcement: &Announcement) -> Delivery {
        // NO WINDOW RESOLVER, and that is not a gap: `Delivery` exposes ids and sessions only
        // ([`Delivery::clients`], [`Delivery::sessions`]), so the field never reaches a surface from
        // here — and resolving one would mean taking the registry lock inside the attachment lock,
        // which is the order this whole module refuses.
        let mut to: Vec<ClientInfo> = self
            .clients(|_, _| None)
            .into_iter()
            .filter(|client| audience.reaches(client))
            .collect();
        to.sort_by(|a, b| a.client.cmp(&b.client));
        for client in &to {
            let waiting = self.client_mail.remove(&client.client);
            self.client_mail
                .insert(client.client.clone(), announcement.clone().over(waiting));
        }
        Delivery { to }
    }

    /// Take the message waiting for the client owning `conn`, if any — a COLLECTION, so one message
    /// is shown once.
    ///
    /// Removing rather than reading is what makes the delivery exactly-once for a live client: a
    /// client that re-asks after painting is not handed the same sentence again, and no cursor has to
    /// be threaded through the wire to say so. A client that dies between the queueing and the
    /// collection loses the message with its mailbox, which is the honest bound
    /// [`Delivery`] documents rather than papers over.
    #[must_use = "collecting a message REMOVES it from the daemon — an answer that is dropped is a \
                  person's message destroyed with no error anywhere, which is exactly the defect \
                  R316's `Report` exists to remove"]
    pub fn collect(&mut self, conn: ConnId) -> Option<Announcement> {
        let client = self.conn_client.get(&conn)?;
        self.client_mail.remove(client)
    }

    /// Whether `client` is present — a client id that [`deliver`](Self::deliver) could name.
    #[must_use]
    pub fn is_attached(&self, client: &str) -> bool {
        self.client_session.contains_key(client)
    }
}

/// Who a message is for.
///
/// Two arms, because there are exactly two questions a caller can answer about an audience it
/// cannot see: *this one particular client* and *whoever is looking at this session*. There is no
/// "everybody on the box" arm and that is deliberate — a message is shown to a person, and a daemon
/// serving several people's sessions has no business letting one of them interrupt all of them.
///
/// The rival has no arm at all: `notification.show` carries no target, its `ToastNotification` has a
/// `target` field the API path always fills with `None`, and delivery is to "the foreground client"
/// or to `NoForegroundClient` (`handle_notification_show`, `app/api.rs`, read at `9a4ce5e1`). That is
/// not a gap in their design so much as a consequence of it — they are one process with one UI, so
/// the question cannot arise. sprag is a daemon with N clients on M sessions, so it must be asked,
/// and answering it is what makes `-c` mean something.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Audience {
    /// One named client, wherever it is attached — tmux `display-message -c`.
    Client(ClientId),
    /// Every client attached to one session — the default, and what "tell whoever is watching this"
    /// means when the caller is a script that knows a session and not a window on somebody's desk.
    Session(String),
}

impl Audience {
    /// Whether this audience includes `client`.
    #[must_use]
    fn reaches(&self, client: &ClientInfo) -> bool {
        match self {
            Self::Client(id) => client.client == *id,
            Self::Session(session) => client.session == *session,
        }
    }
}

/// Who a message actually reached.
///
/// # Why the answer is a VALUE and not a `bool`
///
/// R316's whole finding one level up: an outcome nobody reads is a defect waiting for a user to find.
/// An agent that says *"the deploy needs you"* into a daemon where nobody is attached has told
/// nobody, and `ok` is the wrong answer to that. So the verb answers the LIST, `#[must_use]`, and
/// every surface has to decide what to do with it.
///
/// # What "delivered" claims, exactly
///
/// That the message is in front of that client and will be painted on its next frame — not that a
/// person has read it. A read receipt would need an acknowledgement round trip, and a client that
/// dies in the millisecond after this answer loses its mailbox with itself. The honest bound is
/// stated here rather than implied by a hopeful word.
///
/// The rival answers one word for one implicit destination (`shown` plus a reason:
/// `Disabled | RateLimited | NoForegroundClient | Busy`), which is a better answer than "ok" and
/// worse than a list — with two windows open it cannot say which one got it.
#[derive(Clone, PartialEq, Eq, Debug)]
#[must_use = "a message that reached nobody is exactly the outcome this type exists to report — \
              read the delivery, or the caller has been told `ok` for a sentence no person saw"]
pub struct Delivery {
    /// The clients the message was put in front of, ordered by client id.
    to: Vec<ClientInfo>,
}

impl Delivery {
    /// The clients the message reached, ordered by client id.
    #[must_use]
    pub fn clients(&self) -> Vec<&str> {
        self.to
            .iter()
            .map(|client| client.client.as_str())
            .collect()
    }

    /// The sessions whose change channels must be woken so the clients above repaint promptly —
    /// deduplicated, because two clients on one session share one channel.
    ///
    /// Derived from the delivery rather than recomputed by the caller: the set that must be woken is
    /// exactly the set that was written to, and a caller free to compute its own could wake a
    /// different one. That is [`crate::notify`]'s standing hazard — a client parked forever because
    /// the bump landed on the wrong session — kept impossible by construction.
    #[must_use]
    pub fn sessions(&self) -> Vec<&str> {
        let mut sessions: Vec<&str> = self
            .to
            .iter()
            .map(|client| client.session.as_str())
            .collect();
        sessions.sort_unstable();
        sessions.dedup();
        sessions
    }
}

/// The sentence a surface prints — ONE wording, so the CLI and any other reader cannot describe one
/// delivery differently.
impl std::fmt::Display for Delivery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.to.as_slice() {
            [] => write!(f, "shown to nobody: no client is attached"),
            [one] => write!(f, "shown to {} on session \"{}\"", one.client, one.session),
            many => {
                write!(f, "shown to {} clients: ", many.len())?;
                for (n, client) in many.iter().enumerate() {
                    if n > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{} on session \"{}\"", client.client, client.session)?;
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn(n: u64) -> ConnId {
        // ConnId has no public constructor from a raw value (it is transport-minted), so allocate
        // fresh, real ids; the tests only need distinct, stable tokens, which allocation gives.
        let _ = n;
        ConnId::allocate()
    }

    /// A stable, distinct identity for each session NAME these tests use.
    ///
    /// The daemon mints one [`SessionId`] per session and hands it here beside the name; this
    /// registry never resolves either, so all a test needs is that the two correspond — the same
    /// name means the same session throughout a test, and two names never collide.
    ///
    /// A test about a name being REISSUED (a different session under a name an earlier one wore)
    /// must NOT use this: it says the opposite of what that test is about. Those call
    /// [`AttachmentRegistry::attach`] with an explicit fresh id, which is the whole point of there
    /// being an id at all.
    /// A deterministic [`WindowId`] for a fixture's window, the way [`sid`] mints a session's — so
    /// a test can say "the window of session X" without building a registry to hold one.
    fn wid(name: &str) -> WindowId {
        WindowId(sid(name).0 ^ 0x5555_5555_5555_5555)
    }

    fn sid(name: &str) -> SessionId {
        // A tiny FNV-1a over the name — deterministic, order-independent, and no dependency.
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in name.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100_0000_01b3);
        }
        SessionId(hash)
    }

    /// An announcement at `severity`, for the delivery tests below.
    fn say(text: &str, severity: crate::report::Severity) -> Announcement {
        Announcement {
            text: crate::report::MessageText::parse(text).expect("a plain sentence"),
            severity,
        }
    }

    /// **A message reaches every client attached to the session and NOBODY ELSE** — the default
    /// audience, with a client on another session as the control that must not be reached.
    #[test]
    fn a_session_message_reaches_that_sessions_viewers_only() {
        let mut registry = AttachmentRegistry::default();
        let (watching, also, elsewhere) = (conn(1), conn(2), conn(3));
        registry.hello(watching, "one".into(), None);
        registry.hello(also, "two".into(), None);
        registry.hello(elsewhere, "three".into(), None);
        registry.attach(watching, "build".into(), sid("build"), wid("build"));
        registry.attach(also, "build".into(), sid("build"), wid("build"));
        registry.attach(elsewhere, "notes".into(), sid("notes"), wid("notes"));

        let delivery = registry.deliver(
            &Audience::Session("build".into()),
            &say("the deploy finished", crate::report::Severity::Note),
        );
        assert_eq!(delivery.clients(), ["one", "two"]);
        assert_eq!(delivery.sessions(), ["build"], "one channel, not two");

        assert!(registry.collect(watching).is_some());
        assert!(registry.collect(also).is_some());
        assert!(
            registry.collect(elsewhere).is_none(),
            "the client on another session is the CONTROL: it must have been given nothing",
        );
    }

    /// A `-c` message reaches ONE named client, even when a second client shares its session.
    #[test]
    fn a_named_client_is_the_only_one_reached() {
        let mut registry = AttachmentRegistry::default();
        let (named, neighbour) = (conn(1), conn(2));
        registry.hello(named, "one".into(), None);
        registry.hello(neighbour, "two".into(), None);
        registry.attach(named, "build".into(), sid("build"), wid("build"));
        registry.attach(neighbour, "build".into(), sid("build"), wid("build"));

        let delivery = registry.deliver(
            &Audience::Client("one".into()),
            &say("your turn", crate::report::Severity::Alert),
        );
        assert_eq!(delivery.clients(), ["one"]);
        assert!(registry.collect(named).is_some());
        assert!(registry.collect(neighbour).is_none());
    }

    /// **A message to nobody says so.** The daemon holds nothing, the delivery is empty, and its own
    /// sentence names the reason — the outcome R316's thesis says must not come back as `ok`.
    #[test]
    fn a_message_with_no_client_attached_reaches_nobody_and_says_which() {
        let mut registry = AttachmentRegistry::default();
        let hello_only = conn(1);
        registry.hello(hello_only, "one".into(), None);

        let delivery = registry.deliver(
            &Audience::Session("build".into()),
            &say("nobody is here", crate::report::Severity::Warn),
        );
        assert_eq!(delivery.clients(), Vec::<&str>::new());
        assert_eq!(delivery.sessions(), Vec::<&str>::new());
        assert_eq!(
            delivery.to_string(),
            "shown to nobody: no client is attached",
        );
        assert!(
            registry.collect(hello_only).is_none(),
            "a client that has said hello but attached to nothing is painting no row",
        );
    }

    /// A message is collected ONCE — the second ask gets nothing, so a client that repaints does not
    /// show the same sentence again.
    #[test]
    fn a_message_is_handed_over_exactly_once() {
        let mut registry = AttachmentRegistry::default();
        let client = conn(1);
        registry.hello(client, "one".into(), None);
        registry.attach(client, "build".into(), sid("build"), wid("build"));

        let _ = registry.deliver(
            &Audience::Session("build".into()),
            &say("once", crate::report::Severity::Note),
        );
        assert_eq!(
            registry.collect(client).map(|a| a.text.as_str().to_owned()),
            Some("once".to_owned()),
        );
        assert!(registry.collect(client).is_none());
    }

    /// **A note arriving behind an undelivered alert does not displace it**, and the reverse does —
    /// the row's own precedence rule, held one step earlier so a client is never handed the message
    /// it would then have refused to show.
    #[test]
    fn a_waiting_alert_is_not_displaced_by_a_note() {
        let mut registry = AttachmentRegistry::default();
        let client = conn(1);
        registry.hello(client, "one".into(), None);
        registry.attach(client, "build".into(), sid("build"), wid("build"));
        let audience = Audience::Session("build".into());

        let _ = registry.deliver(&audience, &say("your turn", crate::report::Severity::Alert));
        let _ = registry.deliver(&audience, &say("a note", crate::report::Severity::Note));
        assert_eq!(
            registry.collect(client).map(|a| a.text.as_str().to_owned()),
            Some("your turn".to_owned()),
            "the alert kept the slot",
        );

        let _ = registry.deliver(&audience, &say("a note", crate::report::Severity::Note));
        let _ = registry.deliver(&audience, &say("your turn", crate::report::Severity::Alert));
        assert_eq!(
            registry.collect(client).map(|a| a.text.as_str().to_owned()),
            Some("your turn".to_owned()),
            "and it takes the slot from a note",
        );
    }

    /// A message nobody collected dies with the client it was addressed to — a client id is a
    /// lifecycle token, so the next holder of one must not inherit somebody else's sentence.
    #[test]
    fn an_uncollected_message_does_not_outlive_its_client() {
        let mut registry = AttachmentRegistry::default();
        let gone = conn(1);
        registry.hello(gone, "one".into(), None);
        registry.attach(gone, "build".into(), sid("build"), wid("build"));
        let _ = registry.deliver(
            &Audience::Session("build".into()),
            &say("never read", crate::report::Severity::Alert),
        );
        registry.disconnect(gone);

        let reborn = conn(2);
        registry.hello(reborn, "one".into(), None);
        registry.attach(reborn, "build".into(), sid("build"), wid("build"));
        assert!(
            registry.collect(reborn).is_none(),
            "the new client wearing the same id inherits nothing",
        );
    }

    /// A client with TWO connections is one mailbox: the message is queued once and whichever
    /// connection asks first gets it, so a two-connection client cannot show one sentence twice.
    #[test]
    fn a_clients_several_connections_share_one_mailbox() {
        let mut registry = AttachmentRegistry::default();
        let (requests, poll) = (conn(1), conn(2));
        registry.hello(requests, "one".into(), None);
        registry.hello(poll, "one".into(), None);
        registry.attach(requests, "build".into(), sid("build"), wid("build"));

        let delivery = registry.deliver(
            &Audience::Session("build".into()),
            &say("once", crate::report::Severity::Note),
        );
        assert_eq!(
            delivery.clients(),
            ["one"],
            "one client, not two connections"
        );
        assert!(registry.collect(poll).is_some());
        assert!(registry.collect(requests).is_none());
    }

    /// The wording a surface prints is this type's, in all three shapes — so a listing cannot say
    /// "1 clients" or omit which session a client is watching.
    #[test]
    fn the_delivery_says_who_in_its_own_words() {
        let mut registry = AttachmentRegistry::default();
        let (one, two) = (conn(1), conn(2));
        registry.hello(one, "gui-1".into(), None);
        registry.hello(two, "gui-2".into(), None);
        registry.attach(one, "build".into(), sid("build"), wid("build"));
        let audience = Audience::Session("build".into());

        assert_eq!(
            registry
                .deliver(&audience, &say("x", crate::report::Severity::Note))
                .to_string(),
            "shown to gui-1 on session \"build\"",
        );
        registry.attach(two, "build".into(), sid("build"), wid("build"));
        assert_eq!(
            registry
                .deliver(&audience, &say("x", crate::report::Severity::Note))
                .to_string(),
            "shown to 2 clients: gui-1 on session \"build\", gui-2 on session \"build\"",
        );
    }

    /// `is_attached` answers about the same set `clients` lists, which is what makes a `-c` refusal
    /// able to say the name is not one of these rather than guessing.
    #[test]
    fn a_client_is_addressable_exactly_while_it_is_listed() {
        let mut registry = AttachmentRegistry::default();
        let client = conn(1);
        registry.hello(client, "one".into(), None);
        assert!(
            !registry.is_attached("one"),
            "hello alone is not an attachment",
        );
        registry.attach(client, "build".into(), sid("build"), wid("build"));
        assert!(registry.is_attached("one"));
        assert!(!registry.is_attached("two"));
        registry.disconnect(client);
        assert!(!registry.is_attached("one"));
    }

    #[test]
    fn hello_then_attach_counts_one() {
        let mut reg = AttachmentRegistry::default();
        let c = conn(1);
        reg.hello(c, "client-a".to_owned(), None);
        assert_eq!(
            reg.attach(c, "work".to_owned(), sid("work"), wid("work")),
            AttachOutcome::Changed { previous: None },
            "a FIRST attach left no session behind",
        );
        assert_eq!(reg.attached_count("work"), 1);
        assert_eq!(reg.attached_count("other"), 0);
    }

    #[test]
    fn one_client_two_connections_counts_once() {
        // A GUI's poll + request connections both hello with the same client id; only the request
        // attaches. The window is ONE attached client, not two.
        let mut reg = AttachmentRegistry::default();
        let poll = conn(1);
        let request = conn(2);
        reg.hello(poll, "gui".to_owned(), None);
        reg.hello(request, "gui".to_owned(), None);
        reg.attach(request, "work".to_owned(), sid("work"), wid("work"));
        assert_eq!(
            reg.attached_count("work"),
            1,
            "one window is one attachment"
        );
    }

    #[test]
    fn re_attach_same_session_is_unchanged() {
        let mut reg = AttachmentRegistry::default();
        let c = conn(1);
        reg.hello(c, "client-a".to_owned(), None);
        assert_eq!(
            reg.attach(c, "work".to_owned(), sid("work"), wid("work")),
            AttachOutcome::Changed { previous: None }
        );
        assert_eq!(
            reg.attach(c, "work".to_owned(), sid("work"), wid("work")),
            AttachOutcome::Unchanged,
            "an idempotent re-send moves no count"
        );
    }

    #[test]
    fn switch_moves_the_count_between_sessions() {
        let mut reg = AttachmentRegistry::default();
        let c = conn(1);
        reg.hello(c, "client-a".to_owned(), None);
        reg.attach(c, "one".to_owned(), sid("one"), wid("one"));
        assert_eq!(
            reg.attach(c, "two".to_owned(), sid("two"), wid("two")),
            AttachOutcome::Changed {
                previous: Some("one".to_owned())
            },
            "a SWITCH names the session it left, so that one's badge can be announced too",
        );
        assert_eq!(reg.attached_count("one"), 0, "left the old session");
        assert_eq!(reg.attached_count("two"), 1, "on the new session");
    }

    #[test]
    fn attach_without_hello_is_refused() {
        let mut reg = AttachmentRegistry::default();
        assert_eq!(
            reg.attach(conn(1), "work".to_owned(), sid("work"), wid("work")),
            AttachOutcome::NoClient
        );
        assert_eq!(reg.attached_count("work"), 0);
    }

    #[test]
    fn disconnect_of_the_only_connection_releases_the_client() {
        let mut reg = AttachmentRegistry::default();
        let c = conn(1);
        reg.hello(c, "client-a".to_owned(), None);
        reg.attach(c, "work".to_owned(), sid("work"), wid("work"));
        assert_eq!(
            reg.disconnect(c).as_deref(),
            Some("work"),
            "releasing the attached client reports its session"
        );
        assert_eq!(reg.attached_count("work"), 0);
    }

    #[test]
    fn client_stays_attached_until_its_last_connection_closes() {
        let mut reg = AttachmentRegistry::default();
        let poll = conn(1);
        let request = conn(2);
        reg.hello(poll, "gui".to_owned(), None);
        reg.hello(request, "gui".to_owned(), None);
        reg.attach(request, "work".to_owned(), sid("work"), wid("work"));
        assert!(
            reg.disconnect(poll).is_none(),
            "the client still has its request connection"
        );
        assert_eq!(reg.attached_count("work"), 1, "still one attachment");
        assert_eq!(
            reg.disconnect(request).as_deref(),
            Some("work"),
            "now the last connection closed"
        );
        assert_eq!(reg.attached_count("work"), 0);
    }

    #[test]
    fn distinct_clients_on_one_session_each_count() {
        let mut reg = AttachmentRegistry::default();
        let a = conn(1);
        let b = conn(2);
        reg.hello(a, "client-a".to_owned(), None);
        reg.hello(b, "client-b".to_owned(), None);
        reg.attach(a, "work".to_owned(), sid("work"), wid("work"));
        reg.attach(b, "work".to_owned(), sid("work"), wid("work"));
        assert_eq!(reg.attached_count("work"), 2, "two windows, two viewers");
        reg.disconnect(a);
        assert_eq!(reg.attached_count("work"), 1, "one left, one remains");
    }

    #[test]
    fn disconnect_of_a_hello_only_connection_moves_nothing() {
        let mut reg = AttachmentRegistry::default();
        let c = conn(1);
        reg.hello(c, "client-a".to_owned(), None);
        assert!(
            reg.disconnect(c).is_none(),
            "a connection that never attached releases no count"
        );
    }

    #[test]
    fn clients_lists_each_attached_client_with_its_session() {
        let mut reg = AttachmentRegistry::default();
        let a = conn(1);
        let b = conn(2);
        let hello_only = conn(3);
        reg.hello(a, "client-b".to_owned(), None);
        reg.hello(b, "client-a".to_owned(), None);
        reg.hello(hello_only, "client-c".to_owned(), None);
        reg.attach(a, "work".to_owned(), sid("work"), wid("work"));
        reg.attach(b, "home".to_owned(), sid("home"), wid("home"));
        // Only ONE of them reports an area, so the listing is asserted in both states: a size that
        // was reported, and the honest absence of one that was not.
        reg.size(
            b,
            ClientSize {
                cols: 120,
                rows: 40,
            },
        );
        // client-c said hello but never attached: it views nothing, so it is NOT listed.
        let clients = reg.clients(|_, window| Some(format!("w{}", window.0)));
        assert_eq!(
            clients,
            vec![
                ClientInfo {
                    client: "client-a".to_owned(),
                    session: "home".to_owned(),
                    size: Some(ClientSize {
                        cols: 120,
                        rows: 40
                    }),
                    window: Some(format!("w{}", wid("home").0)),
                    // Neither said which build it is — the wire every client older than
                    // `CLIENT_BUILD_PARAM` sends, and an absence a reader must not read as a match.
                    build: None,
                },
                ClientInfo {
                    client: "client-b".to_owned(),
                    session: "work".to_owned(),
                    size: None,
                    window: Some(format!("w{}", wid("work").0)),
                    build: None,
                },
            ],
            "attached clients only, sorted by client id"
        );
    }

    /// ⚠⚠⚠⚠⚠ **WHAT A WINDOW SAID IT WAS BUILT FROM REACHES THE LISTING, AND LEAVES WITH THE
    /// WINDOW** — register item 463's daemon-side half, which is the only half that can be asked
    /// about a process nobody resolved.
    ///
    /// Three facts, and each one is a defect somewhere else if it goes the other way:
    ///
    /// * **A client's several connections are ONE PROCESS.** A `sprag-gui` opens a request stream
    ///   and a long poll, and both say hello. A later hello carrying nothing must not ERASE what
    ///   the first one stated, or the answer would depend on which connection spoke last.
    /// * **Nothing said is not something said.** A client older than
    ///   [`sprag_rpc::CLIENT_BUILD_PARAM`] sends no build at all, and that has to reach the listing
    ///   as an absence — the comparison's fourth answer is built on it.
    /// * **A client id is a lifecycle token.** The build must die with the window, exactly as its
    ///   size, its view and its mailbox do; a stale one left behind would be reported as the build
    ///   of whoever takes that id next, which is this registry's standing capture hazard.
    #[test]
    fn what_a_window_said_it_was_built_from_is_listed_and_dies_with_the_window() {
        /// A build no image in this tree can be — twelve hex digits, the shape `build.rs` stamps.
        const A_BUILD: &str = "0000deadbeef";

        let mut reg = AttachmentRegistry::default();
        let (poll, request, quiet) = (conn(1), conn(2), conn(3));
        reg.hello(poll, "gui".to_owned(), Some(A_BUILD.to_owned()));
        reg.hello(request, "gui".to_owned(), None);
        reg.hello(quiet, "tui".to_owned(), None);
        reg.attach(request, "work".to_owned(), sid("work"), wid("work"));
        reg.attach(quiet, "work".to_owned(), sid("work"), wid("work"));

        let listed = reg.clients(|_, _| None);
        assert_eq!(
            listed
                .iter()
                .map(|c| c.build.as_deref())
                .collect::<Vec<_>>(),
            vec![Some(A_BUILD), None],
            "⚠⚠⚠ the window that stated a build is listed with it — from its FIRST connection, \
             which a second hello carrying nothing must not overwrite — and the one that stated \
             nothing is listed with nothing, because *did not say* is not *matches*",
        );

        // ── THE TOKEN IS REUSED, which is the capture every other per-client field refuses ──
        reg.disconnect(poll);
        reg.disconnect(request);
        let reborn = conn(4);
        reg.hello(reborn, "gui".to_owned(), None);
        reg.attach(reborn, "work".to_owned(), sid("work"), wid("work"));
        assert_eq!(
            reg.clients(|_, _| None)
                .iter()
                .map(|c| c.build.as_deref())
                .collect::<Vec<_>>(),
            vec![None, None],
            "⚠⚠⚠⚠⚠ A NEW WINDOW ON A REUSED ID INHERITS NOTHING. A build left behind would be \
             reported as this window's, which is worse than not knowing: it is a wrong answer to \
             the one question this field exists for",
        );
    }

    /// **A window's size comes from the clients WATCHING it, not from the session's.**
    ///
    /// The decision layer of R346, on the fixture the whole arc is about: a big client on one window
    /// and a small one on another, in one session. Before the split there was one list and the small
    /// client squeezed the big one's panes; here each window sees only its own viewers.
    ///
    /// REVERT-PROOF: fold on the session alone and both windows answer both areas; drop the
    /// [`watch`](AttachmentRegistry::watch) write and the small client stays on the landing window,
    /// so the second window answers nothing at all.
    #[test]
    fn a_windows_sizes_are_the_clients_looking_at_that_window() {
        let mut reg = AttachmentRegistry::default();
        let (zero, one) = (WindowId(10), WindowId(11));
        let (desk, phone) = (conn(1), conn(2));
        let (big, small) = (
            ClientSize {
                cols: 100,
                rows: 30,
            },
            ClientSize { cols: 60, rows: 20 },
        );

        reg.hello(desk, "desk".to_owned(), None);
        reg.hello(phone, "phone".to_owned(), None);
        reg.attach(desk, "work".to_owned(), sid("work"), zero);
        reg.attach(phone, "work".to_owned(), sid("work"), zero);
        reg.size(desk, big);
        reg.size(phone, small);

        // THE STATE THE COMPLAINT IS ABOUT: both clients landed on the same window, so both areas
        // arbitrate it and the smallest policy would collapse it onto the phone.
        assert_eq!(
            reg.sizes_for("work", zero),
            vec![big, small],
            "two clients on one window are two inputs to it",
        );
        assert_eq!(
            reg.sizes_for("work", one),
            Vec::new(),
            "and a window nobody is watching has no size to take",
        );

        // ...and now the phone goes to its own window.
        assert!(reg.watch(phone, one), "the phone moved");
        assert!(
            !reg.watch(phone, one),
            "re-selecting where it already is moves nothing"
        );
        assert_eq!(
            reg.sizes_for("work", zero),
            vec![big],
            "the window the phone LEFT is the desk's alone again — the whole point",
        );
        assert_eq!(
            reg.sizes_for("work", one),
            vec![small],
            "...and the one it went to is the phone's",
        );
        // AN IDEMPOTENT RE-ATTACH MUST NOT DRAG A CLIENT BACK. A display client re-sends
        // `client/attach` on a reconnect, and a landing that overwrote the seat every time would
        // take the phone off the window it chose the moment its poll thread re-declared itself.
        assert_eq!(
            reg.attach(phone, "work".to_owned(), sid("work"), zero),
            AttachOutcome::Unchanged,
        );
        assert_eq!(
            reg.sizes_for("work", one),
            vec![small],
            "re-declaring the session it is already on moves nobody's view",
        );

        // A WINDOW THAT DIES TAKES NOBODY'S SEAT WITH IT: the phone is put back on the landing
        // window rather than left sizing something that is gone.
        assert!(reg.reseat("work", &[zero], zero), "the phone was stranded");
        assert_eq!(
            reg.sizes_for("work", zero),
            vec![big, small],
            "a stranded client sizes the window it was moved to, not nothing",
        );
        assert!(
            !reg.reseat("work", &[zero], zero),
            "and a second pass over live seats moves nobody",
        );
    }

    #[test]
    fn a_size_needs_a_hello_and_is_announced_only_when_it_moves() {
        let mut reg = AttachmentRegistry::default();
        let c = conn(1);
        let size = ClientSize { cols: 80, rows: 24 };
        assert_eq!(
            reg.size(c, size),
            SizeOutcome::NoClient,
            "a connection that never said hello has no client to size"
        );
        reg.hello(c, "tui".to_owned(), None);
        assert_eq!(reg.size(c, size), SizeOutcome::Changed, "the first report");
        assert_eq!(
            reg.size(c, size),
            SizeOutcome::Unchanged,
            "the same numbers again move no window, so nothing is announced"
        );
        assert_eq!(
            reg.size(c, ClientSize { cols: 80, rows: 25 }),
            SizeOutcome::Changed,
            "one row is a different window"
        );
    }

    #[test]
    fn sizes_are_the_attached_clients_own_areas_oldest_first() {
        let mut reg = AttachmentRegistry::default();
        let (big, small, elsewhere, silent) = (conn(1), conn(2), conn(3), conn(4));
        for (c, name) in [
            (big, "big"),
            (small, "small"),
            (elsewhere, "elsewhere"),
            (silent, "silent"),
        ] {
            reg.hello(c, name.to_owned(), None);
        }
        reg.attach(big, "work".to_owned(), sid("work"), wid("work"));
        reg.attach(small, "work".to_owned(), sid("work"), wid("work"));
        reg.attach(elsewhere, "home".to_owned(), sid("home"), wid("home"));
        reg.attach(silent, "work".to_owned(), sid("work"), wid("work"));
        reg.size(
            big,
            ClientSize {
                cols: 120,
                rows: 40,
            },
        );
        reg.size(small, ClientSize { cols: 80, rows: 24 });
        // A client of ANOTHER session must not reach this session's arbitration, and one that never
        // reported must not appear as a zero — a `smallest` policy handed 0x0 would collapse every
        // pane in the session.
        reg.size(
            elsewhere,
            ClientSize {
                cols: 200,
                rows: 60,
            },
        );

        assert_eq!(
            reg.sizes_for("work", wid("work")),
            vec![
                ClientSize {
                    cols: 120,
                    rows: 40
                },
                ClientSize { cols: 80, rows: 24 },
            ],
            "this session's reporters only, in report order"
        );
        assert_eq!(
            reg.sizes_for("nobody", wid("nobody")),
            Vec::new(),
            "an unviewed session"
        );
    }

    #[test]
    fn the_recency_order_follows_the_latest_report_and_a_real_attach() {
        let mut reg = AttachmentRegistry::default();
        let (a, b) = (conn(1), conn(2));
        reg.hello(a, "a".to_owned(), None);
        reg.hello(b, "b".to_owned(), None);
        reg.attach(a, "work".to_owned(), sid("work"), wid("work"));
        reg.attach(b, "work".to_owned(), sid("work"), wid("work"));
        reg.size(
            a,
            ClientSize {
                cols: 100,
                rows: 30,
            },
        );
        reg.size(b, ClientSize { cols: 80, rows: 24 });
        assert_eq!(
            reg.sizes_for("work", wid("work")).last(),
            Some(&ClientSize { cols: 80, rows: 24 }),
            "b reported last"
        );

        // A window change on `a` makes it the most recent again — this is what a user resizing
        // their terminal means by "latest".
        reg.size(a, ClientSize { cols: 90, rows: 30 });
        assert_eq!(
            reg.sizes_for("work", wid("work")).last(),
            Some(&ClientSize { cols: 90, rows: 30 }),
            "a moved last"
        );

        // An IDEMPOTENT re-attach must not reorder: a client re-declaring the session it is already
        // on has not moved, and letting it take the window would make a harmless re-send steal the
        // size from the client the user just resized.
        reg.attach(b, "work".to_owned(), sid("work"), wid("work"));
        assert_eq!(
            reg.sizes_for("work", wid("work")).last(),
            Some(&ClientSize { cols: 90, rows: 30 }),
            "an unchanged attach leaves the order alone"
        );

        // A real SWITCH does reorder: the client just arrived at this session.
        reg.attach(b, "home".to_owned(), sid("home"), wid("home"));
        reg.attach(b, "work".to_owned(), sid("work"), wid("work"));
        assert_eq!(
            reg.sizes_for("work", wid("work")).last(),
            Some(&ClientSize { cols: 80, rows: 24 }),
            "b attached most recently"
        );
    }

    #[test]
    fn a_departed_clients_area_stops_arbitrating() {
        let mut reg = AttachmentRegistry::default();
        let (stays, leaves) = (conn(1), conn(2));
        reg.hello(stays, "stays".to_owned(), None);
        reg.hello(leaves, "leaves".to_owned(), None);
        reg.attach(stays, "work".to_owned(), sid("work"), wid("work"));
        reg.attach(leaves, "work".to_owned(), sid("work"), wid("work"));
        reg.size(
            stays,
            ClientSize {
                cols: 120,
                rows: 40,
            },
        );
        reg.size(leaves, ClientSize { cols: 80, rows: 24 });
        reg.disconnect(leaves);
        assert_eq!(
            reg.sizes_for("work", wid("work")),
            vec![ClientSize {
                cols: 120,
                rows: 40
            }],
            "a closed client's area would keep the window small forever"
        );
    }

    #[test]
    fn clients_drops_a_client_when_its_last_connection_closes() {
        let mut reg = AttachmentRegistry::default();
        let c = conn(1);
        reg.hello(c, "gui".to_owned(), None);
        reg.attach(c, "work".to_owned(), sid("work"), wid("work"));
        assert_eq!(
            reg.clients(|_, _| None).len(),
            1,
            "the attached client is listed"
        );
        reg.disconnect(c);
        assert!(
            reg.clients(|_, _| None).is_empty(),
            "the released client leaves the listing"
        );
    }

    /// A session RENAME carries every attachment with it: nobody stopped looking at anything, so
    /// the viewer count must not fall to zero on the renamed session and `list-clients` must not go
    /// on naming one the registry no longer holds.
    #[test]
    fn attachments_follow_a_renamed_session() {
        let mut reg = AttachmentRegistry::default();
        let (a, b, elsewhere) = (ConnId::allocate(), ConnId::allocate(), ConnId::allocate());
        reg.hello(a, "client-a".to_owned(), None);
        reg.hello(b, "client-b".to_owned(), None);
        reg.hello(elsewhere, "client-c".to_owned(), None);
        reg.attach(a, "work".to_owned(), sid("work"), wid("work"));
        reg.attach(b, "work".to_owned(), sid("work"), wid("work"));
        reg.attach(elsewhere, "play".to_owned(), sid("play"), wid("play"));

        assert_eq!(reg.rename_session("work", "prod"), 2, "both viewers moved");

        assert_eq!(reg.attached_count("prod"), 2, "the badge follows the name");
        assert_eq!(
            reg.attached_count("work"),
            0,
            "and nothing is left attached to a name no session answers to",
        );
        assert_eq!(
            reg.attached_count("play"),
            1,
            "control: another session's viewer is untouched",
        );
        assert_eq!(
            reg.rename_session("ghost", "x"),
            0,
            "a session nobody views moves no attachment",
        );
    }

    /// A session's DEATH releases its viewers — the twin of the rename above, and the half that was
    /// missing. An attachment left behind names a session the registry no longer holds, and the
    /// next session to take that name inherits the viewer.
    #[test]
    fn a_killed_sessions_viewers_are_released_and_cannot_be_inherited() {
        let mut reg = AttachmentRegistry::default();
        let (a, b, elsewhere) = (ConnId::allocate(), ConnId::allocate(), ConnId::allocate());
        for (conn, client) in [(a, "client-a"), (b, "client-b"), (elsewhere, "client-c")] {
            reg.hello(conn, client.to_owned(), None);
        }
        reg.attach(a, "work".to_owned(), sid("work"), wid("work"));
        reg.attach(b, "work".to_owned(), sid("work"), wid("work"));
        reg.attach(elsewhere, "play".to_owned(), sid("play"), wid("play"));
        reg.size(
            a,
            ClientSize {
                cols: 120,
                rows: 40,
            },
        );

        assert_eq!(reg.session_ended("work"), 2, "both viewers were released");
        assert_eq!(reg.attached_count("work"), 0);
        assert_eq!(
            reg.attached_count("play"),
            1,
            "control: another session's viewer is untouched",
        );
        assert!(
            reg.clients(|_, _| None)
                .iter()
                .all(|info| info.session == "play"),
            "and `list-clients` stops naming a session the registry no longer holds",
        );

        // THE INHERITANCE, which is what a lingering attachment actually costs: a NEW session takes
        // the freed name and must find no viewers waiting for it.
        assert_eq!(
            reg.attached_count("work"),
            0,
            "a fresh session of the same name inherits nobody",
        );

        // The client's SIZE stays — it describes that client's surface, not the dead session — so a
        // switch to another session arbitrates with it at once. Asserted through the public reading
        // rather than the field: re-attaching `a` elsewhere must bring its area with it.
        reg.attach(a, "play".to_owned(), sid("play"), wid("play"));
        assert!(
            reg.sizes_for("play", wid("play")).contains(&ClientSize {
                cols: 120,
                rows: 40
            }),
            "a released client keeps the area it reported",
        );

        assert_eq!(
            reg.session_ended("ghost"),
            0,
            "a session nobody views releases nothing",
        );
    }

    /// A stand-in for `SessionRegistry::name_of`: what each id is CALLED right now, or nothing when
    /// that session is gone.
    ///
    /// It is a `Vec` of pairs rather than a map from names, because the three things the history
    /// must tell apart are all expressed as moves of one against the other: a RENAME gives an
    /// existing id a new name, a KILL removes an id, and a REISSUE gives an old NAME to a new id.
    /// A fixture keyed by name could not state the third at all — which is the defect.
    #[derive(Default)]
    struct Sessions(Vec<(SessionId, String)>);

    impl Sessions {
        fn born(&mut self, id: u64, name: &str) -> SessionId {
            let id = SessionId(id);
            self.0.push((id, name.to_owned()));
            id
        }

        fn renamed(&mut self, id: SessionId, to: &str) {
            for entry in &mut self.0 {
                if entry.0 == id {
                    entry.1 = to.to_owned();
                }
            }
        }

        fn killed(&mut self, id: SessionId) {
            self.0.retain(|entry| entry.0 != id);
        }

        /// The resolver `last_viewed` walks — liveness and the current name in one answer, which is
        /// what the real registry's `name_of` gives and the whole reason the history holds ids.
        fn name_of(&self) -> impl Fn(SessionId) -> Option<String> + '_ {
            move |id| {
                self.0
                    .iter()
                    .find(|entry| entry.0 == id)
                    .map(|entry| entry.1.clone())
            }
        }
    }

    /// A revisit MOVES the entry it already has rather than adding another — the dedup that is this
    /// history's only bound, so a client toggling between two sessions all day holds two entries.
    ///
    /// Asserted through the resolver's own call count, because that is the only place the length is
    /// observable from outside: a history that grew per visit would resolve the same session again
    /// and again on one walk.
    #[test]
    fn a_revisit_moves_one_entry_rather_than_adding_another() {
        let mut sessions = Sessions::default();
        let (a, b) = (sessions.born(1, "alpha"), sessions.born(2, "beta"));
        let mut reg = AttachmentRegistry::default();
        let conn = conn(1);
        reg.hello(conn, "gui".to_owned(), None);
        for _ in 0..8 {
            reg.attach(conn, "alpha".to_owned(), a, wid("alpha"));
            reg.attach(conn, "beta".to_owned(), b, wid("beta"));
        }

        let walked = std::cell::RefCell::new(Vec::new());
        let counting = |id: SessionId| {
            walked.borrow_mut().push(id);
            sessions.name_of()(id)
        };
        assert_eq!(
            reg.last_viewed(conn, counting, false),
            Some((a, "alpha".to_owned())),
        );
        assert_eq!(
            *walked.borrow(),
            vec![b, a],
            "sixteen visits to two sessions are two entries, newest first",
        );
    }

    /// A client that has been around goes back to the session it was on BEFORE this one — not to
    /// the one it is on, and not to the one before that.
    #[test]
    fn the_last_viewed_session_is_the_most_recent_other_one() {
        let mut sessions = Sessions::default();
        let (a, b, c) = (
            sessions.born(1, "alpha"),
            sessions.born(2, "beta"),
            sessions.born(3, "gamma"),
        );
        let mut reg = AttachmentRegistry::default();
        let conn = conn(1);
        reg.hello(conn, "gui".to_owned(), None);
        reg.attach(conn, "alpha".to_owned(), a, wid("alpha"));
        reg.attach(conn, "beta".to_owned(), b, wid("beta"));
        reg.attach(conn, "gamma".to_owned(), c, wid("gamma"));

        assert_eq!(
            reg.last_viewed(conn, sessions.name_of(), false),
            Some((b, "beta".to_owned())),
            "the one it was on before this one",
        );

        // Going back is itself a visit, so the answer moves with it — tmux's `switch-client -l`
        // toggles, and this is why.
        reg.attach(conn, "beta".to_owned(), b, wid("beta"));
        assert_eq!(
            reg.last_viewed(conn, sessions.name_of(), false),
            Some((c, "gamma".to_owned())),
            "and the visit that went back is itself the newest visit",
        );
    }

    /// A client that never went anywhere else has nowhere to go back to — and neither does one
    /// whose whole history has been killed. `None` is an ANSWER here, not a failure.
    #[test]
    fn a_client_with_no_other_live_session_has_no_last_viewed() {
        let mut sessions = Sessions::default();
        let (a, b) = (sessions.born(1, "alpha"), sessions.born(2, "beta"));
        let mut reg = AttachmentRegistry::default();
        let conn = conn(1);
        reg.hello(conn, "gui".to_owned(), None);
        reg.attach(conn, "alpha".to_owned(), a, wid("alpha"));
        assert_eq!(
            reg.last_viewed(conn, sessions.name_of(), false),
            None,
            "a client that never switched",
        );

        // The CONTROL: with a second visit it does have one, so the `None` above is about the
        // history and not about a resolver that answers nothing.
        reg.attach(conn, "beta".to_owned(), b, wid("beta"));
        reg.attach(conn, "alpha".to_owned(), a, wid("alpha"));
        assert_eq!(
            reg.last_viewed(conn, sessions.name_of(), false),
            Some((b, "beta".to_owned())),
        );

        sessions.killed(b);
        assert_eq!(
            reg.last_viewed(conn, sessions.name_of(), false),
            None,
            "everything else it visited is gone",
        );
    }

    /// **The round's claim.** A session the client visited is RENAMED and a new session takes the
    /// freed name. Going back must land on the session it actually visited — under whatever that
    /// session is called now — and never on the stranger wearing its old name.
    ///
    /// The fixture is built so the two answers DISAGREE: the impostor exists, is live, and is the
    /// exact string a history of names would have matched. MEASURED before the fix, through a real
    /// client against a real daemon: it landed on the impostor.
    #[test]
    fn a_renamed_session_is_still_the_one_to_go_back_to_and_an_impostor_never_is() {
        let mut sessions = Sessions::default();
        let (work, here) = (sessions.born(1, "work"), sessions.born(2, "here"));
        let mut reg = AttachmentRegistry::default();
        let conn = conn(1);
        reg.hello(conn, "gui".to_owned(), None);
        reg.attach(conn, "work".to_owned(), work, wid("work"));
        reg.attach(conn, "here".to_owned(), here, wid("here"));

        sessions.renamed(work, "renamed");
        let impostor = sessions.born(3, "work");
        assert_ne!(
            impostor, work,
            "the fixture must DISAGREE with itself, or this test could not fail",
        );

        assert_eq!(
            reg.last_viewed(conn, sessions.name_of(), false),
            Some((work, "renamed".to_owned())),
            "the session it visited, by identity, under the name that session has now",
        );
    }

    /// tmux `detach-on-destroy no-detached`: the most recent session it viewed that NO OTHER client
    /// is viewing. Exact here, because this registry IS the attachment map.
    #[test]
    fn the_unattached_filter_skips_a_session_another_client_is_viewing() {
        let mut sessions = Sessions::default();
        let (a, b, c) = (
            sessions.born(1, "alpha"),
            sessions.born(2, "beta"),
            sessions.born(3, "gamma"),
        );
        let mut reg = AttachmentRegistry::default();
        let mine = conn(1);
        reg.hello(mine, "gui".to_owned(), None);
        for (name, id) in [("alpha", a), ("beta", b), ("gamma", c)] {
            reg.attach(mine, name.to_owned(), id, wid(name));
        }
        // Somebody else joins `beta`, the one I would otherwise go back to.
        let theirs = conn(2);
        reg.hello(theirs, "tui".to_owned(), None);
        reg.attach(theirs, "beta".to_owned(), b, wid("beta"));

        assert_eq!(
            reg.last_viewed(mine, sessions.name_of(), false),
            Some((b, "beta".to_owned())),
            "unfiltered, an occupied session is still where I was",
        );
        assert_eq!(
            reg.last_viewed(mine, sessions.name_of(), true),
            Some((a, "alpha".to_owned())),
            "filtered, it is skipped for the next one nobody holds",
        );
    }

    /// The history's own garbage collection: an id that no longer resolves is dropped as the walk
    /// passes it, because an id is never reissued and so can never come back.
    ///
    /// Asserted through the resolver's own call count rather than the private field — the claim is
    /// that a dead session is not walked TWICE, which is what "pruned" means to a caller.
    #[test]
    fn resolving_prunes_the_sessions_that_are_gone() {
        let mut sessions = Sessions::default();
        let (a, b, c) = (
            sessions.born(1, "alpha"),
            sessions.born(2, "beta"),
            sessions.born(3, "gamma"),
        );
        let mut reg = AttachmentRegistry::default();
        let conn = conn(1);
        reg.hello(conn, "gui".to_owned(), None);
        for (name, id) in [("alpha", a), ("beta", b), ("gamma", c)] {
            reg.attach(conn, name.to_owned(), id, wid(name));
        }
        sessions.killed(a);
        sessions.killed(b);

        let asked = std::cell::RefCell::new(Vec::new());
        let counting = |id: SessionId| {
            asked.borrow_mut().push(id);
            sessions.name_of()(id)
        };
        assert_eq!(reg.last_viewed(conn, counting, false), None);
        assert_eq!(
            asked.borrow().len(),
            3,
            "the first walk sees the whole history",
        );

        asked.borrow_mut().clear();
        let counting = |id: SessionId| {
            asked.borrow_mut().push(id);
            sessions.name_of()(id)
        };
        assert_eq!(reg.last_viewed(conn, counting, false), None);
        assert_eq!(
            *asked.borrow(),
            vec![c],
            "the second walk sees only what is left — the dead ids were dropped",
        );
    }

    /// A client id is a lifecycle token, not identity: the next client to hold one is a different
    /// client, and inheriting somebody else's "go back" would be this round's own defect in another
    /// key.
    #[test]
    fn a_departed_clients_history_does_not_outlive_it() {
        let mut sessions = Sessions::default();
        let (a, b) = (sessions.born(1, "alpha"), sessions.born(2, "beta"));
        let mut reg = AttachmentRegistry::default();
        let first = conn(1);
        reg.hello(first, "gui".to_owned(), None);
        reg.attach(first, "alpha".to_owned(), a, wid("alpha"));
        reg.attach(first, "beta".to_owned(), b, wid("beta"));
        assert_eq!(
            reg.last_viewed(first, sessions.name_of(), false),
            Some((a, "alpha".to_owned())),
            "the CONTROL: it had a history while it was here",
        );
        reg.disconnect(first);

        let second = conn(2);
        reg.hello(second, "gui".to_owned(), None); // the SAME token, a new client
        reg.attach(second, "beta".to_owned(), b, wid("beta"));
        assert_eq!(
            reg.last_viewed(second, sessions.name_of(), false),
            None,
            "a fresh client starts with nowhere to go back to",
        );
    }
}
