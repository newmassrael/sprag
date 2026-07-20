//! The multiplexer control surface — pane + layout management as a pinion `External`.
//!
//! The multiplexer's state (a [`SessionRegistry`] of sessions → windows → pane pools, a
//! producer-layer concern in sprag-terminal) is exposed to AI peers through one engine
//! `External`: the mux control plane. It generalizes the R6 input pattern — producer
//! mutations ride pinion's canonical `scene/invoke` against a producer-owned handler,
//! never new RPC methods (pinion RPC vocabulary stays SSOT).
//!
//! Action channel (`scene/invoke`):
//!
//! * `spawn {cmd?:[..], cols?, rows?}` → spawns a pane, returns its id.
//! * `close {id}` → reaps a pane.
//! * `resize {id, cols, rows}` → resizes a pane's PTY + emulator.
//! * `set_layout {tree}` → installs a client's settled arrangement, returns the canonical one.
//! * `set_floating {id, floating}` → takes a pane out of the tiling / puts it back.
//! * `new_session {name?, cmd?, cols?, rows?}` → creates a session BORN with one pane (absent
//!   name → lowest free; `cmd`/`cols`/`rows` shape the birth pane), returns its name.
//! * `kill_session {name}` → kills a session; the last one ends the daemon (tmux kill-session).
//!
//! Read channel (`scene/query`):
//!
//! * `panes` → the live pane list as JSON.
//! * `layout` → the scoped session's current-window LOGICAL arrangement + its revision.
//! * `sessions` → every session, and which one an unscoped request lands in.
//!
//! ## Why this surface holds the REGISTRY (and the plugin host does not)
//!
//! This is the multiplexer's own control plane, and sessions / windows / layout ARE mux
//! concerns — so it holds the `Arc<Mutex<SessionRegistry>>`. The PLUGIN host
//! ([`crate::PluginsExternal`]) deliberately still takes only `Arc<Mutex<Workspace>>`: a
//! plugin operates on a pane pool and has no business knowing about the session tree
//! (Interface Segregation).
//!
//! ## Why it is the one surface carrying a scope
//!
//! Holding the registry is holding EVERY session, so this surface alone can act outside the
//! one the request named — which is why it is handed a [`SessionScope`] and every action but
//! the registry-wide session ones goes through it. The rest of the scene needs no such care: a
//! pane child and the plugin host are built from the scoped pool and can address nothing
//! else. The privilege is what creates the obligation.
//!
//! Three members are deliberately registry-WIDE rather than scoped, and for the same reason:
//! their subject is the set of sessions itself, so answering them within one session would
//! answer a question nobody asked. `sessions` enumerates the scopes a client may name;
//! `new_session` makes one and `kill_session` removes one — each NAMES its session directly
//! rather than acting on the request's scope, and none can silently disturb another client.
//!
//! Still no PIXEL division here — headless multiplexing is pane control, not screen
//! division (the Round 7 note). The `layout` slot is the LOGICAL arrangement (which
//! panes are split, in what order, at what proportion), which is session state a
//! detached client must not take with it; rects stay a rendering concern of whichever
//! client projects it (see [`sprag_terminal::layout`]).

use std::fmt;
use std::sync::{Arc, Mutex};

use pinion_core::SceneRevision;
use pinion_core::external::{
    ExternalIntrospect, InterveneError, IntrospectSchema, IntrospectValue, InvokeError, SchemaField,
};
use serde_json::{Map, Value};
use sprag_terminal::{
    CommandBuilder, KillOutcome, LayoutSnapshot, LayoutWire, PaneId, SessionInfo, SessionRegistry,
    WindowKillOutcome, Workspace,
};

use crate::bump_on_dirty;
use crate::external::{as_object, lock, opt_dim, require_pane_id, rpc_external_impl};
use crate::scope::SessionScope;

// The mux control action names + query slots are the shared wire ABI vocabulary
// ([`crate::wire`]) — the SAME consts a client addresses for pane lifecycle.
use crate::wire::{
    CLIENTS_SLOT, CLOSE_ACTION, KILL_SESSION_ACTION, KILL_WINDOW_ACTION, LAYOUT_SLOT,
    NEW_SESSION_ACTION, NEW_WINDOW_ACTION, PANES_SLOT, RENAME_WINDOW_ACTION, RESIZE_ACTION,
    SELECT_WINDOW_ACTION, SESSIONS_SLOT, SET_FLOATING_ACTION, SET_LAYOUT_ACTION, SPAWN_ACTION,
    WINDOWS_SLOT,
};

/// The mux-management engine `External`: a control surface over the shared
/// [`SessionRegistry`]. Holds `Arc<Mutex<SessionRegistry>>` so its `scene/invoke`
/// handlers mutate the live pane pool of the CURRENT window (which the serve loop also
/// reads to assemble the scene) and its `layout` slot can serve that window's
/// arrangement, plus the shared [`SceneRevision`] so a pane-lifecycle mutation wakes any
/// parked `scene/waitFor`.
pub struct WorkspaceExternal {
    registry: Arc<Mutex<SessionRegistry>>,
    /// The session this surface may act on — the request's own, resolved once at the door.
    ///
    /// **The one child of the scene that needs telling.** Every other surface is built from
    /// the scoped pool and so cannot reach another session; this one holds the REGISTRY
    /// (sessions / windows / layout are mux concerns), which is every session. Without the
    /// scope it would read the arrangement of the session it was assembled for and write to
    /// whichever one happened to be the default — the silent cross-session write, which is
    /// the exact failure the scope param exists to prevent.
    scope: SessionScope,
    /// The shared scene-version token ([`crate::HostState`]'s). Two roles:
    /// each pane this surface SPAWNS is wired with a `bump_on_dirty(&revision)`
    /// hook (so its output wakes waiters, like the boot pane), and a spawn /
    /// close bumps it directly (so a pane-set change wakes a waiter before the
    /// new pane's first output). Cloned per scene-assembly from the ONE token
    /// [`crate::HostState`] observes, so a mux-spawned pane can never be wired
    /// to a revision no waiter watches.
    ///
    /// **v1 bound:** ONE token for the whole registry, so a spawn in one session wakes
    /// clients attached to every other, which re-read and find nothing changed. Waste, not
    /// error — see [`crate::workspace_scene`].
    revision: Arc<SceneRevision>,
    /// The self-cleaning daemon's pane-`on_exit` death-signal hook ([`crate::spawn_reaper`]),
    /// or `None` off a daemon (a GUI's in-process host, the unit tests). Wired into each pane
    /// this surface SPAWNS so its death feeds the reaper. Injected (not named here) so this
    /// library never decides process lifetime, and so a test spawns and reaps panes through
    /// this exact surface without ending its own process. It is registry-FREE (just a channel
    /// send), so it is safe to run from any thread.
    on_pane_exit: Option<Arc<dyn Fn() + Send + Sync>>,
    /// The daemon's per-client attachment map ([`crate::AttachmentRegistry`]), or `None` off a
    /// daemon (a GUI's in-process host, the unit tests — nothing attaches to those over a wire).
    /// Read ONLY to fill each [`SessionInfo::attached`](sprag_terminal::SessionInfo::attached)
    /// when the `sessions` slot is served; this surface never mutates it (the dispatch layer owns
    /// the writes, off the frame's connection id, which no external sees). `None` leaves every
    /// `attached` at 0 — an honest "no wire clients here".
    attachments: Option<Arc<Mutex<crate::AttachmentRegistry>>>,
}

/// A VALIDATED pane-spawn request — the command, its label, and any explicit dims. Produced by
/// [`WorkspaceExternal::parse_spawn`] from the wire args, so a MALFORMED request is rejected
/// (`TypeMismatch`) before any pane — or session — is built. `cols`/`rows` left `None` take the
/// target pool's default size at spawn.
struct SpawnSpec {
    command: CommandBuilder,
    label: String,
    cols: Option<u16>,
    rows: Option<u16>,
}

impl WorkspaceExternal {
    /// Build the control surface over the shared mux registry, the session it is scoped to,
    /// the shared scene-version token, and the daemon's `on_pane_exit` death-signal (`None` off
    /// a daemon) — see the struct docs for each field's role.
    #[must_use]
    pub fn new(
        registry: Arc<Mutex<SessionRegistry>>,
        scope: SessionScope,
        revision: Arc<SceneRevision>,
        on_pane_exit: Option<Arc<dyn Fn() + Send + Sync>>,
        attachments: Option<Arc<Mutex<crate::AttachmentRegistry>>>,
    ) -> Self {
        Self {
            registry,
            scope,
            revision,
            on_pane_exit,
            attachments,
        }
    }

    /// The scoped session's current-window pane pool — resolved when the scope was, so a
    /// spawn lands in the session the request named and nowhere else. No registry lock is
    /// taken to reach it, so it cannot nest with the workspace lock.
    fn workspace(&self) -> &Arc<Mutex<Workspace>> {
        self.scope.workspace()
    }

    /// Parse + VALIDATE the `{cmd?, cols?, rows?}` spawn spec — the REQUEST-validation half of a
    /// spawn, pool-free so it runs before anything is built. A malformed field (`cmd` present but
    /// not an array, a non-`u16` `cols`/`rows`) is a `TypeMismatch` HERE, the same refusal the
    /// `spawn` action and `new_session`'s own `name` field give a type error — so a `new_session`
    /// can reject a malformed birth spec before it creates the session. `cmd` (an argv array)
    /// defaults to `$SHELL`; `cols`/`rows` left `None` take the pool's default size at spawn.
    fn parse_spawn(map: &Map<String, Value>) -> Result<SpawnSpec, InvokeError> {
        let (command, label) = match map.get("cmd") {
            None => sprag_terminal::default_shell_command(),
            Some(Value::Array(argv)) => build_command(argv)?,
            Some(_) => return Err(InvokeError::TypeMismatch),
        };
        Ok(SpawnSpec {
            command,
            label,
            cols: opt_dim(map, "cols")?,
            rows: opt_dim(map, "rows")?,
        })
    }

    /// Fork/exec a validated [`SpawnSpec`] into `pool` — the RUNTIME half, shared by the `spawn`
    /// action and a [`new_session`](Self::new_session)'s birth pane.
    ///
    /// Wired WITH the change-notification hook (so this pane's output bumps the SAME revision the
    /// boot pane's does — a client's `scene/waitFor` wakes on it exactly as on the boot pane) and,
    /// under a daemon, the `on_pane_exit` death-signal (so THIS pane's death feeds the reaper). A
    /// fork/exec failure is `Rejected` — a WELL-FORMED request the OS could not honor (a broken
    /// `$SHELL`, an argv it cannot `exec`), DISTINCT from the malformed request [`parse_spawn`]
    /// already rejected. Does NOT bump the revision — the caller signals its set change once (a
    /// plain `spawn`, or the create that births this pane), so the two never double-bump or drift.
    fn spawn_parsed(
        &self,
        pool: &Arc<Mutex<Workspace>>,
        spec: SpawnSpec,
    ) -> Result<PaneId, InvokeError> {
        let on_exit = self.on_pane_exit.as_ref().map(crate::pane_exit_hook);
        let mut workspace = lock(pool);
        let (default_cols, default_rows) = workspace.default_size();
        workspace
            .spawn_with_dirty(
                spec.command,
                spec.label,
                spec.cols.unwrap_or(default_cols),
                spec.rows.unwrap_or(default_rows),
                Some(bump_on_dirty(&self.revision)),
                on_exit,
            )
            .map_err(|_| InvokeError::Rejected)
    }

