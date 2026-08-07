//! A daemon OLDER than this build — the stand-in every front's version-skew test needs.
//!
//! # Why this is a crate and not four copies
//!
//! Four test files had each written one out by hand — `tests/cli.rs` (twice), `mcp_stdio.rs` and
//! `pty_round_trip.rs` — and, measured at `d651f50`, they had **drifted into three different
//! policies**: two refused reads only, one refused reads and actions, one proxied a live daemon and
//! refused actions. All four called themselves *"an older daemon"*. A stand-in whose meaning depends
//! on which file you opened is one that cannot be reasoned about, and the four had already stopped
//! agreeing about what the phrase means.
//!
//! So the policy is DATA here ([`Missing`]) and the transport is written once. A front picks the
//! shape it needs, and *"older daemon"* has one definition in the workspace.
//!
//! # What an older daemon IS, on this wire
//!
//! **It passes the handshake at THIS build's protocol number**, deliberately: a slot is additive and
//! so is an action, so `WIRE_PROTOCOL` does not rise when one is added (`wire::PINNED_SURFACE`
//! asserts that). *"The numbers agree and the address is missing"* is therefore the reachable skew,
//! and the one every sentence under test is written for. A peer that failed the handshake would be
//! testing the door instead.
//!
//! What it lacks is spelled in the daemon's own words —
//! [`sprag_rpc::UNKNOWN_SLOT_FAULT`] and [`sprag_rpc::UNKNOWN_ACTION_FAULT`], the strings the code
//! under test MATCHES on. A stand-in that invented its own would be exercising a grammar nobody
//! speaks.
//!
//! # Two shapes, and why both are needed
//!
//! * [`OldDaemon::serving_nothing`] answers every read with *"I do not have that address"*. That is
//!   the skew a slot READER meets, and it is all a boot-path test needs.
//! * [`OldDaemon::proxying`] puts a REAL daemon behind it and intercepts one method. A write verb
//!   reaches its `scene/invoke` only after its pre-flight READS succeed, so against a peer that
//!   serves nothing every acting verb stops at a read and the acting path is never exercised at all
//!   (R322 measured 21 of 24 acting CLI verbs blaming the user, and only a proxy could reach them).
//!   Every read is the daemon's own answer — real pane ids, real window names — and only the verb is
//!   missing. It has a second property a sweep depends on: **nothing it is asked to do happens**, so
//!   one fixture survives `kill-window`, `kill-session` and `kill-server`.
//!
//! # It is DEV-ONLY
//!
//! Nothing sprag ships depends on this crate; it appears in `[dev-dependencies]` and nowhere else.
//! The dependency cycle that creates (a crate's dev-dependency depending on that crate) is the one
//! Cargo allows, and it is what keeps a test double out of a shipped binary.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// How long [`OldDaemon::proxying`] waits for the daemon behind it to bind its socket.
///
/// A bounded wait rather than a sleep: the daemon binds a moment after it is spawned, and the first
/// client through the proxy may arrive before it has.
const UPSTREAM_DEADLINE: Duration = Duration::from_secs(5);

/// How long the accept loop parks between polls of a non-blocking listener.
///
/// Short enough that a test's first connection is not measurably delayed, long enough that an idle
/// peer is not a busy loop.
const ACCEPT_POLL: Duration = Duration::from_millis(5);

/// WHAT this older daemon does not have.
///
/// The policy as data, which is the whole reason this crate exists: four hand-written peers had
/// three different answers to *"what is missing?"* and no way to say so.
/// **Not `Copy`, since the addresses are owned**: a caller names them through the same builder the
/// client addresses them with (`mux_action_path(WINDOWS_SLOT)`), which is a `String` — and a test
/// that spelled the path by hand would go on passing after the wire moved it.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Missing {
    /// It does not serve the ADDRESS a `scene/query` names — every read is refused with
    /// [`sprag_rpc::UNKNOWN_SLOT_FAULT`].
    pub slots: bool,
    /// It does not have the ACTION a `scene/invoke` names — every write is refused with
    /// [`sprag_rpc::UNKNOWN_ACTION_FAULT`].
    pub actions: bool,
    /// It HAS the action a `scene/invoke` names, refuses it, and CANNOT SAY WHY — every write is
    /// refused with the payload-free [`sprag_rpc::UNSTATED_REFUSAL_FAULT`].
    ///
    /// The one shape here that is not an absence, and it belongs with the others anyway: this is
    /// precisely what every daemon built before PINION-PR82 (pinion R1564) did, because
    /// `InvokeError::Rejected` was a unit variant and a producer that knew exactly why it was
    /// refusing had nowhere to put the sentence. So *"older"* has a third meaning on the acting
    /// side — not *"cannot do that"* but *"will not, and cannot tell you"* — and a consumer that
    /// treats the two alike sends a person to restart a daemon whose answer was about their
    /// argument. R325 measured that hole open on this crate's own front: deleting ten client-side
    /// guesses left a Rust variant name reaching an operator until the sentence for it existed.
    pub refuses_without_reason: bool,
    /// It is missing exactly THESE addresses, whatever the two flags above say.
    ///
    /// The sharpest shape, and the most realistic: a daemon one build behind lacks the addresses
    /// that build added and serves every other one. A client boots against it and fails one read —
    /// which is the state a blanket refusal cannot express, because a client that cannot read its
    /// panes never starts.
    pub paths: Vec<String>,
}

