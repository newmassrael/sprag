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
/// point of topology B. Each method addresses a pane by its host [`PaneId`] — the
/// host's OWN stable identity (monotonic, never reused), NOT a display slot: "slots"
/// are a GUI display concept the display client maps onto these ids ITSELF (see
/// `sprag-gui`'s `SlotView`), and the host has no notion of them. [`pane_ids`](HostClient::pane_ids)
/// is the membership source; an absent id returns each method's graceful default.
///
/// [`Host::pane_handle`] is deliberately NOT on this trait: a live [`SessionHandle`]
/// cannot cross a wire, so it stays an inherent `Host` method used only to build
/// in-process input surfaces (retired as input clients attach to the host).
pub trait HostClient {
    /// The host's live pane identities, in host order — the ONE membership source a
    /// display client reads (it maps these to its own display slots). Replaces the
    /// former `pane_count` / `occupied_slots` (slot concepts that moved to the GUI's
    /// `SlotView`).
    ///
    /// CONTRACT: yields exactly the panes this client can RENDER right now — membership is
    /// "renderable now", not merely "exists". An impl MAY briefly omit a pane the host has
    /// but it cannot yet render (e.g. a frame not fetched), so a consumer never maps a
    /// frameless pane; the omitted pane appears once it becomes renderable. An impl that
    /// renders the host's state directly reports the live set with no lag; a
    /// transport-mediated impl may lag by however long it takes a new pane to become
    /// renderable. (Each impl's own `pane_ids` documents how it honors this.)
    fn pane_ids(&self) -> Vec<PaneId>;

    /// Pane `id`'s cell DATA scrolled `offset_lines` rows up — the paint buffer a
    /// client renders. `offset_lines == 0` is the live view; a larger offset windows
    /// into scrollback (self-clamped to the retained depth). A `1x1` placeholder if
    /// `id` is absent.
    fn pane_cells(&self, id: PaneId, offset_lines: usize) -> GridBuffer;

    /// Pane `id`'s non-cell per-frame facts ([`PaneScrollFacts`]): scrollback depth +
    /// visible rows. A zero-depth / one-row default if `id` is absent.
    fn pane_scroll_facts(&self, id: PaneId) -> PaneScrollFacts;

    /// Pane `id`'s current grid `(cols, rows)` — the emulator screen size, which tracks
    /// the last reflow target (the reflow no-op guard + an undock window's intrinsic
    /// open size read it). `(1, 1)` if `id` is absent.
    fn pane_grid_size(&self, id: PaneId) -> (u16, u16);

    /// Resize pane `id`'s PTY (`TIOCSWINSZ`) + emulator — the reflow control path. A
    /// no-op for an absent id.
    fn resize(&self, id: PaneId, cols: u16, rows: u16);

    /// Send a W3C `key` + `mods` to pane `id` — the CLIENT input path. `true` if it
    /// reached the PTY; `false` if `id` is absent, the key is unencodable, or the send
    /// failed.
    #[must_use]
    fn send_key(&self, id: PaneId, key: &str, mods: Modifiers) -> bool;

    /// Write literal committed `text` to pane `id` — the IME-commit / paste client
    /// path. Empty is a no-op success. `true` if it reached the PTY.
    #[must_use]
    fn send_text(&self, id: PaneId, text: &str) -> bool;

    /// Pane `id`'s full text (scrollback + visible) — the a11y text SSOT. Empty if
    /// `id` is absent.
    fn pane_full_text(&self, id: PaneId) -> String;

    /// Pane `id`'s command label (the a11y node name). Empty if `id` is absent.
    fn pane_command_label(&self, id: PaneId) -> String;

    /// Pane `id`'s child-reported window TITLE (`OSC 0` / `OSC 2`), or `None` if the
    /// child never set one (or `id` is absent).
    ///
    /// This is LIVE, CHILD-CONTROLLED state — a shell's `PROMPT_COMMAND` rewrites it on
    /// every prompt, vim sets the edited file, ssh the remote host — so it is strictly a
    /// DISPLAY name: a surface prefers it and falls back to a stable one
    /// ([`Self::pane_command_label`] / the client's own panel id). Pane IDENTITY (ids,
    /// tags, panel ids) must NEVER derive from it, because the child sets it freely.
    /// Distinct from `pane_command_label`, which names what was LAUNCHED and never
    /// changes — conflating the two would silently rewrite the a11y node name too.
    fn pane_title(&self, id: PaneId) -> Option<String>;
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

    /// Pane `id`'s cloneable I/O handle — the ONE non-wire-shaped method (module
    /// docs), so it is NOT on [`HostClient`]. It hands out a live [`SessionHandle`]
    /// to build the headless host's own RPC input `SpragPaneExternal`s; a display
    /// client's OWN keyboard / IME go through [`HostClient::send_key`] /
    /// [`HostClient::send_text`], NOT this handle. `None` for an absent id.
    #[must_use]
    pub fn pane_handle(&self, id: PaneId) -> Option<SessionHandle> {
        self.with_pane_id(id, Pane::handle)
    }

