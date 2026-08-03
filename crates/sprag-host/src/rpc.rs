//! The headless JSON-RPC server loop.
//!
//! Serves pinion's scene-as-data wire over a line-delimited transport,
//! resolving each request's [`SessionScope`] and assembling that session's current-window
//! live [`Workspace`] panes into a fresh scene. This is the runnable form of the headless
//! data path
//! (DESIGN.md §1 + §3): an external AI peer reads the terminals as data and
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
use std::sync::{Arc, Mutex, MutexGuard};

use pinion_core::SceneRevision;
use pinion_rpc::preview::PreviewLedger;
use pinion_rpc::{
    ConnId, DispatchContext, Request, RpcError, RpcFrame, RpcIngress, RpcReply, WaiterRegistry,
    dispatch, dispatch_parsed, parse_request, try_async_wait_for,
};
use sprag_terminal::{SessionRegistry, Workspace};

use crate::PaneCells;
use crate::attach::{AttachOutcome, AttachmentRegistry, ClientSize, SizeOutcome};
use crate::external::lock;
use crate::host::Host;
use crate::notify::ChannelRegistry;
use crate::runs::RunRegistry;
use crate::scope::{ScopeError, SessionScope};
use crate::wire::{
    CLIENT_ATTACH_METHOD, CLIENT_HELLO_METHOD, CLIENT_PARAM, CLIENT_SIZE_METHOD, COLS_PARAM,
    EVENTS_WAIT_METHOD, INVALID_PARAMS, PROTOCOL_FIELD, PROTOCOL_PARAM, ROWS_PARAM, SINCE_PARAM,
    WIRE_PROTOCOL,
};

/// The long-lived host state threaded through the serve loop: the booted [`Host`]
/// (the single [`SessionRegistry`] owner), the background plugin-run registry, pinion's
/// per-session dispatch ledgers, and the async `scene/waitFor` waiter registry.
/// Bundled so the per-request handler signature stays stable as future control
/// surfaces are added.
///
/// ## Change-notification (pinion §6.3, PR-50, R115a)
///
/// Every session has its OWN scene-version token and its own parked `scene/waitFor` replies
/// ([`ChannelRegistry`]): a pane's output [`bump`](SceneRevision::bump)s the token its session
/// owns, which (a) advances that session's OCC token and (b) fires the wake observer the channel
/// installed, so the replies parked on THAT session fire and no others. A wire client thus blocks
/// on `scene/waitFor` until its own session changes, instead of busy-polling `scene/snapshot` —
/// and, since the grain became the session, instead of waking on every other session's traffic.
/// No version counter is parked here; each channel's revision is the single source of truth
/// (pinion's [`WaiterRegistry`] contract).
pub struct HostState {
    host: Host,
    runs: Arc<Mutex<RunRegistry>>,
    previews: PreviewLedger,
    /// Per-session scene-version tokens + parked waits — shared (`Arc`) with the mux control
    /// surface, which announces a session's changes on it and closes a killed session's channel.
    channels: Arc<ChannelRegistry>,
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
    /// The daemon's agent-state memory (H3), shared into the mux control surface per assembly like
    /// [`Self::attachments`], or `None` for a host that runs no detector.
    ///
    /// **A host that installs this must also drive the settle waker**
    /// ([`crate::agent`]'s module docs, and `sprag-term`'s `spawn_agent_waker`). The two are paired by
    /// this comment rather than by a type, so the reason is worth stating: the registry alone gives
    /// verdicts that rest on PRESENT evidence, because the output that paints a dialog is the same
    /// event that wakes a reader — while a verdict resting on an ABSENCE is confirmed by a clock
    /// nothing else in this daemon runs. Installing the registry without the waker is therefore not a
    /// smaller feature; it is one that reports `Blocked` promptly and `Idle` only by luck.
    ///
    /// `None` is the honest state for a host without both. It leaves the `agent` key absent, which D8
    /// already defines as "no agent here", so the wire shape is the pre-H3 one rather than a wrong
    /// answer. Who that is, measured rather than repeated from the comments nearby: `sprag-latency`
    /// (which measures the pane list, so a detector it did not ask for would land in the instrument)
    /// and the test harnesses. Nothing outside this crate builds a [`HostState`] at all.
    agents: Option<Arc<crate::AgentClock>>,
}

