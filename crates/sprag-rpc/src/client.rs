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
//! A [`HostConn`] is single-threaded (one outstanding request at a time), so a connection never
//! desyncs its read stream. A caller that needs concurrency uses more connections, not shared
//! mutable access.
//!
//! ## ⚠ "No server push" was true until R298 and is not true any more
//!
//! This doc said the transport was *strictly* request→response, and that an async `scene/waitFor` is
//! a deferred RESPONSE rather than a push — one reply per request, always. That was accurate and it
//! described a limitation sprag had filed upstream as PINION-PR83: a reply sink was `FnOnce`, so one
//! request producing many answers was inexpressible at any price.
//!
//! pinion R1552 delivered a per-connection writer, and [`EVENTS_SUBSCRIBE_METHOD`] is sprag's
//! consumer of it: **the daemon now writes NOTIFICATIONS on this connection that nobody asked for.**
//! The consequence for this module is inside [`HostConn::call`] — the reader can no longer take the
//! next line as its answer, so it discriminates by `id` (JSON-RPC 2.0, section 4.1) and sets aside what is
//! not its own. A frame set aside is delivered by
//! [`next_notification`](HostConn::next_notification), never dropped.
//!
//! One request at a time still holds, and it is now a rule about REQUESTS rather than a property of
//! the stream.

use std::collections::VecDeque;
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

/// The JSON-RPC `params` key asking for the scope of the session THIS CONNECTION's client is
/// ATTACHED to — `{"attached": true}`, the alternative to naming one in [`SESSION_PARAM`].
///
/// Defined beside its sibling and for the same reason: the writer owns the spelling and the host
/// reads it. What it is FOR is on [`ScopeAsk::Attached`].
pub const ATTACHED_PARAM: &str = "attached";

/// The `params` key narrowing a request to ONE WINDOW of the scoped session — `{"window": "build"}`.
///
/// ORTHOGONAL to the three [`ScopeAsk`] arms rather than a fourth one: they answer WHICH SESSION and
/// this answers WHICH WINDOW OF IT, so it composes with every one of them (a display client can
/// narrow its own attachment). Absent ⇒ the session's CURRENT window, which is what every request
/// meant before this key existed.
///
/// # What it is FOR (R311)
///
/// `sprag_host::scope::Scope` has always carried a window NAME and that window's POOL, resolved off
/// one `Window` under one lock — every read downstream is already per-window. What it lacked was a
/// way for a request to ASK for one, so the scene a read is addressed through held only the CURRENT
/// window's panes, and a pane one window over answered `NoExternalAtPath`. Meanwhile the WRITE verbs
/// (`rename_pane`, `swap_pane`, `move_pane`) resolve a pane registry-wide and cross a window freely.
/// **So an agent could rename and swap a pane it could not read** — measured, and an artifact of two
/// addressing paths rather than a decision anyone took.
pub const WINDOW_PARAM: &str = "window";

/// WHICH session a request acts on, as the request ASKS for it — the scope grammar, defined ONCE
/// for both ends of the wire.
///
/// A resolved scope is the host's (`sprag_host::SessionScope`, which carries proof the session
/// exists); this is the question that precedes it, and it lives here because both sides need the
/// SAME answer to "what does this params object ask for". The two directions are
/// [`write_into`](Self::write_into) (what a client sends) and [`parse`](Self::parse) (what the
/// daemon reads), so a spelling cannot drift between them and neither can the rule for a key that
/// is present but empty.
///
/// The three arms are three different questions, not three spellings of one:
///
/// * [`Default`](Self::Default) — "whichever session this daemon calls its default". What a caller
///   with no session in mind means: the `sprag` CLI's un-`-t`'d verbs, a fresh connection.
/// * [`Named`](Self::Named) — "the session called this". The PUBLIC address, the thing a human
///   types after `-t`, and the only arm that can address a session the caller is not viewing.
/// * [`Attached`](Self::Attached) — "the session I am VIEWING". See its own doc: this is the arm a
///   display client's every scoped read wants, and the one that cannot be stolen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ScopeAsk {
    /// No scope key at all ⇒ the daemon's DEFAULT session.
    #[default]
    Default,
    /// `{"session": "<name>"}` ⇒ the session with that NAME — the public address every `-t` takes.
    Named(String),
    /// `{"attached": true}` ⇒ the session this connection's client is attached to
    /// (`client/attach`), whatever it is currently CALLED.
    ///
    /// # Why a display client must use this and not its session's name
    ///
    /// A name is an ADDRESS, and an address can be retired and re-issued. A client that re-sends
    /// the name it booted with is asking a question that stops meaning what it meant: after
    /// `rename-session`, the name resolves to NOTHING (the client is refused and detaches from a
    /// session that is alive and that it is still attached to); after a new session then takes the
    /// retired name, it resolves to SOMEBODY ELSE (the client silently reads — and types into — a
    /// session it never named). Both were measured at R303 against a live daemon.
    ///
    /// An attachment is not an address but a POINTER the daemon maintains: it moves with a rename
    /// (R302) and ends with a kill, both inside the dispatch that does the thing. So this arm is
    /// not a narrower race, it is no race — a request arriving on either side of a rename resolves
    /// to the same session.
    ///
    /// It addresses only the client's OWN view, deliberately. Acting on another session is what
    /// [`Named`](Self::Named) is for, and keeping the two apart is tmux's own split between a
    /// client's attached session and a command's `-t` target.
    Attached,
}

