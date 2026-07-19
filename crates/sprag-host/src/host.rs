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
//! hands out a live [`PanePtyHandle`] that cannot cross a wire; it stays an
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
use sprag_terminal::{
    CommandBuilder, LayoutSnapshot, LayoutWire, Pane, PaneId, PanePtyError, PanePtyHandle,
    SessionRegistry, Workspace,
};
use sprag_vt::Screen;

use crate::external::lock;
use crate::scope::SessionScope;

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
/// and the wire's `cells.<offset>` query family ([`CELLS_FIELD`](crate::wire::CELLS_FIELD))
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
/// * the in-process [`Host`] (this crate) — resolves the DEFAULT session's current-window
///   [`Workspace`] out of its [`SessionRegistry`] (it has no request to name another);
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
/// [`Host::pane_handle`] is deliberately NOT on this trait: a live [`PanePtyHandle`]
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

    /// The current window's LOGICAL arrangement of its TILED panes — which panes are
    /// split, in what order, at what proportion — reconciled against the live pane set,
    /// with the revision it is at.
    ///
    /// Logical ONLY — it carries no rect, because pixel geometry belongs to whichever
    /// client is rendering (a TUI and a GUI at different sizes project the same tree
    /// differently). A client PROJECTS this into its own surface; it never receives
    /// pixels here.
    ///
    /// A client re-reads exactly when [`revision`](sprag_terminal::LayoutSnapshot::revision)
    /// changes, which is what keeps its tree a projection rather than a fork.
    ///
    /// **Scope note:** this and its two writes are WINDOW state on an otherwise
    /// pane-addressed trait. They live here because both impls and the client's one
    /// `Box<dyn HostClient>` already existed; when the window surface grows (window list,
    /// select-window) they should move to their own mux/window client trait.
    fn layout(&self) -> LayoutSnapshot;

    /// Install `tree` as the current window's arrangement, returning the CANONICAL result
    /// — the write half of the arc (see [`sprag_terminal::layout`]).
    ///
    /// `expected` is the revision this gesture was authored against; the write is REFUSED if
    /// the arrangement has moved on (another client, a plugin's spawn), because a gesture
    /// means "in the layout I am looking at". The answer is the tree as the host stores it,
    /// with every divider the client minted now named, so the caller adopts it directly
    /// rather than re-reading — and a refused write answers with the arrangement actually in
    /// force, so a client always learns the truth it must project.
    fn set_layout(&self, tree: LayoutWire, expected: u64) -> LayoutSnapshot;

    /// Take pane `id` out of the tiling (`floating == true`) or put it back, returning the
    /// resulting arrangement.
    ///
    /// Floating collapses the pane's leaf host-side, so the siblings reclaim its space. A
    /// pane docked back with no gesture to place it returns to the place its float captured
    /// (`sprag_terminal::FloatHome`): the pane SEQUENCE comes home, and the shares come home
    /// too when its sibling was a bare leaf. It falls back to the arrangement's END only when
    /// the home cannot be honored — its sibling has since exited, or been floated out itself.
    /// WHERE a floating pane's window then sits on screen is the client's own business.
    ///
    /// REFUSED if it would leave the window tiling nothing (a terminal window always shows a
    /// terminal). The answer then carries the arrangement still in force, so a client that
    /// asked anyway learns the truth rather than being trusted to have checked first.
    fn set_floating(&self, id: PaneId, floating: bool) -> LayoutSnapshot;

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

/// The owner of the session / window tree (a [`SessionRegistry`]), and the
/// **in-process** arm of the [`HostClient`] protocol. See the module docs for the
/// wire-shape + ownership notes. Constructed with one empty session / window
/// ([`new`](Host::new)) and populated with [`spawn`](Host::spawn); the headless server
/// boots its panes this way and serves them, while the GUI reaches an out-of-process
/// `Host` through a wire client.
///
/// The registry is the durable authority the detach/reattach arc rests on; `Host`
/// resolves the CURRENT window's [`Workspace`] out of it per operation, so the scene
/// assembly and the control / plugin externals keep speaking a single
/// `Arc<Mutex<Workspace>>` and never learn about the tree above them.
pub struct Host {
    registry: Arc<Mutex<SessionRegistry>>,
}

