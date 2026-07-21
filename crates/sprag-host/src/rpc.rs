//! The headless JSON-RPC server loop.
//!
//! Serves pinion's scene-as-data wire over a line-delimited transport,
//! resolving each request's [`SessionScope`] and assembling that session's current-window
//! live [`Workspace`] panes into a fresh scene. This is the runnable form of the headless
//! data path
//! (DESIGN.md §1/§3): an external AI peer reads the terminals as data and
//! drives input / pane lifecycle, with no GPU and no shell event loop.
//!
//! ## Method boundary (enforced, not incidental)
//!
//! [`handle_request`] gates to an explicit [`SUPPORTED_METHODS`] allowlist
//! and returns a JSON-RPC method-not-found error for everything else.
//!
//! Reads (`scene/snapshot`, `scene/query`) and input (`scene/invoke`)
//! operate on the same per-request pane scene. Input does *not* go through
//! pinion's `scene/key` (which enqueues a `DeferredInput` for an embedder
//! drain a headless host has no equivalent for); it rides the canonical
//! `scene/invoke` action channel against the pane's engine `External`, whose
//! handler encodes the key (sprag-owned, R2.6) and writes to the live PTY
//! (R1.7). The scene is rebuilt and discarded per request, but the mutation
//! target — the PTY — lives behind the External's
//! `PanePtyHandle`, so the write reaches live state even though the scene
//! does not persist.

use std::io::{self, BufRead, Write};
use std::ops::ControlFlow;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

use pinion_core::SceneRevision;
use pinion_rpc::preview::PreviewLedger;
use pinion_rpc::{
    ConnId, DispatchContext, Request, RpcError, RpcFrame, RpcIngress, RpcReply, WaiterRegistry,
    dispatch, dispatch_parsed, parse_request, try_async_wait_for,
};
use sprag_terminal::{SessionRegistry, Workspace};

use crate::attach::{AttachOutcome, AttachmentRegistry};
use crate::external::lock;
use crate::host::Host;
use crate::runs::RunRegistry;
use crate::scope::{ScopeError, SessionScope};
use crate::wire::{CLIENT_ATTACH_METHOD, CLIENT_HELLO_METHOD, CLIENT_PARAM};

