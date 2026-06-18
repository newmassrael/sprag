//! `PaneAccess` — the plugin extension API.
//!
//! A plugin's whole view of the core: enumerate panes, read a pane's screen as
//! scene-as-data, and inject input — all addressed by [`PaneId`], never by
//! reaching into a `SessionHandle` or `Screen` directly. This is the single
//! read+inject path: every plugin (and any future control consumer) goes
//! through it, so reads and injections are consistent and the input-encoding
//! lives in one place.
//!
//! [`WorkspacePaneAccess`] is the production implementation over a shared
//! [`Workspace`]; it stays pinion-free (the producer/control layer).

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use sprag_input::{encode, Modifiers};
use sprag_terminal::{CommandBuilder, Pane, PaneId, SessionHandle, Workspace};
use sprag_vt::Screen;

/// One screen row: its damage `generation` paired with its (trailing-trimmed)
/// text, read in a single locked snapshot so the two never tear.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneRow {
    pub generation: u64,
    pub text: String,
}

/// A key to inject: a W3C `KeyboardEvent.key` string plus modifiers, encoded
/// to PTY bytes by [`PaneAccess::inject`] (the sprag-owned encoder, R2.6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyStroke {
    pub key: String,
    pub mods: Modifiers,
}

impl KeyStroke {
    /// A single unmodified named key (e.g. `KeyStroke::named("Enter")`).
    #[must_use]
    pub fn named(key: &str) -> Self {
        Self {
            key: key.to_string(),
            mods: Modifiers::default(),
        }
    }

    /// Expand text into one unmodified character keystroke per `char`.
    #[must_use]
    pub fn text(s: &str) -> Vec<Self> {
        s.chars()
            .map(|ch| Self {
                key: ch.to_string(),
                mods: Modifiers::default(),
            })
            .collect()
    }
}

/// Why [`PaneAccess::inject`] failed — a typed cause, not a discarded error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InjectError {
    /// No pane has the given id.
    UnknownPane(PaneId),
    /// A keystroke had no PTY-byte encoding (the offending key).
    Encode(String),
    /// Writing the encoded bytes to the pane failed (the IO error message).
    Write(String),
    /// Spawning a pane failed: no [`PaneLifecycle`] support, an empty argv, or
    /// the pseudoterminal/child could not start (the cause message).
    Spawn(String),
}

/// The plugin extension API: a plugin's view of the core's panes.
pub trait PaneAccess {
    /// The ids of the live panes, in order.
    fn pane_ids(&self) -> Vec<PaneId>;

    /// The pane's collapsed screen text (each row trailing-trimmed, rows joined
    /// without separators) — the read for substring/sentinel matching across
    /// wrapped lines. `None` if no pane has that id.
    fn pane_collapsed(&self, id: PaneId) -> Option<String>;

    /// The pane's screen as per-row `(generation, text)`, read in one snapshot.
    /// `None` if no pane has that id.
    fn pane_rows(&self, id: PaneId) -> Option<Vec<PaneRow>>;

    /// Whether the pane's child has closed its PTY (exited): no more output is
    /// coming and every byte it produced has already been applied to the
    /// screen. `None` if no pane has that id. This is the race-free completion
    /// signal a one-shot adapter (a tool that replies then exits) converges on.
    fn pane_eof(&self, id: PaneId) -> Option<bool>;

    /// Inject `keys` into the pane, returning the number of PTY bytes written.
    ///
    /// # Errors
    ///
    /// [`InjectError`] when the pane is unknown, a key cannot be encoded, or
    /// the write fails.
    fn inject(&self, id: PaneId, keys: &[KeyStroke]) -> Result<u64, InjectError>;

    /// The pane *lifecycle* surface (spawn/close), if this implementation
    /// supports it. `None` by default — read/inject plugins never need it, so
    /// they (and test doubles) pay nothing; a plugin that manages panes (e.g.
    /// an AI dialogue spawning one pane per turn) asks for it and fails cleanly
    /// when it is absent. Kept a separate sub-trait so [`PaneAccess`] stays the
    /// read/inject surface (interface segregation).
    fn lifecycle(&self) -> Option<&dyn PaneLifecycle> {
        None
    }
}