impl Host {
    /// A new host over a registry with one empty session / window whose dimension-less
    /// spawns adopt `default_size`. Boot panes are added with [`spawn`](Self::spawn).
    #[must_use]
    pub fn new(default_size: (u16, u16)) -> Self {
        Self {
            registry: Arc::new(Mutex::new(SessionRegistry::new(default_size))),
        }
    }

    /// Spawn a boot pane running `command` (labelled `label`) at `cols x rows`,
    /// returning its id. `on_dirty` is the pinion-free wake hook a windowed client
    /// passes (`Some(Box::new(move || sink.request_repaint()))`, the R999
    /// `RepaintSink` seam) so this pane's output repaints the window; the headless
    /// server passes `bump_on_dirty`. `on_exit` is the "this child is gone" hook the daemon
    /// feeds to its reaper to end its own process when the last live pane dies (the headless
    /// server passes [`pane_exit_hook`](crate::pane_exit_hook); a windowed / test caller passes
    /// `None`). Both are `Box<dyn Fn() + Send>` (not pinion types), so the display and
    /// lifetime concerns live above while the spawn lives here.
    ///
    /// # Errors
    ///
    /// Returns [`PanePtyError`] if the pseudoterminal or child cannot be started.
    pub fn spawn(
        &self,
        command: CommandBuilder,
        label: String,
        cols: u16,
        rows: u16,
        on_dirty: Option<Box<dyn Fn() + Send>>,
        on_exit: Option<Box<dyn Fn() + Send>>,
    ) -> Result<PaneId, PanePtyError> {
        let workspace = self.workspace();
        lock(&workspace).spawn_with_dirty(command, label, cols, rows, on_dirty, on_exit)
    }

    /// The DEFAULT session's current window pane pool — this arm's panes.
    ///
    /// Resolved out of the [`SessionRegistry`] on each call (a cloned `Arc`, not a borrow),
    /// so once window switching lands the next call picks up the new current window with no
    /// re-plumbing. The one place the raw `Workspace` handle escapes; the [`HostClient`]
    /// methods are how a client reaches panes.
    ///
    /// The DEFAULT session, because an in-process caller has no request to name another and
    /// this is what an unscoped one gets ([`SessionScope::unscoped`]). A caller that wants a
    /// specific session comes over the wire and names it — see
    /// [`SESSION_PARAM`](crate::wire::SESSION_PARAM).
    #[must_use]
    pub fn workspace(&self) -> Arc<Mutex<Workspace>> {
        Arc::clone(self.scope().workspace())
    }

    /// This arm's scope: the default session (see [`workspace`](Self::workspace)).
    fn scope(&self) -> SessionScope {
        SessionScope::unscoped(&self.registry)
    }

    /// The mux state tree, for the scene-as-data assembly
    /// ([`workspace_scene`](crate::workspace_scene)) and the mux control external. Each
    /// request's [`SessionScope`] is resolved once at the door and passed to the assembly;
    /// the registry travels alongside it for the mux external's registry-WIDE operations
    /// (resolving a scoped window by name, creating a session, listing sessions) — the pane
    /// pool a request acts on comes from the scope, not from a re-resolution here. The plugin
    /// host deliberately gets [`workspace`](Self::workspace) instead — a plugin operates on a
    /// pane pool, not a session tree (Interface Segregation).
    #[must_use]
    pub fn registry(&self) -> &Arc<Mutex<SessionRegistry>> {
        &self.registry
    }

    /// Pane `id`'s cloneable I/O handle — the ONE non-wire-shaped method (module
    /// docs), so it is NOT on [`HostClient`]. It hands out a live [`PanePtyHandle`]
    /// to build the headless host's own RPC input `SpragPaneExternal`s; a display
    /// client's OWN keyboard / IME go through [`HostClient::send_key`] /
    /// [`HostClient::send_text`], NOT this handle. `None` for an absent id.
    #[must_use]
    pub fn pane_handle(&self, id: PaneId) -> Option<PanePtyHandle> {
        self.with_pane_id(id, Pane::handle)
    }

