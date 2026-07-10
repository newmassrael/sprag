//! The host — the single [`Workspace`] owner, used two ways.
//!
//! [`Host`] owns the one live [`Workspace`] (and thus the PTYs) and serves a
//! typed client protocol over it: cell DATA, per-frame scroll facts, resize
//! control, INPUT (`send_key` / `send_text`), input handles, and pane text.
//! This is the single home for "who owns the panes", shared by both frontends
//! (the north-star's two-frontend platform, [DESIGN.md §5]):
//!
//! * the **GUI** (`sprag-gui`) holds a `Host` in-process — it boots its tiled
//!   panes through [`Host::spawn`] and reaches every pane ONLY through the
//!   methods below (no direct `Workspace` access), so the display client is
//!   structurally a client of this host;
//! * the **headless server** (`sprag-term`) boots its pane through the same
//!   `Host` and wraps it in [`HostState`](crate::HostState) to serve the
//!   scene-as-data RPC surface an AI peer drives.
//!
//! ## The protocol (shaped like the eventual wire)
//!
//! * cell DATA ([`pane_cells`](Host::pane_cells)) + the non-cell per-frame facts
//!   that ride alongside it ([`pane_scroll_facts`](Host::pane_scroll_facts));
//! * resize control ([`resize`](Host::resize)) + grid geometry
//!   ([`pane_grid_size`](Host::pane_grid_size));
//! * INPUT — the display client's keyboard / IME are client SENDs
//!   ([`send_key`](Host::send_key) / [`send_text`](Host::send_text)), encoded by
//!   the shared [`crate::send_key`] / [`crate::send_text`] SSOT (the same encoder
//!   the RPC `scene/invoke` path uses); only the TRANSPORT (an in-process handle
//!   write today, an RPC send tomorrow) is a later step;
//! * pane text ([`pane_full_text`](Host::pane_full_text) /
//!   [`pane_command_label`](Host::pane_command_label)) for the a11y tree.
//!
//! These are wire-shaped: their call sites stay stable across the transport step
//! (topology B — the GUI becomes a pure wire client of this host). The ONE
//! exception is [`pane_handle`](Host::pane_handle) — it hands out a live
//! [`SessionHandle`] that cannot cross a wire; it exists only to build the GUI's
//! own RPC input `SpragPaneExternal`s (see the GUI `main.rs`), and it retires when
//! input clients attach to the host instead of the GUI.
//!
//! ## Ownership
//!
//! The `Workspace` lives behind `Arc<Mutex<_>>` — the shape [`HostState`](crate::HostState) and the
//! plugin/control externals already share (a background plugin run reads a pane
//! from a worker thread, so the pool is genuinely shared). The in-process GUI is
//! single-threaded, so its per-frame reads take an uncontended lock; the same
//! `Arc` clone reaches [`workspace_scene`](crate::workspace_scene). Presentation
//! (cell metric, font size) is NOT here — that is the display client's own state.

use std::sync::{Arc, Mutex};

use pinion_core::GridBuffer;
use sprag_input::Modifiers;
use sprag_terminal::{CommandBuilder, Pane, PaneId, SessionError, SessionHandle, Workspace};
use sprag_vt::Screen;

use crate::external::lock;

/// Per-pane facts the client reads each frame that are NOT carried in the cell
/// buffer but ride ALONGSIDE it in one pane-frame: the scrollback depth (the
/// scrollbar extent + the top-anchored offset math) and the visible row count
/// (one scrollback page). Host-owned; over the wire these travel WITH the
/// [`pane_cells`](Host::pane_cells) buffer as one message (not a separate
/// round-trip). Named "facts", not "dims", so it is never confused with the grid
/// geometry ([`pane_grid_size`](Host::pane_grid_size)) — `scrollback_len` is a
/// history depth, not a dimension.
pub struct PaneScrollFacts {
    pub scrollback_len: usize,
    pub visible_rows: u16,
}

/// The single [`Workspace`] owner + typed client protocol (topology B). See the
/// module docs for the wire-shape + ownership notes. Constructed empty
/// ([`new`](Host::new)) and populated with [`spawn`](Host::spawn); both frontends
/// boot their panes this way and reach them only through the methods below.
pub struct Host {
    workspace: Arc<Mutex<Workspace>>,
}

