//! The GUI's client-of-the-host boundary (topology B). The GUI reaches its panes
//! ONLY through [`LocalHost`], which owns the single [`Workspace`] and serves the
//! typed client protocol — the method surface a wire transport backs in a later
//! increment (the Workspace-ownership flip's transport step), WITHOUT the GUI call
//! sites changing. Encapsulating the Workspace behind this protocol is the seam:
//! today an in-process owner; tomorrow a wire client of the host process.
//!
//! The protocol is deliberately shaped like the eventual wire: cell DATA
//! ([`pane_cells`](LocalHost::pane_cells)), non-cell per-frame facts
//! ([`pane_dims`](LocalHost::pane_dims)), resize control, and pane text — each a
//! host query the wire will carry. Input still flows through the pane's
//! [`SessionHandle`](LocalHost::pane_handle) (in-process); moving it over the
//! client is a later step. The GUI's own rendering config (cell metric, font size)
//! is NOT here — that is client-side presentation, held in
//! [`TerminalView`](crate::terminal::TerminalView).

use pinion_core::GridBuffer;
use sprag_terminal::{Pane, PaneId, SessionHandle, Workspace};

/// Per-pane facts the client reads each frame that are NOT carried in the cell
/// buffer: the scrollback depth (the scrollbar extent + the top-anchored offset
/// math) and the visible row count (one scrollback page). Host-owned; over the
/// wire they ride alongside the cells.
pub(crate) struct PaneDims {
    pub(crate) scrollback_len: usize,
    pub(crate) visible_rows: u16,
}

/// The single [`Workspace`] owner the GUI is a CLIENT of (topology B). In-process
/// for this increment: it owns the Workspace (and thus the PTYs) and serves the
/// GUI pane DATA (cells + dims), resize control, input handles, and pane text
/// through typed methods — the client protocol a wire transport backs later
/// without the GUI call sites changing.
pub(crate) struct LocalHost {
    workspace: Workspace,
}

impl LocalHost {
    /// Wrap the spawned [`Workspace`] as the client's host. The boot panes are
    /// spawned by [`use_terminal`](crate::terminal::use_terminal) (which holds the
    /// Owner-scoped repaint sink); this owns the result, and from here the GUI
    /// touches the Workspace only through the methods below.
    pub(crate) fn new(workspace: Workspace) -> Self {
        Self { workspace }
    }

    /// The number of live panes.
    pub(crate) fn pane_count(&self) -> usize {
        self.workspace.panes().len()
    }

    /// Pane `index`'s cell DATA scrolled `offset_lines` rows up — the wire-contract
    /// cell query ([`sprag_host::pane_cells`]). The GUI never touches the session or
    /// screen directly; it asks the host for cells. The `with_screen` lock is
    /// released before this returns (the owned buffer needs none).
    pub(crate) fn pane_cells(&self, index: usize, offset_lines: usize) -> GridBuffer {
        sprag_host::pane_cells(self.pane(index).session(), offset_lines)
    }

    /// Pane `index`'s non-cell per-frame facts ([`PaneDims`]): scrollback depth +
    /// visible rows, read in one screen lock.
    pub(crate) fn pane_dims(&self, index: usize) -> PaneDims {
        self.pane(index).session().with_screen(|screen| PaneDims {
            scrollback_len: screen.scrollback_len(),
            visible_rows: screen.rows(),
        })
    }

    /// Pane `index`'s current PTY `(cols, rows)` — the reflow no-op guard and an
    /// undock window's intrinsic open size.
    pub(crate) fn pane_pty_size(&self, index: usize) -> (u16, u16) {
        self.pane(index).session().dimensions()
    }

    /// Pane `index`'s stable [`PaneId`].
    pub(crate) fn pane_id(&self, index: usize) -> PaneId {
        self.pane(index).id()
    }

    /// Resize pane `index`'s PTY (`TIOCSWINSZ`) — the reflow control path. Ignores
    /// the error of a closed pane, matching the prior direct call.
    pub(crate) fn resize(&self, index: usize, cols: u16, rows: u16) {
        let id = self.pane_id(index);
        let _ = self.workspace.resize(id, cols, rows);
    }

    /// Pane `index`'s cloneable I/O handle — the input `SpragPaneExternal`'s seam
    /// (in-process). Over the wire, input becomes a client send (a later step).
    pub(crate) fn pane_handle(&self, index: usize) -> SessionHandle {
        self.pane(index).handle()
    }

    /// Pane `index`'s full text (scrollback + visible) — the a11y text SSOT, the
    /// same string the RPC `full_text` query and the plugin capture read.
    pub(crate) fn pane_full_text(&self, index: usize) -> String {
        self.pane(index)
            .session()
            .with_screen(|screen| screen.full_text())
    }

    /// Pane `index`'s command label (the a11y node name).
    pub(crate) fn pane_command_label(&self, index: usize) -> &str {
        self.pane(index).command_label()
    }

    /// The pane at tile `index` — the ONE place "which pane?" resolves. The boot
    /// panes are spawned in [`pane_tag`](crate::terminal::pane_tag) order and never
    /// closed this increment, so `index` (sourced from a pane / focus tag) is a hard
    /// in-range invariant, not an `Option`.
    fn pane(&self, index: usize) -> &Pane {
        self.workspace
            .panes()
            .get(index)
            .expect("pane index in range (boot panes spawned 0..pane_count, never closed)")
    }
}
