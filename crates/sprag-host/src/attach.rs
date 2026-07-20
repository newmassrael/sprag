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

/// One attached client, for the `clients` wire slot (tmux `list-clients`): the opaque client id
/// and the session it is currently viewing. The daemon's honest analog of tmux's `struct client`
/// row — sprag has no tty/size/activity to report (a `sprag-gui` window is not a terminal the
/// daemon owns), so this carries only what the [`AttachmentRegistry`] actually knows.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ClientInfo {
    /// The opaque, client-minted id (e.g. a `sprag-gui` window's `gui-{pid}-{nanos}`).
    pub client: ClientId,
    /// The session this client is attached to (tmux `client -> session`).
    pub session: String,
}

/// What an [`attach`](AttachmentRegistry::attach) did, so the caller knows whether the per-session
/// counts moved (and the scene must be bumped so other clients' long-polls re-read the badge).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AttachOutcome {
    /// The connection never sent [`crate::wire::CLIENT_HELLO_METHOD`] — a protocol error; the
    /// caller refuses the request rather than inventing a client.
    NoClient,
    /// The client's attached session changed (a first attach, or a switch): the counts moved.
    Changed,
    /// The client was already attached to that session: an idempotent re-send, counts unmoved.
    Unchanged,
}

/// The daemon's `conn -> client -> attached session` map (see the module docs).
#[derive(Default, Debug)]
pub struct AttachmentRegistry {
    /// Which client each LIVE connection belongs to (from `client/hello`). A connection is in
    /// here from its hello until its close; a client is PRESENT while any of its connections is.
    conn_client: HashMap<ConnId, ClientId>,
    /// Which session each present, ATTACHED client is viewing (from `client/attach`). A client
    /// appears only after it attaches; every entry is a present client (removed when its last
    /// connection closes), so the values ARE the live attachments.
    client_session: HashMap<ClientId, String>,
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
                self.client_session.insert(client, session);
                AttachOutcome::Changed
            }
        }
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
        assert_eq!(reg.attach(c, "work".to_owned()), AttachOutcome::Changed);
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
        assert_eq!(reg.attach(c, "work".to_owned()), AttachOutcome::Changed);
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
        assert_eq!(reg.attach(c, "two".to_owned()), AttachOutcome::Changed);
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
        // client-c said hello but never attached: it views nothing, so it is NOT listed.
        let clients = reg.clients();
        assert_eq!(
            clients,
            vec![
                ClientInfo {
                    client: "client-a".to_owned(),
                    session: "home".to_owned(),
                },
                ClientInfo {
                    client: "client-b".to_owned(),
                    session: "work".to_owned(),
                },
            ],
            "attached clients only, sorted by client id"
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
