//! The host — the single [`Workspace`] owner, used two ways.
//!
//! [`Host`] owns the one live [`Workspace`] (and thus the PTYs) and serves the
//! typed [`HostClient`] protocol over it: cell DATA, per-frame scroll facts,
//! resize control, INPUT (`send_key` / `send_text`), input handles, and pane
//! text. This is the single home for "who owns the panes", shared by both
//! frontends (the north-star's two-frontend platform, [DESIGN.md §5]):
//!
//! * the **GUI** (`sprag-gui`) reaches every pane through a `Box<dyn HostClient>`
//!   — a wire client (`WireHost`) attached to a `sprag-term` host PROCESS — so the
//!   display client is a structurally-separate client of this host (topology B);
//! * the **headless server** (`sprag-term`) boots its pane through a `Host`
//!   in-process and wraps it in [`HostState`](crate::HostState) to serve the
//!   scene-as-data RPC surface an AI peer (and the GUI) drives.
//!
//! ## The protocol (shaped like the wire)
//!
//! [`HostClient`] is that protocol as a Rust trait, with two impls: the
//! in-process [`Host`] (below) and the GUI's `WireHost` (the same surface over an
//! RPC socket). Its methods are:
//!
//! * cell DATA ([`pane_cells`](HostClient::pane_cells)) + the non-cell per-frame
//!   facts that ride alongside it ([`pane_scroll_facts`](HostClient::pane_scroll_facts));
//! * resize control ([`resize`](HostClient::resize)) + grid geometry
//!   ([`pane_grid_size`](HostClient::pane_grid_size));
//! * INPUT — the display client's keyboard / IME are client SENDs
//!   ([`send_key`](HostClient::send_key) / [`send_text`](HostClient::send_text)),
//!   encoded by the shared [`crate::send_key`] / [`crate::send_text`] SSOT (the
//!   same encoder the RPC `scene/invoke` path uses); the wire client's
//!   implementation sends them as an RPC `scene/invoke` to the host's pane input
//!   surface, the in-process `Host` writes the PTY directly;
//! * pane text ([`pane_full_text`](HostClient::pane_full_text) /
//!   [`pane_command_label`](HostClient::pane_command_label)) for the a11y tree.
//!
//! The ONE method NOT on the trait is [`pane_handle`](Host::pane_handle) — it
//! hands out a live [`SessionHandle`] that cannot cross a wire; it stays an
//! inherent [`Host`] method used only by in-process input surfaces, and retires as
//! input clients attach to the host.
//!
//! ## Ownership
//!
//! The `Workspace` lives behind `Arc<Mutex<_>>` — the shape [`HostState`](crate::HostState) and the
//! plugin/control externals already share (a background plugin run reads a pane
//! from a worker thread, so the pool is genuinely shared). Presentation (cell
//! metric, font size) is NOT here — that is the display client's own state.

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
/// [`pane_cells`](HostClient::pane_cells) buffer as one message (not a separate
/// round-trip). Named "facts", not "dims", so it is never confused with the grid
/// geometry ([`pane_grid_size`](HostClient::pane_grid_size)) — `scrollback_len` is
/// a history depth, not a dimension.
///
/// This is the ONE definition of the frame's non-cell field set: the in-process
/// client reads it via [`Host::pane_scroll_facts`](HostClient::pane_scroll_facts),
/// and the wire `cells` action ([`SpragPaneExternal::read_cells`](crate::pane))
/// flattens the SAME type into its JSON frame (serde-derived), so the field
/// names + wire keys cannot drift between the two clients. `Serialize` /
/// `Deserialize` for the wire; `Eq` so a test can compare two reads.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PaneScrollFacts {
    pub scrollback_len: usize,
    pub visible_rows: u16,
}

impl PaneScrollFacts {
    /// Read the non-cell facts from a live `screen` — the SINGLE population site,
    /// shared by [`Host::pane_scroll_facts`](HostClient::pane_scroll_facts) and the
    /// wire `cells` action, so the two never disagree on how a fact is derived
    /// (adding a fact edits only here + the struct).
    pub(crate) fn from_screen(screen: &Screen) -> Self {
        Self {
            scrollback_len: screen.scrollback_len(),
            visible_rows: screen.rows(),
        }
    }
}

/// The typed client protocol a display client reaches the host's panes through —
/// the topology-B wire contract expressed as a trait, with two impls:
///
/// * the in-process [`Host`] (this crate) — direct `Arc<Mutex<Workspace>>` access;
/// * the GUI's wire client (`sprag-gui`'s `WireHost`) — the SAME method surface
///   over an RPC socket to a `sprag-term` host process.
///
/// The GUI holds a `Box<dyn HostClient>` and reaches every pane ONLY through these
/// methods, so the frontend code is identical whether the `Workspace` lives in its
/// own process (in-process) or another (wire) — that structural equivalence is the
/// point of topology B. Each method addresses a pane by its display SLOT `index`,
/// drawn from [`occupied_slots`](HostClient::occupied_slots) (NOT the dense range
/// `0..pane_count()` — a mirroring impl may leave holes, see that method); a wire impl
/// maps the slot to the host's [`PaneId`] internally.
///
/// [`Host::pane_handle`] is deliberately NOT on this trait: a live [`SessionHandle`]
/// cannot cross a wire, so it stays an inherent `Host` method used only to build
/// in-process input surfaces (retired as input clients attach to the host).
pub trait HostClient {
    /// The number of live panes.
    fn pane_count(&self) -> usize;