    /// Run `f` over the pane with `id` under the workspace lock — the ONE place an id
    /// resolves to a pane. `None` if no live pane has that id (closed / never existed),
    /// so every [`HostClient`] method returns its graceful default for an absent id
    /// rather than panicking (the widened identity-addressed contract).
    fn with_pane_id<R>(&self, id: PaneId, f: impl FnOnce(&Pane) -> R) -> Option<R> {
        let workspace = self.workspace();
        lock(&workspace).pane(id).map(f)
    }
}

/// The arrangement of the window `scope` names, self-healed against its live pane set, plus
/// the revision it is at — the ONE place this sequence exists (the in-process
/// [`Host::layout`], both write paths below, and the mux control external's `layout` slot all
/// call it).
///
/// It is single-sourced because its CORRECTNESS IS ITS ORDERING: the pane ids are read
/// under the WORKSPACE lock and the reconcile runs under the REGISTRY lock, taken
/// SEQUENTIALLY. Fusing them (`lock(registry)…reconcile(&lock(pool)…)`) would nest the
/// workspace lock inside the registry lock and invert the order the rest of the host
/// holds them in. A rule that load-bearing must live in one function, not in a comment
/// copied to each caller.
///
/// A pane spawning / closing between the two steps leaves the arrangement one read
/// behind; the next read heals it (the tree is not the membership authority — the
/// workspace is).
///
/// `None` if no session carries `scope`'s name. Unreachable through a wire request, whose
/// scope resolved before the scene was assembled — but the registry is the authority on
/// which sessions exist, and asking it again at the moment of use is what keeps this honest
/// once a session can be killed.
pub(crate) fn reconciled_layout(
    registry: &Arc<Mutex<SessionRegistry>>,
    scope: &SessionScope,
) -> Option<LayoutSnapshot> {
    // The pool travels with the scope, so there is no lookup to fail here and no lock to
    // take — the fallible half is the WINDOW below, which only the registry can hand out.
    let panes: Vec<PaneId> = lock(scope.workspace())
        .panes()
        .iter()
        .map(Pane::id)
        .collect();
    let mut registry = lock(registry);
    let window = registry.window_mut(scope.session())?;
    let tree = LayoutWire::from(window.reconcile_layout(&panes));
    let mut floating: Vec<PaneId> = window.floating().iter().copied().collect();
    floating.sort_unstable(); // a HashSet's order is arbitrary; the wire must be stable or
    // a client watching for change would see one where there is none
    Some(LayoutSnapshot {
        revision: window.layout_revision(),
        tree,
        floating,
    })
}

/// Install a client's settled arrangement, then answer with the canonical one — the ONE
/// place a write lands (the in-process [`Host::set_layout`] and the mux `set_layout` action
/// share it).
///
/// A malformed arrangement is TRACED and dropped, and the caller is answered with the
/// layout that is actually in force. Rejecting is the honest outcome: the alternative —
/// storing a tree that violates the type's invariants — would outlive the buggy client that
/// sent it and corrupt the session for every later one.
///
/// The registry lock is released before [`reconciled_layout`] takes the workspace lock, so
/// the two are sequential here as everywhere else.
pub(crate) fn set_layout(
    registry: &Arc<Mutex<SessionRegistry>>,
    scope: &SessionScope,
    tree: LayoutWire,
    expected: u64,
) -> Option<LayoutSnapshot> {
    match lock(registry).window_mut(scope.session()) {
        Some(window) => {
            if let Err(error) = window.set_layout(tree, Some(expected)) {
                tracing::warn!(
                    target: "sprag_host",
                    %error,
                    session = scope.session(),
                    "a client's arrangement was rejected; keeping the one in force",
                );
            }
        }
        None => return None,
    }
    reconciled_layout(registry, scope)
}

/// Take a pane out of the tiling or put it back, then answer with the resulting
/// arrangement — the ONE place a float lands (see [`HostClient::set_floating`]).
pub(crate) fn set_floating(
    registry: &Arc<Mutex<SessionRegistry>>,
    scope: &SessionScope,
    id: PaneId,
    floating: bool,
) -> Option<LayoutSnapshot> {
    // The window's invariant needs the live pane set to judge "would this untile the last
    // one?", so it is read under the WORKSPACE lock and handed down — the same sequential
    // order as everywhere else, never nested.
    let panes: Vec<PaneId> = lock(scope.workspace())
        .panes()
        .iter()
        .map(Pane::id)
        .collect();
    if !lock(registry)
        .window_mut(scope.session())?
        .set_floating(id, floating, &panes)
    {
        tracing::debug!(
            target: "sprag_host",
            %id,
            session = scope.session(),
            "refused to float the last tiled pane; the window keeps a terminal",
        );
    }
    reconciled_layout(registry, scope)
}