impl Host {
    /// A new host over an empty [`Workspace`] whose dimension-less spawns adopt
    /// `default_size`. Boot panes are added with [`spawn`](Self::spawn).
    #[must_use]
    pub fn new(default_size: (u16, u16)) -> Self {
        Self {
            workspace: Arc::new(Mutex::new(Workspace::new(default_size))),
        }
    }

    /// Spawn a boot pane running `command` (labelled `label`) at `cols x rows`,
    /// returning its id. `on_dirty` is the pinion-free wake hook a windowed client
    /// passes (`Some(Box::new(move || sink.request_repaint()))`, the R999
    /// `RepaintSink` seam) so this pane's output repaints the window; the headless
    /// server passes `None`. Keeping the hook a `Box<dyn Fn() + Send>` (not a
    /// pinion type) is why the display concern can live in the GUI while the spawn
    /// lives here.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] if the pseudoterminal or child cannot be started.
    pub fn spawn(
        &self,
        command: CommandBuilder,
        label: String,
        cols: u16,
        rows: u16,
        on_dirty: Option<Box<dyn Fn() + Send>>,
    ) -> Result<PaneId, SessionError> {
        lock(&self.workspace).spawn_with_dirty(command, label, cols, rows, on_dirty)
    }

    /// The shared pane pool, for the scene-as-data assembly ([`workspace_scene`](crate::workspace_scene))
    /// and the control / plugin externals that hold their own `Arc` clone. The one
    /// place the raw `Workspace` handle escapes; the typed methods are how a client
    /// reaches panes.
    #[must_use]
    pub fn workspace(&self) -> &Arc<Mutex<Workspace>> {
        &self.workspace
    }

    /// The number of live panes.
    #[must_use]
    pub fn pane_count(&self) -> usize {
        lock(&self.workspace).panes().len()
    }

    /// Pane `index`'s cell DATA scrolled `offset_lines` rows up — the wire-contract
    /// cell query ([`crate::pane_cells`]). The client never touches the session or
    /// screen directly; it asks the host for cells.
    #[must_use]
    pub fn pane_cells(&self, index: usize, offset_lines: usize) -> GridBuffer {
        self.with_pane(index, |pane| {
            crate::pane_cells(pane.session(), offset_lines)
        })
    }

    /// Pane `index`'s non-cell per-frame facts ([`PaneScrollFacts`]): scrollback
    /// depth + visible rows, read in one screen lock.
    #[must_use]
    pub fn pane_scroll_facts(&self, index: usize) -> PaneScrollFacts {
        self.with_pane(index, |pane| {
            pane.session().with_screen(|screen| PaneScrollFacts {
                scrollback_len: screen.scrollback_len(),
                visible_rows: screen.rows(),
            })
        })
    }

    /// Pane `index`'s current grid `(cols, rows)` — the emulator screen size, which
    /// tracks the last reflow target. The reflow no-op guard and an undock window's
    /// intrinsic open size read it. (It reads the emulator, not the PTY winsize
    /// directly; the two agree at steady state since [`resize`](Self::resize) keeps
    /// them synced.)
    #[must_use]
    pub fn pane_grid_size(&self, index: usize) -> (u16, u16) {
        self.with_pane(index, |pane| pane.session().dimensions())
    }

    /// Resize pane `index`'s PTY (`TIOCSWINSZ`) + emulator — the reflow control
    /// path. A closed / absent pane is TRACED and ignored (it cannot happen this
    /// increment — boot panes never close — but the swallow is honest, not silent);
    /// so is a winsize-ioctl failure.
    pub fn resize(&self, index: usize, cols: u16, rows: u16) {
        let workspace = lock(&self.workspace);
        let Some(id) = workspace.panes().get(index).map(Pane::id) else {
            tracing::trace!(
                target: "sprag_host",
                pane = index,
                "resize of a closed/absent pane ignored",
            );
            return;
        };
        if let Err(error) = workspace.resize(id, cols, rows) {
            tracing::trace!(
                target: "sprag_host",
                pane = index,
                ?error,
                "resize winsize ioctl failed; ignored",
            );
        }
    }

