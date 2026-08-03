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

use std::fmt;
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

/// The SHAPE this build speaks — the number both ends of the wire compare before either acts on
/// the other's bytes.
///
/// Bump it whenever a wire type's serialised shape changes in a way an older peer cannot read.
/// You will not have to remember to: `sprag_host::wire`'s shape pin renders one canonical value of
/// each such type and fails on the bytes, naming this constant. It covers the shapes sprag owns
/// AND the cell frame it borrows from pinion — the latter because a shape can move with no sprag
/// source line changed at all (see version 2 below), which is the case a reviewer cannot catch.
///
/// # Why it exists
///
/// Without it, a client and daemon whose shapes disagree find out as a serde message about a
/// value's type, at whichever slot happens to have changed, AFTER doing real work. That is not
/// hypothetical: R264 flattened the layout wire so a window's `root` became an arena index, and a
/// `sprag-tui` left over from before it died on `invalid type: integer 0, expected string or map`
/// at its ninth request — having already created a session it then abandoned. Five rounds
/// excluded five hypotheses without finding it, because a version skew is invisible from either
/// end alone (`sprag_terminal::layout`'s
/// `an_older_build_cannot_read_this_ones_root_and_says_so_by_type` pins the sentence).
///
/// # Why a single number rather than a negotiation
///
/// A daemon serving two shapes at once would have to keep every retired shape alive forever. The
/// daemon is restartable and its sessions survive the restart through the durability snapshot, so
/// "restart the daemon" is a complete remedy and the mismatch message says exactly that.
///
/// # What each number stands for
///
/// * **1** — the shape after R264 flattened the layout wire (`root` an arena index).
/// * **2** — the cell frame's underline spelling. pinion R1540 gave `UnderlineStyle` a
///   `rename_all = "lowercase"`, so a style that crossed as `"underline":"None"` now crosses as
///   `"underline":"none"`, and a peer on either side of the pin bump cannot read the other's
///   frame. **No sprag source line changed to cause it**: the cell frame carries pinion's own
///   cell vocabulary verbatim (`sprag_grid::wire`'s `CellStyle`, deliberately, so an upstream
///   ADDITION is a compile error rather than silent data loss) — and a respelling is neither an
///   addition nor a compile error. That is why the shape pin renders the frame's bytes rather
///   than trusting the diff: a dependency bump is a wire change this project cannot see in its
///   own diff.
/// * **3** — the session list stopped carrying what it had to SAMPLE. R282 took `cwd`, `branch` and
///   `ports` off `SessionInfo` and gave them their own address (`session_activity.<max_age_ms>`),
///   because the registry's structure and the operating system are different kinds of fact and
///   serving them together made the cheapest question in the mux cost a `/proc` walk of every
///   process on the box, on every poll wake of every attached client. A pre-R282 client reading a
///   post-R282 daemon would find every session working nowhere and serving nothing — a wrong answer
///   that decodes cleanly, which is exactly the failure this number exists to turn into a sentence.
/// * **4** — the arrangement gained a ZOOM (`LayoutSnapshot.zoomed`, R285): which pane, if any, is
///   filling the window on its own. A pre-R285 client reading a post-R285 daemon would decode the
///   snapshot cleanly, ignore the new key, and paint the whole arrangement while the daemon had
///   already reflowed the zoomed pane's PTY to the full window — so its grid would be the wrong
///   size for every pane on screen. The same wrong-answer-that-parses this number exists for.
pub const WIRE_PROTOCOL: u32 = 4;

/// The JSON-RPC `params` key carrying [`WIRE_PROTOCOL`] — merged into EVERY request by
/// [`HostConn::call`], beside [`SESSION_PARAM`] and for the same reason: a fact every request
/// must carry belongs at the one seam that builds them all, never at each call site.
pub const PROTOCOL_PARAM: &str = "protocol";