impl HostClient for Host {
    fn pane_ids(&self) -> Vec<PaneId> {
        let workspace = self.workspace();
        lock(&workspace).panes().iter().map(Pane::id).collect()
    }

    fn pane_cells(&self, id: PaneId, offset_lines: usize) -> GridBuffer {
        self.with_pane_id(id, |pane| crate::pane_cells(pane.pty(), offset_lines))
            .unwrap_or_else(|| GridBuffer::new(1, 1))
    }

    fn pane_scroll_facts(&self, id: PaneId) -> PaneScrollFacts {
        self.with_pane_id(id, |pane| {
            pane.pty().with_screen(PaneScrollFacts::from_screen)
        })
        .unwrap_or(PaneScrollFacts {
            scrollback_len: 0,
            visible_rows: 1,
        })
    }

    fn pane_grid_size(&self, id: PaneId) -> (u16, u16) {
        self.with_pane_id(id, |pane| pane.pty().dimensions())
            .unwrap_or((1, 1))
    }

    /// A closed / absent pane is TRACED and ignored (the swallow is honest, not
    /// silent); so is a winsize-ioctl failure.
    fn resize(&self, id: PaneId, cols: u16, rows: u16) {
        let ws = self.workspace();
        let workspace = lock(&ws);
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
        self.with_pane_id(id, |pane| pane.pty().with_screen(Screen::full_text))
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

    /// The DEFAULT session's window (see [`Host::workspace`]).
    fn layout(&self) -> LayoutSnapshot {
        reconciled_layout(&self.registry, &self.scope()).expect(DEFAULT_ALWAYS_RESOLVES)
    }

    fn set_layout(&self, tree: LayoutWire, expected: u64) -> LayoutSnapshot {
        set_layout(&self.registry, &self.scope(), tree, expected).expect(DEFAULT_ALWAYS_RESOLVES)
    }

    fn set_floating(&self, id: PaneId, floating: bool) -> LayoutSnapshot {
        set_floating(&self.registry, &self.scope(), id, floating).expect(DEFAULT_ALWAYS_RESOLVES)
    }
}

/// Why the in-process arm may unwrap a scoped layout read that a wire caller must handle.
///
/// The `Option` those three return is about a NAMED session having gone; this arm names none
/// — it scopes to the default, which [`SessionRegistry::default_session`] makes total by
/// construction (`sessions` is seeded non-empty and has no removal path). The wire path never
/// unwraps: it answers a vanished scope with a refusal, because there a name really can come
/// from a client and really can be stale.
///
/// This is a panic guarding an invariant, not a shortcut around one. If the daemon increment
/// gives `sessions` a way to shrink, this is the site that must be revisited — and it says so
/// loudly rather than silently serving an empty arrangement, which is the failure that would
/// actually reach a user.
const DEFAULT_ALWAYS_RESOLVES: &str = "the default session resolves by construction: it is the first of a never-empty, \
     never-shrinking session list (SessionRegistry::default_session)";

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
        let id = host
            .spawn(cat(), "cat".to_owned(), 40, 6, None, None)
            .unwrap();
        assert_eq!(host.pane_ids(), vec![id]);
        assert_eq!(host.pane_grid_size(id), (40, 6));
    }

    #[test]
    fn resize_updates_the_grid_geometry() {
        let host = Host::new((40, 6));
        let id = host
            .spawn(cat(), "cat".to_owned(), 40, 6, None, None)
            .unwrap();
        host.resize(id, 100, 30);
        assert_eq!(host.pane_grid_size(id), (100, 30));
    }

    #[test]
    fn send_text_reaches_the_pane_pty() {
        use std::time::{Duration, Instant};
        let host = Host::new((40, 6));
        let id = host
            .spawn(cat(), "cat".to_owned(), 40, 6, None, None)
            .unwrap();
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
