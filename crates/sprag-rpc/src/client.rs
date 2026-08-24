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
use std::path::{Path, PathBuf};
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
/// * **16** — a request can address a window by the IDENTITY a client PICKED off a list it painted,
///   rather than by the name it was carrying then (`window_id`, `sprag_host::wire::WindowRef`,
///   R330). The NINTH bump caused by an added ARGUMENT. **Written at R335, five rounds late**: the
///   bump shipped in `7e2c5b2` with the reason in its commit message and no bullet here, which is
///   the one drift a list of reasons can carry and the reason this list exists.
///   The drop is the class's worst shape: a pre-R330 daemon reads no `window_id`, so a kill or a
///   join that carried only an identity is refused — the LOUD half — while the client that also
///   sends a name falls back to whatever holds that name NOW, which after a rename is a bystander
///   window. Measured at the registry: the two readings land on DIFFERENT windows across a rename.
///   The other direction is refused by number for the usual reason: an old client never sends the
///   key, so a new daemon changes nothing for it.
/// * **17** — a pane BROKEN OUT into a new window can say how that window is born: without taking
///   the screen, and recording who asked for it (`detached` / `opened_by` on
///   `sprag_host::wire::BREAK_PANE_ACTION`, R335). The TENTH bump caused by an added ARGUMENT, and
///   it is version 12's failure exactly — the same key, the same drop, the other window-creating
///   door. A pre-R335 daemon accepts the request, breaks the pane out, and SELECTS the new window
///   anyway, so a caller that asked to tidy up quietly has moved every attached client; the answer
///   (the new window's name) is byte-identical either way. `opened_by` fails more quietly still:
///   the window is created unclaimed, and the caller learns that only later, when the surface that
///   reads authorship refuses to close what this caller made.
///   The other direction is refused by number for version 12's reason: an old client never sends
///   either key and a new daemon treats their absence as the behaviour it already had, but a new
///   client that sent them and was ignored would report a quiet window that is not quiet.
/// * **18** — a pane can say the KERNEL refused to admit it, and the machine report can say the
///   panes never got into their cgroups (`sprag_terminal::Unmeasured::Refused`,
///   `sprag_terminal::Check::PaneAdmission`, R342).
///
///   ⚠ **THE FIRST BUMP CAUSED BY AN ANSWER'S VALUE SPACE RATHER THAN BY A NAME.** Every version
///   above moved for an added argument, an added method or a changed meaning — things the
///   surface pin in `sprag_host::wire` can see, because they are ADDRESSES. This one adds no
///   address and renames nothing: two enums that a client decodes WHOLE each gained an arm, so
///   the daemon can now answer a word that is inside the type's meaning and outside an older
///   build's copy of it. serde rejects an unknown variant outright, so the failure is not a
///   missing field a reader can default — it is the whole answer failing to parse.
///   Measured both directions against stand-in decoders of the previous shape:
///   `sprag doctor`'s report and `sprag resources`' rows each parse under this build and are
///   REFUSED by the older one, naming the variant.
///
///   The other direction is safe and stays safe: an old daemon simply never produces either word,
///   so a new client decoding its answer meets only arms it already had. That asymmetry is why
///   this is a version and not a capability check — the break is in the ANSWER, where a client has
///   nothing to negotiate with.
/// * **19** — a search match can SPAN A SOFT WRAP, so `sprag_host::PaneMatch` says which LINE it is
///   in and which ROW it starts on (`line`, `row`, `col`, `cols`, `wrapped`), and
///   `sprag_host::PaneFindLine` carries the whole logical line's text (R344).
///
///   ⚠ **THE SECOND BUMP FROM AN ANSWER, AND THE FIRST FROM A KEY THAT CHANGED MEANING.** Version
///   18 added words to a value space; this one keeps every key parsing and changes what one of
///   them SAYS. `line` used to be the retained row a match sat on, and is now the retained row its
///   LOGICAL line begins on — the same number for every match that does not start past a wrap, and
///   a different one for exactly the matches this version made findable. Nothing in the JSON can
///   tell those apart, which is precisely why it needs the number: an old client parses the new
///   answer perfectly and highlights the wrong row.
///   Measured against a stand-in decoder of the previous shape
///   (`sprag_host::wire::tests::a_reader_of_the_previous_shape_misreads_a_match_past_a_wrap`): it
///   ACCEPTS the new answer without complaint and reads the line's row for a match whose cells are
///   a row lower, while this build reads `row` and paints there — and the control shows the two
///   agree on every match that was findable before, which is why no pin and no test in the suite
///   could see the meaning move.
///   `wrapped` is absent-not-wrong on its own (an old reader would paint the head of a match and
///   miss its tail); the version is owed by `line` and `text`, not by it.
/// * **20** — a published call FORM says which SHAPE it is. `action_grammar` answers each verb a list
///   of `{form, args}` objects where it answered a list of argument ARRAYS, and every pane input verb
///   publishes its grammar for the first time (R353).
///
///   ⚠ **THE THIRD BUMP FROM AN ANSWER, AND THE FIRST FROM A VALUE THAT CHANGED SHAPE.** Version 18
///   added words to a value space and 19 changed what a key SAID; this one changes what a value IS,
///   from an array to an object. A client that walked `answer[verb][0]` as a list of arguments now
///   meets a map, so its very first index is the wrong kind — the failure is immediate rather than
///   subtle, which is the one mercy in it.
///
///   The shape had to move because a form could not say that its arguments are NOT an object.
///   `invoke("text", "한")` is how an IME commit reaches a pane, and three of that surface's six
///   verbs take a bare scalar — describing them as objects would have been an affirmative false
///   statement, and leaving them out would have kept the surface an agent uses most undiscoverable.
///   The added *addresses* (a per-pane `action_grammar`) are additive on their own and would not have
///   earned a bump; the changed value did.
///
///   ⚠ Two of the vocabularies this version publishes were spelled TWICE before it — the display
///   client encoded a mouse button and the host decoded it, in two crates, with nothing comparing
///   the lists. They read one array now, which is also why an OLD client is refused rather than
///   quietly mismatched: the handshake is at the daemon's door.
/// * **21** — a run can be `interrupted`. A daemon leaves its RUN LOG for its successor, and a run
///   that was still going when its process died comes back under a fourth `status` word instead of
///   vanishing (R357).
///
///   ⚠ **THE SECOND BUMP FROM A VALUE SPACE** (version 18 was the first). `state.status` on the
///   plugin host's `runs` slot answered `running` | `done` | `panicked`, and a peer that decodes a
///   closed set WHOLE fails the entire document on a word it has never seen — no address moves and
///   no shape moves, so neither the address pin nor the shape pin can see it.
///
///   What earned it is not the file. Persistence on its own is invisible on the wire: a successor
///   daemon could have reported restored runs as `done` and broken nothing. It would also have been
///   a LIE — a run killed mid-flight did not finish — and the whole reason to keep the record is
///   that *"no runs"* and *"the daemon that was running yours died"* are different answers a person
///   acts on differently. The honest word is what costs the number.
///
///   ⚠ The restored run's `opened_by` is DROPPED rather than carried, which is an authority
///   decision and not a serialization gap: panes come back across a restart but a restored pane's
///   OCCUPANT is a plain shell, so carrying the provenance would hand a NEW agent the previous
///   occupant's runs through `list_runs`'s own filter. See `RunRegistry::restore`.
/// * **22** — `ready_when` says WHICH QUESTION its marker is asking. A run's readiness barrier
///   answered `{"ready_when": "TOOL-UP"}` and answers `{"ready_when": {"match": "prints"|"shows",
///   "marker": "TOOL-UP"}}` on all three of the plugins that inject (R359b).
///
///   ⚠ **THE SECOND BUMP FROM A VALUE THAT CHANGED SHAPE** (version 20 was the first, an array to
///   an object). A string became an object, so an old caller's value is refused at the door rather
///   than read as one of the two meanings — which is the entire reason the shape moved.
///
///   What earned it is that ONE NEEDLE COULD NOT ANSWER BOTH QUESTIONS, and answering the wrong
///   one is silent. A marker matched against the whole screen is satisfied by text that was
///   already there, and the likeliest such text is THE ECHO OF THE COMMAND LINE THAT STARTED THE
///   PROGRAM — a pty puts it on screen before the program exists. Measured: a run told to wait for
///   `TOOL-UP` cleared the barrier in 50 MILLISECONDS against the echo of
///   `printf "TOOL-UP\n"; exec cat`, spent both its turns on the shell that was still there, and
///   the peer never saw a word.
///
///   The old meaning could not simply be tightened, because it is RIGHT for the other question: a
///   REPL already sitting at its prompt has that prompt on screen and will print nothing more
///   until it is fed, so demanding new output would wait for ever. The two are different KINDS and
///   nothing in the marker says which — **only the caller knows, so the type makes them say.**
///   A default would have re-answered every existing call silently, which is the failure this
///   whole ceiling exists to prevent.
/// * **23** — an `agent` run can say its peer SHOWS the prompt typed at it, and so be DELIVERED to
///   rather than written at (`shows_prompt` on `sprag_host::wire::AGENT_FORM`, R364). The ELEVENTH
///   bump caused by an added ARGUMENT, and it is version 17's failure exactly: the request is
///   accepted, the run converges, and the answer is byte-identical either way.
///
///   What earned it is that the argument buys a GUARANTEE and its absence is indistinguishable
///   from the guarantee holding. A pre-R364 daemon writes the prompt, its Enter and its Ctrl-D in
///   one injection and never looks — so a peer that discards what is typed at it while its own
///   input layer finishes starting is submitted to, end-of-input'd, and answers the empty question
///   it was left with. Measured: `REPLY[]` published to the caller **as the model's answer**, with
///   nothing in the outcome, the cost or the note to say the peer had never been asked. That is the
///   exact failure the caller sends this key to prevent.
///
///   ⚠ **And the silence is now load-bearing in the other direction too.** A new daemon that could
///   not confirm a delivery says so in the step's note; a client that has learned to read that
///   caveat reads its ABSENCE as *confirmed*, and an old daemon never writes one. So the quiet
///   half is not just a withheld guarantee, it is a false one.
///
///   The other direction is refused by number for version 12's reason: an old client never sends
///   the key, and a new daemon treats its absence as the behaviour it already had — the write, not
///   the delivery, which is what a one-shot peer that renders nothing needs.
/// * **24** — a pane input action says when what it just wrote MEANT a signal this pane will raise
///   none. `key`, `text` and `paste` answered `null` on every success and answer
///   `{"unsignalled": [{"key": "interrupt", "because": "raw"|"unbound"}]}` when the bytes that went
///   in were a `Ctrl-C` the terminal will not turn into one (`sprag_host::wire::UNSIGNALLED_KEY`,
///   R365).
///
///   ⚠ **THE THIRD BUMP FROM A VALUE THAT CHANGED SHAPE** (20 and 22 were the others), and version
///   23's argument in the same breath. `null` became `null | object`, so a client that decoded the
///   answer as a unit meets a map — but that is the smaller half. The larger one is that **the
///   silence is load-bearing**: a client that has learned to read this caveat reads its ABSENCE as
///   *the signal was raised*, and a pre-R365 daemon never writes one. The quiet half is not a
///   withheld warning, it is a false guarantee — which is exactly why the number moves rather than
///   the key being added quietly.
///
///   What earned it is that the write cannot report its own consequence. `0x03` becomes a `SIGINT`
///   only while the line discipline has `ISIG` set, which every editor, every full-screen TUI and
///   every interactive agent CLI clears on startup — and `PanePty::write` succeeds either way.
///   Measured (R363): a pane running `stty -isig; sleep 300`, sent `C-c` through this product's own
///   `send-keys`, echoes `^C` and the `sleep` lives on. The caller was told it sent a key.
///
///   ⚠ The byte is still WRITTEN and the call still succeeds. A person's `Ctrl-C` must reach a
///   full-screen program as input — refusing the write to protect the automation caller would break
///   the display client. This reports; it does not withhold.
/// * **25** — an `agent` run says WHAT MAKES ITS TURN OVER. `done_when` on
///   `sprag_host::wire::AGENT_FORM`, `{"match": "exits"|"settles", "agent": "claude"}`, where the
///   rule was hard-coded to *the pane's child exited* (R365).
///
///   ⚠ **THE TWELFTH BUMP CAUSED BY AN ADDED ARGUMENT**, and the first one whose additivity was
///   MEASURED rather than reasoned. `an_argument_this_surface_does_not_declare_is_swallowed_rather_than_refused`
///   sends the plugin host a key no version has ever declared: the run starts and converges. So an
///   older daemon does not refuse this key by name the way it refuses an unknown ADDRESS or ACTION
///   — it accepts the request and runs the other contract.
///
///   What earned it is what that silence costs. *The child exited* is a ONE-SHOT tool's completion;
///   a long-lived peer — an agent CLI that answers and goes on waiting — never exits, so its every
///   turn ran the whole reply timeout out (two minutes by default) and captured whatever was on
///   screen when it did. A caller naming `settles` to a pre-R365 daemon is answered `ok`, waits for
///   an exit that cannot come, and is handed a snapshot in the same shape a working call returns.
///
///   ⚠ The DEFAULT does not move: absent `done_when` is `exits`, which is what every existing call
///   already got. A default that re-answered them silently is the failure version 22's shape change
///   exists to prevent.
///
///   ⚠⚠ **AND `eof` IS REDEFINED IN TERMS OF IT, which is the same version's business and not a
///   separate one.** Absent `eof` stopped meaning `true` and started meaning *whatever the
///   completion contract implies* — send one under `exits`, none under `settles`, because an
///   end-of-input and a peer that stays alive are contradictory requests and a Ctrl-D into a
///   full-screen agent is a keystroke that may well mean *quit*. An explicit `eof` still wins.
///   Every pre-R365 call is unaffected: with no `done_when` the contract is `exits` and the
///   implication is the `true` they already had.
///
///   ⚠ One behaviour DOES change for an unchanged request: a run no longer writes a `Ctrl-D` into
///   a pane whose terminal is not in canonical mode, because there it could only arrive as an
///   ordinary byte. It says so in the step's note instead — at the moment of the decision rather
///   than after the reply timeout, which is where that diagnosis used to live.
/// * **26** — a run can end `blocked`. A peer that stopped to ASK ends the run under a fifth
///   `state` word instead of being typed at, and the question it is asking is published beside it
///   (`sprag_host::plugins::RUN_ASKING_KEY`, R365).
///
///   ⚠ **THE THIRD BUMP FROM A VALUE SPACE** (18 and 21 were the others), and version 21's argument
///   word for word: a peer that decodes a closed set WHOLE fails the entire document on a word it
///   has never seen, and no address moves and no shape moves, so neither the address pin nor the
///   shape pin can see it.
///
///   What earned it is the same honesty test R357 applied to `interrupted`. The run could have been
///   reported `failed` — it did stop — but a failed run wants something FIXED and this one wants an
///   ANSWER that is not the run's to give. Measured before the word existed: an orchestrator whose
///   peer popped a dialog after its first step typed the stimulus three more times and reported
///   `exhausted — iterations`, which tells a reader to raise a budget.
///
///   ⚠ The behaviour that goes with it is the point, not the word: an agent that stops to ask shows
///   a NUMBERED CHOICE LIST, a menu consumes keystrokes, and every injection these plugins make
///   ends with Enter — so a loop that kept going confirmed whatever option was highlighted. On a
///   tool-permission dialog that is an approval nobody read.
///
///   ⚠ `asking` is ABSENT rather than empty when the peer blocked on something this host cannot
///   read, which is a real case with its own remedy (hand the pane to a person). A caller tells the
///   two apart by the key's presence, this wire's rule for `ceiling` and `opened_by` already.
/// * **27** — a run can be given CONSENT to answer its peer's question, and every run says what it
///   answered and why it did not. `may_answer` on the three injecting forms
///   (`{"asked": …, "answer": …}`), a fifth `verdict` word (`answered`), a `why` beside a blocked
///   run's `asking`, and an `answered` tally on every outcome (R366).
///
///   ⚠ **THREE OF THIS WIRE'S FOUR BUMP CAUSES AT ONCE**, which is why it is one number and not
///   three: an added ARGUMENT (version 25's measurement — this surface SWALLOWS an undeclared key
///   and the run succeeds, so a caller naming `may_answer` at an older daemon is answered `ok` and
///   gets a run that will never answer anything); an added VALUE (`answered` joins the closed
///   `verdict` set a journal reader decodes whole, version 26's argument); and an added answer KEY
///   whose ABSENCE a reader would take as a claim.
///
///   That last one is version 24's shape and it is the sharpest here. A caller who has learned to
///   read `answered: 0` reads its absence as *this run answered nothing* — and a pre-R366 daemon,
///   which could not answer anything, never writes the key. The silence happens to be true today
///   and would be a false guarantee the moment anything about it changed, so it is not left to
///   luck.
///
///   ⚠ **THE DEFAULT DOES NOT MOVE, and that is the whole safety of the feature.** A run with no
///   `may_answer` answers nothing and reports `blocked` exactly as version 26 does. Answering a
///   peer's dialog is a decision with consequences outside the loop — a tool-permission prompt is
///   one of these — so it happens only where a caller named the question AND the option in advance,
///   and only where exactly one option on offer carries that answer. See
///   `sprag_plugin::Consent`, whose whole design is that a consent cannot stretch to cover a
///   question the caller did not picture.
/// * **28** — a BLOCKED PANE says what it is asking, on the pane-level surface. `asking` on a
///   pane's `agent` object in the `panes` slot — the same `{asked, choices:[{number, label,
///   selected}]}` a run's outcome carries, from the same parse (R367).
///
///   ⚠ **AN ADDED ANSWER KEY WHOSE ABSENCE A READER WOULD TAKE AS A CLAIM** — version 27's third
///   cause, and 24's shape. On a version-28 daemon, a `blocked` pane with no `asking` means *this
///   daemon looked at that screen and could not read a menu there*, whose remedy is a person. On
///   every older daemon it means *nothing ever looks*, and the two are indistinguishable to a
///   caller that has learned to read the key. No address moves and no shape moves, so neither the
///   address pin nor the shape pin can see it.
///
///   What earned it is what the silence cost. An agent watching a sibling pane go `blocked` had to
///   `read_pane` and re-derive the menu — for a question this daemon had ALREADY PARSED, off the
///   same screen, in the same instant, to publish on the RUN surface. So the parse existed, the
///   answer existed, and the surface an agent actually watches its neighbours through published
///   the state without it. Re-deriving from scraped text is where a supervisor mistakes *"2. No,
///   and tell Claude what to do differently"* for an approval.
///
///   ⚠ The pane's object carries NO `why` beside it, unlike a run's. A run may be given a consent
///   and refuse to use it, and owes a reason; a pane was given none and refuses nothing. Inventing
///   the key here to make the two objects match would publish a refusal nobody made.
/// * **29** — a run's consent is a LIST, because ONE TURN ASKS MORE THAN ONE QUESTION. `may_answer`
///   changed shape from `{"asked": …, "answer": …}` to `[{"asked": …, "answer": …}, …]` on all five
///   forms that take it, and a seventh `why` word (`contradicted`) says what a list can do that a
///   single clause could not (R370).
///
///   ⚠⚠ **A VALUE THAT CHANGED SHAPE** — this wire's second bump cause, and the first time it has
///   been earned by an ARGUMENT rather than by an answer. Neither pin can see it: the address is
///   the same, the key is the same, and the surface pin catches an added name rather than a
///   re-typed one. What moves is what the value IS, in both directions — a version-28 client's
///   object reaches a version-29 daemon as `TypeMismatch`, and a version-29 client's array reaches
///   a version-28 daemon as `TypeMismatch`. Both are the safe direction: the call is refused at the
///   door rather than half-read, so neither side can answer a dialog under a consent the other one
///   spelled differently, which is the only outcome that would have been worse than a bump.
///
///   What earned it is what one clause could not do. Measured on a turn shaped like a real one — an
///   agent that runs a command and then edits a file asks *"Bash command … Do you want to proceed?"*
///   and then *"Edit file … Do you want to make this edit?"* — an unattended run answered the first
///   and stopped at the second reporting `other_question`. Correct, honest, and still a run a
///   person has to come back to, which is the case this contract exists to serve at all. A caller
///   leaving a run unattended has to be able to write down every decision they have already made,
///   and no number of single-clause runs adds up to that: the clauses have to be weighed against
///   ONE question, together.
///
///   ⚠ **AND THE WIDENING BROUGHT ITS OWN REFUSAL, which is why the `why` vocabulary moves too.**
///   Two clauses about one question naming DIFFERENT options is a caller who has written a broad
///   rule and a narrow exception, and nothing on this wire says which outranks which. Answering
///   either would be a precedence policy nobody chose, so it is `contradicted`, nothing is typed
///   and the run stops — version 26's argument about a closed set a reader decodes whole, applied
///   to the vocabulary that says why a peer was left for a person.
///
///   ⚠ **THE DEFAULT STILL DOES NOT MOVE.** A run with no `may_answer` answers nothing and reports
///   `blocked`, exactly as versions 26, 27 and 28 do. And an EMPTY list is malformed rather than a
///   second spelling of that default: `[]` arriving by accident — a client that built its clause
///   list from a filter that matched nothing — is precisely the caller who wants telling.
///
/// * **30 — AN ADDED REQUEST ARGUMENT: `await_person_ms`, on the three forms that LOOP.** A run
///   may now be told that somebody is watching the pane it drives, and wait that long for them
///   instead of ending the moment its peer asks something no clause covers.
///
///   ⚠⚠ **THE NUMBER MOVES FOR VERSION 25's MEASURED REASON AND NOT FOR A WIDENED SPACE.** This
///   surface SWALLOWS an argument it does not declare and answers `ok`
///   (`an_argument_this_surface_does_not_declare_is_swallowed_rather_than_refused`), so a client
///   that asks an older daemon to wait for a person gets a run that reports `blocked` the instant
///   its peer asks — the exact behaviour the caller paid an argument to avoid, reported as a
///   success. The handshake is what turns that into a refusal a client can read.
///
///   ⚠ **AND THE WIDENING BRINGS ITS OWN REFUSAL AGAIN**, the eighth `why` word: `unattended`, a
///   run that waited for the person it was promised and gave up. It is the only reason in that
///   vocabulary about a HUMAN rather than about a clause, and it has a remedy of its own — the
///   clause-level reason rides underneath it in the free-text detail rather than being replaced,
///   so a caller learns both what they would have been answering and that nobody came.
///
///   ⚠ **THE DEFAULT DOES NOT MOVE, AND NEITHER DOES WHAT A RUN MAY DECIDE.** Absent, a run is
///   unattended and behaves exactly as 26 through 29 do. Present, it still types nothing of its
///   own: `may_answer` remains the only thing that can put a byte into a dialog, and this argument
///   only widens what a run may WAIT for. Zero is malformed rather than a second spelling of
///   *"nobody"*, for the empty list's reason one version above.
/// * **31 — A SIXTH OUTCOME AND A FIFTH VERDICT: `taken_over`, because A PERSON CAN TAKE A PANE A
///   RUN IS DRIVING.** A run that finds somebody typing into its pane stops driving and reports
///   that, where before it could not find out at all.
///
///   ⚠⚠ **A WIDENED ANSWER VALUE SPACE — this wire's first bump cause, and the pin that guards it
///   said so before the number moved** (`an_answers_value_space_cannot_widen_under_the_protocol_
///   number`, which named `outcome:taken_over` and `verdict:taken_over`). A peer decodes these
///   enums WHOLE, and serde fails the entire document rather than the field, so a version-30 client
///   reading a version-31 run's outcome does not get an unknown word — it gets nothing. The
///   handshake is what turns that into a refusal it can read.
///
///   ⚠⚠⚠ **WHAT EARNED IT, MEASURED BEFORE ANY OF IT EXISTED.** A person reached into a pane an
///   orchestrator was driving and typed one key. The run typed its stimulus at them **twice more**
///   and reported `exhausted — iterations`, which tells whoever reads it to raise a budget. It
///   could not have done better: `sprag_host::pane::send_key` is one encoder shared by a display
///   client's keyboard and the `scene/invoke` wire — *deliberately*, so the two encode identically
///   — and nothing downstream recorded WHICH had written. The fix is at that door
///   (`sprag_terminal::Hand`): the bytes are still encoded identically and the HAND is now
///   recorded, so the barrier every injecting plugin passes through can ask.
///
///   ⚠ **A WORD OF ITS OWN RATHER THAN A FLAVOUR OF `blocked`**, which is version 26's argument
///   applied to the opposite fact. `blocked` is *the PEER stopped to ask and nobody answered*; this
///   is *a PERSON is here and already acting*. A reader told the first goes looking for a question
///   to answer; a reader told the second must do nothing at all. Folding them would make the report
///   false in the direction that matters — `blocked` says nobody came, and somebody did.
///
///   ⚠ **THE DEFAULT DOES NOT MOVE, AND NEITHER DOES WHAT A RUN MAY DECIDE.** No argument was
///   added: every run gets this, because typing over somebody was never a behaviour a caller chose.
///   A host that cannot name the hand (`PaneAccess::hands` absent) drives exactly as 26 through
///   30 do — an absence of evidence that a person is present is never read as evidence that one is,
///   which is gated (`a_host_that_cannot_name_the_hand_keeps_driving`).
///
///   ⚠ **WHAT IT DOES NOT DO: RESUME.** `ai_loop.scxml` has an edge back from `awaiting_human`, and
///   taking it automatically needs a measured answer to *when has somebody stopped typing*. This
///   version does not have one and did not guess: the run reports and ends, and a supervisor starts
///   the next one, exactly as they do for `blocked`.
/// * **32 — A MALFORMED MEMBER GETS ITS OWN REFUSAL.** All ELEVEN parametric families
///   (`cells.` `find.` `regex.` `image_data.` · `session_activity.` `pane_processes.` `doctor.`
///   `pane_resources.` · `events.` `neighbors.` `project.`) answered `null` for an argument that is
///   not the declared type. They now refuse with `QueryTypeMismatch` (`-32602`).
///
///   ⚠⚠⚠ **A VALUE THAT BECAME A REFUSAL — this wire's third bump cause, and the first time NO PIN
///   COULD SEE IT.** The address did not move, the argument shapes did not move, and the answer
///   ENUMS did not move: `QueryTypeMismatch` is pinion's word, not one of sprag's own closed
///   vocabularies, so `PINNED_VALUES` never carried it. What changed is that a path which used to
///   hand back a document now hands back a fault — invisible to all four pins by construction, and
///   the reason this entry exists rather than a re-stamp.
///
///   ⚠⚠⚠ **WHAT EARNED IT: ONE `null` WAS CARRYING TWO FACTS WITH OPPOSITE REMEDIES.** At every one
///   of the eleven, the same `null` was also what a serialisation failure degraded to
///   (`encoded_answer(..).unwrap_or(Null)`). So *fix your argument* and *this daemon could not
///   encode its own reading* reached a client as one answer it could not tell apart. Driven live
///   against the shipped daemon before any of this was built: `scene/query` on `…/events.zzz`
///   answered `null`. R155 chose that correctly against the API it had — `query` returned an
///   `Option`, and there was no third thing to say. pinion R1667/R1674 built the third thing.
///
///   ⚠ **AND THE ELEVENTH FAMILY WAS LYING ABOUT ITS OWN ADDRESS.** `project.` was the catch-all
///   arm of its surface, where `strip_prefix(..)?` (*not my address* — correct) and
///   `.parse().ok()?` (*malformed member* — a lie) both fell to one `None`, so `project.zzz`
///   answered `UnknownIntrospectPath` about a path `$schema` publishes. `project.` now joins the
///   ten declared empty members: an ADDED name, which alone would not move the number.
///
///   ⚠ **`NoSuchMember` IS NOT ADOPTED.** *"Well typed, addresses nothing"* (`image_data.<id>` for
///   an image the pane is not showing) is a per-path decision about what a surface knows, and
///   inventing eleven sentences for it is not this round's to do.
///
///   ⚠ **A DEAD SCOPE STOPPED SWALLOWING THE NEW REFUSAL.** The registry-only door served only what
///   answered a VALUE, so a caller on a destroyed session asking `session_activity.zzz` was told
///   *"no session named …"* — sending them to re-attach over a typo. It now falls through only on
///   `UnknownIntrospectPath`, the one answer that means *not mine*.
/// * **33 — A PANE A PERSON TOOK CAN COME BACK.** The three LOOPING run forms (`orchestrator`,
///   `pipe`, `agent`) gained an optional `handback_still_ms`: how long a person's hand must be
///   STILL before the pane they took is the run's again. Absent is version 31's behaviour exactly —
///   the run reports `taken_over` and ends.
///
///   ⚠⚠⚠ **AN ADDED ARGUMENT, WHICH IS THIS WIRE'S SECOND-COMMONEST BUMP CAUSE AND THE ONE MOST
///   EASILY TALKED OUT OF.** This surface IGNORES an undeclared key and the run SUCCEEDS (measured
///   live at R371), so a client that sends this to a version-32 daemon does not get an error — it
///   gets a run that ends on the first keystroke while its request plainly asked the daemon to wait.
///   The failure is silent, it is in the direction of doing LESS than asked, and the caller cannot
///   see it in the result: the outcome word for *"a person took it and I gave up"* is the same word
///   as for *"a person took it and I never had permission to wait"*.
///
///   ⚠⚠⚠ **WHAT EARNED IT: `turn.interrupted` WAS BUILT AT 31 AND ONLY HALF OF IT.**
///   `ai_loop.scxml`'s `awaiting_human` is a WAITING state with four exits, of which one ends the
///   run; version 31 had the ending and no waiting. Measured before this was built, against the
///   shipped barrier: a supervisor typed ONE key into a pane a run was driving, finished, and let
///   go — and the run ended holding **thirty-seven of its forty iterations unspent**, its goal one
///   turn away, with `HANDED BACK` on the screen and nothing after it.
///
///   ⚠⚠ **AND VERSION 31's REFUSAL TO GUESS IS WHAT THIS KEY IS.** 31 says above: *taking that edge
///   automatically needs a measured answer to "when has somebody stopped typing", and this version
///   did not guess.* It still does not. Nobody but the caller knows how long a still hand means
///   done — a supervisor answering one dialog is a second, one editing a file by hand is a minute —
///   so the caller says it, which is `await_person_ms`'s own argument one door over.
///
///   ⚠ **THE PAIR IS ONE REQUEST.** `handback_still_ms` without `await_person_ms` is MALFORMED
///   (`-32602`), not a quiet *"nobody is watching"*: the type puts `Handback` inside
///   `Attended::APerson`, so a handback for a run nobody watches cannot be constructed, and
///   answering `NoOne` would hand the caller the opposite of what they sent. **Zero is malformed**
///   too, `await_person_ms`'s rule — every person pauses between keystrokes.
///
///   ⚠ **NO ANSWER WORD MOVED.** A pane coming back is not a decision this run made, so it is
///   `continue` with a journal note, exactly as a person's ANSWER is (version 30's ruling: a run
///   that counted a human's answer as its own would lose the distinction that makes an approval
///   traceable). `taken_over` still means what it meant — the person still has it.
///
///   ⚠⚠⚠ **AND THE SAME VERSION CARRIES A SECOND ADDED ARGUMENT, WHICH IS WHAT MADE VERSION 31
///   WORK AT ALL.** The three pane-input verbs that WRITE (`key`, `text`, `paste`) gained an
///   optional `hand: "person" | "program"` on their OBJECT forms. Absent is `program` — every
///   existing caller unchanged.
///
///   Version 31 taught a pane to record whose hand wrote each input, and its own note said the
///   display client's path was the one stamped `person`. **That was a premise nothing checked:
///   there is no in-process display client in production.** Both frontends attach over this socket
///   through `sprag_client::WireHost`, whose `send_key` is a `scene/invoke` on the pane's input
///   surface — the door stamped `program`. So a person typing at `sprag-tui` or `sprag-gui` was
///   recorded as a program and no supervised run could ever see them. Measured end to end through a
///   real client, in the round that added the handback: the control run — the one that must report
///   `taken_over` — **converged**, because the person it was meant to notice was invisible.
///
///   ⚠⚠ **A NEW CLOSED VOCABULARY ON THIS WIRE**, `sprag_terminal::Hand::WIRE_WORDS`, which the
///   value-space pin walks. The daemon cannot infer this: an RPC caller may be an agent or a
///   keyboard, and only the caller knows which. Absent means `program` because that is the half
///   that cannot be claimed by silence — an unauthenticated caller may not pass for a person by
///   omitting an argument.
///
///   ⚠ NOT on the scalar spellings (a bare string has nowhere to put it) and not on `mouse` or
///   `focus`, which stay `program` by version 31's reasoning: a hover would make a false positive
///   of the whole signal, and a focus edge is raised by the window system, which has no hand.
/// * **34 — AN `orchestrator` RUN SAYS WHAT MAKES ITS PEER'S TURN OVER.** Two optional arguments on
///   that form: `done_when` — the word the `agent` form has taken since version 25 — and
///   `turn_within_ms`, how long one turn may take. Absent is the behaviour every run has always
///   had, and that is load-bearing rather than polite.
///
///   ⚠⚠⚠ **WHAT EARNED IT, MEASURED BEFORE ANY OF IT WAS BUILT.** A step that has typed its
///   stimulus has to decide when to stop waiting, and this plugin decided it with a 500 ms
///   constant. Against a peer that thinks for THREE SECONDS the run spent **six turns on one
///   question** (`iterations: 6`, `Bytes(30)`), every prompt after the first landing while the peer
///   was still answering the one before. Scaled to a `claude` turn of half a minute that is sixty
///   prompts, and each is a turn of that agent's own bounded budget spent re-answering a question
///   it already had. **The `agent` form never had the defect** — it asks a `DoneWhen` instead of a
///   clock — so this is not *"panes are hard"*, it is an asymmetry between two plugins in one
///   crate, and the one without the contract is the one this verb and the outer AI loop drive.
///
///   ⚠⚠ **AN ADDED ARGUMENT, WHICH IS THIS WIRE'S SECOND-COMMONEST BUMP CAUSE.** This surface
///   IGNORES an undeclared key and the run SUCCEEDS, so a client that sends these to a version-33
///   daemon gets no error — it gets the 500 ms timer back, and a run that re-prompts a working
///   agent while its request plainly asked the daemon to wait for the turn. Silent, and in the
///   direction of doing MORE than asked.
///
///   ⚠⚠⚠ **THE PAIR IS ONE REQUEST**, `handback_still_ms`'s rule and for a sharper reason: the type
///   holds both (`sprag_plugin::Turn`), so half of it cannot be constructed. `done_when` alone is a
///   contract with no bound — a run that would wait for ever on a peer that never finishes;
///   `turn_within_ms` alone would quietly become *"wait this long, then type at it anyway"*, which
///   is the timer the caller was getting away from with a bigger number. Both are `-32602`. **Zero
///   is malformed** too, `await_person_ms`'s rule: *wait no time at all for my peer to finish* is
///   not a thing a caller can mean.
///
///   ⚠ **NO ANSWER WORD MOVED, AND NO VOCABULARY IS NEW.** `done_when`'s two words have been on
///   this wire since 25; what is new is the argument's presence on a second form, and the bound
///   beside it.
///
///   ⚠ NOT on `pipe`, whose destination has turns too. A scope cut, named rather than implied.
/// * **35 — A RUN THAT WAS STOPPED WITH A KEY ALREADY SENT SAYS SO, INSTEAD OF BLAMING THE PEER.**
///   An ELEVENTH `asking.why` word, `unwitnessed`, and no new key, argument or form.
///
///   ⚠⚠⚠ **WHAT EARNED IT, MEASURED BEFORE ANY OF IT WAS BUILT.** Two acts in this daemon press a
///   key at a dialog and then wait to see what became of it: the answering act types the option a
///   consent authorised, and the screening act presses the key that refuses a tool call. Each ended
///   its wait with a word about the PEER — `not_taken` (*"the run typed the option and did not see
///   the peer take it"*) and `not_dismissed` (*"the key went in and the dialog stayed"*) — and each
///   said it just as readily when the wait had not finished at all, because the RUN was cancelled
///   or out of time inside it. Against a fixture peer that commits the option it is given, the run
///   typed the digit, saw the marker land, sent the Enter and was stopped: the peer's own screen
///   read `TOOK 2 VIA 10` and the run reported **`not_taken`**. A supervisor acting on that hands a
///   person a pane whose dialog is already answered, and a tally of refusals counts an agent's
///   fault where there was none.
///
///   ⚠⚠ **AN ADDED ANSWER WORD, ONE OF THIS WIRE'S FOUR NAMED BUMP CAUSES**, and reachable by
///   every client that predates this build: `may_answer` has been on the injecting forms since
///   version 27 and `must_answer` is the whole content of the `answer` form, so an old caller whose
///   run meets its own `max_duration_ms` mid-answer receives this word today. That is what
///   separates it from version 34's `no_rule` and `not_dismissed`, which cost nothing because only
///   `ai_loop` — a plugin no older client can select — can produce them.
///
///   ⚠ **THE SAFE BEHAVIOUR IS UNCHANGED, deliberately.** The run still stops, still reports the
///   question, still charges every byte it really sent, and still types nothing further. What moves
///   is the SENTENCE: *read the pane, and give the run longer* rather than *your peer ignored it*.
///   A word rather than free text by this wire's standing test — the remedy differs from every
///   other arm's, and a caller branches on it.
///
///   ⚠ It is the sibling of `Delivered::Unwitnessed`, which the round before this one built for the
///   submit keystroke and spent no number on, because a delivery is a Rust API this wire does not
///   publish. **The rule the two share: a run that stopped may report what it did, never what the
///   other side did about it.**
/// * **36 — A RUN WHOSE PEER'S PROGRAM HAS EXITED SAYS SO, INSTEAD OF TYPING INTO A TERMINAL
///   NOBODY IS READING.** An EIGHTH `verdict` word, `peer_gone`, and no new key, argument or form.
///
///   ⚠⚠⚠ **WHAT EARNED IT, MEASURED BEFORE ANY OF IT WAS BUILT — AND IT COST 43 HOURS OF A BUILD
///   MACHINE FIRST.** Bytes written to a pseudoterminal master land in the slave's input queue;
///   with nobody reading it the queue fills and the next `write(2)` **blocks for ever**, holding
///   the pane's shared writer lock, and a blocked write cannot be cancelled. Measured on this
///   workstation through the product's own door: a dead pane takes **16,896 bytes** of
///   newline-terminated input and then parks. (Without the newline it takes a megabyte in 0.09 s
///   and never blocks — a cooked tty will not hold a line it has no end for. **The arm that wedges
///   is the arm an agent loop is made of**: a prompt, then Enter.)
///
///   ⚠⚠⚠ **AND NOTHING ABOUT THE WALK TO IT LOOKED WRONG.** An `orchestrator` types its stimulus at
///   the start of every step: **5 bytes and 509 ms a step, so 3,380 steps — about 29 minutes —
///   from a dead peer to a wedged machine.** Not a burst; a patient march, which is why nobody saw
///   the hours being spent. The run reported nothing wrong the entire time.
///
///   ⚠⚠ **AN ADDED ANSWER WORD, ONE OF THIS WIRE'S FOUR NAMED BUMP CAUSES**, and version 34's
///   escape does not reach it. `no_rule`, `not_dismissed` and `ceiling: "turns"` are free because
///   only `ai_loop` — a plugin no older client can select — produces them. This word is produced by
///   `orchestrator` as well, a form every version of this wire has been able to send, and that
///   plugin is the one the preserved stack showed inside the wedge. An old client receives it
///   today.
///
///   ⚠⚠ **THE BEHAVIOUR THAT GOES WITH IT IS THE POINT, NOT THE WORD**, and it is the first time
///   this wire has bought a refusal to WRITE. `PaneAccess::inject` — the one door every plugin
///   types through — now declines at a pane whose child has exited, so a run that used to walk to
///   the wall stops on its first step and says which pane went. What a caller loses is the silent
///   headroom under the threshold, which was camouflage rather than service: every injection below
///   it LOOKED like it succeeded, and then one parked.
///
///   ⚠ **A PERSON'S KEYSTROKES ARE NOT AFFECTED BY *THIS* REFUSAL**, deliberately, exactly as
///   version 24 left the `Ctrl-C` write alone: `send-keys` from a keyboard is a different door, and
///   refusing it to protect an automation caller would break the display client.
///
///   ⚠⚠ **AND THAT SENTENCE HAS SINCE BEEN QUALIFIED WITHOUT A NUMBER, WHICH IS ITSELF A
///   JUDGEMENT.** The door was only half of the 43 hours: the keyboard's own route still held the
///   pane's writer lock across the blocking `write(2)`, so one dead pane still stopped every write
///   to it. That is now bounded rather than refused — a pane's device has a thread of its own and
///   a caller waits at most half a second — so a person's `send-keys` at a pane whose device has
///   stopped taking input can come back `false` where it used to come back never.
///   **No number, and the reason is reachability**: that state cannot be entered in the old shape
///   without a writer already parked for ever, so no client of any version had an answer here to
///   lose. No key, argument, form or answer word moved; one boolean gained a reason. See
///   `sprag_terminal::pane_pty`'s `DeviceInput`, where the trade is written out.
/// * **37 — AN AGENT REPORTS WHAT IT WAS ASKED, SO A DELIVERY STOPS BEING GUESSED FROM PIXELS.**
///   Two optional arguments on `report_agent`: `asked`, the prompt the agent states it received,
///   and `transcript`, the file it states it is writing.
///
///   ⚠⚠⚠ **AN ADDED REQUEST ARGUMENT, WHICH IS ONE OF THIS WIRE'S NAMED BUMP CAUSES AND IS
///   MEASURED RATHER THAN ARGUED** — `an_argument_this_surface_does_not_declare_is_swallowed_rather
///   _than_refused` sends a key no version declared and the call **succeeds**. So a reporter that
///   names `asked` to a daemon predating this key is answered `accepted` while the fact is dropped,
///   and a delivery waiting on that evidence would wait for something the daemon threw away. Same
///   shape as 34 (`done_when`) and for the same reason.
///
///   ⚠⚠⚠⚠ **WHY IT IS WORTH A NUMBER: THE SCREEN CANNOT ANSWER THE QUESTION AND NEVER COULD.**
///   Whether a prompt arrived has been decided by hunting a fragment of the typed text on the
///   pane, and each way that failed bought another predicate — 40 characters became 40 COLUMNS
///   when a Korean prompt asked for twice the pane's width; the HEAD became the TAIL when a
///   composer was found to scroll it away; an exact match became a whitespace-insensitive one when
///   the box was found to re-wrap what it was given. Three live runs died at that predicate in one
///   evening, the last of them on a prompt the caller could not shorten. **The gate at
///   `a_prompt_typed_onto_a_dirty_composer_is_confirmed_and_submitted_anyway` had already recorded
///   that tightening it is ruled out** and that what is needed is *"evidence from the PROGRAM
///   rather than the screen"*. The program was already sending it: `UserPromptSubmit`'s payload
///   carries `prompt` and `transcript_path`, captured from a real agent, and sprag reduced the
///   whole message to the single word *working* and dropped the rest.
///
///   ⚠⚠ **NO ANSWER WORD MOVED.** `report_agent` answers `{accepted, changed}` as it always has;
///   what a caller may SAY grew, and what it is told did not.
///
///   ⚠ **AND THE BUMP HAS A COST THIS REPOSITORY HAS PAID BEFORE (register item 344)**: the hook
///   binary is hardlinked to `target/debug/sprag`, so building this silences every live agent's
///   reporter until the daemon is restarted — a hook refused at `client/hello` leaves the last
///   `working` true for ever. Build and restart are one act, not two.
///
/// * **38 — AN ADDRESS WAS WITHDRAWN, WHICH IS THE HALF OF THIS NUMBER'S JOB THAT RARELY COMES UP.**
///   Register item 567. `recent_input` served a pane's whole **echo trail** as a string; it is gone,
///   and `recent_input_has.<needle>` answers a bool in its place. A client that asks the old address
///   is answered nothing — the first REMOVAL on this wire rather than an addition, so an older
///   client genuinely breaks and the number must say so.
///
///   ⚠⚠⚠⚠⚠ **WHY THE TRAIL COULD NOT STAY PUBLISHED.** Anything holding this socket can already
///   inject keys, spawn processes and read every screen — the socket is the trust boundary, and no
///   read grants a privilege a writer did not have. What the trail added is the one class of text a
///   wire read reaches that a SCREEN read cannot: **input the terminal was told not to echo.** A
///   password typed at a `sudo` or `ssh` prompt is in the trail and is nowhere on the grid, so a
///   client that only READS could harvest it.
///
///   ⚠⚠ **AND NOTHING WANTED THE TRAIL.** Measured with the compiler rather than argued: removing
///   the trail method from `PaneInputEcho` produced exactly ONE error outside tests —
///   `ReadyWhen::Prints`' refusal, which asks *is my marker in what was typed* about a marker the
///   caller already holds. Every other reader is a gate running in the same process as the panes,
///   and those keep the trail through `PaneInputTrail`, a capability a remote surface declines
///   outright rather than answers emptily.
///
/// * **39 — A PANE CAN BE WAITED ON FROM OUTSIDE THE DAEMON.** Register item 631.
///   [`PANE_WAIT_REVISION_METHOD`] is a new METHOD and `pane.<id>.revision` a new READ address, so
///   this is an ADDITION and an older client loses nothing — the number moves for the rule this
///   wire has always applied to a new address, and because the two halves must ship together: a
///   daemon serving the slot but not the park would let a driver read a number it can only poll.
///
///   ⚠⚠⚠ **WHY IT IS WORTH A NUMBER RATHER THAN BEING WAVED THROUGH AS «NOTHING BROKE».** A driver
///   that finds the method absent falls back to the documented degradation — one whole SCREEN over
///   the wire per ten milliseconds — and that fallback is INVISIBLE: it is correct, it is slow, and
///   nothing says which one is running. The number is what lets a driver tell *this daemon cannot
///   be waited on* from *this daemon did not move*, which is exactly the discrimination
///   `sprag_plugin::Settling::Unknown` was given a third arm for.
///
///   ⚠⚠ **NO ANSWER WORD OR ARGUMENT MOVED ANYWHERE ELSE.** The park's answer shape is its own
///   (`{pane, revision}`), modelled on [`PANE_WAIT_OUTPUT_METHOD`]'s `{pane, find}`.
///
/// * **40 — A PANE SAYS WHO HAS WRITTEN INTO IT, AND THIS IS THE FIRST ADDITION WHOSE ABSENCE IS A
///   FALSE ANSWER.** Register item 653. `pane.<id>.hands` is a new READ address answering
///   `{person, program}` — the counts `sprag_terminal::Hands` has kept since a person's keystrokes
///   were first told apart from a program's.
///
///   ⚠⚠⚠⚠⚠ **EVERY EARLIER ADDITION LEFT THE NUMBER STANDING ON ONE ARGUMENT, AND THAT ARGUMENT
///   DOES NOT HOLD HERE.** The rule this wire has applied since version 5 is *an added address is
///   absent-not-wrong to an old reader*: a client that never asks is unaffected, and one that asks
///   an old daemon learns nothing it did not already know. That is true exactly when the CONSUMER
///   of the missing answer degrades. `sprag_plugin::Readiness::reached` asks this address **first,
///   ahead of the barrier and ahead of any consent**, and a `None` there is not *I could not look*
///   — it is *nobody has reached in*. So a driver on an older daemon does not lose a feature: it is
///   told, for every pane and for its whole run, that the pane is unattended.
///
///   ⚠⚠⚠⚠ **MEASURED BEFORE THE ADDRESS EXISTED**, in the shape that is now this address's gate: a
///   real daemon, a real pane, a person's write DECLARED as `{"hand": "person"}` and visibly on the
///   screen — and the out-of-process barrier's very next look answered `Yes`. The in-process reader
///   answers `Interrupted` to the same fact, so the two halves of one product disagreed about
///   whether somebody was at the keyboard, and only the half that types was wrong.
///
///   ⚠⚠⚠ **IT IS ALSO WHY THE HANDSHAKE IS THE RIGHT ENFORCEMENT AND A PER-READ FALLBACK IS NOT.**
///   `handshake` refuses a daemon whose number is not this one, so a driver that needs this fact
///   cannot start against a daemon that cannot supply it. A client that instead read the absence
///   and carried on would be choosing the wrong answer at every step, quietly — which is version
///   39's *"the fallback is INVISIBLE"* argument with the fallback no longer merely slow.
///
///   ⚠⚠ **NO ANSWER WORD, ARGUMENT OR FORM MOVED.** `Hand`'s two words (`person`, `program`) were
///   already published as the WRITE argument this address's keys are taken from; nothing that
///   already answers is touched.
///
/// * **41 — A STOP SAYS HOW FAR IT MAY REACH, AND THE OLD SILENCE MEANT THE WIDER OF TWO DIFFERENT
///   ACTS.** Register item 654. `stop_job` gains a `reach` argument carrying a
///   `sprag_terminal::Reach` word (`under_the_program`, `the_program_too`); absent still asks for
///   the wide one, which is what every caller of this verb has always meant.
///
///   ⚠⚠⚠⚠⚠ **THE ADDED-ARGUMENT RULE IS [`CLIENT_BUILD_PARAM`]'S, AND THIS IS ITS SHARPEST CASE.**
///   That rule is not *arguments are additive*: an unknown argument is SWALLOWED rather than
///   refused, so the number moves when something WAITS ON THE FACT. What waits here is whether the
///   pane survives. Under `Reach::UnderTheProgram` a stop that would KILL the pane's own program is
///   declined (`Unstopped::WouldEndThePane`); under the wide reach it is delivered, and
///   `sprag_terminal::stop`'s own measurement of that path is *it closed one, and the daemon exited
///   behind it*. A daemon predating this key therefore does not perform a degraded version of the
///   request — it performs a DIFFERENT one, and the pane it may take does not come back.
///
///   ⚠⚠⚠⚠ **AND IT IS ONLY REACHABLE BECAUSE THE STOP ITSELF NOW CROSSES.** Until this version
///   `sprag_host::remote_access::RemotePaneAccess` offered no `job_control` at all, so a run driven
///   from another process answered `Stopped::Unsupported` on every cancel and every passed
///   deadline. That word was HONEST — it is what a host with no job control must say — which is why
///   the absence survived where item 653's did not; what it was not is EQUAL, and the same
///   `orchestrate` request meaning two things depending on which process drove it is what
///   `RUN_DRIVER_PROCESS` forbids.
///
///   ⚠⚠ **THE ANSWER SHAPE DID NOT MOVE.** `{stop, pgid, job?}` is unchanged and is now READ by a
///   second party; the refusal a caller gets back is still `Unstopped`'s own sentence, which that
///   type now maps back to its own word (`Unstopped::from_sentence`) so a remote run publishes the
///   clause an in-process one would.
///
/// * **42 — A PANE SAYS WHAT ITS CHILD WROTE, AND THE ABSENCE PUBLISHED AN EMPTY REPLY AND A SPEND
///   OF ZERO.** Register item 656. `pane.<id>.raw_output` is a new READ address answering
///   `{bytes: "<base64>", truncated: bool}` — the capture `sprag_terminal::RawOutput` has held
///   since a structured reply first had to be parsed from something the grid had not touched.
///
///   ⚠⚠⚠⚠⚠ **VERSION 40'S TEST, AT THE ADDRESS THAT LOOKED SAFEST.** The rule is *an added address
///   is absent-not-wrong to an old reader*, and it holds exactly when the CONSUMER of the missing
///   answer degrades. This consumer's documentation says it degrades — *no raw capture, a truncated
///   buffer, or an unparsable envelope → the raw text and `Tokens(0)`* — and that sentence is true
///   of the last two and empty of the first: **there is no raw text to fall back to when there are
///   no bytes.** `sprag_plugin`'s dialogue decoder `unwrap_or_default()`s the capture, so a
///   `claude -p --output-format json` turn driven from another process published a reply of `""`,
///   a spend of `Cost::Tokens(0)` and no session to resume, while the same turn in-process
///   published the model's text, its real billed tokens and its session. One request, two answers,
///   nobody told — which is what `RUN_DRIVER_PROCESS` forbids.
///
///   ⚠⚠⚠⚠ **AND THE ZERO IS THE HALF THAT IS NOT MERELY LOST DATA.** `sprag_plugin::Guardrails`
///   ends a run when the accumulated cost REACHES `max_cost`, and a dialogue's unit is tokens. A
///   turn that reports zero every time never accumulates, so a run driven from another process
///   could not reach ANY ceiling it was given: the guardrail was not unreported, it could not fire.
///   Version 39's *"the fallback is INVISIBLE"* with the fallback no longer merely slow, and
///   version 40's *"confident and wrong"* with a budget behind it.
///
///   ⚠⚠ **NO ANSWER WORD, ARGUMENT OR FORM MOVED.** The address is new, its object is spelled in
///   one place (`sprag_host::wire::raw_output_json`) and read in one (`raw_output_of`), and
///   nothing that already answers is touched. The bytes ride base64 because a child's SOURCE
///   stream carries escape sequences and can be cut mid-UTF-8 at the capture cap — a JSON string
///   cannot hold them, and the lossy replacement that would make it fit is the corruption this
///   address exists to route around.
///
/// [`CLIENT_BUILD_PARAM`]: crate::CLIENT_BUILD_PARAM
pub const WIRE_PROTOCOL: u32 = 42;