/// The long-lived host state threaded through the serve loop: the booted [`Host`]
/// (the single [`SessionRegistry`] owner), the background plugin-run registry, pinion's
/// per-session dispatch ledgers, and the async `scene/waitFor` waiter registry.
/// Bundled so the per-request handler signature stays stable as future control
/// surfaces are added.
///
/// ## Change-notification (PR-50 §6.3, R115a)
///
/// The [`SceneRevision`] is the ONE scene-version token, shared (`Arc`) with the
/// pane `on_dirty` hooks: a pane's output [`bump`](SceneRevision::bump)s it, which
/// (a) advances the OCC token and (b) fires the wake observer installed in
/// [`new`](Self::new) — `move |n| waiters.wake(n)` — so any parked async
/// `scene/waitFor` reply fires. A wire client thus blocks on `scene/waitFor`
/// until a pane produces output *it did not cause*, instead of busy-polling
/// `scene/snapshot`. The registry parks no version counter of its own; the
/// revision is the single source of truth (pinion's [`WaiterRegistry`] contract).
pub struct HostState {
    host: Host,
    runs: Arc<Mutex<RunRegistry>>,
    previews: PreviewLedger,
    /// The one scene-version token, shared with the pane `on_dirty` bumpers.
    revision: Arc<SceneRevision>,
    /// Parked async `scene/waitFor` replies, woken off `revision`'s observer.
    waiters: Arc<WaiterRegistry>,
    /// Per-client session attachment (R-PR67 Stage 1): `conn -> client -> attached session`,
    /// fed by the `client/hello` + `client/attach` intercepts and the transport's `on_disconnect`
    /// (all on this one dispatch thread), read when the `sessions` slot is served to fill each
    /// [`SessionInfo::attached`](sprag_terminal::SessionInfo::attached). Shared into the scene's
    /// mux control surface per assembly (like `revision`), so the read and the writes see one map.
    attachments: Arc<Mutex<AttachmentRegistry>>,
    /// The self-cleaning daemon's pane-`on_exit` death-signal hook ([`spawn_reaper`]), threaded
    /// into each mux- AND plugin-spawned pane's `on_exit` (via
    /// [`workspace_scene`](crate::workspace_scene) → the mux + plugin externals) so any pane's
    /// death feeds the reaper. `None` off a daemon (the GUI's in-process host, the tests) — the
    /// boot pane's hook is wired separately by the binary, since it is spawned before this
    /// state exists. `Option`, not a hidden default: a non-daemon caller states `None` at the
    /// call site rather than inheriting a policy silently.
    on_pane_exit: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl HostState {
    /// Build host state over a booted [`Host`], sharing `revision` — the ONE
    /// scene-version token the pane `on_dirty` hooks bump. Installs the async
    /// `scene/waitFor` wake observer on it (`move |n| waiters.wake(n)`), so a
    /// revision bump (a pane's output) wakes every parked waiter. A fresh run
    /// registry and waiter registry are created here.
    ///
    /// `on_pane_exit` is the self-cleaning daemon's death-signal hook ([`spawn_reaper`]),
    /// carried to each mux/plugin-spawned pane's `on_exit`; `None` off a daemon (the GUI's
    /// in-process host, the tests state it explicitly).
    #[must_use]
    pub fn new(
        host: Host,
        revision: Arc<SceneRevision>,
        on_pane_exit: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Self {
        let waiters = Arc::new(WaiterRegistry::new());
        // The wake half of the no-lost-wakeup discipline: a revision bump (an OCC
        // mutation OR a pane's external output via on_dirty) fires this, draining
        // and replying to every waiter the new revision surpassed.
        let wake = Arc::clone(&waiters);
        // `set_observer` is install-once (pinion): the FIRST caller wins, later ones
        // no-op and return false. This wake seam is the ONLY thing that fires parked
        // `scene/waitFor` replies, so a silent install-failure would hang every wait
        // forever with no error. Assert we won the install — a fresh revision per
        // HostState makes this always true today; the assert catches a future refactor
        // that reuses an already-observed revision (exactly the silent-failure class
        // the textbook bar wants caught at the wiring point).
        assert!(
            revision.set_observer(move |n| {
                wake.wake(n);
            }),
            "HostState requires a fresh SceneRevision: its wake observer must install \
             (an already-observed revision would leave scene/waitFor parked forever)",
        );
        Self {
            host,
            runs: Arc::new(Mutex::new(RunRegistry::default())),
            previews: PreviewLedger::default(),
            revision,
            waiters,
            on_pane_exit,
            attachments: Arc::new(Mutex::new(AttachmentRegistry::default())),
        }
    }

    /// The daemon's pane-`on_exit` death-signal hook, cloned for a scene assembly
    /// ([`workspace_scene`](crate::workspace_scene)) — `None` off a daemon, so a non-daemon
    /// caller wires no pane to the reaper.
    #[must_use]
    pub fn on_pane_exit(&self) -> Option<Arc<dyn Fn() + Send + Sync>> {
        self.on_pane_exit.clone()
    }

    /// The mux state tree (the [`Host`]'s [`SessionRegistry`]), for the scene-as-data
    /// assembly. The assembly resolves the CURRENT window's pane pool out of it per
    /// request, so a later window switch needs no re-plumbing.
    #[must_use]
    pub fn registry(&self) -> &Arc<Mutex<SessionRegistry>> {
        self.host.registry()
    }

    /// The shared background plugin-run registry.
    #[must_use]
    pub fn runs(&self) -> &Arc<Mutex<RunRegistry>> {
        &self.runs
    }

    /// The one scene-version token (the async `scene/waitFor` / OCC baseline).
    #[must_use]
    pub fn revision(&self) -> &SceneRevision {
        &self.revision
    }

    /// The async `scene/waitFor` waiter registry.
    #[must_use]
    pub fn waiters(&self) -> &WaiterRegistry {
        &self.waiters
    }

    /// The per-client attachment map (R-PR67 Stage 1), for the scene assembly to read the
    /// per-session attached count and for the lifecycle intercepts to write it.
    #[must_use]
    pub fn attachments(&self) -> &Arc<Mutex<AttachmentRegistry>> {
        &self.attachments
    }
}

/// The pane `on_dirty` hook that bumps `revision` on every batch of PTY output —
/// the change-notification recipe a wire server boots each pane with. Passed as the
/// `on_dirty` of [`Host::spawn`](crate::Host::spawn); the bump advances the OCC
/// token AND wakes any parked async `scene/waitFor` (the observer [`HostState::new`]
/// installs). The single home for this closure so the "a pane's output bumps THIS
/// revision" invariant is not hand-rewritten per boot site (the server binary and
/// the tests share it); a client that spawns a pane against a different revision than
/// the one `HostState` observes would silently never wake, so it lives in one place.
#[must_use]
pub fn bump_on_dirty(revision: &Arc<SceneRevision>) -> Box<dyn Fn() + Send> {
    let revision = Arc::clone(revision);
    Box::new(move || {
        revision.bump();
    })
}

/// Spawn the daemon's REAPER and return the registry-free pane-`on_exit` hook that feeds it —
/// the self-cleaning lifetime seam.
///
/// A dedicated reaper thread owns the `registry` and a death-signal channel. The returned hook
/// (wired as every spawned pane's `on_exit`) does nothing but SEND a signal on that channel;
/// each death wakes the reaper thread, which runs `no_live_panes` and, when the child that
/// just exited was the LAST live one across all sessions, runs `on_empty`. The daemon injects
/// `on_empty` (it raises SIGTERM so the one shutdown routine cancels + joins runs), so this
/// library never names process exit itself — a test injects a recording action instead, which
/// is what makes the exact-once behaviour assertable without ending the test process.
///
/// **Why a thread + channel, not a scan on the pane hook (the structural point).** The scan
/// takes workspace locks, and `PanePty::Drop` JOINS the PTY reader thread — so running the
/// scan ON that reader thread would deadlock the moment a future pane-drop site held a
/// workspace lock across the drop (Drop waits on the join; the reader waits on the lock). By
/// moving the scan to a dedicated thread the reader hook only SENDS (non-blocking, no lock),
/// so a Drop-join can never wait on a workspace lock a reader holds — the deadlock is
/// impossible by construction, not by a convention future drop sites must remember. It also
/// keeps the liveness check off the per-output hot path (the R152 lesson): the hook fires once
/// per pane DEATH (it is an `on_exit`), and the scan is one message per death.
///
/// **Wired at EVERY spawn site.** The hook is registry-free (just a channel send), so wiring
/// it into the deliberately registry-free plugin surface (`WorkspacePaneAccess`, the R144
/// Interface-Segregation decision) hands that layer NO lifetime concern — it holds an opaque
/// `Fn`, not the registry. So boot, mux-spawned, AND plugin-spawned panes all feed the one
/// reaper, and no pane category can leave a lingering daemon.
///
/// A daemon with no panes has no hook, so it cannot exit before its first pane. A stale signal
/// (the pane it announced was not the last) is harmless: the scan is idempotent and only the
/// last live pane's death finds the registry empty.
#[must_use]
pub fn spawn_reaper(
    registry: Arc<Mutex<SessionRegistry>>,
    on_empty: Arc<dyn Fn() + Send + Sync>,
) -> Arc<dyn Fn() + Send + Sync> {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    std::thread::Builder::new()
        .name("sprag-reaper".to_owned())
        .spawn(move || {
            for () in rx {
                if no_live_panes(&registry) {
                    on_empty();
                }
            }
        })
        .expect("spawn the reaper thread");
    Arc::new(move || {
        // A dead channel (the reaper thread gone) means the process is already ending; drop
        // the signal, matching the `on_dirty` / RpcIngress error-absorption convention.
        let _ = tx.send(());
    })
}

/// Wrap the shared death-`signal` ([`spawn_reaper`]) as a fresh per-pane `on_exit` box — the
/// ONE conversion every spawn site (boot, mux, plugin) uses, so the `Arc` → `Box` shape is not
/// re-spelled three times.
#[must_use]
pub fn pane_exit_hook(signal: &Arc<dyn Fn() + Send + Sync>) -> Box<dyn Fn() + Send> {
    let signal = Arc::clone(signal);
    Box::new(move || signal())
}

/// Whether NO pane in ANY session's ANY window is still live (every one has reached
/// [`is_eof`](sprag_terminal::PanePty::is_eof)).
///
/// The registry is the authority for pane MEMBERSHIP and each pane's `is_eof` for its
/// LIVENESS, so this reads both rather than keeping a parallel counter that could drift from
/// them. It collects the pools under the registry lock and releases it BEFORE locking any
/// workspace — the registry→workspace order the rest of the host keeps, so it nests with
/// neither. Runs on the dedicated reaper thread ([`spawn_reaper`]), never a PTY reader thread,
/// so a pane Drop that joins a reader cannot deadlock it. Short-circuits on the first live
/// pane, so on the common path (some pane alive) it stops at once.
///
/// **A session with no panes counts as having no LIVE ones** (`[].all(..)` is vacuously
/// true). So an EMPTY session does not keep the daemon alive: if the last pane elsewhere dies
/// while a freshly-created, not-yet-populated session exists, the daemon still exits and that
/// session is discarded. This matches the owner's "zero live panes ⇒ exit" policy (the
/// daemon's lifetime is tied to live panes, not to session existence — session-close
/// semantics are a later increment), but a client that `new_session`s should spawn into it
/// promptly. The unscoped-default totality this rests on is noted on
/// [`SessionRegistry::default_session`](sprag_terminal::SessionRegistry::default_session).
fn no_live_panes(registry: &Arc<Mutex<SessionRegistry>>) -> bool {
    let pools: Vec<Arc<Mutex<Workspace>>> = {
        let reg = lock(registry);
        reg.sessions()
            .iter()
            .flat_map(|session| session.windows().iter())
            .map(|window| Arc::clone(window.workspace()))
            .collect()
    };
    pools
        .iter()
        .all(|pool| lock(pool).panes().iter().all(|pane| pane.pty().is_eof()))
}

/// The methods the headless host answers: pure reads over the pane scene
/// (`scene/snapshot`, `scene/query`), the `scene/invoke` input + plugin channels,
/// and the async change-notification pair (`scene/revision` reads the current
/// scene-version token; `scene/waitFor {since}` blocks until it advances — the
/// async form is intercepted before dispatch, in the per-frame `dispatch_one`).
/// Anything else gets a JSON-RPC method-not-found error.
pub const SUPPORTED_METHODS: &[&str] = &[
    "scene/snapshot",
    "scene/query",
    "scene/invoke",
    "scene/revision",
    "scene/waitFor",
];

/// Answer one JSON-RPC `request_json` string against the workspace's current
/// panes, returning the response JSON (`None` for a notification with no reply).
///
/// Parses, then delegates a well-formed request to [`handle_parsed`]; a malformed
/// request lets pinion's `dispatch` emit the canonical JSON-RPC parse error. This
/// is the string entry point (the tests + the malformed path use it); the live
/// dispatch owner (`dispatch_one`) has already parsed the frame and calls
/// [`handle_parsed`] directly, so a valid request is parsed exactly once.
#[must_use]
pub fn handle_request(state: &HostState, request_json: &str) -> Option<String> {
    match parse_request(request_json) {
        Ok(request) => handle_parsed(state, request),
        Err(_) => {
            // Malformed: assemble a ctx only for the canonical parse-error reply. It cannot
            // carry a scope (there is no parsed request to read one off), and it does not
            // need one — the reply is about the envelope, not about any session.
            let scope = SessionScope::unscoped(state.registry());
            let mut scene = crate::workspace_scene(
                &scope,
                state.registry(),
                &state.runs,
                &state.revision,
                state.on_pane_exit(),
                Some(Arc::clone(state.attachments())),
            );
            let mut ctx = DispatchContext::new(&mut scene, &state.previews, state.revision());
            dispatch(&mut ctx, request_json)
        }
    }
}

/// Answer one already-parsed JSON-RPC `request` against the panes of the session it is
/// SCOPED to — the dispatch core shared by the string entry ([`handle_request`]) and
/// the live dispatch owner (`dispatch_one`, which parses once to intercept async
/// `scene/waitFor` and hands the parsed request straight here). Resolves the request's
/// [`SessionScope`], assembles a fresh scene for that session
/// (`Container[panes… + control External]`), then dispatches an allowlisted method
/// ([`SUPPORTED_METHODS`]) or rejects a non-allowlisted one with a method-not-found error.
/// Only the async `scene/waitFor` form is handled earlier (in `dispatch_one`); the v0
/// since-less form falls through here to pinion's synchronous handler.
///
/// **Scope before method**, copying pinion's own ordering (`dispatch_parsed` validates its
/// `window` scope ahead of routing): a request whose scope cannot be honored is refused
/// whole, whatever it was going to ask. Routing first would mean deciding what a
/// malformed-scope read or write "probably meant", and the honest answer is that it means
/// nothing.
#[must_use]
pub fn handle_parsed(state: &HostState, request: Request) -> Option<String> {
    match SessionScope::resolve(state.registry(), &request) {
        Ok(scope) => handle_scoped(state, &scope, request),
        Err(error) => scope_refused(&request, &error),
    }
}

/// Answer `request` against the session `scope` already names — the dispatch body, split from
/// [`handle_parsed`] so the live owner (`dispatch_one`) can resolve the scope EARLY, refuse a
/// bad one before the async intercept, and then hand the resolved answer straight here.
///
/// The split is what keeps one request to one resolution: without it `dispatch_one` would
/// check the scope and `handle_parsed` would immediately derive it again, and two derivations
/// of one fact is how they come to disagree.
#[must_use]
fn handle_scoped(state: &HostState, scope: &SessionScope, request: Request) -> Option<String> {
    let mut scene = crate::workspace_scene(
        scope,
        state.registry(),
        &state.runs,
        &state.revision,
        state.on_pane_exit(),
        Some(Arc::clone(state.attachments())),
    );
    let mut ctx = DispatchContext::new(&mut scene, &state.previews, state.revision());
    if SUPPORTED_METHODS.contains(&request.method.as_str()) {
        dispatch_parsed(&mut ctx, request)
    } else {
        Some(method_not_supported(&request))
    }
}

/// The JSON-RPC reply for a request whose session scope could not be honored — `-32602
/// Invalid params`, built through pinion's own [`RpcError::invalid_params`] so the code and
/// envelope are its vocabulary rather than a second spelling of them.
///
/// `None` for a NOTIFICATION (no `id`): there is nobody to tell, and inventing a reply for a
/// request that asked for none would violate JSON-RPC. Silent, but not silently WRONG — the
/// scope is refused either way, which is the property that matters. pinion's `dispatch_parsed`
/// makes the identical choice at the identical point.
fn scope_refused(request: &Request, error: &ScopeError) -> Option<String> {
    let id = request.id.clone()?;
    let rpc = RpcError::invalid_params(error);
    Some(
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": rpc.code, "message": rpc.message, "data": rpc.data },
        })
        .to_string(),
    )
}

/// Build the JSON-RPC method-not-found (-32601) reply for a well-formed but
/// non-allowlisted request, naming the supported set. The list is derived from
/// [`SUPPORTED_METHODS`] (not re-typed), so the const stays the single source.
fn method_not_supported(request: &Request) -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": request.id,
        "error": {
            "code": -32601,
            "message": format!(
                "sprag-term host: '{}' is unsupported; use one of: {}",
                request.method,
                SUPPORTED_METHODS.join(", "),
            ),
        }
    })
    .to_string()
}

/// The JSON-RPC success reply for an intercepted client-lifecycle method: `result: true` for a
/// request, `None` for a notification (no `id` — nobody to answer, and inventing one would break
/// JSON-RPC), the same id-less choice [`scope_refused`] makes.
fn lifecycle_ok(request: &Request) -> Option<String> {
    let id = request.id.clone()?;
    Some(serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": true }).to_string())
}

/// The JSON-RPC `-32602 Invalid params` reply for a malformed client-lifecycle request (a missing
/// client id, an attach with no prior hello). `None` for a notification, like [`lifecycle_ok`].
fn lifecycle_invalid(request: &Request, message: String) -> Option<String> {
    let id = request.id.clone()?;
    Some(
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32602, "message": message },
        })
        .to_string(),
    )
}