/// Why a params object does not name a scope this grammar admits. Every arm refuses the request
/// WHOLE — none of them falls back to the default, because a scope that cannot be honoured means
/// nothing rather than "probably the usual one".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopeFault {
    /// [`SESSION_PARAM`] is present and is not a string (`{"session": 42}`).
    NotAString,
    /// [`ATTACHED_PARAM`] is present and is neither a boolean nor null (`{"attached": 7}`).
    AttachedNotABool,
    /// BOTH keys ask for a scope at once. They are different questions and there is no honest way
    /// to pick one, so neither is answered.
    TwoScopes,
    /// [`WINDOW_PARAM`] is present and is not a string (`{"window": 3}`).
    ///
    /// Its own variant rather than [`NotAString`](Self::NotAString), because the two name different
    /// keys and the sentence a surface renders has to say which one the caller got wrong. A window
    /// NUMBER is the likeliest way to get it wrong — sprag windows have names and no numbers — so
    /// this is the refusal a real caller meets.
    WindowNotAString,
}

impl ScopeAsk {
    /// Write this ask into a request's `params` map — the ONE place a client spells a scope.
    ///
    /// [`Default`](Self::Default) writes NOTHING: an absent scope is what a request meant before
    /// either key existed, so the commonest request on the wire is unchanged byte for byte and a
    /// reader of a trace can still tell the three asks apart by eye. (The same rule
    /// `sprag_host::wire::SelectAsk::to_args` follows for an absent origin.)
    pub fn write_into(&self, params: &mut serde_json::Map<String, Value>) {
        match self {
            Self::Default => {}
            Self::Named(session) => {
                params.insert(SESSION_PARAM.to_owned(), Value::String(session.clone()));
            }
            Self::Attached => {
                params.insert(ATTACHED_PARAM.to_owned(), Value::Bool(true));
            }
        }
    }

    /// Write a WINDOW narrowing into a request's `params` — the ONE place a client spells one.
    ///
    /// A separate function rather than a field of the three arms, for [`WINDOW_PARAM`]'s reason: it
    /// is an orthogonal question, so it composes with whichever arm wrote the session. [`None`]
    /// writes NOTHING, so a request that does not narrow is unchanged byte for byte —
    /// [`write_into`](Self::write_into)'s own rule.
    pub fn write_window_into(window: Option<&str>, params: &mut serde_json::Map<String, Value>) {
        if let Some(window) = window {
            params.insert(WINDOW_PARAM.to_owned(), Value::String(window.to_owned()));
        }
    }

    /// The WINDOW a request narrows itself to, or [`None`] for the scoped session's current one.
    ///
    /// Parsed here rather than beside the caller so the whole scope grammar has ONE home: a
    /// resolver that read the session key from this type and the window key by hand would be two
    /// places deciding what a params object asks for.
    ///
    /// `null` is REFUSED rather than read as absent, which is [`parse`](Self::parse)'s rule and for
    /// its reason one level down: a window that reads as absent silently retargets the request at
    /// whichever window the session happens to be showing, and "wrong data for reads" is exactly
    /// what that rule exists to stop.
    ///
    /// # Errors
    ///
    /// [`ScopeFault::WindowNotAString`].
    pub fn window(params: Option<&Value>) -> Result<Option<String>, ScopeFault> {
        match params.and_then(|params| params.get(WINDOW_PARAM)) {
            None => Ok(None),
            Some(Value::String(name)) => Ok(Some(name.clone())),
            Some(_) => Err(ScopeFault::WindowNotAString),
        }
    }