/// The [`CLIENT_HELLO_METHOD`] REPLY key carrying the daemon's own [`WIRE_PROTOCOL`] — the other
/// direction of the same check.
///
/// A daemon outlives its clients by design, so the common skew after a rebuild is a NEW client
/// reaching an OLD daemon; the old one ignores the unknown request param and answers happily, so
/// the client can only learn the truth from a reply. A reply with no such key is a daemon from
/// before the handshake, which is a mismatch and is reported as one.
pub const PROTOCOL_FIELD: &str = "protocol";

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

/// The JSON-RPC method a client sends to BLOCK until a change it named actually happens —
/// `params: { "since": <cursor>, "match": [<clause>…]? }`, answering the same
/// `{events, next, lost}` batch the `events.<since>` slot serves.
///
/// ## Why this is not `scene/waitFor`
///
/// `scene/waitFor` is a DISPLAY client's wake: it parks on the scene revision, which a pane's output
/// bumps, because output is exactly what makes a projection stale. A client waiting for a NAMED
/// change is asking a different question, and answering it with that wake does not work — measured
/// on a real daemon, the pair `scene/waitFor` + `events.<since>` returns **22 431 times a second**
/// against a pane producing build-rate output, every answer empty, where a quiet pane returns none.
/// The cursor cannot even advance past it: a batch's `next` is the last RECORD's revision, so the
/// scene runs away from the reader and every subsequent park is already stale.
///
/// So this method parks on the JOURNAL instead. Output appends no record, so it does not wake this;
/// a change does, and only if it matches what the caller asked for. The filter is evaluated
/// server-side, under the lock the append takes.
///
/// ## Intercepted, like the client-lifecycle methods
///
/// Handled in the host's per-frame dispatch before the generic core, because it PARKS its reply
/// rather than answering it — the same shape pinion's own async `scene/waitFor` takes, and for the
/// same reason: the host's dispatch is one thread for every client, so a handler that blocked would
/// freeze the daemon. It carries no deadline of its own; a caller's socket read deadline is its
/// timeout, and the close that follows releases the park.
pub const EVENTS_WAIT_METHOD: &str = "events/waitFor";

/// The [`EVENTS_WAIT_METHOD`] params key carrying the cursor to wait FROM — a revision the caller has
/// already accounted for, exclusive, the same half-open convention `scene/waitFor {since}` and the
/// `events.<since>` slot both use, so a number from any of the three can be handed to the others.
pub const SINCE_PARAM: &str = "since";

/// The JSON-RPC method a client sends to BLOCK until a named pane's retained output matches —
/// `params: { "pane": <id>, "needle": <string> }` or `{ "pane": <id>, "pattern": <string> }`,
/// answering `{ "pane": <id>, "find": {matches, lines, truncated} }`.
///
/// ## Why this is a THIRD wait and not either of the other two
///
/// [`EVENTS_WAIT_METHOD`] parks on the journal, which output deliberately never appends to — a
/// record per PTY batch would evict the ring at output rate and destroy the delivery guarantee the
/// ring exists to give. `scene/waitFor` is woken by output but answers every bump, so a caller
/// would still be writing the search loop this method exists to remove.
///
/// So it parks on the revision (the only token output moves) carrying a PREDICATE, and the predicate
/// is the search the pane's own `find.<needle>` / `regex.<pattern>` slots already run, over the same
/// retained output — scrollback INCLUDED. That last word is the contract: a line printed and
/// scrolled off the visible screen while the caller was not looking still matches, because the
/// search reads what the pane kept rather than what it is showing.
///
/// ## Two params, never one plus a mode
///
/// `needle` is a literal (ASCII case folded); `pattern` is a regular expression (case-sensitive —
/// `(?i)` is in the language itself). Exactly one is required. They are separate keys for the reason
/// the two query slots are separate addresses: a needle and a pattern are separate languages, so one
/// string must not mean both depending on a flag carried beside it.
///
/// ## Intercepted, like the other parked method
///
/// Handled in the host's per-frame dispatch before the generic core, because it PARKS its reply.
/// It carries no deadline of its own, for the reason [`EVENTS_WAIT_METHOD`] gives: a caller's socket
/// read deadline is exact where a daemon-side one would need a clock the daemon does not have, and
/// the close that follows releases the park however the caller goes away.
pub const PANE_WAIT_OUTPUT_METHOD: &str = "pane/waitForOutput";

