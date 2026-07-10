//! The GUI's client-of-the-host boundary (topology B). The GUI reaches its panes
//! ONLY through [`LocalHost`], which owns the single [`Workspace`] and serves the
//! typed client protocol — the method surface a wire transport backs in a later
//! increment (the Workspace-ownership flip's transport step). Encapsulating the
//! Workspace behind this protocol is the seam: today an in-process owner; tomorrow
//! a wire client of the host process.
//!
//! ## The protocol (shaped like the eventual wire)
//!
//! * cell DATA ([`pane_cells`](LocalHost::pane_cells)) + the non-cell per-frame
//!   facts that ride alongside it
//!   ([`pane_scroll_facts`](LocalHost::pane_scroll_facts));
//! * resize control ([`resize`](LocalHost::resize)) + grid geometry
//!   ([`pane_grid_size`](LocalHost::pane_grid_size));
//! * INPUT — the GUI's keyboard / IME are now client SENDs
//!   ([`send_key`](LocalHost::send_key) / [`send_text`](LocalHost::send_text)),
//!   encoded by the shared host SSOT (R110); only the TRANSPORT (an in-process
//!   handle write today, an RPC send tomorrow) is a later step;
//! * pane text ([`pane_full_text`](LocalHost::pane_full_text) /
//!   [`pane_command_label`](LocalHost::pane_command_label)) for the a11y tree.
//!
//! These are wire-shaped: their GUI call sites are stable across the transport step.
//! The ONE exception is [`pane_handle`](LocalHost::pane_handle) — it hands out a
//! live [`SessionHandle`] that cannot cross a wire; it exists only to build the
//! GUI's own RPC input `SpragPaneExternal`s (see `main.rs`), and that method + those
//! externals retire when input clients attach to the HOST instead of the GUI (see
//! the transitional note).
//!
//! ## Transitional state (topology B, mid-arc)
//!
//! Two host authorities exist over the `Workspace` type today: this `LocalHost`
//! (typed methods, the GUI's client) and [`sprag_host::WorkspaceExternal`] (the
//! `scene/invoke` pane-lifecycle surface the RPC socket drives). Right now they wrap
//! DIFFERENT `Workspace`s in different processes (the GUI's vs the headless host's).
//! The convergence target is a SINGLE `Workspace` owned by the host process, with
//! `LocalHost` becoming a wire client of it — at which point the GUI's own pane
//! `SpragPaneExternal`s + [`pane_handle`](LocalHost::pane_handle) retire (input goes
//! to the host). Until then the GUI keeps them as its own always-on RPC input
//! surface; the two input paths (keyboard via `LocalHost`, socket via the externals)
//! funnel through the ONE shared `sprag_host::send_key` / `send_text` SSOT, so there
//! is no encoder drift.
//!
//! The GUI's own rendering config (cell metric, font size) is NOT here — that is
//! client-side presentation, held in [`TerminalView`](crate::terminal::TerminalView).

use pinion_core::GridBuffer;
use sprag_input::Modifiers;
use sprag_terminal::{Pane, PaneId, SessionHandle, Workspace};

/// Per-pane facts the client reads each frame that are NOT carried in the cell
/// buffer but ride ALONGSIDE it in one pane-frame: the scrollback depth (the
/// scrollbar extent + the top-anchored offset math) and the visible row count (one
/// scrollback page). Host-owned; over the wire these travel WITH the
/// [`pane_cells`](LocalHost::pane_cells) buffer as one message (not a separate
/// round-trip). Named "facts", not "dims", so it is never confused with the grid
/// geometry ([`pane_grid_size`](LocalHost::pane_grid_size)) — `scrollback_len` is a
/// history depth, not a dimension.
pub(crate) struct PaneScrollFacts {
    pub(crate) scrollback_len: usize,
    pub(crate) visible_rows: u16,
}

/// The single [`Workspace`] owner the GUI is a CLIENT of (topology B). In-process
/// for this increment: it owns the Workspace (and thus the PTYs) and serves the GUI
/// pane DATA (cells + scroll facts), resize control, INPUT (`send_key` / `send_text`),
/// input handles, and pane text through typed methods — the client protocol a wire
/// transport backs later. See the module docs for the wire-shape + transitional
/// (dual-authority / retirement) notes.
pub(crate) struct LocalHost {
    workspace: Workspace,
}