/// WHICH BUILD THIS IMAGE IS — the identity [`WIRE_PROTOCOL`] above cannot carry, stamped in by
/// this crate's build script as the commit it was compiled from (or `unknown`).
///
/// # ⚠⚠⚠⚠⚠ Why it sits beside a number that answers a different question (register item 438)
///
/// The constant above is a SHAPE, and it moves only when a shape moves. A fix that changes what a
/// run DOES — a new transition, a guard, a different word on a walk — earns no bump by that pin's
/// own list, so both ends agree across it and neither can tell that one of them predates the fix.
/// A daemon outlives its clients by design, so that skew is the ordinary state after a rebuild
/// rather than an exotic one, and it is invisible from either end.
///
/// Measured 2026-08-18, which is what this exists for: a loop's entire walk was produced by a
/// daemon built before two commits that changed the very edges the walk was being read for, and it
/// was indistinguishable from a walk that carried them. The only probe that answered was `grep`
/// over `/proc/<pid>/exe`.
///
/// ⚠⚠ **It is deliberately NOT a second version check.** Nothing refuses a connection over it and
/// nothing should: a skew here is a fact a reader needs, not a shape neither end can parse. The
/// number above owns refusal; this owns provenance, and conflating them would make every rebuild a
/// forced restart.
///
/// ⚠ Every binary linking this crate is stamped with the SAME value — this is the identity of the
/// image, not of a role — so a client and a daemon built from one `cargo build` agree, and one
/// built later does not.
pub const BUILD: &str = env!("SPRAG_BUILD");

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

