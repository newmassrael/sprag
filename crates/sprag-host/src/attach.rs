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
use std::collections::HashMap;

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
    /// Each present client's reported area (from `client/size`), stamped with its recency.
    ///
    /// Keyed by CLIENT rather than by connection because a client's several connections describe
    /// one surface. Independent of `client_session`: a client may report a size before it attaches
    /// (which is the order both frontends use, so the first arbitration already counts it) and an
    /// attached client may never report one.
    client_size: HashMap<ClientId, Reported>,
    /// The stamp the next report takes. Monotone for the life of the daemon — it orders reports,
    /// it does not count them, so wrapping is not a concern at one per window change.
    next_ordinal: u64,
}

impl AttachmentRegistry {
    /// Associate `conn` with the `client` it belongs to (`client/hello`). Idempotent; every
    /// connection of a client calls this once so the client stays present while any is live.
    pub fn hello(&mut self, conn: ConnId, client: ClientId) {
        self.conn_client.insert(conn, client);
    }

    /// Attach (or switch — tmux `switch-client`) the client owning `conn` to `session`. The
    /// connection must have said hello first; otherwise [`AttachOutcome::NoClient`].
    pub fn attach(&mut self, conn: ConnId, session: String) -> AttachOutcome {
        let Some(client) = self.conn_client.get(&conn) else {
            return AttachOutcome::NoClient;
        };
        let client = client.clone();
        match self.client_session.get(&client) {
            Some(prev) if *prev == session => AttachOutcome::Unchanged,
            _ => {
                // An attach makes this client the most recent one, which is what `window-size
                // latest` reads. Only on a real change: an idempotent re-send says nothing new, and
                // reordering on it would let a client that merely re-declared its session take the
                // window from one the user had just resized.
                self.restamp(&client);
                let previous = self.client_session.insert(client, session);
                AttachOutcome::Changed { previous }
            }
        }
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

    /// Every area reported by a client attached to `session`, OLDEST FIRST — so the last element is
    /// the most recent report, which is what `window-size latest` names.
    ///
    /// Clients that never reported an area are absent rather than present with a zero: a policy
    /// taking the smallest attached client must not be handed a size nobody has.
    #[must_use]
    pub fn sizes(&self, session: &str) -> Vec<ClientSize> {
        let mut reported: Vec<Reported> = self
            .client_session
            .iter()
            .filter(|(_, viewing)| viewing.as_str() == session)
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
        self.client_session.remove(&client)
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
    pub fn clients(&self) -> Vec<ClientInfo> {
        let mut clients: Vec<ClientInfo> = self
            .client_session
            .iter()
            .map(|(client, session)| ClientInfo {
                client: client.clone(),
                session: session.clone(),
                size: self.client_size.get(client).map(|held| held.size),
            })
            .collect();
        clients.sort_by(|a, b| {
            a.client
                .cmp(&b.client)
                .then_with(|| a.session.cmp(&b.session))
        });
        clients
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

    #[test]
    fn hello_then_attach_counts_one() {
        let mut reg = AttachmentRegistry::default();
        let c = conn(1);
        reg.hello(c, "client-a".to_owned());
        assert_eq!(
            reg.attach(c, "work".to_owned()),
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
        reg.hello(poll, "gui".to_owned());
        reg.hello(request, "gui".to_owned());
        reg.attach(request, "work".to_owned());
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
        reg.hello(c, "client-a".to_owned());
        assert_eq!(
            reg.attach(c, "work".to_owned()),
            AttachOutcome::Changed { previous: None }
        );
        assert_eq!(
            reg.attach(c, "work".to_owned()),
            AttachOutcome::Unchanged,
            "an idempotent re-send moves no count"
        );
    }

    #[test]
    fn switch_moves_the_count_between_sessions() {
        let mut reg = AttachmentRegistry::default();
        let c = conn(1);
        reg.hello(c, "client-a".to_owned());
        reg.attach(c, "one".to_owned());
        assert_eq!(
            reg.attach(c, "two".to_owned()),
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
            reg.attach(conn(1), "work".to_owned()),
            AttachOutcome::NoClient
        );
        assert_eq!(reg.attached_count("work"), 0);
    }

    #[test]
    fn disconnect_of_the_only_connection_releases_the_client() {
        let mut reg = AttachmentRegistry::default();
        let c = conn(1);
        reg.hello(c, "client-a".to_owned());
        reg.attach(c, "work".to_owned());
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
        reg.hello(poll, "gui".to_owned());
        reg.hello(request, "gui".to_owned());
        reg.attach(request, "work".to_owned());
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
        reg.hello(a, "client-a".to_owned());
        reg.hello(b, "client-b".to_owned());
        reg.attach(a, "work".to_owned());
        reg.attach(b, "work".to_owned());
        assert_eq!(reg.attached_count("work"), 2, "two windows, two viewers");
        reg.disconnect(a);
        assert_eq!(reg.attached_count("work"), 1, "one left, one remains");
    }

    #[test]
    fn disconnect_of_a_hello_only_connection_moves_nothing() {
        let mut reg = AttachmentRegistry::default();
        let c = conn(1);
        reg.hello(c, "client-a".to_owned());
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
        reg.hello(a, "client-b".to_owned());
        reg.hello(b, "client-a".to_owned());
        reg.hello(hello_only, "client-c".to_owned());
        reg.attach(a, "work".to_owned());
        reg.attach(b, "home".to_owned());
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
        let clients = reg.clients();
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
                },
                ClientInfo {
                    client: "client-b".to_owned(),
                    session: "work".to_owned(),
                    size: None,
                },
            ],
            "attached clients only, sorted by client id"
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
        reg.hello(c, "tui".to_owned());
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
            reg.hello(c, name.to_owned());
        }
        reg.attach(big, "work".to_owned());
        reg.attach(small, "work".to_owned());
        reg.attach(elsewhere, "home".to_owned());
        reg.attach(silent, "work".to_owned());
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
            reg.sizes("work"),
            vec![
                ClientSize {
                    cols: 120,
                    rows: 40
                },
                ClientSize { cols: 80, rows: 24 },
            ],
            "this session's reporters only, in report order"
        );
        assert_eq!(reg.sizes("nobody"), Vec::new(), "an unviewed session");
    }

