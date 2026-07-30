//! The client half of the transport: a blocking JSON-RPC connection to a host
//! socket.
//!
//! [`mount`](crate::mount) is the SERVER end (bind + accept + dispatch). This is
//! the CLIENT end a display client (`sprag-gui`'s `WireHost`) drives: connect to a
//! `sprag-term` host's always-on Unix socket and issue newline-delimited JSON-RPC
//! requests, reading one response line per request. The host serves each
//! connection on its own handler thread and funnels every frame into ONE dispatch
//! owner, so a client may hold SEVERAL [`HostConn`]s concurrently (e.g. one parked
//! on a long-poll `scene/waitFor` while another issues cell reads) without
//! head-of-line blocking — each connection is an independent request/response
//! stream.
//!
//! A [`HostConn`] is single-threaded (one outstanding request at a time): the
//! transport is strictly request→response (no server push — an async
//! `scene/waitFor` is a *deferred response* to the client's own request, still one
//! reply per request), so a connection never desyncs its read stream. A caller
//! that needs concurrency uses more connections, not shared mutable access.

use std::io::{self, BufRead, BufReader, ErrorKind, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::thread::sleep;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// The JSON-RPC `params` key naming the SESSION a request is scoped to — the out-of-band
/// scope param that "one daemon holds every session" needs, so a request says which session
/// it is about.
///
/// It is defined HERE, in the transport client that WRITES it ([`HostConn::scope_to`] merges
/// it into every request), and re-exported by the host that READS it
/// (`sprag_host::wire::SESSION_PARAM`), so the two ends of the wire share ONE spelling and
/// cannot drift. The host's own doc records the contract it enforces (absent → the default
/// session, a string → that session, anything else → refused whole).
pub const SESSION_PARAM: &str = "session";

/// The JSON-RPC method a connection sends ONCE to announce which CLIENT it belongs to
/// (R-PR67 Stage 1) — `params: { "client": "<opaque client id>" }`.
///
/// A single logical client (one `sprag-gui` window) opens SEVERAL connections (its request
/// stream and its long-poll) to avoid head-of-line blocking; each announces the same client
/// id so the host groups them into ONE attached client rather than counting the connections.
/// The id is opaque and client-minted (a lifecycle token, not identity); the host keys its
/// per-client attachment state on it and releases a client when its LAST connection closes
/// (via the transport's `on_disconnect`, the crash-safe path). Intercepted host-side before
/// the generic dispatch core, since it needs the frame's connection id, which no scene
/// external sees. Defined HERE (the writer) and read by the host, like [`SESSION_PARAM`].
pub const CLIENT_HELLO_METHOD: &str = "client/hello";

/// The JSON-RPC method a connection sends to declare (or CHANGE — tmux `switch-client`) the
/// session its client is attached to (R-PR67 Stage 1) — `params: { "session": "<name>" }`,
/// reusing [`SESSION_PARAM`], so an unknown session is refused by the same scope check every
/// other request uses. The calling connection must have sent [`CLIENT_HELLO_METHOD`] first;
/// the host attributes the attachment to that connection's client.
pub const CLIENT_ATTACH_METHOD: &str = "client/attach";

/// The [`CLIENT_HELLO_METHOD`] params key carrying the opaque client id.
pub const CLIENT_PARAM: &str = "client";

/// The JSON-RPC method a connection sends to report the cell area its client can give a window —
/// `params: { "cols": <u16>, "rows": <u16> }` — once when it attaches and again on every window
/// change. The calling connection must have sent [`CLIENT_HELLO_METHOD`] first.
///
/// This is what makes tmux's `window-size` answerable: the size a WINDOW takes is a fact about
/// every client attached to its session, so the daemon has to hold each client's own area before
/// it can arbitrate between them. Reported rather than inferred, because only the client knows
/// what it has — a terminal's winsize, or a GUI window's pixels divided by its font metric — and
/// the daemon owns neither.
///
/// A size is a CLIENT's, not a connection's: a client's several connections describe one surface,
/// and the last report wins for all of them. It is also not the same fact as
/// [`CLIENT_ATTACH_METHOD`]'s session, which is why it is its own method — a window change moves
/// the size without touching the attachment, and re-declaring an attachment to say so would make
/// every resize look like a `switch-client`.
pub const CLIENT_SIZE_METHOD: &str = "client/size";

/// The [`CLIENT_SIZE_METHOD`] params key carrying the client's width in cells.
pub const COLS_PARAM: &str = "cols";

/// The [`CLIENT_SIZE_METHOD`] params key carrying the client's height in cells.
pub const ROWS_PARAM: &str = "rows";

/// Mint a `sprag-gui` window's client id: `gui-<pid>-<launch nanos>`, process-unique and stable
/// for that window's whole life, shared by its request and poll connections.
///
/// Opaque TO THE HOST, which only ever groups connections by it — but not opaque to whoever
/// LAUNCHED the window, and that is why the shape lives here rather than in the GUI that mints it.
/// `sprag attach --no-wait` has to recognise the window IT spawned among every attached client, and
/// it knows only the pid it got back from the spawn; matching on [`gui_client_prefix`] answers that
/// without inventing a second identity channel. Minted here and matched here, so the two halves
/// cannot drift the way they would with the format spelled out at each end (mirroring
/// [`SESSION_PARAM`] and `HOST_SOCKET_NAME`).
///
/// The nanos are what make it unique rather than merely distinct: a pid is recyclable, so two
/// GUIs launched far apart could share one, and the launch instant separates them.
#[must_use]
pub fn new_gui_client_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    format!("gui-{}-{nanos}", std::process::id())
}