    /// The ask a request's `params` names — the ONE place the scope keys are read.
    ///
    /// `params` is the whole params value (`None` for a request that carries none).
    ///
    /// # `false` is absent; `null` is NOT — and this grammar diverges from its neighbour on purpose
    ///
    /// `{"attached": false}` reads as [`Default`](Self::Default): a well-typed "no, not by my
    /// attachment" says the same thing as omitting the key, so a client that fills in a whole scope
    /// struct asks what one that omits it asks.
    ///
    /// `{"attached": null}` and `{"session": null}` are REFUSED, which is the opposite of the
    /// null-is-absent rule `sprag_host::wire::SelectAsk` follows one layer down. The divergence is
    /// deliberate and the asymmetry is the reason: a `select_pane` origin that reads as absent
    /// selects from the active pane, which is the commonest thing the caller could have meant,
    /// while a SCOPE that reads as absent silently retargets the request at the DEFAULT session —
    /// "wrong target for writes, wrong data for reads", the corner pinion's aliasing campaign
    /// missed for an entire round. A caller that put a key there was addressing something; where
    /// being wrong is unrecoverable, an unreadable address is refused rather than guessed.
    ///
    /// # Errors
    ///
    /// [`ScopeFault`], one variant per way a scope can be malformed.
    pub fn parse(params: Option<&Value>) -> Result<Self, ScopeFault> {
        let named = match params.and_then(|params| params.get(SESSION_PARAM)) {
            None => None,
            Some(Value::String(name)) => Some(name.clone()),
            Some(_) => return Err(ScopeFault::NotAString),
        };
        let attached = match params.and_then(|params| params.get(ATTACHED_PARAM)) {
            None => false,
            Some(Value::Bool(asked)) => *asked,
            Some(_) => return Err(ScopeFault::AttachedNotABool),
        };
        match (named, attached) {
            (Some(_), true) => Err(ScopeFault::TwoScopes),
            (Some(name), false) => Ok(Self::Named(name)),
            (None, true) => Ok(Self::Attached),
            (None, false) => Ok(Self::Default),
        }
    }
}

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
/// * **5** — `select_pane` gained an ORIGIN (`{dir, from}`, R300): the pane a directional step is
///   measured from, so an agent can ask for the pane next to a named one instead of joining a layout
///   read to a select. **The first bump caused by an added ARGUMENT rather than an added or moved
///   ANSWER.** An added answer key is absent-not-wrong to an old reader, which is why R299's
///   `outcome` moved nothing; an argument is the opposite — R294 measured an old daemon ACCEPTING an
///   argument it did not know and DROPPING it, and the request still parses. So a post-R300 client
///   asking a pre-R300 daemon for "the pane left of pane 7" would be answered "the pane left of
///   wherever the user happens to be", the user's cursor would move, and nothing anywhere would
///   report a failure. The number turns that into the one sentence it should be.
/// * **6** — a request can scope itself to the client's ATTACHMENT rather than to a session's name
///   ([`ScopeAsk::Attached`], R303). The second bump caused by an added ARGUMENT, and the one with
///   the worst failure yet if it were skipped: a post-R303 display client sends `{"attached":true}`
///   and NO [`SESSION_PARAM`], which to a pre-R303 daemon is an ABSENT scope — i.e. *the default
///   session*. Every read it paints and every KEYSTROKE it forwards would land in a session the
///   user never opened it on, and both ends would report success. There is no answer key to be
///   absent-not-wrong here; the argument's whole meaning is which session gets written to.
/// * **7** — a client can ask to be attached to the session it was viewing BEFORE this one
///   (`AttachAsk::LastViewed`, R304), and [`CLIENT_ATTACH_METHOD`] answers the name it landed on.
///   The THIRD bump caused by an added ARGUMENT, and it fails the same silent way: a pre-R304
///   daemon does not read `last`, so the attach falls through to the connection's SCOPE —
///   which for a display client is its own attachment. The client asks to go back where it was, the
///   daemon re-attaches it to where it already is and answers success, and the gesture is a no-op
///   nothing reports. The reply's change (a session NAME where a bare ok used to be) needs no bump
///   of its own — an old client discards it — but it is what a new client reads to learn where it
///   landed, so a new client against an old daemon would also read a name that is not there.
/// * **8** — `select_window` gained a RELATIVE arm (`{relative: "next"|"previous"}`, R305), so a key
///   can walk a session's windows and the daemon is the one that resolves the ring. The FOURTH bump
///   caused by an added ARGUMENT, and it fails the way that class always does: a pre-R305 daemon
///   does not read `relative`, finds no `window` key, and refuses the request as malformed — which
///   is the LOUD half. The quiet half is the answer: `select_window` now answers the window it
///   landed on where it used to answer `null`, so a new client reading that answer against an old
///   daemon would learn nothing about where it went.
/// * **9** — a WINDOW name has a grammar, and `rename_window` answers the name it RECORDED
///   (R306). The FIRST bump caused by a REFUSAL rather than by an argument or an answer, and the
///   two directions fail differently, which is why both were measured:
///   the OLD behaviour is what the refusal is about — MEASURED against a parent-commit daemon,
///   which answered `renamed to ` to `rename-window ""` and then listed a window with no name at
///   all, and which stored `  main  ` padding and all. That is the half a user sees.
///   The other direction is the QUIET one this number exists for: the prompt behind `prefix ,`
///   paints the recorded name off this answer, and a daemon that predates it answers `null` — so
///   without the bump a new client would report nothing about a rename that happened. With it, both
///   directions are refused at `client/hello` by number, which is the whole point.
///   The grammar itself the client can check on its own — it calls the same `WindowName` the
///   daemon does (named rather than linked: this crate does not depend on `sprag-terminal`) — but
///   what a daemon RECORDED is only ever the daemon's to say.
/// * **10** — a kill CASCADES and says how far it reached (R309). `close` (tmux `kill-pane`) now
///   ends the WINDOW its last pane emptied, which ends the SESSION and then the SERVER, and all
///   three kill actions answer `{ended}` where they answered `null`. The SECOND bump caused by
///   changed BEHAVIOUR rather than by an argument, and its two directions fail differently:
///   a NEW client against an OLD daemon asks a `kill-pane` that leaves an empty window behind, then
///   reads no `ended` key — and the honest reading of an absent key here is *"this daemon cannot
///   say"*, never "only the pane went", because the difference between those two is whether the
///   user's session still exists. A new client that assumed the cheapest answer would tell somebody
///   their window survived a kill that emptied it.
///   The OLD-client-against-NEW-daemon direction is the one a bump cannot fix by an absent key,
///   which is why it needs the number: an old client's `kill-pane` now DESTROYS a window (and
///   possibly the session) where the build it was compiled against destroyed one pane. Nothing in
///   the old answer could carry that, because the old answer was `null` for every case.
/// * **11** — a request can narrow itself to ONE WINDOW of its scoped session ([`WINDOW_PARAM`],
///   R311). The FIFTH bump caused by an added ARGUMENT, and it fails the way that class always
///   does — R294 measured an old daemon ACCEPTING an argument it does not know and DROPPING it,
///   with the request still parsing. Here the drop is the worst kind: a post-R311 agent asking to
///   read the pane called `buildout` in the window called `build` would be answered about
///   WHICHEVER WINDOW the session happens to be showing, and the reply is a well-formed screenful
///   of the wrong pane. There is no answer key to be absent-not-wrong about, because the argument's
///   whole meaning is which panes the request can see.
///   The other direction is refused by number and needs to be: an old client's reads were
///   window-scoped by construction and a new daemon changes nothing for them, but a new client that
///   sent the key and got the current window would report success about the wrong screen.
/// * **12** — a window can be born WITHOUT taking the screen (`detached`, R313). The SIXTH bump
///   caused by an added ARGUMENT, and the drop is the sharpest of the class so far because the
///   thing dropped is a promise about SOMEBODY ELSE: measured at `37d3971`, a daemon that does not
///   know the key accepts it, creates the window, and SELECTS it anyway — so every client attached
///   to that session is moved, and the answer (the new window's name) is byte-identical to the one
///   a detached create would have given. A caller that asked for a quiet workbench has taken over
///   the user's screen with nothing in the reply to say so.
///   The other direction is refused by number and needs to be for the usual reason: an old client
///   never sends the key and a new daemon treats its absence as `false`, which is exactly what that
///   client already got — but a new client that sent it and was ignored would report a quiet window
///   that is not quiet.
/// * **13** — a client can ask to be moved one step along the DAEMON's session order
///   (`{"step": "next"|"previous"}` on [`CLIENT_ATTACH_METHOD`], `sprag_host::wire::AttachAsk::Step`,
///   R314). The SEVENTH bump caused by an added ARGUMENT, and it is the same silent drop version 7
///   describes, one target wider: a pre-R314 daemon reads no `step` key, so the attach falls through
///   to the connection's SCOPE — which for a display client is its own attachment. A user presses
///   `prefix )`, the daemon re-attaches the client to the session it is already on, the reply
///   carries that same name, and the ANSWER IS INDISTINGUISHABLE from a legitimate one-session
///   wrap. Nothing on either side can tell the two apart, which is why this needs the number rather
///   than a check.
///   The other direction is refused by number and needs to be: an old client never sends the key,
///   so a new daemon changes nothing for it, but a new client whose step was dropped would report
///   that it had moved.
/// * **14** — a client can send a CHOOSER's pick, a path of identities
///   (`{"goto": {"session": N, "window": N?, "pane": N?}}` on [`CLIENT_ATTACH_METHOD`],
///   `sprag_host::wire::AttachAsk::Goto`, R315), and the daemon publishes the tree those identities
///   come from (`sprag_host::wire::TREE_SLOT`). The EIGHTH bump caused by an added ARGUMENT, and
///   the silent drop it prevents is version 13's with a longer reach: a pre-R315 daemon reads no
///   `goto` key, so the attach falls through to the connection's SCOPE. A person picks another
///   session's pane out of a list, the daemon re-attaches them where they already were, and the
///   reply carries that same session name — so the answer is INDISTINGUISHABLE from picking the row
///   they were on. The tree slot alone would not need a number (an unknown query path is refused by
///   address, loudly); the pick does, and the two ship together.
///   The other direction is refused by number and needs to be, for the usual reason: an old client
///   never sends the key and never reads the slot, so a new daemon changes nothing for it — but a
///   new client whose pick was dropped would report that it had gone somewhere.
/// * **15** — a message can be ADDRESSED to a client and the daemon says who got it
///   (`sprag_host::wire::DISPLAY_MESSAGE_ACTION`, [`CLIENT_MESSAGES_METHOD`], R317). The THIRD bump
///   caused by a whole new capability rather than by an added argument, and it was measured both
///   ways against a parent-commit daemon with a control on each side:
///   a NEW client against an OLD daemon is refused at `client/hello` by number, naming both versions
///   and the remedy — and the raw probe behind that shows WHY the number is what does it: a
///   protocol-14 handshake on the same connection SUCCEEDS, and `client/messages` then comes back
///   `-32601 'client/messages' is unsupported`. That read is on a display client's reconcile path,
///   so without the number a build skew would surface as an error on every wake rather than as one
///   sentence at the door.
///   The other direction is the one that MUST have the number: an OLD daemon has no mailbox at all,
///   so a message sent to it reaches nobody — and there is no key whose absence could say so, since
///   the whole answer is a delivery list that does not exist. Measured: the old CLI against the new
///   daemon is refused by number, while the new CLI on the same daemon delivers.
pub const WIRE_PROTOCOL: u32 = 15;

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