impl Missing {
    /// A daemon that serves no address and knows no verb — the widest skew.
    #[must_use]
    pub fn everything() -> Self {
        Self {
            slots: true,
            actions: true,
            refuses_without_reason: false,
            paths: Vec::new(),
        }
    }

    /// A daemon whose ADDRESSES this build does not find — the skew a reader meets.
    #[must_use]
    pub fn slots() -> Self {
        Self {
            slots: true,
            actions: false,
            refuses_without_reason: false,
            paths: Vec::new(),
        }
    }

    /// A daemon that serves every read and knows no VERB — the skew only a proxy can express.
    #[must_use]
    pub fn actions() -> Self {
        Self {
            slots: false,
            actions: true,
            refuses_without_reason: false,
            paths: Vec::new(),
        }
    }

    /// A daemon that HAS every verb, refuses the one it is asked for, and cannot say why — the
    /// acting-side skew PINION-PR82 removed.
    ///
    /// See [`refuses_without_reason`](Missing::refuses_without_reason) for why this is *"older"*
    /// rather than *"broken"*.
    #[must_use]
    pub fn refusing_without_reason() -> Self {
        Self {
            slots: false,
            actions: false,
            refuses_without_reason: true,
            paths: Vec::new(),
        }
    }

    /// A daemon serving every address EXCEPT these — the one build behind, which is what a running
    /// client actually meets.
    #[must_use]
    pub fn addresses(paths: &[String]) -> Self {
        Self {
            slots: false,
            actions: false,
            refuses_without_reason: false,
            paths: paths.to_vec(),
        }
    }

    /// The reply this peer gives one request, or [`None`] to pass it on / answer it normally.
    ///
    /// It reads the PATH as well as the method, because [`paths`](Self::paths) is what an older
    /// daemon really looks like: it serves what it has and refuses what it does not.
    fn refusal(&self, request: &Value, id: &Value) -> Option<Value> {
        let method = request["method"].as_str();
        let path = request["params"]["path"].as_str().unwrap_or_default();
        let named = self.paths.iter().any(|missing| missing == path);
        let data = match method {
            Some("scene/query") if self.slots || named => sprag_rpc::UNKNOWN_SLOT_FAULT,
            Some("scene/invoke") if self.actions || named => sprag_rpc::UNKNOWN_ACTION_FAULT,
            // AFTER the missing-action arm, so a peer configured as both answers the sharper
            // fact: not having a verb is a different situation from having it and declining.
            Some("scene/invoke") if self.refuses_without_reason => {
                sprag_rpc::UNSTATED_REFUSAL_FAULT
            }
            _ => return None,
        };
        Some(json!({
            "jsonrpc": "2.0",
            "id": id.clone(),
            "error": {
                "code": sprag_rpc::INVALID_PARAMS,
                "message": "Invalid params",
                "data": data,
            },
        }))
    }
}

/// A peer on a Unix socket that answers like a daemon older than this build, until it is dropped.
///
/// Bind it, point a client at [`sock`](Self::sock), and drive whatever that client does.
pub struct OldDaemon {
    sock: PathBuf,
    stop: Arc<AtomicBool>,
    accepting: Option<JoinHandle<()>>,
}

impl Drop for OldDaemon {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(accepting) = self.accepting.take() {
            let _ = accepting.join();
        }
        let _ = std::fs::remove_file(&self.sock);
    }
}

impl OldDaemon {
    /// A peer that passes the handshake and serves NO address — and answers everything else with a
    /// null result, which is what a client's own writes then see.
    ///
    /// `sock` is where it binds; the caller owns the path (tests give each their own, so a run's
    /// peers cannot find each other).
    ///
    /// # Panics
    ///
    /// If the socket cannot be bound — a test cannot proceed without its peer, and a silent
    /// stand-in would make every assertion after it meaningless.
    #[must_use]
    pub fn serving_nothing(sock: &Path) -> Self {
        Self::bind(sock, Missing::slots(), None)
    }

    /// A peer that serves NO address and knows NO action — the widest skew, and what an agent
    /// surface meets when its whole tool set predates nothing.
    ///
    /// # Panics
    ///
    /// If the socket cannot be bound, for [`serving_nothing`](Self::serving_nothing)'s reason.
    #[must_use]
    pub fn serving_nothing_or_acting(sock: &Path) -> Self {
        Self::bind(sock, Missing::everything(), None)
    }

