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
//!
//! Read channel (`scene/query`):
//!
//! * `panes` → the live pane list as JSON.
//! * `layout` → the current window's LOGICAL arrangement + its revision, as JSON.
//!
//! ## Why this surface holds the REGISTRY (and the plugin host does not)
//!
//! This is the multiplexer's own control plane, and sessions / windows / layout ARE mux
//! concerns — so it holds the `Arc<Mutex<SessionRegistry>>` and resolves the current
//! window per action. The PLUGIN host ([`crate::PluginsExternal`]) deliberately still
//! takes only `Arc<Mutex<Workspace>>`: a plugin operates on a pane pool and has no
//! business knowing about the session tree (Interface Segregation).
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

// The mux control action names + query slots are the shared wire ABI vocabulary
// ([`crate::wire`]) — the SAME consts a client addresses for pane lifecycle.
use crate::wire::{
    CLOSE_ACTION, LAYOUT_SLOT, PANES_SLOT, RESIZE_ACTION, SET_FLOATING_ACTION, SET_LAYOUT_ACTION,
    SPAWN_ACTION,
};

/// The mux-management engine `External`: a control surface over the shared
/// [`SessionRegistry`]. Holds `Arc<Mutex<SessionRegistry>>` so its `scene/invoke`
/// handlers mutate the live pane pool of the CURRENT window (which the serve loop also
/// reads to assemble the scene) and its `layout` slot can serve that window's
/// arrangement, plus the shared [`SceneRevision`] so a pane-lifecycle mutation wakes any
/// parked `scene/waitFor`.
pub struct WorkspaceExternal {
    registry: Arc<Mutex<SessionRegistry>>,
    /// The shared scene-version token ([`crate::HostState`]'s). Two roles:
    /// each pane this surface SPAWNS is wired with a `bump_on_dirty(&revision)`
    /// hook (so its output wakes waiters, like the boot pane), and a spawn /
    /// close bumps it directly (so a pane-set change wakes a waiter before the
    /// new pane's first output). Cloned per scene-assembly from the ONE token
    /// [`crate::HostState`] observes, so a mux-spawned pane can never be wired
    /// to a revision no waiter watches.
    revision: Arc<SceneRevision>,
}

impl WorkspaceExternal {
    /// Build the control surface over the shared mux registry + the shared
    /// scene-version token (see the struct docs for `revision`'s two roles).
    #[must_use]
    pub fn new(registry: Arc<Mutex<SessionRegistry>>, revision: Arc<SceneRevision>) -> Self {
        Self { registry, revision }
    }

    /// The CURRENT window's pane pool. Resolved per action (a cloned `Arc`), so the
    /// registry lock is released before the workspace lock is taken — never nested — and
    /// a later current-window switch is picked up with no re-plumbing.
    fn workspace(&self) -> Arc<Mutex<Workspace>> {
        lock(&self.registry).current_workspace()
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
            let mut workspace = lock(&pool);
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
        let removed = lock(&self.workspace()).close(id);
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
        match lock(&self.workspace()).resize(id, cols, rows) {
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
        let snapshot = crate::host::set_layout(&self.registry, tree, expected);
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
        let snapshot = crate::host::set_floating(&self.registry, id, floating);
        self.revision.bump();
        layout_value(snapshot).ok_or(InvokeError::Rejected)
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
                    SchemaField::new(PANES_SLOT, "list"),
                    SchemaField::new(LAYOUT_SLOT, "tree"),
                ]
            },
        )
    }

    fn query(&self, path: &str) -> Option<IntrospectValue> {
        match path {
            PANES_SLOT => {
                let panes = lock(&self.workspace()).list();
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
            // The current window's LOGICAL arrangement (no pixels) + its revision: what a
            // display client projects, and what lets a reattaching client restore the
            // layout. Reconciled against the live pool first, since pane lifecycle runs
            // through the Workspace directly and the tree is not the membership authority.
            LAYOUT_SLOT => layout_value(crate::host::reconciled_layout(&self.registry)),
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

    /// The registry's CURRENT window's pane pool — where the control surface's spawns
    /// land, so a test asserts against the same pool the surface resolves.
    fn pool(reg: &Arc<Mutex<SessionRegistry>>) -> Arc<Mutex<Workspace>> {
        lock(reg).current_workspace()
    }

    /// A control surface over `reg` sharing a fresh revision (returned so a test can
    /// assert the pane-lifecycle bumps). No `HostState` / observer is installed —
    /// [`SceneRevision::bump`] advances [`current`](SceneRevision::current) either
    /// way, which is all these tests read.
    fn control(reg: &Arc<Mutex<SessionRegistry>>) -> (WorkspaceExternal, Arc<SceneRevision>) {
        let revision = Arc::new(SceneRevision::new());
        (
            WorkspaceExternal::new(Arc::clone(reg), Arc::clone(&revision)),
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
                .session()
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
}
