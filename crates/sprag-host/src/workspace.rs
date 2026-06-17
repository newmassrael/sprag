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
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use pinion_core::external::{
    Backend, BackendFallback, BackendSupport, External, ExternalIntrospect, IntrospectSchema,
    IntrospectValue, InterveneError, InvokeError, RepaintOwner, ThreadOwnership,
};
use serde_json::{Map, Value};
use sprag_terminal::{CommandBuilder, PaneId, Workspace};

const SPAWN_ACTION: &str = "spawn";
const CLOSE_ACTION: &str = "close";
const RESIZE_ACTION: &str = "resize";
const PANES_SLOT: &str = "panes";

/// The pane-management engine `External`: a control surface over the shared
/// [`Workspace`]. Holds `Arc<Mutex<Workspace>>` so its `scene/invoke`
/// handlers mutate the live pane pool (which the serve loop also reads to
/// assemble the scene).
pub struct WorkspaceExternal {
    workspace: Arc<Mutex<Workspace>>,
}

impl WorkspaceExternal {
    /// Build the control surface over a shared workspace.
    #[must_use]
    pub fn new(workspace: Arc<Mutex<Workspace>>) -> Self {
        Self { workspace }
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
            None => default_shell_command(),
            Some(Value::Array(argv)) => build_command(argv)?,
            Some(_) => return Err(InvokeError::TypeMismatch),
        };
        let mut workspace = lock(&self.workspace);
        let (default_cols, default_rows) = workspace.default_size();
        let cols = opt_dim(map, "cols")?.unwrap_or(default_cols);
        let rows = opt_dim(map, "rows")?.unwrap_or(default_rows);
        let id = workspace
            .spawn(command, label, cols, rows)
            .map_err(|_| InvokeError::Rejected)?;
        Ok(IntrospectValue::Int(i64::try_from(id.0).unwrap_or(i64::MAX)))
    }

    /// `close` action: reap the pane with `id`.
    fn close(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let id = parse_id(args)?;
        if lock(&self.workspace).close(id) {
            Ok(IntrospectValue::Null)
        } else {
            Err(InvokeError::Rejected) // no such pane
        }
    }

    /// `resize` action: resize the pane with `id` to `cols x rows`.
    fn resize(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let IntrospectValue::Json(Value::Object(map)) = args else {
            return Err(InvokeError::TypeMismatch);
        };
        let id = map
            .get("id")
            .and_then(Value::as_u64)
            .map(PaneId)
            .ok_or(InvokeError::TypeMismatch)?;
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

impl External for WorkspaceExternal {
    fn backends(&self) -> BackendSupport {
        BackendSupport::new(&[Backend::Rpc], BackendFallback::Skip)
    }

    fn repaint_ownership(&self) -> RepaintOwner {
        RepaintOwner::Framework
    }

    fn thread_ownership(&self) -> ThreadOwnership {
        // Each pane's reader thread sits behind the producer boundary (R1.7);
        // this control surface's own work is synchronous.
        ThreadOwnership::UiThreadSync
    }

    fn introspect(&self) -> Option<&dyn ExternalIntrospect> {
        Some(self)
    }

    fn introspect_mut(&mut self) -> Option<&mut dyn ExternalIntrospect> {
        Some(self)
    }
}

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

    fn invoke(&mut self, path: &str, args: IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        match path {
            SPAWN_ACTION => self.spawn(&args),
            CLOSE_ACTION => self.close(&args),
            RESIZE_ACTION => self.resize(&args),
            _ => Err(InvokeError::UnknownPath),
        }
    }
}

/// Lock the workspace, recovering the guard if a holder panicked.
fn lock(workspace: &Mutex<Workspace>) -> MutexGuard<'_, Workspace> {
    workspace.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Build a [`CommandBuilder`] from an argv JSON array (`[program, args…]`),
