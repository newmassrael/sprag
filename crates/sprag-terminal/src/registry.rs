//! The session / window hierarchy — the durable server's client-independent state.
//!
//! tmux's core value is that terminal state outlives any client: detach, the session
//! keeps running, reattach and your windows + panes are exactly as you left them. That
//! demands the state live in an authority no client can take down. sprag's PTYs already
//! live host-side; this module adds the tree ABOVE the pane pool that makes the rest of
//! the detach/reattach arc (and windows/tabs) possible:
//!
//! ```text
//! SessionRegistry            -- all sessions + the current one + the ONE global id counter
//!   Session (named)          -- the attach unit: an ordered set of windows + a current one
//!     Window (named)         -- the layout unit: a pane pool + its LayoutTree
//!       Workspace            -- the pane pool (crate::workspace), shared id counter
//!         Pane (PTY + emulator)
//! ```
//!
//! ## What this layer does and does not own
//!
//! A [`Window`] holds a [`Workspace`] (its panes), a [`LayoutTree`] (how the tiled ones are
//! arranged), and the set of panes a client has FLOATED out of the tiling. This layer is
//! deliberately pinion-free (producer concern) and keeps the plugin/control surfaces
//! speaking `Arc<Mutex<Workspace>>` — a plugin operates on a *workspace*, not a session
//! tree (Interface Segregation). The host resolves "which workspace is current" through
//! this registry and hands that one workspace down, so the surfaces above never learn
//! about sessions or windows until they must.
//!
//! ## The load-bearing invariant
//!
//! Every window's [`Workspace`] shares ONE `Arc<AtomicU64>` id counter
//! ([`Workspace::with_id_source`]), so a [`PaneId`] is unique across the
//! WHOLE registry, monotonic, and never reused. That is what lets a pane be addressed
//! by id alone regardless of which window/session holds it — the per-pane wire path
//! stays window-free, and adding windows later needs no address migration.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use crate::PaneId;
use crate::layout::{FloatHome, LayoutError, LayoutTree, LayoutWire};
use crate::workspace::Workspace;

/// One window: a named layout unit owning a pane pool, how its tiled panes are ARRANGED,
/// and which of them a client has FLOATED out of the tiling.
///
/// The [`LayoutTree`] is the logical arrangement only (no pixels — see
/// [`layout`](crate::layout)); it lives here, client-independently, so a detached session
/// keeps the user's layout. Membership stays the [`Workspace`]'s: the arrangement
/// self-heals against the pane set via [`LayoutTree::reconcile`], since pane lifecycle runs
/// through the workspace directly.
///
/// ## Why float lives here and not in the client
///
/// A floating pane is one the user took OUT of the tiling — that is the same class of fact
/// as how the rest are split, so it is session state and it belongs on the same side of the
/// wire. Keeping it here is also what makes the client's tree an exact projection: the host
/// reconciles over `panes − floating`, so what a client renders IS [`Self::layout`], with no
/// client-side filter to diverge and no merge to reconstruct on the way back. The seam holds
/// on the same line the rest of the module draws: WHICH panes are tiled is logical and lives
/// here; WHERE a floating window sits on the user's screen is pixels and never does.
pub struct Window {
    name: String,
    workspace: Arc<Mutex<Workspace>>,
    layout: LayoutTree,
    /// Panes taken out of the tiling — [`layout`](Self::layout) holds no leaf for these.
    /// Pruned against the live pool by [`reconcile_layout`](Self::reconcile_layout), so a
    /// floating pane that exits leaves no entry behind.
    floating: HashSet<PaneId>,
    /// Where each floated pane came FROM, so it docks back into its own place rather than
    /// at the end ([`FloatHome`]).
    ///
    /// A sidecar, not an authority: `floating` alone says which panes are out, and a missing
    /// or unhonorable home costs an append, never correctness. It is deliberately NOT the
    /// same map as `floating`, because the two have different lifetimes — a home is captured
    /// when the pane floats and spent when the pane is TILED AGAIN, which is one
    /// [`reconcile_layout`](Self::reconcile_layout) LATER than the moment it stops floating.
    /// Keyed in one map, the dock-back that clears the float flag would drop the home on the
    /// floor before the leaf it was captured for could be placed.
    homes: HashMap<PaneId, FloatHome>,
    layout_revision: u64,
}