    #[test]
    fn the_recency_order_follows_the_latest_report_and_a_real_attach() {
        let mut reg = AttachmentRegistry::default();
        let (a, b) = (conn(1), conn(2));
        reg.hello(a, "a".to_owned());
        reg.hello(b, "b".to_owned());
        reg.attach(a, "work".to_owned());
        reg.attach(b, "work".to_owned());
        reg.size(
            a,
            ClientSize {
                cols: 100,
                rows: 30,
            },
        );
        reg.size(b, ClientSize { cols: 80, rows: 24 });
        assert_eq!(
            reg.sizes("work").last(),
            Some(&ClientSize { cols: 80, rows: 24 }),
            "b reported last"
        );

        // A window change on `a` makes it the most recent again — this is what a user resizing
        // their terminal means by "latest".
        reg.size(a, ClientSize { cols: 90, rows: 30 });
        assert_eq!(
            reg.sizes("work").last(),
            Some(&ClientSize { cols: 90, rows: 30 }),
            "a moved last"
        );

        // An IDEMPOTENT re-attach must not reorder: a client re-declaring the session it is already
        // on has not moved, and letting it take the window would make a harmless re-send steal the
        // size from the client the user just resized.
        reg.attach(b, "work".to_owned());
        assert_eq!(
            reg.sizes("work").last(),
            Some(&ClientSize { cols: 90, rows: 30 }),
            "an unchanged attach leaves the order alone"
        );

        // A real SWITCH does reorder: the client just arrived at this session.
        reg.attach(b, "home".to_owned());
        reg.attach(b, "work".to_owned());
        assert_eq!(
            reg.sizes("work").last(),
            Some(&ClientSize { cols: 80, rows: 24 }),
            "b attached most recently"
        );
    }

    #[test]
    fn a_departed_clients_area_stops_arbitrating() {
        let mut reg = AttachmentRegistry::default();
        let (stays, leaves) = (conn(1), conn(2));
        reg.hello(stays, "stays".to_owned());
        reg.hello(leaves, "leaves".to_owned());
        reg.attach(stays, "work".to_owned());
        reg.attach(leaves, "work".to_owned());
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
            reg.sizes("work"),
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
        reg.hello(c, "gui".to_owned());
        reg.attach(c, "work".to_owned());
        assert_eq!(reg.clients().len(), 1, "the attached client is listed");
        reg.disconnect(c);
        assert!(
            reg.clients().is_empty(),
            "the released client leaves the listing"
        );
    }
}