/// The [`CLIENT_HELLO_METHOD`] REPLY key carrying the daemon's own [`BUILD`].
///
/// # ⚠⚠⚠⚠ An ABSENT key here means "this daemon cannot say", NEVER "it matches"
///
/// That reading is the whole reason this needs no [`WIRE_PROTOCOL`] bump. An added ANSWER key is
/// absent-not-wrong to an old reader — the rule version 5 states beside the first argument bump —
/// so a daemon predating this answers as it always did and a new client learns nothing, which is
/// the honest outcome. **The moment a reader treats the absence as agreement, that stops being
/// true and the key earns a number**, exactly as version 10 spells out for `ended`: the difference
/// between *"this daemon cannot say"* and the cheapest answer is the whole fact.
pub const BUILD_FIELD: &str = "build";

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

/// The [`CLIENT_HELLO_METHOD`] REQUEST key carrying **WHICH BUILD THIS CLIENT IS** ([`BUILD`]) —
/// [`BUILD_FIELD`] turned around, so a daemon can say *that window is not my image*.
///
/// # ⚠⚠⚠⚠⚠ The window on a person's screen is the one companion nothing could date
///
/// This wire already carries the fact in the two directions where the daemon is one end: it TELLS
/// its own build in the hello reply ([`BUILD_FIELD`]), and a hook STATES its build when it reports
/// (`sprag_host::wire::AGENT_BUILD_KEY`). The third party is the display client, and it is the one
/// a person is actually looking at. Register item 463 is that hole: a `sprag-gui` is started by
/// hand from wherever somebody points, its daemon is whatever was promoted, and **the two are
/// routinely different builds with nothing anywhere able to say so** — this repository's own
/// promotion procedure copies the daemon into one directory and runs the GUI out of `target/debug`,
/// so the skew is the ORDINARY state here rather than an exotic one.
///
/// ⚠⚠⚠ **ABSENT MEANS *"THIS CLIENT DID NOT SAY"*, NEVER *"IT MATCHES"*** — [`BUILD_FIELD`]'s rule,
/// which this is the third direction of. Every client older than this key sends exactly that
/// silence, so a reader taking it for agreement would make the commonest case look like the safe
/// one — the inversion all three keys exist to end.
///
/// # ⚠⚠⚠⚠ Why an ADDED REQUEST KEY earns no [`WIRE_PROTOCOL`] bump here, where version 37's did
///
/// Version 37 bumped for `report_agent`'s `asked`, and its argument was measured rather than
/// argued: an unknown argument is SWALLOWED rather than refused, so a caller that names it to a
/// daemon predating it is answered `accepted` while the fact is dropped — **and something was
/// waiting on that fact**. A delivery decided whether a prompt had landed by reading it back.
///
/// Nothing waits on this one. A daemon that drops it holds no build for that client, which renders
/// as *did not say* — the honest answer, and the same one that daemon would give about a client
/// that never sent it. No caller branches on a promise, so nothing is silently converted into
/// agreement. **That licence is CONDITIONAL and is gated**: the moment a surface renders the
/// absence as a match, this key is making a claim old daemons cannot support and it earns the
/// number after all.
///
/// ⚠ Sent by [`HostConn::handshake`], which is the ONE seam every client of this wire passes
/// through — so no client can omit it, including the ones that do not exist yet. A fact every
/// connection must carry belongs where the connection is made, never at each call site.
pub const CLIENT_BUILD_PARAM: &str = "build";

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