impl Window {
    /// The window's display name (default `"0"`, `"1"`, …; user-renamable later).
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The window's pane pool — the `Arc<Mutex<Workspace>>` the host hands to the scene
    /// assembly and the control / plugin externals.
    #[must_use]
    pub fn workspace(&self) -> &Arc<Mutex<Workspace>> {
        &self.workspace
    }

    /// How this window's TILED panes are arranged (logical only, never pixels).
    ///
    /// May lag the pane set until [`reconcile_layout`](Self::reconcile_layout) folds in a
    /// spawn/close that went straight to the [`Workspace`] — read it through the host,
    /// which reconciles first.
    #[must_use]
    pub fn layout(&self) -> &LayoutTree {
        &self.layout
    }

    /// Which panes are floated out of the tiling (see the type docs).
    #[must_use]
    pub fn floating(&self) -> &HashSet<PaneId> {
        &self.floating
    }

    /// How many times this window's arrangement has CHANGED — the number a client watches
    /// to know its projection is stale.
    ///
    /// Bumped only on a real change (a write that differs, a reconcile that moves a leaf, a
    /// float), never on a read, so a client that re-reads on every bump does no wasted work
    /// and — more importantly — never re-projects on top of a gesture the user is mid-way
    /// through. Monotonic for the window's life.
    #[must_use]
    pub fn layout_revision(&self) -> u64 {
        self.layout_revision
    }

    /// Self-heal the arrangement against `panes` (the workspace's live ids) and return it.
    ///
    /// Reconciles over the TILED panes (`panes − floating`), so a floated pane holds no
    /// leaf, and prunes float entries whose pane has exited — the float set is a view of
    /// the pool, never an authority over it.
    ///
    /// A pane that is tiled again lands at the [`FloatHome`] its float captured, if that home
    /// is still honorable; this is the one place a leaf moves, so it is also the one place a
    /// home is spent.
    ///
    /// The caller resolves `panes` under the WORKSPACE lock and calls this under the
    /// registry lock, so the two are never nested (see [`crate::layout`]).
    pub fn reconcile_layout(&mut self, panes: &[PaneId]) -> &LayoutTree {
        let live: HashSet<PaneId> = panes.iter().copied().collect();
        self.bump_if_changed(|window| {
            // Prune INSIDE the compare: a floating pane that exits changes what a client
            // must draw (one fewer window) while leaving the tiling untouched, so pruning
            // outside would drop that change on the floor.
            window.floating.retain(|pane| live.contains(pane));
            // A pane that exits takes its home with it — nothing will ever come back to it,
            // and its id must not sit here waiting to collide with a future one.
            window.homes.retain(|pane, _| live.contains(pane));
            let tiled: Vec<PaneId> = panes
                .iter()
                .copied()
                .filter(|pane| !window.floating.contains(pane))
                .collect();
            window.layout.reconcile_homing(&tiled, &mut window.homes);
        });
        &self.layout
    }

    /// Install a client's settled arrangement, but only if it was authored against the
    /// arrangement still in force — a compare-and-set on
    /// [`layout_revision`](Self::layout_revision).
    ///
    /// `expected` is the revision the client last read. A gesture is a statement about a
    /// SPECIFIC arrangement ("put this divider here, in the layout I am looking at"), so
    /// applying it to a different one is not what the user asked for. Two attached clients
    /// are the whole point of a durable session, and without this the later write silently
    /// reverts the earlier one with neither client told. Refusing instead makes the loser
    /// re-read and re-project, which is the outcome it would have reached anyway had it
    /// seen the truth first.
    ///
    /// `None` writes unconditionally — for a caller with no prior read to be stale against.
    ///
    /// # Errors
    ///
    /// [`LayoutError::Stale`] if `expected` is not the current revision, or another
    /// [`LayoutError`] if the arrangement is not well-formed. Either way the window keeps the
    /// one it had, unchanged and un-bumped.
    pub fn set_layout(
        &mut self,
        wire: LayoutWire,
        expected: Option<u64>,
    ) -> Result<(), LayoutError> {
        if let Some(expected) = expected
            && expected != self.layout_revision
        {
            return Err(LayoutError::Stale {
                expected,
                actual: self.layout_revision,
            });
        }
        let mut next = self.layout.clone();
        next.set_from_wire(wire)?;
        self.bump_if_changed(|window| window.layout = next);
        Ok(())
    }