/// `client/hello` — register this connection under the client id its params carry, so the daemon
/// groups a client's several connections into one attached unit. Identity only: no session, so it
/// moves no attachment count and needs no scene bump.
fn handle_hello(state: &HostState, conn: ConnId, request: &Request) -> Option<String> {
    match request.params.as_ref().and_then(|p| p.get(CLIENT_PARAM)) {
        Some(serde_json::Value::String(client)) => {
            lock(state.attachments()).hello(conn, client.clone());
            lifecycle_ok(request)
        }
        _ => lifecycle_invalid(request, format!("params.{CLIENT_PARAM} must be a string")),
    }
}

/// `client/attach` — attach (or switch — tmux `switch-client`) this connection's client to the
/// already-validated `scope` session. A first attach or a switch moves a per-session count, so it
/// bumps the scene to wake parked `scene/waitFor`s to re-read the badge; an idempotent re-send
/// does not. An attach with no prior `client/hello` is a protocol error, refused `-32602`.
fn handle_attach(
    state: &HostState,
    conn: ConnId,
    scope: &SessionScope,
    request: &Request,
) -> Option<String> {
    match lock(state.attachments()).attach(conn, scope.session().to_owned()) {
        AttachOutcome::NoClient => lifecycle_invalid(
            request,
            format!("{CLIENT_ATTACH_METHOD} requires {CLIENT_HELLO_METHOD} first"),
        ),
        AttachOutcome::Changed => {
            tracing::info!(
                target: "sprag_host::attach",
                session = %scope.session(),
                "client attached"
            );
            state.revision().bump();
            lifecycle_ok(request)
        }
        AttachOutcome::Unchanged => lifecycle_ok(request),
    }
}

/// One item on the dispatch owner's FIFO: a frame to dispatch, or a connection-closed signal.
///
/// Routing the close signal ([`RpcIngress::on_disconnect`]) onto the SAME channel as frames is
/// what makes it crash-safe AND correctly ordered. pinion guarantees that, for one connection,
/// every `submit` of its frames happens-before its `on_disconnect` on the transport's reader
/// thread; funnelling both through this one queue preserves that order on the dispatch side, so
/// the owner tears down a connection's attachment strictly AFTER dispatching all of its frames —
/// never a disconnect racing ahead of the frames it should follow.
pub enum IngressEvent {
    /// A JSON-RPC frame to dispatch (from any transport's `submit`).
    Frame(RpcFrame),
    /// A connection closed (EOF / reset / crash) — release its per-client attachment.
    Disconnect(ConnId),
}

/// An [`RpcIngress`] that funnels frames — and connection-close signals — from any transport into
/// the host's single dispatch owner via one channel.
///
/// The GUI dispatches on pinion-shell's winit event loop; the headless host
/// has no event loop, so it owns one dispatch thread ([`dispatch_frames`]) and
/// every transport -- stdin and the always-on socket -- submits through this
/// into that one owner. Serialising dispatch this way means a concurrent
/// socket connection and a stdin line share one consistent [`HostState`] view,
/// the same single-owner discipline pinion's UI thread gives the GUI. The close
/// signal rides the same channel (see [`IngressEvent`]) so it is ordered after the
/// connection's frames and mutates the shared state on the same one thread.
pub struct FrameIngress {
    tx: Sender<IngressEvent>,
}

impl FrameIngress {
    /// Wrap the sending half of the dispatch owner's channel.
    #[must_use]
    pub fn new(tx: Sender<IngressEvent>) -> Self {
        Self { tx }
    }
}

impl RpcIngress for FrameIngress {
    fn submit(&self, frame: RpcFrame) {
        // A closed channel means the dispatch owner has exited; drop the frame
        // (its reply never fires, so the client's connection simply closes).
        let _ = self.tx.send(IngressEvent::Frame(frame));
    }

    fn on_disconnect(&self, conn: ConnId) {
        // R-PR67: the transport's per-connection reader ended (EOF / reset / crash). Route the
        // signal onto the dispatch FIFO so the owner releases this connection's attachment on the
        // one thread that owns the map, after its frames. A closed channel means the owner has
        // already exited — nothing to release.
        let _ = self.tx.send(IngressEvent::Disconnect(conn));
    }
}

/// The single dispatch owner: pull [`IngressEvent`]s and, per frame, dispatch against `state`
/// through the same [`handle_request`] core (routing the response back to the frame's originating
/// transport via its reply sink); per connection-close, release that connection's attachment. One
/// thread, so all dispatch AND attachment mutation is serialised over the shared [`HostState`].
/// Runs until every sender has dropped (the channel closes) -- for a server with an always-on
/// socket that is process lifetime.
pub fn dispatch_frames(state: &HostState, rx: Receiver<IngressEvent>) {
    for event in rx {
        match event {
            IngressEvent::Frame(frame) => dispatch_one(state, frame),
            IngressEvent::Disconnect(conn) => {
                // Release the closed connection's attachment. If that dropped an ATTACHED client
                // (its last connection), a per-session count fell, so the scene changed — bump the
                // revision to wake every parked `scene/waitFor` to re-read the badge.
                let released = lock(state.attachments()).disconnect(conn);
                if let Some(session) = released {
                    tracing::info!(
                        target: "sprag_host::attach",
                        %session,
                        "client detached (connection closed)"
                    );
                    state.revision().bump();
                }
            }
        }
    }
}

/// Dispatch one frame against `state` — the per-frame body of [`dispatch_frames`],
/// split out so the async `scene/waitFor` park/wake path is unit-testable without
/// standing up the channel loop.
///
/// Parses the frame ONCE, then resolves its session scope ONCE. An async
/// `scene/waitFor {since}` is intercepted BEFORE
/// the synchronous core: [`try_async_wait_for`] either answers it immediately (the
/// scene already advanced past `since`) or PARKS its reply in the waiter registry —
/// in which case the reply fires LATER, off this dispatch thread, on the scene bump
/// that wakes it ([`HostState`] installed the wake observer). A non-`waitFor` frame
/// (or a since-less v0 `waitFor`) is handed straight to [`handle_scoped`] with the
/// already-parsed request and its already-resolved scope — no re-parse, no re-resolve. A
/// malformed frame goes to [`handle_request`]
/// for the canonical parse-error reply. Parking does not build the workspace scene,
/// so a blocked wait costs nothing until a pane actually produces output.
///
/// ## The scope is resolved before the async intercept, deliberately
///
/// `try_async_wait_for` parks on the revision without ever looking at a scope, so validating
/// inside [`handle_parsed`] would leave `scene/waitFor` the ONE method where a malformed or
/// unknown `session` was accepted and ignored — a hole in exactly the shape of the bug the
/// param exists to close, in exactly the corner (the async path) that pinion's own R890.1
/// scar hid in. Resolving here covers every method by construction rather than by each one
/// remembering.
///
/// **v1 bound, honest and documented rather than silent:** a waitFor's scope is CHECKED but
/// not yet HONORED — the revision is one token for the whole registry, so a client scoped to
/// `work` is woken when any session moves. The contract it can rely on is therefore "you are
/// woken AT LEAST when your session changes": the wake is a hint to re-read, and the re-read
/// is scoped and exact. Over-reporting is safe by construction here (`park_if_current` only
/// ever answers early; nothing reads the revision as a write precondition), so tightening
/// this to "only when" strengthens the contract without breaking a client built on it. That
/// is why a scoped waitFor is accepted rather than refused — refusing would force every
/// client to special-case the one method it scopes uniformly, and buy nothing.
fn dispatch_one(state: &HostState, frame: RpcFrame) {
    // R-PR67: keep the frame's originating connection id — the client-lifecycle intercepts below
    // attribute `client/hello` + `client/attach` to it. Every other method ignores it (a stateless
    // ingress), which is why `conn` never reaches pinion's generic dispatch core.
    let RpcFrame {
        conn,
        request,
        reply,
    } = frame;
    match parse_request(&request) {
        Ok(parsed) => {
            // `client/hello` carries identity, not a session, so it is handled before scope
            // resolution: it registers this connection's client id (the group key a client's
            // several connections share) and returns.
            if parsed.method.as_str() == CLIENT_HELLO_METHOD {
                if let Some(response) = handle_hello(state, conn, &parsed) {
                    reply.send(response);
                }
                return;
            }
            let scope = match SessionScope::resolve(state.registry(), &parsed) {
                Ok(scope) => scope,
                Err(error) => {
                    if let Some(response) = scope_refused(&parsed, &error) {
                        reply.send(response);
                    }
                    return;
                }
            };
            // `client/attach` declares (or switches — tmux `switch-client`) this connection's
            // client to the session the scope check above just validated, so an unknown attach
            // target is refused by the same machinery every other request uses.
            if parsed.method.as_str() == CLIENT_ATTACH_METHOD {
                if let Some(response) = handle_attach(state, conn, &scope, &parsed) {
                    reply.send(response);
                }
                return;
            }
            match try_async_wait_for(&parsed, state.revision(), state.waiters(), reply) {
                // Parked (or answered immediately) by the registry — nothing more to do.
                ControlFlow::Break(()) => {}
                // Not an async waitFor: dispatch the ALREADY-parsed request (no re-parse).
                ControlFlow::Continue(reply) => {
                    if let Some(response) = handle_scoped(state, &scope, parsed) {
                        reply.send(response);
                    }
                }
            }
        }
        // Malformed: the string entry emits the canonical JSON-RPC parse error.
        Err(_) => {
            if let Some(response) = handle_request(state, &request) {
                reply.send(response);
            }
        }
    }
}