/// The [`PANE_WAIT_OUTPUT_METHOD`] params key naming the pane whose output is the subject — the
/// host pane id, the same handle `sprag panes` prints and every other wire address takes.
pub const PANE_PARAM: &str = "pane";

/// The [`PANE_WAIT_OUTPUT_METHOD`] params key carrying a LITERAL needle to wait for.
pub const NEEDLE_PARAM: &str = "needle";

/// The [`PANE_WAIT_OUTPUT_METHOD`] params key carrying a REGULAR EXPRESSION to wait for.
pub const PATTERN_PARAM: &str = "pattern";

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

/// How a failed request names itself: the method, plus the `path` its params address when they
/// carry one — `scene/query /sprag_mux/external/layout`.
///
/// The path and not the whole params, because the params are what makes a request BIG (a paste,
/// a cell buffer) and an error line that quotes a screenful of text is unreadable in exactly the
/// situation it is read in. `path` is the slot, which is the discriminating half — two failures
/// of `scene/query` are told apart by it, and nothing else in the params tells them apart at all.
fn request_label(method: &str, params: &Value) -> String {
    match params.get("path").and_then(Value::as_str) {
        Some(path) => format!("{method} {path}"),
        None => method.to_owned(),
    }
}

/// The bytes one request goes out as: the encoded value plus its terminating newline, built
/// WHOLE before anything is written.
///
/// Separate from [`write_request`] on purpose — the pair is what keeps a request one syscall.
/// See there for what the alternative cost.
fn request_line(request: &Value) -> String {
    let mut line = request.to_string();
    line.push('\n');
    line
}