    /// Take `pane` out of the tiling (`floating == true`) or put it back.
    ///
    /// The tree is not touched here: the leaf appears or collapses on the next
    /// [`reconcile_layout`](Self::reconcile_layout), which every read goes through — so there
    /// is ONE place a leaf moves, rather than a second removal path to keep in step with it.
    /// A float therefore moves the revision TWICE (once here, once when the tiling follows);
    /// they are two real changes to what a client must draw, and the revision is opaque, so
    /// the cost is one extra re-read rather than a correctness question. Going through the
    /// one revision-bumping seam is what makes that structural: a caller
    /// cannot leave this window claiming a revision that predates its own float set.
    ///
    /// **Floating CAPTURES the pane's place** ([`FloatHome`]) before the leaf collapses, so
    /// docking it back returns it there rather than to the end: float the middle of `0|1|2`,
    /// detach, reattach, dock back, and it is `0|1|2` again, at the share the user dragged.
    /// The home is read here because here is the last moment it exists — once the tiling
    /// reflows over the gap, nothing can reconstruct where the pane sat. It is honored on the
    /// next [`reconcile_layout`](Self::reconcile_layout), which is where the leaf reappears.
    ///
    /// A home is a memo, not a promise: if its sibling has since exited or been floated out
    /// too, the pane docks back at the END (the old behaviour) rather than failing. A client
    /// that wants it somewhere specific still drops it there and writes the tree
    /// ([`set_layout`](Self::set_layout)) — a gesture outranks a memo.
    ///
    /// A no-op if `pane` is already in that state.
    pub fn set_floating(&mut self, pane: PaneId, floating: bool, panes: &[PaneId]) -> bool {
        if floating && self.would_untile_the_last(pane, panes) {
            return false;
        }
        self.bump_if_changed(|window| {
            if floating {
                // Capture BEFORE the float set collapses the leaf. `None` is the honest
                // answer for a pane holding no leaf to remember (never yet reconciled), and
                // for the sole tiled pane — which has no sibling to come home to, and which
                // `would_untile_the_last` refuses to float anyway.
                if let Some(home) = window.layout.leaf_home(pane) {
                    window.homes.insert(pane, home);
                }
                window.floating.insert(pane);
            } else {
                window.floating.remove(&pane);
            }
        });
        true
    }

    /// Whether floating `pane` would leave the window tiling NOTHING — the invariant a
    /// terminal multiplexer keeps: a window always shows at least one terminal.
    ///
    /// It lives HERE because the fact it guards lives here. Float became session state, and
    /// [`set_floating`](Self::set_floating) is reachable over a public wire action — from a
    /// second client, an AI peer, or a plugin — so a client-side check guards only the client
    /// that happens to make it, and the authority would accept from anyone else the one state
    /// it is supposed to forbid. An invariant enforced anywhere but at its authority is a
    /// convention, not an invariant.
    ///
    /// A pane the window does not hold cannot untile anything, so it is never refused here
    /// (it is pruned instead). A CLOSE is a different event class and is not subject to this:
    /// a gone pane may legitimately empty the tiling, and forcing a deliberately-floated pane
    /// back would be more surprising than an empty window.
    fn would_untile_the_last(&self, pane: PaneId, panes: &[PaneId]) -> bool {
        panes.contains(&pane)
            && panes
                .iter()
                .all(|p| *p == pane || self.floating.contains(p))
    }

    /// Apply `change` and bump [`layout_revision`](Self::layout_revision) only if the
    /// arrangement actually differed — the ONE place the revision moves, so "the number
    /// changed" and "a client's projection is stale" cannot come apart.
    ///
    /// Compares the tree AND the float set, because both are state a client projects: a pane
    /// that stops floating changes what the client must draw even on the rare path where the
    /// tiling comes out identical. It does NOT compare `homes` — a home is not served and not
    /// projected, so capturing one changes nothing a client could re-read. Bumping on it would
    /// wake every client to fetch an arrangement identical to the one it holds.
    fn bump_if_changed(&mut self, change: impl FnOnce(&mut Self)) {
        let tree = self.layout.clone();
        let floating = self.floating.clone();
        change(self);
        if self.layout != tree || self.floating != floating {
            self.layout_revision += 1;
        }
    }
}