/// Pane *lifecycle* control: spawn and close panes. The capability a plugin
/// needs to orchestrate one-shot tools across turns (each turn a fresh pane).
/// Reached via [`PaneAccess::lifecycle`] so it does not fatten the read/inject
/// surface every plugin depends on.
pub trait PaneLifecycle {
    /// Spawn a pane running `argv` (`[program, args…]`) at `cols × rows`,
    /// returning its [`PaneId`].
    ///
    /// # Errors
    ///
    /// [`InjectError::Spawn`] when `argv` is empty or the pane cannot start.
    fn spawn(&self, argv: &[String], cols: u16, rows: u16) -> Result<PaneId, InjectError>;

    /// Close (reap) the pane with `id`, returning whether it existed. The
    /// pane's blocking teardown runs outside any shared lock.
    fn close(&self, id: PaneId) -> bool;
}

/// [`PaneAccess`] over a shared [`Workspace`] — the production implementation.
pub struct WorkspacePaneAccess {
    workspace: Arc<Mutex<Workspace>>,
}

impl WorkspacePaneAccess {
    /// Wrap a shared workspace as the plugin pane-access surface.
    #[must_use]
    pub fn new(workspace: Arc<Mutex<Workspace>>) -> Self {
        Self { workspace }
    }

    /// Clone the pane's I/O handle under the workspace lock (released before
    /// the handle is used), so screen reads / writes never hold the workspace
    /// lock.
    fn handle(&self, id: PaneId) -> Option<SessionHandle> {
        lock(&self.workspace).pane(id).map(Pane::handle)
    }
}

impl PaneAccess for WorkspacePaneAccess {
    fn pane_ids(&self) -> Vec<PaneId> {
        lock(&self.workspace).panes().iter().map(Pane::id).collect()
    }

    fn pane_collapsed(&self, id: PaneId) -> Option<String> {
        Some(self.handle(id)?.with_screen(read_collapsed))
    }

    fn pane_rows(&self, id: PaneId) -> Option<Vec<PaneRow>> {
        Some(self.handle(id)?.with_screen(read_rows))
    }

    fn pane_eof(&self, id: PaneId) -> Option<bool> {
        // A quick atomic load; reading it under the workspace lock (rather than
        // cloning the handle) is negligible and needs no producer change.
        lock(&self.workspace)
            .pane(id)
            .map(|pane| pane.session().is_eof())
    }

    fn inject(&self, id: PaneId, keys: &[KeyStroke]) -> Result<u64, InjectError> {
        let handle = self.handle(id).ok_or(InjectError::UnknownPane(id))?;
        let modes = handle.input_modes();
        let mut bytes = Vec::new();
        for stroke in keys {
            let encoded = encode(&stroke.key, stroke.mods, modes)
                .ok_or_else(|| InjectError::Encode(stroke.key.clone()))?;
            bytes.extend_from_slice(&encoded);
        }
        handle
            .write(&bytes)
            .map_err(|e| InjectError::Write(e.to_string()))?;
        Ok(bytes.len() as u64)
    }

    fn lifecycle(&self) -> Option<&dyn PaneLifecycle> {
        Some(self)
    }
}

impl PaneLifecycle for WorkspacePaneAccess {
    fn spawn(&self, argv: &[String], cols: u16, rows: u16) -> Result<PaneId, InjectError> {
        let (program, rest) = argv
            .split_first()
            .ok_or_else(|| InjectError::Spawn("empty argv".to_string()))?;
        let mut command = CommandBuilder::new(program.as_str());
        for arg in rest {
            command.arg(arg.as_str());
        }
        // The emulator parses (and strips) escape sequences, so captured cell
        // text stays clean regardless of TERM; match the host's spawn default.
        command.env("TERM", "xterm-256color");
        lock(&self.workspace)
            .spawn(command, program.clone(), cols, rows)
            .map_err(|e| InjectError::Spawn(e.to_string()))
    }

    fn close(&self, id: PaneId) -> bool {
        // Bind the removed Pane so the workspace guard (the temporary) drops
        // first; the Pane's blocking Drop (kill/wait/join) then runs OUTSIDE
        // the workspace lock (R11 lesson).
        let removed = lock(&self.workspace).close(id);
        removed.is_some()
    }
}