/// Put one already-complete request line on the wire.
///
/// ## Why the line is built first, and why this is not `writeln!`
///
/// It was `writeln!(self.writer, "{request}")` against a **raw** `UnixStream` — the read half is
/// buffered, the write half never was. `writeln!` lowers to `write_fmt`, and `serde_json::Value`'s
/// `Display` writes the value TOKEN BY TOKEN, so with nothing buffering underneath, every brace,
/// every quote and every key became its own `sendto`. Measured on the daemon's own `panes` request:
/// **84 `sendto` calls for what is one 105-byte line**, and 202 syscalls for a CLI invocation that
/// herdr answers in 52.
///
/// `write_all` of a finished line is one call into the kernel and cannot be half-sent, so the
/// property holds by construction rather than by a `flush` somebody must remember: there is no
/// buffered state here to leave unflushed, which is also why the writer stays a plain `UnixStream`
/// and [`HostConn::shutdown_handle`] can keep cloning it.
fn write_request(writer: &mut impl Write, line: &str) -> io::Result<()> {
    writer.write_all(line.as_bytes())
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
        let mut map = match params {
            Value::Object(map) => map,
            // A request with no params of its own still has to declare its SHAPE, so the object is
            // created rather than the declaration dropped. Absent params and an empty object mean
            // the same thing to every handler.
            Value::Null => serde_json::Map::new(),
            // Anything else is a caller spelling params in a form this wire has no key to add to.
            // Left exactly as given: refusing here would turn a caller's mistake into a transport
            // failure, and the daemon refuses it by name.
            other => return other,
        };
        // EVERY request declares the shape it was written against ([`WIRE_PROTOCOL`]). Merged
        // here, at the one seam that builds a request, so no client can omit it — including the
        // ones that do not exist yet.
        map.insert(PROTOCOL_PARAM.to_owned(), Value::from(WIRE_PROTOCOL));
        if let Some(session) = &self.session {
            map.insert(SESSION_PARAM.to_owned(), Value::String(session.clone()));
        }
        Value::Object(map)
    }

    /// Announce this connection's client id AND agree on the wire's shape — the door check every
    /// client passes through, on traffic it was already sending.
    ///
    /// Sends [`CLIENT_HELLO_METHOD`] and reads [`PROTOCOL_FIELD`] out of the reply. The
    /// daemon-side half of the agreement (an OLD client reaching a NEW daemon) is enforced by the
    /// daemon on every request; this is the half only a client can make — a daemon OLDER than the
    /// client answers the unknown protocol param happily and would otherwise be discovered slot by
    /// slot.
    ///
    /// # Errors
    ///
    /// The hello failing, or the daemon answering with a different [`WIRE_PROTOCOL`] — or with
    /// none, which means a daemon from before this handshake existed. Both are reported with both
    /// numbers and the remedy, because a mismatched pair cannot be made to work by retrying.
    pub fn handshake(&mut self, client_id: &str) -> io::Result<()> {
        let reply = self.call(
            CLIENT_HELLO_METHOD,
            serde_json::json!({ CLIENT_PARAM: client_id }),
        )?;
        match reply.get(PROTOCOL_FIELD).and_then(Value::as_u64) {
            Some(daemon) if daemon == u64::from(WIRE_PROTOCOL) => Ok(()),
            Some(daemon) => Err(protocol_mismatch(&daemon.to_string())),
            None => Err(protocol_mismatch("none (a daemon older than this check)")),
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
    /// # Every failure NAMES the request it came from
    ///
    /// A caller's boot issues a dozen of these, and until R278 a failure from any of them
    /// arrived as the bare cause — `invalid type: integer 0, expected string or map` and nothing
    /// else. That sentence identifies neither the step, nor the slot, nor the daemon, so the
    /// reader has to bisect a sequence they cannot see; it cost a full session to find out which
    /// call it was, and the answer turned out to be a step nobody suspected.
    ///
    /// So the funnel names it: every error out of here is prefixed with the method and, when the
    /// params carry one, the path — `scene/query /sprag_mux/external/layout: <cause>`. Done HERE
    /// rather than at each call site because there is one of these and dozens of those, and the
    /// one that gets forgotten is always the one that fires.
    ///
    /// The [`ErrorKind`] is PRESERVED across the wrap. Callers switch on it (an
    /// [`ErrorKind::UnexpectedEof`] means the host is gone and a client should exit rather than
    /// retry), so a wrap that flattened every failure to `Other` would trade a readable message
    /// for a behavioural regression.
    ///
    /// # Errors
    ///
    /// I/O failure writing the request or reading the reply, a malformed reply, or
    /// a JSON-RPC `error` object in the response.
    pub fn call(&mut self, method: &str, params: Value) -> io::Result<Value> {
        // The label is built BEFORE the call, because the params move into it.
        let label = request_label(method, &params);
        self.call_inner(method, params)
            .map_err(|error| io::Error::new(error.kind(), format!("{label}: {error}")))
    }

    /// [`call`](Self::call), but a JSON-RPC `error` object comes back as itself rather than as a
    /// message — for the caller that has to ACT on which failure it was.
    ///
    /// [`call`](Self::call) renders every failure as text, which is right for a command that is
    /// about to print it and stop. It is wrong for a caller deciding between "the daemon answered
    /// no" and "the daemon is gone": those differ by the JSON-RPC `code`, and recovering a code
    /// from a rendered sentence means one crate matching on another's wording. The refusal a
    /// scoped pre-flight reads (`sprag`'s `session_exists`) is exactly that case.
    ///
    /// # Errors
    ///
    /// [`CallError::Transport`] for I/O failure or a malformed reply — named exactly as
    /// [`call`](Self::call) names it, so nothing is lost by choosing this one — and
    /// [`CallError::Fault`] for a JSON-RPC `error` object.
    pub fn try_call(&mut self, method: &str, params: Value) -> Result<Value, CallError> {
        let label = request_label(method, &params);
        match self.call_inner(method, params) {
            Ok(value) => Ok(value),
            Err(CallFailure::Fault(fault)) => Err(CallError::Fault(fault)),
            Err(CallFailure::Transport(error)) => Err(CallError::Transport(io::Error::new(
                error.kind(),
                format!("{label}: {error}"),
            ))),
        }
    }

    /// [`call`](Self::call)'s body, wrapped by it so that EVERY exit is named — including the
    /// early refusal below, which is otherwise the one a `?` inside the body would skip.
    ///
    /// It keeps the daemon's `error` object apart from a transport failure ([`CallFailure`]) so
    /// that [`try_call`](Self::try_call) can hand the object out; [`call`](Self::call) flattens
    /// the two, which is the whole difference between them.
    fn call_inner(&mut self, method: &str, params: Value) -> Result<Value, CallFailure> {
        if self.timed_out {
            return Err(io::Error::new(
                ErrorKind::TimedOut,
                "connection abandoned after a read deadline expired",
            )
            .into());
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": self.scoped(params),
        });
        write_request(&mut self.writer, &request_line(&request))?;

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
                return Err(
                    io::Error::new(ErrorKind::UnexpectedEof, "host closed the connection").into(),
                );
            }
            if !line.trim().is_empty() {
                break;
            }
        }

        let response: Value = serde_json::from_str(line.trim())
            .map_err(|error| io::Error::new(ErrorKind::InvalidData, error))?;
        if let Some(error) = response.get("error") {
            return Err(CallFailure::Fault(RpcFault::from_wire(error)));
        }
        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    }
}