/// One session: a named attach unit owning an ordered, non-empty set of [`Window`]s
/// with exactly one current window.
///
/// A client attaches to a session and views its current window. Increment A boots with
/// a single window; multi-window (new/select/rename) is a later increment, additive on
/// this shape.
pub struct Session {
    name: String,
    windows: Vec<Window>,
    current_window: usize,
}

impl Session {
    /// The session's display name (default `"0"`; the tmux `-s` name later).
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// All windows, in creation order.
    #[must_use]
    pub fn windows(&self) -> &[Window] {
        &self.windows
    }

    /// The current window (the one an attached client views). Never panics:
    /// `current_window` is maintained `< windows.len()` and `windows` is never empty.
    #[must_use]
    pub fn current_window(&self) -> &Window {
        &self.windows[self.current_window]
    }
}

/// The durable server's whole state: every [`Session`] and which one is current.
///
/// The default pane size is NOT held here — each window's [`Workspace`] owns it, and that
/// is the only copy production reads, so there is nothing to drift.
///
/// The SINGLE global [`PaneId`] counter is not held here separately — it
/// lives with the thing it counts, shared (`Arc`) by every window's [`Workspace`] and
/// seeded once at [`new`](Self::new). A future new-window/new-session path clones it
/// out of an existing window's workspace, so there is no duplicated handle to keep in
/// sync.
///
/// The host owns this behind an `Arc<Mutex<SessionRegistry>>` and resolves the current
/// window's workspace out of it per request (so window/session switching, added later,
/// needs no re-plumbing of the scene or externals).
pub struct SessionRegistry {
    sessions: Vec<Session>,
    current_session: usize,
}

impl SessionRegistry {
    /// A registry with one empty session (`"0"`) holding one empty window (`"0"`) — the
    /// behaviour-preserving boot state that mirrors the single [`Workspace`] the host
    /// owned before this layer existed. The boot window's workspace is seeded with a
    /// fresh global id counter (which later windows will share).
    #[must_use]
    pub fn new(default_size: (u16, u16)) -> Self {
        let id_counter = Arc::new(AtomicU64::new(0));
        let window = Window {
            name: "0".to_owned(),
            workspace: Arc::new(Mutex::new(Workspace::with_id_source(
                default_size,
                id_counter,
            ))),
            layout: LayoutTree::new(),
            floating: HashSet::new(),
            homes: HashMap::new(),
            layout_revision: 0,
        };
        let session = Session {
            name: "0".to_owned(),
            windows: vec![window],
            current_window: 0,
        };
        Self {
            sessions: vec![session],
            current_session: 0,
        }
    }

    /// All sessions, in creation order.
    #[must_use]
    pub fn sessions(&self) -> &[Session] {
        &self.sessions
    }

    /// The current session. Never panics: `current_session` is kept `< sessions.len()`
    /// and `sessions` is never empty.
    #[must_use]
    pub fn current_session(&self) -> &Session {
        &self.sessions[self.current_session]
    }

    /// The current window (the current session's current window).
    #[must_use]
    pub fn current_window(&self) -> &Window {
        self.current_session().current_window()
    }

    /// The current window, mutably — the seam a caller reconciles the arrangement
    /// through ([`Window::reconcile_layout`]).
    pub fn current_window_mut(&mut self) -> &mut Window {
        let session = &mut self.sessions[self.current_session];
        &mut session.windows[session.current_window]
    }

    /// A clone of the current window's pane-pool handle — the `Arc<Mutex<Workspace>>`
    /// the host hands to the per-request scene assembly and the control / plugin
    /// externals. Cloned (not borrowed) so the registry lock is released before the
    /// workspace lock is taken; because the scene + externals are rebuilt per request
    /// from this call, a later current-window switch is reflected on the next request
    /// with no re-plumbing.
    #[must_use]
    pub fn current_workspace(&self) -> Arc<Mutex<Workspace>> {
        Arc::clone(self.current_window().workspace())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CommandBuilder, LayoutNode, Pane, SplitDir};

    /// A long-lived `cat` child so a spawned pane's PTY stays open across assertions.
    fn cmd() -> CommandBuilder {
        let mut c = CommandBuilder::new("/bin/sh");
        c.arg("-c");
        c.arg("cat");
        c.env("TERM", "dumb");
        c
    }

