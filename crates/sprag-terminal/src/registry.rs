//! The session / window hierarchy — the durable server's client-independent state.
//!
//! tmux's core value is that terminal state outlives any client: detach, the session
//! keeps running, reattach and your windows + panes are exactly as you left them. That
//! demands the state live in an authority no client can take down. sprag's PTYs already
//! live host-side; this module adds the tree ABOVE the pane pool that makes the rest of
//! the detach/reattach arc (and windows/tabs) possible:
//!
//! ```text
//! SessionRegistry            -- every session; one of them is the default scope
//!   Session (named)          -- the attach unit: an ordered set of windows + a current one
//!     Window (named)         -- the layout unit: a pane pool + its LayoutTree
//!       Workspace            -- the pane pool (crate::workspace); OWNS the shared id counter
//!         Pane (PTY + emulator)
//! ```
//!
//! A session is addressed by NAME, from outside this module and over the wire alike. The
//! registry keeps no "current session" pointer: which session a request acts on is the
//! request's own business (an out-of-band scope param), and the only unnamed scope is the
//! immutable [`SessionRegistry::default_session`]. See its type docs for why a server-side
//! selector would be the wrong shape.
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
//! ([`Workspace::sibling`]), so a [`PaneId`] is unique across the
//! WHOLE registry, monotonic, and never reused. That is what lets a pane be addressed
//! by id alone regardless of which window/session holds it — the per-pane wire path
//! stays window-free, and adding windows later needs no address migration.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};

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
    /// An empty window named `name` over `pool` — which the caller obtains from
    /// [`Workspace::sibling`], so every window in the registry mints from ONE id counter
    /// (the load-bearing invariant; see the module docs).
    fn new(name: &str, pool: Workspace) -> Self {
        Self {
            name: name.to_owned(),
            workspace: Arc::new(Mutex::new(pool)),
            layout: LayoutTree::new(),
            floating: HashSet::new(),
            homes: HashMap::new(),
            layout_revision: 0,
        }
    }

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
            // A pane that exits takes its home with it: nothing will ever come back to it, so
            // the entry is dead weight that would accumulate for the window's life. (NOT to
            // avoid an id collision — ids are minted from one registry-wide counter and are
            // never reused, so a stale home cannot be mistaken for a future pane's.)
            window.homes.retain(|pane, _| live.contains(pane));
            let tiled: Vec<PaneId> = panes
                .iter()
                .copied()
                .filter(|pane| !window.floating.contains(pane))
                .collect();
            window.layout.reconcile(&tiled, &mut window.homes);
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

/// Why a session operation was refused. The registry is unchanged in either case.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SessionError {
    /// The name is already taken ([`SessionRegistry::new_session`]).
    Duplicate(String),
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Duplicate(name) => write!(f, "a session named {name:?} already exists"),
        }
    }
}

impl std::error::Error for SessionError {}

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
    /// A session named `name` holding one empty window `"0"` — a session always has at
    /// least one window, which is what makes [`current_window`](Self::current_window)
    /// total.
    fn new(name: &str, pool: Workspace) -> Self {
        Self {
            name: name.to_owned(),
            windows: vec![Window::new("0", pool)],
            current_window: 0,
        }
    }

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

/// The durable server's whole state: every [`Session`].
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
/// The host owns this behind an `Arc<Mutex<SessionRegistry>>` and resolves the session a
/// request is SCOPED to out of it per request, by NAME
/// ([`session`](Self::session) / [`window_mut`](Self::window_mut)).
///
/// ## Why there is no "current session" pointer
///
/// There used to be one, moved by a `select_session`, and it was a single-client-era
/// artifact. tmux's server has no such thing: each CLIENT is attached to a session, and
/// `switch-client` changes THAT client's attachment, not a server-wide global. Under an
/// out-of-band `session` scope param a client says which session each request is about, so
/// switching is purely a client-side change — it sends a different name. A server-side
/// mutator's only remaining job would be to move the default OUT FROM UNDER every other
/// attached client, which is the hazard [`new_session`](Self::new_session) already refuses
/// to create. So the only scope that is not named by the caller is
/// [`default_session`](Self::default_session), and nothing can move it.
pub struct SessionRegistry {
    /// Never empty: [`new`](Self::new) seeds one, and no removal path exists — which is
    /// what makes [`default_session`](Self::default_session) total.
    sessions: Vec<Session>,
}