/// The JSON-RPC method a client sends to BLOCK until a named pane has MOVED —
/// `params: { "pane": <id>, "since": <revision> }`, answering
/// `{ "pane": <id>, "revision": <revision> }` with the pane's revision as it stands the moment it
/// passes `since`.
///
/// ## ⚠⚠⚠⚠⚠ Why a driver could not use either of the other two
///
/// This is the address register item 631 was open for: a run driven from OUTSIDE the daemon had no
/// way to be TOLD a pane moved, so `sprag_plugin::run::park_until` fell to its documented
/// degradation and re-read the whole SCREEN over the wire every ten milliseconds. Measured on the
/// remote surface, a 600 ms settle cost **61** screen reads where the in-process path cost 3.
///
/// * [`PANE_WAIT_OUTPUT_METHOD`] answers *has this pane SAID something in particular*. A driver's
///   predicate is not a search: it renders the screen and runs a supervisor's detector over the
///   result, and no needle expresses that.
/// * `scene/waitFor` answers *has this SESSION moved*, which is every pane in it plus every mux
///   mutation. A driver watching one pane while a neighbour builds would wake on the neighbour, so
///   the cost of its wait would follow somebody else's output — the same objection
///   [`PANE_WAIT_OUTPUT_METHOD`]'s own documentation makes.
///
/// ## ⚠⚠ It answers a NUMBER, and that is the whole contract
///
/// Nothing here says what the pane now shows. A caller compares the answer with what it sent:
/// greater means *worth a look*, and the look is the caller's own. That is exactly
/// [`PaneChanges`](../../sprag_plugin/access/trait.PaneChanges.html)'s in-process contract, so the
/// remote surface and the local one answer one question rather than two that can drift.
///
/// ## Intercepted and parked, like the other two
///
/// Handled in the host's per-frame dispatch before the generic core, because it PARKS its reply. It
/// carries no deadline of its own, for [`EVENTS_WAIT_METHOD`]'s reason — and a caller that must
/// give up on a slice and come back to the SAME park has
/// [`begin`](HostConn::begin)/[`settle`](HostConn::settle) rather than a daemon-side clock. A
/// daemon-side bound would need a timer per park, which for a ten-millisecond slice is the polling
/// this method exists to remove, moved into the daemon.
pub const PANE_WAIT_REVISION_METHOD: &str = "pane/waitForRevision";