/// The JSON-RPC method a connection sends to COLLECT whatever the daemon is holding for its client
/// — `params: {}`, answering `{ "message": { "text": <string>, "severity": <word> } | null }`.
///
/// The read half of `sprag display-message`. A message is addressed to a CLIENT, so it cannot ride a
/// session-scoped slot: two clients on one session must not read each other's. It is resolved from
/// the frame's connection exactly as [`CLIENT_SIZE_METHOD`] and [`CLIENT_ATTACH_METHOD`] are, which
/// is also why a caller cannot ask for somebody else's — the parameter that would let it does not
/// exist.
///
/// ## It COLLECTS, and that word is load-bearing
///
/// The answer removes the message from the daemon, so one message is shown once. A cursor would have
/// been the alternative and is the wrong shape here: `events`'s `since` exists so a RECONNECTING
/// client resumes exactly where it left off, and a status-line sentence has no such need — a message
/// worth showing is worth showing now, and one whose client has gone is one nobody can be told about.
/// See `sprag_host::AttachmentRegistry::collect`.
///
/// ## When a client asks
///
/// On the wake it already has: a client's reconcile re-reads everything else the daemon owns, and
/// this rides that pass. No new poll and no new thread.
///
/// The daemon also bumps the change channel of every session it delivered into. **What that bump
/// contributes is NOT established** — a revert-proof deleting it left a settled cross-session
/// fixture green, and the target's `scene/revision` does not visibly move for a delivery either
/// way. It is kept because it is the only wake this code owns, and this comment says what was
/// measured rather than asserting the mechanism (see the R317 gate
/// `a_named_client_is_reached_from_a_request_scoped_to_another_session`). What IS measured: the
/// message arrives promptly, and an unrelated session's mutation does not wake a client, so the
/// per-session contract holds.
pub const CLIENT_MESSAGES_METHOD: &str = "client/messages";