impl HostState {
    /// Build host state over a booted [`Host`], sharing `channels` — the per-session scene-version
    /// tokens the pane `on_dirty` hooks bump. Each channel installs its own async `scene/waitFor`
    /// wake observer when it is minted, so a bump on a session's token wakes exactly the waits
    /// parked on that session. A fresh run registry is created here.
    ///
    /// `channels` is passed IN rather than made here because the caller wires the boot pane's
    /// bumper before this state exists, and that pane's output must announce on the very channel
    /// this state later parks waits against — two registries would leave the boot pane bumping a
    /// token nobody waits on.
    ///
    /// `on_pane_exit` is the self-cleaning daemon's death-signal hook ([`spawn_reaper`]),
    /// carried to each mux/plugin-spawned pane's `on_exit`; `None` off a daemon (the GUI's
    /// in-process host, the tests state it explicitly).
    #[must_use]
    pub fn new(
        host: Host,
        channels: Arc<ChannelRegistry>,
        on_pane_exit: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Self {
        Self {
            host,
            runs: Arc::new(Mutex::new(RunRegistry::default())),
            previews: PreviewLedger::default(),
            channels,
            on_pane_exit,
            attachments: Arc::new(Mutex::new(AttachmentRegistry::default())),
            agents: None,
        }
    }

    /// Install the agent-state memory (H3), sharing it with whoever drives the settle waker.
    ///
    /// A builder rather than a fourth parameter on [`new`](Self::new) because exactly one of this
    /// project's four host owners wants it: the daemon. A parameter would make the other three — two
    /// test harnesses and `sprag-latency` — state `None` about a subsystem they have nothing to do
    /// with, and `sprag-latency` in particular measures the pane list, so a detector it did not ask
    /// for would land in the instrument.
    ///
    /// See the field's docs for why installing this without a waker is the one configuration to avoid.
    #[must_use]
    pub fn with_agents(mut self, agents: Arc<crate::AgentClock>) -> Self {
        self.agents = Some(agents);
        self
    }

    /// The agent-state memory, cloned for a scene assembly — `None` for a host running no detector.
    #[must_use]
    pub fn agents(&self) -> Option<Arc<crate::AgentClock>> {
        self.agents.clone()
    }

    /// Everything a DAEMON shares into a scene assembly, as one value.
    ///
    /// One accessor rather than three at each call site, so the two assembly sites cannot come to
    /// disagree about which of the three a scene gets — which is exactly how a surface ends up half
    /// wired and answering plausibly. The `attachments` map is always present on a `HostState`; the
    /// other two are present only when their owner installed them.
    #[must_use]
    pub fn shared(&self) -> crate::DaemonShared {
        crate::DaemonShared {
            on_pane_exit: self.on_pane_exit(),
            attachments: Some(Arc::clone(self.attachments())),
            agents: self.agents(),
            // The HOST's samplers, not fresh ones: a scene is assembled per request, so a sampler
            // minted here would hold nothing at the moment it was asked and every request would take
            // its own `/proc` walk. One set per host is what makes the answers shared.
            samplers: self.host.samplers().clone(),
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

    /// The per-session change channels — the async `scene/waitFor` tokens and waiter sets.
    #[must_use]
    pub fn channels(&self) -> &Arc<ChannelRegistry> {
        &self.channels
    }

    /// `session`'s scene-version token (its async `scene/waitFor` / OCC baseline).
    ///
    /// Named by SESSION, because a revision number means nothing without one: two sessions'
    /// counters advance independently, so a `since` read under one scope is not a baseline under
    /// another. The one client that waits reads its baseline on the connection it has already
    /// scoped, which is what keeps that contract kept.
    #[must_use]
    pub fn revision(&self, session: &str) -> Arc<SceneRevision> {
        self.channels.revision(session)
    }

    /// `session`'s parked async `scene/waitFor` replies.
    #[must_use]
    pub fn waiters(&self, session: &str) -> Arc<WaiterRegistry> {
        self.channels.waiters(session)
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
/// token AND wakes the async `scene/waitFor` replies parked on it (the observer that token's
/// channel installed). The single home for this closure so the "a pane's output bumps THIS
/// revision" invariant is not hand-rewritten per boot site (the server binary and
/// the tests share it); a client that spawns a pane against a different revision than
/// the one its session's waits park on would silently never wake, so it lives in one place.
///
/// `revision` must be the token of the session the pane is being spawned INTO
/// ([`ChannelRegistry::revision`]). Capturing it once here is sound because a pane cannot change
/// session: `break_pane` / `join_pane` move one between WINDOWS of a session and nothing moves one
/// between sessions, so the answer this closure bakes cannot go stale. The `crate::notify` module
/// docs record that dependency, because it is the reason the grain is the session.
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
/// true), and that is the right reading AT REST: the daemon's lifetime is tied to live panes,
/// not to session existence, so an idle empty session does not keep it up. It is the wrong
/// reading MID-CREATE, which is what [`BirthPin`] exists for — a session whose first pane is
/// still being spawned reads as empty here while a pane is genuinely on its way, and the claim
/// is the only thing that can tell the two apart. So the birth check comes FIRST, under the same
/// lock the pool collection takes. The unscoped-default totality the empty-session reading rests
/// on is noted on
/// [`SessionRegistry::default_session`](sprag_terminal::SessionRegistry::default_session).
fn no_live_panes(registry: &Arc<Mutex<SessionRegistry>>) -> bool {
    let pools: Vec<Arc<Mutex<Workspace>>> = {
        let reg = lock(registry);
        if reg.birth_in_flight() {
            return false;
        }
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

/// A claim on the daemon's life, held for as long as a session exists without the pane that is
/// meant to populate it — the "zero live panes ⇒ exit" policy's blind spot, closed.
///
/// The policy reads liveness off the pane pools, and a session between its create and its birth
/// spawn has none. An unrelated last pane dying in that gap therefore found a registry that looked
/// finished and ended the daemon under the client that had just asked for the session — which saw
/// its next request fail on `UnexpectedEof` and had no way to tell that from a daemon that was
/// never there. The gap is inherent to reading liveness off panes; the claim is the fact the panes
/// cannot carry.
///
/// **Take it under the lock the create holds.** [`taken`](Self::taken) demands the guard rather
/// than the `Mutex`, so "you already hold it" is a type-level requirement instead of a comment to
/// forget — a claim taken after the lock is released re-opens the same gap, only narrower.
///
/// **Dropping it NUDGES the reaper** when the caller supplies a `signal`, and that half is the easy
/// one to miss. While the claim stood, a death that found nothing live was answered "not yet", and
/// no further death is coming: the pane that would have signalled is precisely the one that failed
/// to be born. Without the nudge a daemon whose birth failed would sit forever with nothing running
/// in it — trading a daemon that exits too eagerly for one that never exits at all.
///
/// A caller passes `None` when zero panes is a legitimate resting state at that moment rather than
/// a daemon that has outlived its work — which is exactly the difference between a create (the
/// daemon was serving; the claim deferred a due exit) and a boot-time restore (the daemon has never
/// served; an empty one waits for a client, as a daemon with no snapshot does).
///
/// **Never drop it while holding the registry lock**: `Drop` takes that lock to release the claim.
pub struct BirthPin {
    /// The registry the claim is recorded in — an `Arc` because `Drop` must re-lock it after the
    /// create's own guard is long gone.
    registry: Arc<Mutex<SessionRegistry>>,
    /// The daemon's death-signal ([`spawn_reaper`]), fired on release so the reaper re-reads a
    /// liveness question that may have been answered "not yet" while the claim stood. `None` off a
    /// daemon (a GUI's in-process host, the tests), where nothing reaps and nothing needs waking.
    ///
    /// The same `Box` shape a pane's `on_exit` takes ([`pane_exit_hook`]), because it IS one more
    /// death-signal: the release is the moment a birth stops being pending, which is exactly the
    /// kind of event the reaper's one question is asked on.
    signal: Option<Box<dyn Fn() + Send>>,
}

impl BirthPin {
    /// Claim a birth on a registry the caller ALREADY holds the lock on, so the claim and the
    /// create it covers are one critical section.
    #[must_use]
    pub fn taken(
        registry: &Arc<Mutex<SessionRegistry>>,
        held: &mut MutexGuard<'_, SessionRegistry>,
        signal: Option<Box<dyn Fn() + Send>>,
    ) -> Self {
        held.pin_birth();
        Self {
            registry: Arc::clone(registry),
            signal,
        }
    }
}

impl Drop for BirthPin {
    fn drop(&mut self) {
        lock(&self.registry).release_birth();
        if let Some(signal) = &self.signal {
            signal();
        }
    }
}

/// The methods the headless host answers THROUGH THE GENERIC DISPATCH CORE: pure reads over the pane
/// scene (`scene/snapshot`, `scene/query`), the `scene/invoke` input + plugin channels, and the async
/// change-notification pair (`scene/revision` reads the current scene-version token;
/// `scene/waitFor {since}` blocks until it advances — the async form is intercepted before dispatch,
/// in the per-frame dispatch body). Anything else gets a JSON-RPC method-not-found error naming
/// this list.
///
/// **Four more are answered without appearing here**, because they are intercepted in
/// the per-frame dispatch body before the allowlist is ever consulted: `client/hello`, `client/attach` and
/// `client/size` (they carry a connection id, which no scene external sees) and
/// [`EVENTS_WAIT_METHOD`] (it parks its reply). The list is what a REFUSAL offers, so it names the
/// methods a caller can reach by the ordinary path; a client sending one of the four to a daemon too
/// old to intercept it is refused by this same list, which is exactly the loud answer that case
/// needs.
pub const SUPPORTED_METHODS: &[&str] = &[
    "scene/snapshot",
    "scene/query",
    "scene/invoke",
    "scene/revision",
    "scene/waitFor",
];

/// Whether a request for `method` needs the assembled scene's panes to carry their projected
/// cells — the one place that decides it, and the reason a read of a single integer no longer
/// costs a whole-screen walk per pane.
///
/// ## Why the METHOD is the honest discriminator, and not the path
///
/// Exactly one supported method can reach a `TextGrid` node:
///
/// * `scene/snapshot` reads the whole tree, and pinion's `snapshot` REQUIRES an empty scene
///   path (a tail is `UnsupportedPath`), so it can neither be narrowed to one pane nor served
///   without every pane's cells;
/// * `scene/query` and `scene/invoke` resolve to an `External` — a path that names anything
///   else answers `NoExternalAtPath`, so no grid is ever read through them;
/// * `scene/revision` and `scene/waitFor` read the revision token and never walk the scene.
///
/// So the answer follows from ONE field this layer already holds. Deciding it from the request
/// PATH instead would mean re-implementing pinion's resolution here, and a second spelling of
/// someone else's rule is how the two come to disagree — the same reason `handle_parsed`
/// refuses to guess what a malformed-scope request "probably meant".
///
/// ## The fallback direction is deliberate
///
/// An unrecognised method PROJECTS. The cheap answer is the one that can be silently wrong (a
/// method that does read a grid, served an empty one, reports blank cells rather than an
/// error), so it is opt-in per method and never the default. A method added to
/// [`SUPPORTED_METHODS`] later is merely slow until someone classifies it.
#[must_use]
pub fn pane_cells_for(method: &str) -> PaneCells {
    match method {
        "scene/snapshot" => PaneCells::Projected,
        "scene/query" | "scene/invoke" | "scene/revision" | "scene/waitFor" => PaneCells::Omitted,
        _ => PaneCells::Projected,
    }
}

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
            // need one — the reply is about the envelope, not about any session. Nor does it
            // need any pane's cells: a reply about the envelope reads no node at all.
            let scope = SessionScope::unscoped(state.registry());
            let mut scene = crate::workspace_scene(
                &scope,
                state.registry(),
                &state.runs,
                &state.channels,
                state.shared(),
                PaneCells::Omitted,
            );
            let revision = state.revision(scope.session());
            let mut ctx = DispatchContext::new(&mut scene, &state.previews, &revision);
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
    let cells = pane_cells_for(&request.method);
    handle_scoped_with_cells(state, scope, request, cells)
}

/// [`handle_scoped`] with the projection policy supplied rather than derived — the dispatch
/// body, split off so the policy is APPLIED at exactly one call site (above) while a test can
/// drive the same body both ways and compare the answers.
///
/// That comparison is the guard the split exists for: every method must answer identically
/// under either policy except `scene/snapshot`, which must differ. Without a seam that can
/// force the policy, the guard could only be written against whatever `pane_cells_for`
/// currently says — which would make it a restatement of the policy instead of a check on it.
#[must_use]
fn handle_scoped_with_cells(
    state: &HostState,
    scope: &SessionScope,
    request: Request,
    cells: PaneCells,
) -> Option<String> {
    let mut scene = crate::workspace_scene(
        scope,
        state.registry(),
        &state.runs,
        &state.channels,
        state.shared(),
        cells,
    );
    // The SCOPED session's token, which is what makes pinion's own OCC bump land in the right
    // place: it advances the revision it is handed after every mutating handler returns `Ok`, from
    // inside its dispatcher, so handing it this session's token is what attributes that bump — and
    // wakes that session's waits — without any call site here having to remember to.
    let revision = state.revision(scope.session());
    let mut ctx = DispatchContext::new(&mut scene, &state.previews, &revision);
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
            "error": { "code": INVALID_PARAMS, "message": message },
        })
        .to_string(),
    )
}

/// `events/waitFor` — park this connection's reply until a change it NAMED lands in the scoped
/// session's journal, or answer it now if one already has.
///
/// ## Why it is intercepted here rather than served as a slot or an action
///
/// * **It cannot block.** [`dispatch_frames`] is one thread for every client of the daemon, so a
///   handler that waited would freeze all of them. It parks its reply and returns — pinion's own
///   shape for `scene/waitFor`, reached through sprag's own registry because the condition is
///   sprag's ([`crate::notify::JournalChannel`]).
/// * **It cannot be an action.** An argument-bearing read served as an invoke is a
///   `MethodOcc::Mutate`, so it BUMPS ([`crate::wire::EVENTS_FIELD`] records the episode) — a wait
///   that bumped would wake every other parked client on the session, and waiting for events would
///   generate events.
/// * **It must be scoped by the machinery every other method uses.** The scope is resolved before
///   this point in [`dispatch_one`], which is why `session` cannot be accepted-and-ignored here the
///   way it once was for `scene/waitFor`.
///
/// A malformed `since` or `match` is a `-32602 Invalid params` carrying the sentence
/// [`EventFilter::from_wire`](crate::events::EventFilter::from_wire) wrote — a caller whose filter is
/// a mistake is told so immediately rather than parked forever on a predicate that can never hold.
fn handle_events_wait(
    state: &HostState,
    conn: ConnId,
    scope: &SessionScope,
    request: &Request,
    reply: RpcReply,
) {
    let params = request.params.as_ref();
    // ⚠ ESTABLISH THE SHAPE BEFORE PARKING, and a failing test is what put this line here.
    //
    // A journal records nothing on its FIRST observation — there is no predecessor to have changed
    // from ([`crate::events::SessionJournal`]). Every other method observes on its way out of
    // `dispatch_one`, so by the time a client's second call arrives the shape exists; this method
    // returns EARLY and so used to skip it. The consequence was a real hole and not a test artifact:
    // a client whose FIRST host call is a wait parks against an unestablished shape, and the next
    // structural change would then be swallowed as the establishing observation — the caller sleeping
    // through exactly the change it asked about.
    //
    // Observing here also states the semantics honestly. "Tell me what changes from now" requires
    // knowing what *now* is, so taking that reading is part of the question rather than a courtesy.
    state
        .channels()
        .observe(&lock(state.registry()), scope.session());
    let Some(since) = params
        .and_then(|params| params.get(SINCE_PARAM))
        .and_then(serde_json::Value::as_u64)
    else {
        if let Some(response) = lifecycle_invalid(
            request,
            format!(
                "params.{SINCE_PARAM} must be the revision you have already read (a whole number)"
            ),
        ) {
            reply.send(response);
        }
        return;
    };
    let filter = match crate::events::EventFilter::from_wire(
        params.and_then(|params| params.get(crate::events::EventFilter::WIRE_KEY)),
    ) {
        Ok(filter) => filter,
        Err(message) => {
            if let Some(response) = lifecycle_invalid(request, message) {
                reply.send(response);
            }
            return;
        }
    };
    state.channels().journal(scope.session()).park_or_answer(
        conn,
        since,
        filter,
        request.id.clone(),
        reply,
    );
}

/// A client lifecycle event moved `session`'s window: re-derive every tiled pane's size, then wake
/// the clients watching it.
///
/// The three ways a window moves without the arrangement changing are all here — a client REPORTED
/// a new area, one ATTACHED, one DETACHED — and each changes the set [`crate::window::arbitrate`]
/// folds. (The fourth way, the arrangement itself changing, is re-derived at the mux action
/// boundary instead, where the tree is written.)
///
/// Re-derive BEFORE the bump, deliberately: the bump wakes every parked `scene/waitFor`, and a
/// client woken by it should read a scene whose panes are already the size the new window gives
/// them, rather than the previous one and then a second wake later.
fn window_moved(state: &HostState, session: &str) {
    crate::window::retile(state.registry(), state.attachments(), session);
    state.channels().bump(session);
}

/// `client/hello` — register this connection under the client id its params carry, so the daemon
/// groups a client's several connections into one attached unit. Identity only: no session, so it
/// moves no attachment count and needs no scene bump.
/// The reply also carries THIS daemon's [`WIRE_PROTOCOL`], which is the half of the shape
/// agreement only a reply can make: a daemon OLDER than its client ignores the unknown protocol
/// param and answers every request happily, so a client that never heard back about the shape
/// would go on to misread the first slot that changed. A daemon outliving its clients is the
/// design, so that skew is the ORDINARY one after a rebuild, not an exotic case.
fn handle_hello(state: &HostState, conn: ConnId, request: &Request) -> Option<String> {
    match request.params.as_ref().and_then(|p| p.get(CLIENT_PARAM)) {
        Some(serde_json::Value::String(client)) => {
            lock(state.attachments()).hello(conn, client.clone());
            let id = request.id.clone()?;
            Some(
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": { PROTOCOL_FIELD: WIRE_PROTOCOL },
                })
                .to_string(),
            )
        }
        _ => lifecycle_invalid(request, format!("params.{CLIENT_PARAM} must be a string")),
    }
}

/// Refuse a request written against a wire shape this build does not speak — the door every
/// socket client passes through, checked BEFORE the scope and before any handler.
///
/// `None` admits the request. `Some(reply)` is the refusal, and refusing here rather than letting
/// the request through is the point: a skewed pair does not fail cleanly on its own. R264
/// flattened the layout wire, and a `sprag-tui` left over from before it created a session, then
/// died decoding the ninth reply with a serde message about an integer — no part of which named a
/// version, a daemon, or an action.
///
/// **This is the half a CLIENT cannot perform.** An old client contains no check to run, so the
/// only end that can catch it is the new one; a design that checks only client-side (herdr's, at
/// `9a4ce5e1`) is silent in exactly this direction.
///
/// A request with no `protocol` at all is a client from before the handshake, reported as such
/// rather than as a malformed request, because that is what it is.
fn protocol_refused(request: &Request) -> Option<String> {
    let spoken = request
        .params
        .as_ref()
        .and_then(|params| params.get(PROTOCOL_PARAM))
        .and_then(serde_json::Value::as_u64);
    if spoken == Some(u64::from(WIRE_PROTOCOL)) {
        return None;
    }
    let client = spoken.map_or_else(
        || "none (a client older than this check)".to_owned(),
        |value| value.to_string(),
    );
    let id = request.id.clone()?;
    Some(
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": INVALID_PARAMS,
                "message": format!(
                    "this daemon speaks wire protocol {WIRE_PROTOCOL} and the client speaks \
                     {client}; they cannot understand each other. Rebuild the client, or restart \
                     this daemon to the client's build — `sprag kill-server` (sessions are \
                     restored from the durability snapshot)",
                ),
            },
        })
        .to_string(),
    )
}

