//! The workspace — sprag's pane registry (the multiplexer's producer pool).
//!
//! README core scope ("멀티플렉싱: ... pane 생명주기"): the multiplexer
//! manages a set of live [`TerminalSession`] panes. This is a producer-layer
//! concern — owning PTYs and their lifecycle — so it stays pinion-free here;
//! the pinion scene/control surface lives one layer up in sprag-host (the
//! `WorkspaceExternal`).
//!
//! Headless multiplexing is pane *control*, not visual tiling: each pane is
//! an independently-sized terminal addressed by [`PaneId`]. There is no
//! geometric split tree — that only has meaning relative to a display
//! surface to divide, so it is a rendering concern deferred to the GUI round
//! (Round 7 design note).

use std::fmt;

use crate::session::{CommandBuilder, SessionError, SessionHandle, TerminalSession};

/// A stable, monotonic identifier for a pane within a [`Workspace`].
///
/// Ids are never reused, so a stale reference fails closed (the pane is
/// simply absent) rather than aliasing a pane that took its place.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct PaneId(pub u64);

impl fmt::Display for PaneId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// One managed pane: a live [`TerminalSession`] plus its id and a
/// human/AI-readable command label (surfaced via introspection).
pub struct Pane {
    id: PaneId,
    session: TerminalSession,
    command_label: String,
}

impl Pane {
    /// The pane's stable id.
    #[must_use]
    pub fn id(&self) -> PaneId {
        self.id
    }

    /// The live terminal session backing this pane.
    #[must_use]
    pub fn session(&self) -> &TerminalSession {
        &self.session
    }

    /// The label this pane was spawned with (typically the program name).
    #[must_use]
    pub fn command_label(&self) -> &str {
        &self.command_label
    }

    /// A cloneable I/O handle onto this pane's session.
    #[must_use]
    pub fn handle(&self) -> SessionHandle {
        self.session.handle()
    }
}

/// Read-only metadata describing a pane, for introspection over the
/// scene-as-data control surface (the host maps this to JSON).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneInfo {
    pub id: u64,
    pub cols: u16,
    pub rows: u16,
    pub command_label: String,
}

/// The multiplexer's pane pool: a set of live panes, the monotonic id
/// counter, and the default size a dimension-less spawn adopts.
///
/// Pinion-free by design (producer layer). The host wraps this in
/// `Arc<Mutex<Workspace>>` and exposes spawn/close/resize as `scene/invoke`
/// actions on the `WorkspaceExternal`.
pub struct Workspace {
    panes: Vec<Pane>,
    next_id: u64,
    default_size: (u16, u16),
}

impl Workspace {
    /// A new, empty workspace whose dimension-less spawns adopt
    /// `default_size`.
    #[must_use]
    pub fn new(default_size: (u16, u16)) -> Self {
        Self {
            panes: Vec::new(),
            next_id: 0,
            default_size,
        }
    }

    /// The default `(cols, rows)` a dimension-less spawn adopts.
    #[must_use]
    pub fn default_size(&self) -> (u16, u16) {
        self.default_size
    }