/// returning it plus the program label. Empty or non-string argv is a
/// [`InvokeError::TypeMismatch`].
fn build_command(argv: &[Value]) -> Result<(CommandBuilder, String), InvokeError> {
    let parts: Vec<&str> = argv
        .iter()
        .map(Value::as_str)
        .collect::<Option<_>>()
        .ok_or(InvokeError::TypeMismatch)?;
    let (program, rest) = parts.split_first().ok_or(InvokeError::TypeMismatch)?;
    let mut command = CommandBuilder::new(*program);
    for arg in rest {
        command.arg(*arg);
    }
    command.env("TERM", "xterm-256color");
    Ok((command, (*program).to_string()))
}

/// The default pane command: `$SHELL` (or `/bin/sh`), labelled by program.
fn default_shell_command() -> (CommandBuilder, String) {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let mut command = CommandBuilder::new(&shell);
    command.env("TERM", "xterm-256color");
    (command, shell)
}

/// Read an optional positive `u16` dimension: `None` when absent, `Err` when
/// present but not a positive `u16`.
fn opt_dim(map: &Map<String, Value>, key: &str) -> Result<Option<u16>, InvokeError> {
    match map.get(key) {
        None => Ok(None),
        Some(value) => value
            .as_u64()
            .and_then(|n| u16::try_from(n).ok())
            .filter(|&n| n > 0)
            .map(Some)
            .ok_or(InvokeError::TypeMismatch),
    }
}

/// Parse `{id: <u64>}` from an invoke args object.
fn parse_id(args: &IntrospectValue) -> Result<PaneId, InvokeError> {
    match args {
        IntrospectValue::Json(Value::Object(map)) => map
            .get("id")
            .and_then(Value::as_u64)
            .map(PaneId)
            .ok_or(InvokeError::TypeMismatch),
        _ => Err(InvokeError::TypeMismatch),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn workspace() -> Arc<Mutex<Workspace>> {
        Arc::new(Mutex::new(Workspace::new((80, 24))))
    }

    #[test]
    fn spawn_default_returns_first_id_and_adds_a_pane() {
        let ws = workspace();
        let mut ext = WorkspaceExternal::new(Arc::clone(&ws));
        let id = ext.invoke(SPAWN_ACTION, IntrospectValue::Null).unwrap();
        assert_eq!(id, IntrospectValue::Int(0));
        assert_eq!(lock(&ws).panes().len(), 1);
    }

    #[test]
    fn spawn_with_cmd_array_sets_label() {
        let ws = workspace();
        let mut ext = WorkspaceExternal::new(Arc::clone(&ws));
        ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
            .unwrap();
        assert_eq!(lock(&ws).list()[0].command_label, "cat");
    }

    #[test]
    fn close_existing_then_missing() {
        let ws = workspace();
        let mut ext = WorkspaceExternal::new(Arc::clone(&ws));
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
    fn resize_requires_dims_and_targets_a_pane() {
        let ws = workspace();
        let mut ext = WorkspaceExternal::new(Arc::clone(&ws));
        ext.invoke(SPAWN_ACTION, IntrospectValue::Null).unwrap();
        // Missing rows -> type mismatch.
        assert_eq!(
            ext.invoke(RESIZE_ACTION, IntrospectValue::Json(json!({"id": 0, "cols": 100}))),
            Err(InvokeError::TypeMismatch)
        );
        assert_eq!(
            ext.invoke(RESIZE_ACTION, IntrospectValue::Json(json!({"id": 0, "cols": 100, "rows": 30}))),
            Ok(IntrospectValue::Null)
        );
        assert_eq!(lock(&ws).pane(PaneId(0)).unwrap().session().dimensions(), (100, 30));
    }

    #[test]
    fn query_panes_lists_metadata() {
        let ws = workspace();
        let mut ext = WorkspaceExternal::new(Arc::clone(&ws));
        ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"], "cols": 40, "rows": 12})))
            .unwrap();
        let panes = ext.query(PANES_SLOT).unwrap();
        assert_eq!(
            panes,
            IntrospectValue::Json(json!([{"id": 0, "cols": 40, "rows": 12, "command": "cat"}]))
        );
    }

    #[test]
    fn unknown_action_is_unknown_path() {
        let mut ext = WorkspaceExternal::new(workspace());
        assert_eq!(ext.invoke("teleport", IntrospectValue::Null), Err(InvokeError::UnknownPath));
    }
}