    /// A peer that relays every frame to the daemon at `upstream` and refuses only the methods
    /// `missing` names.
    ///
    /// Every answer a test reads is the real daemon's, so the state under it — pane ids, window
    /// names, the session list — cannot drift from the product the way a table of canned replies
    /// would. See the module docs for why this shape is the only one that can reach an acting path.
    ///
    /// # Panics
    ///
    /// If the socket cannot be bound, for [`serving_nothing`](Self::serving_nothing)'s reason.
    #[must_use]
    pub fn proxying(sock: &Path, upstream: &Path, missing: Missing) -> Self {
        Self::bind(sock, missing, Some(upstream.to_path_buf()))
    }

    /// Where this peer is listening — what a client is pointed at.
    #[must_use]
    pub fn sock(&self) -> &Path {
        &self.sock
    }

    fn bind(sock: &Path, missing: Missing, upstream: Option<PathBuf>) -> Self {
        let _ = std::fs::remove_file(sock);
        let listener = UnixListener::bind(sock).expect("bind the older daemon's socket");
        listener
            .set_nonblocking(true)
            .expect("a stoppable accept loop");
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let accepting = std::thread::spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                match listener.accept() {
                    // ONE THREAD PER CONNECTION: several verbs open a second connection while the
                    // first is still open, and serving them in turn would deadlock the run.
                    Ok((stream, _)) => {
                        let upstream = upstream.clone();
                        // CLONED PER CONNECTION, because the policy owns its addresses now — and a
                        // connection is where a policy is applied, so each thread holds its own.
                        let missing = missing.clone();
                        std::thread::spawn(move || match upstream {
                            Some(upstream) => relay(&stream, &upstream, &missing),
                            None => answer(&stream, &missing),
                        });
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(ACCEPT_POLL);
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            sock: sock.to_path_buf(),
            stop,
            accepting: Some(accepting),
        }
    }
}

/// One connection to a peer with nothing behind it: the handshake is answered, the missing methods
/// are refused, and everything else is a null result.
///
/// A null result rather than an error for the rest, because that is what an older daemon does with a
/// method it HAS: it answers. The refusals are the only thing this peer is saying.
fn answer(stream: &UnixStream, missing: &Missing) {
    let reader = BufReader::new(stream.try_clone().expect("split the peer's connection"));
    let mut writer = stream;
    for line in reader.lines() {
        let Ok(line) = line else { return };
        let Some(request) = parse(&line) else {
            if line.trim().is_empty() {
                continue;
            }
            return;
        };
        let id = request["id"].clone();
        let reply = if request["method"].as_str() == Some(sprag_rpc::CLIENT_HELLO_METHOD) {
            handshake(&id)
        } else {
            missing
                .refusal(&request, &id)
                .unwrap_or_else(|| json!({ "jsonrpc": "2.0", "id": id, "result": Value::Null }))
        };
        if write_line(&mut writer, &reply.to_string()).is_err() {
            return;
        }
    }
}

/// One connection to a PROXY: every frame is relayed to the daemon behind it, except the methods
/// `missing` names, which are refused here.
///
/// The return path is its own pump rather than a reply read after each request, because the daemon
/// also writes UNPROMPTED (`events/subscribe` notifications) and a request/reply loop would hold
/// those until the next question.
fn relay(client: &UnixStream, upstream: &Path, missing: &Missing) {
    let deadline = Instant::now() + UPSTREAM_DEADLINE;
    let up = loop {
        match UnixStream::connect(upstream) {
            Ok(up) => break up,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => return,
        }
    };
    let to_client = Arc::new(Mutex::new(
        client.try_clone().expect("split the peer's connection"),
    ));
    let pumping = Arc::clone(&to_client);
    let from_daemon = up.try_clone().expect("split the upstream connection");
    let pump = std::thread::spawn(move || {
        for line in BufReader::new(from_daemon).lines() {
            let Ok(line) = line else { return };
            let mut out = pumping.lock().expect("the peer's writer");
            if writeln!(out, "{line}").is_err() {
                return;
            }
        }
    });

    let mut to_daemon = up;
    for line in BufReader::new(client.try_clone().expect("split the peer's connection")).lines() {
        let Ok(line) = line else { break };
        let Some(request) = parse(&line) else {
            if line.trim().is_empty() {
                continue;
            }
            break;
        };
        match missing.refusal(&request, &request["id"]) {
            Some(reply) => {
                let mut out = to_client.lock().expect("the peer's writer");
                if write_line(&mut *out, &reply.to_string()).is_err() {
                    break;
                }
            }
            None if writeln!(to_daemon, "{line}").is_err() => break,
            None => {}
        }
    }
    drop(to_daemon);
    let _ = pump.join();
}

/// This build's own protocol number, which is what makes the peer OLD rather than incompatible.
fn handshake(id: &Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id.clone(),
        "result": { sprag_rpc::PROTOCOL_FIELD: sprag_rpc::WIRE_PROTOCOL },
    })
}