    /// Send a W3C `key` + `mods` to pane `index` — the CLIENT input path. Encodes to
    /// PTY bytes and writes via the shared [`crate::send_key`] SSOT (the same encoder
    /// the RPC `scene/invoke` path uses). `true` if it reached the PTY; `false` if
    /// the key is unencodable or the write failed. In-process now; over the wire this
    /// becomes an RPC send to the host's pane input surface.
    #[must_use]
    pub fn send_key(&self, index: usize, key: &str, mods: Modifiers) -> bool {
        let handle = self.with_pane(index, Pane::handle);
        crate::send_key(&handle, key, mods)
    }

    /// Write literal committed `text` to pane `index` — the IME-commit / paste client
    /// path ([`crate::send_text`]). Empty is a no-op success. `true` if it reached the
    /// PTY; `false` on a write failure.
    #[must_use]
    pub fn send_text(&self, index: usize, text: &str) -> bool {
        let handle = self.with_pane(index, Pane::handle);
        crate::send_text(&handle, text)
    }

    /// Pane `index`'s cloneable I/O handle — the ONE non-wire-shaped method (module
    /// docs). It hands out a live [`SessionHandle`] to build the GUI's own RPC input
    /// `SpragPaneExternal`s; it retires when input clients attach to the host. A
    /// client's OWN keyboard / IME go through [`send_key`](Self::send_key) /
    /// [`send_text`](Self::send_text), NOT this handle.
    #[must_use]
    pub fn pane_handle(&self, index: usize) -> SessionHandle {
        self.with_pane(index, Pane::handle)
    }

    /// Pane `index`'s full text (scrollback + visible) — the a11y text SSOT, the same
    /// string the RPC `full_text` query and the plugin capture read.
    #[must_use]
    pub fn pane_full_text(&self, index: usize) -> String {
        self.with_pane(index, |pane| pane.session().with_screen(Screen::full_text))
    }

    /// Pane `index`'s command label (the a11y node name). Owned (`String`, not `&str`)
    /// because the workspace lock is released before it returns.
    #[must_use]
    pub fn pane_command_label(&self, index: usize) -> String {
        self.with_pane(index, |pane| pane.command_label().to_owned())
    }

    /// Run `f` over the pane at tile `index` under the workspace lock — the ONE place
    /// "which pane?" resolves. The boot panes are spawned in order and never closed
    /// this increment, so `index` (sourced from a pane / focus tag) is a hard in-range
    /// invariant, not an `Option`. When a `close` path lands this becomes an
    /// `Option`-returning lookup (flagged so it is not forgotten).
    fn with_pane<R>(&self, index: usize, f: impl FnOnce(&Pane) -> R) -> R {
        let workspace = lock(&self.workspace);
        let pane = workspace
            .panes()
            .get(index)
            .expect("pane index in range (boot panes spawned 0..pane_count, never closed)");
        f(pane)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A long-lived `cat` pane (echoes stdin, keeps the PTY open across assertions).
    fn cat() -> CommandBuilder {
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg("cat");
        command.env("TERM", "dumb");
        command
    }

    #[test]
    fn spawn_grows_the_pane_count_and_exposes_geometry() {
        let host = Host::new((40, 6));
        assert_eq!(host.pane_count(), 0);
        host.spawn(cat(), "cat".to_owned(), 40, 6, None).unwrap();
        assert_eq!(host.pane_count(), 1);
        assert_eq!(host.pane_grid_size(0), (40, 6));
    }

    #[test]
    fn resize_updates_the_grid_geometry() {
        let host = Host::new((40, 6));
        host.spawn(cat(), "cat".to_owned(), 40, 6, None).unwrap();
        host.resize(0, 100, 30);
        assert_eq!(host.pane_grid_size(0), (100, 30));
    }

    #[test]
    fn send_text_reaches_the_pane_pty() {
        use std::time::{Duration, Instant};
        let host = Host::new((40, 6));
        host.spawn(cat(), "cat".to_owned(), 40, 6, None).unwrap();
        assert!(host.send_text(0, "hello"));
        // The cooked-mode `cat` echoes it back onto row 0.
        let handle = host.pane_handle(0);
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if host.pane_full_text(0).contains("hello") {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        drop(handle);
        panic!("the sent text never echoed back through the host");
    }
}
