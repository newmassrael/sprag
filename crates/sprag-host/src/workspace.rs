//! The workspace control surface — pane management as a pinion `External`.
//!
//! The multiplexer's pane pool ([`Workspace`], a producer-layer concern in
//! sprag-terminal) is exposed to AI peers through one engine `External`: the
//! pane-management control plane. It generalizes the R6 input pattern —
//! producer mutations ride pinion's canonical `scene/invoke` against a
//! producer-owned handler, never new RPC methods (pinion RPC vocabulary stays
//! SSOT).
//!
//! Action channel (`scene/invoke`):
//!
//! * `spawn {cmd?:[..], cols?, rows?}` → spawns a pane, returns its id.
//! * `close {id}` → reaps a pane.
//! * `resize {id, cols, rows}` → resizes a pane's PTY + emulator.
//!
//! Read channel (`scene/query`): `panes` → the live pane list as JSON.
//!
//! There is no geometric tiling here — headless multiplexing is pane control,
//! not screen division (Round 7 design note); tiling is a rendering concern
//! for the deferred GUI round.

use std::fmt;
use std::sync::{Arc, Mutex};

use pinion_core::SceneRevision;
use pinion_core::external::{
    ExternalIntrospect, InterveneError, IntrospectSchema, IntrospectValue, InvokeError,
};
use serde_json::{Map, Value};
use sprag_terminal::{CommandBuilder, Workspace};

use crate::bump_on_dirty;
use crate::external::{as_object, lock, opt_dim, require_pane_id, rpc_external_impl};

// The mux control action names + query slot are the shared wire ABI vocabulary
// ([`crate::wire`]) — the SAME consts a client addresses for pane lifecycle.
use crate::wire::{CLOSE_ACTION, PANES_SLOT, RESIZE_ACTION, SPAWN_ACTION};

/// The pane-management engine `External`: a control surface over the shared
/// [`Workspace`]. Holds `Arc<Mutex<Workspace>>` so its `scene/invoke`
/// handlers mutate the live pane pool (which the serve loop also reads to
/// assemble the scene), plus the shared [`SceneRevision`] so a pane-lifecycle
/// mutation wakes any parked `scene/waitFor`.
pub struct WorkspaceExternal {
    workspace: Arc<Mutex<Workspace>>,
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
    /// Build the control surface over a shared workspace + the shared
    /// scene-version token (see the struct docs for `revision`'s two roles).
    #[must_use]
    pub fn new(workspace: Arc<Mutex<Workspace>>, revision: Arc<SceneRevision>) -> Self {
        Self {
            workspace,
            revision,
        }
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
            let mut workspace = lock(&self.workspace);
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
        let removed = lock(&self.workspace).close(id);
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
        match lock(&self.workspace).resize(id, cols, rows) {
            Ok(true) => Ok(IntrospectValue::Null),
            Ok(false) => Err(InvokeError::Rejected), // no such pane
            Err(_) => Err(InvokeError::Rejected),    // winsize ioctl failed
        }
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
        IntrospectSchema::new(&[
            (SPAWN_ACTION, "action"),
            (CLOSE_ACTION, "action"),
            (RESIZE_ACTION, "action"),
            (PANES_SLOT, "list"),
        ])
    }

    fn query(&self, path: &str) -> Option<IntrospectValue> {
        match path {
            PANES_SLOT => {
                let panes = lock(&self.workspace).list();
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
            _ => None,
        }
    }

    fn intervene(&mut self, _path: &str, _value: IntrospectValue) -> Result<(), InterveneError> {
        // No writable state slots: pane management is action-shaped (invoke).
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
            _ => Err(InvokeError::UnknownPath),
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

    fn workspace() -> Arc<Mutex<Workspace>> {
        Arc::new(Mutex::new(Workspace::new((80, 24))))
    }

    /// A control surface over `ws` sharing a fresh revision (returned so a test can
    /// assert the pane-lifecycle bumps). No `HostState` / observer is installed —
    /// [`SceneRevision::bump`] advances [`current`](SceneRevision::current) either
    /// way, which is all these tests read.
    fn control(ws: &Arc<Mutex<Workspace>>) -> (WorkspaceExternal, Arc<SceneRevision>) {
        let revision = Arc::new(SceneRevision::new());
        (
            WorkspaceExternal::new(Arc::clone(ws), Arc::clone(&revision)),
            revision,
        )
    }

    #[test]
    fn spawn_default_returns_first_id_and_adds_a_pane() {
        let ws = workspace();
        let (mut ext, _rev) = control(&ws);
        let id = ext.invoke(SPAWN_ACTION, IntrospectValue::Null).unwrap();
        assert_eq!(id, IntrospectValue::Int(0));
        assert_eq!(lock(&ws).panes().len(), 1);
    }

    #[test]
    fn spawn_with_cmd_array_sets_label() {
        let ws = workspace();
        let (mut ext, _rev) = control(&ws);
        ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
            .unwrap();
        assert_eq!(lock(&ws).list()[0].command_label, "cat");
    }

    #[test]
    fn close_existing_then_missing() {
        let ws = workspace();
        let (mut ext, _rev) = control(&ws);
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
        let ws = workspace();
        let (mut ext, rev) = control(&ws);
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
        let ws = workspace();
        let (mut ext, rev) = control(&ws);
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
        let ws = workspace();
        let (mut ext, _rev) = control(&ws);
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
            lock(&ws).pane(PaneId(0)).unwrap().session().dimensions(),
            (100, 30)
        );
    }

    #[test]
    fn query_panes_lists_metadata() {
        let ws = workspace();
        let (mut ext, _rev) = control(&ws);
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
        let ws = workspace();
        let (mut ext, _rev) = control(&ws);
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

    #[test]
    fn unknown_action_is_unknown_path() {
        let (mut ext, _rev) = control(&workspace());
        assert_eq!(
            ext.invoke("teleport", IntrospectValue::Null),
            Err(InvokeError::UnknownPath)
        );
    }
}