/// The [`CLIENT_MESSAGES_METHOD`] reply key carrying the collected message, or `null` when the
/// daemon was holding nothing for this client.
pub const MESSAGE_FIELD: &str = "message";

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
/// **The instrument is in the tree** (`sprag-latency`'s poll-pair row, R320): it reproduces the rate
/// and names the mechanism — a follower's `since` is the JOURNAL's cursor, output bumps the SCENE
/// and writes no record, so `waitFor` answers from the catch-up path every time instead of parking.
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

/// The JSON-RPC method a client sends to follow a session's changes over ONE request —
/// `params: { "since": <cursor>, "match"?: <filter> }`, answering
/// `{ "subscription": <id>, "next": <cursor> }` once and thereafter writing one
/// [`EVENTS_CHANGED_METHOD`] NOTIFICATION per batch, unprompted.
///
/// ## Why this exists beside [`EVENTS_WAIT_METHOD`] rather than replacing it
///
/// The wait is correct and costs a **round trip per change**: its reply is a `FnOnce`, so a client
/// following a session re-issues the request after every batch. That was not a design choice —
/// until pinion R1552 a frame could be answered at most once, so "one request, many answers" was
/// *inexpressible on this transport at any price*. sprag filed it as PINION-PR83 and recorded the
/// consequence rather than working around it.
///
/// R1552 delivered a per-connection writer, and this is sprag's consumer of it. The WAIT stays,
/// because a one-shot question is a different question: an agent asking *"tell me when the build
/// finishes"* wants one answer and then to get on with it, and a tool call cannot hold a stream.
///
/// ## What a subscriber is promised, and what it is not
///
/// * **Exact resume.** `since` is the cursor the caller has already accounted for, the same
///   half-open [`SINCE_PARAM`] every other reader takes — so a client that reconnects passes the
///   last `next` it saw and continues **precisely** there. Nothing is skipped and nothing is
///   replayed. This is the contract the cursor vocabulary was already carrying; the subscription
///   simply stops paying a round trip for it.
/// * **The filter is the wait's filter**, evaluated server-side under the append's own lock. One
///   grammar for both, so a caller that has written a wait's `match` can follow with it unchanged.
/// * **Eviction is reported, never silent.** A batch whose `lost` is set says the ring overwrote
///   records this cursor had not read, which sends the caller to a full re-read — the same flag the
///   wait and the slot carry.
/// * **NOT an every-state feed.** Records are delivered in cursor order with none skipped, but a
///   subscription is not a promise about *timing*: several records landing between two writes
///   arrive as one batch. A caller wanting each record separately is asking for a different
///   transport, not a different parameter.
///
/// ## Intercepted, like the two waits, and for one more reason
///
/// It parks — a subscription outlives the frame that opened it — so it is handled in the host's
/// per-frame dispatch before the generic core, as the waits are. The additional reason is the
/// EGRESS: notifications go to the connection's writer, which only the frame carries, and a
/// transport that cannot be written to unprompted is refused **by name** at subscribe time rather
/// than registered as a stream that would be silent forever.
pub const EVENTS_SUBSCRIBE_METHOD: &str = "events/subscribe";