/// Lock the workspace, recovering the guard if a holder panicked.
fn lock(workspace: &Mutex<Workspace>) -> MutexGuard<'_, Workspace> {
    workspace.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One row's cells as text, with trailing blanks trimmed.
fn row_text(screen: &Screen, row: u16) -> String {
    let mut line = String::new();
    for col in 0..screen.cols() {
        if let Some(cell) = screen.cell(col, row) {
            line.push_str(&cell.cluster);
        }
    }
    line.trim_end().to_string()
}

/// Per-row `(generation, text)` for the whole screen.
fn read_rows(screen: &Screen) -> Vec<PaneRow> {
    (0..screen.rows())
        .map(|row| PaneRow {
            generation: screen.row_generation(row).unwrap_or(0),
            text: row_text(screen, row),
        })
        .collect()
}

/// Collapsed screen text: trailing-trimmed rows joined without separators, so
/// a sentinel the terminal wrapped across rows still matches.
fn read_collapsed(screen: &Screen) -> String {
    (0..screen.rows()).map(|row| row_text(screen, row)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sprag_terminal::CommandBuilder;
    use std::thread::sleep;
    use std::time::{Duration, Instant};

    fn cat_workspace(cols: u16, rows: u16) -> Arc<Mutex<Workspace>> {
        let workspace = Arc::new(Mutex::new(Workspace::new((cols, rows))));
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg("cat");
        command.env("TERM", "dumb");
        lock(&workspace)
            .spawn(command, "cat".to_string(), cols, rows)
            .expect("spawn pane");
        workspace
    }

    #[test]
    fn injects_and_reads_back_through_the_api() {
        let access = WorkspacePaneAccess::new(cat_workspace(20, 4));
        let pane = access.pane_ids()[0];

        let mut keys = KeyStroke::text("hi");
        keys.push(KeyStroke::named("Enter"));
        let written = access.inject(pane, &keys).expect("inject");
        assert!(written >= 3, "wrote {written} bytes");

        // The echo is async; poll the collapsed text until it lands.
        let start = Instant::now();
        let mut echoed = false;
        while !echoed && start.elapsed() < Duration::from_secs(5) {
            echoed = access.pane_collapsed(pane).is_some_and(|t| t.contains("hi"));
            if !echoed {
                sleep(Duration::from_millis(20));
            }
        }
        assert!(echoed, "injected 'hi' never echoed back");

        // pane_rows snapshots generation+text together.
        let rows = access.pane_rows(pane).expect("rows");
        assert_eq!(rows.len(), 4);
        assert!(rows.iter().any(|r| r.text.contains("hi") && r.generation > 0));
    }

    #[test]
    fn inject_into_unknown_pane_is_typed() {
        let access = WorkspacePaneAccess::new(cat_workspace(20, 4));
        let err = access.inject(PaneId(999), &KeyStroke::text("x")).unwrap_err();
        assert_eq!(err, InjectError::UnknownPane(PaneId(999)));
    }

    #[test]
    fn lifecycle_spawn_and_close_roundtrip() {
        let workspace = Arc::new(Mutex::new(Workspace::new((20, 4))));
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));
        let life = access.lifecycle().expect("workspace access exposes lifecycle");

        let id = life
            .spawn(
                &["/bin/sh".to_string(), "-c".to_string(), "cat".to_string()],
                20,
                4,
            )
            .expect("spawn");
        assert!(lock(&workspace).pane(id).is_some(), "pane should be live");

        assert!(life.close(id), "close reports the pane existed");
        assert!(lock(&workspace).pane(id).is_none(), "pane should be gone");
        assert!(!life.close(id), "closing again reports absence");
    }

    #[test]
    fn lifecycle_spawn_rejects_empty_argv() {
        let access = WorkspacePaneAccess::new(Arc::new(Mutex::new(Workspace::new((20, 4)))));
        let life = access.lifecycle().unwrap();
        assert!(matches!(life.spawn(&[], 20, 4), Err(InjectError::Spawn(_))));
    }
}