    fn lock(ws: &Mutex<Workspace>) -> std::sync::MutexGuard<'_, Workspace> {
        ws.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[test]
    fn boots_one_session_one_window_matching_a_standalone_workspace() {
        // Behaviour-preserving boot: exactly one session, one window, an empty pool that
        // mints ids from 0 — the single Workspace the host owned before this layer.
        let reg = SessionRegistry::new((80, 24));
        assert_eq!(reg.sessions().len(), 1);
        assert_eq!(reg.current_session().name(), "0");
        assert_eq!(reg.current_session().windows().len(), 1);
        assert_eq!(reg.current_window().name(), "0");

        let ws = reg.current_workspace();
        let a = lock(&ws).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap();
        let b = lock(&ws).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap();
        assert_eq!((a.0, b.0), (0, 1), "the current window mints from 0");
        assert_eq!(lock(&ws).panes().len(), 2);
    }

    #[test]
    fn a_windows_layout_reconciles_against_its_real_workspace_panes() {
        // The Window seam: pane lifecycle runs through the Workspace directly (a plugin
        // spawns/reaps without ever seeing a Window), so the arrangement must self-heal
        // against the pool rather than be co-mutated. Driven here through a REAL
        // workspace, not a synthetic id list.
        let mut reg = SessionRegistry::new((80, 24));
        let ws = reg.current_workspace();
        let a = lock(&ws).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap();
        let b = lock(&ws).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap();

        // Resolve the pane ids under the WORKSPACE lock, then reconcile under the
        // registry lock — the two are never nested.
        let panes: Vec<_> = lock(&ws).panes().iter().map(Pane::id).collect();
        let window = &mut reg.sessions[0].windows[0];
        assert_eq!(window.reconcile_layout(&panes).panes(), vec![a, b]);

        // A pane reaped straight off the pool: its leaf collapses into its sibling.
        let removed = lock(&ws).close(a);
        assert!(removed.is_some());
        let panes: Vec<_> = lock(&ws).panes().iter().map(Pane::id).collect();
        let window = &mut reg.sessions[0].windows[0];
        assert_eq!(window.reconcile_layout(&panes).panes(), vec![b]);
        assert_eq!(window.layout().root(), Some(&crate::LayoutNode::Leaf(b)));
    }

    /// Float is session state, so taking a pane out of the tiling collapses its leaf
    /// host-side — the client renders an exact projection and needs no filter of its own —
    /// and docking it back returns it to the place the float captured, not to the end.
    #[test]
    fn a_floated_pane_loses_its_leaf_and_docks_back_at_its_home() {
        let mut reg = SessionRegistry::new((80, 24));
        let ws = reg.current_workspace();
        let ids: Vec<_> = (0..3)
            .map(|_| lock(&ws).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap())
            .collect();
        let panes: Vec<_> = lock(&ws).panes().iter().map(Pane::id).collect();

        let window = reg.current_window_mut();
        assert_eq!(window.reconcile_layout(&panes).panes(), ids);

        // Float the MIDDLE pane: its leaf collapses, the siblings reclaim the space.
        let _ = window.set_floating(ids[1], true, &panes);
        assert_eq!(
            window.reconcile_layout(&panes).panes(),
            vec![ids[0], ids[2]],
            "a floated pane holds no leaf",
        );
        assert_eq!(window.floating(), &HashSet::from([ids[1]]));

        // Dock it back with no gesture to say where: it goes HOME — the middle it left,
        // beside the neighbour it left it beside.
        let _ = window.set_floating(ids[1], false, &panes);
        assert_eq!(
            window.reconcile_layout(&panes).panes(),
            ids,
            "the pane's place in the arrangement survived the float",
        );
    }