/// The JSON-RPC method a client sends to end a subscription —
/// `params: { "subscription": <id> }`, answering `{ "subscription": <id>, "delivered": <count> }`.
///
/// Ending it explicitly is the polite form; the connection CLOSING ends it too, however it closes,
/// because a subscription is per-connection state and the disconnect arm releases it exactly as it
/// releases a parked wait. So a crashed client leaks nothing and there is no cleanup to remember.
///
/// `delivered` is answered so a client can reconcile its own count against the daemon's without a
/// second method — the honest way to learn whether it missed a write.
pub const EVENTS_UNSUBSCRIBE_METHOD: &str = "events/unsubscribe";

/// The method name of the NOTIFICATION a subscription delivers — no `id`, so a client tells it
/// apart from an answer to something it asked.
///
/// A notification and not a second response, and the reason is JSON-RPC 2.0, section 5: one Response pairs
/// with one Request, and every client built on that pairing keys a pending map by `id` and REMOVES
/// the entry when the first answer lands. A second response carrying a live `id` is unreadable by
/// such a client. A notification (section 4.1 — a `method`, no `id`) is the one form that is separable
/// from a client's own answers on a channel they share, which is what LSP's `$/progress`, DAP and
/// `eth_subscription` all settled on. pinion's own `scene/changed` reaches the same conclusion from
/// its own harness.
///
/// `params` carries `{ "subscription": <id>, "events": [...], "next": <cursor>, "lost": <bool> }` —
/// the subscription's id first, then the same batch shape [`EVENTS_WAIT_METHOD`] answers with, so a
/// client's batch reader is one function for both.
pub const EVENTS_CHANGED_METHOD: &str = "events/changed";