/// `client/attach` — attach (or switch — tmux `switch-client`) this connection's client to the
/// already-validated `scope` session. A first attach or a switch moves a per-session count, so it
/// bumps the scene to wake parked `scene/waitFor`s to re-read the badge; an idempotent re-send
/// does not. An attach with no prior `client/hello` is a protocol error, refused `-32602`.
/// Record the client's reported cell area ([`CLIENT_SIZE_METHOD`]) and, when it moved, announce it
/// to the session that client is watching.
///
/// The bump is what makes a window change on ONE client reach the others: the arbitrated window is
/// derived from every attached client's area, so a report that moves it must wake the long-polls
/// that will re-read it.
///
/// **It is deliberately kept although no test isolates it, and that was MEASURED rather than
/// assumed.** Removing it leaves the whole `window-size` gate green, including the case written to
/// catch exactly this (`a_client_that_reported_nothing_re_tiles_when_the_window_moves`). The reason
/// is an indirection: a report that moves the window makes the REPORTING client re-tile and resize
/// the panes, the reflow marks those panes dirty, and the dirty path bumps the scene — so the other
/// clients wake anyway. That chain is real but it is nobody's stated contract for this, it only
/// holds while a window change always resizes a pane, and it makes the wake arrive a reflow later
/// than the fact that caused it. Announcing the fact itself costs one bump on a window change and
/// does not depend on any of that.
fn handle_size(state: &HostState, conn: ConnId, request: &Request) -> Option<String> {
    let dim = |key: &str| {
        request
            .params
            .as_ref()
            .and_then(|params| params.get(key))
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u16::try_from(value).ok())
            .filter(|value| *value > 0)
    };
    // Both dimensions or neither: half a size is not a size, and a client that sent one number has
    // a bug this refusal names rather than a window whose height came from somewhere else.
    let (Some(cols), Some(rows)) = (dim(COLS_PARAM), dim(ROWS_PARAM)) else {
        return lifecycle_invalid(
            request,
            format!("{CLIENT_SIZE_METHOD} requires positive {COLS_PARAM} and {ROWS_PARAM}"),
        );
    };
    let mut attachments = lock(state.attachments());
    match attachments.size(conn, ClientSize { cols, rows }) {
        SizeOutcome::NoClient => lifecycle_invalid(
            request,
            format!("{CLIENT_SIZE_METHOD} requires {CLIENT_HELLO_METHOD} first"),
        ),
        SizeOutcome::Unchanged => lifecycle_ok(request),
        SizeOutcome::Changed => {
            // A client that reports before attaching moves no session's window yet; its attach
            // announces on its own.
            let session = attachments.session_of(conn).map(str::to_owned);
            drop(attachments);
            if let Some(session) = session {
                window_moved(state, &session);
            }
            lifecycle_ok(request)
        }
    }
}