/// The JSON-RPC `Invalid params` code — the one both ends of this wire already spell, now spelled
/// once.
///
/// It is the code sprag's daemon answers a request whose SCOPE it cannot honour with, and the one
/// a scoped pre-flight reads back off [`RpcFault::code`]. Defined here, in the transport both ends
/// share, for the reason [`SESSION_PARAM`] is: a number the writer and the reader must agree on
/// has one home.
pub const INVALID_PARAMS: i64 = -32602;

/// A JSON-RPC `error` object as its own fact — the code the peer chose, its message, and whatever
/// `data` it attached.
///
/// Carried out of [`HostConn::try_call`] so a caller can branch on the CODE. The alternative is
/// reading the code back out of a rendered sentence, which makes one crate depend on another's
/// wording and fails silently on the day the wording improves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcFault {
    /// The JSON-RPC error code — `-32602` for invalid params, which is what a refused scope is.
    pub code: i64,
    /// The peer's one-line reason.
    pub message: String,
    /// The peer's detail, when it attached one. sprag's daemon puts the specific sentence here
    /// (`no session named "x"`) and keeps `message` at the JSON-RPC category.
    pub data: Option<Value>,
}

impl RpcFault {
    /// Read a JSON-RPC `error` object. Absent fields degrade rather than fail: a reply this
    /// malformed is still a refusal, and reporting it as a transport error would be a lie about
    /// which end went wrong.
    fn from_wire(error: &Value) -> Self {
        Self {
            code: error["code"].as_i64().unwrap_or(0),
            message: error["message"].as_str().unwrap_or_default().to_owned(),
            data: error.get("data").cloned(),
        }
    }
}

impl fmt::Display for RpcFault {
    /// The `data` sentence when there is one, because that is the specific thing the daemon had to
    /// say; the JSON-RPC category otherwise.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.data.as_ref().and_then(Value::as_str) {
            Some(detail) => write!(f, "{detail}"),
            None => write!(f, "{}", self.message),
        }
    }
}

/// What a [`HostConn::try_call`] can fail as: the peer refusing, or the wire itself.
#[derive(Debug)]
pub enum CallError {
    /// The peer answered a JSON-RPC `error` object — it heard the request and said no.
    Fault(RpcFault),
    /// The request never completed: I/O, a deadline, or a reply that is not JSON-RPC.
    Transport(io::Error),
}