    /// `spawn` action: create a pane in THIS request's session and return its id. `cmd` (an argv
    /// array) defaults to `$SHELL`; `cols`/`rows` default to the workspace's default size.
    fn spawn(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let empty = Map::new();
        let map = match args {
            IntrospectValue::Json(Value::Object(m)) => m,
            IntrospectValue::Null => &empty,
            _ => return Err(InvokeError::TypeMismatch),
        };
        let id = self.spawn_parsed(self.workspace(), Self::parse_spawn(map)?)?;
        // A NEW pane changed the set: wake parked waiters now, before its first output, so a
        // mirror learns the pane exists immediately (the pane-set change-notification, distinct
        // from the per-pane output bump the hook fires).
        self.revision.bump();
        Ok(IntrospectValue::Int(
            i64::try_from(id.0).unwrap_or(i64::MAX),
        ))
    }

    /// `close` action: reap the pane with `id`. The removed `Pane` is bound
    /// here so the workspace guard drops first and the pane's blocking
    /// `Drop` (kill/wait/join) runs *outside* the lock.
    fn close(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let id = require_pane_id(as_object(args)?, "id")?;
        let removed = lock(self.workspace()).close(id);
        if removed.is_some() {
            // The set shrank: wake parked waiters so a mirror drops the pane's
            // tile promptly. `removed` (the reaped `Pane`) is still bound, so its
            // blocking `Drop` (kill/wait/join) runs after this returns, outside the
            // lock — the bump only signals the already-completed removal.
            self.revision.bump();
            Ok(IntrospectValue::Null)
        } else {
            Err(InvokeError::Rejected) // no such pane
        }
    }

    /// `resize` action: resize the pane with `id` to `cols x rows`.
    fn resize(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let map = as_object(args)?;
        let id = require_pane_id(map, "id")?;
        let cols = opt_dim(map, "cols")?.ok_or(InvokeError::TypeMismatch)?;
        let rows = opt_dim(map, "rows")?.ok_or(InvokeError::TypeMismatch)?;
        match lock(self.workspace()).resize(id, cols, rows) {
            Ok(true) => Ok(IntrospectValue::Null),
            Ok(false) => Err(InvokeError::Rejected), // no such pane
            Err(_) => Err(InvokeError::Rejected),    // winsize ioctl failed
        }
    }

    /// `set_layout {tree}` action: install a client's settled arrangement, answering with
    /// the canonical one — the write half of the arc (see [`crate::wire::SET_LAYOUT_ACTION`]).
    ///
    /// The answer carries the tree as the host stores it, with every client-minted divider
    /// now named, so one round trip both records the gesture and tells the client which
    /// identities to key its per-split state on.
    fn set_layout(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let map = as_object(args)?;
        // The revision this gesture was authored against — the arrangement the client was
        // looking at. Required: a write with no answer to "which layout is this about?"
        // cannot be adjudicated, and silently accepting it is how one client reverts
        // another's gesture with neither told.
        let expected = map
            .get("expected_revision")
            .and_then(Value::as_u64)
            .ok_or(InvokeError::TypeMismatch)?;
        let tree = map.get("tree").ok_or(InvokeError::TypeMismatch)?.clone();
        // A tree that will not even deserialise is a malformed REQUEST (the client and host
        // disagree on the shape), distinct from the well-formed-JSON-but-invalid-arrangement
        // the tree's own validation rejects — so it fails here as a TypeMismatch rather than
        // reaching the window.
        let tree: LayoutWire = serde_json::from_value(tree).map_err(|error| {
            tracing::warn!(target: "sprag_host", %error, "set_layout: undeserialisable tree");
            InvokeError::TypeMismatch
        })?;
        // Optional: the NAME of the window the gesture was authored against. Absent ⇒ no
        // window-staleness check (an older or single-client caller); present-but-not-a-string is
        // malformed, the same refusal a wrong-typed `expected_revision` gets. It closes the
        // per-window-revision bound: a write drawn on a window the client has switched away from
        // is refused rather than mis-applied to whatever is current now.
        let expected_window = match map.get("expected_window") {
            None => None,
            Some(Value::String(name)) => Some(name.as_str()),
            Some(_) => return Err(InvokeError::TypeMismatch),
        };
        let snapshot =
            crate::host::set_layout(&self.registry, &self.scope, tree, expected, expected_window)
                .ok_or(InvokeError::Rejected)?;
        // The arrangement changed: wake parked waiters so another attached client
        // re-projects promptly, exactly as a pane-set change does.
        self.revision.bump();
        layout_value(snapshot).ok_or(InvokeError::Rejected)
    }

    /// `set_floating {id, floating}` action: take a pane out of the tiling or put it back,
    /// answering with the resulting arrangement (see [`crate::wire::SET_FLOATING_ACTION`]).
    fn set_floating(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let map = as_object(args)?;
        let id = require_pane_id(map, "id")?;
        let floating = map
            .get("floating")
            .and_then(Value::as_bool)
            .ok_or(InvokeError::TypeMismatch)?;
        let snapshot = crate::host::set_floating(&self.registry, &self.scope, id, floating)
            .ok_or(InvokeError::Rejected)?;
        self.revision.bump();
        layout_value(snapshot).ok_or(InvokeError::Rejected)
    }

    /// `new_session {name?, cmd?, cols?, rows?}` action: create a session BORN WITH A SHELL,
    /// answering with its name.
    ///
    /// `name` mirrors the `session` scope param's own three-way shape: ABSENT asks the
    /// registry to allocate the lowest free name (tmux's `new-session` with no `-s`), a STRING
    /// names it, and a NON-STRING is a malformed request — rejected, never silently allocated,
    /// which would be the same alias smell the scope param refuses in its type-error corner. The
    /// `cmd`/`cols`/`rows` birth spec is validated the SAME way, before the session is created
    /// ([`parse_spawn`](Self::parse_spawn)): a malformed one is rejected with nothing built.
    ///
    /// On the happy path a session is born with one pane — tmux's `new-session`, where creating a
    /// session always spawns its first window's pane — so it is not empty. `cmd`/`cols`/`rows`
    /// shape that birth pane, mirroring tmux's `new-session -x -y [command]`: the GUI passes its
    /// own first pane so a client's configured layout tops up from it without a default-shell
    /// mismatch; the CLI passes nothing and gets the default `$SHELL` at the pool's default size.
    /// Putting the spawn HERE (the server-side command handler, sprag's `cmd-new-session`) rather
    /// than in each caller keeps "a session has a shell" an invariant at the authority, not a
    /// convention every caller must remember — and it is here, not in the pinion-free registry,
    /// because the pane's death-signal ([`on_pane_exit`]) is a daemon-lifetime concern the
    /// registry does not carry. A RUNTIME fork/exec failure is the one case that still leaves the
    /// session empty (see the birth site below) — best-effort, not an absolute invariant.
    ///
    /// Creating is not attaching, and nothing here changes what any other client sees: every
    /// client's scope is either its own name or the default (which a `kill_session` of the current
    /// default re-points, but a `new_session` never moves). The answer is the name — indispensable
    /// for the allocated case (the caller did not choose it) — so a caller can scope its next
    /// request with what it just made, without a round trip to [`SESSIONS_SLOT`] to confirm.
    ///
    /// [`on_pane_exit`]: Self::on_pane_exit
    fn new_session(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let map = as_object(args)?;
        let name = match map.get("name") {
            None => None,
            Some(Value::String(name)) => Some(name.as_str()),
            Some(_) => return Err(InvokeError::TypeMismatch),
        };
        // VALIDATE the birth spec BEFORE creating anything, so a malformed `cmd`/`cols`/`rows` is
        // rejected with no session built — uniform with the `spawn` action and with `name` above.
        // Only a RUNTIME fork/exec failure (the birth site below) is tolerated non-fatally.
        let spec = Self::parse_spawn(map)?;
        // Create the empty session shell under the registry authority, then clone its pool Arc OUT
        // (via `workspace_of`, the one helper for exactly this) so the birth pane spawns OFF the
        // registry lock — the established registry->workspace order, never nested across a
        // fork/exec that would otherwise stall every other request on the registry lock. This
        // narrows the once-a-round-trip empty-session window to two in-handler lock ops. The
        // residual — an unrelated last pane dying between them, self-exiting the daemon under a
        // just-connecting client — is the INHERENT "zero live panes ⇒ exit" race: fail-safe (no
        // corruption — the birth pane either wins the pool lock and the daemon survives, or the
        // daemon SIGTERMs and this call returns `UnexpectedEof`). It is NOT recovered here: the
        // joining client's boot fails hard, and only a fresh relaunch's connect-or-spawn brings up
        // the next daemon. It closes fully once a just-created session can pin liveness (increment).
        let (allocated, pool) = {
            let mut registry = lock(&self.registry);
            let allocated = registry.new_session(name).map_err(|error| {
                tracing::debug!(target: "sprag_host", %error, "refused to create a session");
                // A taken name is the client's mistake, not a malformed request: it is
                // well-formed and simply cannot be honored.
                InvokeError::Rejected
            })?;
            let pool = registry
                .workspace_of(&allocated)
                .expect("the session just created resolves");
            (allocated, pool)
        };
        // Birth the pane. Only a RUNTIME fork/exec failure reaches here (a broken `$SHELL`, an argv
        // the OS cannot `exec`) — the malformed request was already rejected above. It is logged,
        // not fatal: the session still exists as a valid attach target, merely empty until a pane
        // is added, so "a well-formed create with a free name succeeds" stays total rather than
        // orphaning a half-created session behind an error.
        if let Err(error) = self.spawn_parsed(&pool, spec) {
            tracing::warn!(
                target: "sprag_host",
                ?error,
                session = %allocated,
                "the birth pane could not spawn; the session was created empty",
            );
        }
        // The session SET changed AND (on success) it now holds a live pane: wake a client
        // watching the surface once, the way it learns of any pane-set change.
        self.revision.bump();
        Ok(IntrospectValue::Json(Value::String(allocated)))
    }