impl SessionRegistry {
    /// A registry with one empty session (`"0"`) holding one empty window (`"0"`) — the
    /// behaviour-preserving boot state that mirrors the single [`Workspace`] the host
    /// owned before this layer existed. The boot window's workspace is seeded with a
    /// fresh global id counter (which later windows will share).
    #[must_use]
    pub fn new(default_size: (u16, u16)) -> Self {
        Self {
            sessions: vec![Session::new("0", Workspace::new(default_size))],
        }
    }

    /// All sessions, in creation order.
    #[must_use]
    pub fn sessions(&self) -> &[Session] {
        &self.sessions
    }

    /// Resolve a session by NAME. `None` if no session carries it.
    ///
    /// Name, never index, is how a session is addressed from outside this type: an index
    /// supplied from outside is a number that means nothing until it is checked, and the
    /// checking is what every caller forgets. A name that does not resolve is `None` here
    /// and a refusal at the wire, rather than an out-of-range value some later, unrelated
    /// request panics on.
    #[must_use]
    pub fn session(&self, name: &str) -> Option<&Session> {
        self.sessions.iter().find(|s| s.name == name)
    }

    /// Create a session named `name`, holding one empty window.
    ///
    /// Its pane pool clones the id counter out of a pool that already exists, so ids stay
    /// unique across the WHOLE registry (the module's load-bearing invariant) with no second
    /// home to keep in step. Size is inherited from the default session's pool, which is the
    /// only copy production reads.
    ///
    /// Does NOT change any other client's scope: creating and attaching are separate acts,
    /// and a client that creates a session for someone else must not yank the scope out from
    /// under whoever is attached now. Nothing here can — the default is immutable, and every
    /// other client names its own scope.
    ///
    /// # Errors
    ///
    /// [`SessionError::Duplicate`] if `name` is already taken — a name is how a session is
    /// addressed, so two of them would make the address ambiguous and let one client's
    /// request silently land in another's session.
    /// Returns nothing rather than a borrow of the new session: creating is not attaching,
    /// so a caller that wants it looks it up by the name it just chose ([`session`](Self::session)).
    pub fn new_session(&mut self, name: &str) -> Result<(), SessionError> {
        if self.session(name).is_some() {
            return Err(SessionError::Duplicate(name.to_owned()));
        }
        let seed = Arc::clone(self.default_session().current_window().workspace());
        let pool = seed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .sibling();
        self.sessions.push(Session::new(name, pool));
        Ok(())
    }

    /// The session an UNSCOPED request acts on — the one the host booted with.
    ///
    /// Total, and immutably so: `sessions` is seeded non-empty and has no removal path, and
    /// nothing moves which session is the default (see the type docs — a client that wants
    /// another session NAMES it). So this is not a pointer that must be maintained; it is the
    /// first session, for the life of the registry.
    ///
    /// **Bound for the daemon increment:** "exit when the registry empties" would introduce
    /// the first way for `sessions` to shrink, and a kill of the boot session is exactly what
    /// this totality rests on. That increment must decide what an unscoped request means with
    /// the default gone — the honest answers are to re-establish a default explicitly or to
    /// make this fallible — rather than discovering it as a panic.
    #[must_use]
    pub fn default_session(&self) -> &Session {
        &self.sessions[0]
    }

    /// The window a request scoped to the session named `session` acts on, mutably — the seam
    /// a caller reconciles the arrangement through ([`Window::reconcile_layout`]). `None` if
    /// no session carries the name.
    ///
    /// The `Option` is what makes a vanished scope a REFUSAL at the caller rather than a
    /// panic here: a scope is validated when a request arrives, but the authority for "does
    /// this session exist" is this type, and asking it again at the moment of use is what
    /// keeps the two from drifting once a removal path exists.
    pub fn window_mut(&mut self, session: &str) -> Option<&mut Window> {
        let session = self.sessions.iter_mut().find(|s| s.name == session)?;
        Some(&mut session.windows[session.current_window])
    }