impl LocalHost {
    /// Wrap the spawned [`Workspace`] as the client's host. The boot panes are
    /// spawned by [`use_terminal`](crate::terminal::use_terminal), which holds the
    /// Owner-scoped [`RepaintSink`](pinion_core::RepaintSink) the `on_dirty` hook
    /// needs — a display concern a pure host would not know about, so the spawn
    /// stays there and hands the populated Workspace here. This owns the result;
    /// from here the GUI touches the Workspace only through the methods below.
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

    /// Pane `index`'s non-cell per-frame facts ([`PaneScrollFacts`]): scrollback
    /// depth + visible rows, read in one screen lock.
    pub(crate) fn pane_scroll_facts(&self, index: usize) -> PaneScrollFacts {
        self.pane(index)
            .session()
            .with_screen(|screen| PaneScrollFacts {
                scrollback_len: screen.scrollback_len(),
                visible_rows: screen.rows(),
            })
    }

    /// Pane `index`'s current grid `(cols, rows)` — the emulator screen size, which
    /// tracks the last reflow target. The reflow no-op guard and an undock window's
    /// intrinsic open size read it. (It reads the emulator, not the PTY winsize
    /// directly; the two agree at steady state since [`resize`](Self::resize) keeps
    /// them synced — the emulator size is exactly the right proxy for "already sized
    /// to the target".)
    pub(crate) fn pane_grid_size(&self, index: usize) -> (u16, u16) {
        self.pane(index).session().dimensions()
    }

    /// Resize pane `index`'s PTY (`TIOCSWINSZ`) — the reflow control path. A closed /
    /// absent pane's error is TRACED and ignored (it cannot happen this increment —
    /// boot panes never close — but the swallow is honest, not silent).
    pub(crate) fn resize(&self, index: usize, cols: u16, rows: u16) {
        let id = self.pane_id(index);
        if let Err(error) = self.workspace.resize(id, cols, rows) {
            tracing::trace!(
                target: "sprag_gui::host",
                pane = index,
                ?error,
                "resize of a closed/absent pane ignored",
            );
        }
    }

    /// Send a W3C `key` + `mods` to pane `index` — the CLIENT input path. Encodes to
    /// PTY bytes and writes via the shared host SSOT ([`sprag_host::send_key`], the
    /// same encoder the RPC `scene/invoke` path uses). `true` if it reached the PTY;
    /// `false` if the key is unencodable or the write failed. In-process now; over
    /// the wire this becomes an RPC send to the host's pane input surface.
    #[must_use]
    pub(crate) fn send_key(&self, index: usize, key: &str, mods: Modifiers) -> bool {
        sprag_host::send_key(&self.pane(index).handle(), key, mods)
    }

    /// Write literal committed `text` to pane `index` — the IME-commit / paste
    /// client path ([`sprag_host::send_text`]). Empty is a no-op success. `true` if
    /// it reached the PTY; `false` on a write failure.
    #[must_use]
    pub(crate) fn send_text(&self, index: usize, text: &str) -> bool {
        sprag_host::send_text(&self.pane(index).handle(), text)
    }

    /// Pane `index`'s cloneable I/O handle — the ONE non-wire-shaped method (module
    /// docs). It hands out a live [`SessionHandle`] to build the GUI's own RPC input
    /// `SpragPaneExternal`s (`main.rs`); it retires when input clients attach to the
    /// host. The GUI's OWN keyboard / IME go through [`send_key`](Self::send_key) /
    /// [`send_text`](Self::send_text), NOT this handle.
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

    /// Pane `index`'s stable [`PaneId`] — an INTERNAL handle for
    /// [`resize`](Self::resize); NOT part of the client protocol (the wire addresses
    /// panes by index, never a `PaneId`).
    fn pane_id(&self, index: usize) -> PaneId {
        self.pane(index).id()
    }

    /// The pane at tile `index` — the ONE place "which pane?" resolves. The boot
    /// panes are spawned in [`pane_tag`](crate::terminal::pane_tag) order and never
    /// closed this increment, so `index` (sourced from a pane / focus tag) is a hard
    /// in-range invariant, not an `Option`. When a `close` path lands this becomes an
    /// `Option`-returning lookup (flagged so it is not forgotten).
    fn pane(&self, index: usize) -> &Pane {
        self.workspace
            .panes()
            .get(index)
            .expect("pane index in range (boot panes spawned 0..pane_count, never closed)")
    }
}