/// The [`PANE_WAIT_REVISION_METHOD`] answer key carrying the pane's revision.
///
/// Its own constant for [`NEEDLE_PARAM`]'s reason: a key both ends spell has one home. ⚠ It is
/// deliberately NOT shared with the `scene/revision` and `layout` answers that spell the same word
/// — those are the SESSION's scene version and a layout's own counter, and a reader that took one
/// for the other would be comparing two clocks.
pub const PANE_REVISION_FIELD: &str = "revision";

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
    /// The deadline [`set_read_deadline`](Self::set_read_deadline) last installed, remembered so
    /// that [`settle`](Self::settle) — which narrows it to its own bound for one read — can put
    /// back what the owner chose.
    ///
    /// ⚠ Remembered rather than read back off the socket because there is no getter for
    /// `SO_RCVTIMEO` in the standard library, and a `settle` that guessed `None` would silently
    /// remove a deadline its owner set on purpose. The field is the only place that knows.
    read_deadline: Option<Duration>,
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
    /// WHICH BUILD the daemon on the other end said it is, read from the [`CLIENT_HELLO_METHOD`]
    /// reply's [`BUILD_FIELD`] — `None` until [`handshake`](Self::handshake) has run, and `None`
    /// after it against a daemon that does not carry the key.
    ///
    /// ⚠⚠ **The two `None`s mean the same thing on purpose**: *this connection cannot say what the
    /// daemon is*. Neither is *"it matches"* — see [`BUILD_FIELD`], which is the sentence that keeps
    /// this key off [`WIRE_PROTOCOL`]'s ledger.
    daemon_build: Option<String>,
    /// WHERE this connection was dialled, when it was dialled by path — so a holder that meets a
    /// dead connection can ask whether the DAEMON died or only this connection did.
    ///
    /// # ⚠⚠⚠⚠⚠ The two are the same event on the wire, and telling them apart by errno is unsound
    ///
    /// A client whose SESSION is killed and a client whose DAEMON exits both meet a failed read.
    /// Which `io::ErrorKind` that read carries is decided by how the peer's end was torn down and by
    /// the platform's socket layer, not by which of the two happened — and the difference matters:
    /// one means *leave*, the other means *go to another session*. [`socket`](Self::socket) is what
    /// lets that be ASKED (re-dial: an answer means the daemon is there) instead of guessed.
    ///
    /// `None` for a connection wrapped around an already-open stream, which names no address
    /// anybody could dial again — an honest *cannot ask* rather than a *no*.
    socket: Option<PathBuf>,
}