    /// The home is a memo the authority keeps, not a promise it can always keep: float a pane
    /// AND its home sibling, and the first one back has nothing to come home to. It appends —
    /// the old behaviour — rather than failing the dock-back.
    ///
    /// This is the case [`crate::LayoutTree`] cannot tell apart from an exited sibling (it
    /// sees only "absent from the tiling"), so it is pinned here, where `floating` is a fact.
    #[test]
    fn a_home_whose_sibling_floated_out_too_docks_back_at_the_end() {
        let mut reg = SessionRegistry::new((80, 24));
        let ws = reg.current_workspace();
        let ids: Vec<_> = (0..3)
            .map(|_| lock(&ws).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap())
            .collect();
        let panes: Vec<_> = lock(&ws).panes().iter().map(Pane::id).collect();
        let window = reg.current_window_mut();
        window.reconcile_layout(&panes);

        // Pane 1's home names pane 2; float both, so 1's home is unhonorable while 2 is out.
        assert!(window.set_floating(ids[1], true, &panes));
        window.reconcile_layout(&panes);
        assert!(window.set_floating(ids[2], true, &panes));
        assert_eq!(
            window.reconcile_layout(&panes).panes(),
            vec![ids[0]],
            "both floated out; only pane 0 is tiled",
        );

        // Pane 1 comes back ALONE: its sibling is alive but not tiled.
        assert!(window.set_floating(ids[1], false, &panes));
        assert_eq!(
            window.reconcile_layout(&panes).panes(),
            vec![ids[0], ids[1]],
            "no home to honor, so it appends",
        );
        // …and pane 2, still floating, keeps its own home for when it returns.
        assert!(window.set_floating(ids[2], false, &panes));
        assert_eq!(
            window.reconcile_layout(&panes).panes(),
            vec![ids[0], ids[2], ids[1]],
            "pane 2 went home (beside 0), which is now ahead of the appended pane 1",
        );
    }

    /// A home is captured at the float and spent when the leaf comes back — so a pane that
    /// EXITS while floating must not leave one behind for its id to outlive it.
    #[test]
    fn a_home_is_pruned_when_its_pane_exits_while_floating() {
        let mut reg = SessionRegistry::new((80, 24));
        let ws = reg.current_workspace();
        let ids: Vec<_> = (0..3)
            .map(|_| lock(&ws).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap())
            .collect();
        let panes: Vec<_> = lock(&ws).panes().iter().map(Pane::id).collect();
        let window = reg.current_window_mut();
        window.reconcile_layout(&panes);

        assert!(window.set_floating(ids[1], true, &panes));
        window.reconcile_layout(&panes);
        assert!(window.homes.contains_key(&ids[1]), "the float captured one");

        // Pane 1 exits while floating: the pool no longer holds it.
        let live = vec![ids[0], ids[2]];
        window.reconcile_layout(&live);
        assert!(window.floating().is_empty(), "the float set is pruned");
        assert!(
            window.homes.is_empty(),
            "and so is its home — nothing will ever come back to it",
        );
    }

    /// Capturing a home is invisible to a client: it is not served and not projected, so it
    /// must not move the revision every attached client watches.
    #[test]
    fn capturing_a_home_does_not_bump_the_revision_on_its_own() {
        let mut reg = SessionRegistry::new((80, 24));
        let ws = reg.current_workspace();
        let ids: Vec<_> = (0..2)
            .map(|_| lock(&ws).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap())
            .collect();
        let panes: Vec<_> = lock(&ws).panes().iter().map(Pane::id).collect();
        let window = reg.current_window_mut();
        window.reconcile_layout(&panes);
        let before = window.layout_revision();

        // A float bumps ONCE for the float set (the tiling follows on the next reconcile);
        // the home captured alongside it adds nothing a client could re-read.
        assert!(window.set_floating(ids[0], true, &panes));
        assert_eq!(
            window.layout_revision(),
            before + 1,
            "the float set changed once; the home is not a client-visible change",
        );
    }