    /// Spawn `command` on a fresh `cols x rows` pane, returning its id.
    /// `label` is the introspection label (typically the program name).
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] if the pseudoterminal or child cannot be
    /// started; on failure no pane is added and the id is not consumed.
    pub fn spawn(
        &mut self,
        command: CommandBuilder,
        label: String,
        cols: u16,
        rows: u16,
    ) -> Result<PaneId, SessionError> {
        self.spawn_with_dirty(command, label, cols, rows, None)
    }

    /// [`Self::spawn`] with an `on_dirty` callback wired into the pane's PTY
    /// reader (threaded to [`TerminalSession::spawn_with_dirty`]).
    ///
    /// A windowed host passes `Some(Box::new(move || sink.request_repaint()))`
    /// (the pinion R999 `RepaintSink` seam) so this pane's output wakes the
    /// shell to repaint. The callback is pinion-free (`Box<dyn Fn() + Send>`),
    /// keeping this crate decoupled from the GUI shell. Headless callers use
    /// [`Self::spawn`] (`None`).
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] if the pseudoterminal or child cannot be
    /// started; on failure no pane is added and the id is not consumed.
    pub fn spawn_with_dirty(
        &mut self,
        command: CommandBuilder,
        label: String,
        cols: u16,
        rows: u16,
        on_dirty: Option<Box<dyn Fn() + Send>>,
    ) -> Result<PaneId, SessionError> {
        let session = TerminalSession::spawn_with_dirty(command, cols, rows, on_dirty)?;
        let id = PaneId(self.next_id);
        self.next_id += 1;
        self.panes.push(Pane {
            id,
            session,
            command_label: label,
        });
        Ok(id)
    }

    /// Remove the pane with `id`, **returning it** so the caller drops it —
    /// running [`TerminalSession`]'s `kill` / `wait` / `join` on `Drop` —
    /// *outside* any lock the caller is holding (those are blocking process
    /// ops; reaping under a shared lock would stall everything contending on
    /// it, e.g. an in-flight plugin run). Returns `None` if no pane has `id`.
    #[must_use]
    pub fn close(&mut self, id: PaneId) -> Option<Pane> {
        let index = self.panes.iter().position(|pane| pane.id == id)?;
        Some(self.panes.remove(index))
    }

    /// Resize the pane with `id` to `cols x rows` (PTY + emulator).
    ///
    /// Returns `Ok(true)` when the pane exists and was resized, `Ok(false)`
    /// when no pane has that id.
    ///
    /// Takes `&self`: [`TerminalSession::resize`] is `&self` (interior-mutable
    /// PTY + emulator), so a shared `&Workspace` — e.g. one reached through an
    /// `Rc` in the GUI's resize Effect — can reflow a pane without owning the
    /// pool. The host caller (which holds a `MutexGuard<Workspace>`) is
    /// unaffected: a `&mut` guard still calls a `&self` method.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] if the PTY winsize ioctl fails.
    pub fn resize(&self, id: PaneId, cols: u16, rows: u16) -> Result<bool, SessionError> {
        match self.panes.iter().find(|p| p.id == id) {
            Some(pane) => {
                pane.session.resize(cols, rows)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// The pane with `id`, or `None`.
    #[must_use]
    pub fn pane(&self, id: PaneId) -> Option<&Pane> {
        self.panes.iter().find(|p| p.id == id)
    }

    /// All panes, in spawn order.
    #[must_use]
    pub fn panes(&self) -> &[Pane] {
        &self.panes
    }

    /// Introspection metadata for every pane, in spawn order.
    #[must_use]
    pub fn list(&self) -> Vec<PaneInfo> {
        self.panes
            .iter()
            .map(|p| {
                let (cols, rows) = p.session.dimensions();
                PaneInfo {
                    id: p.id.0,
                    cols,
                    rows,
                    command_label: p.command_label.clone(),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A long-lived child (`cat` reads stdin) so the pane's PTY stays open
    /// across resize/close assertions.
    fn cmd() -> CommandBuilder {
        let mut c = CommandBuilder::new("/bin/sh");
        c.arg("-c");
        c.arg("cat");
        c.env("TERM", "dumb");
        c
    }

    #[test]
    fn spawn_assigns_monotonic_ids() {
        let mut ws = Workspace::new((80, 24));
        let a = ws.spawn(cmd(), "sh".to_string(), 80, 24).unwrap();
        let b = ws.spawn(cmd(), "sh".to_string(), 80, 24).unwrap();
        assert_eq!(a, PaneId(0));
        assert_eq!(b, PaneId(1));
        assert_eq!(ws.panes().len(), 2);
    }

    #[test]
    fn close_removes_and_ids_are_not_reused() {
        let mut ws = Workspace::new((80, 24));
        let a = ws.spawn(cmd(), "sh".to_string(), 80, 24).unwrap();
        let _b = ws.spawn(cmd(), "sh".to_string(), 80, 24).unwrap();
        assert!(ws.close(a).is_some());
        assert!(ws.close(a).is_none()); // already gone
        assert!(ws.pane(a).is_none());
        // The freed id is not reclaimed by the next spawn.
        let c = ws.spawn(cmd(), "sh".to_string(), 80, 24).unwrap();
        assert_eq!(c, PaneId(2));
    }

    #[test]
    fn resize_updates_dimensions() {
        let mut ws = Workspace::new((80, 24));
        let a = ws.spawn(cmd(), "sh".to_string(), 80, 24).unwrap();
        assert!(ws.resize(a, 100, 30).unwrap());
        assert_eq!(ws.pane(a).unwrap().session().dimensions(), (100, 30));
        assert!(!ws.resize(PaneId(999), 10, 10).unwrap());
        // Through a SHARED &Workspace — the path the GUI reflow Effect uses via
        // an Rc; resize needs no &mut now that the session is interior-mutable.
        let shared: &Workspace = &ws;
        assert!(shared.resize(a, 64, 20).unwrap());
        assert_eq!(ws.pane(a).unwrap().session().dimensions(), (64, 20));
    }

    #[test]
    fn list_reports_metadata() {
        let mut ws = Workspace::new((80, 24));
        ws.spawn(cmd(), "alpha".to_string(), 40, 12).unwrap();
        let info = ws.list();
        assert_eq!(info.len(), 1);
        assert_eq!(info[0].id, 0);
        assert_eq!((info[0].cols, info[0].rows), (40, 12));
        assert_eq!(info[0].command_label, "alpha");
    }
}