/// A request [`HostConn::begin`] has SENT and [`HostConn::settle`] has not yet been given an answer
/// for — the handle a wait carries across the slices it is taken in.
///
/// It carries the request's LABEL as well as its id, so a failure met three slices later still
/// names the call it came from. That is [`HostConn::call`]'s own rule (*every failure names the
/// request it came from*) applied to the one shape where the failure and the request are separated
/// in time — and the shape that needs it most, because by then the method is nowhere on the stack.
///
/// ⚠ Not [`Copy`], and not because of the string: a caller holding two of these against one
/// connection has abandoned one of them, and having to move it is the smallest reminder there is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outstanding {
    /// The JSON-RPC id this request went out under.
    id: u64,
    /// What [`request_label`] called it — the sentence a later failure is prefixed with.
    ///
    /// ⚠ NOT published through an accessor. A first draft had one and nothing read it, which is
    /// this repository's own recorded shape (register item 492: a number authored and never read).
    /// The label exists to be SPENT by [`HostConn::settle`]'s error messages; a caller that wants to
    /// name the request it is waiting on already holds the method and params it passed to
    /// [`HostConn::begin`].
    label: String,
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
                Ok(stream) => {
                    return Self::from_stream(stream).map(|conn| Self {
                        socket: Some(path.to_path_buf()),
                        ..conn
                    });
                }
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
            read_deadline: None,
            pending: VecDeque::new(),
            daemon_build: None,
            socket: None,
        })
    }

    /// The socket this connection was dialled on, or [`None`] where it was wrapped around a stream
    /// somebody else opened. See the field's own doc for why a holder wants it.
    #[must_use]
    pub fn socket(&self) -> Option<&Path> {
        self.socket.as_deref()
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
        self.reader.get_ref().set_read_timeout(deadline)?;
        self.read_deadline = deadline;
        Ok(())
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
    /// ⚠⚠ **It also STATES which build this client is** ([`CLIENT_BUILD_PARAM`]), which is the
    /// SHAPE agreement's companion and not part of it: the number decides whether the two can
    /// speak, the build says whether the window a person is looking at is running the daemon's
    /// code. Sent from here rather than by each client for the reason the protocol param is merged
    /// at [`call`](Self::call) — a client that had to remember is a client that will not.
    ///
    /// # Errors
    ///
    /// The hello failing, or the daemon answering with a different [`WIRE_PROTOCOL`] — or with
    /// none, which means a daemon from before this handshake existed. Both are reported with both
    /// numbers and the remedy, because a mismatched pair cannot be made to work by retrying.
    pub fn handshake(&mut self, client_id: &str) -> io::Result<()> {
        let reply = self.call(
            CLIENT_HELLO_METHOD,
            // ⚠ THE BUILD RIDES HERE, at the one seam every client passes through
            // ([`CLIENT_BUILD_PARAM`]): a `sprag-gui` a person started by hand states which image
            // it is without its author remembering to, and so does a client written after this.
            serde_json::json!({ CLIENT_PARAM: client_id, CLIENT_BUILD_PARAM: BUILD }),
        )?;
        // ⚠ TAKEN BEFORE THE SHAPE IS JUDGED, deliberately: the reply that REFUSES is the one a
        // reader most wants attributed, and a mismatch is exactly the moment somebody asks which
        // daemon they are talking to. Storing it costs nothing on the happy path and is the only
        // chance to store it on the other one.
        self.daemon_build = reply
            .get(BUILD_FIELD)
            .and_then(Value::as_str)
            .map(str::to_owned);
        match reply.get(PROTOCOL_FIELD).and_then(Value::as_u64) {
            Some(daemon) if daemon == u64::from(WIRE_PROTOCOL) => Ok(()),
            Some(daemon) => Err(protocol_mismatch(&daemon.to_string())),
            None => Err(protocol_mismatch("none (a daemon older than this check)")),
        }
    }

    /// WHICH BUILD the daemon on the other end said it is, or `None` when this connection cannot
    /// say — see [`BUILD_FIELD`] for why those are one answer and not two.
    ///
    /// Answers `None` before [`handshake`](Self::handshake) has run, because a connection that has
    /// not asked has not been told.
    #[must_use]
    pub fn daemon_build(&self) -> Option<&str> {
        self.daemon_build.as_deref()
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

    /// **SEND A REQUEST AND DO NOT WAIT FOR IT** — the first half of a wait that can be given up on
    /// and resumed. Answered by [`settle`](Self::settle).
    ///
    /// # ⚠⚠⚠⚠⚠ Why the wire needed a third shape of connection
    ///
    /// [`set_read_deadline`](Self::set_read_deadline) names two: a REQUEST connection, whose daemon
    /// answers at once so a deadline is a safeguard, and a LONG-POLL connection, which parks
    /// indefinitely so a deadline would be a bug. Both are about a wait a caller intends to see
    /// through.
    ///
    /// A DRIVER's wait is neither. `sprag_plugin::run::park_until` parks in slices — ten
    /// milliseconds at a time — not because it doubts the daemon but because between slices it must
    /// ask the RUN whether it has been cancelled or has run out of time, which are facts no pane can
    /// announce. Expressed with the two shapes above, each slice is a deadline that expires, and a
    /// connection that trips its deadline is FINISHED: driving a pane over the wire would burn and
    /// redial a socket a hundred times a second, which is worse than the polling it replaces.
    ///
    /// So the request is sent ONCE and waited on in slices. Between them the daemon holds a parked
    /// reply and this end holds nothing but an id, so a slice that ends in silence costs a socket
    /// read timeout and NOTHING on the wire or in the daemon — which is exactly what the
    /// `sleep` it replaces cost.
    ///
    /// # ⚠⚠ One outstanding request, still
    ///
    /// This does not make the connection multiplexed. A [`HostConn`] answers ONE id at a time, and
    /// beginning a second request while one is outstanding leaves two replies to arrive in an order
    /// this end does not choose — [`settle`](Self::settle) drops a frame whose id it is not waiting
    /// for, so the abandoned one is LOST rather than misattributed, but it is lost. **A caller that
    /// parks must give this connection to that park.**
    ///
    /// # Errors
    ///
    /// [`CallError::Transport`] if the request cannot be written, or if this connection has already
    /// been retired by a mid-frame deadline.
    pub fn begin(&mut self, method: &str, params: Value) -> Result<Outstanding, CallError> {
        let label = request_label(method, &params);
        if self.timed_out {
            return Err(CallError::Transport(io::Error::new(
                ErrorKind::TimedOut,
                format!("{label}: connection abandoned after a read deadline expired"),
            )));
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": self.scoped(params),
        });
        write_request(&mut self.writer, &request_line(&request)).map_err(|error| {
            CallError::Transport(io::Error::new(error.kind(), format!("{label}: {error}")))
        })?;
        Ok(Outstanding { id, label })
    }

    /// **WAIT UP TO `within` FOR AN OUTSTANDING REPLY, AND KEEP THE CONNECTION EITHER WAY** — the
    /// second half of [`begin`](Self::begin).
    ///
    /// * `Ok(Some(result))` — it answered.
    /// * `Ok(None)` — the bound elapsed and nothing arrived. The request is STILL outstanding, this
    ///   connection is still usable, and asking again with the same [`Outstanding`] resumes the
    ///   same wait. Beginning a different request instead abandons this one.
    /// * `Err(..)` — the daemon faulted, or the transport failed. A transport failure here is
    ///   terminal for the connection exactly as it is for [`call`](Self::call).
    ///
    /// ⚠⚠⚠ **`Ok(None)` IS NOT A TIMEOUT IN [`set_read_deadline`](Self::set_read_deadline)'s
    /// SENSE.** That one retires the connection because a deadline may have taken half a frame out
    /// of the stream. This one is only ever returned when the read consumed NOTHING, which
    /// `read_frame_inner` is the only place that can know — a slice that ended mid-frame comes back
    /// as `Err` and retires the connection like any other.
    ///
    /// ⚠ A `within` of zero answers `Ok(None)` without reading, because zero means *block forever*
    /// to the socket layer and a caller asking for no wait never meant that.
    ///
    /// # Errors
    ///
    /// [`CallError::Fault`] for the daemon's own `error` object; [`CallError::Transport`] for a
    /// failed or malformed read, and for a connection already retired.
    pub fn settle(
        &mut self,
        outstanding: &Outstanding,
        within: Duration,
    ) -> Result<Option<Value>, CallError> {
        if self.timed_out {
            return Err(CallError::Transport(io::Error::new(
                ErrorKind::TimedOut,
                format!(
                    "{}: connection abandoned after a read deadline expired",
                    outstanding.label
                ),
            )));
        }
        if within.is_zero() {
            return Ok(None);
        }
        // ⚠ THE OWNER'S DEADLINE IS PUT BACK WHATEVER HAPPENS. This narrows `SO_RCVTIMEO` to its
        // own slice, and a connection left carrying a ten-millisecond deadline would trip the next
        // ordinary call that was entitled to wait — a defect that would appear far from here.
        let restore = self.read_deadline;
        let outcome = self.settle_inner(outstanding, within);
        let restored = self.set_read_deadline(restore);
        match (outcome, restored) {
            // The wait's own failure outranks a failure to restore: they have the same cause on a
            // socket that has died, and the first one names the request.
            (Err(error), _) => Err(error),
            (Ok(answer), Ok(())) => Ok(answer),
            (Ok(_), Err(error)) => Err(CallError::Transport(io::Error::new(
                error.kind(),
                format!("{}: {error}", outstanding.label),
            ))),
        }
    }

    /// [`settle`](Self::settle)'s body, wrapped by it so the owner's deadline is restored on every
    /// exit — including the `?` this body uses.
    fn settle_inner(
        &mut self,
        outstanding: &Outstanding,
        within: Duration,
    ) -> Result<Option<Value>, CallError> {
        let started = Instant::now();
        loop {
            // Recomputed per frame rather than set once, because a NOTIFICATION arriving mid-slice
            // must not restart the bound: a subscription's traffic would otherwise keep a slice
            // alive past the moment the run wanted to be asked about itself.
            let left = within.checked_sub(started.elapsed()).unwrap_or_default();
            if left.is_zero() {
                return Ok(None);
            }
            self.set_read_deadline(Some(left)).map_err(|error| {
                CallError::Transport(io::Error::new(
                    error.kind(),
                    format!("{}: {error}", outstanding.label),
                ))
            })?;
            match self.read_frame_inner() {
                Err(ReadStop::Idle) => return Ok(None),
                Err(ReadStop::Failed(error)) => {
                    return Err(CallError::Transport(io::Error::new(
                        error.kind(),
                        format!("{}: {error}", outstanding.label),
                    )));
                }
                Ok(frame) => {
                    // Set aside, never dropped — `call_inner`'s rule, and for its reason: a batch
                    // exists nowhere else once the daemon has advanced past it.
                    if frame.get("id").is_none() && frame.get("method").is_some() {
                        self.pending.push_back(frame);
                        continue;
                    }
                    if frame.get("id").and_then(Value::as_u64) != Some(outstanding.id) {
                        continue;
                    }
                    if let Some(error) = frame.get("error") {
                        return Err(CallError::Fault(RpcFault::from_wire(error)));
                    }
                    return Ok(Some(frame.get("result").cloned().unwrap_or(Value::Null)));
                }
            }
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
    ///
    /// ⚠ A deadline that elapses having consumed NOTHING is retired here too, and that is this
    /// caller's decision rather than a fact about the socket — see [`ReadStop::Idle`], which is
    /// where the two are told apart, and [`settle`](Self::settle), which is the caller that does
    /// not retire.
    fn read_frame(&mut self) -> io::Result<Value> {
        self.read_frame_inner().map_err(|stop| match stop {
            // SAID, not passed on. What the OS hands back for an elapsed `SO_RCVTIMEO` is
            // `Resource temporarily unavailable`, which describes a socket rather than the
            // situation: the host is THERE — it accepted this connection — and it is not
            // answering. An operator reading the first spelling goes looking for a socket that is
            // missing; one reading the second restarts the daemon.
            ReadStop::Idle => {
                self.timed_out = true;
                io::Error::new(ErrorKind::TimedOut, HOST_SILENT)
            }
            ReadStop::Failed(error) => error,
        })
    }

    /// One non-blank line, parsed — with *the deadline elapsed and nothing consumed* kept APART
    /// from every other failure.
    ///
    /// # ⚠⚠⚠⚠⚠ The two timeouts are different facts, and only this function can tell them apart
    ///
    /// `read_line` appends what it managed to read before it failed. So a deadline that fires with
    /// `line` still EMPTY consumed nothing: the next byte on the wire is still the first byte of a
    /// frame, and the connection is exactly as usable as it was. A deadline that fires with bytes
    /// already in `line` has taken half a frame out of the stream and cannot put it back — that
    /// connection is finished, and [`set_read_deadline`](Self::set_read_deadline) says so.
    ///
    /// Everything above this call used to see one word for both, which is why a bounded wait had to
    /// burn a connection per bound. ⚠ The mid-line half is NOT softened: it still ends the
    /// connection, and it is [`ReadStop::Failed`] carrying [`ErrorKind::TimedOut`] so a caller
    /// cannot mistake it for the idle one.
    fn read_frame_inner(&mut self) -> Result<Value, ReadStop> {
        let mut line = String::new();
        loop {
            line.clear();
            let read = match self.reader.read_line(&mut line) {
                Ok(read) => read,
                // Both spellings the platforms use for "the timeout elapsed" mean the same thing
                // here — see `set_read_deadline`.
                Err(error)
                    if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
                {
                    if line.is_empty() {
                        return Err(ReadStop::Idle);
                    }
                    // ⚠ HALF A FRAME IS OUT OF THE STREAM. Retired here as well as by the caller,
                    // because a caller that chose not to retire on `Idle` must not be able to
                    // choose it here by forgetting.
                    self.timed_out = true;
                    return Err(ReadStop::Failed(io::Error::new(
                        ErrorKind::TimedOut,
                        HOST_SILENT,
                    )));
                }
                Err(error) => return Err(ReadStop::Failed(error)),
            };
            if read == 0 {
                return Err(ReadStop::Failed(io::Error::new(
                    ErrorKind::UnexpectedEof,
                    "host closed the connection",
                )));
            }
            if !line.trim().is_empty() {
                return serde_json::from_str(line.trim()).map_err(|error| {
                    ReadStop::Failed(io::Error::new(ErrorKind::InvalidData, error))
                });
            }
        }
    }
}

/// Why [`HostConn::read_frame_inner`] came back without a frame.
///
/// Two variants and not an [`io::Error`] with a kind, because the discrimination is not in the
/// kind: [`Idle`](Self::Idle) and the mid-line timeout are BOTH [`ErrorKind::TimedOut`] to the
/// operating system, and only the reader knows whether anything was consumed.
enum ReadStop {
    /// The read deadline elapsed having consumed NOTHING. The stream is at a frame boundary and the
    /// connection is still usable; whether that is an answer or a failure belongs to the caller.
    Idle,
    /// Anything else — including a deadline that fired mid-frame, which has already retired the
    /// connection.
    Failed(io::Error),
}

/// The JSON-RPC `Invalid params` code — the one both ends of this wire already spell, now spelled
/// once.
///
/// It is the code sprag's daemon answers a request whose SCOPE it cannot honour with, and the one
/// a scoped pre-flight reads back off [`RpcFault::code`]. Defined here, in the transport both ends
/// share, for the reason [`SESSION_PARAM`] is: a number the writer and the reader must agree on
/// has one home.
pub const INVALID_PARAMS: i64 = -32602;

/// What a client says when a host ACCEPTED its connection and then answered nothing within the
/// deadline the caller set ([`HostConn::set_read_deadline`]).
///
/// Its own sentence because it is its own diagnosis, and the two it sits between are acted on
/// differently: *"no server running"* sends a person to start one, a daemon's own refusal sends
/// them to their argument, and this one — the host is there, it took the connection, it is not
/// talking — sends them to the daemon's log. Left as the OS's `Resource temporarily unavailable`
/// it reads like the first of those and is the third.
///
/// A constant rather than a literal for [`UNKNOWN_SLOT_FAULT`]'s reason: a test asserting the
/// sentence must assert THE sentence, and a copy of it in a test file is a second authority that
/// goes on passing after this one is reworded.
pub const HOST_SILENT: &str = "the host accepted this connection and did not answer in time";

/// The `data` a daemon attaches when it does not have the ADDRESS a read named.
///
/// **pinion's own word, matched and never invented.** It is the whole discriminator between *"this
/// daemon is older than this build"* and *"your argument is wrong"*, and both of those sentences
/// are rendered from it — see `sprag_host::wire::unknown_slot`.
///
/// It lives here, in the transport both ends share, for [`INVALID_PARAMS`]'s reason and one more:
/// a TEST STAND-IN has to answer exactly what a daemon answers, and four of them had spelled it out
/// by hand (R324). A string a reader matches on and a stand-in produces has one home.
pub const UNKNOWN_SLOT_FAULT: &str = "UnknownIntrospectPath";

/// The `data` a daemon attaches when it does not have the ACTION a write named.
///
/// [`UNKNOWN_SLOT_FAULT`]'s twin on the acting side, and the one that distinguishes an older
/// daemon from a refusal: an action a daemon HAS and declines answers under
/// [`ACTION_REFUSED`].
pub const UNKNOWN_ACTION_FAULT: &str = "UnknownInvokePath";

/// The `data` a daemon attaches when the path reaches NO EXTERNAL AT ALL.
///
/// **pinion's own word, matched and never invented**, exactly like its two neighbours — and the
/// third member of a discrimination that reads as one fault to anybody who does not know all three.
/// [`UNKNOWN_ACTION_FAULT`] says *the surface is there and has no such verb* (an older daemon);
/// this says *there is no surface at that path at all*. For a PANE-addressed call the difference is
/// the whole answer: the pane is gone, which is a fact about the workspace, where the other is a
/// fact about the build and its remedy is a restart.
///
/// ⚠ Found by a gate rather than by reading: a driver's injection into a pane nobody knows was
/// mapped to *this daemon does not perform that action* — telling an operator to restart a daemon
/// that was perfectly current (register item 544, stage 1c).
pub const NO_EXTERNAL_FAULT: &str = "NoExternalAtPath";

/// The `data` a daemon OLDER than PINION-PR82 attaches to a refusal it cannot explain.
///
/// pinion's own word again, matched and never invented: before R1564 every refused action published
/// this string under [`INVALID_PARAMS`], because `InvokeError::Rejected` was a payload-free variant.
/// A consumer needs it for two reasons — to keep a Rust variant name off an operator's screen
/// (register item 9's original leak), and so a TEST STAND-IN can produce the shape a real old daemon
/// produces.
pub const UNSTATED_REFUSAL_FAULT: &str = "InvokeRejected";

/// The code a peer answers when an action it HAS declined to fire, with the producer's own
/// sentence in `data` — pinion's `ACTION_REFUSED`, re-exported here so a sprag client reads one
/// vocabulary.
///
/// # Why this is not `-32602`, and why that matters to sprag specifically
///
/// [`INVALID_PARAMS`] means the parameters were wrong. A refused action's parameters were RIGHT:
/// the path resolved, the argument type matched, and the daemon then declined on a fact about its
/// own state. The split is what makes the `data` on a refusal safely readable — every other `data`
/// string on this wire is a word pinion authored (matched by [`UNKNOWN_SLOT_FAULT`] and its twin),
/// and a refusal's is arbitrary application prose that a consumer must never branch on.
///
/// It also removes a collision this crate's caller was living with by accident: sprag reads
/// `-32602` off a `scene/query` as *"no such session"*, which was safe only because a READ cannot
/// carry an action refusal. Use [`RpcFault::refusal`] rather than comparing the number.
pub const ACTION_REFUSED: i64 = pinion_rpc::ACTION_REFUSED as i64;

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

    /// The PRODUCER'S OWN sentence, when this fault is an action the daemon had and declined —
    /// [`None`] for every other refusal on this wire.
    ///
    /// The CODE is the discriminator, never the text. Every other `data` on this wire is a word
    /// pinion authored and a consumer may match; this one is arbitrary application prose, and a
    /// caller that decided *"it does not look like `UnknownInvokePath`, so it must be a reason"*
    /// would be one wording change away from printing a transport word at a person. That is the
    /// whole argument PINION-PR82 spent a second error code on.
    ///
    /// [`None`] also covers a refusal that arrived with no `data` at all — a daemon older than the
    /// build that made a reason mandatory. A caller renders its own words for that, which is
    /// exactly what it did for every refusal before this existed.
    #[must_use]
    pub fn refusal(&self) -> Option<&str> {
        if self.code != ACTION_REFUSED {
            return None;
        }
        self.data
            .as_ref()
            .and_then(Value::as_str)
            .filter(|reason| !reason.is_empty())
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

    use std::sync::atomic::{AtomicBool, Ordering};

    /// What a [`HostConn::settle`] answered, in the one shape a test can compare — [`CallError`] is
    /// deliberately not [`PartialEq`] (a fault carries a daemon's own words, and comparing two by
    /// value would invite matching on a rendering), so the discrimination a gate here needs is
    /// spelled out rather than derived.
    #[derive(Debug, PartialEq, Eq)]
    enum Settled {
        /// The bound elapsed with the request still outstanding.
        Nothing,
        /// It answered.
        Answered(Value),
        /// The daemon faulted, or the transport did.
        Failed,
    }

    fn settled(outcome: Result<Option<Value>, CallError>) -> Settled {
        match outcome {
            Ok(None) => Settled::Nothing,
            Ok(Some(value)) => Settled::Answered(value),
            Err(_) => Settled::Failed,
        }
    }

    /// A connection whose peer is a socket THIS TEST holds, so a parked reply can be answered — or
    /// deliberately not answered — at an instant the test chooses.
    ///
    /// A real daemon cannot stage the case these gates are about (a slice that ends in silence),
    /// because it answers as fast as it can. The pair is what makes *nothing happened* a thing a
    /// test can produce on purpose.
    fn paired() -> (HostConn, UnixStream) {
        let (mine, theirs) = UnixStream::pair().expect("a socket pair");
        (
            HostConn::from_stream(mine).expect("wrap the client end"),
            theirs,
        )
    }

    /// Write one JSON-RPC response line for `id` into the peer end.
    fn answer(peer: &mut UnixStream, id: u64, result: Value) {
        let line = json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string();
        peer.write_all(format!("{line}\n").as_bytes())
            .expect("answer the parked request");
        peer.flush().expect("flush the answer");
    }

    /// **THE PROPERTY THE WHOLE PRIMITIVE EXISTS FOR**: a slice that ends in silence leaves the
    /// request outstanding and the connection usable, and the answer arrives on a LATER slice
    /// without the request being sent again.
    ///
    /// ⚠⚠⚠ The last clause is the one that makes this a PARK rather than a poll, and it is asserted
    /// against the BYTES the peer received rather than against a count this end keeps: a
    /// re-send would be invisible to any assertion made on the client's own side.
    #[test]
    fn a_slice_that_ends_in_silence_keeps_both_the_wait_and_the_connection() {
        let (mut conn, mut peer) = paired();
        let outstanding = conn
            .begin("pane/waitForRevision", json!({"pane": 3, "since": 7}))
            .expect("send the park");

        for slice in 0..3 {
            assert_eq!(
                settled(conn.settle(&outstanding, Duration::from_millis(20))),
                Settled::Nothing,
                "slice {slice} must answer 'nothing yet' rather than retiring the connection",
            );
        }

        answer(&mut peer, outstanding.id, json!({"pane": 3, "revision": 9}));
        assert_eq!(
            settled(conn.settle(&outstanding, Duration::from_secs(5))),
            Settled::Answered(json!({"pane": 3, "revision": 9})),
            "the SAME outstanding request resumes and is answered",
        );

        // ⚠ ONE request line reached the daemon, for four slices of waiting. A poll wearing this
        // API's clothes would have sent four.
        peer.set_read_timeout(Some(Duration::from_millis(200)))
            .expect("bound the peer read");
        let mut sent = String::new();
        let mut reader = BufReader::new(peer);
        let _ = reader.read_line(&mut sent);
        let mut second = String::new();
        let read_again = reader.read_line(&mut second).unwrap_or(0);
        assert!(
            sent.contains("pane/waitForRevision"),
            "the peer received the request: {sent:?}",
        );
        assert_eq!(
            read_again, 0,
            "the request was sent ONCE across four slices; a second line means this polls: {second:?}",
        );
    }

    /// **AND THE HALF THAT MUST NOT BE SOFTENED**: a deadline that fires with half a frame already
    /// out of the stream still retires the connection.
    ///
    /// ⚠⚠⚠⚠ This is the gate that keeps [`HostConn::settle`] honest. The whole primitive rests on
    /// *the read consumed nothing*, and a repair that answered `Ok(None)` for every elapsed
    /// deadline would pass the gate above and silently desynchronise a connection here — one call's
    /// result attributed to another, which is the failure
    /// [`HostConn::set_read_deadline`] retires a connection to prevent.
    #[test]
    fn a_slice_that_ends_mid_frame_still_retires_the_connection() {
        let (mut conn, mut peer) = paired();
        let outstanding = conn
            .begin("pane/waitForRevision", json!({"pane": 3, "since": 7}))
            .expect("send the park");

        // Half a response line, deliberately without its newline: the reader consumes these bytes
        // and then meets the deadline with no way to put them back.
        peer.write_all(br#"{"jsonrpc": "2.0", "id": 1, "res"#)
            .expect("write a partial frame");
        peer.flush().expect("flush the partial frame");

        let stopped = conn.settle(&outstanding, Duration::from_millis(50));
        let Err(CallError::Transport(error)) = stopped else {
            panic!("a mid-frame deadline is a transport failure, not 'nothing yet': {stopped:?}");
        };
        assert_eq!(error.kind(), ErrorKind::TimedOut, "{error}");

        assert!(
            matches!(
                conn.settle(&outstanding, Duration::from_millis(50)),
                Err(CallError::Transport(_)),
            ),
            "the connection stays retired",
        );
        assert!(
            conn.call("scene/query", json!({"path": "/x"})).is_err(),
            "and so does every ordinary call on it",
        );
    }

    /// A bounded slice puts back the deadline its OWNER set — including when the owner set none.
    ///
    /// ⚠ The second case is the CONTROL, and it is the one a naive restore gets wrong: a `settle`
    /// that only restored a deadline it found would leave a long-poll connection carrying this
    /// slice's ten milliseconds, and the next park on it would trip immediately.
    #[test]
    fn a_bounded_slice_puts_back_the_deadline_its_owner_set() {
        for owners in [None, Some(Duration::from_secs(30))] {
            let (mut conn, _peer) = paired();
            conn.set_read_deadline(owners).expect("set the owner's own");
            let outstanding = conn
                .begin("scene/query", json!({"path": "/x"}))
                .expect("send");
            assert_eq!(
                settled(conn.settle(&outstanding, Duration::from_millis(20))),
                Settled::Nothing,
            );
            assert_eq!(
                conn.reader.get_ref().read_timeout().expect("read it back"),
                owners,
                "the slice's own bound must not outlive the slice",
            );
        }
    }

    /// A NOTIFICATION arriving mid-slice is set aside rather than lost, and it does not restart the
    /// slice's bound.
    ///
    /// ⚠⚠⚠ The second half is what a driver's cancellation rests on. `park_until` slices at ten
    /// milliseconds precisely so a run hears a stop; a slice whose bound restarted on every frame
    /// would be deaf for as long as a subscription on the same connection stayed busy — and a
    /// subscription IS busy exactly when a pane is producing, which is when a driver most wants to
    /// be asked.
    ///
    /// Asserted against a peer that keeps talking, with a ceiling **fifty times** the bound rather
    /// than a tight one: the discrimination is between *comes back while the flood is running* and
    /// *comes back when it stops*, which does not need a sharp clock. (Register item 613 is what
    /// tight wall-clock assertions cost on a shared runner.)
    #[test]
    fn a_notification_mid_slice_is_kept_and_does_not_restart_the_bound() {
        let (mut conn, mut peer) = paired();
        let outstanding = conn
            .begin("scene/query", json!({"path": "/x"}))
            .expect("send");

        let flooding = Arc::new(AtomicBool::new(true));
        let stop = Arc::clone(&flooding);
        let flood = std::thread::spawn(move || {
            let line = json!({"jsonrpc": "2.0", "method": "events/changed", "params": {"n": 1}})
                .to_string();
            // ⚠ THE FLOOD ENDS ON ITS OWN as well as on the flag, and that is what makes the
            // failure a RED rather than a HANG: under a `settle` whose bound restarts per frame,
            // the call comes back only when the peer goes quiet, and the assertion below then has
            // a number to fail on. A flood that ran until the call returned would deadlock exactly
            // the build the gate is aimed at.
            let until = Instant::now() + Duration::from_secs(2);
            while stop.load(Ordering::Acquire) && Instant::now() < until {
                if peer.write_all(format!("{line}\n").as_bytes()).is_err() {
                    break;
                }
                sleep(Duration::from_millis(1));
            }
            peer
        });

        let started = Instant::now();
        let answered = conn.settle(&outstanding, Duration::from_millis(20));
        let took = started.elapsed();
        flooding.store(false, Ordering::Release);
        let _peer = flood.join().expect("the flood thread");

        assert_eq!(
            settled(answered),
            Settled::Nothing,
            "the slice ends with no answer to its own request",
        );
        assert!(
            took < Duration::from_secs(1),
            "the bound is the SLICE's, not one restarted per frame — took {took:?}",
        );
        assert_eq!(
            conn.next_notification("events/changed")
                .expect("the notification was kept")["n"],
            json!(1),
            "a frame read while waiting is set aside, never dropped",
        );
    }

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
        assert_eq!(
            error.kind(),
            ErrorKind::TimedOut,
            "the failure must say it timed out, not something a caller would retry: {error:?}",
        );
        // AND IT MUST SAY SO IN WORDS. The kind alone reaches an operator as whatever the OS put in
        // the message, which for an elapsed `SO_RCVTIMEO` is `Resource temporarily unavailable` —
        // a sentence about a socket, for a situation about a daemon that is right there and silent.
        assert!(
            error.to_string().contains(HOST_SILENT),
            "and it must say WHICH silence this is: {error}",
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