    /// A gesture authored against an arrangement that has moved on is REFUSED — a durable
    /// session's whole point is more than one client, and silent last-write-wins would let
    /// one revert the other with neither told.
    #[test]
    fn a_write_against_a_stale_arrangement_is_refused() {
        let mut reg = SessionRegistry::new((80, 24));
        let ws = reg.current_workspace();
        let a = lock(&ws).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap();
        let b = lock(&ws).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap();
        let window = reg.current_window_mut();
        window.reconcile_layout(&[a, b]);
        let read_by_both = window.layout_revision();

        // Client A's gesture lands first.
        let vertical = |ratio: f32| LayoutWire {
            root: Some(crate::LayoutNodeWire::Split {
                id: None,
                dir: SplitDir::Vertical,
                ratio,
                first: Box::new(crate::LayoutNodeWire::Leaf(a)),
                second: Box::new(crate::LayoutNodeWire::Leaf(b)),
            }),
        };
        window
            .set_layout(vertical(0.7), Some(read_by_both))
            .expect("A wrote against what it read");
        let after_a = window.layout_revision();
        assert!(after_a > read_by_both);

        // Client B settled its gesture against the SAME revision it read, before A's landed.
        assert_eq!(
            window.set_layout(vertical(0.2), Some(read_by_both)),
            Err(LayoutError::Stale {
                expected: read_by_both,
                actual: after_a,
            }),
            "B's gesture is about a layout that no longer exists",
        );
        let LayoutNode::Split { ratio, .. } = window.layout().root().unwrap() else {
            panic!("a split");
        };
        assert!(
            (*ratio - 0.7).abs() < f32::EPSILON,
            "A's arrangement stands; B did not silently revert it",
        );
        assert_eq!(
            window.layout_revision(),
            after_a,
            "a refused write is inert"
        );

        // B re-reads and re-writes against the truth: now it wins.
        window
            .set_layout(vertical(0.2), Some(window.layout_revision()))
            .expect("a gesture against the live arrangement applies");
    }

    /// The window keeps a terminal: floating the LAST tiled pane is REFUSED — by the HOST,
    /// because the host is what owns float now.
    ///
    /// The client has its own guard, but `set_floating` is a public wire action reachable by
    /// a second client, an AI peer, or a plugin. A guard that lives only in one client is a
    /// convention; this is the invariant.
    #[test]
    fn floating_the_last_tiled_pane_is_refused_by_the_authority() {
        let mut reg = SessionRegistry::new((80, 24));
        let ws = reg.current_workspace();
        let a = lock(&ws).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap();
        let b = lock(&ws).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap();
        let panes = [a, b];
        let window = reg.current_window_mut();
        window.reconcile_layout(&panes);

        assert!(window.set_floating(a, true, &panes), "one of two may float");
        assert_eq!(window.reconcile_layout(&panes).panes(), vec![b]);

        let settled = window.layout_revision();
        assert!(
            !window.set_floating(b, true, &panes),
            "the LAST tiled pane may not float, however politely asked",
        );
        assert_eq!(window.floating(), &HashSet::from([a]), "b never floated");
        assert_eq!(
            window.layout_revision(),
            settled,
            "a refused float is inert — it must not even move the revision",
        );
        assert_eq!(
            window.reconcile_layout(&panes).panes(),
            vec![b],
            "b still tiles"
        );

        // A pane the window does not hold cannot untile anything: never refused, just pruned.
        assert!(window.set_floating(PaneId(999), true, &panes));
        window.reconcile_layout(&panes);
        assert_eq!(
            window.floating(),
            &HashSet::from([a]),
            "the ghost was pruned"
        );
    }

    /// A floating pane that EXITS must leave no entry behind, or the set would slowly
    /// become an authority over membership instead of a view of it — and worse, a reused
    /// id could be born floating.
    #[test]
    fn a_floating_pane_that_exits_is_pruned_from_the_set() {
        let mut reg = SessionRegistry::new((80, 24));
        let ws = reg.current_workspace();
        let a = lock(&ws).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap();
        let b = lock(&ws).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap();

        let window = reg.current_window_mut();
        let _ = window.set_floating(b, true, &[a, b]);
        window.reconcile_layout(&[a, b]);
        assert_eq!(window.floating(), &HashSet::from([b]));

        assert!(lock(&ws).close(b).is_some());
        let panes: Vec<_> = lock(&ws).panes().iter().map(Pane::id).collect();
        let window = reg.current_window_mut();
        window.reconcile_layout(&panes);
        assert!(window.floating().is_empty(), "the exited pane was pruned");
    }