    /// The live display SLOTS, in display order — the set a client ITERATES instead
    /// of assuming a contiguous `0..pane_count()`. Every other method addresses a
    /// pane by its slot (`index` == slot).
    ///
    /// The default is the dense range `0..pane_count()` — correct for the in-process
    /// [`Host`], whose panes are a simple list. A wire client that MIRRORS a host's
    /// pane set overrides this: it keeps a pane's slot STABLE across pane-set changes
    /// (why is the client's own concern), so a closed pane leaves a HOLE and the set
    /// can be non-contiguous. Callers must therefore iterate this set, not
    /// `0..pane_count()`.
    fn occupied_slots(&self) -> Vec<usize> {
        (0..self.pane_count()).collect()
    }

    /// Whether display slot `slot` currently holds a pane. The `slot < pane_count()`
    /// index guard is WRONG once slots go non-contiguous (a slot past the count can
    /// be occupied while a lower one is a hole), so callers ask this. Default derives
    /// it from [`occupied_slots`](HostClient::occupied_slots).
    fn is_pane_occupied(&self, slot: usize) -> bool {
        self.occupied_slots().contains(&slot)
    }

    /// Pane `index`'s cell DATA scrolled `offset_lines` rows up — the paint buffer
    /// a client renders. `offset_lines == 0` is the live view; a larger offset
    /// windows into scrollback (self-clamped to the retained depth).
    fn pane_cells(&self, index: usize, offset_lines: usize) -> GridBuffer;

    /// Pane `index`'s non-cell per-frame facts ([`PaneScrollFacts`]): scrollback
    /// depth + visible rows.
    fn pane_scroll_facts(&self, index: usize) -> PaneScrollFacts;

    /// Pane `index`'s current grid `(cols, rows)` — the emulator screen size, which
    /// tracks the last reflow target (the reflow no-op guard + an undock window's
    /// intrinsic open size read it).
    fn pane_grid_size(&self, index: usize) -> (u16, u16);

    /// Resize pane `index`'s PTY (`TIOCSWINSZ`) + emulator — the reflow control path.
    fn resize(&self, index: usize, cols: u16, rows: u16);

    /// Send a W3C `key` + `mods` to pane `index` — the CLIENT input path. `true` if
    /// it reached the PTY; `false` if the key is unencodable or the send failed.
    #[must_use]
    fn send_key(&self, index: usize, key: &str, mods: Modifiers) -> bool;

    /// Write literal committed `text` to pane `index` — the IME-commit / paste
    /// client path. Empty is a no-op success. `true` if it reached the PTY.
    #[must_use]
    fn send_text(&self, index: usize, text: &str) -> bool;

    /// Pane `index`'s full text (scrollback + visible) — the a11y text SSOT.
    fn pane_full_text(&self, index: usize) -> String;

    /// Pane `index`'s command label (the a11y node name).
    fn pane_command_label(&self, index: usize) -> String;
}

/// The single [`Workspace`] owner (topology B), and the **in-process** arm of the
/// [`HostClient`] protocol. See the module docs for the wire-shape + ownership
/// notes. Constructed empty ([`new`](Host::new)) and populated with
/// [`spawn`](Host::spawn); the headless server boots its panes this way and serves
/// them, while the GUI reaches an out-of-process `Host` through a wire client.
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
    /// place the raw `Workspace` handle escapes; the [`HostClient`] methods are how a
    /// client reaches panes.
    #[must_use]
    pub fn workspace(&self) -> &Arc<Mutex<Workspace>> {
        &self.workspace
    }

    /// Pane `index`'s cloneable I/O handle — the ONE non-wire-shaped method (module
    /// docs), so it is NOT on [`HostClient`]. It hands out a live [`SessionHandle`]
    /// to build the headless host's own RPC input `SpragPaneExternal`s; a display
    /// client's OWN keyboard / IME go through [`HostClient::send_key`] /
    /// [`HostClient::send_text`], NOT this handle.
    #[must_use]
    pub fn pane_handle(&self, index: usize) -> SessionHandle {
        self.with_pane(index, Pane::handle)
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

impl HostClient for Host {
    fn pane_count(&self) -> usize {
        lock(&self.workspace).panes().len()
    }

    fn pane_cells(&self, index: usize, offset_lines: usize) -> GridBuffer {
        self.with_pane(index, |pane| {
            crate::pane_cells(pane.session(), offset_lines)
        })
    }

    fn pane_scroll_facts(&self, index: usize) -> PaneScrollFacts {
        self.with_pane(index, |pane| {
            pane.session().with_screen(PaneScrollFacts::from_screen)
        })
    }

    fn pane_grid_size(&self, index: usize) -> (u16, u16) {
        self.with_pane(index, |pane| pane.session().dimensions())
    }

    /// A closed / absent pane is TRACED and ignored (it cannot happen this increment
    /// — boot panes never close — but the swallow is honest, not silent); so is a
    /// winsize-ioctl failure.
    fn resize(&self, index: usize, cols: u16, rows: u16) {
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

    /// Encodes to PTY bytes and writes via the shared [`crate::send_key`] SSOT (the
    /// same encoder the RPC `scene/invoke` path uses).
    fn send_key(&self, index: usize, key: &str, mods: Modifiers) -> bool {
        let handle = self.with_pane(index, Pane::handle);
        crate::send_key(&handle, key, mods)
    }

    fn send_text(&self, index: usize, text: &str) -> bool {
        let handle = self.with_pane(index, Pane::handle);
        crate::send_text(&handle, text)
    }

    fn pane_full_text(&self, index: usize) -> String {
        self.with_pane(index, |pane| pane.session().with_screen(Screen::full_text))
    }

    /// Owned (`String`, not `&str`) because the workspace lock is released before it
    /// returns.
    fn pane_command_label(&self, index: usize) -> String {
        self.with_pane(index, |pane| pane.command_label().to_owned())
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
        // The cooked-mode `cat` echoes it back into the pane's screen.
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if host.pane_full_text(0).contains("hello") {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("the sent text never echoed back through the host");
    }
}