impl From<CallError> for io::Error {
    /// Back to the flat form [`HostConn::call`] hands out, so a caller that opted into the typed
    /// error can still `?` it into an `io::Result` without re-spelling the rendering.
    fn from(error: CallError) -> Self {
        match error {
            CallError::Transport(error) => error,
            CallError::Fault(fault) => Self::other(format!("host rpc error: {fault}")),
        }
    }
}

/// The internal half of [`CallError`] — the same split, before the label is applied.
#[derive(Debug)]
enum CallFailure {
    Fault(RpcFault),
    Transport(io::Error),
}

impl From<io::Error> for CallFailure {
    fn from(error: io::Error) -> Self {
        Self::Transport(error)
    }
}

impl CallFailure {
    /// The kind [`HostConn::call`]'s wrap preserves. A fault reached the peer and came back, so it
    /// is [`ErrorKind::Other`] exactly as it was before the split.
    fn kind(&self) -> ErrorKind {
        match self {
            Self::Transport(error) => error.kind(),
            Self::Fault(_) => ErrorKind::Other,
        }
    }
}

impl fmt::Display for CallFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(error) => write!(f, "{error}"),
            Self::Fault(fault) => write!(f, "host rpc error: {fault}"),
        }
    }
}

/// The mismatch report: what this build speaks, what the other end does, and the one action that
/// fixes it.
///
/// A restart is the WHOLE remedy because the daemon's sessions survive it (the durability
/// snapshot), which is why this says so plainly rather than hedging — a message that leaves the
/// reader wondering whether they are about to lose their panes is a message they will not act on.
fn protocol_mismatch(daemon: &str) -> io::Error {
    io::Error::new(
        ErrorKind::InvalidData,
        format!(
            "this client speaks wire protocol {WIRE_PROTOCOL} and the daemon speaks {daemon}; \
             they cannot understand each other. Restart the daemon to bring it to this build — \
             `sprag kill-server` (sessions are restored from the durability snapshot)",
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An `io::Write` that reports what it was ASKED to do, not only what it received — the
    /// instrument this file's one-syscall claim needs, since a socket peer cannot see write
    /// boundaries (a `SOCK_STREAM` reader coalesces them, which is exactly why the defect
    /// survived: nothing downstream could tell 84 writes from one).
    #[derive(Default)]
    struct CountingWriter {
        writes: usize,
        bytes: Vec<u8>,
    }

    impl Write for CountingWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.writes += 1;
            self.bytes.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// One request is ONE call into the writer, and the CONTROL shows the counter can move.
    ///
    /// The control is the whole test. `writes == 1` proves nothing on its own — a counter that
    /// never increments passes it — so the retired form (`writeln!` of the value straight at the
    /// writer, which is what this crate did) runs through the SAME instrument and must count
    /// many. That is also the measurement in miniature: on the live socket that shape issued 84
    /// `sendto` calls for this very request.
    #[test]
    fn a_request_reaches_the_writer_as_one_call() {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "scene/query",
            "params": { "session": "0", "path": "/sprag_mux/external/panes" },
        });

        let mut writer = CountingWriter::default();
        write_request(&mut writer, &request_line(&request)).expect("the line is written");
        assert_eq!(writer.writes, 1, "one request, one call into the writer");
        assert!(
            writer.bytes.ends_with(b"\n"),
            "the line the server frames on is complete when it leaves",
        );
        // It is still the request, byte for byte — a cheaper write that dropped a field would
        // pass the count above.
        let sent: Value =
            serde_json::from_slice(&writer.bytes).expect("what was written parses back");
        assert_eq!(
            sent, request,
            "the encoding is unchanged, only the syscall count"
        );

        // CONTROL: the retired form, same instrument.
        let mut retired = CountingWriter::default();
        writeln!(retired, "{request}").expect("the control writes");
        assert!(
            retired.writes > 10,
            "the instrument counts calls: the retired form issued {} of them",
            retired.writes,
        );
        assert_eq!(
            retired.bytes, writer.bytes,
            "both forms put the same bytes on the wire — only the call count differs",
        );
    }

    /// A failed request says WHICH request it was, and stays the KIND it was.
    ///
    /// Both halves or neither: a message that names the slot but flattens every failure to
    /// `Other` would be readable and would break the callers that exit on
    /// [`ErrorKind::UnexpectedEof`] rather than retry. The control is the second case — a method
    /// with no `path` must name the method alone rather than printing `null` or an empty gap,
    /// because `client/hello` and `client/attach` fail for different reasons and neither carries
    /// a path.
    #[test]
    fn a_failed_request_names_itself_and_keeps_its_kind() {
        let with_path = request_label(
            "scene/query",
            &json!({ "path": "/sprag_mux/external/layout", "session": "1" }),
        );
        assert_eq!(with_path, "scene/query /sprag_mux/external/layout");
        assert_eq!(
            request_label("client/attach", &json!({ "session": "1" })),
            "client/attach"
        );
        assert_eq!(request_label("ping", &json!(null)), "ping");

        // The wrap `call` applies, exercised on the shape that cost R278 a session to find.
        let cause = io::Error::new(
            ErrorKind::InvalidData,
            "invalid type: integer `0`, expected string or map",
        );
        let named = io::Error::new(cause.kind(), format!("{with_path}: {cause}"));
        assert_eq!(
            named.kind(),
            ErrorKind::InvalidData,
            "the kind survives, so a caller can still switch on it",
        );
        assert!(
            named.to_string().contains("/sprag_mux/external/layout"),
            "the message names the slot: {named}",
        );
    }

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
        // Two calls prove the id increments and the read stream stays in sync. Each carries the
        // shape declaration every request does ([`WIRE_PROTOCOL`]), which the echo mirrors back.
        assert_eq!(
            conn.call("scene/echo", json!({"hello": "world"})).unwrap(),
            json!({"hello": "world", PROTOCOL_PARAM: WIRE_PROTOCOL}),
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

        // Unscoped: no SESSION is added — but the shape declaration is, on every request there
        // is. The two are different kinds of fact: a session is this connection's, and a
        // protocol is this build's, so one is conditional and the other never.
        assert_eq!(
            conn.call("scene/query", json!({ "path": "p" })).unwrap(),
            json!({ "path": "p", PROTOCOL_PARAM: WIRE_PROTOCOL }),
            "an unscoped connection adds no session, and still declares its shape",
        );

        // Scope it, and EVERY subsequent request carries the session as a params sibling —
        // the one seam a client scopes through, so its connections cannot drift.
        conn.scope_to("work");
        assert_eq!(
            conn.call("scene/query", json!({ "path": "p" })).unwrap(),
            json!({ "path": "p", "session": "work", PROTOCOL_PARAM: WIRE_PROTOCOL }),
            "a scoped connection names its session on every request",
        );
        assert_eq!(
            conn.call("scene/invoke", json!({ "path": "spawn", "args": {} }))
                .unwrap(),
            json!({ "path": "spawn", "args": {}, "session": "work", PROTOCOL_PARAM: WIRE_PROTOCOL }),
            "...including a different method with its own args",
        );

        // A non-object params has no place for either key, so it is passed through untouched
        // rather than reshaped. It is also therefore a request no daemon of this build will
        // serve — it carries no shape declaration — which is a caller's mistake to be refused by
        // name, not one the transport should paper over by inventing an object around it.
        assert_eq!(
            conn.call("scene/echo", json!(42)).unwrap(),
            json!(42),
            "a non-object params carries neither scope nor shape",
        );

        drop(control);
        let _ = std::fs::remove_file(&path);
    }
}