/// The prefix every [`new_gui_client_id`] minted by process `pid` begins with — the test a launcher
/// applies to find ITS window in the daemon's client list.
///
/// The TRAILING DASH is load-bearing, not decoration: without it `gui-123` is a prefix of
/// `gui-1234-…`, so a launcher would accept a stranger's window whose pid merely starts with its
/// own digits and report success for a window that never came up.
#[must_use]
pub fn gui_client_prefix(pid: u32) -> String {
    format!("gui-{pid}-")
}

/// A blocking JSON-RPC connection to a host socket — the client end of the wire.
///
/// One request/response at a time (see the module docs). Construct with
/// [`connect`](Self::connect) (which tolerates the spawn race by retrying until
/// the socket accepts), then [`call`](Self::call) per request.
pub struct HostConn {
    /// The write half (requests out). A `UnixStream` is bidirectional; this clone
    /// owns writes while `reader` owns the buffered read half.
    writer: UnixStream,
    /// The buffered read half (newline-delimited responses in).
    reader: BufReader<UnixStream>,
    /// The next JSON-RPC request id. Monotonic; the server echoes it back.
    next_id: u64,
    /// The session every request on this connection is scoped to
    /// ([`SESSION_PARAM`]), or `None` for the default session. Set once by
    /// [`scope_to`](Self::scope_to) after the session is known, and merged into each
    /// request's params by [`call`](Self::call) — the ONE place scoping happens, so a
    /// client's several connections (its request stream and its long-poll) cannot address
    /// different sessions.
    session: Option<String>,
    /// Set once a read deadline expired mid-reply. See [`set_read_deadline`](Self::set_read_deadline)
    /// for why a timed-out connection can never be used again.
    timed_out: bool,
}

impl HostConn {
    /// Connect to the host socket at `path`, retrying until it accepts or `timeout`
    /// elapses — so a client that spawned its host tolerates the bind race (the
    /// child has not yet bound the socket at the instant the parent connects).
    ///
    /// # Errors
    ///
    /// Returns the last connect error if `timeout` elapses before the socket
    /// accepts, or an I/O error if the accepted stream cannot be split for reading.
    pub fn connect(path: &Path, timeout: Duration) -> io::Result<Self> {
        let start = Instant::now();
        loop {
            match UnixStream::connect(path) {
                Ok(stream) => return Self::from_stream(stream),
                Err(error) => {
                    if start.elapsed() >= timeout {
                        return Err(error);
                    }
                    sleep(Duration::from_millis(20));
                }
            }
        }
    }

    /// Wrap an already-connected stream (splitting it into read + write halves).
    fn from_stream(stream: UnixStream) -> io::Result<Self> {
        let reader = BufReader::new(stream.try_clone()?);
        Ok(Self {
            writer: stream,
            reader,
            next_id: 1,
            session: None,
            timed_out: false,
        })
    }