    /// `kill_session {name}` action: kill a session (tmux `kill-session`).
    ///
    /// A non-last kill removes the session and answers `null` — a set change, so a client
    /// watching [`SESSIONS_SLOT`] is woken. Killing the LAST session drains it and ENDS the
    /// daemon: it fires the SAME death-signal a pane exit does ([`on_pane_exit`]), so the reaper
    /// re-checks liveness, finds none, and exits through the one SIGTERM shutdown funnel — the
    /// library names neither exit nor SIGTERM. Off a daemon (a GUI's in-process host, the tests)
    /// `on_pane_exit` is `None`, so the kill removes the session but nothing exits, which is
    /// exactly right there.
    ///
    /// [`on_pane_exit`]: Self::on_pane_exit
    fn kill_session(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let name = as_object(args)?
            .get("name")
            .and_then(Value::as_str)
            .ok_or(InvokeError::TypeMismatch)?;
        // Remove/drain UNDER the lock, then RELEASE it before dropping the reaped owners: binding
        // `outcome` here means the lock guard falls at the `;`, so the removed session / drained
        // panes ride in `outcome` and their blocking `Drop` (kill / wait / join the reader) runs
        // OFF the lock — the `close` action's discipline.
        let outcome = lock(&self.registry).kill_session(name);
        match outcome {
            Ok(outcome) => self.handle_session_kill(outcome),
            Err(error) => {
                tracing::debug!(target: "sprag_host", %error, "refused to kill a session");
                return Err(InvokeError::Rejected);
            }
        }
        Ok(IntrospectValue::Json(Value::Null))
    }

    /// React to a [`KillOutcome`] that has already been bound OFF the registry lock (so its
    /// reaped owners drop here, outside the lock): a removed session is a set change that wakes a
    /// client watching the sessions list; the last-session case nudges the reaper (via the pane
    /// death-signal) to re-check liveness and exit through the SIGTERM funnel. Shared by
    /// [`kill_session`](Self::kill_session) and the last-window escalation in
    /// [`kill_window`](Self::kill_window), so the two cannot drift.
    fn handle_session_kill(&self, outcome: KillOutcome) {
        match outcome {
            KillOutcome::Removed(_removed) => {
                self.revision.bump();
            }
            KillOutcome::KilledServer(_drained) => {
                if let Some(on_pane_exit) = &self.on_pane_exit {
                    on_pane_exit();
                }
            }
        }
    }

    /// Resolve a `window` TARGET arg (used by `rename_window` / `kill_window`): absent ⇒ the
    /// current window (the scope's), a string ⇒ that window, present-but-not-a-string ⇒ malformed
    /// — the same aliasing corner the session scope param refuses, rather than silently falling
    /// back to the current window and acting on the wrong one.
    fn window_target<'a>(&'a self, map: &'a Map<String, Value>) -> Result<&'a str, InvokeError> {
        match map.get("window") {
            None => Ok(self.scope.window()),
            Some(Value::String(name)) => Ok(name),
            Some(_) => Err(InvokeError::TypeMismatch),
        }
    }

    /// `new_window {name?, cmd?, cols?, rows?}` action: create a window in THIS request's session,
    /// born with a shell, SELECT it, and answer with its name — tmux `new-window`.
    ///
    /// The window is created + selected under the registry lock, then its pool (now the current
    /// window's) is cloned OUT and the birth pane spawned OFF the lock — the exact
    /// [`new_session`](Self::new_session) pattern one level down, so the same death-signal and
    /// change-notification wiring applies and no fork runs under the registry lock. A runtime
    /// fork/exec failure leaves the window empty (logged, non-fatal); a malformed birth spec is
    /// rejected before anything is created.
    fn new_window(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let map = as_object(args)?;
        let name = match map.get("name") {
            None => None,
            Some(Value::String(name)) => Some(name.as_str()),
            Some(_) => return Err(InvokeError::TypeMismatch),
        };
        // Validate the birth spec BEFORE creating anything (uniform with `new_session`).
        let spec = Self::parse_spawn(map)?;
        let (created, pool) = {
            let mut registry = lock(&self.registry);
            let created = registry
                .new_window(self.scope.session(), name)
                .map_err(|error| {
                    tracing::debug!(target: "sprag_host", %error, "refused to create a window");
                    // A taken window name is well-formed and simply cannot be honored.
                    InvokeError::Rejected
                })?;
            // `new_window` selected the new window, so the scoped session's current-window pool IS
            // the new window's — clone it out to birth off-lock.
            let pool = registry
                .workspace_of(self.scope.session())
                .expect("the scoped session resolves; new_window just selected the new window");
            (created, pool)
        };
        if let Err(error) = self.spawn_parsed(&pool, spec) {
            tracing::warn!(
                target: "sprag_host",
                ?error,
                session = self.scope.session(),
                window = %created,
                "the window's birth pane could not spawn; the window was created empty",
            );
        }
        self.revision.bump();
        Ok(IntrospectValue::Json(Value::String(created)))
    }

    /// `select_window {window}` action: make a window current in THIS request's session — tmux
    /// `select-window`. Session state: every attached client follows on its next read.
    fn select_window(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let window = as_object(args)?
            .get("window")
            .and_then(Value::as_str)
            .ok_or(InvokeError::TypeMismatch)?;
        lock(&self.registry)
            .select_window(self.scope.session(), window)
            .map_err(|error| {
                tracing::debug!(target: "sprag_host", %error, "refused to select a window");
                InvokeError::Rejected
            })?;
        self.revision.bump();
        Ok(IntrospectValue::Json(Value::Null))
    }

    /// `rename_window {window?, name}` action: rename a window of THIS request's session — tmux
    /// `rename-window`. `window` absent ⇒ the current one; `name` is the new name (required).
    fn rename_window(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let map = as_object(args)?;
        let window = self.window_target(map)?.to_owned();
        let new = map
            .get("name")
            .and_then(Value::as_str)
            .ok_or(InvokeError::TypeMismatch)?;
        lock(&self.registry)
            .rename_window(self.scope.session(), &window, new)
            .map_err(|error| {
                tracing::debug!(target: "sprag_host", %error, "refused to rename a window");
                InvokeError::Rejected
            })?;
        self.revision.bump();
        Ok(IntrospectValue::Json(Value::Null))
    }

    /// `kill_window {window?}` action: kill a window of THIS request's session — tmux
    /// `kill-window`. `window` absent ⇒ the current one. Killing the session's LAST window ends
    /// the SESSION (the last session ends the daemon), handled through the SAME
    /// [`handle_session_kill`](Self::handle_session_kill) path a `kill_session` uses.
    fn kill_window(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let map = as_object(args)?;
        let window = self.window_target(map)?.to_owned();
        // Bind off-lock so the reaped panes' blocking Drop runs outside the registry lock.
        let outcome = lock(&self.registry).kill_window(self.scope.session(), &window);
        match outcome {
            Ok(WindowKillOutcome::Removed(_panes)) => {
                // A non-last window: its drained panes drop here, off-lock; wake clients watching
                // the windows list.
                self.revision.bump();
            }
            Ok(WindowKillOutcome::Session(kill)) => self.handle_session_kill(kill),
            Err(error) => {
                tracing::debug!(target: "sprag_host", %error, "refused to kill a window");
                return Err(InvokeError::Rejected);
            }
        }
        Ok(IntrospectValue::Json(Value::Null))
    }
}

impl fmt::Debug for WorkspaceExternal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkspaceExternal").finish_non_exhaustive()
    }
}

rpc_external_impl!(WorkspaceExternal);

impl ExternalIntrospect for WorkspaceExternal {
    fn schema(&self) -> IntrospectSchema {
        IntrospectSchema::new(
            const {
                &[
                    SchemaField::new(SPAWN_ACTION, "action"),
                    SchemaField::new(CLOSE_ACTION, "action"),
                    SchemaField::new(RESIZE_ACTION, "action"),
                    SchemaField::new(SET_LAYOUT_ACTION, "action"),
                    SchemaField::new(SET_FLOATING_ACTION, "action"),
                    SchemaField::new(NEW_SESSION_ACTION, "action"),
                    SchemaField::new(KILL_SESSION_ACTION, "action"),
                    SchemaField::new(NEW_WINDOW_ACTION, "action"),
                    SchemaField::new(SELECT_WINDOW_ACTION, "action"),
                    SchemaField::new(RENAME_WINDOW_ACTION, "action"),
                    SchemaField::new(KILL_WINDOW_ACTION, "action"),
                    SchemaField::new(PANES_SLOT, "list"),
                    SchemaField::new(LAYOUT_SLOT, "tree"),
                    SchemaField::new(SESSIONS_SLOT, "list"),
                    SchemaField::new(CLIENTS_SLOT, "list"),
                    SchemaField::new(WINDOWS_SLOT, "list"),
                ]
            },
        )
    }

