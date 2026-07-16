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
//! * `new_session {name}` → creates a session, returns its name.
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
//! `new_session` / `sessions` goes through it. The rest of the scene needs no such care: a
//! pane child and the plugin host are built from the scoped pool and can address nothing
//! else. The privilege is what creates the obligation.
//!
//! Two members are deliberately registry-WIDE rather than scoped, and for the same reason:
//! their subject is the set of sessions itself, so answering them within one session would
//! answer a question nobody asked. `sessions` enumerates the scopes a client may name;
//! `new_session` makes one — and neither can disturb another client, because creating is not
//! attaching and no scope but the immutable default is anyone else's.
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
use sprag_terminal::{CommandBuilder, LayoutSnapshot, LayoutWire, SessionRegistry, Workspace};

use crate::bump_on_dirty;
use crate::external::{as_object, lock, opt_dim, require_pane_id, rpc_external_impl};
use crate::scope::SessionScope;

// The mux control action names + query slots are the shared wire ABI vocabulary
// ([`crate::wire`]) — the SAME consts a client addresses for pane lifecycle.
use crate::wire::{
    CLOSE_ACTION, LAYOUT_SLOT, NEW_SESSION_ACTION, PANES_SLOT, RESIZE_ACTION, SESSIONS_SLOT,
    SET_FLOATING_ACTION, SET_LAYOUT_ACTION, SPAWN_ACTION,
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
}

impl WorkspaceExternal {
    /// Build the control surface over the shared mux registry, the session it is scoped to,
    /// and the shared scene-version token (see the struct docs for each field's role).
    #[must_use]
    pub fn new(
        registry: Arc<Mutex<SessionRegistry>>,
        scope: SessionScope,
        revision: Arc<SceneRevision>,
    ) -> Self {
        Self {
            registry,
            scope,
            revision,
        }
    }

    /// The scoped session's current-window pane pool — resolved when the scope was, so a
    /// spawn lands in the session the request named and nowhere else. No registry lock is
    /// taken to reach it, so it cannot nest with the workspace lock.
    fn workspace(&self) -> &Arc<Mutex<Workspace>> {
        self.scope.workspace()
    }

    /// `spawn` action: create a pane and return its id. `cmd` (an argv
    /// array) defaults to `$SHELL`; `cols`/`rows` default to the
    /// workspace's default size.
    fn spawn(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let empty = Map::new();
        let map = match args {
            IntrospectValue::Json(Value::Object(m)) => m,
            IntrospectValue::Null => &empty,
            _ => return Err(InvokeError::TypeMismatch),
        };
        let (command, label) = match map.get("cmd") {
            None => sprag_terminal::default_shell_command(),
            Some(Value::Array(argv)) => build_command(argv)?,
            Some(_) => return Err(InvokeError::TypeMismatch),
        };
        // Spawn WITH the change-notification hook (not the plain `spawn`), so this
        // pane's output bumps the SAME revision the boot pane's does — a client's
        // `scene/waitFor` then wakes on a mux-spawned pane exactly as it does on the
        // boot pane. The lock is scoped so the set-change bump below fires without it.
        let id = {
            let pool = self.workspace();
            let mut workspace = lock(pool);
            let (default_cols, default_rows) = workspace.default_size();
            let cols = opt_dim(map, "cols")?.unwrap_or(default_cols);
            let rows = opt_dim(map, "rows")?.unwrap_or(default_rows);
            workspace
                .spawn_with_dirty(
                    command,
                    label,
                    cols,
                    rows,
                    Some(bump_on_dirty(&self.revision)),
                )
                .map_err(|_| InvokeError::Rejected)?
        };
        // A NEW pane changed the set: wake parked waiters now, before its first
        // output, so a mirror learns the pane exists immediately (the pane-set
        // change-notification, distinct from the per-pane output bump above).
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
        let snapshot = crate::host::set_layout(&self.registry, &self.scope, tree, expected)
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

    /// `new_session {name}` action: create a session, answering with its name.
    ///
    /// Creating is not attaching, and nothing here changes what any other client sees: the
    /// new session starts empty, and every client's scope is either its own name or the
    /// immutable default. The answer is the name so a caller can scope its next request with
    /// what it just made, without a round trip to [`SESSIONS_SLOT`] to confirm.
    fn new_session(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let name = as_object(args)?
            .get("name")
            .and_then(Value::as_str)
            .ok_or(InvokeError::TypeMismatch)?;
        match lock(&self.registry).new_session(name) {
            Ok(()) => {}
            Err(error) => {
                tracing::debug!(target: "sprag_host", %error, "refused to create a session");
                // A taken name is the client's mistake, not a malformed request: it is
                // well-formed and simply cannot be honored.
                return Err(InvokeError::Rejected);
            }
        }
        // The session SET changed, so a client watching the surface learns of it the way it
        // learns of a pane-set change — by being woken, not by polling.
        self.revision.bump();
        Ok(IntrospectValue::Json(Value::String(name.to_owned())))
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
                    SchemaField::new(PANES_SLOT, "list"),
                    SchemaField::new(LAYOUT_SLOT, "tree"),
                    SchemaField::new(SESSIONS_SLOT, "list"),
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
                        serde_json::json!({
                            "id": p.id,
                            "cols": p.cols,
                            "rows": p.rows,
                            "command": p.command_label,
                            // The child's live OSC 0/2 window title, `null` until it sets
                            // one. A DISPLAY name (a client prefers it over the command
                            // label and falls back); never identity — the child sets it.
                            "title": p.title,
                        })
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
                let registry = lock(&self.registry);
                let default = registry.default_session().name();
                let entries = registry
                    .sessions()
                    .iter()
                    .map(|session| {
                        serde_json::json!({
                            "name": session.name(),
                            "windows": session.windows().len(),
                            // Not "is it current" — nothing is current. This says where an
                            // unscoped request goes, which is the only unnamed scope there is.
                            "default": session.name() == default,
                        })
                    })
                    .collect();
                Some(IntrospectValue::Json(Value::Array(entries)))
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
            WorkspaceExternal::new(Arc::clone(reg), scope, Arc::clone(&revision)),
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
        lock(&reg).new_session("work").unwrap();

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
        lock(&reg).new_session("work").unwrap();

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
        lock(&reg).new_session("work").unwrap();
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
        lock(&reg).new_session("work").unwrap();
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
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        assert_eq!(
            ext.query(SESSIONS_SLOT),
            Some(IntrospectValue::Json(
                json!([{"name": "0", "windows": 1, "default": true}])
            )),
            "at boot: one session, and it is where an unscoped request goes",
        );

        ext.invoke(
            NEW_SESSION_ACTION,
            IntrospectValue::Json(json!({"name": "work"})),
        )
        .unwrap();
        assert_eq!(
            ext.query(SESSIONS_SLOT),
            Some(IntrospectValue::Json(json!([
                {"name": "0", "windows": 1, "default": true},
                {"name": "work", "windows": 1, "default": false},
            ]))),
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

        // The name is required, and must be a string.
        assert_eq!(
            ext.invoke(NEW_SESSION_ACTION, IntrospectValue::Json(json!({}))),
            Err(InvokeError::TypeMismatch),
        );
        assert_eq!(
            ext.invoke(
                NEW_SESSION_ACTION,
                IntrospectValue::Json(json!({"name": 42}))
            ),
            Err(InvokeError::TypeMismatch),
        );
    }
}