/// Read newline-delimited JSON-RPC requests from `input` and submit each as an
/// [`RpcFrame`] whose reply writes the response (newline-terminated) to stdout,
/// through `tx` into the dispatch owner. Returns when `input` reaches EOF -- the
/// stdin transport ends, but any other transport (the socket) keeps the server
/// alive. Blank lines are skipped.
pub fn stdin_frames(input: impl BufRead, tx: &Sender<IngressEvent>) {
    // The stdin reader is one logical connection, so every frame it produces
    // carries the same, stable id (R-PR67) -- mirroring pinion's built-in
    // stdin transport, which stamps its frames with its own single id.
    let conn = ConnId::allocate();
    for line in input.lines() {
        let Ok(text) = line else {
            break;
        };
        let request = text.trim();
        if request.is_empty() {
            continue;
        }
        let reply = RpcReply::new(|response| {
            let mut out = io::stdout().lock();
            if writeln!(out, "{response}").is_ok() {
                let _ = out.flush();
            }
        });
        if tx
            .send(IngressEvent::Frame(RpcFrame::new(
                conn,
                request.to_owned(),
                reply,
            )))
            .is_err()
        {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::external::lock;
    use sprag_terminal::{CommandBuilder, PaneId};
    use std::thread::sleep;
    use std::time::{Duration, Instant};

    fn sh(script: &str) -> CommandBuilder {
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(script);
        command.env("TERM", "dumb");
        command
    }

    /// Host state with one initial pane running `script`, wired the way a wire
    /// server boots: the pane's `on_dirty` bumps the shared [`SceneRevision`], so
    /// its output wakes any parked async `scene/waitFor` (the change-notification
    /// path R115a serves).
    fn host_with(script: &str, cols: u16, rows: u16) -> HostState {
        let revision = Arc::new(SceneRevision::new());
        let host = Host::new((cols, rows));
        // The SAME boot recipe prod uses (sprag-term.rs) — the shared `bump_on_dirty`
        // helper, so the test exercises the real "pane output bumps THIS revision" wire.
        host.spawn(
            sh(script),
            "sh".to_string(),
            cols,
            rows,
            Some(bump_on_dirty(&revision)),
            None,
        )
        .expect("spawn pane");
        HostState::new(host, revision, None)
    }

    /// One request through the dispatch path (no serve loop / shutdown join), so
    /// the `HostState` persists across calls and a background run is not joined
    /// between requests.
    fn serve_one(state: &HostState, request: &str) -> serde_json::Value {
        let response = handle_request(state, request).expect("a response");
        serde_json::from_str(response.trim()).expect("valid json-rpc response")
    }

    /// Block (bounded) until pane 0's child has closed its PTY.
    fn wait_for_pane0_eof(state: &HostState) {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            let eof = lock(&state.host.workspace())
                .pane(PaneId(0))
                .is_none_or(|p| p.pty().is_eof());
            if eof {
                break;
            }
            sleep(Duration::from_millis(20));
        }
    }

    /// Block (bounded) until pane `id` in `host`'s default workspace has reached EOF.
    fn wait_for_eof(host: &Host, id: PaneId) {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if lock(&host.workspace())
                .pane(id)
                .is_none_or(|p| p.pty().is_eof())
            {
                return;
            }
            sleep(Duration::from_millis(20));
        }
        panic!("pane {id:?} never reached EOF");
    }

    /// `no_live_panes` tracks registry liveness both ways: false while a child runs, true
    /// once every child has exited. It is the predicate the daemon ends its process on, so a
    /// wrong answer either strands a daemon over dead panes or kills one still serving.
    #[test]
    fn no_live_panes_tracks_the_registrys_liveness() {
        // A running `cat` keeps it false.
        let host = Host::new((40, 6));
        host.spawn(sh("exec cat"), "cat".into(), 40, 6, None, None)
            .expect("spawn cat");
        assert!(
            !no_live_panes(host.registry()),
            "a running child means panes are live",
        );

        // A sole pane whose child exits flips it true (polled — the child exits async).
        let host = Host::new((40, 6));
        let id = host
            .spawn(sh("exec true"), "true".into(), 40, 6, None, None)
            .expect("spawn true");
        wait_for_eof(&host, id);
        assert!(
            no_live_panes(host.registry()),
            "the sole pane's child exited, so nothing is live",
        );
    }

    /// A recording `on_empty` for [`spawn_reaper`], plus the death-signal it returns.
    fn recording_reaper(
        registry: &Arc<Mutex<SessionRegistry>>,
    ) -> (
        Arc<dyn Fn() + Send + Sync>,
        Arc<std::sync::atomic::AtomicUsize>,
    ) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let fired = Arc::new(AtomicUsize::new(0));
        let f = Arc::clone(&fired);
        let on_empty: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            f.fetch_add(1, Ordering::SeqCst);
        });
        (spawn_reaper(Arc::clone(registry), on_empty), fired)
    }

    /// The self-cleaning edge, driven through the REAL seam ([`spawn_reaper`] +
    /// [`pane_exit_hook`]): a pane's death signals a dedicated reaper thread, which scans the
    /// registry and runs `on_empty` ONLY when nothing live remains. The scan is off the PTY
    /// reader threads by construction, so it cannot deadlock a pane Drop.
    #[test]
    fn the_reaper_fires_only_when_the_last_pane_is_gone() {
        use std::sync::atomic::Ordering;

        // The last pane's death fires the action exactly once.
        let host = Host::new((40, 6));
        let (signal, fired) = recording_reaper(host.registry());
        host.spawn(
            sh("exec true"),
            "true".into(),
            40,
            6,
            None,
            Some(pane_exit_hook(&signal)),
        )
        .expect("spawn true");
        // Poll for the fire (the reaper thread scans asynchronously; poll, don't sleep).
        let start = Instant::now();
        while fired.load(Ordering::SeqCst) == 0 && start.elapsed() < Duration::from_secs(5) {
            sleep(Duration::from_millis(20));
        }
        assert_eq!(
            fired.load(Ordering::SeqCst),
            1,
            "the last pane's death ends the daemon exactly once",
        );

        // A pane dying beside a LIVE one must not fire — the daemon serves on.
        let host = Host::new((40, 6));
        let (signal, fired) = recording_reaper(host.registry());
        host.spawn(sh("exec cat"), "cat".into(), 40, 6, None, None)
            .expect("spawn cat"); // stays live
        let dying = host
            .spawn(
                sh("exec true"),
                "true".into(),
                40,
                6,
                None,
                Some(pane_exit_hook(&signal)),
            )
            .expect("spawn true");
        wait_for_eof(&host, dying);
        sleep(Duration::from_millis(200)); // ample for the reaper to have scanned, had it fired
        assert_eq!(
            fired.load(Ordering::SeqCst),
            0,
            "a pane dying beside a live one must not end the daemon",
        );
    }

    /// A PLUGIN-spawned pane's death ALSO feeds the reaper — the R160-review gap, closed. The
    /// plugin surface ([`WorkspacePaneAccess`]) is deliberately registry-free, but it carries
    /// the same opaque death-signal via [`with_pane_exit`](sprag_plugin::WorkspacePaneAccess::with_pane_exit),
    /// so a pane it spawns is no longer a category that can leave a lingering daemon. Driven
    /// through the real `PaneLifecycle::spawn` (the path that had NO hook before the fix).
    #[test]
    fn a_plugin_spawned_panes_death_also_feeds_the_reaper() {
        use sprag_plugin::{PaneLifecycle, WorkspacePaneAccess};
        use std::sync::atomic::Ordering;

        let host = Host::new((40, 6));
        let (signal, fired) = recording_reaper(host.registry());
        // The plugin surface over the default session's pool (what the reaper scans),
        // carrying the daemon's death-signal — the wiring `workspace_scene` does in production.
        let access = WorkspacePaneAccess::new(host.workspace()).with_pane_exit(Some(signal));
        let id = access
            .spawn(&["/bin/sh".into(), "-c".into(), "exec true".into()], 40, 6)
            .expect("plugin-spawn a pane");

        wait_for_eof(&host, id);
        let start = Instant::now();
        while fired.load(Ordering::SeqCst) == 0 && start.elapsed() < Duration::from_secs(5) {
            sleep(Duration::from_millis(20));
        }
        assert_eq!(
            fired.load(Ordering::SeqCst),
            1,
            "a plugin-spawned pane was the last to die, so the daemon must self-clean — \
             before the fix this pane category fed no hook and left a lingering daemon",
        );
    }

    /// The multi-SESSION case C1a made first-class, exercised rather than assumed: a live pane
    /// in ANOTHER session keeps the daemon alive when the default session empties. The reaper
    /// scans every session, so the default's dying pane finds `work`'s live one and does not
    /// fire. Without the cross-session traversal this would false-exit.
    #[test]
    fn a_live_pane_in_another_session_keeps_the_daemon_alive() {
        use std::sync::atomic::Ordering;

        let host = Host::new((40, 6));
        let registry = host.registry();
        let (signal, fired) = recording_reaper(registry);

        // Session "work" with a live `cat` (blocks on its PTY, so it stays live).
        lock(registry)
            .new_session(Some("work"))
            .expect("a free name");
        let work_pool = lock(registry)
            .workspace_of("work")
            .expect("the created session resolves");
        lock(&work_pool)
            .spawn(sh("exec cat"), "work-cat".into(), 40, 6)
            .expect("spawn cat into work");

        // The default session's sole pane exits, carrying the death-signal.
        let dying = host
            .spawn(
                sh("exec true"),
                "0-true".into(),
                40,
                6,
                None,
                Some(pane_exit_hook(&signal)),
            )
            .expect("spawn true into the default");
        wait_for_eof(&host, dying);
        sleep(Duration::from_millis(200)); // let the reaper's check run

        assert!(
            !no_live_panes(registry),
            "work's live cat means panes remain — the daemon must not be empty",
        );
        assert_eq!(
            fired.load(Ordering::SeqCst),
            0,
            "the default emptying must not end a daemon that still serves another session",
        );
    }

    fn invoke_key(state: &HostState, pane: u64, key: &str) {
        let request = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"scene/invoke","params":{{"path":"/pane_{pane}/sprag_input/external/key","args":{{"key":"{key}"}}}}}}"#
        );
        let value = serve_one(state, &request);
        assert!(value.get("error").is_none(), "invoke error: {value}");
    }

    /// Poll the live snapshot until it contains `needle`.
    fn wait_for_snapshot(state: &HostState, needle: &str) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            let snap = serve_one(
                state,
                r#"{"jsonrpc":"2.0","id":9,"method":"scene/snapshot","params":{"path":""}}"#,
            );
            if snap["result"].to_string().contains(needle) {
                return true;
            }
            sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn serve_answers_scene_snapshot_with_live_screen() {
        let state = host_with("printf hi", 20, 4);
        wait_for_pane0_eof(&state);
        let value = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/snapshot","params":{"path":""}}"#,
        );
        assert_eq!(value["id"], 1);
        assert!(value.get("error").is_none(), "unexpected error: {value}");
        // The grid text nests under workspace -> pane_0 -> TextGrid.
        assert!(
            value["result"].to_string().contains("hi"),
            "expected 'hi' in result, got: {}",
            value["result"]
        );
    }

    #[test]
    fn serve_rejects_scene_key_in_favor_of_scene_invoke() {
        // Input rides scene/invoke against a pane's engine External, not
        // pinion's widget-oriented scene/key — so scene/key stays unsupported.
        let state = host_with("printf hi", 20, 4);
        let value = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":2,"method":"scene/key","params":{"key":"a"}}"#,
        );
        assert_eq!(value["id"], 2);
        assert_eq!(value["error"]["code"], -32601);
    }

    #[test]
    fn serve_injects_key_into_a_pane() {
        let state = host_with("cat", 20, 4);
        invoke_key(&state, 0, "h");
        invoke_key(&state, 0, "i");
        assert!(
            wait_for_snapshot(&state, "hi"),
            "injected 'hi' never appeared"
        );
    }

    #[test]
    fn serve_spawns_addresses_and_closes_panes() {
        // Multiplex lifecycle over the wire: spawn a 2nd pane, address it,
        // list panes, close one.
        let state = host_with("cat", 20, 4);
        let spawned = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/invoke","params":{"path":"/sprag_mux/external/spawn","args":{"cmd":["cat"],"cols":20,"rows":4}}}"#,
        );
        assert_eq!(
            spawned["result"].as_i64(),
            Some(1),
            "new pane id: {spawned}"
        );

        invoke_key(&state, 1, "Z");
        assert!(wait_for_snapshot(&state, "Z"), "pane 1 never echoed 'Z'");

        let panes = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":2,"method":"scene/query","params":{"path":"/sprag_mux/external/panes"}}"#,
        );
        assert_eq!(panes["result"].as_array().map(Vec::len), Some(2));

        let closed = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":3,"method":"scene/invoke","params":{"path":"/sprag_mux/external/close","args":{"id":0}}}"#,
        );
        assert!(closed.get("error").is_none(), "close error: {closed}");
        assert_eq!(lock(&state.host.workspace()).panes().len(), 1);
    }

    /// Poll `query("runs")` until run 0 reports `done`, returning its outcome
    /// state (or `None` on timeout).
    fn wait_for_run_done(state: &HostState) -> Option<String> {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            let runs = serve_one(
                state,
                r#"{"jsonrpc":"2.0","id":7,"method":"scene/query","params":{"path":"/sprag_plugins/external/runs"}}"#,
            );
            let run = &runs["result"][0];
            if run["state"]["status"] == "done" {
                return run["state"]["outcome"]["state"]
                    .as_str()
                    .map(str::to_string);
            }
            sleep(Duration::from_millis(20));
        }
        None
    }

    /// Poll `query("runs")` until run 0 reports `done`, returning its full
    /// `state` JSON (outcome + any captured `output`), or `None` on timeout.
    fn wait_for_run0_state(state: &HostState) -> Option<serde_json::Value> {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(10) {
            let runs = serve_one(
                state,
                r#"{"jsonrpc":"2.0","id":7,"method":"scene/query","params":{"path":"/sprag_plugins/external/runs"}}"#,
            );
            let run = &runs["result"][0];
            if run["state"]["status"] == "done" {
                return Some(run["state"].clone());
            }
            sleep(Duration::from_millis(20));
        }
        None
    }

    #[test]
    fn runs_an_agent_plugin_to_done_capturing_its_reply() {
        // The agent adapter over a one-shot fake AI (read the prompt until EOF,
        // reply deterministically). The reply is surfaced as the run's `output`.
        let state = host_with("in=$(cat); echo \"REPLY[$in]\"", 40, 6);
        let started = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/invoke","params":{"path":"/sprag_plugins/external/run","args":{"plugin":"agent","pane":0,"prompt":"ping"}}}"#,
        );
        assert_eq!(started["result"].as_i64(), Some(0), "run id: {started}");

        let run_state = wait_for_run0_state(&state).expect("agent run reached done");
        assert_eq!(run_state["outcome"]["state"], "converged");
        assert!(
            run_state["output"]
                .as_str()
                .is_some_and(|o| o.contains("REPLY[ping]")),
            "expected the captured reply in output, got: {}",
            run_state["output"]
        );
    }

    #[test]
    fn runs_a_dialogue_plugin_to_done_with_a_transcript() {
        // Two count-fake endpoints: each replies with the newline-count of its
        // prompt, which grows as the transcript accumulates — proving the host
        // run passes the WHOLE history each turn. Each turn spawns a transient
        // pane that must be reaped, so only the host's initial pane survives.
        let state = host_with("cat", 40, 6);
        let endpoint = serde_json::json!([
            "/bin/sh",
            "-c",
            "n=$(printf '%s' \"$1\" | wc -l | tr -d ' '); printf 'saw%s\\n' \"$n\"",
            "_"
        ]);
        let request = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "scene/invoke",
            "params": {
                "path": "/sprag_plugins/external/run",
                "args": {
                    "plugin": "dialogue",
                    "endpoint_a": endpoint,
                    "endpoint_b": endpoint,
                    "seed": "count upward",
                    "cols": 40, "rows": 6,
                    "guardrails": { "max_iterations": 3, "max_tokens": 1048576 }
                }
            }
        })
        .to_string();
        let started = serve_one(&state, &request);
        assert_eq!(started["result"].as_i64(), Some(0), "run id: {started}");

        let run_state = wait_for_run0_state(&state).expect("dialogue run reached done");
        assert_eq!(run_state["outcome"]["state"], "exhausted");
        // The transcript alternates labels and the reported line-counts strictly
        // increase (the history accumulates each turn) — asserted on the trend,
        // not exact counts, so the prompt format can change freely.
        let output = run_state["output"].as_str().unwrap_or_default();
        assert!(
            output.contains("A: saw") && output.contains("B: saw"),
            "expected an alternating accumulating transcript, got: {output:?}"
        );
        let counts: Vec<u32> = output
            .match_indices("saw")
            .map(|(i, _)| {
                output[i + 3..]
                    .chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
                    .parse()
                    .expect("a saw count")
            })
            .collect();
        assert!(
            counts.len() == 3 && counts.windows(2).all(|w| w[0] < w[1]),
            "history must accumulate (strictly increasing): {counts:?}"
        );
        // Only the initial pane remains — every per-turn pane was reaped.
        assert_eq!(
            lock(&state.host.workspace()).panes().len(),
            1,
            "dialogue leaked a pane"
        );
    }

    #[test]
    fn runs_a_claude_json_dialogue_with_real_token_cost() {
        // A JSON fake emits a one-line `--output-format json` envelope with
        // fixed usage; `format_*: claude_json` makes the run parse it off the
        // RAW source for the real token cost and the clean reply text — the
        // round's whole point, surfaced over RPC.
        let state = host_with("cat", 40, 6);
        let endpoint = serde_json::json!([
            "/bin/sh",
            "-c",
            "printf '%s' '{\"result\":\"hi there\",\"usage\":{\"input_tokens\":30,\"output_tokens\":20}}'",
            "_"
        ]);
        let request = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "scene/invoke",
            "params": {
                "path": "/sprag_plugins/external/run",
                "args": {
                    "plugin": "dialogue",
                    "endpoint_a": endpoint,
                    "endpoint_b": endpoint,
                    "seed": "go",
                    "format_a": "claude_json",
                    "format_b": "claude_json",
                    "cols": 40, "rows": 6,
                    "guardrails": { "max_iterations": 2, "max_tokens": 1048576 }
                }
            }
        })
        .to_string();
        let started = serve_one(&state, &request);
        assert_eq!(started["result"].as_i64(), Some(0), "run id: {started}");

        let run_state = wait_for_run0_state(&state).expect("dialogue run reached done");
        assert_eq!(run_state["outcome"]["state"], "exhausted");
        // Two turns × (30 + 20) tokens — the real billed cost over RPC.
        assert_eq!(
            run_state["outcome"]["cost"].as_u64(),
            Some(100),
            "{run_state}"
        );
        assert_eq!(
            run_state["outcome"]["unit"], "tokens",
            "cost unit must be tokens: {run_state}"
        );
        let output = run_state["output"].as_str().unwrap_or_default();
        // The clean `result` is the transcript, not the raw envelope.
        assert!(
            output.contains("hi there"),
            "clean reply missing: {output:?}"
        );
        assert!(
            !output.contains("input_tokens"),
            "raw envelope leaked: {output:?}"
        );
        assert_eq!(
            lock(&state.host.workspace()).panes().len(),
            1,
            "dialogue leaked a pane"
        );
    }

    #[test]
    fn rejects_an_unknown_reply_format() {
        // A bad `format_*` is a synchronous Rejected (a typo, not an async Fail).
        let state = host_with("cat", 20, 4);
        let rejected = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/invoke","params":{"path":"/sprag_plugins/external/run","args":{"plugin":"dialogue","endpoint_a":["true"],"endpoint_b":["true"],"seed":"x","format_a":"yaml"}}}"#,
        );
        assert!(
            rejected.get("error").is_some(),
            "expected a rejection: {rejected}"
        );
    }

    #[test]
    fn rejects_a_wrong_unit_guardrail() {
        // The dialogue is token-denominated; a `max_bytes` bound is the wrong
        // currency for it and must be a synchronous Rejected — never a silently
        // ignored or mis-unit bound (the guardrail is the spend defence).
        let state = host_with("cat", 20, 4);
        let rejected = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/invoke","params":{"path":"/sprag_plugins/external/run","args":{"plugin":"dialogue","endpoint_a":["true"],"endpoint_b":["true"],"seed":"x","guardrails":{"max_bytes":4096}}}}"#,
        );
        assert!(
            rejected.get("error").is_some(),
            "wrong-unit guardrail must reject: {rejected}"
        );
    }

    #[test]
    fn full_text_query_includes_scrolled_off_lines() {
        // Read-path parity: an external RPC peer reads the same full output
        // (scrollback + visible) the in-process capture path sees, so a scrolled
        // reply is not invisible over the wire.
        let state = host_with("seq 1 30", 20, 4);
        wait_for_pane0_eof(&state);
        let resp = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/query","params":{"path":"/pane_0/sprag_input/external/full_text"}}"#,
        );
        let text = resp["result"].as_str().unwrap_or_default();
        assert!(
            text.contains("\n5\n"),
            "scrolled-off line 5 missing over RPC: {text:?}"
        );
        assert!(
            text.contains("\n30"),
            "last line missing over RPC: {text:?}"
        );
    }

    #[test]
    fn last_command_query_slices_the_childs_osc_133_cycle() {
        // A child emitting a full OSC 133 cycle: prompt (A) + typed command, output
        // start (C), one output line, command end (D) exit 0. The last_command slot
        // slices it into {command, output, exit_status, running} over the real wire —
        // the command-scoped read tmux's whole-pane capture cannot express.
        let state = host_with(
            r"printf '\033]133;A\007$ echo hi\033]133;B\007\r\n\033]133;C\007hi\r\n\033]133;D;0\007'",
            20,
            6,
        );
        wait_for_pane0_eof(&state);
        let resp = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/query","params":{"path":"/pane_0/sprag_input/external/last_command"}}"#,
        );
        let cmd = &resp["result"];
        assert_eq!(cmd["command"], "$ echo hi", "the command line: {resp}");
        assert_eq!(cmd["output"], "hi", "the sliced output: {resp}");
        assert_eq!(cmd["exit_status"].as_i64(), Some(0), "{resp}");
        assert_eq!(cmd["running"], false, "{resp}");
    }

    #[test]
    fn prompt_marks_query_lists_the_childs_prompt_positions() {
        // Two prompt cycles from the child; the prompt_marks slot lists the logical row
        // index of each prompt-start (0 and 3) over the real wire — the jump-to-prompt
        // targets a display client scrolls to.
        let state = host_with(
            r"printf '\033]133;A\007$ a\033]133;B\007\r\n\033]133;C\007a\r\n\033]133;D;0\007\r\n\033]133;A\007$ b\033]133;B\007\r\n\033]133;C\007b\r\n\033]133;D;0\007'",
            20,
            8,
        );
        wait_for_pane0_eof(&state);
        let resp = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/query","params":{"path":"/pane_0/sprag_input/external/prompt_marks"}}"#,
        );
        assert_eq!(
            resp["result"],
            serde_json::json!([0, 3]),
            "prompt rows over RPC: {resp}"
        );
    }

    #[test]
    fn lists_the_agent_among_available_plugins() {
        let state = host_with("cat", 20, 4);
        let plugins = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/query","params":{"path":"/sprag_plugins/external/plugins"}}"#,
        );
        let names = plugins["result"].as_array().expect("a plugins array");
        assert!(
            names.iter().any(|n| n == "agent"),
            "expected 'agent' in plugins, got: {}",
            plugins["result"]
        );
        assert!(
            names.iter().any(|n| n == "dialogue"),
            "expected 'dialogue' in plugins, got: {}",
            plugins["result"]
        );
    }

    #[test]
    fn runs_an_orchestrator_plugin_in_the_background_to_convergence() {
        // cat echoes the stimulus, so the orchestrator converges on the sentinel.
        let state = host_with("cat", 20, 4);
        let started = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/invoke","params":{"path":"/sprag_plugins/external/run","args":{"plugin":"orchestrator","pane":0,"stimulus":"ping","sentinel":"ping","guardrails":{"max_iterations":5,"max_bytes":4096}}}}"#,
        );
        assert_eq!(started["result"].as_i64(), Some(0), "run id: {started}");
        assert_eq!(wait_for_run_done(&state).as_deref(), Some("converged"));
    }

    #[test]
    fn run_with_an_unknown_pane_is_rejected_synchronously() {
        // Submit-time validation: a missing pane is a synchronous Rejected,
        // not an async Failed the peer has to poll for.
        let state = host_with("cat", 20, 4);
        let rejected = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/invoke","params":{"path":"/sprag_plugins/external/run","args":{"plugin":"orchestrator","pane":99,"stimulus":"x"}}}"#,
        );
        assert!(
            rejected.get("error").is_some(),
            "expected a rejection: {rejected}"
        );
    }

    #[test]
    fn a_running_plugin_does_not_block_the_serve_loop() {
        // A `sleep` pane never echoes, so each orchestrator step burns its full
        // observe timeout — the run takes ~1s. Meanwhile an immediate snapshot
        // must still return promptly, proving the run is off the serve path.
        let state = host_with("sleep 5", 20, 4);
        serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/invoke","params":{"path":"/sprag_plugins/external/run","args":{"plugin":"orchestrator","pane":0,"stimulus":"x","guardrails":{"max_iterations":2,"max_bytes":1048576}}}}"#,
        );
        let start = Instant::now();
        let snap = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":2,"method":"scene/snapshot","params":{"path":""}}"#,
        );
        assert!(snap.get("error").is_none(), "snapshot error: {snap}");
        assert!(
            start.elapsed() < Duration::from_millis(500),
            "snapshot blocked behind the run: {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn cancels_a_running_plugin_over_rpc() {
        // A sleep pane never echoes, so the orchestrator loops until cancelled.
        let state = host_with("sleep 30", 20, 4);
        let started = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/invoke","params":{"path":"/sprag_plugins/external/run","args":{"plugin":"orchestrator","pane":0,"stimulus":"x","guardrails":{"max_iterations":1000000,"max_bytes":1073741824}}}}"#,
        );
        assert_eq!(started["result"].as_i64(), Some(0), "run id: {started}");

        // An unknown run id is a synchronous rejection.
        let bad = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":2,"method":"scene/invoke","params":{"path":"/sprag_plugins/external/cancel","args":{"id":999}}}"#,
        );
        assert!(
            bad.get("error").is_some(),
            "unknown id should reject: {bad}"
        );

        // Cancel the live run; it then reaches done = cancelled.
        let cancelled = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":3,"method":"scene/invoke","params":{"path":"/sprag_plugins/external/cancel","args":{"id":0}}}"#,
        );
        assert!(
            cancelled.get("error").is_none(),
            "cancel error: {cancelled}"
        );
        assert_eq!(wait_for_run_done(&state).as_deref(), Some("cancelled"));
    }

    #[test]
    fn shutdown_cancels_in_flight_runs_promptly() {
        // The serve-shutdown path: cancel_all() then join_all(). With a sleep
        // pane and no cancel, join would block on the looping orchestrator;
        // cancelling first makes shutdown return promptly with the run reaped.
        let state = host_with("sleep 30", 20, 4);
        serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/invoke","params":{"path":"/sprag_plugins/external/run","args":{"plugin":"orchestrator","pane":0,"stimulus":"x","guardrails":{"max_iterations":1000000,"max_bytes":1073741824}}}}"#,
        );

        let start = Instant::now();
        {
            let mut runs = lock(state.runs());
            runs.cancel_all();
            runs.join_all();
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "shutdown blocked on the in-flight run: {:?}",
            start.elapsed()
        );

        let runs = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":2,"method":"scene/query","params":{"path":"/sprag_plugins/external/runs"}}"#,
        );
        assert_eq!(runs["result"][0]["state"]["outcome"]["state"], "cancelled");
    }

    // ─── R115a: async change-notification (scene/revision + scene/waitFor) ───

    /// A recording reply sink: collects whatever the reply is sent, so a test can
    /// assert what (if anything) a parked / immediate `scene/waitFor` fired.
    fn recording_reply(sink: &Arc<Mutex<Vec<String>>>) -> RpcReply {
        let sink = Arc::clone(sink);
        RpcReply::new(move |response| sink.lock().unwrap().push(response))
    }

    /// One frame through the real per-frame dispatch body (`dispatch_one`) with a
    /// recording reply, so the async park/immediate paths are exercised exactly as
    /// the serve loop runs them.
    fn dispatch_recording(state: &HostState, request: &str, sink: &Arc<Mutex<Vec<String>>>) {
        dispatch_one(
            state,
            RpcFrame::new(
                ConnId::allocate(),
                request.to_owned(),
                recording_reply(sink),
            ),
        );
    }

    // ─── C1a: the session scope, over the real dispatch path ───

    /// A scope that cannot be honored refuses the request WHOLE — `-32602`, whatever it was
    /// going to ask, across every method the host serves.
    ///
    /// The per-method sweep is the point rather than thoroughness theatre: the failure this
    /// guards is a scope silently ignored on ONE surface, so checking a single method would
    /// leave exactly the hole the test claims to close.
    ///
    /// **Scope of the claim, and it is narrower than it looks:** this drives the SYNCHRONOUS
    /// entry ([`handle_request`]), so its `scene/waitFor` row proves nothing about the async
    /// intercept — that frame is answered here by the very core `dispatch_one` skips. The
    /// path that matters for waitFor has its own test
    /// ([`a_wait_for_with_an_unhonorable_scope_is_refused_and_never_parks`]), because it is
    /// the one place a scope could be accepted and ignored.
    #[test]
    fn a_scope_that_cannot_be_honored_refuses_every_method() {
        let state = host_with("cat", 20, 4);
        for method in SUPPORTED_METHODS {
            for (scope, why) in [
                (r#"42"#, "a non-string scope"),
                (r#""ghost""#, "a name no session carries"),
            ] {
                let request = format!(
                    r#"{{"jsonrpc":"2.0","id":1,"method":"{method}","params":{{"path":"","since":0,"session":{scope}}}}}"#
                );
                let value = serve_one(&state, &request);
                assert_eq!(
                    value["error"]["code"], -32602,
                    "{why} on {method} must be Invalid params, not acted on: {value}",
                );
            }
        }
    }

    /// The refusal is not merely an error — it is an error INSTEAD of the act.
    ///
    /// A `-32602` that had already spawned the pane would satisfy the test above while being
    /// the exact bug it is written against, so this one checks the pane set.
    #[test]
    fn a_refused_scope_never_reaches_the_default_session() {
        let state = host_with("cat", 20, 4);
        let before = lock(&state.host.workspace()).panes().len();
        let spawn = |scope: &str| {
            format!(
                r#"{{"jsonrpc":"2.0","id":1,"method":"scene/invoke","params":{{"session":{scope},"path":"/sprag_mux/external/spawn","args":{{"cmd":["cat"]}}}}}}"#
            )
        };
        for scope in ["42", r#""ghost""#] {
            let value = serve_one(&state, &spawn(scope));
            assert_eq!(value["error"]["code"], -32602, "{value}");
        }
        assert_eq!(
            lock(&state.host.workspace()).panes().len(),
            before,
            "a request whose scope was refused must not have spawned anything, least of all \
             into the session it did not name",
        );
        // The control: the SAME spawn with a scope that resolves does land — so the
        // assertion above is about the scope, not about a spawn path that never works.
        let value = serve_one(&state, &spawn(r#""0""#));
        assert!(value.get("error").is_none(), "{value}");
        assert_eq!(lock(&state.host.workspace()).panes().len(), before + 1);
    }

    /// An unscoped request keeps working, unchanged — every client that predates the param
    /// sends none, and the default is what it has always meant.
    #[test]
    fn an_unscoped_request_still_answers_from_the_default_session() {
        let state = host_with("printf hi", 20, 4);
        wait_for_pane0_eof(&state);
        let value = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/snapshot","params":{"path":""}}"#,
        );
        assert!(value.get("error").is_none(), "unexpected error: {value}");
        assert!(
            value["result"].to_string().contains("hi"),
            "the boot pane's screen still answers with no session named: {}",
            value["result"],
        );
    }

    /// A second session over the REAL dispatch: created by name (BORN with its own shell), then
    /// addressed by it — and what it holds is its own.
    #[test]
    fn a_named_session_is_addressable_and_holds_its_own_panes() {
        let state = host_with("cat", 20, 4);
        let created = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/invoke","params":{"path":"/sprag_mux/external/new_session","args":{"name":"work"}}}"#,
        );
        assert_eq!(created["result"], "work", "{created}");

        // `work` is born with one pane (its birth shell), landed in IT — not the default. Then a
        // `cat`, spawned by NAMING `work`, joins it. A surface that ignored the scope would put
        // both the birth pane and this spawn in the DEFAULT — leaving work at 0 and the default
        // at 3 — so work == 2 and default == 1 proves the scope routed each to `work` alone.
        let spawned = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":2,"method":"scene/invoke","params":{"session":"work","path":"/sprag_mux/external/spawn","args":{"cmd":["cat"]}}}"#,
        );
        assert!(spawned.get("error").is_none(), "{spawned}");

        let panes = |scope: &str| -> usize {
            let request = format!(
                r#"{{"jsonrpc":"2.0","id":3,"method":"scene/query","params":{{"session":"{scope}","path":"/sprag_mux/external/panes"}}}}"#
            );
            serve_one(&state, &request)["result"]
                .as_array()
                .map(Vec::len)
                .expect("the panes slot answers with an array")
        };
        assert_eq!(
            panes("work"),
            2,
            "work holds its birth pane plus the one spawned into it"
        );
        assert_eq!(panes("0"), 1, "the default still holds only its boot pane");
    }

    /// The async intercept honors the scope too: a `scene/waitFor` whose scope cannot be
    /// honored is REFUSED and never parks — exercised through the REAL `dispatch_one`, the
    /// path `handle_request` cannot reach.
    ///
    /// This is the guard the per-method sweep could not give. `try_async_wait_for` parks on
    /// the revision without ever seeing a scope, so if the check lived in `handle_parsed`
    /// instead of ahead of the intercept, a bad-scope waitFor would slip past it and park —
    /// the accept-and-ignore failure, in the exact corner (the async path) pinion's own
    /// R890.1 scar hid in. Resolving the scope before the intercept is what closes it, and a
    /// parked-count of zero after a refusal is what proves the close.
    #[test]
    fn a_wait_for_with_an_unhonorable_scope_is_refused_and_never_parks() {
        let state = host_with("cat", 20, 4);
        let since = state.revision().current();
        for (scope, why) in [
            ("42", "a non-string scope"),
            (r#""ghost""#, "an unknown session"),
        ] {
            let sink = Arc::new(Mutex::new(Vec::new()));
            dispatch_recording(
                &state,
                &format!(
                    r#"{{"jsonrpc":"2.0","id":5,"method":"scene/waitFor","params":{{"since":{since},"session":{scope}}}}}"#
                ),
                &sink,
            );
            assert_eq!(
                state.waiters().parked_count(),
                0,
                "{why} must be refused BEFORE the async park, not parked and ignored",
            );
            let responses = sink.lock().unwrap();
            assert_eq!(responses.len(), 1, "the refusal was answered: {why}");
            let v: serde_json::Value = serde_json::from_str(&responses[0]).unwrap();
            assert_eq!(v["error"]["code"], -32602, "{why}: {v}");
        }

        // The control: a well-scoped waitFor against a live baseline DOES park — so the
        // zero-parked assertions above are the refusal at work, not a waitFor that never
        // parks regardless.
        let sink = Arc::new(Mutex::new(Vec::new()));
        dispatch_recording(
            &state,
            &format!(
                r#"{{"jsonrpc":"2.0","id":6,"method":"scene/waitFor","params":{{"since":{since},"session":"0"}}}}"#
            ),
            &sink,
        );
        assert_eq!(
            state.waiters().parked_count(),
            1,
            "a well-scoped waitFor against the current revision parks normally",
        );
        assert!(
            sink.lock().unwrap().is_empty(),
            "...and is not answered while parked",
        );
    }

    /// A pane of another session is not addressable — not refused by a check, but absent
    /// from the scene the request is answered against, which is why there is no check to
    /// forget.
    #[test]
    fn a_pane_of_another_session_is_not_in_the_scoped_scene() {
        let state = host_with("cat", 20, 4);
        serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/invoke","params":{"path":"/sprag_mux/external/new_session","args":{"name":"work"}}}"#,
        );
        // Pane 0 is the default session's boot pane. Ask `work` for it.
        let value = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":2,"method":"scene/query","params":{"session":"work","path":"/pane_0/sprag_input/external/full_text"}}"#,
        );
        assert!(
            value.get("error").is_some(),
            "pane 0 belongs to the default session; `work` must not be able to read it: {value}",
        );
        // The control: the session that DOES hold it answers.
        let value = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":3,"method":"scene/query","params":{"session":"0","path":"/pane_0/sprag_input/external/full_text"}}"#,
        );
        assert!(
            value.get("error").is_none(),
            "the default session holds pane 0 and must answer for it: {value}",
        );
    }

    #[test]
    fn scene_revision_reports_the_current_token() {
        // The non-blocking read a wire client bootstraps its waitFor `since` from.
        let state = host_with("cat", 20, 4);
        let resp = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/revision","params":{}}"#,
        );
        assert!(
            resp.get("error").is_none(),
            "scene/revision never errors: {resp}"
        );
        let reported = resp["result"]["revision"]
            .as_u64()
            .expect("a numeric revision");
        assert_eq!(
            reported,
            state.revision().current(),
            "reads the one shared token"
        );
    }

    #[test]
    fn async_wait_for_parks_then_a_scene_bump_wakes_it() {
        // The park/wake integration: dispatch_one routes a `scene/waitFor {since}`
        // into the registry (park at the current revision), and the wake observer
        // HostState installed fires the parked reply on the next bump. Deterministic
        // (a direct bump stands in for a pane's on_dirty), no pane-timing.
        let state = host_with("cat", 20, 4);
        let since = state.revision().current();
        let sink = Arc::new(Mutex::new(Vec::new()));
        dispatch_recording(
            &state,
            &format!(
                r#"{{"jsonrpc":"2.0","id":5,"method":"scene/waitFor","params":{{"since":{since}}}}}"#
            ),
            &sink,
        );
        assert_eq!(
            state.waiters().parked_count(),
            1,
            "parked at the current revision"
        );
        assert!(sink.lock().unwrap().is_empty(), "not answered while parked");

        let new = state.revision().bump();
        assert_eq!(
            state.waiters().parked_count(),
            0,
            "the bump drained the parked waiter"
        );
        let responses = sink.lock().unwrap();
        assert_eq!(responses.len(), 1, "the parked reply fired on the bump");
        let v: serde_json::Value = serde_json::from_str(&responses[0]).unwrap();
        assert_eq!(v["id"], 5);
        assert_eq!(v["result"]["changed"], true);
        assert_eq!(v["result"]["revision"], new);
    }

    #[test]
    fn async_wait_for_answers_immediately_when_the_scene_already_advanced() {
        // A stale baseline (`since` < current) is answered at dispatch, not parked —
        // so a client that fell behind catches up without blocking.
        let state = host_with("cat", 20, 4);
        state.revision().bump();
        let current = state.revision().current();
        let sink = Arc::new(Mutex::new(Vec::new()));
        dispatch_recording(
            &state,
            r#"{"jsonrpc":"2.0","id":6,"method":"scene/waitFor","params":{"since":0}}"#,
            &sink,
        );
        assert_eq!(
            state.waiters().parked_count(),
            0,
            "a stale baseline does not park"
        );
        let responses = sink.lock().unwrap();
        assert_eq!(responses.len(), 1, "answered immediately");
        let v: serde_json::Value = serde_json::from_str(&responses[0]).unwrap();
        assert_eq!(v["result"]["revision"], current);
    }

    #[test]
    fn a_panes_output_wakes_a_parked_async_wait_for() {
        // The end-to-end wire-client path, headless: block on scene/waitFor, then
        // the pane produces output with NO client input, its on_dirty bumps the
        // shared revision, and the parked reply fires — the change-driven repaint
        // signal a wire GUI long-polls. Bounded poll (no wall-clock assertion).
        let state = host_with("sleep 0.2; printf X", 20, 4);
        let since = state.revision().current();
        let sink = Arc::new(Mutex::new(Vec::new()));
        dispatch_recording(
            &state,
            &format!(
                r#"{{"jsonrpc":"2.0","id":7,"method":"scene/waitFor","params":{{"since":{since}}}}}"#
            ),
            &sink,
        );
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if !sink.lock().unwrap().is_empty() {
                break;
            }
            sleep(Duration::from_millis(20));
        }
        let responses = sink.lock().unwrap();
        assert_eq!(
            responses.len(),
            1,
            "the pane's own output woke the parked waiter"
        );
        let v: serde_json::Value = serde_json::from_str(&responses[0]).unwrap();
        assert_eq!(v["result"]["changed"], true);
        assert!(
            v["result"]["revision"].as_u64().unwrap() > since,
            "woke at a revision past the client's baseline",
        );
    }

    #[test]
    fn a_mux_spawn_wakes_a_parked_async_wait_for() {
        // Round 1 rail, through the REAL dispatch: a pane-SET change (a mux `spawn`,
        // not pane output) wakes a parked waiter — the pane-lifecycle
        // change-notification a mirror long-polls to learn the host gained a pane.
        // Deterministic: the spawn's set-change bump fires the parked reply
        // synchronously on this thread, so no pane-timing / wall-clock is involved.
        let state = host_with("cat", 20, 4);
        let since = state.revision().current();
        let sink = Arc::new(Mutex::new(Vec::new()));
        dispatch_recording(
            &state,
            &format!(
                r#"{{"jsonrpc":"2.0","id":8,"method":"scene/waitFor","params":{{"since":{since}}}}}"#
            ),
            &sink,
        );
        assert_eq!(
            state.waiters().parked_count(),
            1,
            "parked at the current revision"
        );
        // Spawn a second pane over the real `/sprag_mux` control surface. `cat`
        // produces no output on its own, so the ONLY bump is the spawn's set-change.
        let spawned = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":9,"method":"scene/invoke","params":{"path":"/sprag_mux/external/spawn","args":{"cmd":["cat"]}}}"#,
        );
        assert!(spawned.get("error").is_none(), "spawn error: {spawned}");
        assert_eq!(
            state.waiters().parked_count(),
            0,
            "the spawn's set-change bump drained the parked waiter"
        );
        let responses = sink.lock().unwrap();
        assert_eq!(responses.len(), 1, "the parked reply fired on the spawn");
        let v: serde_json::Value = serde_json::from_str(&responses[0]).unwrap();
        assert_eq!(v["result"]["changed"], true);
        assert!(
            v["result"]["revision"].as_u64().unwrap() > since,
            "woke at a revision past the client's baseline",
        );
    }

    // ─── R115b: pane cells over the wire (the client's per-frame data read) ───

    /// `scene/query` pane 0's input external at `member`, over the full dispatch path.
    fn query_pane0(state: &HostState, member: &str) -> serde_json::Value {
        serve_one(
            state,
            &format!(
                r#"{{"jsonrpc":"2.0","id":1,"method":"scene/query","params":{{"path":"/pane_0/sprag_input/external/{member}"}}}}"#
            ),
        )
    }

    /// The pane's frame at scrollback `offset`, over the full dispatch path — the
    /// `cells.<offset>` QUERY, the ONE address of the concept since PINION-PR61 (`offset ==
    /// 0` is the live view the poll loop reads each wake). A `MethodOcc::Read` at every
    /// offset, so no frame read — live or history — bumps the revision.
    ///
    /// Built through [`cells_slot_at`], the same builder the client uses, so a test cannot
    /// address a member the production path never would.
    fn cells_frame(state: &HostState, offset: usize) -> serde_json::Value {
        query_pane0(state, &crate::wire::cells_slot_at(offset))
    }

    /// One concept, ONE address — and the ABI, not a comment, is what says so.
    ///
    /// This replaces R154's `the_cells_invoke_refuses_the_live_view_…`, whose subject no
    /// longer exists: that test pinned a REFUSAL that policed a two-door split (a `frame`
    /// query beside a `cells` invoke), and PR-61 retired the split rather than the door.
    /// The property it was really defending survives here — a frame is reachable at exactly
    /// one kind of address — so the two retired doors must now answer nothing at all. A
    /// tolerated alias is how a split grows back.
    #[test]
    fn the_retired_two_door_addresses_are_gone() {
        let state = host_with("printf hi", 20, 4);
        wait_for_pane0_eof(&state);

        let old_slot = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/query","params":{"path":"/pane_0/sprag_input/external/frame"}}"#,
        );
        assert!(
            old_slot.get("error").is_some(),
            "the retired `frame` slot must answer nothing: {old_slot}",
        );
        let old_action = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/invoke","params":{"path":"/pane_0/sprag_input/external/cells","args":{"offset":3}}}"#,
        );
        assert!(
            old_action.get("error").is_some(),
            "the retired `cells` invoke must answer nothing: {old_action}",
        );
        // ...and the surviving family serves the live view.
        assert!(
            cells_frame(&state, 0)["result"]["cells"].is_object(),
            "`cells.0` is the live view",
        );
    }

    /// What the family ANSWERS matches what it DECLARES — over the real dispatch path, with
    /// a live pane, which is the only place this can be observed.
    ///
    /// The split with `wire.rs`'s tripwire is deliberate: that one pins the declaration's
    /// spelling, this one pins the surface's behaviour. R155's first draft tried to do both
    /// in `wire.rs` with `SchemaField::addresses` and did neither — `addresses` is not the
    /// predicate `query` dispatch uses, so the test asserted an agreement between two things
    /// that never meet.
    ///
    /// The taxonomy under test is pinion's, not sprag's invention:
    /// * a NON-member (`cells`, the bare stem) is ABSENT — it carries no argument, so it
    ///   addresses no frame;
    /// * a MEMBER whose argument is malformed (`cells.zzz`, `cells.-1`) is PRESENT-BUT-EMPTY
    ///   (`Null`) — "`width.zzz` belongs to `width` and is malformed, not unknown";
    /// * a MEMBER past the top CLAMPS, because the domain says which offsets are meaningful,
    ///   not which are answerable.
    #[test]
    fn the_cells_family_answers_the_paths_it_declares() {
        let state = host_with("printf hi", 20, 4);
        wait_for_pane0_eof(&state);

        // The count that bounds the family — served, or `IndexOf(FRAMES_SLOT)` would name a
        // path an agent cannot read, which is the discovery hole PR-61 was filed about.
        let frames = query_pane0(&state, crate::wire::FRAMES_SLOT);
        assert_eq!(
            frames["result"], 1,
            "a pane with no scrollback addresses exactly the live frame: {frames}",
        );

        // A member the argument makes malformed: present-but-empty, NOT absent.
        for malformed in ["cells.zzz", "cells.-1", "cells.+1", "cells.007"] {
            let answer = query_pane0(&state, malformed);
            assert!(
                answer.get("error").is_none(),
                "`{malformed}` is a MEMBER of a declared family — denying the path exists \
                 tells an agent something false about the surface: {answer}",
            );
            assert!(
                answer["result"].is_null(),
                "...and a malformed argument answers Null, never a plausible frame: {answer}",
            );
        }

        // A non-member really is absent: the stem carries no argument.
        assert!(
            query_pane0(&state, "cells").get("error").is_some(),
            "the bare stem addresses no frame",
        );

        // Past the top clamps to a real frame rather than erroring (the projection's
        // documented behaviour, which `IndexOf` does not promise away).
        assert!(
            query_pane0(&state, "cells.99999")["result"]["cells"].is_object(),
            "an out-of-range offset clamps to the top",
        );
    }

    #[test]
    fn the_cells_family_returns_a_deserializable_grid_frame() {
        // The wire client's per-frame read: `cells.0` returns a JSON frame whose `cells`
        // deserialize back into the EXACT GridBuffer the host projected (PR-49 round-trip),
        // carrying the pane content, plus the scroll facts that ride with it.
        let state = host_with("printf hi", 20, 4);
        wait_for_pane0_eof(&state);
        let frame = cells_frame(&state, 0);
        assert!(frame.get("error").is_none(), "frame error: {frame}");
        let result = &frame["result"];
        assert!(
            result["scrollback_len"].is_u64(),
            "scroll facts present: {result}"
        );
        assert_eq!(result["visible_rows"], 4);

        let cells: pinion_core::GridBuffer = serde_json::from_value(result["cells"].clone())
            .expect("GridBuffer deserializes off the wire");
        assert_eq!(
            (cells.cols(), cells.rows()),
            (20, 4),
            "buffer dims match the pane"
        );
        // "hi" is on row 0 — the wire buffer carries the exact projected content.
        assert_eq!(cells.cell(0, 0).map(|c| c.cluster.as_ref()), Some("h"));
        assert_eq!(cells.cell(1, 0).map(|c| c.cluster.as_ref()), Some("i"));
    }

    #[test]
    fn the_cells_family_honors_the_scrollback_offset() {
        // 40 lines into a 4-row pane: most scroll off into history. The live view
        // (`cells.0`) and a scrolled-up view (`cells.20`) differ, proving the offset reaches
        // the projection over the wire — carried on the PATH, which is the whole of PR-61.
        let state = host_with("seq 1 40", 20, 4);
        wait_for_pane0_eof(&state);

        let live = cells_frame(&state, 0);
        assert!(
            live["result"]["scrollback_len"].as_u64().unwrap() > 0,
            "lines scrolled off into history: {live}",
        );
        let live_cells: pinion_core::GridBuffer =
            serde_json::from_value(live["result"]["cells"].clone()).unwrap();

        let scrolled = cells_frame(&state, 20);
        let scrolled_cells: pinion_core::GridBuffer =
            serde_json::from_value(scrolled["result"]["cells"].clone()).unwrap();

        assert_ne!(
            live_cells, scrolled_cells,
            "a scrollback offset changes the projected buffer",
        );
    }
}