    /// Bound how long a [`call`](Self::call) on this connection may wait for its reply, or `None`
    /// (the default) to wait forever.
    ///
    /// Per connection, deliberately, because the two things a client does with one are opposites.
    /// A REQUEST connection asks a local daemon a question it answers immediately, so waiting
    /// without limit buys nothing and costs everything: the GUI issues these from its reducer, on
    /// the UI thread, and a daemon that accepts but never answers freezes the window for as long as
    /// it stays that way. A LONG-POLL connection parks on `scene/waitFor` precisely so it can wait
    /// indefinitely — a deadline there would be a bug, not a safeguard. One knob, set by whoever
    /// knows which kind of connection this is.
    ///
    /// A connection that trips the deadline is FINISHED: the reply may still arrive afterwards, and
    /// a `HostConn` carries one outstanding request at a time with no way to tell a late answer
    /// from the next one, so reading it later would attribute one call's result to another. Every
    /// subsequent `call` therefore fails immediately with [`ErrorKind::TimedOut`] and the owner must
    /// reconnect. Silently desynchronising would be far worse than a connection that says it is
    /// done.
    ///
    /// # Errors
    ///
    /// Fails if the socket rejects the timeout (which includes a zero `Duration`, since that means
    /// "block forever" to the OS and is never what a caller asking for a deadline meant).
    pub fn set_read_deadline(&mut self, deadline: Option<Duration>) -> io::Result<()> {
        self.reader.get_ref().set_read_timeout(deadline)
    }

    /// Scope every subsequent request on this connection to the session named `session`, by
    /// merging [`SESSION_PARAM`] into each request's params.
    ///
    /// A client learns its session name once (it attaches to a named one, or the daemon
    /// allocates one), then scopes ALL its connections to it — both the request connection
    /// and the long-poll — through this single seam, so no request can silently address a
    /// different session than its siblings. Idempotent and settable again, though a client
    /// scopes once at boot.
    pub fn scope_to(&mut self, session: impl Into<String>) {
        self.session = Some(session.into());
    }

    /// Merge this connection's session scope (if any) into a request's params. Only an object
    /// `params` can carry the key; every scoped request the wire client issues is object-shaped
    /// (`{"path": ..}`), so a non-object is passed through untouched rather than reshaped —
    /// carrying a scope on a request that has no place for it is not something the wire client
    /// ever needs.
    fn scoped(&self, params: Value) -> Value {
        match (&self.session, params) {
            (Some(session), Value::Object(mut map)) => {
                map.insert(SESSION_PARAM.to_owned(), Value::String(session.clone()));
                Value::Object(map)
            }
            (_, params) => params,
        }
    }

    /// A clone of the underlying stream usable ONLY to cancel a blocked
    /// [`call`](Self::call): another thread (typically a `Drop`) calls
    /// [`shutdown`](UnixStream::shutdown)`(Both)` on it to force this connection's
    /// in-flight blocking read to return, so a thread parked on a long-poll
    /// `scene/waitFor` unblocks deterministically instead of leaking. All clones name
    /// the same OS socket, so the shutdown reaches the reader half. Not for issuing
    /// requests (use [`call`](Self::call)).
    ///
    /// # Errors
    ///
    /// Fails if the stream cannot be duplicated (an exhausted fd table).
    pub fn shutdown_handle(&self) -> io::Result<UnixStream> {
        self.writer.try_clone()
    }

    /// Issue one `method` request with `params` and block until its response line
    /// arrives, returning the JSON-RPC `result` value (`Null` when absent). A
    /// JSON-RPC `error` object in the reply is surfaced as an [`io::Error`]; a
    /// closed connection (host gone) is [`ErrorKind::UnexpectedEof`].
    ///
    /// Blocking is the point for `scene/waitFor {since}`: the host parks that
    /// reply until a pane produces output, so this read blocks (cheaply) until the
    /// change-notification fires — the long-poll a wire client repaints off.
    ///
    /// # Errors
    ///
    /// I/O failure writing the request or reading the reply, a malformed reply, or
    /// a JSON-RPC `error` object in the response.
    pub fn call(&mut self, method: &str, params: Value) -> io::Result<Value> {
        if self.timed_out {
            return Err(io::Error::new(
                ErrorKind::TimedOut,
                "connection abandoned after a read deadline expired",
            ));
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": self.scoped(params),
        });
        writeln!(self.writer, "{request}")?;
        self.writer.flush()?;