    /// Run `f` over the pane with `id` under the workspace lock — the ONE place an id
    /// resolves to a pane. `None` if no live pane has that id (closed / never existed),
    /// so every [`HostClient`] method returns its graceful default for an absent id
    /// rather than panicking (the widened identity-addressed contract).
    fn with_pane_id<R>(&self, id: PaneId, f: impl FnOnce(&Pane) -> R) -> Option<R> {
        lock(&self.workspace).pane(id).map(f)
    }
}

impl HostClient for Host {
    fn pane_ids(&self) -> Vec<PaneId> {
        lock(&self.workspace).panes().iter().map(Pane::id).collect()
    }

    fn pane_cells(&self, id: PaneId, offset_lines: usize) -> GridBuffer {
        self.with_pane_id(id, |pane| crate::pane_cells(pane.session(), offset_lines))
            .unwrap_or_else(|| GridBuffer::new(1, 1))
    }

    fn pane_scroll_facts(&self, id: PaneId) -> PaneScrollFacts {
        self.with_pane_id(id, |pane| {
            pane.session().with_screen(PaneScrollFacts::from_screen)
        })
        .unwrap_or(PaneScrollFacts {
            scrollback_len: 0,
            visible_rows: 1,
        })
    }

    fn pane_grid_size(&self, id: PaneId) -> (u16, u16) {
        self.with_pane_id(id, |pane| pane.session().dimensions())
            .unwrap_or((1, 1))
    }

    /// A closed / absent pane is TRACED and ignored (the swallow is honest, not
    /// silent); so is a winsize-ioctl failure.
    fn resize(&self, id: PaneId, cols: u16, rows: u16) {
        let workspace = lock(&self.workspace);
        if workspace.pane(id).is_none() {
            tracing::trace!(target: "sprag_host", %id, "resize of a closed/absent pane ignored");
            return;
        }
        if let Err(error) = workspace.resize(id, cols, rows) {
            tracing::trace!(target: "sprag_host", %id, ?error, "resize winsize ioctl failed; ignored");
        }
    }

    /// Encodes to PTY bytes and writes via the shared [`crate::send_key`] SSOT (the
    /// same encoder the RPC `scene/invoke` path uses); `false` for an absent id.
    fn send_key(&self, id: PaneId, key: &str, mods: Modifiers) -> bool {
        self.with_pane_id(id, Pane::handle)
            .is_some_and(|handle| crate::send_key(&handle, key, mods))
    }

    fn send_text(&self, id: PaneId, text: &str) -> bool {
        self.with_pane_id(id, Pane::handle)
            .is_some_and(|handle| crate::send_text(&handle, text))
    }

    fn pane_full_text(&self, id: PaneId) -> String {
        self.with_pane_id(id, |pane| pane.session().with_screen(Screen::full_text))
            .unwrap_or_default()
    }

    /// Owned (`String`, not `&str`) because the workspace lock is released before it
    /// returns.
    fn pane_command_label(&self, id: PaneId) -> String {
        self.with_pane_id(id, |pane| pane.command_label().to_owned())
            .unwrap_or_default()
    }

    /// Flattens "absent pane" and "pane set no title" to the same `None` — both mean
    /// "no title to display", and a caller that must distinguish them has `pane_ids`.
    fn pane_title(&self, id: PaneId) -> Option<String> {
        self.with_pane_id(id, Pane::title).flatten()
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
    fn spawn_grows_the_pane_set_and_exposes_geometry() {
        let host = Host::new((40, 6));
        assert!(host.pane_ids().is_empty());
        let id = host.spawn(cat(), "cat".to_owned(), 40, 6, None).unwrap();
        assert_eq!(host.pane_ids(), vec![id]);
        assert_eq!(host.pane_grid_size(id), (40, 6));
    }

    #[test]
    fn resize_updates_the_grid_geometry() {
        let host = Host::new((40, 6));
        let id = host.spawn(cat(), "cat".to_owned(), 40, 6, None).unwrap();
        host.resize(id, 100, 30);
        assert_eq!(host.pane_grid_size(id), (100, 30));
    }

    #[test]
    fn send_text_reaches_the_pane_pty() {
        use std::time::{Duration, Instant};
        let host = Host::new((40, 6));
        let id = host.spawn(cat(), "cat".to_owned(), 40, 6, None).unwrap();
        assert!(host.send_text(id, "hello"));
        // The cooked-mode `cat` echoes it back into the pane's screen.
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if host.pane_full_text(id).contains("hello") {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("the sent text never echoed back through the host");
    }

    #[test]
    fn an_absent_id_returns_graceful_defaults() {
        // The widened identity contract (R121): a `PaneId` with no live pane no-ops /
        // placeholders rather than panicking (was `with_pane`'s `.expect` before).
        let host = Host::new((40, 6));
        let ghost = PaneId(999);
        assert_eq!(host.pane_grid_size(ghost), (1, 1));
        assert_eq!(
            (
                host.pane_cells(ghost, 0).cols(),
                host.pane_cells(ghost, 0).rows()
            ),
            (1, 1)
        );
        assert!(!host.send_text(ghost, "x"));
        assert!(!host.send_key(ghost, "a", Modifiers::default()));
        assert!(host.pane_full_text(ghost).is_empty());
        assert!(host.pane_command_label(ghost).is_empty());
        assert!(host.pane_handle(ghost).is_none());
        host.resize(ghost, 10, 10); // no panic
    }
}