/// The params key naming a subscription — answered by [`EVENTS_SUBSCRIBE_METHOD`], carried by every
/// [`EVENTS_CHANGED_METHOD`] notification, and taken by [`EVENTS_UNSUBSCRIBE_METHOD`].
///
/// Opaque and process-unique: a client compares it for equality and never derives anything from it,
/// so a daemon is free to change how one is minted.
pub const SUBSCRIPTION_PARAM: &str = "subscription";

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
///
/// Public because [`call`](HostConn::call) applies it to faults and
/// [`try_call`](HostConn::try_call) hands faults out unrendered: a caller that must ACT on which
/// fault it was, and then report the ones it cannot explain, needs to name them the way the other
/// method would. Without this it would keep a second copy of this format and be free to drift from
/// the messages every other caller prints.
#[must_use]
pub fn request_label(method: &str, params: &Value) -> String {
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
    /// What every request on this connection asks its scope to be ([`ScopeAsk`]) — the default
    /// session until a caller says otherwise. Set by [`scope_to`](Self::scope_to) (a name) or
    /// [`scope_to_attached`](Self::scope_to_attached) (this client's own view), and merged into
    /// each request's params by [`call`](Self::call) — the ONE place scoping happens, so a
    /// client's several connections (its request stream and its long-poll) cannot address
    /// different sessions.
    scope: ScopeAsk,
    /// Set once a read deadline expired mid-reply. See [`set_read_deadline`](Self::set_read_deadline)
    /// for why a timed-out connection can never be used again.
    timed_out: bool,
    /// NOTIFICATIONS read while waiting for a response, in arrival order.
    ///
    /// A subscription's batches share this connection with request/response
    /// ([`EVENTS_SUBSCRIBE_METHOD`]), so a call can read one before its own answer. Set aside rather
    /// than dropped, because a batch exists nowhere else: the daemon has advanced this
    /// subscription's cursor past it, so a discarded frame is data lost for good.
    ///
    /// Bounded in practice by how many batches land during one round trip, which is a handful; a
    /// client that opens a subscription and then never reads it is choosing to buffer, exactly as one
    /// that never reads its socket is.
    pending: VecDeque<Value>,
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
            scope: ScopeAsk::Default,
            timed_out: false,
            pending: VecDeque::new(),
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

    /// Scope every subsequent request on this connection to the session NAMED `session`
    /// ([`ScopeAsk::Named`]).
    ///
    /// The address form: it can name any session, including one this client is not viewing, which
    /// is what `client/attach` itself and every `-t` verb need. A DISPLAY client wants
    /// [`scope_to_attached`](Self::scope_to_attached) instead for its own reads — a name it
    /// re-sends can be retired under it and re-issued to another session (see
    /// [`ScopeAsk::Attached`]).
    ///
    /// Idempotent and settable again; a client scopes by name to attach, then moves to its
    /// attachment.
    pub fn scope_to(&mut self, session: impl Into<String>) {
        self.scope = ScopeAsk::Named(session.into());
    }

    /// Scope every subsequent request on this connection to the session THIS CONNECTION's client
    /// is ATTACHED to ([`ScopeAsk::Attached`]) — what a display client's own reads mean.
    ///
    /// The caller must have completed [`CLIENT_HELLO_METHOD`] on this connection (so the daemon
    /// knows which client it belongs to) and its client must have attached on one of them;
    /// otherwise every subsequent request is refused, exactly as an unknown name is. That is the
    /// same shape as any other scope that cannot be honoured, and it is the honest one: a client
    /// that is attached to nothing is viewing nothing.
    ///
    /// A client's several connections all resolve through the same `conn -> client -> session`
    /// map, so scoping them all here keeps the request stream and the long-poll on ONE session
    /// without any of them holding its name.
    pub fn scope_to_attached(&mut self) {
        self.scope = ScopeAsk::Attached;
    }

    /// Merge this connection's scope ([`ScopeAsk`]) into a request's params. Only an object
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
        self.scope.write_into(&mut map);
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

        // Read until THIS request's response, setting aside anything else.
        //
        // ## Why this is a loop over frames and not a read of the next line
        //
        // It was a single read until R298, and that was correct for exactly as long as the daemon
        // could only ever answer: one frame arrived per request, so the next line WAS the reply. With
        // `events/subscribe` the daemon also writes NOTIFICATIONS unprompted, on this same
        // connection — so a client that took the next line would read somebody's change batch as its
        // own answer, decode a `result` that is not there, and hand its caller `Null`.
        //
        // JSON-RPC 2.0 gives the discriminator and this is the whole of it: a response carries an
        // `id`, a notification (section 4.1) carries a `method` and no `id`. So a notification is SET ASIDE
        // for [`next_notification`](Self::next_notification) rather than dropped — it is somebody's
        // data, and this connection is the only place it exists — and a response whose `id` is not
        // the one just sent is dropped, because a client with one outstanding request can only be
        // looking at a duplicate.
        loop {
            let frame = self.read_frame()?;
            if frame.get("id").is_none() && frame.get("method").is_some() {
                self.pending.push_back(frame);
                continue;
            }
            if frame.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = frame.get("error") {
                return Err(CallFailure::Fault(RpcFault::from_wire(error)));
            }
            return Ok(frame.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    /// The next NOTIFICATION this connection has been sent, blocking until one arrives.
    ///
    /// The reading half of a subscription ([`EVENTS_SUBSCRIBE_METHOD`]): one request opens the
    /// stream and this reads each batch, with no further request and so no round trip per change.
    ///
    /// Notifications set aside by an in-flight [`call`](Self::call) are answered FIRST, in arrival
    /// order — which is what makes it safe to interleave calls with a follow. A client that opened a
    /// subscription and then asked something else has not lost the batches that landed in between.
    ///
    /// `method` is the notification this caller is following. A notification naming anything else is
    /// skipped rather than answered, because a caller reading one stream must not be handed
    /// another's; a RESPONSE arriving with nothing outstanding is skipped for
    /// [`call`](Self::call)'s reason.
    ///
    /// # Errors
    ///
    /// Whatever the read fails with, including the read deadline this connection carries — so a
    /// caller that wants to give up says so with [`set_read_deadline`](Self::set_read_deadline)
    /// rather than needing a second method.
    pub fn next_notification(&mut self, method: &str) -> io::Result<Value> {
        loop {
            while let Some(frame) = self.pending.pop_front() {
                if frame.get("method").and_then(Value::as_str) == Some(method) {
                    return Ok(frame.get("params").cloned().unwrap_or(Value::Null));
                }
            }
            let frame = self.read_frame()?;
            if frame.get("id").is_none() && frame.get("method").is_some() {
                self.pending.push_back(frame);
            }
        }
    }

    /// One non-blank line off the connection, parsed.
    ///
    /// Split out of [`call_inner`](Self::call_inner) when a second reader appeared, so the deadline
    /// bookkeeping below has ONE home: a deadline that expires mid-line leaves the stream at an
    /// unknown offset, and a connection in that state is retired rather than retried. Two copies of
    /// that rule is one copy that can forget it.
    fn read_frame(&mut self) -> io::Result<Value> {
        let mut line = String::new();
        loop {
            line.clear();
            let read = self.reader.read_line(&mut line).inspect_err(|error| {
                // Both spellings the platforms use for "the timeout elapsed" mean the same thing
                // here — see `set_read_deadline`.
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
                return serde_json::from_str(line.trim())
                    .map_err(|error| io::Error::new(ErrorKind::InvalidData, error));
            }
        }
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

    /// The grammar round trips through the BYTES, both ways, for every scope a caller can ask for
    /// — the check that keeps the writer and the reader one grammar rather than two that agree
    /// today. A key respelled on one side alone fails here.
    #[test]
    fn every_scope_round_trips_through_the_params_it_writes() {
        for ask in [
            ScopeAsk::Default,
            ScopeAsk::Named("work".to_owned()),
            ScopeAsk::Attached,
        ] {
            let mut params = serde_json::Map::new();
            // Written beside a key of its own, because a scope never travels alone: the merge in
            // `scoped` puts it next to the protocol declaration and whatever the method asked for,
            // and a parse that read the whole params object as its own would break on that.
            params.insert("path".to_owned(), Value::String("/x".to_owned()));
            ask.write_into(&mut params);
            assert_eq!(
                ScopeAsk::parse(Some(&Value::Object(params))),
                Ok(ask.clone()),
                "{ask:?}",
            );
        }
    }

    /// Every way a scope object can be malformed, each its OWN refusal — because the daemon turns
    /// each into a different sentence for an operator, and folding two together would make one of
    /// them a lie.
    ///
    /// `{"attached": false}` is in here as the CONTROL: it is the one present-but-empty spelling
    /// that is NOT a fault, so a parse that refused everything it did not understand would fail on
    /// this line rather than pass the whole test vacuously.
    #[test]
    fn each_malformed_scope_is_its_own_refusal() {
        let parse = |params: Value| ScopeAsk::parse(Some(&params));
        assert_eq!(parse(json!({"session": 42})), Err(ScopeFault::NotAString));
        assert_eq!(parse(json!({"session": null})), Err(ScopeFault::NotAString));
        assert_eq!(
            parse(json!({"attached": 1})),
            Err(ScopeFault::AttachedNotABool),
        );
        assert_eq!(
            parse(json!({"attached": null})),
            Err(ScopeFault::AttachedNotABool),
            "an explicit null is refused here and read as absent by `SelectAsk` one layer down — \
             see `parse`'s doc for why a SCOPE is the one place guessing is unrecoverable",
        );
        assert_eq!(
            parse(json!({"session": "work", "attached": true})),
            Err(ScopeFault::TwoScopes),
        );
        assert_eq!(
            parse(json!({"attached": false})),
            Ok(ScopeAsk::Default),
            "the CONTROL: a well-typed no is an absent key, not a fault",
        );
        assert_eq!(
            parse(json!({"session": "work", "attached": false})),
            Ok(ScopeAsk::Named("work".to_owned())),
            "and it does not poison the name beside it",
        );
        assert_eq!(
            ScopeAsk::parse(None),
            Ok(ScopeAsk::Default),
            "a request with no params at all asks for the default",
        );
    }

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

    /// **The reader tells a NOTIFICATION from its own answer, and loses neither.**
    ///
    /// This is the correctness pinion R1552 made necessary: until a daemon could speak unprompted,
    /// the next line WAS the reply, and [`HostConn::call_inner`] read exactly one. Now a subscription
    /// writes on the same connection, so a client that took the next line would read somebody's
    /// change batch as its own `result` — and, worse, would then never see the batch again, because
    /// the daemon has advanced that subscription's cursor past it.
    ///
    /// The dispatch here writes a notification BEFORE the response, which is the ordering that
    /// breaks a single-read client. Both halves are asserted: the call gets its OWN result, and the
    /// notification set aside during it is delivered afterwards rather than dropped.
    ///
    /// REVERT-PROOF (both measured): read one line and return it and the call answers `Null` — the
    /// notification's params have no `result`; drop the frame instead of queueing it and
    /// `next_notification` blocks until the deadline.
    #[test]
    fn a_notification_arriving_first_is_set_aside_and_not_read_as_the_answer() {
        let path =
            std::env::temp_dir().join(format!("sprag-rpc-notify-test-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let (tx, rx) = channel::<RpcFrame>();
        thread::spawn(move || {
            for frame in rx {
                let request: Value = serde_json::from_str(&frame.request).unwrap();
                // UNPROMPTED, and FIRST — the order a single-read client cannot survive.
                frame.egress.send_frame(
                    json!({
                        "jsonrpc": "2.0",
                        "method": "events/changed",
                        "params": { "subscription": 7, "events": ["landed"] },
                    })
                    .to_string(),
                );
                frame.reply.send(
                    json!({
                        "jsonrpc": "2.0",
                        "id": request["id"],
                        "result": { "mine": true },
                    })
                    .to_string(),
                );
            }
        });
        let control = UnixSocketTransport::serve(&path, Arc::new(ChannelIngress { tx }))
            .expect("bind the test socket");
        control.set_enabled(true);

        let mut conn =
            HostConn::connect(&path, Duration::from_secs(2)).expect("connect to the socket");
        assert_eq!(
            conn.call("scene/echo", json!({})).unwrap(),
            json!({ "mine": true }),
            "the call reads ITS OWN response, not the notification that arrived first",
        );
        // A deadline, so a reader that DROPPED the frame fails here in seconds instead of hanging.
        conn.set_read_deadline(Some(Duration::from_secs(5)))
            .expect("set a deadline");
        assert_eq!(
            conn.next_notification("events/changed")
                .expect("the notification was set aside, not discarded"),
            json!({ "subscription": 7, "events": ["landed"] }),
            "and it is delivered afterwards — a dropped frame is data lost for good, because the \
             daemon's cursor has moved past it",
        );

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