        // Read the next non-blank response line (the server terminates each reply
        // with a newline; blank lines, if any, are skipped).
        let mut line = String::new();
        loop {
            line.clear();
            let read = self.reader.read_line(&mut line).inspect_err(|error| {
                // A deadline that expires here has left the reply stream at an unknown offset (the
                // partial line is already consumed), so the connection is retired rather than
                // retried — see `set_read_deadline`. Both spellings the platforms use for "the
                // timeout elapsed" mean the same thing to this loop.
                if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) {
                    self.timed_out = true;
                }
            })?;
            if read == 0 {
                return Err(io::Error::new(
                    ErrorKind::UnexpectedEof,
                    "host closed the connection",
                ));
            }
            if !line.trim().is_empty() {
                break;
            }
        }

        let response: Value = serde_json::from_str(line.trim())
            .map_err(|error| io::Error::new(ErrorKind::InvalidData, error))?;
        if let Some(error) = response.get("error") {
            return Err(io::Error::other(format!("host rpc error: {error}")));
        }
        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two halves that must not drift, pinned in ONE place: what a window MINTS is what its
    /// launcher MATCHES. Split across crates this is a convention nothing checks.
    #[test]
    fn a_minted_gui_client_id_matches_this_process_prefix() {
        let id = new_gui_client_id();
        assert!(
            id.starts_with(&gui_client_prefix(std::process::id())),
            "a window's launcher recognises the id that window mints: {id}",
        );
    }

    /// The trailing dash earning its keep: pid 123 must not accept pid 1234's window. Without it
    /// a launcher reports success for a window that is not its own and never came up.
    #[test]
    fn a_pid_prefix_does_not_match_a_longer_pid() {
        let longer = "gui-1234-99999";
        assert!(
            !longer.starts_with(&gui_client_prefix(123)),
            "gui-123- must not swallow {longer}",
        );
        assert!(
            longer.starts_with(&gui_client_prefix(1234)),
            "but its own pid still matches",
        );
    }

    use pinion_rpc::{RpcFrame, RpcIngress};
    use pinion_rpc_transport::UnixSocketTransport;
    use std::sync::Arc;
    use std::sync::mpsc::{Receiver, Sender, channel};
    use std::thread;

    /// A trivial ingress: funnel frames to a channel a test-owned dispatch thread
    /// answers, so `HostConn` drives a REAL socket end-to-end (the same
    /// [`UnixSocketTransport`] the GUI/host mount) without standing up a full
    /// `HostState` or the mount policy layer (env + process-global `ENDPOINT`).
    struct ChannelIngress {
        tx: Sender<RpcFrame>,
    }
    impl RpcIngress for ChannelIngress {
        fn submit(&self, frame: RpcFrame) {
            let _ = self.tx.send(frame);
        }
    }

    /// Answer frames by echoing the request's `params` back as the `result` — enough
    /// to prove request framing + response parsing round-trip over the real socket.
    fn echo_dispatch(rx: Receiver<RpcFrame>) {
        for frame in rx {
            let request: Value = serde_json::from_str(&frame.request).unwrap();
            let response = json!({
                "jsonrpc": "2.0",
                "id": request["id"],
                "result": request["params"].clone(),
            });
            frame.reply.send(response.to_string());
        }
    }

    #[test]
    fn call_round_trips_a_request_over_the_socket() {
        // A unique socket under the temp dir (pid-scoped so parallel test binaries
        // do not collide). Bind the transport directly — no env, no global.
        let path =
            std::env::temp_dir().join(format!("sprag-rpc-client-test-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let (tx, rx) = channel();
        thread::spawn(move || echo_dispatch(rx));
        let control = UnixSocketTransport::serve(&path, Arc::new(ChannelIngress { tx }))
            .expect("bind the test socket");
        control.set_enabled(true);

        let mut conn =
            HostConn::connect(&path, Duration::from_secs(2)).expect("connect to the socket");
        // Two calls prove the id increments and the read stream stays in sync.
        assert_eq!(
            conn.call("scene/echo", json!({"hello": "world"})).unwrap(),
            json!({"hello": "world"})
        );
        assert_eq!(conn.call("scene/echo", json!(42)).unwrap(), json!(42));

        drop(control);
        let _ = std::fs::remove_file(&path);
    }

    /// A host that ACCEPTS and then never answers costs the caller its deadline, not its life —
    /// and the connection retires rather than pretending it can be used again.
    ///
    /// The listener here answers nothing on purpose, which is precisely the state a real wedged
    /// daemon presents: the socket is up, the connect succeeds, and the reply never comes. That is
    /// why the connect timeout was never a defence — it had already succeeded. Without the
    /// deadline the first `call` below would block until this test binary was killed.
    #[test]
    fn a_host_that_never_answers_costs_the_deadline_and_retires_the_connection() {
        let path = std::env::temp_dir().join(format!(
            "sprag-rpc-deadline-test-{}.sock",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let listener = std::os::unix::net::UnixListener::bind(&path).expect("bind the test socket");
        // HOLD the accepted stream: dropping it would close the connection and the read would end
        // with EOF, which is the very outcome this test must not be able to pass by.
        let accepted = thread::spawn(move || listener.accept().map(|(stream, _)| stream));

        let mut conn =
            HostConn::connect(&path, Duration::from_secs(2)).expect("connect to the socket");
        let deadline = Duration::from_millis(200);
        conn.set_read_deadline(Some(deadline))
            .expect("bound the reads");

        let start = Instant::now();
        let error = conn
            .call("scene/never", json!({}))
            .expect_err("a host that never answers must not answer");
        let waited = start.elapsed();
        assert!(
            matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut),
            "the failure must say it timed out, not something a caller would retry: {error:?}",
        );
        assert!(
            waited >= deadline && waited < deadline * 20,
            "the call must return AT the deadline, not before it and not much after ({waited:?})",
        );

        // Retired: a second call cannot go out, because a reply arriving late would be read as its
        // answer. It fails on the connection's own state, without touching the socket.
        let after = conn
            .call("scene/never", json!({}))
            .expect_err("a timed-out connection is finished");
        assert_eq!(after.kind(), ErrorKind::TimedOut, "{after:?}");

        drop(conn);
        let _ = accepted.join();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_scoped_connection_puts_the_session_on_every_request() {
        let path =
            std::env::temp_dir().join(format!("sprag-rpc-scope-test-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let (tx, rx) = channel();
        thread::spawn(move || echo_dispatch(rx));
        let control = UnixSocketTransport::serve(&path, Arc::new(ChannelIngress { tx }))
            .expect("bind the test socket");
        control.set_enabled(true);

        let mut conn =
            HostConn::connect(&path, Duration::from_secs(2)).expect("connect to the socket");

        // Unscoped: params reach the host verbatim (the echo mirrors them back).
        assert_eq!(
            conn.call("scene/query", json!({ "path": "p" })).unwrap(),
            json!({ "path": "p" }),
            "an unscoped connection adds nothing",
        );

        // Scope it, and EVERY subsequent request carries the session as a params sibling —
        // the one seam a client scopes through, so its connections cannot drift.
        conn.scope_to("work");
        assert_eq!(
            conn.call("scene/query", json!({ "path": "p" })).unwrap(),
            json!({ "path": "p", "session": "work" }),
            "a scoped connection names its session on every request",
        );
        assert_eq!(
            conn.call("scene/invoke", json!({ "path": "spawn", "args": {} }))
                .unwrap(),
            json!({ "path": "spawn", "args": {}, "session": "work" }),
            "...including a different method with its own args",
        );

        // A non-object params has no place for the key, so it is passed through untouched
        // rather than reshaped — the wire client never scopes such a request.
        assert_eq!(
            conn.call("scene/echo", json!(42)).unwrap(),
            json!(42),
            "a non-object params carries no scope",
        );

        drop(control);
        let _ = std::fs::remove_file(&path);
    }
}