fn handle_attach(
    state: &HostState,
    conn: ConnId,
    scope: &SessionScope,
    request: &Request,
) -> Option<String> {
    // Bound and RELEASED before the arms run. A `lock(..)` written into the `match` scrutinee lives
    // for the whole match, and an arm below re-derives the window — which reads this same map. It
    // deadlocked the daemon's dispatch thread outright, and that is the honest failure mode: a
    // re-entrant lock is not a race that shows up occasionally, it is every attach, forever.
    let outcome = lock(state.attachments()).attach(conn, scope.session().to_owned());
    match outcome {
        AttachOutcome::NoClient => lifecycle_invalid(
            request,
            format!("{CLIENT_ATTACH_METHOD} requires {CLIENT_HELLO_METHOD} first"),
        ),
        AttachOutcome::Changed { previous } => {
            tracing::info!(
                target: "sprag_host::attach",
                session = %scope.session(),
                "client attached"
            );
            // BOTH sides of a switch: the session gained a viewer, and (on a switch rather than a
            // first attach) the one it left lost one. Each announces on its own channel, so a
            // client watching the session being left learns its badge fell — which the single
            // registry-wide token used to deliver as a side effect of waking everybody.
            window_moved(state, scope.session());
            // A SWITCH left a session too, and that session's window lost a reporter — so it is a
            // window that moved for the same reason, not merely a badge that fell.
            if let Some(left) = previous.filter(|left| left != scope.session()) {
                window_moved(state, &left);
            }
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
                // Forget anything this connection was waiting for. THE lifecycle answer for a
                // filtered wait: it carries no deadline, so a client that gives up (or crashes)
                // leaves an entry whose filter may never match again, and this is the hook that
                // fires however the client goes away. Done before the attachment release below
                // because it cannot fail and needs nothing from it.
                let waits = state.channels().release(conn);
                if waits > 0 {
                    tracing::debug!(
                        target: "sprag_host::notify",
                        conn = conn.get(),
                        waits,
                        "released the change waits of a closed connection"
                    );
                }
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
                    window_moved(state, &session);
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
/// The scope is also HONORED, not merely checked. It was checked-and-ignored for as long as the
/// daemon had one registry-wide revision: a client scoped to `work` woke whenever ANY session
/// moved, re-read its own, found nothing, and re-parked. Safe (the wake was a hint and the re-read
/// was exact) but it made the cost of a change scale with the number of ATTACHED clients rather
/// than with the number that could care. Each session now owns its token and its parked replies
/// ([`ChannelRegistry`]), so the wait sleeps through every other session's traffic — a strictly
/// tighter contract than the "at least when your session changes" clients were built on.
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
            // THE DOOR. Every socket client's every request arrives here (the stdin transport
            // funnels through this same frame channel), so one check covers them all — and it runs
            // before the scope and before any handler, because a request written against a shape
            // this build does not speak must not act, not even partly. See [`protocol_refused`].
            if let Some(response) = protocol_refused(&parsed) {
                reply.send(response);
                return;
            }
            // `client/hello` carries identity, not a session, so it is handled before scope
            // resolution: it registers this connection's client id (the group key a client's
            // several connections share) and returns.
            if parsed.method.as_str() == CLIENT_HELLO_METHOD {
                if let Some(response) = handle_hello(state, conn, &parsed) {
                    reply.send(response);
                }
                return;
            }
            // `client/size` reports the area its client can give a window. Handled beside the hello
            // and BEFORE scope resolution for the same reason: it carries no session. Which session
            // it moves is the one that client is attached to, which only the registry knows.
            if parsed.method.as_str() == CLIENT_SIZE_METHOD {
                if let Some(response) = handle_size(state, conn, &parsed) {
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
            // `events/waitFor` parks on the JOURNAL rather than on the revision, so output cannot
            // wake it. Beside pinion's intercept and not inside `handle_scoped`, for the two reasons
            // [`handle_events_wait`] gives: it parks its reply, and it must be scoped by the same
            // machinery every other method uses.
            if parsed.method.as_str() == EVENTS_WAIT_METHOD {
                handle_events_wait(state, conn, &scope, &parsed, reply);
                return;
            }
            // Parked against the SCOPED session's channel — the half `scene/waitFor` used to check
            // and then ignore. The scope was resolved above, so the wait sleeps on the session it
            // named and no other session's traffic can reach it.
            let revision = state.revision(scope.session());
            let waiters = state.waiters(scope.session());
            match try_async_wait_for(&parsed, &revision, &waiters, reply) {
                // Parked (or answered immediately) by the registry — nothing more to do.
                ControlFlow::Break(()) => {}
                // Not an async waitFor: dispatch the ALREADY-parsed request (no re-parse).
                ControlFlow::Continue(reply) => {
                    let response = handle_scoped(state, &scope, parsed);
                    // THE derive site. Every mutating wire method passes through this one arm, so a
                    // method added later records what it changed without a line of its own — the
                    // structural property `notify`'s wake already has, extended to the wake's
                    // payload. Run BEFORE the reply, so a caller that reads the journal the instant
                    // its own call returns is never told its change has not happened yet.
                    //
                    // pinion has already bumped the scoped revision by now (it bumps after a
                    // mutating handler returns `Ok`, inside its own dispatcher), so the records land
                    // at the number a client woken by that bump will read with.
                    state
                        .channels()
                        .observe(&lock(state.registry()), scope.session());
                    if let Some(response) = response {
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

    /// The name of the session a boot pane lands in and an unscoped request resolves to.
    const BOOT: &str = "0";

    /// Host state with one initial pane running `script`, wired the way a wire
    /// server boots: the pane's `on_dirty` bumps its SESSION's [`SceneRevision`], so
    /// its output wakes the parked async `scene/waitFor` replies on that session (the
    /// change-notification path R115a serves).
    fn host_with(script: &str, cols: u16, rows: u16) -> HostState {
        let channels = Arc::new(ChannelRegistry::default());
        let host = Host::new((cols, rows));
        // The SAME boot recipe prod uses (sprag-term.rs) — the shared `bump_on_dirty`
        // helper over the boot session's own token, so the test exercises the real
        // "pane output bumps ITS SESSION's revision" wire.
        host.spawn(
            sh(script),
            "sh".to_string(),
            cols,
            rows,
            Some(bump_on_dirty(&channels.revision(BOOT))),
            None,
        )
        .expect("spawn pane");
        HostState::new(host, channels, None)
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

    /// A claimed birth outranks an empty registry: the daemon does NOT conclude it is finished
    /// while a session it just made is still waiting for its shell.
    ///
    /// This is the race the policy could not see. Liveness is read off the pane pools, and a
    /// session between its create and its birth spawn has none — so an unrelated last pane dying
    /// in that gap read as "everything is over" and ended the daemon under the client that had
    /// just asked for the session. Driven against a registry whose sole pane HAS exited, which is
    /// exactly the state the race presents.
    ///
    /// REVERT-PROOF: drop the `birth_in_flight` early return from `no_live_panes` and the first
    /// assertion fails — the daemon calls itself finished with a birth still pending.
    #[test]
    fn a_pending_birth_keeps_the_daemon_alive() {
        let host = Host::new((40, 6));
        let id = host
            .spawn(sh("exec true"), "true".into(), 40, 6, None, None)
            .expect("spawn true");
        wait_for_eof(&host, id);
        assert!(
            no_live_panes(host.registry()),
            "precondition: with nothing claimed, the dead pane leaves nothing live",
        );

        let pin = {
            let mut held = lock(host.registry());
            BirthPin::taken(host.registry(), &mut held, None)
        };
        assert!(
            !no_live_panes(host.registry()),
            "a pane is on its way, so the daemon is not finished",
        );

        drop(pin);
        assert!(
            no_live_panes(host.registry()),
            "and the claim releases — it holds the daemon open, it does not pin it open",
        );
    }

    /// Releasing a claim NUDGES the reaper, so a birth that FAILED still lets an idle daemon go.
    ///
    /// The half that is easy to miss, and the one that would turn this fix into a worse bug: while
    /// the claim stands, a death that finds nothing live is answered "not yet" — and no further
    /// death is coming, because the pane that would have signalled is the one that failed to be
    /// born. Without the nudge the daemon would sit forever with nothing running in it.
    ///
    /// REVERT-PROOF: drop the `signal()` call from `BirthPin::drop` and this times out at zero
    /// fires — the daemon never learns the question changed.
    #[test]
    fn releasing_a_claim_re_asks_the_reaper() {
        use std::sync::atomic::Ordering;

        let host = Host::new((40, 6));
        let (signal, fired) = recording_reaper(host.registry());
        // A pane that dies WHILE the claim stands: the reaper scans, finds the claim, and holds off.
        let pin = {
            let mut held = lock(host.registry());
            BirthPin::taken(host.registry(), &mut held, Some(pane_exit_hook(&signal)))
        };
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
            "the claim stands, so the death is answered `not yet`",
        );

        // The birth fails (nothing else spawns) and the claim falls. Nothing else will ever signal,
        // so the release must — or this daemon is immortal.
        drop(pin);
        let start = Instant::now();
        while fired.load(Ordering::SeqCst) == 0 && start.elapsed() < Duration::from_secs(5) {
            sleep(Duration::from_millis(20));
        }
        assert_eq!(
            fired.load(Ordering::SeqCst),
            1,
            "releasing the claim re-asks the question, and this time the answer is yes",
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

    /// The projection policy is a decision PER METHOD, and its fallback projects.
    ///
    /// The length assertion is the exhaustiveness check a `&[&str]` table cannot give
    /// structurally: the arms below name every supported method, so adding one to
    /// [`SUPPORTED_METHODS`] without deciding what it reads fails here rather than silently
    /// inheriting the fallback.
    #[test]
    fn the_projection_policy_is_decided_per_method_and_fails_safe() {
        assert_eq!(
            pane_cells_for("scene/snapshot"),
            PaneCells::Projected,
            "the one method that reads a TextGrid — and it reads every pane's",
        );
        for method in [
            "scene/query",
            "scene/invoke",
            "scene/revision",
            "scene/waitFor",
        ] {
            assert_eq!(
                pane_cells_for(method),
                PaneCells::Omitted,
                "{method} resolves to an External (or to no node at all), never to a grid",
            );
        }
        assert_eq!(
            SUPPORTED_METHODS.len(),
            5,
            "a newly supported method needs its own projection decision above",
        );
        assert_eq!(
            pane_cells_for("scene/somethingLater"),
            PaneCells::Projected,
            "an unclassified method is merely slow; it must never be served empty cells",
        );
    }

    /// Dispatch one request through the SAME dispatch body twice — cells projected, then
    /// omitted — and return both responses.
    fn serve_both_policies(
        state: &HostState,
        request_json: &str,
    ) -> (serde_json::Value, serde_json::Value) {
        let once = |cells| {
            let request = parse_request(request_json).expect("a well-formed request");
            let scope =
                SessionScope::resolve(state.registry(), &request).expect("a resolvable scope");
            let response =
                handle_scoped_with_cells(state, &scope, request, cells).expect("a response");
            serde_json::from_str::<serde_json::Value>(response.trim())
                .expect("valid json-rpc response")
        };
        (once(PaneCells::Projected), once(PaneCells::Omitted))
    }

    /// THE guard on the projection gate: omitting the panes' cells changes what
    /// `scene/snapshot` reports, and changes NOTHING else.
    ///
    /// Both halves are load-bearing. The equality half is the safety claim — if pinion ever
    /// let a `scene/query` reach a `TextGrid`, an omitted grid would answer blank cells
    /// instead of erroring, and this fails at the pin bump rather than in production. The
    /// inequality half is the non-vacuity: without it the guard would still pass if
    /// `PaneCells` did nothing whatsoever.
    ///
    /// The pane's child has EXITED before any comparison, so its screen, its revision and its
    /// scroll facts are frozen — the two dispatches of one request cannot differ because live
    /// output landed between them. That is also why the mutating `scene/invoke` case is
    /// comparable at all: on an EOF'd PTY both runs take the identical branch.
    #[test]
    fn omitting_the_panes_cells_changes_only_what_a_snapshot_reports() {
        let state = host_with("printf hi", 20, 4);
        assert!(
            wait_for_snapshot(&state, "hi"),
            "pane 0 printed before the comparison",
        );
        wait_for_pane0_eof(&state);

        for request in [
            // The client's steady-state hot path: a pane's cells, read through its External.
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/query","params":{"path":"/pane_0/sprag_input/external/cells.0"}}"#,
            // Pane CONTENT through the External — the read most likely to be confused with a grid read.
            r#"{"jsonrpc":"2.0","id":2,"method":"scene/query","params":{"path":"/pane_0/sprag_input/external/full_text"}}"#,
            // The mux surface, which reaches past the pool.
            r#"{"jsonrpc":"2.0","id":3,"method":"scene/query","params":{"path":"/sprag_mux/external/panes"}}"#,
            // The read that used to cost a whole pane set to answer with one integer.
            r#"{"jsonrpc":"2.0","id":4,"method":"scene/revision","params":{}}"#,
        ] {
            let (with, without) = serve_both_policies(&state, request);
            assert!(
                with.get("error").is_none(),
                "the guard is vacuous unless the path resolves: {with}",
            );
            assert_eq!(
                with, without,
                "an omitted grid changed the answer to {request}"
            );
        }

        // The mutating method, kept separate because its outcome is a rejection on a dead PTY
        // rather than a result — identical either way, which is the claim, but no evidence
        // that any path resolved.
        let (with, without) = serve_both_policies(
            &state,
            r#"{"jsonrpc":"2.0","id":5,"method":"scene/invoke","params":{"path":"/pane_0/sprag_input/external/key","args":{"key":"a"}}}"#,
        );
        assert_eq!(
            with, without,
            "an omitted grid changed a scene/invoke outcome"
        );

        // ...and the one method that CAN read a grid must see the difference.
        let (with, without) = serve_both_policies(
            &state,
            r#"{"jsonrpc":"2.0","id":6,"method":"scene/snapshot","params":{"path":""}}"#,
        );
        assert_ne!(
            with, without,
            "omitting the cells must change what a snapshot reports",
        );
        assert!(
            snapshot_grid_text(&with["result"]).contains("hi"),
            "the projected snapshot's GRID carries the pane's text",
        );
        assert_eq!(
            snapshot_grid_text(&without["result"]),
            "",
            "an omitted grid contributes no cells to the snapshot",
        );
    }

    /// Every `grid_rows` text a snapshot result carries, concatenated — the CELLS half of a
    /// snapshot, located STRUCTURALLY rather than by searching the response for a string.
    ///
    /// The distinction is not pedantry: a snapshot serializes each `External`'s introspect
    /// fields too, and a pane's include `full_text`, so its screen text appears in the JSON a
    /// second time by a path that has nothing to do with the grid. A `result.contains("hi")`
    /// check therefore passes even with the cells removed entirely — which is exactly the
    /// false negative this walk exists to avoid.
    fn snapshot_grid_text(result: &serde_json::Value) -> String {
        let mut text = String::new();
        fn walk(value: &serde_json::Value, out: &mut String) {
            match value {
                serde_json::Value::Object(map) => {
                    if let Some(serde_json::Value::Array(rows)) = map.get("grid_rows") {
                        for row in rows {
                            if let Some(line) = row.get("text").and_then(serde_json::Value::as_str)
                            {
                                out.push_str(line);
                            }
                        }
                    }
                    for nested in map.values() {
                        walk(nested, out);
                    }
                }
                serde_json::Value::Array(items) => {
                    for item in items {
                        walk(item, out);
                    }
                }
                _ => {}
            }
        }
        walk(result, &mut text);
        text
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
        // Read out of the GRID rows specifically. A `result.contains("hi")` check — which this
        // test used to make — is satisfied by the pane External's `full_text` introspect field
        // alone, so it passed whether or not the cells were there at all.
        assert!(
            snapshot_grid_text(&value["result"]).contains("hi"),
            "expected 'hi' in the projected grid, got: {}",
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
    fn clipboard_write_reaches_the_slot_and_the_on_demand_fetch() {
        // A child sets the system clipboard via OSC 52 (`aGk=` is base64("hi")). The CHEAP write
        // seq rides the pane list; the actual payload (targets + text) is fetched ON DEMAND from
        // the pane's clipboard_write slot — the split that keeps a large paste off the per-poll path.
        let state = host_with(r"printf '\033]52;c;aGk=\007'", 20, 4);
        wait_for_pane0_eof(&state);
        let panes = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/query","params":{"path":"/sprag_mux/external/panes"}}"#,
        );
        assert_eq!(
            panes["result"][0]["clipboard_write_seq"].as_u64(),
            Some(1),
            "the cheap write seq rides the pane list: {panes}"
        );
        let write = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":2,"method":"scene/query","params":{"path":"/pane_0/sprag_input/external/clipboard_write"}}"#,
        );
        let w = &write["result"];
        assert_eq!(
            w["text"], "hi",
            "the base64-decoded write payload over RPC: {write}"
        );
        assert_eq!(w["targets"]["clipboard"], true, "{write}");
        assert_eq!(w["targets"]["primary"], false, "{write}");
        assert_eq!(w["seq"].as_u64(), Some(1), "{write}");
    }

    #[test]
    fn clipboard_read_query_reaches_the_panes_slot() {
        // A child asks to READ the clipboard (`OSC 52 ; c ; ?`). The tiny query (selection + seq)
        // rides the pane list INLINE — a display client answers it (subject to policy).
        let state = host_with(r"printf '\033]52;p;?\007'", 20, 4);
        wait_for_pane0_eof(&state);
        let panes = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/query","params":{"path":"/sprag_mux/external/panes"}}"#,
        );
        let q = &panes["result"][0]["clipboard_query"];
        assert_eq!(q["sel"], "p", "the queried selection (primary): {panes}");
        assert_eq!(q["seq"].as_u64(), Some(1), "{panes}");
        // A read is NOT a write — no write seq appears.
        assert!(
            panes["result"][0].get("clipboard_write_seq").is_none(),
            "a read must not report a write seq: {panes}"
        );
    }

    /// Poll (bounded) the pane 0 clipboard_write slot until its text equals `expected` — used to
    /// observe an OSC 52 reply that a `cat` child echoes back and the emulator re-parses.
    fn wait_for_clipboard_write_text(state: &HostState, expected: &str) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            let write = serve_one(
                state,
                r#"{"jsonrpc":"2.0","id":9,"method":"scene/query","params":{"path":"/pane_0/sprag_input/external/clipboard_write"}}"#,
            );
            if write["result"]["text"] == expected {
                return true;
            }
            sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn clipboard_answer_writes_the_reply_exactly_once() {
        // A live `cat` echoes whatever is written to its PTY. Answering a read query writes an
        // OSC 52 reply to the PTY; cat echoes it and the emulator re-parses it as a clipboard
        // WRITE — proving the reply actually reached the child. The host admits EXACTLY ONE reply
        // per query seq (the multi-client arbitration tmux has no analog for — it does no reads).
        //
        // `stty raw -echo` puts the PTY in raw mode: cat reads each byte immediately (the reply has
        // no newline, so a cooked line-buffer would never release it) and echoes it VERBATIM (a
        // cooked tty would render the ESC as a visible `^[`, which the emulator would not re-parse).
        let state = host_with("stty raw -echo 2>/dev/null; exec cat", 20, 4);
        let answer = |seq: u64, text: &str| {
            let req = format!(
                r#"{{"jsonrpc":"2.0","id":1,"method":"scene/invoke","params":{{"path":"/pane_0/sprag_input/external/clipboard_answer","args":{{"seq":{seq},"sel":"c","text":"{text}"}}}}}}"#
            );
            serve_one(&state, &req)
        };
        // The FIRST answer for query seq 1 wins and writes the reply.
        assert_eq!(
            answer(1, "rt")["result"]["wrote"],
            true,
            "first answer writes"
        );
        // It reached cat's PTY, echoed, and round-tripped through the real parser back to a write.
        assert!(
            wait_for_clipboard_write_text(&state, "rt"),
            "the OSC 52 reply never round-tripped through the child"
        );
        // A SECOND answer for the SAME seq is DROPPED — exactly-once across clients.
        assert_eq!(
            answer(1, "again")["result"]["wrote"],
            false,
            "a duplicate answer for an already-answered query must be dropped"
        );
        // A NEWER query seq is admitted again.
        assert_eq!(
            answer(2, "rt2")["result"]["wrote"],
            true,
            "a newer query seq is admitted"
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
    ///
    /// It declares the wire's SHAPE the way a client does, at this one seam, because these tests
    /// STAND IN for a wire client and `dispatch_one`'s door refuses anything that does not
    /// ([`protocol_refused`]). Adding it per test would have been the same fact spelled thirty
    /// times, and the first one written without it would have been read as a product bug.
    fn dispatch_recording(state: &HostState, request: &str, sink: &Arc<Mutex<Vec<String>>>) {
        dispatch_one(
            state,
            RpcFrame::new(
                ConnId::allocate(),
                declaring_the_protocol(request),
                recording_reply(sink),
            ),
        );
    }

    /// `request` with [`PROTOCOL_PARAM`] merged into its params — what `HostConn::call` does for
    /// every real client, done here so a test's literal request stays readable.
    ///
    /// A request with no params at all gets an object holding only the declaration, which is the
    /// same rule the transport follows: absent params and an empty object mean one thing to every
    /// handler, but a missing SHAPE means "a client from before the agreement".
    fn declaring_the_protocol(request: &str) -> String {
        let mut value: serde_json::Value =
            serde_json::from_str(request).expect("a test's request is JSON");
        let params = value
            .as_object_mut()
            .expect("a request is an object")
            .entry("params")
            .or_insert_with(|| serde_json::json!({}));
        if let Some(params) = params.as_object_mut() {
            params.insert(PROTOCOL_PARAM.to_owned(), serde_json::json!(WIRE_PROTOCOL));
        }
        value.to_string()
    }

    /// One `scene/invoke` through the real per-frame dispatch body, discarding the reply.
    fn invoke_recording(state: &HostState, action: &str, args: serde_json::Value) {
        let sink = Arc::new(Mutex::new(Vec::new()));
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "scene/invoke",
            "params": { "path": crate::wire::mux_action_path(action), "args": args },
        });
        dispatch_recording(state, &request.to_string(), &sink);
    }

    /// Everything `BOOT`'s journal has recorded above `cursor`.
    fn journal_since(state: &HostState, cursor: u64) -> Vec<crate::events::Event> {
        state.channels().journal(BOOT).since(cursor).events
    }

    /// One `events/waitFor` frame from `conn`, through the real per-frame dispatch body.
    fn wait_recording(
        state: &HostState,
        conn: ConnId,
        filter: serde_json::Value,
        sink: &Arc<Mutex<Vec<String>>>,
    ) {
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": EVENTS_WAIT_METHOD,
            "params": { SINCE_PARAM: state.revision(BOOT).current(), "match": filter },
        });
        dispatch_one(
            state,
            RpcFrame::new(
                conn,
                declaring_the_protocol(&request.to_string()),
                recording_reply(sink),
            ),
        );
    }

    #[test]
    fn a_filtered_wait_parks_through_the_real_dispatch_and_the_change_answers_it() {
        // The method is intercepted BEFORE the generic core and before the allowlist, like the three
        // client-lifecycle methods — so what this pins is that `dispatch_one` routes it at all, and
        // that the scope it parks against is the one the frame resolved.
        let state = host_with("cat", 20, 4);
        let sink = Arc::new(Mutex::new(Vec::new()));

        wait_recording(
            &state,
            ConnId::allocate(),
            serde_json::json!([{ "kind": "pane_created" }]),
            &sink,
        );
        assert!(
            sink.lock().unwrap().is_empty(),
            "nothing matching has happened, so the reply is PARKED, not sent",
        );
        assert_eq!(state.channels().journal(BOOT).parked_count(), 1);

        invoke_recording(&state, crate::wire::SPAWN_ACTION, serde_json::json!({}));

        let replies = sink.lock().unwrap();
        let reply: serde_json::Value =
            serde_json::from_str(replies.first().expect("the spawn answered the wait"))
                .expect("valid JSON-RPC");
        assert_eq!(
            reply["result"]["events"],
            serde_json::json!([{ "type": "pane_created", "pane": 1 }]),
            "answered with the change it named, in the vocabulary the slot serves: {reply}",
        );
        assert_eq!(
            state.channels().journal(BOOT).parked_count(),
            0,
            "and it is no longer parked — a reply fires at most once",
        );
    }

    #[test]
    fn the_dispatch_loop_releases_a_closed_connections_waits() {
        // ⚠ THIS TEST EXISTS BECAUSE A REVERT-PROOF PASSED GREEN. Deleting
        // `state.channels().release(conn)` from `dispatch_frames`'s disconnect arm left the WHOLE
        // suite green — 504 unit tests and 40 wire tests — because `notify`'s own unit test calls
        // `release` DIRECTLY. That pinned the method and said nothing about whether the loop calls
        // it, which is R291's finding repeating one round later on a different tracker.
        //
        // So this drives `dispatch_frames` itself: a park frame and a disconnect down the same
        // channel, in that order, which is the ordering the transport guarantees.
        let state = host_with("cat", 20, 4);
        let sink = Arc::new(Mutex::new(Vec::new()));
        let conn = ConnId::allocate();
        let (tx, rx) = std::sync::mpsc::channel();

        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": EVENTS_WAIT_METHOD,
            "params": { SINCE_PARAM: 0, "match": [{ "kind": "pane_created" }] },
        });
        tx.send(IngressEvent::Frame(RpcFrame::new(
            conn,
            declaring_the_protocol(&request.to_string()),
            recording_reply(&sink),
        )))
        .expect("queue the park");
        tx.send(IngressEvent::Disconnect(conn))
            .expect("queue the close");
        // Dropping the sender is what ends the loop, so this runs to completion on this thread.
        drop(tx);
        dispatch_frames(&state, rx);

        assert_eq!(
            state.channels().journal(BOOT).parked_count(),
            0,
            "the loop released the wait of a connection that closed — without this, a filter that \
             never matches retains its entry for the daemon's remaining life",
        );
        assert!(
            sink.lock().unwrap().is_empty(),
            "and a gone connection is not written to: the release drops, it does not answer",
        );
    }

    /// **The measurement `JOURNAL_CAPACITY` is DERIVED from, pinned so the derivation cannot rot.**
    ///
    /// The ring's size is chosen against how many records a workspace-scale burst produces, which
    /// is a function of records-per-OPERATION. A future change that made every mutation emit five
    /// records would not fail any test above — every one of them asserts that a change is reported,
    /// not how loudly — and would quietly cut the ring's reach to a fifth. So the ratio is asserted.
    ///
    /// The control matters here: `invoke_recording` discards the reply, so a REFUSED action and one
    /// that legitimately records nothing look identical. The first version of this measurement
    /// passed `{"pane": 1}` to `close`, whose argument is `id`, and read the refusal as "close emits
    /// no record". Each case below therefore checks the WORLD moved, not only the log.
    #[test]
    fn the_records_per_operation_ratio_the_ring_is_sized_against() {
        let state = host_with("sleep 30", 80, 24);
        state.channels().observe(&lock(state.registry()), BOOT);
        let mut mark = state.revision(BOOT).current();
        let recorded = |state: &HostState, mark: &mut u64| {
            let events = journal_since(state, *mark);
            *mark = state.revision(BOOT).current();
            events
        };

        invoke_recording(&state, crate::wire::SPAWN_ACTION, serde_json::json!({}));
        assert_eq!(
            recorded(&state, &mut mark),
            vec![crate::events::Event::PaneCreated(1)],
            "a spawn is ONE record: it appends to the pool and the tree reconciles lazily, so the \
             arrangement counter has not moved yet",
        );

        invoke_recording(
            &state,
            crate::wire::NEW_WINDOW_ACTION,
            serde_json::json!({}),
        );
        assert_eq!(
            recorded(&state, &mut mark).len(),
            1,
            "a new window is ONE record — its birth pane is not reported separately, or a reader \
             would apply the same fact twice in an order this cannot promise",
        );

        invoke_recording(
            &state,
            crate::wire::SELECT_WINDOW_ACTION,
            serde_json::json!({ "window": "0" }),
        );
        assert_eq!(
            recorded(&state, &mut mark).len(),
            1,
            "a select is ONE record"
        );

        // The FIRST split of this window is TWO, and the reason is a rule this log states rather
        // than a special case: nothing had reconciled window "0" yet, so it had no active pane, and
        // a window gaining its first one is the window ESTABLISHING itself rather than the user
        // moving (see `SessionShape::diff`). The split's own move is that establishment, so it is
        // not reported — and the pane it created is, which is what a reader re-reads anyway.
        invoke_recording(
            &state,
            crate::wire::SPLIT_ACTION,
            serde_json::json!({ "pane": 0, "dir": "horizontal" }),
        );
        assert_eq!(
            recorded(&state, &mut mark).len(),
            2,
            "the first split of a window is TWO — the pane and the arrangement — because the \
             active pane it sets is that window's first",
        );

        invoke_recording(
            &state,
            crate::wire::SELECT_PANE_ACTION,
            serde_json::json!({ "pane": 0 }),
        );
        assert_eq!(
            recorded(&state, &mut mark),
            vec![crate::events::Event::PaneSelected(0)],
            "a select-pane is ONE record",
        );

        invoke_recording(
            &state,
            crate::wire::SPLIT_ACTION,
            serde_json::json!({ "pane": 0, "dir": "vertical" }),
        );
        assert_eq!(
            recorded(&state, &mut mark).len(),
            3,
            "and a split of a window that HAS an active pane is THREE — the pane, the \
             arrangement, and the active pane it moves onto — the worst case the ring is sized \
             against",
        );

        let before = lock(&state.host.workspace()).panes().len();
        invoke_recording(
            &state,
            crate::wire::CLOSE_ACTION,
            serde_json::json!({ "id": 1 }),
        );
        assert_eq!(
            lock(&state.host.workspace()).panes().len(),
            before - 1,
            "THE CONTROL: the close actually happened, so an empty log below would be a defect \
             rather than a refusal this probe could not see",
        );
        assert_eq!(
            recorded(&state, &mut mark),
            vec![crate::events::Event::PaneClosed(1)],
            "a close is ONE record",
        );
    }

    /// The BURST the ring is sized against: building the widest workspace this project measures.
    ///
    /// `REGISTRY_SIZES` in `sprag-latency` tops out at 64 panes, so a 64-pane build is the largest
    /// reconstruction any cost row here contemplates. By SPLIT — the three-record shape, since a
    /// split also moves the active pane — it is 188 records, which is what makes
    /// `JOURNAL_CAPACITY` a derivation rather than a round number.
    #[test]
    fn a_workspace_scale_burst_fits_the_ring_with_room() {
        let state = host_with("sleep 30", 80, 24);
        state.channels().observe(&lock(state.registry()), BOOT);
        for _ in 0..63 {
            invoke_recording(
                &state,
                crate::wire::SPLIT_ACTION,
                serde_json::json!({ "pane": 0, "dir": "horizontal" }),
            );
        }
        assert_eq!(
            lock(&state.host.workspace()).panes().len(),
            64,
            "THE CONTROL: 64 panes were actually built",
        );
        let records = journal_since(&state, 0).len();
        assert_eq!(
            records, 188,
            "three per split, and this is the number quoted"
        );
        assert!(
            records <= crate::events::JOURNAL_CAPACITY,
            "the ring holds a whole workspace-scale build ({records} vs {}) — the headroom the \
             capacity is chosen for",
            crate::events::JOURNAL_CAPACITY,
        );
    }

    /// **THE claim of the derive site.** `spawn` does not mention this log, this module, or an
    /// event of any kind — and a pane appearing is still reported.
    ///
    /// That is the whole reason the record is DERIVED at the one arm every mutating method passes
    /// through rather than EMITTED by each handler. An emitting design is a rule the next method
    /// added has to remember, and a forgotten one is a client that never learns, with no error
    /// anywhere to say so — the failure `notify`'s wake was shaped to make impossible, extended
    /// here to the wake's payload.
    #[test]
    fn a_spawned_pane_is_reported_without_the_action_emitting_anything() {
        let state = host_with("sleep 30", 80, 24);
        // The boot observation: a session's FIRST shape has no predecessor, so it records nothing.
        state.channels().observe(&lock(state.registry()), BOOT);
        let baseline = state.revision(BOOT).current();
        assert!(
            journal_since(&state, 0).is_empty(),
            "arriving first is not a change — the initial shape records nothing",
        );

        invoke_recording(&state, crate::wire::SPAWN_ACTION, serde_json::json!({}));

        let events = journal_since(&state, baseline);
        assert!(
            events
                .iter()
                .any(|event| matches!(event, crate::events::Event::PaneCreated(_))),
            "the spawn reached the journal without a line of its own: {events:?}",
        );
    }

    /// The other half of the same claim, and the one that keeps the log affordable: a keystroke is
    /// a `scene/invoke` too, so the derive site runs at TYPING rate. It must record nothing.
    ///
    /// A shape that walked every pane here would pay an N-lock walk per key. The gate is each
    /// window's `layout_revision`, and this is what holds it to that: input moves no arrangement,
    /// so no pane list is re-read and no record is written.
    #[test]
    fn typing_into_a_pane_records_nothing() {
        let state = host_with("sleep 30", 80, 24);
        state.channels().observe(&lock(state.registry()), BOOT);
        let baseline = state.revision(BOOT).current();

        for _ in 0..8 {
            let sink = Arc::new(Mutex::new(Vec::new()));
            let request = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "scene/invoke",
                "params": {
                    "path": crate::wire::pane_input_path(0, crate::wire::KEY_ACTION),
                    "args": { "key": "a" },
                },
            });
            dispatch_recording(&state, &request.to_string(), &sink);
        }

        assert!(
            journal_since(&state, baseline).is_empty(),
            "input is not a structural change, and the derive site must not invent one",
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
        let since = state.revision(BOOT).current();
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
                state.waiters(BOOT).parked_count(),
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
            state.waiters(BOOT).parked_count(),
            1,
            "a well-scoped waitFor against the current revision parks normally",
        );
        assert!(
            sink.lock().unwrap().is_empty(),
            "...and is not answered while parked",
        );
    }

    /// A `scene/waitFor` sleeps through ANOTHER session's changes and wakes on its own — the scope
    /// HONORED, not merely checked, through the real `dispatch_one`.
    ///
    /// The check above proves an unhonorable scope is refused. This proves the honorable one is
    /// obeyed, which is the half that was missing: with one registry-wide token every attached
    /// client woke on every session's output, re-read its own, found nothing, and re-parked. The
    /// wake was safe (a hint to re-read, and the re-read was scoped and exact) and its cost scaled
    /// with the number of ATTACHED clients rather than with the number that could care.
    ///
    /// Both halves are asserted in one test on purpose. "Did not wake" alone is satisfied by a
    /// waiter that can never wake at all — a park against a token nothing bumps looks identical —
    /// so the second half is what makes the first mean anything.
    ///
    /// REVERT-PROOF: park against `state.waiters(BOOT)` / `state.revision(BOOT)` in `dispatch_one`
    /// (the one-token behaviour) and the first assertion fails on `work`'s wait being answered by
    /// the default session's bump.
    #[test]
    fn a_wait_sleeps_through_another_sessions_changes() {
        let state = host_with("cat", 20, 4);
        // A second session, born with its own pane — the state where two sessions can move
        // independently, which is the only state in which this claim is stateable.
        let created = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":1,"method":"scene/invoke","params":{"path":"/sprag_mux/external/new_session","args":{"name":"work"}}}"#,
        );
        assert_eq!(
            created["result"], "work",
            "the second session exists: {created}"
        );

        // Park a wait scoped to `work`, against `work`'s OWN baseline. The baseline is read under
        // that name for the same reason a client re-reads `scene/revision` after re-scoping: two
        // sessions' counters advance independently, so a number from one is not a baseline in the
        // other.
        let since = state.revision("work").current();
        let sink = Arc::new(Mutex::new(Vec::new()));
        dispatch_recording(
            &state,
            &format!(
                r#"{{"jsonrpc":"2.0","id":7,"method":"scene/waitFor","params":{{"since":{since},"session":"work"}}}}"#
            ),
            &sink,
        );
        assert_eq!(
            state.waiters("work").parked_count(),
            1,
            "the wait parked on its own session",
        );

        // The DEFAULT session moves — a real mutation through the real dispatch, so pinion's own
        // OCC bump is what advances that session's token, not a hand-written bump this test could
        // have aimed anywhere it liked.
        let spawned = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":2,"method":"scene/invoke","params":{"path":"/sprag_mux/external/spawn","args":{}}}"#,
        );
        assert!(
            spawned["error"].is_null(),
            "the default session moved: {spawned}"
        );
        assert!(
            sink.lock().unwrap().is_empty(),
            "another session's change is not this client's business",
        );
        assert_eq!(
            state.waiters("work").parked_count(),
            1,
            "and it is still asleep, not woken-and-re-parked",
        );

        // ...and its OWN session moving reaches it.
        let own = serve_one(
            &state,
            r#"{"jsonrpc":"2.0","id":3,"method":"scene/invoke","params":{"path":"/sprag_mux/external/spawn","args":{},"session":"work"}}"#,
        );
        assert!(own["error"].is_null(), "work moved: {own}");
        let answered = sink.lock().unwrap();
        assert_eq!(
            answered.len(),
            1,
            "the wait was answered by its own session"
        );
        let v: serde_json::Value = serde_json::from_str(&answered[0]).unwrap();
        assert_eq!(v["result"]["changed"], true, "{v}");
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
            state.revision(BOOT).current(),
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
        let since = state.revision(BOOT).current();
        let sink = Arc::new(Mutex::new(Vec::new()));
        dispatch_recording(
            &state,
            &format!(
                r#"{{"jsonrpc":"2.0","id":5,"method":"scene/waitFor","params":{{"since":{since}}}}}"#
            ),
            &sink,
        );
        assert_eq!(
            state.waiters(BOOT).parked_count(),
            1,
            "parked at the current revision"
        );
        assert!(sink.lock().unwrap().is_empty(), "not answered while parked");

        let new = state.revision(BOOT).bump();
        assert_eq!(
            state.waiters(BOOT).parked_count(),
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
        state.revision(BOOT).bump();
        let current = state.revision(BOOT).current();
        let sink = Arc::new(Mutex::new(Vec::new()));
        dispatch_recording(
            &state,
            r#"{"jsonrpc":"2.0","id":6,"method":"scene/waitFor","params":{"since":0}}"#,
            &sink,
        );
        assert_eq!(
            state.waiters(BOOT).parked_count(),
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
        let since = state.revision(BOOT).current();
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
        let since = state.revision(BOOT).current();
        let sink = Arc::new(Mutex::new(Vec::new()));
        dispatch_recording(
            &state,
            &format!(
                r#"{{"jsonrpc":"2.0","id":8,"method":"scene/waitFor","params":{{"since":{since}}}}}"#
            ),
            &sink,
        );
        assert_eq!(
            state.waiters(BOOT).parked_count(),
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
            state.waiters(BOOT).parked_count(),
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

    /// The `find.<needle>` family over the FULL dispatch path: a live pane, a real search, and the
    /// answer in the pane's logical coordinate.
    ///
    /// The property that separates this family from `cells.<offset>` is the one worth an
    /// integration test: **the needle rides the path VERBATIM**. pinion hands an External
    /// everything after the first `/external/` untouched, so a needle containing the family's own
    /// `.` separator — or a space — must still address one search, with no escaping and therefore no
    /// second spelling of the same needle. A unit test on the argument parser cannot show that; only
    /// a request through the dispatch can.
    #[test]
    fn the_find_family_answers_matches_over_the_wire() {
        // Row 0 = `a.b hit`, row 1 = `hit`: two matches for `hit`, and a needle with a dot+space.
        let state = host_with("printf 'a.b hit\\nhit'", 20, 4);
        wait_for_pane0_eof(&state);

        let answer = query_pane0(&state, &crate::wire::find_slot_for("hit"));
        assert!(answer.get("error").is_none(), "find error: {answer}");
        assert_eq!(
            answer["result"]["matches"],
            serde_json::json!([
                { "line": 0, "col": 4, "cols": 3 },
                { "line": 1, "col": 0, "cols": 3 },
            ]),
            "matches carry the LOGICAL line + CELL columns a client scrolls and highlights by: \
             {answer}",
        );
        assert_eq!(answer["result"]["truncated"], false);

        // The verbatim-path property: `.` and ` ` inside the needle are needle, not grammar.
        let dotted = query_pane0(&state, &crate::wire::find_slot_for("a.b hit"));
        assert_eq!(
            dotted["result"]["matches"],
            serde_json::json!([{ "line": 0, "col": 0, "cols": 7 }]),
            "a needle containing the separator and a space addresses ONE search: {dotted}",
        );

        // The family taxonomy, same as `cells`: an EMPTY argument is a malformed MEMBER
        // (present-but-empty), while the bare stem carries no argument and is absent.
        let empty = query_pane0(&state, "find.");
        assert!(
            empty.get("error").is_none() && empty["result"].is_null(),
            "an empty needle is a malformed member -> Null, not an error: {empty}",
        );
        assert!(
            query_pane0(&state, "find").get("error").is_some(),
            "the bare stem addresses no search",
        );
    }

    /// The `regex.<pattern>` family over the FULL dispatch path, and the two properties that make
    /// it worth a second address rather than a flag.
    ///
    /// First, the SAME string addresses two different searches: `find.a.b` matches three literal
    /// characters and `regex.a.b` matches "a, anything, b" — over the same pane, in the same
    /// request shape, differing only in which family the path names. Second, a pattern the engine
    /// REFUSES answers the normal shape carrying the engine's message, not `Null`: an invalid
    /// pattern is a well-formed address whose value was rejected, and `Null` there would be
    /// indistinguishable from "no such pane".
    #[test]
    fn the_regex_family_is_a_second_language_at_a_second_address() {
        // Row 0 = `axb a.b`: the literal needle `a.b` matches once, the pattern matches twice.
        let state = host_with("printf 'axb a.b'", 20, 4);
        wait_for_pane0_eof(&state);

        let literal = query_pane0(&state, &crate::wire::find_slot_for("a.b"));
        assert_eq!(
            literal["result"]["matches"],
            serde_json::json!([{ "line": 0, "col": 4, "cols": 3 }]),
            "literally, only the real dot: {literal}",
        );
        let pattern = query_pane0(&state, &crate::wire::regex_slot_for("a.b"));
        assert_eq!(
            pattern["result"]["matches"],
            serde_json::json!([
                { "line": 0, "col": 0, "cols": 3 },
                { "line": 0, "col": 4, "cols": 3 },
            ]),
            "as a pattern, the dot matches any character: {pattern}",
        );

        // A refused pattern: the answer is the normal shape, carrying WHY.
        let refused = query_pane0(&state, &crate::wire::regex_slot_for("a(b"));
        assert!(refused.get("error").is_none(), "not a protocol error");
        assert!(
            refused["result"]["error"].is_string(),
            "the engine's message rides the answer: {refused}",
        );
        assert_eq!(
            refused["result"]["matches"],
            serde_json::json!([]),
            "and it searched nothing: {refused}",
        );
        // A VALID pattern never carries the field at all, so a caller can test for its presence.
        assert!(
            pattern["result"].get("error").is_none(),
            "a successful search carries no error key: {pattern}",
        );

        // Same taxonomy as `find`: an EMPTY pattern is a malformed member, the bare stem is absent.
        let empty = query_pane0(&state, "regex.");
        assert!(
            empty.get("error").is_none() && empty["result"].is_null(),
            "an empty pattern is a malformed member -> Null: {empty}",
        );
        assert!(
            query_pane0(&state, "regex").get("error").is_some(),
            "the bare stem addresses no search",
        );
    }

    /// Searching a pane is a READ: it must not move the scene revision.
    ///
    /// This is the PR-61 livelock lesson applied BEFORE it can bite. A find bar re-queries on every
    /// keystroke; if that bumped, one client's typing would wake every other attached client's
    /// parked `waitFor` into a full re-fetch — the exact defect that made `cells` a query.
    #[test]
    fn a_find_query_does_not_bump_the_revision() {
        let state = host_with("printf hit", 20, 4);
        wait_for_pane0_eof(&state);
        let before = state.revision(BOOT).current();
        for _ in 0..5 {
            let answer = query_pane0(&state, &crate::wire::find_slot_for("hit"));
            assert!(answer.get("error").is_none(), "find error: {answer}");
        }
        assert_eq!(
            state.revision(BOOT).current(),
            before,
            "a search changes nothing about the pane, so it wakes no waiter",
        );
    }

    /// The reply to a `cells.<offset>` query, read back through the ONE wire type both ends
    /// share — the same [`CellFrame`](crate::CellFrame) `sprag-gui`'s `WireHost` deserializes.
    ///
    /// Deliberately NOT `serde_json::from_value::<GridBuffer>(result["cells"])`: that reaches past
    /// the frame's own definition and spells the grid's wire shape a second time, which is exactly
    /// what broke when R222 replaced that shape with a run-length encoding. Going through
    /// `CellFrame` means the host end and the client end of this test are the same code the
    /// daemon and the display client are.
    fn cells_frame_typed(state: &HostState, offset: usize) -> crate::CellFrame {
        let answer = cells_frame(state, offset);
        assert!(answer.get("error").is_none(), "frame error: {answer}");
        serde_json::from_value(answer["result"].clone())
            .expect("the frame deserializes through the one wire type")
    }

    #[test]
    fn the_cells_family_returns_a_deserializable_grid_frame() {
        // The wire client's per-frame read: `cells.0` returns a JSON frame that deserializes back
        // into the EXACT GridBuffer the host projected, carrying the pane content, plus the scroll
        // facts that ride with it as top-level keys.
        let state = host_with("printf hi", 20, 4);
        wait_for_pane0_eof(&state);
        let answer = cells_frame(&state, 0);
        assert!(answer.get("error").is_none(), "frame error: {answer}");
        let result = &answer["result"];
        // The facts are asserted on the RAW keys, because being top-level is the whole of what
        // `#[serde(flatten)]` buys and a typed read would not notice it moving.
        assert!(
            result["scrollback_len"].is_u64(),
            "scroll facts present: {result}"
        );
        assert_eq!(result["visible_rows"], 4);

        let frame = cells_frame_typed(&state, 0);
        assert_eq!(
            (frame.cells.cols(), frame.cells.rows()),
            (20, 4),
            "buffer dims match the pane"
        );
        // "hi" is on row 0 — the wire buffer carries the exact projected content.
        assert_eq!(
            frame.cells.cell(0, 0).map(|c| c.cluster.as_ref()),
            Some("h")
        );
        assert_eq!(
            frame.cells.cell(1, 0).map(|c| c.cluster.as_ref()),
            Some("i")
        );
    }

    #[test]
    fn the_cells_family_honors_the_scrollback_offset() {
        // 40 lines into a 4-row pane: most scroll off into history. The live view
        // (`cells.0`) and a scrolled-up view (`cells.20`) differ, proving the offset reaches
        // the projection over the wire — carried on the PATH, which is the whole of PR-61.
        let state = host_with("seq 1 40", 20, 4);
        wait_for_pane0_eof(&state);

        let live = cells_frame_typed(&state, 0);
        assert!(
            live.facts.scrollback_len > 0,
            "lines scrolled off into history",
        );

        assert_ne!(
            live.cells,
            cells_frame_typed(&state, 20).cells,
            "a scrollback offset changes the projected buffer",
        );
    }
}