/// One line of the wire, or [`None`] for a blank line and for anything that is not JSON.
fn parse(line: &str) -> Option<Value> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    serde_json::from_str(line).ok()
}

fn write_line(writer: &mut impl Write, frame: &str) -> std::io::Result<()> {
    writeln!(writer, "{frame}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The instrument is itself unmeasured until somebody asks** (R320's lesson, on the round
    /// that builds one): every claim this peer makes about what it refuses, driven over a real
    /// socket.
    ///
    /// The CONTROL is the third block: a method the policy does NOT name is answered, so "it
    /// refuses X" is a statement about X rather than about every frame.
    #[test]
    fn a_peer_refuses_exactly_what_it_is_missing() {
        let sock = std::env::temp_dir().join(format!("sprag-peer-it-{}.sock", std::process::id()));
        let peer = OldDaemon::serving_nothing(&sock);
        let mut conn = sprag_rpc::HostConn::connect(peer.sock(), Duration::from_secs(2))
            .expect("connect to the peer");

        // THE HANDSHAKE PASSES, at this build's number — the property that makes it OLD rather
        // than incompatible, and the one a client checks before anything else.
        let hello = conn
            .call(
                sprag_rpc::CLIENT_HELLO_METHOD,
                json!({ sprag_rpc::PROTOCOL_PARAM: sprag_rpc::WIRE_PROTOCOL }),
            )
            .expect("the peer shakes hands");
        assert_eq!(
            hello[sprag_rpc::PROTOCOL_FIELD].as_u64(),
            Some(u64::from(sprag_rpc::WIRE_PROTOCOL)),
        );

        // A READ is refused in the daemon's own words.
        let refused = conn
            .try_call(
                "scene/query",
                json!({ "path": "/sprag_mux/external/panes" }),
            )
            .expect_err("a peer serving nothing refuses a read");
        match refused {
            sprag_rpc::CallError::Fault(fault) => {
                assert_eq!(fault.code, sprag_rpc::INVALID_PARAMS);
                assert_eq!(
                    fault.data.as_ref().and_then(Value::as_str),
                    Some(sprag_rpc::UNKNOWN_SLOT_FAULT),
                );
            }
            other => panic!("a refusal, not {other:?}"),
        }

        // THE CONTROL: an ACTION is not what this peer is missing, so it is answered.
        let answered = conn.call("scene/invoke", json!({ "path": "/x", "args": Value::Null }));
        assert!(
            answered.is_ok(),
            "a peer missing only slots must not refuse an action: {answered:?}",
        );
    }

    /// The two fault strings are the ones the READER matches on, not two copies of them.
    ///
    /// A peer that spelled its own would pass every test in this file and fail to exercise the one
    /// discrimination the product makes — which is what four hand-written copies were one typo
    /// away from at all times.
    #[test]
    fn the_refusals_are_the_wires_own_words() {
        let id = json!(1);
        let query = json!({ "method": "scene/query", "params": { "path": "/x" } });
        let invoke = json!({ "method": "scene/invoke", "params": { "path": "/x" } });
        let slot = Missing::slots()
            .refusal(&query, &id)
            .expect("a missing slot is refused");
        assert_eq!(
            slot["error"]["data"].as_str(),
            Some(sprag_rpc::UNKNOWN_SLOT_FAULT),
        );
        let action = Missing::actions()
            .refusal(&invoke, &id)
            .expect("a missing action is refused");
        assert_eq!(
            action["error"]["data"].as_str(),
            Some(sprag_rpc::UNKNOWN_ACTION_FAULT),
        );
        // Each policy refuses ONLY its own half — the property `Missing::actions` exists for.
        assert!(Missing::actions().refusal(&query, &id).is_none());
        assert!(Missing::slots().refusal(&invoke, &id).is_none());
        assert!(
            Missing::everything()
                .refusal(&json!({ "method": sprag_rpc::CLIENT_HELLO_METHOD }), &id,)
                .is_none(),
            "the handshake is never refused: an older daemon still speaks this protocol",
        );

        // ONE ADDRESS MISSING, which is what a daemon one build behind looks like: it serves
        // everything else, so a client boots against it and fails exactly one read.
        let one = Missing::addresses(&["/x".to_owned()]);
        assert!(
            one.refusal(&query, &id).is_some(),
            "the named address is gone"
        );
        assert!(
            one.refusal(
                &json!({ "method": "scene/query", "params": { "path": "/other" } }),
                &id,
            )
            .is_none(),
            "and every other address is served",
        );
    }
}
