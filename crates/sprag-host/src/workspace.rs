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
//! * `new_session {name?, cmd?, cols?, rows?, remote?}` → creates a session BORN with one pane (absent
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
use std::path::Path;
use std::sync::{Arc, Mutex};

use pinion_core::external::{
    ExternalIntrospect, InterveneError, IntrospectSchema, IntrospectValue, InvokeError, RawJson,
    SchemaField,
};
use serde_json::{Map, Value};
use sprag_terminal::{
    CommandBuilder, KillOutcome, LayoutSnapshot, LayoutWire, PaneId, SessionInfo, SessionRegistry,
    SplitDir, SplitSide, SshRemote, WindowKillOutcome, Workspace,
};

use crate::bump_on_dirty;
use crate::external::{as_object, lock, opt_dim, require_pane_id, rpc_external_impl};
use crate::notify::ChannelRegistry;
use crate::scope::SessionScope;

// The mux control action names + query slots are the shared wire ABI vocabulary
// ([`crate::wire`]) — the SAME consts a client addresses for pane lifecycle.
use crate::wire::{
    BREAK_PANE_ACTION, CLIENTS_SLOT, CLOSE_ACTION, DROP_FILE_ACTION, GLOBAL_COMMANDS_SLOT,
    GRID_WORK_SLOT, JOIN_PANE_ACTION, KILL_SESSION_ACTION, KILL_WINDOW_ACTION, LAYOUT_SLOT,
    NEW_SESSION_ACTION, NEW_WINDOW_ACTION, PANES_SLOT, PROJECT_FIELD, RENAME_WINDOW_ACTION,
    RESIZE_ACTION, SELECT_WINDOW_ACTION, SESSIONS_SLOT, SET_FLOATING_ACTION, SET_LAYOUT_ACTION,
    SPAWN_ACTION, SPLIT_ACTION, WINDOWS_SLOT,
};

/// The mux-management engine `External`: a control surface over the shared
/// [`SessionRegistry`]. Holds `Arc<Mutex<SessionRegistry>>` so its `scene/invoke`
/// handlers mutate the live pane pool of the CURRENT window (which the serve loop also
/// reads to assemble the scene) and its `layout` slot can serve that window's
/// arrangement, plus the per-session change channels ([`ChannelRegistry`]) so a pane-lifecycle
/// mutation wakes the `scene/waitFor` replies parked on the session it happened in.
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
    /// The per-session change channels ([`crate::HostState`]'s). Two roles: each pane this surface
    /// SPAWNS is wired with a `bump_on_dirty` hook over ITS SESSION's token (so its output wakes
    /// the waits on that session, like the boot pane), and a spawn / close announces on that
    /// session directly (so a pane-set change wakes a waiter before the new pane's first output).
    /// The whole registry is cloned per scene-assembly rather than one session's token, because
    /// two actions announce somewhere other than this request's scope — a `new_session` births a
    /// pane in a session that did not exist when the scope was resolved, and a `kill_session`
    /// closes one.
    channels: Arc<ChannelRegistry>,
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
    /// The structured remote endpoint for a `sprag ssh` birth pane, stamped onto the pane after the
    /// spawn so the host can reconnect it on restore and `scp` to it. `None` for an ordinary spawn.
    remote: Option<SshRemote>,
}