    /// The revision is the client's staleness signal, so it must move on every real change
    /// and on nothing else — a spurious bump re-projects on top of a live gesture, a missed
    /// one leaves the client rendering a layout the session no longer has.
    #[test]
    fn the_revision_moves_on_a_real_change_and_only_then() {
        let mut reg = SessionRegistry::new((80, 24));
        let ws = reg.current_workspace();
        let a = lock(&ws).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap();
        let b = lock(&ws).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap();
        let window = reg.current_window_mut();

        assert_eq!(
            window.layout_revision(),
            0,
            "an untouched window is at zero"
        );
        window.reconcile_layout(&[a, b]);
        let arranged = window.layout_revision();
        assert!(arranged > 0, "arranging the boot panes is a change");

        // Reading / reconciling an unchanged set is not.
        window.reconcile_layout(&[a, b]);
        window.reconcile_layout(&[a, b]);
        assert_eq!(window.layout_revision(), arranged, "a read never bumps");

        // A write that installs the SAME arrangement is not a change either.
        let same = LayoutWire::from(window.layout());
        window.set_layout(same, None).expect("valid");
        assert_eq!(
            window.layout_revision(),
            arranged,
            "an identical write does not bump",
        );

        // A write that moves the divider IS.
        let LayoutNode::Split { id, dir, .. } = window.layout().root().unwrap() else {
            panic!("two panes root at a split");
        };
        window
            .set_layout(
                LayoutWire {
                    root: Some(crate::LayoutNodeWire::Split {
                        id: Some(*id),
                        dir: *dir,
                        ratio: 0.8,
                        first: Box::new(crate::LayoutNodeWire::Leaf(a)),
                        second: Box::new(crate::LayoutNodeWire::Leaf(b)),
                    }),
                },
                None,
            )
            .expect("valid");
        assert_eq!(window.layout_revision(), arranged + 1, "a drag bumps once");

        // A REJECTED write changes nothing, so it must not bump.
        assert!(
            window
                .set_layout(
                    LayoutWire {
                        root: Some(crate::LayoutNodeWire::Leaf(a)),
                    },
                    None
                )
                .is_ok(),
        );
        let dropped = window.layout_revision();
        assert!(
            window
                .set_layout(
                    LayoutWire {
                        root: Some(crate::LayoutNodeWire::Split {
                            id: None,
                            dir: SplitDir::Horizontal,
                            ratio: f32::NAN,
                            first: Box::new(crate::LayoutNodeWire::Leaf(a)),
                            second: Box::new(crate::LayoutNodeWire::Leaf(b)),
                        }),
                    },
                    None
                )
                .is_err(),
        );
        assert_eq!(
            window.layout_revision(),
            dropped,
            "a rejected write is inert"
        );

        // That write dropped b's leaf, so reconciling re-arranges it (a change) — and only
        // then is floating it one.
        window.reconcile_layout(&[a, b]);
        let rearranged = window.layout_revision();
        assert!(rearranged > dropped, "an unarranged pane gets placed");

        // Floating lands on the next reconcile — the one place a leaf collapses.
        let _ = window.set_floating(b, true, &[a, b]);
        let floated = window.layout_revision();
        assert_eq!(
            floated,
            rearranged + 1,
            "taking a pane out of the tiling is itself a change a client must see",
        );
        window.reconcile_layout(&[a, b]);
        assert_eq!(
            window.layout_revision(),
            floated + 1,
            "and the tiling following it is a second, real one (the leaf collapsed)",
        );
        window.reconcile_layout(&[a, b]);
        assert_eq!(
            window.layout_revision(),
            floated + 1,
            "but reconciling a settled float again changes nothing",
        );
    }

    #[test]
    fn a_shared_counter_makes_ids_globally_unique_across_windows() {
        // The load-bearing invariant: two windows drawing from ONE registry counter never
        // collide, so a pane is addressable by id alone regardless of which window holds
        // it. (Windows are constructed directly here — the registry's own new-window API
        // is a later increment; this proves the counter-sharing the registry relies on.)
        let counter = Arc::new(AtomicU64::new(0));
        let win_a = Arc::new(Mutex::new(Workspace::with_id_source(
            (80, 24),
            Arc::clone(&counter),
        )));
        let win_b = Arc::new(Mutex::new(Workspace::with_id_source(
            (80, 24),
            Arc::clone(&counter),
        )));

        let a0 = lock(&win_a).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap();
        let b0 = lock(&win_b).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap();
        let a1 = lock(&win_a).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap();

        // Interleaved spawns across two windows still yield distinct, monotonic ids.
        let mut ids = [a0.0, b0.0, a1.0];
        ids.sort_unstable();
        assert_eq!(ids, [0, 1, 2], "ids are globally unique across windows");
    }
}