    fn query(&self, path: &str) -> Option<IntrospectValue> {
        match path {
            PANES_SLOT => {
                let panes = lock(self.workspace()).list();
                let entries = panes
                    .iter()
                    .map(|p| {
                        let mut entry = serde_json::json!({
                            "id": p.id,
                            "cols": p.cols,
                            "rows": p.rows,
                            "command": p.command_label,
                            // The child's live OSC 0/2 window title, `null` until it sets
                            // one. A DISPLAY name (a client prefers it over the command
                            // label and falls back); never identity — the child sets it.
                            "title": p.title,
                        });
                        // The most recent attention notification (OSC 9 / 777;notify / 99),
                        // with its monotonic `seq`. ADDITIVE: the key is present only when the
                        // child has raised one, so a pane that never did is byte-identical to
                        // the pre-notification wire shape. A client detects a NEW one by the
                        // `seq` growing past the last it acknowledged (the attention badge).
                        if let Some(note) = &p.notification {
                            entry["notification"] = serde_json::json!({
                                "title": note.title,
                                "body": note.body,
                                "seq": p.notification_seq,
                            });
                        }
                        // The tmux monitor-bell count (`\a`), kept SEPARATE from the
                        // notification (a bell carries no text). ADDITIVE: present only once the
                        // child has rung one, so a pane that never did is byte-identical to the
                        // pre-bell wire shape. A viewer's "unseen attention" combines this with
                        // the notification `seq`.
                        if p.bell_seq > 0 {
                            entry["bell_seq"] = serde_json::json!(p.bell_seq);
                        }
                        // Shell-integration (OSC 133) summary: the idle/running state and the last
                        // command's exit status. ADDITIVE — the state key is present only when the
                        // child emitted a mark (`wire_str` returns `None` for `Unknown`), and the
                        // exit status only when a command finished with one, so a pane without
                        // shell integration is byte-identical to the pre-OSC133 wire shape.
                        if let Some(shell) = p.shell_state.wire_str() {
                            entry["shell"] = serde_json::json!(shell);
                        }
                        if let Some(status) = p.last_exit_status {
                            entry["exit_status"] = serde_json::json!(status);
                        }
                        entry
                    })
                    .collect();
                Some(IntrospectValue::Json(Value::Array(entries)))
            }
            // The SCOPED session's current-window LOGICAL arrangement (no pixels) + its
            // revision: what a display client projects, and what lets a reattaching client
            // restore the layout. Reconciled against the live pool first, since pane
            // lifecycle runs through the Workspace directly and the tree is not the
            // membership authority.
            LAYOUT_SLOT => {
                layout_value(crate::host::reconciled_layout(&self.registry, &self.scope)?)
            }
            // Every session, plus which one an unscoped request lands in — how a client
            // discovers what it may name in `session`. Registry-WIDE by design: this is the
            // one slot whose subject is the set of scopes, so scoping it to the caller's own
            // session would answer a question nobody asked.
            SESSIONS_SLOT => {
                // One ENRICHED builder ([`SessionRegistry::session_infos_live`]) shared with the
                // in-process arm, serialised here the way `windows` serialises its `WindowInfo`s —
                // so neither the shape NOR what `windows`/`default`/`cwd`/`branch` mean can drift
                // between the wire path and the in-process one. `_live` is two-phase (registry then
                // workspace lock, never nested) so reading each session's cwd + git branch stays off
                // the registry lock. `default` says where an UNSCOPED request lands — not "is it
                // current", nothing is current here.
                let mut infos: Vec<SessionInfo> =
                    SessionRegistry::session_infos_live(&self.registry);
                // Fill the per-session attached count HOST-side: it is dispatch-layer state the
                // registry cannot know (a session has no idea who is watching it). `None` off a
                // daemon leaves every `attached` at the structural 0. One brief lock, no nesting
                // with the registry lock (already released by `session_infos_live`).
                if let Some(attachments) = &self.attachments {
                    let attachments = lock(attachments);
                    for info in &mut infos {
                        info.attached = attachments.attached_count(&info.name);
                    }
                }
                match serde_json::to_value(&infos) {
                    Ok(json) => Some(IntrospectValue::Json(json)),
                    Err(error) => {
                        tracing::error!(target: "sprag_host", %error, "sessions failed to serialise");
                        None
                    }
                }
            }
            // Every currently-attached client and the session it views — tmux `list-clients`.
            // Registry-WIDE like `sessions` (its subject is the set of clients), and filled from
            // the SAME dispatch-layer attachment map that fills each session's `attached` count.
            // `None` off a daemon (no wire clients) serialises to an empty list — an honest "no
            // clients", the same additive story as an unattached session's absent `attached`.
            CLIENTS_SLOT => {
                let clients = match &self.attachments {
                    Some(attachments) => lock(attachments).clients(),
                    None => Vec::new(),
                };
                match serde_json::to_value(&clients) {
                    Ok(json) => Some(IntrospectValue::Json(json)),
                    Err(error) => {
                        tracing::error!(target: "sprag_host", %error, "clients failed to serialise");
                        None
                    }
                }
            }
            // The SCOPED session's windows — each window's name and whether it is the CURRENT
            // one — how a tabbed client learns which tabs to draw and which is active. Scoped
            // (unlike `sessions`): windows are a property of a session, so this answers about the
            // one the request named. `None` only if that session has since gone (a killed scope),
            // which within a single request is unreachable.
            WINDOWS_SLOT => {
                let registry = lock(&self.registry);
                let infos = registry.session(self.scope.session())?.window_infos();
                // One `WindowInfo` shape, shared with a client's mirror and the in-process arm —
                // serialised here the way the `layout` slot serialises its snapshot.
                match serde_json::to_value(&infos) {
                    Ok(json) => Some(IntrospectValue::Json(json)),
                    Err(error) => {
                        tracing::error!(target: "sprag_host", %error, "windows failed to serialise");
                        None
                    }
                }
            }
            _ => None,
        }
    }

    fn intervene(&mut self, _path: &str, _value: IntrospectValue) -> Result<(), InterveneError> {
        // No writable state slots. Pane management and the arrangement write are both
        // action-shaped (invoke): neither is a plain assignment — a spawn answers with a
        // new id, and an arrangement write names the client's dividers, validates the
        // shape, and answers with the canonical tree.
        Err(InterveneError::UnknownPath)
    }

    fn invoke(
        &mut self,
        path: &str,
        args: IntrospectValue,
    ) -> Result<IntrospectValue, InvokeError> {
        match path {
            SPAWN_ACTION => self.spawn(&args),
            CLOSE_ACTION => self.close(&args),
            RESIZE_ACTION => self.resize(&args),
            SET_LAYOUT_ACTION => self.set_layout(&args),
            SET_FLOATING_ACTION => self.set_floating(&args),
            NEW_SESSION_ACTION => self.new_session(&args),
            KILL_SESSION_ACTION => self.kill_session(&args),
            NEW_WINDOW_ACTION => self.new_window(&args),
            SELECT_WINDOW_ACTION => self.select_window(&args),
            RENAME_WINDOW_ACTION => self.rename_window(&args),
            KILL_WINDOW_ACTION => self.kill_window(&args),
            _ => Err(InvokeError::UnknownPath),
        }
    }
}

/// Serialise an arrangement for the wire — the ONE place a [`LayoutSnapshot`] becomes JSON,
/// shared by the `layout` read and both writes' answers, so a client cannot meet two shapes
/// for one fact.
///
/// A serialisation failure is unreachable (the tree's own validation rejects the non-finite
/// ratio that is the only way to author bad JSON here), but it is TRACED rather than
/// silently answered as "unknown slot" — this file's own "the swallow is honest, not
/// silent" bar.
fn layout_value(snapshot: LayoutSnapshot) -> Option<IntrospectValue> {
    match serde_json::to_value(&snapshot) {
        Ok(json) => Some(IntrospectValue::Json(json)),
        Err(error) => {
            tracing::error!(target: "sprag_host", %error, "layout failed to serialise");
            None
        }
    }
}