    /// A clone of the pane-pool handle of the window a request scoped to `session` acts on —
    /// the `Arc<Mutex<Workspace>>` the host hands to the per-request scene assembly and the
    /// control / plugin externals. `None` if no session carries the name.
    ///
    /// Cloned (not borrowed) so the registry lock is released before the workspace lock is
    /// taken; because the scene + externals are rebuilt per request from this call, a window
    /// switch is reflected on the next request with no re-plumbing.
    #[must_use]
    pub fn workspace_of(&self, session: &str) -> Option<Arc<Mutex<Workspace>>> {
        self.session(session)
            .map(|s| Arc::clone(s.current_window().workspace()))
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

    /// The DEFAULT session's name — the scope an unscoped request resolves to. Tests address
    /// through this rather than hardcoding `"0"`, so what the host boots with stays the
    /// registry's business and only [`boots_one_session_one_window_matching_a_standalone_workspace`]
    /// pins the literal.
    fn default_name(reg: &SessionRegistry) -> String {
        reg.default_session().name().to_owned()
    }

    /// The default session's pane pool — what an unscoped request acts on.
    fn pool(reg: &SessionRegistry) -> Arc<Mutex<Workspace>> {
        reg.workspace_of(&default_name(reg))
            .expect("the default session always resolves")
    }

    /// The default session's window, mutably.
    fn default_window(reg: &mut SessionRegistry) -> &mut Window {
        let name = default_name(reg);
        reg.window_mut(&name)
            .expect("the default session always resolves")
    }

    #[test]
    fn boots_one_session_one_window_matching_a_standalone_workspace() {
        // Behaviour-preserving boot: exactly one session, one window, an empty pool that
        // mints ids from 0 — the single Workspace the host owned before this layer.
        let reg = SessionRegistry::new((80, 24));
        assert_eq!(reg.sessions().len(), 1);
        assert_eq!(reg.default_session().name(), "0");
        assert_eq!(reg.default_session().windows().len(), 1);
        assert_eq!(reg.default_session().current_window().name(), "0");

        let ws = pool(&reg);
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
        let ws = pool(&reg);
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
        let ws = pool(&reg);
        let ids: Vec<_> = (0..3)
            .map(|_| lock(&ws).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap())
            .collect();
        let panes: Vec<_> = lock(&ws).panes().iter().map(Pane::id).collect();

        let window = default_window(&mut reg);
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
        let ws = pool(&reg);
        let ids: Vec<_> = (0..3)
            .map(|_| lock(&ws).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap())
            .collect();
        let panes: Vec<_> = lock(&ws).panes().iter().map(Pane::id).collect();
        let window = default_window(&mut reg);
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
        let ws = pool(&reg);
        let ids: Vec<_> = (0..3)
            .map(|_| lock(&ws).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap())
            .collect();
        let panes: Vec<_> = lock(&ws).panes().iter().map(Pane::id).collect();
        let window = default_window(&mut reg);
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

    /// A pane that is TILED holds no home, whatever route it took to get there.
    ///
    /// Float then un-float with no reconcile between: the leaf never collapsed, so the pane
    /// is still tiled AND holds a home. Nothing places it (it is already arranged), so a
    /// spend-on-placement rule would leave that memo forever, to hijack some later
    /// re-placement. `sprag-host` cannot reach this today — it reconciles after every float —
    /// but that is the caller being well-behaved, and this type's doc promises the invariant
    /// itself. R154's scar was exactly an invariant that held only by an accident of caller
    /// ordering.
    #[test]
    fn a_tiled_pane_holds_no_home_even_if_it_never_left_the_tiling() {
        let mut reg = SessionRegistry::new((80, 24));
        let ws = pool(&reg);
        let ids: Vec<_> = (0..3)
            .map(|_| lock(&ws).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap())
            .collect();
        let panes: Vec<_> = lock(&ws).panes().iter().map(Pane::id).collect();
        let window = default_window(&mut reg);
        window.reconcile_layout(&panes);

        // Float and un-float with NO reconcile between — the leaf never collapses.
        assert!(window.set_floating(ids[1], true, &panes));
        assert!(window.set_floating(ids[1], false, &panes));
        window.reconcile_layout(&panes);

        assert_eq!(
            window.layout().panes(),
            ids,
            "the pane never left the tiling"
        );
        assert!(
            window.homes.is_empty(),
            "a tiled pane's home is spent; a stale memo could only fight its real position",
        );
    }

    /// Capturing a home is invisible to a client: it is not served and not projected, so it
    /// must not move the revision every attached client watches.
    #[test]
    fn a_homes_only_change_does_not_bump_the_revision() {
        let mut reg = SessionRegistry::new((80, 24));
        let ws = pool(&reg);
        let ids: Vec<_> = (0..3)
            .map(|_| lock(&ws).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap())
            .collect();
        let panes: Vec<_> = lock(&ws).panes().iter().map(Pane::id).collect();
        let window = default_window(&mut reg);
        window.reconcile_layout(&panes);

        // Float and un-float. The float set is back where it started and the tiling never
        // moved, so the ONLY thing this pair of calls changed is `homes` — it captured one
        // and the reconcile spent it. That is the isolation the old version of this test
        // never achieved: it stimulated a FLOAT, which moves the float set, so it read
        // `before + 1` whether or not `homes` was compared. It could not fail.
        assert!(window.set_floating(ids[1], true, &panes));
        assert!(window.set_floating(ids[1], false, &panes));
        let settled = window.layout_revision();
        assert_eq!(window.layout().panes(), ids, "the tiling never moved");

        window.reconcile_layout(&panes);
        assert_eq!(
            window.layout_revision(),
            settled,
            "spending a home changes nothing a client can re-read, so it must not wake one",
        );
    }

    /// A second session is a real, independent attach unit — the shape the owner's
    /// several-windows workflow needs once ONE daemon holds every session.
    #[test]
    fn a_new_session_is_independent_and_is_not_attached_to_on_creation() {
        let mut reg = SessionRegistry::new((80, 24));
        assert_eq!(reg.sessions().len(), 1);

        reg.new_session("work").expect("a free name");
        let created = reg
            .session("work")
            .expect("looked up by the name just chosen");
        assert_eq!(created.name(), "work");
        assert_eq!(created.windows().len(), 1, "a session always has a window");
        assert!(created.current_window().layout().panes().is_empty());

        // Creating is not attaching: whoever is scoped to "0" keeps their scope.
        assert_eq!(
            reg.default_session().name(),
            "0",
            "creating a session for someone else must not move anyone else's scope",
        );
        assert_eq!(reg.sessions().len(), 2);
        assert_eq!(reg.session("work").map(Session::name), Some("work"));
        assert!(reg.session("nope").is_none());
    }

    /// A name is an ADDRESS, so two sessions sharing one would make a request ambiguous —
    /// it could silently land in the wrong client's session.
    #[test]
    fn a_duplicate_session_name_is_refused_and_changes_nothing() {
        let mut reg = SessionRegistry::new((80, 24));
        reg.new_session("work").unwrap();
        assert_eq!(
            reg.new_session("work").unwrap_err(),
            SessionError::Duplicate("work".to_owned()),
        );
        assert_eq!(reg.sessions().len(), 2, "the refused create added nothing");
    }

    /// THE STRUCTURAL CLAIM, and it is stronger than the `select_session` whose test this
    /// replaces: the registry stores NO index for a session, so an unknown name cannot leave
    /// one dangling for a later, unrelated request to panic on.
    ///
    /// The old test guarded that failure mode by proving `select_session` resolved a name
    /// before storing the index it derived. Retiring the selector REMOVED the failure mode
    /// instead: the only way to reach a session is to name it, a name that does not resolve
    /// is `None` at every site that resolves one, and the single unnamed scope is immutable.
    /// A guard is not needed for a state that is unrepresentable.
    #[test]
    fn an_unknown_session_name_resolves_to_nothing_and_moves_no_scope() {
        let mut reg = SessionRegistry::new((80, 24));
        reg.new_session("work").unwrap();

        // Absent at every resolution site — not an error to be handled, just nothing.
        assert!(reg.session("ghost").is_none());
        assert!(reg.workspace_of("ghost").is_none());
        assert!(reg.window_mut("ghost").is_none());

        // ...while a real name resolves at each of them. Without this half, the assertions
        // above would pass just as well against a registry that resolves NOTHING.
        assert_eq!(reg.session("work").map(Session::name), Some("work"));
        assert!(reg.workspace_of("work").is_some());
        assert!(reg.window_mut("work").is_some());

        // And nothing above moved the default: not creating a session, not naming one, not
        // naming a ghost. An unscoped request still lands where it did at boot.
        assert_eq!(reg.default_session().name(), "0");
        assert_eq!(reg.default_session().current_window().name(), "0");
    }

    /// Pane ids stay unique across SESSIONS, not just across windows: the new session's pool
    /// clones the id counter rather than starting its own, so no second home can drift.
    #[test]
    fn a_new_sessions_pool_shares_the_one_global_id_counter() {
        let mut reg = SessionRegistry::new((80, 24));
        let first = pool(&reg);
        let a = lock(&first).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap();

        reg.new_session("work").unwrap();
        let second = reg
            .workspace_of("work")
            .expect("the name just created resolves");
        let b = lock(&second).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap();

        assert_ne!(a, b, "a pane id is unique across the WHOLE registry");
        assert!(b > a, "and monotonic: {a} then {b}");
    }

    /// A gesture authored against an arrangement that has moved on is REFUSED — a durable
    /// session's whole point is more than one client, and silent last-write-wins would let
    /// one revert the other with neither told.
    #[test]
    fn a_write_against_a_stale_arrangement_is_refused() {
        let mut reg = SessionRegistry::new((80, 24));
        let ws = pool(&reg);
        let a = lock(&ws).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap();
        let b = lock(&ws).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap();
        let window = default_window(&mut reg);
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
        let ws = pool(&reg);
        let a = lock(&ws).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap();
        let b = lock(&ws).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap();
        let panes = [a, b];
        let window = default_window(&mut reg);
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
        let ws = pool(&reg);
        let a = lock(&ws).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap();
        let b = lock(&ws).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap();

        let window = default_window(&mut reg);
        let _ = window.set_floating(b, true, &[a, b]);
        window.reconcile_layout(&[a, b]);
        assert_eq!(window.floating(), &HashSet::from([b]));

        assert!(lock(&ws).close(b).is_some());
        let panes: Vec<_> = lock(&ws).panes().iter().map(Pane::id).collect();
        let window = default_window(&mut reg);
        window.reconcile_layout(&panes);
        assert!(window.floating().is_empty(), "the exited pane was pruned");
    }

    /// The revision is the client's staleness signal, so it must move on every real change
    /// and on nothing else — a spurious bump re-projects on top of a live gesture, a missed
    /// one leaves the client rendering a layout the session no longer has.
    #[test]
    fn the_revision_moves_on_a_real_change_and_only_then() {
        let mut reg = SessionRegistry::new((80, 24));
        let ws = pool(&reg);
        let a = lock(&ws).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap();
        let b = lock(&ws).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap();
        let window = default_window(&mut reg);

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
        // it. (Pools are constructed directly here — the registry's own new-window API
        // is a later increment; this proves the counter-sharing the registry relies on.)
        let win_a = Arc::new(Mutex::new(Workspace::new((80, 24))));
        let win_b = Arc::new(Mutex::new(lock(&win_a).sibling()));

        let a0 = lock(&win_a).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap();
        let b0 = lock(&win_b).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap();
        let a1 = lock(&win_a).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap();

        // Interleaved spawns across two windows still yield distinct, monotonic ids.
        let mut ids = [a0.0, b0.0, a1.0];
        ids.sort_unstable();
        assert_eq!(ids, [0, 1, 2], "ids are globally unique across windows");
    }
}