impl WorkspaceExternal {
    /// Build the control surface over the shared mux registry, the session it is scoped to,
    /// the shared scene-version token, and the daemon's `on_pane_exit` death-signal (`None` off
    /// a daemon) — see the struct docs for each field's role.
    #[must_use]
    pub fn new(
        registry: Arc<Mutex<SessionRegistry>>,
        scope: SessionScope,
        channels: Arc<ChannelRegistry>,
        on_pane_exit: Option<Arc<dyn Fn() + Send + Sync>>,
        attachments: Option<Arc<Mutex<crate::AttachmentRegistry>>>,
    ) -> Self {
        Self {
            registry,
            scope,
            channels,
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

    /// Announce that the SCOPED session changed: advance its token, waking the `scene/waitFor`
    /// replies parked on that session and no others.
    ///
    /// Every action here acts on the session the request named, so the scope IS the answer to
    /// "whose scene moved" — which is why this takes no argument. The two places that announce
    /// somewhere ELSE ([`new_session`](Self::new_session), which births a pane in a session that
    /// did not exist when this scope was resolved, and [`kill_session`](Self::kill_session), which
    /// ends one) say so explicitly rather than through this.
    fn announce(&self) {
        self.channels.bump(self.scope.session());
    }

    /// Parse + VALIDATE the `{cmd?, cols?, rows?}` spawn spec — the REQUEST-validation half of a
    /// spawn, pool-free so it runs before anything is built. A malformed field (`cmd` present but
    /// not an array, a non-`u16` `cols`/`rows`) is a `TypeMismatch` HERE, the same refusal the
    /// `spawn` action and `new_session`'s own `name` field give a type error — so a `new_session`
    /// can reject a malformed birth spec before it creates the session. `cmd` (an argv array)
    /// defaults to the user's [`default-command`](crate::options::DEFAULT_COMMAND), then to `$SHELL`
    /// ([`default_pane_command`](crate::config::default_pane_command)); `cols`/`rows` left `None` take
    /// the pool's default size at spawn.
    fn parse_spawn(map: &Map<String, Value>) -> Result<SpawnSpec, InvokeError> {
        let (command, label) = match map.get("cmd") {
            // The user's `default-command`, falling through to `$SHELL` — one resolver for every
            // birth, so a setting cannot be honoured by some spawn paths and not others.
            None => crate::config::default_pane_command(),
            Some(Value::Array(argv)) => build_command(argv)?,
            Some(_) => return Err(InvokeError::TypeMismatch),
        };
        Ok(SpawnSpec {
            command,
            label,
            cols: opt_dim(map, "cols")?,
            rows: opt_dim(map, "rows")?,
            remote: Self::parse_remote(map)?,
        })
    }

    /// Parse the OPTIONAL `remote` object (`{host, user?, port?}`) a `sprag ssh` birth request
    /// carries — the structured endpoint that marks the pane a sanctioned remote workspace. Absent
    /// is `None` (an ordinary spawn); present-but-malformed (no string `host`, a non-string `user`,
    /// or a `port` outside `1..=65535`) is a `TypeMismatch`, validated before anything is built.
    fn parse_remote(map: &Map<String, Value>) -> Result<Option<SshRemote>, InvokeError> {
        let Some(value) = map.get("remote") else {
            return Ok(None);
        };
        let obj = value.as_object().ok_or(InvokeError::TypeMismatch)?;
        let host = obj
            .get("host")
            .and_then(Value::as_str)
            .filter(|host| !host.is_empty())
            .ok_or(InvokeError::TypeMismatch)?
            .to_owned();
        let user = match obj.get("user") {
            None | Some(Value::Null) => None,
            Some(Value::String(user)) => Some(user.clone()),
            Some(_) => return Err(InvokeError::TypeMismatch),
        };
        let port = match obj.get("port") {
            None | Some(Value::Null) => None,
            Some(value) => {
                let port = value.as_u64().ok_or(InvokeError::TypeMismatch)?;
                let port = u16::try_from(port).map_err(|_| InvokeError::TypeMismatch)?;
                if port == 0 {
                    return Err(InvokeError::TypeMismatch);
                }
                Some(port)
            }
        };
        Ok(Some(SshRemote { user, host, port }))
    }

    /// Fork/exec a validated [`SpawnSpec`] into `pool` — the RUNTIME half, shared by the `spawn`
    /// action and a [`new_session`](Self::new_session)'s birth pane.
    ///
    /// Wired WITH the change-notification hook (so this pane's output bumps the SAME revision the
    /// boot pane's does — a client's `scene/waitFor` wakes on it exactly as on the boot pane) and,
    /// under a daemon, the `on_pane_exit` death-signal (so THIS pane's death feeds the reaper). A
    /// fork/exec failure is `Rejected` — a WELL-FORMED request the OS could not honor (a broken
    /// `$SHELL`, an argv it cannot `exec`), DISTINCT from the malformed request
    /// [`parse_spawn`](Self::parse_spawn) already rejected. Does NOT bump the revision — the caller signals its set change once (a
    /// plain `spawn`, or the create that births this pane), so the two never double-bump or drift.
    fn spawn_parsed(
        &self,
        pool: &Arc<Mutex<Workspace>>,
        spec: SpawnSpec,
    ) -> Result<PaneId, InvokeError> {
        let SpawnSpec {
            command,
            label,
            cols,
            rows,
            remote,
        } = spec;
        let on_exit = self.on_pane_exit.as_ref().map(crate::pane_exit_hook);
        let mut workspace = lock(pool);
        let (default_cols, default_rows) = workspace.default_size();
        let id = workspace
            .spawn_with_dirty(
                command,
                label,
                cols.unwrap_or(default_cols),
                rows.unwrap_or(default_rows),
                Some(bump_on_dirty(&self.channels.revision(self.scope.session()))),
                on_exit,
            )
            .map_err(|_| InvokeError::Rejected)?;
        // Stamp the remote endpoint onto the just-born pane (metadata the process does not need),
        // so a restore reconnects it and a dropped-file upload knows its `scp` target.
        if let Some(remote) = remote {
            workspace.set_pane_remote(id, remote);
        }
        Ok(id)
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
        self.announce();
        Ok(IntrospectValue::Int(
            i64::try_from(id.0).unwrap_or(i64::MAX),
        ))
    }

    /// `split {pane, dir, before?, cmd?, cols?, rows?, remote?}` action: divide `pane` and spawn
    /// the new one into the half that opens, answering with its id — see
    /// [`crate::wire::SPLIT_ACTION`].
    ///
    /// Ordered PRE-FLIGHT, spawn, place. The pre-flight is what lets a caller's mistake — a pane
    /// id that names nothing, a floating pane, another window's — be refused with no child
    /// forked, which matters because the alternative is to fork the user's shell and then have to
    /// kill it. The placement is what actually decides the outcome, and it is checked again there
    /// because the two cannot be one atomic step: the spawn needs the workspace lock and the
    /// placement needs the registry's, and this codebase never nests them.
    ///
    /// If the target exits in that window — between the pre-flight and the placement — the pane
    /// is already born and lands APPENDED instead, and the id is still returned. Killing a shell
    /// the user just asked for would be the worse answer, and the arrangement is readable
    /// ([`crate::wire::LAYOUT_SLOT`]), so the outcome is reported rather than guessed at. It is
    /// the same degradation an unhonorable [`LeafHome`](sprag_terminal::LeafHome) already takes.
    fn split(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let map = as_object(args)?;
        let target = require_pane_id(map, "pane")?;
        let dir = match map.get("dir").and_then(Value::as_str) {
            Some("horizontal") => SplitDir::Horizontal,
            Some("vertical") => SplitDir::Vertical,
            _ => return Err(InvokeError::TypeMismatch),
        };
        // Absent is the common side (right / below); a non-bool is malformed rather than
        // silently defaulted, the same rule every other optional flag on this external follows.
        let side = match map.get("before") {
            None | Some(Value::Null) | Some(Value::Bool(false)) => SplitSide::Second,
            Some(Value::Bool(true)) => SplitSide::First,
            Some(_) => return Err(InvokeError::TypeMismatch),
        };
        // The birth spec is validated BEFORE the target is looked up, so a request that is
        // malformed in two ways reports the malformed-request error rather than the refusal.
        let spec = Self::parse_spawn(map)?;
        if !crate::host::tiled_panes(&self.registry, &self.scope).contains(&target) {
            return Err(InvokeError::Rejected);
        }
        let id = self.spawn_parsed(self.workspace(), spec)?;
        if !crate::host::split_pane(&self.registry, &self.scope, id, target, side, dir) {
            tracing::warn!(
                target: "sprag_host",
                %id,
                %target,
                session = self.scope.session(),
                "the split's target left the tiling while its pane was being born; appended it",
            );
        }
        // Both the pane set and the arrangement changed: one announce covers both, exactly as a
        // plain spawn's does for the set alone.
        self.announce();
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
            self.announce();
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
        // The display's cell pixel geometry, OPTIONAL: a GUI client sends it (its font metric) so
        // the PTY winsize and XTWINOPS pixel reports are truthful; a headless / older client omits
        // it and `(0, 0)` leaves the pane's last-known cell geometry untouched.
        let cell_px = (
            opt_dim(map, "cell_width")?.unwrap_or(0),
            opt_dim(map, "cell_height")?.unwrap_or(0),
        );
        match lock(self.workspace()).resize(id, cols, rows, cell_px) {
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
        self.announce();
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
        self.announce();
        layout_value(snapshot).ok_or(InvokeError::Rejected)
    }

    /// `new_session {name?, cmd?, cols?, rows?, remote?}` action: create a session BORN WITH A
    /// SHELL, answering with its name.
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
        // fork/exec that would otherwise stall every other request on the registry lock.
        //
        // The session is EMPTY until the spawn below lands, and an empty session reads as "nothing
        // live" to the daemon's reaper — so an unrelated last pane dying in that gap used to end
        // the daemon under the client that had just asked for the session. A [`BirthPin`] taken
        // under THIS lock (never after it, which would leave the same gap narrower) says a pane is
        // on its way; it is released when `pin` falls at the end of this call, spawned or not, and
        // its release nudges the reaper so a birth that FAILED still lets an idle daemon go.
        let (allocated, pool, pin) = {
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
            let pin = crate::BirthPin::taken(
                &self.registry,
                &mut registry,
                self.on_pane_exit.as_ref().map(crate::pane_exit_hook),
            );
            (allocated, pool, pin)
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
        // Two sessions changed, and only one of them is this request's scope. The NEW session now
        // holds a live pane — announced on its OWN channel, because a client that asked for it will
        // scope its next wait there and would otherwise sleep through its own birth pane's first
        // output. The scoped session's list of sessions grew, which is a change to what IT shows.
        self.channels.bump(&allocated);
        self.announce();
        // Explicit, because the ORDER matters and a lexical drop would not say so: the claim must
        // outlive the spawn above (that is its whole job) and must fall before this call answers,
        // so the client's next request meets a daemon whose liveness is settled either way.
        drop(pin);
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
            KillOutcome::Removed(removed) => {
                // CLOSE the dead session's channel before announcing on this one. A client parked
                // on `scene/waitFor` for the session that just went is waiting on a token nothing
                // can ever advance again — no pane of it survives to produce output and no request
                // will be scoped to it — so closing is what releases it to re-read, meet the scope
                // refusal, and detach. The name comes off the removed session itself rather than
                // from the caller's argument: the last-window escalation reaches here with no name
                // in hand, and one of the two paths guessing would be the one that leaked.
                self.channels.close(removed.name());
                self.announce();
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
        self.announce();
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
        self.announce();
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
        self.announce();
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
                self.announce();
            }
            Ok(WindowKillOutcome::Session(kill)) => self.handle_session_kill(kill),
            Err(error) => {
                tracing::debug!(target: "sprag_host", %error, "refused to kill a window");
                return Err(InvokeError::Rejected);
            }
        }
        Ok(IntrospectValue::Json(Value::Null))
    }

    /// `break_pane {pane, name?}` action: move a pane out of its window into a NEW window of THIS
    /// request's session (born current), and answer with the new window's name — tmux `break-pane`.
    ///
    /// The pane's SOURCE window is derived from its id in the registry (a `PaneId` is unique across
    /// the registry), so the wire carries only the pane and an optional new-window name. The move
    /// is whole (no re-spawn) and runs under the registry lock; nothing blocks (a break drops no
    /// pane). Every client watching the windows list wakes on the revision bump.
    fn break_pane(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let map = as_object(args)?;
        let pane = require_pane_id(map, "pane")?;
        let name = match map.get("name") {
            None => None,
            Some(Value::String(name)) => Some(name.as_str()),
            Some(_) => return Err(InvokeError::TypeMismatch),
        };
        let created = lock(&self.registry)
            .break_pane(self.scope.session(), pane, name)
            .map_err(|error| {
                tracing::debug!(target: "sprag_host", %error, "refused to break a pane out");
                // A rejection is well-formed but cannot be honored (last pane, taken name, no such
                // pane) — the same shape a refused window op reports.
                InvokeError::Rejected
            })?;
        self.announce();
        Ok(IntrospectValue::Json(Value::String(created)))
    }

    /// `join_pane {pane, window}` action: move a pane into another window of THIS request's session,
    /// appending it as a tiled leaf — tmux `join-pane`. Answers `{closed_source}` (whether the join
    /// emptied and closed the pane's old window).
    ///
    /// The pane's SOURCE window is derived from its id; the wire carries the pane and the
    /// DESTINATION window's name. Whole move, under the registry lock; the revision bump wakes every
    /// client (a closed source window drops out of their windows list on the next read).
    fn join_pane(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let map = as_object(args)?;
        let pane = require_pane_id(map, "pane")?;
        let dst = map
            .get("window")
            .and_then(Value::as_str)
            .ok_or(InvokeError::TypeMismatch)?
            .to_owned();
        let closed = lock(&self.registry)
            .join_pane(self.scope.session(), pane, &dst)
            .map_err(|error| {
                tracing::debug!(target: "sprag_host", %error, "refused to join a pane");
                InvokeError::Rejected
            })?;
        self.announce();
        Ok(IntrospectValue::Json(
            serde_json::json!({ "closed_source": closed }),
        ))
    }

    /// `drop_file {pane, path}` action: deliver a file dropped on a display client to `pane`, and
    /// answer `{path}` — the path the pane is handed ([`crate::upload`] owns the paste-vs-upload
    /// policy). A refused delivery (no such pane, an unresolvable path) is `Rejected`.
    ///
    /// The pane is resolved to its PTY handle + recorded remote under the workspace lock, and the
    /// guard is dropped BEFORE the delivery runs: an upload spawns a thread and a local drop writes
    /// to the PTY, neither of which may hold the pool other clients are reading.
    ///
    /// No revision bump: the pane's own output (its shell echoing the pasted path) bumps it through
    /// the spawn-time dirty hook, and an upload's paste lands long after this returns — a bump here
    /// would announce a change that has not happened yet.
    fn drop_file(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let map = as_object(args)?;
        let pane = require_pane_id(map, "pane")?;
        let path = map
            .get("path")
            .and_then(Value::as_str)
            .ok_or(InvokeError::TypeMismatch)?;
        let target = {
            let workspace = lock(self.workspace());
            let pane = workspace.pane(pane).ok_or(InvokeError::Rejected)?;
            (pane.handle(), pane.remote().cloned())
        };
        let (handle, remote) = target;
        let delivered =
            crate::upload::deliver(handle, remote, Path::new(path)).ok_or(InvokeError::Rejected)?;
        Ok(IntrospectValue::Json(
            serde_json::json!({ "path": delivered }),
        ))
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
                    SchemaField::new(SPLIT_ACTION, "action"),
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
                    SchemaField::new(BREAK_PANE_ACTION, "action"),
                    SchemaField::new(JOIN_PANE_ACTION, "action"),
                    SchemaField::new(DROP_FILE_ACTION, "action"),
                    SchemaField::new(PANES_SLOT, "list"),
                    SchemaField::new(LAYOUT_SLOT, "tree"),
                    SchemaField::new(SESSIONS_SLOT, "list"),
                    SchemaField::new(CLIENTS_SLOT, "list"),
                    SchemaField::new(GRID_WORK_SLOT, "object"),
                    SchemaField::new(WINDOWS_SLOT, "list"),
                    SchemaField::new(GLOBAL_COMMANDS_SLOT, "object"),
                    PROJECT_FIELD,
                ]
            },
        )
    }

    fn query(&self, path: &str) -> Option<IntrospectValue> {
        match path {
            PANES_SLOT => {
                // The DTOs and each pane's PROJECTION TOKEN, read under ONE workspace lock so the
                // token a client compares describes the same moment as the rest of its entry. A
                // token read later than the pane list could only ever be NEWER than the frame the
                // client goes on to fetch, which is the one direction that serves a stale pane.
                let (panes, tokens) = {
                    let guard = lock(self.workspace());
                    let tokens: std::collections::HashMap<u64, sprag_grid::ProjectionToken> = guard
                        .panes()
                        .iter()
                        .map(|pane| {
                            (
                                pane.id().0,
                                pane.pty().with_screen_palette(sprag_grid::projection_token),
                            )
                        })
                        .collect();
                    (guard.list(), tokens)
                };
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
                        // Whether the pane's child has EXITED. ADDITIVE and one-way: the key is
                        // present only once it is true (a live pane is byte-identical to the
                        // pre-liveness wire shape), and a pane never comes back to life, so a
                        // client that has seen it needs no re-check.
                        if p.dead {
                            entry["dead"] = serde_json::json!(true);
                        }
                        // ...and HOW it exited, once the child has been reaped. A SECOND key rather
                        // than a richer `dead`, because it is a second fact that arrives later and
                        // may never arrive at all (a child that hands its pty on and lingers) — see
                        // `PaneExit`. ADDITIVE on the same terms, and `code` is always written while
                        // `signal` rides only a signalled death, so a plain exit stays two fields.
                        //
                        // `child_exit`, not `exit`, to keep it clear of `exit_status` below: that
                        // one is the OSC 133 status of the last command the SHELL ran, and the two
                        // answer opposite questions about a stopped pane.
                        if let Some(exit) = &p.child_exit {
                            let mut value = serde_json::json!({ "code": exit.code });
                            if let Some(signal) = &exit.signal {
                                value["signal"] = serde_json::json!(signal);
                            }
                            entry["child_exit"] = value;
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
                        // Mouse-tracking mode (DECSET 1000/1002/1003). ADDITIVE — the key is present
                        // only while the child is tracking (`wire_str` returns `None` for the
                        // resting `MouseProtocol::None`), so a pane that never enabled mouse
                        // reporting is byte-identical to the pre-mouse wire shape. A display client
                        // reads it to decide whether to capture the pointer for reporting.
                        if let Some(mouse) = p.mouse_protocol.wire_str() {
                            entry["mouse"] = serde_json::json!(mouse);
                        }
                        // Focus-tracking mode (DECSET 1004). ADDITIVE — the key is present only while
                        // the child is tracking focus, so a pane that never enabled it is
                        // byte-identical to the pre-focus wire shape (mirrors `mouse`). A display
                        // client reads it to decide whether to emit a focus edge on a pane focus
                        // change; an agent reads it to learn the app reacts to focus.
                        if p.focus_tracking {
                            entry["focus_tracking"] = serde_json::json!(true);
                        }
                        // OSC 52 clipboard signals. The write SEQ travels here (ADDITIVE, present
                        // only once the child has written a clipboard); the write PAYLOAD does NOT
                        // — it can be a whole paste, so a client fetches it on demand off this seq
                        // via the `clipboard_write` pane slot. A pane whose child never wrote is
                        // byte-identical to the pre-OSC52 wire shape.
                        if p.clipboard_write_seq > 0 {
                            entry["clipboard_write_seq"] = serde_json::json!(p.clipboard_write_seq);
                        }
                        // A pending OSC 52 READ query: the single selection the child asked to
                        // read back (`c`/`p`) + its seq. Tiny, so — unlike the write — it travels
                        // inline. ADDITIVE: present only once the child has issued a read.
                        if let Some(query) = p.clipboard_query {
                            entry["clipboard_query"] = serde_json::json!({
                                "sel": query.target.osc_char().to_string(),
                                "seq": p.clipboard_query_seq,
                            });
                        }
                        // Inline images (Kitty graphics / Sixel, R1404). ADDITIVE: present only once
                        // the child has transmitted one, so an image-less pane is byte-identical to
                        // the pre-R1404 wire shape. Each entry is a SUMMARY — `{id, width, height,
                        // anchor, seq}`, NO rgba (R1404 Stage 5): the RGBA is up to a MiB, so a
                        // display client fetches it ON DEMAND via `image_data.<id>` keyed on
                        // `(id, seq)`, not per poll (the `clipboard_write` payload precedent). An
                        // agent reads the summary here to learn "a WxH image sits at cell (col,row)".
                        if !p.images.is_empty() {
                            let images: Vec<Value> = p
                                .images
                                .iter()
                                .map(|img| {
                                    serde_json::json!({
                                        "id": img.id,
                                        "width": img.width,
                                        "height": img.height,
                                        "anchor": [img.anchor.0, img.anchor.1],
                                        "seq": img.seq,
                                    })
                                })
                                .collect();
                            entry["images"] = Value::Array(images);
                        }
                        // What a fetch of this pane's CELLS would depend on
                        // ([`sprag_grid::ProjectionToken`]) — the per-row damage stamps, the
                        // cursor (colour included), the screen kind, the width and the history
                        // depth. A display client that already holds a frame for this pane and
                        // sees an unchanged token can SKIP the fetch: the frame it would receive
                        // is the one it has. ADDITIVE, and its absence means "fetch anyway", so an
                        // older daemon — or a token that failed to serialize — costs a redundant
                        // fetch and never a stale pane.
                        if let Some(token) =
                            tokens.get(&p.id).and_then(|t| serde_json::to_value(t).ok())
                        {
                            entry["projection"] = token;
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
                // Drop the resting empty anchor (and any paneless, unattached session): a human
                // session list shows working sessions + those a client is viewing, matching
                // `tmux ls` at rest. Applied HERE, after `attached` is filled, because that is the
                // one place both facts the rule needs are known (see `SessionInfo::is_listable`).
                infos.retain(SessionInfo::is_listable);
                encoded_answer(&infos, "sessions")
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
                encoded_answer(&clients, "clients")
            }
            // What this host has paid to project its cells. Read straight off the meter rather
            // than recomputed, and UNSCOPED on purpose: the counters are process-wide, so scoping
            // them to the request's session would name a session for work every session shares.
            // Serialised by hand rather than through the type, so the wire keys are spelled once
            // in the place the schema declares them.
            GRID_WORK_SLOT => {
                let work = sprag_grid::work();
                Some(IntrospectValue::Json(serde_json::json!({
                    "projections_total": work.projections_total,
                    "cells_total": work.cells_total,
                })))
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
                encoded_answer(&infos, "windows")
            }
            // The USER's own declared commands — no pane, no session, no scope: this answer is the
            // same for every request the host serves, which is exactly why it is a fixed slot beside
            // the parametric project one rather than a variant of it.
            GLOBAL_COMMANDS_SLOT => Some(global_commands_value()),
            // The project governing ONE pane: the commands its `.sprag.toml` declares. Parametric,
            // so it is matched after the fixed slots above (`project.<pane>`, see `PROJECT_FIELD`
            // for why this lives on the mux external rather than the pane's own).
            path => {
                let pane = path.strip_prefix("project.")?.parse::<u64>().ok()?;
                Some(project_value(self.workspace(), PaneId(pane)))
            }
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
            SPLIT_ACTION => self.split(&args),
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
            BREAK_PANE_ACTION => self.break_pane(&args),
            JOIN_PANE_ACTION => self.join_pane(&args),
            DROP_FILE_ACTION => self.drop_file(&args),
            _ => Err(InvokeError::UnknownPath),
        }
    }
}

/// The `project.<pane>` answer for one pane: the commands the project it sits in declares.
///
/// Three outcomes, each distinct on the wire (see [`PROJECT_FIELD`]): `null` for a pane in no
/// project — or one whose cwd is not local, or that has since gone; the project object for a
/// usable config; and
/// `{error}` for a project whose config is unusable, because a typo must be reported rather than
/// look like "this project declares nothing".
///
/// The workspace lock is taken ONLY to read the two facts the registry owns (is this pane remote,
/// and where is it) and is DROPPED before the filesystem walk. A config read is IO — holding a
/// registry lock across it would stall every other request behind someone else's slow disk, the
/// same discipline the file-drop upload follows by handing its thread only a PTY handle.
fn project_value(workspace: &Arc<Mutex<Workspace>>, pane: PaneId) -> IntrospectValue {
    let cwd = {
        let pool = lock(workspace);
        let Some(pane) = pool.pane(pane) else {
            // A pane the caller named that this window does not hold (closed, or another window's).
            return IntrospectValue::Null;
        };
        if pane.remote().is_some() {
            // A REMOTE workspace's working directory is on another machine, so walking THIS
            // filesystem for a `.sprag.toml` would either find nothing or — worse — find the local
            // project the daemon happens to sit in and offer its commands for a remote shell.
            return IntrospectValue::Null;
        }
        pane.pty().cwd()
    };
    let Some(cwd) = cwd else {
        // No readable cwd: the child has exited, or the platform has no `/proc`.
        return IntrospectValue::Null;
    };
    match crate::project::load(&cwd) {
        None => IntrospectValue::Null,
        Some(Ok(project)) => encoded_answer(&project, "project").unwrap_or(IntrospectValue::Null),
        Some(Err(error)) => IntrospectValue::Json(serde_json::json!({
            "error": error.to_string(),
        })),
    }
}

/// Serialise the USER's declared commands for the wire — the same three-way answer
/// [`project_value`] gives, over the user's config instead of a pane's project: `null` for "no
/// config written", an `{error}` object for one that cannot be used, and the config itself
/// otherwise.
///
/// The error is RENDERED here rather than sent structurally, exactly as the project's is, because
/// what a client needs is the sentence to show — and rendering it host-side is what makes it name
/// `config.toml` ([`crate::ConfigError`]) rather than whichever file the client guessed.
fn global_commands_value() -> IntrospectValue {
    match crate::config::load() {
        None => IntrospectValue::Null,
        Some(Ok(config)) => encoded_answer(&config, "user config").unwrap_or(IntrospectValue::Null),
        Some(Err(error)) => IntrospectValue::Json(serde_json::json!({
            "error": error.to_string(),
        })),
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
    encoded_answer(&snapshot, "layout")
}

/// Answer `subject` with JSON text encoded ONCE, or trace the failure and answer absence.
///
/// The ONE place this file turns a serialisable answer into an [`IntrospectValue`], for the
/// reason `crate::pane`'s `cells` arm states: [`IntrospectValue::Raw`] (pinion R1480, delivering
/// PINION-PR79) carries text the producer already holds and `scene/query` splices it into the
/// reply, so nothing here builds a `serde_json::Value` tree for the dispatch to walk and encode a
/// second time. **The wire bytes do not change** — only how many times they are produced.
///
/// [`RawJson::encode`] rather than [`IntrospectValue::raw`] on purpose: the convenience
/// constructor degrades a failure to `Null` SILENTLY, and every caller here already had an error
/// channel it was using. Keeping the `Result` keeps this file's "the swallow is honest, not
/// silent" bar — the trace names which answer failed, and absence stays distinguishable from a
/// present-but-empty one.
fn encoded_answer<T: ?Sized + serde::Serialize>(
    value: &T,
    subject: &str,
) -> Option<IntrospectValue> {
    match RawJson::encode(value) {
        Ok(raw) => Some(IntrospectValue::Raw(raw)),
        Err(error) => {
            tracing::error!(target: "sprag_host", %error, subject, "answer failed to serialise");
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
    use pinion_core::SceneRevision;
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
    /// gets), plus the SCOPED session's token (returned so a test can assert the pane-lifecycle
    /// bumps). The token is read out of the channels by NAME, which is also what a test asserting
    /// a bump has to do now: a bump lands on one session's counter, so reading "the revision"
    /// without saying whose would be reading the wrong one.
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
        let channels = Arc::new(ChannelRegistry::default());
        let revision = channels.revision(scope.session());
        (
            WorkspaceExternal::new(Arc::clone(reg), scope, channels, None, None),
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
    fn resize_threads_the_optional_cell_metric_to_the_pane() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        ext.invoke(SPAWN_ACTION, IntrospectValue::Null).unwrap();
        // A GUI client sends cell_width/cell_height (its font metric) — they reach the emulator.
        assert_eq!(
            ext.invoke(
                RESIZE_ACTION,
                IntrospectValue::Json(
                    json!({"id": 0, "cols": 100, "rows": 30, "cell_width": 9, "cell_height": 18})
                )
            ),
            Ok(IntrospectValue::Null)
        );
        assert_eq!(
            lock(&pool(&reg))
                .pane(PaneId(0))
                .unwrap()
                .pty()
                .cell_pixel_size(),
            (9, 18),
            "the invoke's cell metric reaches the pane's emulator"
        );
        // A resize WITHOUT the metric (a headless client) is still accepted and preserves it.
        assert_eq!(
            ext.invoke(
                RESIZE_ACTION,
                IntrospectValue::Json(json!({"id": 0, "cols": 80, "rows": 24}))
            ),
            Ok(IntrospectValue::Null)
        );
        assert_eq!(
            lock(&pool(&reg))
                .pane(PaneId(0))
                .unwrap()
                .pty()
                .cell_pixel_size(),
            (9, 18),
            "a metric-less resize preserves the last-known cell geometry"
        );
    }

    /// The pane-list entries with the per-pane PROJECTION TOKEN lifted out — so a test can assert
    /// the stable wire shape exactly, while the token (whose `row_generations` are as long as the
    /// pane is tall) is checked for what it must contain rather than transcribed.
    fn panes_without_projection(value: Option<IntrospectValue>) -> (Value, Vec<Value>) {
        let IntrospectValue::Json(Value::Array(entries)) = value.expect("a pane list") else {
            panic!("the pane list is a JSON array");
        };
        let mut tokens = Vec::new();
        let stripped: Vec<Value> = entries
            .into_iter()
            .map(|mut entry| {
                let token = entry
                    .as_object_mut()
                    .and_then(|map| map.remove("projection"))
                    .expect("every pane entry carries a projection token");
                tokens.push(token);
                entry
            })
            .collect();
        (Value::Array(stripped), tokens)
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
        let (panes, tokens) = panes_without_projection(ext.query(PANES_SLOT));
        assert_eq!(
            panes,
            // `title` is null until the child sets an OSC 0/2 window title (R128).
            json!([{"id": 0, "cols": 40, "rows": 12, "command": "cat", "title": null}])
        );
        // ...and the token beside it describes the pane a client would fetch: one damage stamp per
        // row, at the pane's own width. A client compares it whole; this asserts it is not a stub.
        let token: sprag_grid::ProjectionToken =
            serde_json::from_value(tokens[0].clone()).expect("the token round-trips");
        assert_eq!(token.cols, 40);
        assert_eq!(token.row_generations.len(), 12);
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

    /// A child's live INPUT-MODE state — mouse tracking (DECSET 1000/1002/1003) and focus tracking
    /// (DECSET 1004) — reaches the pane-list WIRE, ADDITIVELY: the keys appear only once the child
    /// has enabled them. This is the producer→wire path the agent-facing `list_panes` MCP tool reads;
    /// `query_panes_lists_metadata` proves the resting shape carries NEITHER key.
    #[test]
    fn query_panes_reports_the_childs_mouse_and_focus_tracking() {
        use std::time::{Duration, Instant};
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        // A raw child that enables button-event mouse tracking (1002) AND focus reporting (1004).
        ext.invoke(
            SPAWN_ACTION,
            IntrospectValue::Json(json!({
                "cmd": ["sh", "-c", "stty raw -echo 2>/dev/null; printf '\\033[?1002h\\033[?1004h'; cat"],
                "cols": 40, "rows": 12,
            })),
        )
        .unwrap();

        // The reader thread applies the bytes asynchronously — poll the wire until both land.
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if let Some(IntrospectValue::Json(Value::Array(entries))) = ext.query(PANES_SLOT)
                && let Some(pane) = entries.first()
                && pane.get("mouse").and_then(Value::as_str) == Some("button")
                && pane.get("focus_tracking").and_then(Value::as_bool) == Some(true)
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("the child's DECSET 1002/1004 modes never reached the pane-list wire");
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
            answer_doc(ext.query(LAYOUT_SLOT)),
            json!({"revision": 0, "tree": {"root": null}, "floating": []}),
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

    /// The DOCUMENT a structural answer carries, whichever way it is encoded.
    ///
    /// A `Raw` answer and a `Json` answer holding the same document are INDISTINGUISHABLE on the
    /// wire — the dispatch splices one and serialises the other to the same bytes — so a test
    /// whose subject is the CONTENT must not accidentally also be an assertion about which
    /// encoding produced it. Every content test below reads through here; the one test whose
    /// subject IS the encoding asserts the variant directly and is named for it
    /// (`a_structural_answer_is_encoded_text_not_a_dom`).
    fn answer_doc(value: Option<IntrospectValue>) -> Value {
        match value.expect("the slot answers") {
            IntrospectValue::Json(v) => v,
            IntrospectValue::Raw(raw) => raw.to_value().expect("an encoded answer is valid JSON"),
            other => panic!("the slot answered a non-structural value: {other:?}"),
        }
    }

    /// [`answer_doc`]'s sibling for the WRITE channel: the layout writes answer with the
    /// arrangement now in force, built by the same `layout_value`, so they carry the same
    /// encoding and their tests must read it the same way.
    fn write_doc(result: Result<IntrospectValue, InvokeError>) -> Value {
        answer_doc(Some(result.expect("the write answers")))
    }

    /// THE ENCODING, which every content test above is deliberately blind to.
    ///
    /// `answer_doc` reads through both variants BECAUSE the wire cannot tell them apart — which
    /// is the point of the change and also what makes it invisible: revert the consumption and
    /// every other assertion in this file still passes. So the claim "this file's structural
    /// answers are encoded once, not built as a `serde_json::Value` for the dispatch to walk and
    /// encode again" gets the one test that can fail for it.
    #[test]
    fn a_structural_answer_is_encoded_text_not_a_dom() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
            .unwrap();

        for slot in [SESSIONS_SLOT, CLIENTS_SLOT, WINDOWS_SLOT, LAYOUT_SLOT] {
            let answer = ext.query(slot).expect("the slot answers");
            assert!(
                answer.as_raw().is_some(),
                "`{slot}` still builds a serde_json::Value DOM for the dispatch to re-encode; \
                 the answer was {answer:?}",
            );
        }
    }

    /// The reason these callers use [`RawJson::encode`] and not the `IntrospectValue::raw`
    /// convenience: the convenience degrades a failure to `Null` SILENTLY, and every caller here
    /// had an error channel worth keeping. A value serde_json refuses (a map whose keys are not
    /// strings) answers ABSENCE — distinguishable from a present-but-empty `Null` — and is traced.
    #[test]
    fn an_unserialisable_answer_is_absent_rather_than_a_silent_null() {
        let refused: std::collections::BTreeMap<[u8; 2], u8> = [([1, 2], 3)].into_iter().collect();
        assert!(
            encoded_answer(&refused, "a test value").is_none(),
            "a refusal must not be spelled as a present answer",
        );
        // The control: an ordinary value DOES answer, so the assertion above is about the
        // serialization failure and not about the helper never answering at all.
        assert!(encoded_answer(&json!({"ok": true}), "a test value").is_some());
    }

    /// The mux `layout` slot as JSON (the shape a client actually parses).
    fn query_layout(ext: &mut WorkspaceExternal) -> Value {
        answer_doc(ext.query(LAYOUT_SLOT))
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
        let answer = write_doc(ext.invoke(
            SET_LAYOUT_ACTION,
            IntrospectValue::Json(
                json!({ "expected_revision": at, "tree": { "root": { "split": {
                "dir": "vertical",
                "ratio": 0.75,
                "first": { "leaf": 1 },
                "second": { "leaf": 0 },
            } } } }),
            ),
        ));

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

        let answer = write_doc(ext.invoke(
            SET_FLOATING_ACTION,
            IntrospectValue::Json(json!({ "id": 1, "floating": true })),
        ));
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
        let answer = write_doc(ext.invoke(
            SET_LAYOUT_ACTION,
            IntrospectValue::Json(
                json!({ "expected_revision": at, "tree": { "root": { "split": {
                "dir": "horizontal",
                "ratio": 4.2,
                "first": { "leaf": 0 },
                "second": { "leaf": 0 },
            } } } }),
            ),
        ));
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
        let answer = write_doc(work.invoke(
            SET_LAYOUT_ACTION,
            IntrospectValue::Json(
                json!({ "expected_revision": at, "tree": { "root": { "split": {
                "dir": "vertical",
                "ratio": 0.75,
                "first": { "leaf": 1 },
                "second": { "leaf": 0 },
            } } } }),
            ),
        ));
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

    /// The `dead` key is ADDITIVE and appears exactly when the child has exited: absent while it
    /// runs (so a live pane's entry is byte-identical to the pre-liveness wire shape), present and
    /// `true` afterwards. That key is the only thing on the wire that distinguishes a finished
    /// command from a hung one — the pane itself stays either way.
    ///
    /// REVERT-PROOF: emit the key unconditionally and the live assertion fails; drop the emission
    /// and the exited one does.
    #[test]
    fn the_panes_slot_reports_a_dead_child_additively() {
        let reg = registry();
        let (mut ext, _guard) = control(&reg);
        // A child that exits at once, beside one that stays up.
        ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
            .unwrap();
        ext.invoke(
            SPAWN_ACTION,
            IntrospectValue::Json(json!({"cmd": ["true"]})),
        )
        .unwrap();

        let entry = |ext: &mut WorkspaceExternal, id: u64| -> Value {
            let Some(IntrospectValue::Json(Value::Array(panes))) = ext.query(PANES_SLOT) else {
                panic!("the panes slot answers with a JSON array");
            };
            panes
                .into_iter()
                .find(|p| p["id"].as_u64() == Some(id))
                .expect("the pane is listed")
        };

        // Wait on the CONDITION the assertion reads, never on a timer.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while entry(&mut ext, 1).get("dead").is_none() {
            assert!(
                std::time::Instant::now() < deadline,
                "the short-lived child never reported dead"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(entry(&mut ext, 1)["dead"], json!(true));
        assert!(
            entry(&mut ext, 1)["id"].as_u64() == Some(1),
            "and it is still LISTED — a dead pane keeps its place"
        );
        assert!(
            entry(&mut ext, 0).get("dead").is_none(),
            "the live pane carries no key at all: {:?}",
            entry(&mut ext, 0)
        );
    }

    /// ...and HOW it exited follows, on its own key, once the host has reaped the child.
    ///
    /// A SECOND wait rather than an assertion folded into the one above, because the two facts are
    /// published at different moments: `dead` lands when the output stream ends, `child_exit` when
    /// `waitpid` returns. Asserting them together would be a race, and would also be a lie about
    /// what the wire promises.
    ///
    /// REVERT-PROOF: drop the `child_exit` emission and this never converges; emit it for a live
    /// pane too and the last assertion fails.
    #[test]
    fn the_panes_slot_reports_how_the_child_exited_once_it_is_reaped() {
        let reg = registry();
        let (mut ext, _guard) = control(&reg);
        // One child that FAILS with a code worth reading, beside one that stays up.
        ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
            .unwrap();
        ext.invoke(
            SPAWN_ACTION,
            IntrospectValue::Json(json!({"cmd": ["sh", "-c", "exit 7"]})),
        )
        .unwrap();

        let entry = |ext: &mut WorkspaceExternal, id: u64| -> Value {
            let Some(IntrospectValue::Json(Value::Array(panes))) = ext.query(PANES_SLOT) else {
                panic!("the panes slot answers with a JSON array");
            };
            panes
                .into_iter()
                .find(|p| p["id"].as_u64() == Some(id))
                .expect("the pane is listed")
        };

        // Wait on the CONDITION the assertion reads — the reap, not the EOF that precedes it.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while entry(&mut ext, 1).get("child_exit").is_none() {
            assert!(
                std::time::Instant::now() < deadline,
                "the failing child was never reaped: {:?}",
                entry(&mut ext, 1)
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(
            entry(&mut ext, 1)["child_exit"],
            json!({ "code": 7 }),
            "the code travels, and `signal` is absent for a process that returned normally",
        );
        assert_eq!(
            entry(&mut ext, 1)["dead"],
            json!(true),
            "and the liveness bit is still there — the status refines it, never replaces it",
        );
        assert!(
            entry(&mut ext, 0).get("child_exit").is_none(),
            "a live pane carries no status: {:?}",
            entry(&mut ext, 0)
        );
    }

    /// The session set is discoverable, so a client learns what it may name in `session` by
    /// ASKING rather than by guessing — and learns where an unscoped request lands. A RESTING
    /// empty anchor is HIDDEN (no pane, nobody attached), matching `tmux ls` at rest while the
    /// daemon lives on; see [`SessionInfo::is_listable`].
    #[test]
    fn the_sessions_slot_lists_working_sessions_and_hides_the_resting_anchor() {
        // Compare only the STRUCTURAL discovery fields — a session's live cwd / git branch (Slice 2)
        // depend on where the birth pane happens to run and on the host's git state, which is
        // orthogonal to what this slot promises (which sessions exist, and which is the default).
        let structural = |value: Option<IntrospectValue>| -> Vec<(String, u64, bool)> {
            let Value::Array(items) = answer_doc(value) else {
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
        // At boot the only session is the empty anchor "0": no pane, nobody attached, so the human
        // list is empty — the phantom "0" that `sprag ls` used to print at rest is gone.
        assert_eq!(
            structural(ext.query(SESSIONS_SLOT)),
            Vec::<(String, u64, bool)>::new(),
            "the resting empty anchor is not listed",
        );

        // A pane makes the default session real: now it lists, and it still names itself default.
        ext.invoke(SPAWN_ACTION, IntrospectValue::Null).unwrap();
        assert_eq!(
            structural(ext.query(SESSIONS_SLOT)),
            vec![("0".to_owned(), 1, true)],
            "a session holding a pane lists, and it is where an unscoped request goes",
        );

        ext.invoke(
            NEW_SESSION_ACTION,
            IntrospectValue::Json(json!({"name": "work"})),
        )
        .unwrap();
        assert_eq!(
            structural(ext.query(SESSIONS_SLOT)),
            vec![("0".to_owned(), 1, true), ("work".to_owned(), 1, false)],
            "the new session (born with a pane) is listed; creating it moved the default for nobody",
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

    /// The tmux-SUPERIOR half of the listing rule: an EMPTY session a client is attached to still
    /// lists (so the client can see where it is), even though it holds no pane. tmux cannot reach
    /// this state at all — killing the last pane there destroys the session — so honestly showing it
    /// is a refinement, not a divergence. Proves the filter reads the HOST-filled `attached`, not
    /// just the registry's pane count.
    #[test]
    fn an_empty_session_a_client_is_attached_to_still_lists() {
        let session_names = |value: Option<IntrospectValue>| -> Vec<String> {
            let Value::Array(items) = answer_doc(value) else {
                panic!("the sessions slot answers with a JSON array");
            };
            items
                .iter()
                .map(|s| s["name"].as_str().unwrap().to_owned())
                .collect()
        };

        let reg = registry(); // the only session is the empty anchor "0"
        let attachments = Arc::new(Mutex::new(crate::AttachmentRegistry::default()));
        {
            let mut a = lock(&attachments);
            let conn = pinion_rpc::ConnId::allocate();
            a.hello(conn, "gui".to_owned());
            a.attach(conn, "0".to_owned()); // a client is viewing the empty anchor
        }
        let ext = WorkspaceExternal::new(
            Arc::clone(&reg),
            SessionScope::unscoped(&reg),
            Arc::new(ChannelRegistry::default()),
            None,
            Some(attachments),
        );
        assert_eq!(
            session_names(ext.query(SESSIONS_SLOT)),
            vec!["0".to_owned()],
            "an empty session a client is attached to lists (the anchor would otherwise hide)",
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
            panes_without_projection(work.query(PANES_SLOT)).0,
            json!([{"id": 0, "cols": 40, "rows": 12, "command": "cat", "title": null}]),
            "the birth pane runs the request's cmd at its size",
        );
        assert!(
            lock(&pool(&reg)).panes().is_empty(),
            "the default session is untouched — a create births a pane in the NEW session",
        );
    }

    /// A create CLAIMS the daemon's life across its own empty window, and hands it back afterwards
    /// — whether the birth pane made it or not.
    ///
    /// The claim is what stops an unrelated last pane's death from ending the daemon between the
    /// session existing and its shell existing ([`sprag_host::BirthPin`]). The failing half is the
    /// one worth pinning: a birth that cannot fork/exec is deliberately non-fatal, so a claim that
    /// only released on success would trade a daemon that exits too eagerly for one that never
    /// exits at all.
    ///
    /// REVERT-PROOF: hold the pin past the end of `new_session` (bind it in a `static`, or take it
    /// without a guard) and both assertions fail.
    #[test]
    fn a_create_releases_its_claim_however_the_birth_ends() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        assert!(
            ext.invoke(
                NEW_SESSION_ACTION,
                IntrospectValue::Json(json!({"name": "work", "cmd": ["cat"]})),
            )
            .is_ok(),
        );
        assert!(
            !lock(&reg).birth_in_flight(),
            "a born session claims nothing once its pane exists",
        );

        // A `cmd` the OS cannot exec: the session is created and left EMPTY (the one tolerated
        // non-fatal path), which is exactly when a leaked claim would be invisible and permanent.
        assert!(
            ext.invoke(
                NEW_SESSION_ACTION,
                IntrospectValue::Json(
                    json!({"name": "broken", "cmd": ["/nonexistent/sprag-no-such-program"]})
                ),
            )
            .is_ok(),
            "a runtime exec failure is non-fatal — the session still exists",
        );
        assert!(
            !lock(&reg).birth_in_flight(),
            "and a birth that FAILED releases its claim too, or the daemon never exits",
        );
    }

    #[test]
    fn parse_remote_reads_the_endpoint_and_rejects_malformed() {
        let obj = |value: Value| value.as_object().cloned().unwrap();
        // Absent -> None; a full and a host-only endpoint parse.
        assert_eq!(WorkspaceExternal::parse_remote(&Map::new()), Ok(None));
        assert_eq!(
            WorkspaceExternal::parse_remote(&obj(
                json!({"remote": {"host": "srv", "user": "me", "port": 2222}})
            )),
            Ok(Some(SshRemote {
                user: Some("me".to_owned()),
                host: "srv".to_owned(),
                port: Some(2222),
            })),
        );
        assert_eq!(
            WorkspaceExternal::parse_remote(&obj(json!({"remote": {"host": "srv"}}))),
            Ok(Some(SshRemote {
                user: None,
                host: "srv".to_owned(),
                port: None,
            })),
        );
        // Malformed: no host, empty host, a zero/overflowing port, or a non-object all reject.
        for bad in [
            json!({"remote": {}}),
            json!({"remote": {"host": ""}}),
            json!({"remote": {"host": "srv", "port": 0}}),
            json!({"remote": {"host": "srv", "port": 99999}}),
            json!({"remote": "srv"}),
        ] {
            assert_eq!(
                WorkspaceExternal::parse_remote(&obj(bad)),
                Err(InvokeError::TypeMismatch),
            );
        }
    }

    #[test]
    fn a_new_session_with_a_remote_marks_its_birth_pane() {
        // Revert-proof for `spawn_parsed`'s `set_pane_remote`: drop it and the birth pane has no
        // endpoint, so this `expect` panics.
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        assert_eq!(
            ext.invoke(
                NEW_SESSION_ACTION,
                IntrospectValue::Json(json!({
                    "name": "remote",
                    "cmd": ["ssh", "-t", "srv"],
                    "remote": {"host": "srv", "user": "me", "port": 2222},
                })),
            ),
            Ok(IntrospectValue::Json(Value::String("remote".to_owned()))),
        );
        let pool = pool_of(&reg, "remote");
        let pool = lock(&pool);
        let pane = pool.panes().first().expect("born with a pane");
        let remote = pane
            .remote()
            .expect("the birth pane is marked a remote workspace");
        assert_eq!(remote.host, "srv");
        assert_eq!(remote.user.as_deref(), Some("me"));
        assert_eq!(remote.port, Some(2222));
    }

    /// A file dropped on an ORDINARY pane is pasted straight in as a local path: the file is already
    /// reachable from a pane running on this machine, so there is nothing to upload. Driven through
    /// the real action, and observed the only honest way — the `cat` pane ECHOES what reached its
    /// PTY, so this proves the paste, not just the return value.
    ///
    /// The name carries a space, which pins the quoting at the same time: an unquoted paste would
    /// hand the shell two words.
    #[test]
    fn a_dropped_file_on_a_local_pane_is_pasted_as_a_quoted_local_path() {
        use std::time::{Duration, Instant};

        let dir = std::env::temp_dir().join(format!("sprag-drop-unit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the drop dir");
        let dropped = dir.join("a file.txt");
        std::fs::write(&dropped, b"payload").expect("write the dropped file");
        let canonical = std::fs::canonicalize(&dropped).expect("canonicalize");
        let quoted = format!("'{}'", canonical.display());

        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        let id = ext
            .invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
            .expect("spawn a local pane");
        let IntrospectValue::Int(id) = id else {
            panic!("spawn answers the pane id")
        };

        assert_eq!(
            ext.invoke(
                DROP_FILE_ACTION,
                IntrospectValue::Json(json!({"pane": id, "path": canonical.to_str().unwrap()})),
            ),
            Ok(IntrospectValue::Json(json!({ "path": quoted }))),
            "a local pane is handed the dropped file's own path, shell-quoted",
        );

        // `cat` echoes what was pasted — the paste is what makes this more than a return value.
        let pool = pool(&reg);
        let pane = PaneId(u64::try_from(id).unwrap());
        let deadline = Instant::now() + Duration::from_secs(5);
        let echoed = loop {
            let text = lock(&pool)
                .pane(pane)
                .expect("the pane is alive")
                .pty()
                .with_screen(sprag_vt::Screen::full_text);
            if text.contains(&quoted) {
                break true;
            }
            if Instant::now() > deadline {
                break false;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(echoed, "the quoted local path never reached the pane's PTY");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The refusals a drop must make, each a DIFFERENT category: a request that names no `path` is
    /// MALFORMED (`TypeMismatch`), while a well-formed request naming a pane that does not exist —
    /// or a file that does not — is `Rejected`. Collapsing the two would tell a client to fix the
    /// wrong end.
    #[test]
    fn drop_file_separates_a_malformed_request_from_a_refused_one() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        let id = ext
            .invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
            .expect("spawn a local pane");
        let IntrospectValue::Int(id) = id else {
            panic!("spawn answers the pane id")
        };

        assert_eq!(
            ext.invoke(DROP_FILE_ACTION, IntrospectValue::Json(json!({"pane": id}))),
            Err(InvokeError::TypeMismatch),
            "a drop with no path is malformed",
        );
        assert_eq!(
            ext.invoke(
                DROP_FILE_ACTION,
                IntrospectValue::Json(json!({"pane": 9999, "path": "/etc/hostname"})),
            ),
            Err(InvokeError::Rejected),
            "a well-formed drop on a pane that does not exist is refused, not a type error",
        );
        assert_eq!(
            ext.invoke(
                DROP_FILE_ACTION,
                IntrospectValue::Json(json!({"pane": id, "path": "/no/such/file/at/all"})),
            ),
            Err(InvokeError::Rejected),
            "a drop naming a file that cannot be resolved is refused",
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
        // The signal is counted DIRECTLY rather than through [`crate::spawn_reaper`], and that is
        // what makes this test sound. Routed through the reaper, the observable is `on_empty`,
        // which fires only when a signal is DRAINED while the workspace happens to be empty — and
        // draining is asynchronous, so the count is at-least-once and attributable to no particular
        // sender. Measured both ways: asserting `== 1` on it failed about 1 full-suite run in 5
        // (two signals drained after the death, 15us apart), and relaxing to `>= 1` made the guard
        // VACUOUS — unhooking the pane still passed, because the [`crate::BirthPin`]'s own release
        // signal was drained late and fired instead. Counting sends removes both problems: a send
        // is synchronous and each one is attributable.
        let signal: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            f.fetch_add(1, Ordering::SeqCst);
        });
        let mut ext = WorkspaceExternal::new(
            Arc::clone(&reg),
            SessionScope::unscoped(&reg),
            Arc::new(ChannelRegistry::default()),
            Some(signal),
            None,
        );
        // `new_session` sends exactly one signal by itself: the [`crate::BirthPin`] it takes fires
        // on release, deliberately, so a birth that FAILED still lets an idle daemon go. A BLOCKING
        // birth (`cat` waits on stdin) keeps the pane's own signal out of that count until this
        // test asks for it, which is what separates the two senders.
        let born = ext
            .invoke(
                NEW_SESSION_ACTION,
                IntrospectValue::Json(json!({"cmd": ["cat"]})),
            )
            .expect("new_session births a pane");
        let IntrospectValue::Json(Value::String(name)) = born else {
            panic!("new_session answers the allocated session name");
        };
        assert_eq!(
            fired.load(Ordering::SeqCst),
            1,
            "the pin's release is the one signal a birth sends on its own",
        );

        // Now end the pane. A SECOND signal can come from one place only — the `on_exit` the birth
        // spawn hooked onto it — so this is the assertion the test is named for: unhook the pane
        // and the count stays at the pin's 1 forever.
        let pool = lock(&reg)
            .workspace_of(&name)
            .expect("the born session resolves");
        let id = lock(&pool).panes().first().expect("the birth pane").id();
        drop(lock(&pool).close(id));
        let start = Instant::now();
        while fired.load(Ordering::SeqCst) < 2 && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            fired.load(Ordering::SeqCst),
            2,
            "the birth pane's death fed the death-signal, proving it was hooked — a \
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
            Arc::new(ChannelRegistry::default()),
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
            answer_doc(ext.query(WINDOWS_SLOT)),
            json!([
                {"name": "0", "current": false},
                {"name": "1", "current": true},
            ]),
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
            answer_doc(ext.query(WINDOWS_SLOT)),
            json!([
                {"name": "0", "current": true},
                {"name": "logs", "current": false},
            ]),
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
            answer_doc(ext.query(WINDOWS_SLOT)),
            json!([
                {"name": "main", "current": false},
                {"name": "logs", "current": true},
            ]),
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
            answer_doc(ext.query(WINDOWS_SLOT)),
            json!([{"name": "0", "current": true}]),
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
            Arc::new(ChannelRegistry::default()),
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
            answer_doc(default_ext.query(WINDOWS_SLOT)),
            json!([{"name": "0", "current": true}]),
            "the default session sees only its own one window",
        );

        let (work, _w) = scoped_control(&reg, scope_of(&reg, "work"));
        assert_eq!(
            answer_doc(work.query(WINDOWS_SLOT)),
            json!([
                {"name": "0", "current": false},
                {"name": "logs", "current": true},
            ]),
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
        let answer = write_doc(ext.invoke(
            SET_LAYOUT_ACTION,
            IntrospectValue::Json(gesture("elsewhere")),
        ));
        assert_eq!(
            answer, good,
            "a window mismatch refused it; window 0 kept its arrangement"
        );

        // Control: naming the ACTUAL scoped window lets the SAME gesture through.
        let answer = write_doc(ext.invoke(SET_LAYOUT_ACTION, IntrospectValue::Json(gesture("0"))));
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