/// Build a [`CommandBuilder`] from an argv JSON array (`[program, args…]`),
/// returning it plus the program label. Empty or non-string argv is a
/// [`InvokeError::TypeMismatch`].
fn build_command(argv: &[Value]) -> Result<(CommandBuilder, String), InvokeError> {
    // Policy: the mux spec is a JSON `[program, args...]` string array (validated
    // here). The assembly (TERM, args, label) is the shared SSOT.
    let parts: Vec<&str> = argv
        .iter()
        .map(Value::as_str)
        .collect::<Option<_>>()
        .ok_or(InvokeError::TypeMismatch)?;
    let (program, rest) = parts.split_first().ok_or(InvokeError::TypeMismatch)?;
    Ok(sprag_terminal::command_from_parts(
        *program,
        rest.iter().copied(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use sprag_terminal::PaneId;

    /// A registry at its boot state: one session, one window, an empty pool — what the
    /// mux control surface acts on.
    fn registry() -> Arc<Mutex<SessionRegistry>> {
        Arc::new(Mutex::new(SessionRegistry::new((80, 24))))
    }

    /// The DEFAULT session's pane pool — where an unscoped surface's spawns land, so a test
    /// asserts against the same pool the surface resolves.
    fn pool(reg: &Arc<Mutex<SessionRegistry>>) -> Arc<Mutex<Workspace>> {
        Arc::clone(SessionScope::unscoped(reg).workspace())
    }

    /// The pane pool of the session named `session`.
    fn pool_of(reg: &Arc<Mutex<SessionRegistry>>, session: &str) -> Arc<Mutex<Workspace>> {
        lock(reg).workspace_of(session).expect("a real session")
    }

    /// The scope a request naming `session` resolves to — built through the REAL resolution
    /// path, so a test cannot scope a surface to something no request could produce.
    fn scope_of(reg: &Arc<Mutex<SessionRegistry>>, session: &str) -> SessionScope {
        let request = pinion_rpc::parse_request(&format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"scene/query","params":{{"session":"{session}"}}}}"#
        ))
        .expect("a well-formed request");
        SessionScope::resolve(reg, &request).expect("a session that exists")
    }

    /// A control surface over `reg` scoped to the DEFAULT session (what an unscoped request
    /// gets), sharing a fresh revision (returned so a test can
    /// assert the pane-lifecycle bumps). No `HostState` / observer is installed —
    /// [`SceneRevision::bump`] advances [`current`](SceneRevision::current) either
    /// way, which is all these tests read.
    fn control(reg: &Arc<Mutex<SessionRegistry>>) -> (WorkspaceExternal, Arc<SceneRevision>) {
        let scope = SessionScope::unscoped(reg);
        scoped_control(reg, scope)
    }

    /// A control surface scoped to `scope` — what the assembly builds for a request that
    /// named a session.
    fn scoped_control(
        reg: &Arc<Mutex<SessionRegistry>>,
        scope: SessionScope,
    ) -> (WorkspaceExternal, Arc<SceneRevision>) {
        let revision = Arc::new(SceneRevision::new());
        (
            WorkspaceExternal::new(Arc::clone(reg), scope, Arc::clone(&revision), None, None),
            revision,
        )
    }

    #[test]
    fn spawn_default_returns_first_id_and_adds_a_pane() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        let id = ext.invoke(SPAWN_ACTION, IntrospectValue::Null).unwrap();
        assert_eq!(id, IntrospectValue::Int(0));
        assert_eq!(lock(&pool(&reg)).panes().len(), 1);
    }

    #[test]
    fn spawn_with_cmd_array_sets_label() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
            .unwrap();
        assert_eq!(lock(&pool(&reg)).list()[0].command_label, "cat");
    }

    #[test]
    fn close_existing_then_missing() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        ext.invoke(SPAWN_ACTION, IntrospectValue::Null).unwrap();
        assert_eq!(
            ext.invoke(CLOSE_ACTION, IntrospectValue::Json(json!({"id": 0}))),
            Ok(IntrospectValue::Null)
        );
        assert_eq!(
            ext.invoke(CLOSE_ACTION, IntrospectValue::Json(json!({"id": 0}))),
            Err(InvokeError::Rejected)
        );
    }

    #[test]
    fn spawn_and_close_bump_the_revision() {
        // The pane-set change-notification: a spawn (set grew) and a close (set
        // shrank) each bump the revision synchronously, so a client long-polling
        // `scene/waitFor` learns the set changed WITHOUT waiting for pane output.
        // `cat` produces no output on its own, so the ONLY bumps here are the two
        // set-change bumps under test (no output on_dirty to confound the counts).
        let reg = registry();
        let (mut ext, rev) = control(&reg);
        let before = rev.current();
        ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
            .unwrap();
        assert!(
            rev.current() > before,
            "spawn bumps the revision (set grew)"
        );
        let after_spawn = rev.current();
        ext.invoke(CLOSE_ACTION, IntrospectValue::Json(json!({"id": 0})))
            .unwrap();
        assert!(
            rev.current() > after_spawn,
            "close bumps the revision (set shrank)"
        );
    }

    #[test]
    fn mux_spawned_pane_output_bumps_the_revision() {
        use std::time::{Duration, Instant};
        // The subtle half: a mux-`spawn`ed pane is wired with `bump_on_dirty`, so
        // its OWN output bumps the revision with no client input — exactly as the
        // boot pane's does. `spawn` first bumps once (set change), then the pane's
        // "hi" stdout drives its on_dirty into a further bump. Waiting for `+2` over
        // the pre-spawn baseline is race-free regardless of how the synchronous
        // set-change bump and the async output bump interleave.
        let reg = registry();
        let (mut ext, rev) = control(&reg);
        let before = rev.current();
        ext.invoke(
            SPAWN_ACTION,
            IntrospectValue::Json(json!({"cmd": ["sh", "-c", "printf hi"]})),
        )
        .unwrap();
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if rev.current() >= before + 2 {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("a mux-spawned pane's output never bumped the shared revision");
    }

    #[test]
    fn resize_requires_dims_and_targets_a_pane() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        ext.invoke(SPAWN_ACTION, IntrospectValue::Null).unwrap();
        // Missing rows -> type mismatch.
        assert_eq!(
            ext.invoke(
                RESIZE_ACTION,
                IntrospectValue::Json(json!({"id": 0, "cols": 100}))
            ),
            Err(InvokeError::TypeMismatch)
        );
        assert_eq!(
            ext.invoke(
                RESIZE_ACTION,
                IntrospectValue::Json(json!({"id": 0, "cols": 100, "rows": 30}))
            ),
            Ok(IntrospectValue::Null)
        );
        assert_eq!(
            lock(&pool(&reg))
                .pane(PaneId(0))
                .unwrap()
                .pty()
                .dimensions(),
            (100, 30)
        );
    }

    #[test]
    fn query_panes_lists_metadata() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        ext.invoke(
            SPAWN_ACTION,
            IntrospectValue::Json(json!({"cmd": ["cat"], "cols": 40, "rows": 12})),
        )
        .unwrap();
        let panes = ext.query(PANES_SLOT).unwrap();
        assert_eq!(
            panes,
            // `title` is null until the child sets an OSC 0/2 window title (R128).
            IntrospectValue::Json(
                json!([{"id": 0, "cols": 40, "rows": 12, "command": "cat", "title": null}])
            )
        );
    }

    /// The child's `OSC 2` window title reaches the WIRE (R128) — the pane-list query is
    /// how a display client (the GUI) learns it. The child emits the escape on stdout,
    /// exactly as a shell's `PROMPT_COMMAND` does.
    #[test]
    fn query_panes_reports_the_childs_osc_window_title() {
        use std::time::{Duration, Instant};
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        ext.invoke(
            SPAWN_ACTION,
            IntrospectValue::Json(
                json!({"cmd": ["sh", "-c", "printf '\\033]2;vim README\\007'"], "cols": 40, "rows": 12}),
            ),
        )
        .unwrap();

        // The reader thread applies the bytes asynchronously — poll the wire until the
        // title lands (what a client does after a `scene/waitFor` wake).
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if let Some(IntrospectValue::Json(Value::Array(entries))) = ext.query(PANES_SLOT)
                && entries.first().and_then(|pane| pane["title"].as_str()) == Some("vim README")
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("the child's OSC 2 window title never reached the pane-list wire");
    }

    /// The `layout` slot serves the CURRENT window's arrangement — and reconciles it
    /// against the live pool first. That reconcile is load-bearing: panes arrive through
    /// the `Workspace` (here via `spawn`), never through the layout, so an un-reconciled
    /// read would report an empty arrangement for a window that plainly has panes.
    #[test]
    fn query_layout_reports_the_current_windows_arrangement() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        assert_eq!(
            ext.query(LAYOUT_SLOT),
            Some(IntrospectValue::Json(
                json!({"revision": 0, "tree": {"root": null}, "floating": []})
            )),
            "an empty window has no arrangement — and the wire carries no minting counter",
        );

        ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
            .unwrap();
        ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
            .unwrap();

        let layout = query_layout(&mut ext);
        // Two spawned panes arrange as one split of leaf 0 | leaf 1 — the tree the
        // display client projects, carrying no pixels.
        assert_eq!(layout["tree"]["root"]["split"]["first"]["leaf"], 0);
        assert_eq!(layout["tree"]["root"]["split"]["second"]["leaf"], 1);
        assert_eq!(layout["tree"]["root"]["split"]["dir"], "horizontal");
        assert_eq!(layout["tree"]["root"]["split"]["ratio"], 0.5);

        // A closed pane's leaf collapses: the survivor takes the root, no half-split.
        ext.invoke(CLOSE_ACTION, IntrospectValue::Json(json!({"id": 0})))
            .unwrap();
        let layout = query_layout(&mut ext);
        assert_eq!(
            layout["tree"]["root"]["leaf"], 1,
            "the survivor reclaimed the space"
        );
    }

    /// The mux `layout` slot as JSON (the shape a client actually parses).
    fn query_layout(ext: &mut WorkspaceExternal) -> Value {
        let Some(IntrospectValue::Json(layout)) = ext.query(LAYOUT_SLOT) else {
            panic!("the layout slot answers with JSON");
        };
        layout
    }

    /// The write half through the control surface: a client's settled arrangement installs,
    /// its self-minted divider comes back NAMED, and the answer IS what the slot then
    /// serves — so one round trip records the gesture and tells the client the identity to
    /// key its per-split state on.
    #[test]
    fn set_layout_installs_a_clients_arrangement_and_names_its_divider() {
        let reg = registry();
        let (mut ext, revision) = control(&reg);
        for _ in 0..2 {
            ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
                .unwrap();
        }
        let before = revision.current();

        // The client sends a VERTICAL split at a dragged ratio, through a divider it minted
        // itself (no `id` — naming one is the host's job).
        let at = query_layout(&mut ext)["revision"]
            .as_u64()
            .expect("a revision");
        let Ok(IntrospectValue::Json(answer)) = ext.invoke(
            SET_LAYOUT_ACTION,
            IntrospectValue::Json(
                json!({ "expected_revision": at, "tree": { "root": { "split": {
                "dir": "vertical",
                "ratio": 0.75,
                "first": { "leaf": 1 },
                "second": { "leaf": 0 },
            } } } }),
            ),
        ) else {
            panic!("the write answers with JSON");
        };

        assert_eq!(answer["tree"]["root"]["split"]["dir"], "vertical");
        assert_eq!(answer["tree"]["root"]["split"]["ratio"], 0.75);
        assert_eq!(
            answer["tree"]["root"]["split"]["first"]["leaf"], 1,
            "the client's pane ORDER is the user's intent, and it stuck",
        );
        assert!(
            answer["tree"]["root"]["split"]["id"].is_number(),
            "the host NAMED the client's divider: {answer}",
        );
        assert_eq!(
            query_layout(&mut ext),
            answer,
            "the answer is what is served"
        );
        assert!(
            revision.current() > before,
            "an arrangement change wakes parked waiters, as a pane-set change does",
        );
    }

    /// Float is session state: taking a pane out of the tiling collapses its leaf HERE, so
    /// a display client renders an exact projection rather than filtering one itself.
    #[test]
    fn set_floating_takes_a_pane_out_of_the_tiling_and_puts_it_back() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        for _ in 0..2 {
            ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
                .unwrap();
        }

        let Ok(IntrospectValue::Json(answer)) = ext.invoke(
            SET_FLOATING_ACTION,
            IntrospectValue::Json(json!({ "id": 1, "floating": true })),
        ) else {
            panic!("the float write answers with JSON");
        };
        assert_eq!(
            answer["tree"]["root"]["leaf"], 0,
            "the floated pane's leaf collapsed; its sibling reclaimed the space",
        );
        // The float set is SERVED, not just applied: "absent from the tiling" is all a
        // floated pane and an unknown pane have in common, so a client that could not read
        // this would render the floated pane neither as a leaf nor as a window — it would
        // vanish. This is what lets a reattaching client restore the user's floats.
        assert_eq!(answer["floating"], json!([1]));

        // Docked back with no gesture to place it, it returns to the home the float captured
        // (`Window::set_floating`). With two panes that is also the end, so this pins the
        // shape rather than the mechanism — the home path's own guards live beside it, in
        // `sprag-terminal`.
        ext.invoke(
            SET_FLOATING_ACTION,
            IntrospectValue::Json(json!({ "id": 1, "floating": false })),
        )
        .unwrap();
        let layout = query_layout(&mut ext);
        assert_eq!(layout["tree"]["root"]["split"]["first"]["leaf"], 0);
        assert_eq!(layout["tree"]["root"]["split"]["second"]["leaf"], 1);
        assert_eq!(
            layout["floating"],
            json!([]),
            "docked back, it floats no more"
        );
    }

    /// A client cannot install an arrangement that breaks the tree's invariants: the
    /// session keeps the layout it had rather than absorbing a wrong-but-plausible one that
    /// would outlive the buggy client that sent it.
    #[test]
    fn set_layout_rejects_a_malformed_arrangement_and_keeps_the_one_in_force() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        for _ in 0..2 {
            ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
                .unwrap();
        }
        let good = query_layout(&mut ext);

        // The same pane twice, at a ratio that is not a share.
        let at = good["revision"].as_u64().expect("a revision");
        let Ok(IntrospectValue::Json(answer)) = ext.invoke(
            SET_LAYOUT_ACTION,
            IntrospectValue::Json(
                json!({ "expected_revision": at, "tree": { "root": { "split": {
                "dir": "horizontal",
                "ratio": 4.2,
                "first": { "leaf": 0 },
                "second": { "leaf": 0 },
            } } } }),
            ),
        ) else {
            panic!("a rejected write still answers with the truth to project");
        };
        assert_eq!(answer, good, "the arrangement in force is untouched");

        // A tree that does not even deserialise is a malformed REQUEST, not a bad
        // arrangement — the client and host disagree on the shape.
        assert_eq!(
            ext.invoke(
                SET_LAYOUT_ACTION,
                IntrospectValue::Json(
                    json!({ "expected_revision": at, "tree": { "root": "sideways" } })
                ),
            ),
            Err(InvokeError::TypeMismatch),
        );
        assert_eq!(
            ext.invoke(SET_LAYOUT_ACTION, IntrospectValue::Json(json!({}))),
            Err(InvokeError::TypeMismatch),
            "the tree arg is required",
        );
        assert_eq!(
            ext.invoke(
                SET_LAYOUT_ACTION,
                IntrospectValue::Json(json!({ "tree": { "root": { "leaf": 0 } } })),
            ),
            Err(InvokeError::TypeMismatch),
            "so is the revision it was authored against — a write with no answer to \
             'which layout is this about?' cannot be adjudicated",
        );
        assert_eq!(query_layout(&mut ext), good, "and still untouched");
    }

    #[test]
    fn unknown_action_is_unknown_path() {
        let (mut ext, _rev) = control(&registry());
        assert_eq!(
            ext.invoke("teleport", IntrospectValue::Null),
            Err(InvokeError::UnknownPath)
        );
    }

    // ─── C1a: the session scope ───

    /// THE reason this surface carries a scope: a spawn scoped to `work` creates a pane in
    /// WORK, and the default session never sees it.
    ///
    /// The second half is a CONTROL, and without it this proves nothing: an unscoped surface
    /// over the SAME registry must land in the default. Otherwise a surface that always
    /// spawned into `work` — or always into whichever session happened to be first — would
    /// pass the first half exactly as well.
    #[test]
    fn a_spawn_lands_in_the_session_the_request_named_and_nowhere_else() {
        let reg = registry();
        lock(&reg).new_session(Some("work")).unwrap();

        let (mut work, _rev) = scoped_control(&reg, scope_of(&reg, "work"));
        work.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
            .unwrap();
        assert_eq!(lock(&pool_of(&reg, "work")).panes().len(), 1);
        assert_eq!(
            lock(&pool(&reg)).panes().len(),
            0,
            "a spawn scoped to `work` must not reach the default session",
        );

        // The control: unscoped, same registry, lands in the default.
        let (mut default, _rev) = control(&reg);
        default
            .invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
            .unwrap();
        assert_eq!(
            lock(&pool(&reg)).panes().len(),
            1,
            "an unscoped spawn lands in the default",
        );
        assert_eq!(
            lock(&pool_of(&reg, "work")).panes().len(),
            1,
            "...and leaves `work` where it was",
        );
    }

    /// The same property for a WRITE, which is the half that would fail SILENTLY: a pane in
    /// the wrong session is at least visible, but an arrangement written to the wrong one
    /// corrupts a layout the client never named — and answers the client that it worked.
    #[test]
    fn a_layout_write_reaches_the_scoped_sessions_window_only() {
        let reg = registry();
        lock(&reg).new_session(Some("work")).unwrap();

        // Ids are minted from ONE registry-wide counter, so spawning work's pair first makes
        // them 0 and 1, and the default's 2 and 3 — distinct, which is what lets the
        // assertions below tell the two arrangements apart.
        let (mut work, _w) = scoped_control(&reg, scope_of(&reg, "work"));
        let (mut default, _d) = control(&reg);
        for _ in 0..2 {
            work.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
                .unwrap();
        }
        for _ in 0..2 {
            default
                .invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
                .unwrap();
        }
        let default_before = query_layout(&mut default);
        assert_eq!(
            default_before["tree"]["root"]["split"]["first"]["leaf"], 2,
            "the default session holds the second pair: {default_before}",
        );

        // Work's client drags its divider: vertical, 0.75, panes reversed.
        let at = query_layout(&mut work)["revision"]
            .as_u64()
            .expect("a revision");
        let Ok(IntrospectValue::Json(answer)) = work.invoke(
            SET_LAYOUT_ACTION,
            IntrospectValue::Json(
                json!({ "expected_revision": at, "tree": { "root": { "split": {
                "dir": "vertical",
                "ratio": 0.75,
                "first": { "leaf": 1 },
                "second": { "leaf": 0 },
            } } } }),
            ),
        ) else {
            panic!("the write answers with JSON");
        };
        assert_eq!(answer["tree"]["root"]["split"]["ratio"], 0.75);
        assert_eq!(
            answer["tree"]["root"]["split"]["first"]["leaf"], 1,
            "work's own gesture stuck, in work's own window: {answer}",
        );
        assert_eq!(
            query_layout(&mut default),
            default_before,
            "the default session's arrangement was never touched — not its tree, not even \
             its revision",
        );
    }

    /// A read is scoped too, and by the same seam: `layout` answers about the session the
    /// request named. A client that read another session's arrangement would project it over
    /// its own panes.
    #[test]
    fn the_layout_slot_answers_about_the_scoped_session() {
        let reg = registry();
        lock(&reg).new_session(Some("work")).unwrap();
        let (mut work, _w) = scoped_control(&reg, scope_of(&reg, "work"));
        let (mut default, _d) = control(&reg);

        // One pane in work, two in the default: the arrangements are different SHAPES, so
        // neither can be mistaken for the other.
        work.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
            .unwrap();
        for _ in 0..2 {
            default
                .invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
                .unwrap();
        }

        assert_eq!(
            query_layout(&mut work)["tree"]["root"]["leaf"],
            0,
            "work has a lone pane at its root",
        );
        let default_tree = query_layout(&mut default);
        assert_eq!(default_tree["tree"]["root"]["split"]["first"]["leaf"], 1);
        assert_eq!(default_tree["tree"]["root"]["split"]["second"]["leaf"], 2);
    }

    /// A pane of another session is not merely refused — it is not THERE. The scoped
    /// assembly builds pane children from the scoped pool alone, so scoping is structural:
    /// there is no check to forget.
    #[test]
    fn the_panes_slot_lists_only_the_scoped_sessions_panes() {
        let reg = registry();
        lock(&reg).new_session(Some("work")).unwrap();
        let (mut work, _w) = scoped_control(&reg, scope_of(&reg, "work"));
        let (mut default, _d) = control(&reg);
        work.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
            .unwrap();
        default
            .invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
            .unwrap();

        let ids = |ext: &mut WorkspaceExternal| -> Vec<u64> {
            let Some(IntrospectValue::Json(Value::Array(panes))) = ext.query(PANES_SLOT) else {
                panic!("the panes slot answers with a JSON array");
            };
            panes.iter().filter_map(|p| p["id"].as_u64()).collect()
        };
        assert_eq!(ids(&mut work), vec![0]);
        assert_eq!(ids(&mut default), vec![1]);
    }

    /// The session set is discoverable, so a client learns what it may name in `session` by
    /// ASKING rather than by guessing — and learns where an unscoped request lands.
    #[test]
    fn the_sessions_slot_lists_every_session_and_names_the_default() {
        // Compare only the STRUCTURAL discovery fields — a session's live cwd / git branch (Slice 2)
        // depend on where the birth pane happens to run and on the host's git state, which is
        // orthogonal to what this slot promises (which sessions exist, and which is the default).
        let structural = |value: Option<IntrospectValue>| -> Vec<(String, u64, bool)> {
            let Some(IntrospectValue::Json(Value::Array(items))) = value else {
                panic!("the sessions slot answers with a JSON array");
            };
            items
                .iter()
                .map(|s| {
                    (
                        s["name"].as_str().unwrap().to_owned(),
                        s["windows"].as_u64().unwrap(),
                        s["default"].as_bool().unwrap(),
                    )
                })
                .collect()
        };

        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        assert_eq!(
            structural(ext.query(SESSIONS_SLOT)),
            vec![("0".to_owned(), 1, true)],
            "at boot: one session, and it is where an unscoped request goes",
        );

        ext.invoke(
            NEW_SESSION_ACTION,
            IntrospectValue::Json(json!({"name": "work"})),
        )
        .unwrap();
        assert_eq!(
            structural(ext.query(SESSIONS_SLOT)),
            vec![("0".to_owned(), 1, true), ("work".to_owned(), 1, false)],
            "the new session is listed, and creating it moved the default for nobody",
        );

        // This slot is deliberately registry-WIDE: a WORK-scoped surface still sees EVERY
        // session, not just its own. It is the one member whose subject is the set of scopes,
        // so scoping it would answer a question nobody asked — and a client discovers what it
        // may name by asking from wherever it happens to be scoped.
        let (work, _rev) = scoped_control(&reg, scope_of(&reg, "work"));
        assert_eq!(
            work.query(SESSIONS_SLOT),
            ext.query(SESSIONS_SLOT),
            "the sessions list does not narrow to the caller's own scope",
        );
    }

    /// A name is an ADDRESS: two sessions sharing one would make a request ambiguous, so the
    /// duplicate is refused — and refused as a REJECTION, not a type error. The request was
    /// perfectly well-formed; it just cannot be honored.
    #[test]
    fn new_session_creates_by_name_and_refuses_a_duplicate() {
        let reg = registry();
        let (mut ext, revision) = control(&reg);
        let before = revision.current();

        assert_eq!(
            ext.invoke(
                NEW_SESSION_ACTION,
                IntrospectValue::Json(json!({"name": "work"})),
            ),
            Ok(IntrospectValue::Json(Value::String("work".to_owned()))),
            "the answer is the name, so a caller can scope its next request with it",
        );
        assert!(
            revision.current() > before,
            "the session SET changed, which a watching client must be woken for",
        );
        assert!(lock(&reg).session("work").is_some());

        let after = revision.current();
        assert_eq!(
            ext.invoke(
                NEW_SESSION_ACTION,
                IntrospectValue::Json(json!({"name": "work"})),
            ),
            Err(InvokeError::Rejected),
            "a taken name is a refusal, not a TypeMismatch — the request was well-formed",
        );
        assert_eq!(
            lock(&reg).sessions().len(),
            2,
            "the refused create added nothing"
        );
        assert_eq!(
            revision.current(),
            after,
            "and a refused create is inert — it must not even move the revision",
        );

        // A present name must be a STRING — a non-string is a malformed request, never
        // silently allocated (the scope param's own type-error corner).
        assert_eq!(
            ext.invoke(
                NEW_SESSION_ACTION,
                IntrospectValue::Json(json!({"name": 42}))
            ),
            Err(InvokeError::TypeMismatch),
        );
    }

    /// An ABSENT name is no longer an error: it asks the registry to ALLOCATE the lowest free
    /// one (tmux's `new-session` with no `-s`), and the answer tells the caller what it got —
    /// the only way it can learn a name it did not choose.
    #[test]
    fn an_unnamed_new_session_over_the_wire_allocates_the_lowest_free_name() {
        let reg = registry();
        let (mut ext, revision) = control(&reg);
        let before = revision.current();

        // The boot session is "0", so the first allocation is "1".
        assert_eq!(
            ext.invoke(NEW_SESSION_ACTION, IntrospectValue::Json(json!({}))),
            Ok(IntrospectValue::Json(Value::String("1".to_owned()))),
            "no name asks the registry to allocate the lowest free one",
        );
        assert!(
            revision.current() > before,
            "an allocation is a real create — a watching client must be woken",
        );

        // And the next one is "2": each allocation is its own independent session.
        assert_eq!(
            ext.invoke(NEW_SESSION_ACTION, IntrospectValue::Json(json!({}))),
            Ok(IntrospectValue::Json(Value::String("2".to_owned()))),
        );
        assert_eq!(
            lock(&reg).sessions().len(),
            3,
            "the boot session, plus 1 and 2"
        );
    }

    /// tmux's `new-session`: a created session is BORN with one pane — never empty — and the
    /// birth pane takes the request's `cmd`/`cols`/`rows` (tmux's `new-session -x -y command`),
    /// so a client's first pane is exactly what it asked for. The DEFAULT session is untouched: a
    /// create births a pane in the NEW session, never the default (the daemon's empty anchor).
    #[test]
    fn a_new_session_is_born_with_a_shell() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        assert_eq!(
            ext.invoke(
                NEW_SESSION_ACTION,
                IntrospectValue::Json(
                    json!({"name": "work", "cmd": ["cat"], "cols": 40, "rows": 12})
                ),
            ),
            Ok(IntrospectValue::Json(Value::String("work".to_owned()))),
        );
        assert_eq!(
            lock(&pool_of(&reg, "work")).panes().len(),
            1,
            "a session is born with exactly one pane (tmux new-session), never empty",
        );
        // The birth pane runs the request's cmd at its size — the caller's first pane, exact.
        let (work, _w) = scoped_control(&reg, scope_of(&reg, "work"));
        assert_eq!(
            work.query(PANES_SLOT),
            Some(IntrospectValue::Json(json!([
                {"id": 0, "cols": 40, "rows": 12, "command": "cat", "title": null}
            ]))),
            "the birth pane runs the request's cmd at its size",
        );
        assert!(
            lock(&pool(&reg)).panes().is_empty(),
            "the default session is untouched — a create births a pane in the NEW session",
        );
    }

    /// A session born via `new_session` feeds the reaper when its birth pane dies — the guard
    /// that the birth pane is HOOKED with the death-signal. This is WHY the birth spawn lives at
    /// the host layer (which carries [`on_pane_exit`](WorkspaceExternal::on_pane_exit)) and NOT
    /// in the pinion-free registry: a registry-side birth would be the unhooked-pane category
    /// R160/R161 closed, leaving a daemon lingering over a dead session. Driven through the real
    /// action, so a refactor that moved the spawn off the hooked path would fail here.
    #[test]
    fn a_new_session_born_pane_feeds_the_reaper() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::{Duration, Instant};

        let reg = registry();
        let fired = Arc::new(AtomicUsize::new(0));
        let f = Arc::clone(&fired);
        let on_empty: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            f.fetch_add(1, Ordering::SeqCst);
        });
        let signal = crate::spawn_reaper(Arc::clone(&reg), on_empty);
        let mut ext = WorkspaceExternal::new(
            Arc::clone(&reg),
            SessionScope::unscoped(&reg),
            Arc::new(SceneRevision::new()),
            Some(signal),
            None,
        );
        // The birth pane EXITS immediately and is the only pane in the registry (the default "0"
        // is empty), so its death must self-clean the daemon — which happens ONLY if the pane
        // fed the reaper.
        ext.invoke(
            NEW_SESSION_ACTION,
            IntrospectValue::Json(json!({"cmd": ["true"]})),
        )
        .expect("new_session births a pane");
        let start = Instant::now();
        while fired.load(Ordering::SeqCst) == 0 && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            fired.load(Ordering::SeqCst),
            1,
            "the birth pane's death self-cleans the daemon, proving it was hooked — a \
             registry-side birth would feed no reaper and leave a lingering daemon",
        );
    }

    /// A MALFORMED birth spec is rejected BEFORE the session is created — uniform with the `spawn`
    /// action and with a bad `name`. Birthing at this authority buys uniform validation; a `cmd`
    /// that is not an array, or a non-`u16` `cols`/`rows`, must not slip through as a silently
    /// empty session reported as success.
    #[test]
    fn new_session_rejects_a_malformed_birth_spec_and_creates_nothing() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        for bad in [
            json!({"cmd": 42}),
            json!({"name": "work", "cmd": "cat"}),
            json!({"cols": 0}),
        ] {
            assert_eq!(
                ext.invoke(NEW_SESSION_ACTION, IntrospectValue::Json(bad.clone())),
                Err(InvokeError::TypeMismatch),
                "a malformed birth spec is a type error, not a silent empty session: {bad}",
            );
        }
        assert_eq!(
            lock(&reg).sessions().len(),
            1,
            "every rejected create built nothing — only the boot session remains",
        );
    }

    /// A RUNTIME birth failure (an argv the OS cannot `exec`) is non-fatal: the session is still
    /// created (a valid attach target, merely empty) and answered with its name — so "a well-formed
    /// create with a free name succeeds" stays total. This is the ONE case a created session is
    /// empty, distinct from a malformed request (which creates nothing — the guard above).
    #[test]
    fn a_new_session_whose_birth_pane_cannot_exec_is_created_empty() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        assert_eq!(
            ext.invoke(
                NEW_SESSION_ACTION,
                IntrospectValue::Json(json!({
                    "name": "work",
                    "cmd": ["/nonexistent/definitely-not-a-real-binary"],
                })),
            ),
            Ok(IntrospectValue::Json(Value::String("work".to_owned()))),
            "a well-formed create with a free name succeeds even when the birth pane cannot exec",
        );
        assert!(
            lock(&reg).session("work").is_some(),
            "the session exists as a valid attach target",
        );
        assert!(
            lock(&pool_of(&reg, "work")).panes().is_empty(),
            "...but it is empty: the birth pane's exec failed, non-fatally",
        );
    }

    /// Killing a NON-last session over the wire removes it and answers `null`, and — a set
    /// change — wakes a client watching the sessions list.
    #[test]
    fn kill_session_removes_a_non_last_session_over_the_wire() {
        let reg = registry();
        lock(&reg).new_session(Some("work")).unwrap();
        let (mut ext, revision) = control(&reg);
        let before = revision.current();

        assert_eq!(
            ext.invoke(
                KILL_SESSION_ACTION,
                IntrospectValue::Json(json!({"name": "work"})),
            ),
            Ok(IntrospectValue::Json(Value::Null)),
        );
        assert!(lock(&reg).session("work").is_none(), "the session is gone");
        assert_eq!(lock(&reg).sessions().len(), 1, "only the default remains");
        assert!(
            revision.current() > before,
            "the session set changed, which a watching client must be woken for",
        );

        // An unknown name is a REJECTION, not a type error; a missing / non-string name IS a
        // type error (you must name the session to kill).
        assert_eq!(
            ext.invoke(
                KILL_SESSION_ACTION,
                IntrospectValue::Json(json!({"name": "ghost"})),
            ),
            Err(InvokeError::Rejected),
        );
        assert_eq!(
            ext.invoke(KILL_SESSION_ACTION, IntrospectValue::Json(json!({}))),
            Err(InvokeError::TypeMismatch),
        );
        assert_eq!(
            ext.invoke(
                KILL_SESSION_ACTION,
                IntrospectValue::Json(json!({"name": 42})),
            ),
            Err(InvokeError::TypeMismatch),
        );
    }

    /// Killing the LAST session ENDS the daemon: it fires the injected death-signal (the same
    /// one a pane's exit does), so the reaper re-checks liveness and exits through the SIGTERM
    /// funnel. Proven with a recording signal — the empty last session drains nothing, so this
    /// firing, not a pane Drop, is what triggers the exit there.
    #[test]
    fn kill_session_on_the_last_ends_the_daemon_via_the_death_signal() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let reg = registry(); // the only session, "0", empty
        let fired = Arc::new(AtomicUsize::new(0));
        let signal: Arc<dyn Fn() + Send + Sync> = {
            let fired = Arc::clone(&fired);
            Arc::new(move || {
                fired.fetch_add(1, Ordering::SeqCst);
            })
        };
        let mut ext = WorkspaceExternal::new(
            Arc::clone(&reg),
            SessionScope::unscoped(&reg),
            Arc::new(SceneRevision::new()),
            Some(signal),
            None,
        );

        assert_eq!(
            ext.invoke(
                KILL_SESSION_ACTION,
                IntrospectValue::Json(json!({"name": "0"})),
            ),
            Ok(IntrospectValue::Json(Value::Null)),
        );
        assert_eq!(
            fired.load(Ordering::SeqCst),
            1,
            "killing the last session fired the daemon-exit signal exactly once",
        );
        assert_eq!(
            lock(&reg).sessions().len(),
            1,
            "the last session is NOT removed — the shell is kept so the default stays total",
        );

        // Off a daemon (no injected signal), the same kill still removes/drains but nothing
        // exits — exactly right for a GUI's in-process host and the tests.
        let (mut headless, _rev) = control(&reg);
        assert_eq!(
            headless.invoke(
                KILL_SESSION_ACTION,
                IntrospectValue::Json(json!({"name": "0"})),
            ),
            Ok(IntrospectValue::Json(Value::Null)),
            "a last-session kill with no reaper is a no-op exit, not an error",
        );
    }

    // ─── windows: new / select / rename / kill over the wire ───

    /// `new_window` over the surface creates a window in the SCOPED session, SELECTS it, births a
    /// shell into it, and answers with its name — the birth landing in the NEW window, not the one
    /// the request was scoped to.
    #[test]
    fn new_window_creates_a_selected_window_born_with_a_pane() {
        let reg = registry();
        let (mut ext, rev) = control(&reg); // scoped to session "0", window "0"
        let before = rev.current();

        let created = ext
            .invoke(
                NEW_WINDOW_ACTION,
                IntrospectValue::Json(json!({"cmd": ["cat"]})),
            )
            .unwrap();
        assert_eq!(
            created,
            IntrospectValue::Json(Value::String("1".to_owned())),
            "the lowest free window name",
        );
        assert!(rev.current() > before, "a window creation wakes waiters");

        // The windows slot (read fresh) shows two, the new one current.
        assert_eq!(
            ext.query(WINDOWS_SLOT),
            Some(IntrospectValue::Json(json!([
                {"name": "0", "current": false},
                {"name": "1", "current": true},
            ]))),
        );

        // The birth pane landed in the NEW window: a fresh scope to the session (now current =
        // "1") sees exactly one pane, while the request's own window "0" is still empty.
        let (fresh, _r) = scoped_control(&reg, scope_of(&reg, "0"));
        let Some(IntrospectValue::Json(Value::Array(panes))) = fresh.query(PANES_SLOT) else {
            panic!("the panes slot answers with a JSON array");
        };
        assert_eq!(panes.len(), 1, "the new window is born with its shell");
        assert!(
            lock(ext.workspace()).panes().is_empty(),
            "and the window the request was scoped to is untouched",
        );
    }

    /// `select_window` moves the session's current window (a set-ish change that wakes waiters),
    /// and a missing / unknown target is refused with the current window left put.
    #[test]
    fn select_window_moves_the_current_window() {
        let reg = registry();
        let (mut ext, rev) = control(&reg);
        lock(&reg).new_window("0", Some("logs")).unwrap(); // current is now "logs"
        let before = rev.current();

        assert_eq!(
            ext.invoke(
                SELECT_WINDOW_ACTION,
                IntrospectValue::Json(json!({"window": "0"}))
            ),
            Ok(IntrospectValue::Json(Value::Null)),
        );
        assert!(rev.current() > before, "a select wakes waiters to re-read");
        assert_eq!(
            ext.query(WINDOWS_SLOT),
            Some(IntrospectValue::Json(json!([
                {"name": "0", "current": true},
                {"name": "logs", "current": false},
            ]))),
        );

        // A target is required, and an unknown one is a rejection (well-formed, unhonorable).
        assert_eq!(
            ext.invoke(SELECT_WINDOW_ACTION, IntrospectValue::Json(json!({}))),
            Err(InvokeError::TypeMismatch),
        );
        assert_eq!(
            ext.invoke(
                SELECT_WINDOW_ACTION,
                IntrospectValue::Json(json!({"window": "ghost"}))
            ),
            Err(InvokeError::Rejected),
        );
    }

    /// `rename_window` renames the CURRENT window by default (`window` absent ⇒ the scope's), and
    /// a rename onto a name another window holds is refused.
    #[test]
    fn rename_window_renames_the_current_by_default_and_refuses_a_duplicate() {
        let reg = registry();
        let (mut ext, _r) = control(&reg); // scope window "0"

        // window absent ⇒ the current window ("0") is renamed.
        assert_eq!(
            ext.invoke(
                RENAME_WINDOW_ACTION,
                IntrospectValue::Json(json!({"name": "main"}))
            ),
            Ok(IntrospectValue::Json(Value::Null)),
        );
        lock(&reg).new_window("0", Some("logs")).unwrap();
        // Renaming "logs" onto the taken name "main" is refused.
        assert_eq!(
            ext.invoke(
                RENAME_WINDOW_ACTION,
                IntrospectValue::Json(json!({"window": "logs", "name": "main"})),
            ),
            Err(InvokeError::Rejected),
        );
        // The rename took and "logs" kept its name; the slot reads the session fresh, so "current"
        // reflects reality ("logs", which new_window selected).
        assert_eq!(
            ext.query(WINDOWS_SLOT),
            Some(IntrospectValue::Json(json!([
                {"name": "main", "current": false},
                {"name": "logs", "current": true},
            ]))),
        );
    }

    /// Killing a NON-last window over the wire removes it, keeps the current window valid, and
    /// wakes a client watching the windows list.
    #[test]
    fn kill_window_removes_a_non_last_window_over_the_wire() {
        let reg = registry();
        let (mut ext, rev) = control(&reg);
        lock(&reg).new_window("0", Some("logs")).unwrap(); // current = "logs"; two windows
        let before = rev.current();

        assert_eq!(
            ext.invoke(
                KILL_WINDOW_ACTION,
                IntrospectValue::Json(json!({"window": "logs"}))
            ),
            Ok(IntrospectValue::Json(Value::Null)),
        );
        assert!(rev.current() > before, "a window kill wakes waiters");
        assert_eq!(
            ext.query(WINDOWS_SLOT),
            Some(IntrospectValue::Json(
                json!([{"name": "0", "current": true}])
            )),
            "logs is gone and the current fell back to the surviving window",
        );
        assert_eq!(
            ext.invoke(
                KILL_WINDOW_ACTION,
                IntrospectValue::Json(json!({"window": "ghost"}))
            ),
            Err(InvokeError::Rejected),
        );
    }

    /// Killing a session's LAST window ends the SESSION (tmux) — the escalation removes the whole
    /// session when it is not the last one.
    #[test]
    fn kill_window_on_a_sessions_last_window_ends_the_session() {
        let reg = registry();
        lock(&reg).new_session(Some("work")).unwrap(); // "work" holds one window "0"
        assert_eq!(lock(&reg).sessions().len(), 2);

        let (mut work, _r) = scoped_control(&reg, scope_of(&reg, "work"));
        assert_eq!(
            work.invoke(
                KILL_WINDOW_ACTION,
                IntrospectValue::Json(json!({"window": "0"}))
            ),
            Ok(IntrospectValue::Json(Value::Null)),
        );
        assert!(
            lock(&reg).session("work").is_none(),
            "the session went with its last window",
        );
        assert_eq!(lock(&reg).sessions().len(), 1, "only the default remains");
    }

    /// Killing the last window of the LAST session ends the DAEMON: the escalation reaches
    /// `kill_session`'s last-session arm, firing the injected death-signal so the reaper exits
    /// through the SIGTERM funnel — the same path a `kill_session` of the last session takes.
    #[test]
    fn kill_window_on_the_last_window_of_the_last_session_ends_the_daemon() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let reg = registry(); // one session "0", one window "0", empty
        let fired = Arc::new(AtomicUsize::new(0));
        let signal: Arc<dyn Fn() + Send + Sync> = {
            let fired = Arc::clone(&fired);
            Arc::new(move || {
                fired.fetch_add(1, Ordering::SeqCst);
            })
        };
        let mut ext = WorkspaceExternal::new(
            Arc::clone(&reg),
            SessionScope::unscoped(&reg),
            Arc::new(SceneRevision::new()),
            Some(signal),
            None,
        );

        assert_eq!(
            ext.invoke(
                KILL_WINDOW_ACTION,
                IntrospectValue::Json(json!({"window": "0"}))
            ),
            Ok(IntrospectValue::Json(Value::Null)),
        );
        assert_eq!(
            fired.load(Ordering::SeqCst),
            1,
            "the last window's kill escalated to a last-session kill and fired the exit signal once",
        );
        assert_eq!(
            lock(&reg).sessions().len(),
            1,
            "the last session is drained, not removed — the default stays total",
        );
    }

    /// The `windows` slot is SCOPED: a session sees only its OWN windows, with the current one
    /// marked — the read a tabbed client draws from.
    #[test]
    fn the_windows_slot_lists_the_scoped_sessions_windows_and_marks_current() {
        let reg = registry();
        lock(&reg).new_session(Some("work")).unwrap();
        lock(&reg).new_window("work", Some("logs")).unwrap(); // work: "0", "logs"(current)

        let (default_ext, _d) = control(&reg);
        assert_eq!(
            default_ext.query(WINDOWS_SLOT),
            Some(IntrospectValue::Json(
                json!([{"name": "0", "current": true}])
            )),
            "the default session sees only its own one window",
        );

        let (work, _w) = scoped_control(&reg, scope_of(&reg, "work"));
        assert_eq!(
            work.query(WINDOWS_SLOT),
            Some(IntrospectValue::Json(json!([
                {"name": "0", "current": false},
                {"name": "logs", "current": true},
            ]))),
            "work sees its two windows, logs current",
        );
    }

    /// The per-window-revision bound (d): a `set_layout` that NAMES a window other than the one
    /// the request is scoped to is refused — the belt to the revision compare-and-set's suspenders,
    /// so a client that switched windows cannot land a stale write on the wrong one whose revision
    /// happened to collide. The CONTROL is the same gesture naming the scoped window, which applies.
    #[test]
    fn set_layout_refuses_a_write_authored_against_a_different_window() {
        let reg = registry();
        let (mut ext, _r) = control(&reg); // scope.window() == "0"
        for _ in 0..2 {
            ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
                .unwrap();
        }
        let good = query_layout(&mut ext);
        let at = good["revision"].as_u64().expect("a revision");
        let gesture = |window: &str| {
            json!({
                "expected_revision": at,
                "expected_window": window,
                "tree": { "root": { "split": {
                    "dir": "vertical", "ratio": 0.75, "first": { "leaf": 1 }, "second": { "leaf": 0 },
                } } },
            })
        };

        // Naming a window OTHER than the scoped one is refused: the arrangement in force is
        // untouched, and the answer is that truth for the client to re-project.
        let Ok(IntrospectValue::Json(answer)) = ext.invoke(
            SET_LAYOUT_ACTION,
            IntrospectValue::Json(gesture("elsewhere")),
        ) else {
            panic!("a refused write still answers with the truth");
        };
        assert_eq!(
            answer, good,
            "a window mismatch refused it; window 0 kept its arrangement"
        );

        // Control: naming the ACTUAL scoped window lets the SAME gesture through.
        let Ok(IntrospectValue::Json(answer)) =
            ext.invoke(SET_LAYOUT_ACTION, IntrospectValue::Json(gesture("0")))
        else {
            panic!("the accepted write answers with JSON");
        };
        assert_eq!(
            answer["tree"]["root"]["split"]["ratio"], 0.75,
            "naming the current window applied the gesture",
        );
        assert_eq!(
            answer["tree"]["root"]["split"]["first"]["leaf"], 1,
            "the order stuck"
        );
    }

    /// The window actions refuse a NON-STRING arg where the ABI promises a string — never a silent
    /// fall-back (the same aliasing corner the session scope param refuses).
    #[test]
    fn window_actions_reject_non_string_args() {
        let reg = registry();
        let (mut ext, _r) = control(&reg);
        for _ in 0..2 {
            ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
                .unwrap();
        }
        let at = query_layout(&mut ext)["revision"]
            .as_u64()
            .expect("a revision");

        // set_layout: a non-string `expected_window` is malformed, like a wrong-typed
        // `expected_revision` — not a silent "no window check".
        assert_eq!(
            ext.invoke(
                SET_LAYOUT_ACTION,
                IntrospectValue::Json(json!({
                    "expected_revision": at, "expected_window": 42, "tree": { "root": null },
                })),
            ),
            Err(InvokeError::TypeMismatch),
        );
        // rename / kill: a non-string `window` target is malformed (never a silent fall-back to
        // the current window and acting on the wrong one).
        assert_eq!(
            ext.invoke(
                RENAME_WINDOW_ACTION,
                IntrospectValue::Json(json!({ "window": 42, "name": "x" })),
            ),
            Err(InvokeError::TypeMismatch),
        );
        assert_eq!(
            ext.invoke(
                KILL_WINDOW_ACTION,
                IntrospectValue::Json(json!({ "window": 42 }))
            ),
            Err(InvokeError::TypeMismatch),
        );
        // new_window / select_window: a non-string name / target is malformed too.
        assert_eq!(
            ext.invoke(
                NEW_WINDOW_ACTION,
                IntrospectValue::Json(json!({ "name": 42 }))
            ),
            Err(InvokeError::TypeMismatch),
        );
        assert_eq!(
            ext.invoke(
                SELECT_WINDOW_ACTION,
                IntrospectValue::Json(json!({ "window": 42 })),
            ),
            Err(InvokeError::TypeMismatch),
        );
    }
}
