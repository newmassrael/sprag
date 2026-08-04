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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use crate::PaneId;
use crate::layout::{
    LayoutError, LayoutTree, LayoutWire, LeafHome, PaneDir, PaneStep, SplitDir, SplitSide,
};
use crate::session_name::{SessionName, SessionNameError};
use crate::snapshot::WindowSnapshot;
use crate::snapshot::{
    MIN_READABLE_SNAPSHOT_VERSION, PaneRestore, RestorePlan, SNAPSHOT_VERSION, Snapshot,
    SnapshotError,
};
use crate::workspace::{HistoryLimitSource, Pane, PaneEnvSource, Workspace};

/// A session's IDENTITY — what stays put when its NAME moves.
///
/// # Why a session needs one when a pane has had one all along
///
/// A [`PaneId`] is public: it addresses a pane on the wire, and a pane's name is a second,
/// friendlier address on top of it. A SESSION has no such id — `-t` takes a name, tmux's grammar
/// takes a name, and this registry looks one up by name. That is deliberate and stays: the
/// **address is the name**.
///
/// What it cost is the ability to tell a RENAME from a death. A session's public shape is its
/// name, so "`work` is gone and `prod` is here" is what a close plus a create looks like too, and
/// the change funnel (`sprag_host::events`) had no way to prefer the honest reading — the same
/// sentence that crate's docs give for why there was no `WindowRenamed`. So this id exists for
/// exactly one consumer: the DERIVATION, which can now say *the session that was called `work` is
/// now called `prod`* rather than reporting a death and a birth a client acts on by tearing down
/// everything it held.
///
/// # It is a WITHIN-RUN identity and does not cross the wire
///
/// Minted from one registry-wide counter, never reused while the daemon lives, and NOT restored
/// from the durability snapshot — a rebooted daemon mints fresh ones, because the only holder an
/// id could have had is a client that did not survive the reboot either. Nothing serialises it.
/// The trade, stated rather than hidden: a client cannot hold an identity that outlives a rename;
/// it holds a name and is TOLD the moment that name moves.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct SessionId(pub u64);

/// A window's IDENTITY — [`SessionId`]'s twin one level down, for the same derivation and on the
/// same terms: unique registry-wide, minted from the one counter, never on the wire.
///
/// Registry-wide rather than per-session even though every comparison of one happens inside a
/// single session today, because the day a window can MOVE between sessions is the day a
/// per-session id starts colliding — and that failure would show up as a wrong EVENT, which is the
/// class this whole type exists to remove.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct WindowId(pub u64);

/// One step along a session's window RING — tmux `next-window` / `previous-window`, and
/// `select-window -n` / `-p`.
///
/// A direction rather than a target, and a SEPARATE vocabulary from [`PaneDir`] rather than a reuse
/// of it, because the two walks are different kinds of question: a pane walk is spatial (four ways,
/// an edge at each end), a window walk is ordinal (two ways, no ends). Spelling them with one type
/// would let a caller ask for the window "to the left", which the registry would then have to
/// refuse at runtime for a mistake the types can prevent.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WindowStep {
    /// The window AFTER the current one, wrapping past the last onto the first.
    Next,
    /// The window BEFORE it, wrapping past the first onto the last.
    Previous,
}

impl WindowStep {
    /// Every step, for a caller that must be exhaustive over the vocabulary (a flag table, a test).
    pub const ALL: [Self; 2] = [Self::Next, Self::Previous];

    /// How far this moves along the ring — the ONE place the direction becomes arithmetic, so the
    /// two arms cannot come to disagree about which way is forward.
    #[must_use]
    pub const fn offset(self) -> isize {
        match self {
            Self::Next => 1,
            Self::Previous => -1,
        }
    }

    /// Read a step off the wire (`"next"` / `"previous"`), [`None`] for anything else.
    ///
    /// The ONE definition of this vocabulary, exactly as [`PaneDir::from_wire`] is for the four
    /// directions: the wire action, the CLI flags and the keybinding read the same two words, so a
    /// third spelling cannot appear in one of them alone.
    #[must_use]
    pub fn from_wire(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|step| step.wire_str() == word)
    }

    /// This step's wire word — the inverse of [`from_wire`](Self::from_wire), and DERIVED from by it
    /// rather than tabulated beside it, so a parse and a render cannot drift apart while every test
    /// still passes (R296/R297's shape, applied on the way in).
    #[must_use]
    pub const fn wire_str(self) -> &'static str {
        match self {
            Self::Next => "next",
            Self::Previous => "previous",
        }
    }
}

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
    /// This window's IDENTITY — what survives a [`rename`](SessionRegistry::rename_window), where
    /// [`name`](Self::name) is only its address. See [`SessionId`] for why both exist.
    id: WindowId,
    name: String,
    workspace: Arc<Mutex<Workspace>>,
    layout: LayoutTree,
    /// Panes taken out of the tiling — [`layout`](Self::layout) holds no leaf for these.
    /// Pruned against the live pool by [`reconcile_layout`](Self::reconcile_layout), so a
    /// floating pane that exits leaves no entry behind.
    floating: HashSet<PaneId>,
    /// Where each floated pane came FROM, so it docks back into its own place rather than
    /// at the end ([`LeafHome`]).
    ///
    /// A sidecar, not an authority: `floating` alone says which panes are out, and a missing
    /// or unhonorable home costs an append, never correctness. It is deliberately NOT the
    /// same map as `floating`, because the two have different lifetimes — a home is captured
    /// when the pane floats and spent when the pane is TILED AGAIN, which is one
    /// [`reconcile_layout`](Self::reconcile_layout) LATER than the moment it stops floating.
    /// Keyed in one map, the dock-back that clears the float flag would drop the home on the
    /// floor before the leaf it was captured for could be placed.
    homes: HashMap<PaneId, LeafHome>,
    /// The `(cols, rows)` an operator PINNED for this window, or `None` while nobody has.
    ///
    /// It lives here, beside the [`layout`](Self::layout) it sizes, because that is what it is: the
    /// rectangle the arrangement is laid out over. Every OTHER answer to "how big is this window"
    /// is derived from who is currently looking at it, and a derived fact has no business being
    /// stored; this one is not derived from anything, which is why it needs a home and why the home
    /// is durable ([`WindowSnapshot::manual_size`](crate::snapshot::WindowSnapshot::manual_size)).
    ///
    /// Whether it is USED is a policy the host reads from the user's file, not a property of this
    /// field — a pinned size sits here inert until that policy names it, so writing one is never
    /// the same act as switching to it. See `sprag_host::window::WindowSize`.
    manual_size: Option<(u16, u16)>,
    layout_revision: u64,
    /// The pane this window is ON — tmux's active pane, `None` only while the window holds none.
    ///
    /// It lives here for the reason [`floating`](Self::floating) and the arrangement do, and the
    /// reason [`Session::current_window`] does one level up: which pane is current is a decision
    /// about the SESSION, not a property of whoever is looking at it. Every attached client follows
    /// it, a reattaching client inherits it, and a caller that draws nothing — an agent, a shell
    /// running `sprag` — can finally say "here".
    ///
    /// A FLOATING pane may be active: it is still a pane of this window and still takes input.
    /// Adjacency ([`LayoutTree::neighbor`]) simply has no answer for one, which is honest rather
    /// than a special case.
    ///
    /// Healed against the live pool by [`reconcile_layout`](Self::reconcile_layout), which is also
    /// where a closed active pane hands off to its neighbour. Moving it does NOT bump
    /// [`layout_revision`](Self::layout_revision): the arrangement has not changed, and a client
    /// that re-projected its whole tiling because the user pressed `select-pane -L` would be
    /// answering the wrong question.
    active: Option<PaneId>,
    /// The pane filling this window on its own, or `None` while none is — tmux's zoom.
    ///
    /// # The invariant, which is the whole design
    ///
    /// **A zoom names the pane this window is ON, and one it TILES, or there is no zoom.** So the
    /// pane a user types into is always a pane they can see, and that is a property rather than a
    /// convention: every way [`active`](Self::active) can move goes through
    /// [`set_active`](Self::set_active), which ends a zoom that stops naming it, and every way the
    /// TILED set can move goes through [`reconcile_layout`](Self::reconcile_layout), which does the
    /// same.
    ///
    /// Nothing else in this crate or the host mentions the zoom. A split ends it because a split
    /// selects its new pane; closing the zoomed pane ends it because the active pane hands off to a
    /// successor; floating it ends it because it stops being tiled. None of those verbs knows the
    /// zoom exists, and a verb added later still cannot forget it. The rival clears the flag at four
    /// hand-written call sites instead (herdr `9a4ce5e1`, `src/workspace/tab.rs:414` `:483` `:505`
    /// `:527`), which is four chances for a fifth verb to leave a zoom on a pane that is gone.
    ///
    /// A [`MOVE_PANE`](crate::SessionRegistry::move_pane) or a
    /// [`swap`](crate::SessionRegistry::swap_panes) does NOT end it: the arrangement moved and the
    /// filter did not, and because the zoom names a PANE rather than a position it follows the pane
    /// through a swap. herdr refuses a move into or out of a zoomed tab outright
    /// (`src/app/api/panes.rs:656`, `:708`).
    ///
    /// Durable ([`WindowSnapshot::zoomed`](crate::snapshot::WindowSnapshot::zoomed)) on
    /// [`active`](Self::active)'s argument, and healed by the first reconcile after a restore if
    /// the pane it names did not come back.
    zoomed: Option<PaneId>,
}

impl Window {
    /// An empty window named `name` over `pool` — which the caller obtains from
    /// [`Workspace::sibling`], so every window in the registry mints from ONE id counter
    /// (the load-bearing invariant; see the module docs).
    fn new(name: &str, pool: Workspace, id: WindowId) -> Self {
        Self {
            id,
            name: name.to_owned(),
            workspace: Arc::new(Mutex::new(pool)),
            layout: LayoutTree::new(),
            floating: HashSet::new(),
            homes: HashMap::new(),
            manual_size: None,
            layout_revision: 0,
            // An empty window is on no pane; the first reconcile after its birth pane lands makes
            // that pane active, so nothing has to remember to select a newly-born window's pane.
            active: None,
            // A window with nothing in it has nothing to fill itself with.
            zoomed: None,
        }
    }

    /// Rebuild a window from a durability snapshot: an empty `pool`, the recorded arrangement
    /// installed, and the recorded float set.
    ///
    /// The arrangement goes through the SAME [`LayoutTree::set_from_wire`] a client write does, so
    /// a corrupt stored tree is REFUSED here (its [`LayoutError`] rides out) and the daemon falls
    /// back to an empty boot rather than serving a malformed layout. Panes are NOT restored here:
    /// they are re-spawned at the host into `pool` under their old ids (the D4 birth seam), and the
    /// arrangement already names them by id — the first [`reconcile_layout`](Self::reconcile_layout)
    /// heals any that failed to come back. `homes` starts empty (not persisted; see the snapshot
    /// module docs) and `layout_revision` at 0 — a restored window is NEW, and every pre-reboot
    /// client that held a revision is gone.
    ///
    /// `manual_size`, by contrast, COMES BACK, and the difference is the same one that decides every
    /// field here: a home is a client's pixels and a revision is a client's bookmark, while a pinned
    /// size is an OPERATOR's decision about the window itself — the same kind of fact as the
    /// arrangement, restored for the same reason. A reboot that reopened a pinned window at whatever
    /// the first client to attach happened to be would have thrown the decision away.
    ///
    /// [`active`](Self::active) comes back on exactly that argument: it is a decision, and a reboot
    /// is precisely the moment there is no client to ask. If that pane failed to re-spawn, the first
    /// reconcile hands it on the same way a close does.
    ///
    /// So does [`zoomed`](Self::zoomed), and naming the PANE is what makes that restore exact: a
    /// stored boolean would come back bound to whichever pane the restore happened to make active,
    /// while an id either finds its pane or does not — and if it does not, the first reconcile ends
    /// the zoom rather than moving it to a stranger. Restored WITHOUT re-checking the invariant
    /// here on purpose: at this moment the pool is empty and every pane is still coming, so any
    /// check would refuse the whole set. The first reconcile after the re-spawns is where the
    /// question can be answered, and it is where it is answered.
    ///
    /// The IDENTITY is minted FRESH rather than restored, and that is the boundary
    /// [`SessionId`] states: an id says which window a change is about within one run of the
    /// daemon, and every client that could have held one across the reboot is gone. What comes
    /// back addressed is the NAME.
    ///
    /// Takes the whole [`WindowSnapshot`] rather than its seven fields spread out, because that
    /// is what its one caller has in hand and a positional list that long is a list a later field
    /// can be inserted into wrongly.
    fn restore(
        pool: Workspace,
        id: WindowId,
        snapshot: WindowSnapshot,
    ) -> Result<Self, LayoutError> {
        let mut tree = LayoutTree::new();
        tree.set_from_wire(snapshot.layout)?;
        Ok(Self {
            id,
            name: snapshot.name,
            workspace: Arc::new(Mutex::new(pool)),
            layout: tree,
            floating: snapshot.floating.into_iter().collect(),
            homes: HashMap::new(),
            manual_size: snapshot.manual_size,
            layout_revision: 0,
            active: snapshot.active,
            zoomed: snapshot.zoomed,
        })
    }

    /// The window's display name (default `"0"`, `"1"`, …; renamable via
    /// [`SessionRegistry::rename_window`]).
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The window's IDENTITY — unchanged by a rename, where [`name`](Self::name) is not.
    #[must_use]
    pub const fn id(&self) -> WindowId {
        self.id
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

    /// The `(cols, rows)` an operator pinned for this window, or `None` while nobody has — tmux's
    /// `resize-window` size, read by the host's `window-size manual` policy.
    #[must_use]
    pub fn manual_size(&self) -> Option<(u16, u16)> {
        self.manual_size
    }

    /// Pin this window's size, or `None` to un-pin it and hand the window back to whatever policy
    /// derives one from the attached clients — tmux `resize-window`.
    ///
    /// It does NOT bump [`layout_revision`](Self::layout_revision), and the reason is what that
    /// number means: a revision says a client's PROJECTION of the arrangement is stale, and the
    /// arrangement here is untouched — the same tree, over a different rectangle. What a client has
    /// to re-read is the session's window, which reaches it as its own wire fact. Bumping here would
    /// tell every client its layout cache was invalid to deliver news about something else.
    pub fn set_manual_size(&mut self, size: Option<(u16, u16)>) {
        self.manual_size = size;
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
    /// A pane that is tiled again lands at the [`LeafHome`] its float captured, if that home
    /// is still honorable; this is the one place a leaf moves, so it is also the one place a
    /// home is spent.
    ///
    /// The caller resolves `panes` under the WORKSPACE lock and calls this under the
    /// registry lock, so the two are never nested (see [`crate::layout`]).
    pub fn reconcile_layout(&mut self, panes: &[PaneId]) -> &LayoutTree {
        let live: HashSet<PaneId> = panes.iter().copied().collect();
        self.bump_if_changed(|window| {
            // FIRST, while the arrangement still holds the leaf of a pane that has just gone: the
            // successor is a question about where the user WAS, and one step further on there is
            // nothing left to ask it of. Inside the closure so it runs on every reconcile, and
            // deliberately not part of what the closure COMPARES — the active pane is not the
            // arrangement (see the field's docs).
            window.heal_active(panes);
            // Prune INSIDE the compare: a floating pane that exits changes what a client
            // must draw (one fewer window) while leaving the tiling untouched, so pruning
            // outside would drop that change on the floor.
            window.floating.retain(|pane| live.contains(pane));
            // A pane that exits takes its home with it: nothing will ever come back to it, so
            // the entry is dead weight that would accumulate for the window's life. (NOT to
            // avoid an id collision — ids are minted from one registry-wide counter and are
            // never reused, so a stale home cannot be mistaken for a future pane's.)
            window.homes.retain(|pane, _| live.contains(pane));
            let tiled = window.tiled(panes);
            window.layout.reconcile(&tiled, &mut window.homes);
            // LAST, because it is a claim about where the user ended up and about what this window
            // tiles, and both of those have just been decided above. This is the half of the zoom
            // invariant that no verb triggers: a zoomed pane that EXITED, or that a client floated
            // out, ends its zoom here rather than in whatever closed or floated it.
            window.heal_zoom(&tiled);
        });
        &self.layout
    }

    /// The panes this window would TILE — its live pool minus what is floated out.
    ///
    /// Asked of the POOL and the float set rather than of the tree, which is R284's rule: the tree
    /// reconciles LAZILY, so a question answered from it refuses a pane spawned since the last
    /// read. This is exactly what a reconcile arranges, so the answer never depends on when anyone
    /// last took one.
    fn tiled(&self, panes: &[PaneId]) -> Vec<PaneId> {
        panes
            .iter()
            .copied()
            .filter(|pane| !self.floating.contains(pane))
            .collect()
    }

    /// The pane this window is ON, or `None` while it holds no panes — tmux's active pane.
    #[must_use]
    pub fn active_pane(&self) -> Option<PaneId> {
        self.active
    }

    /// Make `pane` this window's active one — tmux `select-pane`. Answers whether it was there to
    /// select.
    ///
    /// `panes` is the window's live pool, resolved by the caller under the WORKSPACE lock exactly
    /// as [`reconcile_layout`](Self::reconcile_layout)'s is, so this stays pure and the two locks
    /// stay sequential. Membership is checked against the POOL rather than the tiling, because a
    /// floated pane is still a pane a user can be on.
    ///
    /// A pane the pool does not hold is REFUSED and nothing moves: silently keeping the old one
    /// would answer "selected" for a pane that is not there, and silently clearing it would lose
    /// the user's place over a typo.
    pub fn select_pane(&mut self, pane: PaneId, panes: &[PaneId]) -> bool {
        if !panes.contains(&pane) {
            return false;
        }
        self.bump_if_changed(|window| {
            let tiled = window.tiled(panes);
            window.set_active(pane, &tiled);
        });
        true
    }

    /// The pane filling this window on its own, or `None` while none is — tmux's zoom.
    #[must_use]
    pub fn zoomed(&self) -> Option<PaneId> {
        self.zoomed
    }

    /// Fill this window with `pane` alone, or end the zoom — tmux `resize-pane -Z`.
    ///
    /// `on` absent TOGGLES the target's own zoom, so a key bound to it is a switch whichever pane
    /// it is aimed at. Answers what happened, or `None` — refused, with nothing moved — for a pane
    /// this window does not TILE: one that is not in the pool at all, and one a client has floated
    /// out.
    ///
    /// **A floating target is refused rather than answered**, and the two sibling verbs are why. A
    /// zoom acts on the TILING, and `Window::place_pane`'s callers
    /// (`sprag_host::wire::SPLIT_ACTION`) and
    /// [`SessionRegistry::move_pane`](crate::SessionRegistry::move_pane) both already refuse a
    /// target the tiling does not hold. Treating it as an EDGE instead — R284's "a direction with
    /// no neighbour is not an error" — was the wrong analogy: an edge is a boundary a MOVEMENT ran
    /// into, while a floated pane is a target that cannot be zoomed at all in the state it is in.
    /// It also cost an ambiguity, which is how it was caught: accepted, a toggle aimed at a
    /// floating pane answered exactly what toggling a zoom OFF answers, so no caller could tell
    /// them apart. Membership is asked of the POOL MINUS THE FLOAT SET, never of the tree — R284's
    /// rule, because the tree reconciles LAZILY and a question answered from it refuses a pane
    /// spawned since anyone last read one.
    ///
    /// **Zooming SELECTS**, because the invariant [`zoomed`](Self::zoomed) states leaves no other
    /// coherent option: the pane filling the window is the pane the window is on. Turning a zoom
    /// OFF by naming a pane selects it too — the caller named where it wants to be.
    ///
    /// The window's own SIZE is nowhere in this: a zoom changes which pane the arrangement projects
    /// to, and how many cells that is remains `tile`'s answer over the session's arbitrated window.
    pub fn zoom_pane(
        &mut self,
        pane: PaneId,
        on: Option<bool>,
        panes: &[PaneId],
    ) -> Option<ZoomOutcome> {
        let tiled = self.tiled(panes);
        if !tiled.contains(&pane) {
            return None;
        }
        // Read BEFORE the select, which can clear it: `zoom-pane <other>` with no mode is a request
        // to zoom that other pane, never to un-zoom it because the select got there first.
        let was = self.zoomed;
        let want = on.unwrap_or(was != Some(pane));
        self.bump_if_changed(|window| {
            window.set_active(pane, &tiled);
            window.zoomed = want.then_some(pane);
        });
        Some(ZoomOutcome {
            zoomed: self.zoomed.is_some(),
            // Compared as the two PANE ids rather than as two flags, so re-zooming a DIFFERENT pane
            // — which is one request, not an un-zoom and a zoom — reports the change it is.
            changed: self.zoomed != was,
        })
    }

    /// Move [`active`](Self::active) to `pane` and end a zoom that no longer names it — the ONE
    /// place the active pane is written, which is what makes the zoom invariant unforgettable
    /// rather than merely documented.
    ///
    /// Membership is the caller's to check ([`select_pane`](Self::select_pane) refuses, a zoom
    /// refuses, [`heal_active`](Self::heal_active) picks from the live set), because the three have
    /// different answers for a pane that is not there and only one of them is a refusal.
    fn set_active(&mut self, pane: PaneId, tiled: &[PaneId]) {
        self.active = Some(pane);
        self.heal_zoom(tiled);
    }

    /// End a zoom that no longer names a pane this window is ON and TILES — the invariant, stated
    /// once and called from both places its inputs can move.
    fn heal_zoom(&mut self, tiled: &[PaneId]) {
        if self
            .zoomed
            .is_some_and(|pane| self.active != Some(pane) || !tiled.contains(&pane))
        {
            self.zoomed = None;
        }
    }

    /// Keep [`active`](Self::active) naming a pane that is actually in `panes`.
    ///
    /// Three cases, and the middle one is the whole design: a window that has just gained its first
    /// pane takes it; one whose active pane is still there keeps it; and one whose active pane is
    /// GONE hands off to [`successor`](Self::successor) — never to "the first pane", which would
    /// throw a user from pane 7 of 8 back to the top of the window for closing one.
    fn heal_active(&mut self, panes: &[PaneId]) {
        match self.active {
            Some(active) if panes.contains(&active) => {}
            Some(gone) => {
                self.active = self
                    .successor(gone, panes)
                    .or_else(|| panes.first().copied());
            }
            None => self.active = panes.first().copied(),
        }
    }

    /// Who inherits when `gone` leaves: its neighbour in the arrangement AS IT STANDS — the next
    /// pane in paint order, else the previous one.
    ///
    /// Paint order rather than [`LayoutTree::neighbor`], and the difference is the point: a
    /// direction has to be chosen for the user and any choice of one is arbitrary, while "the next
    /// one along" is total, is what the pane list already shows, and is what tmux does. `None` when
    /// `gone` held no leaf (it was floating, or already reconciled away) — the caller falls back.
    fn successor(&self, gone: PaneId, panes: &[PaneId]) -> Option<PaneId> {
        let order = self.layout.panes();
        let at = order.iter().position(|pane| *pane == gone)?;
        order[at + 1..]
            .iter()
            .chain(order[..at].iter().rev())
            .find(|pane| panes.contains(pane))
            .copied()
    }

    /// This window's live pool ids, read under its OWN workspace lock and returned — what
    /// [`place_pane`](Self::place_pane) and [`select_pane`](Self::select_pane) take as `panes` on
    /// the paths that start INSIDE the registry, where no host has resolved them already.
    ///
    /// The lock is taken and released here, never held across the call that consumes the answer,
    /// so the registry-then-workspace order the module keeps is unbroken.
    fn pane_ids(&self) -> Vec<PaneId> {
        let pool = self
            .workspace
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        pool.panes().iter().map(Pane::id).collect()
    }

    /// Whether this window TILES `pane` — it is in the pool and not floated out.
    ///
    /// Asked of the POOL and the float set rather than of the arrangement, which is the whole
    /// point: the tree reconciles lazily, so a pane spawned since the last read holds no leaf yet
    /// and a tree-membership test would refuse a target that is plainly there. Pool-minus-floating
    /// is exactly the set [`reconcile_layout`](Self::reconcile_layout) arranges, so this answers
    /// what the tree WILL say without depending on when anybody last read it.
    fn tiles(&self, pane: PaneId) -> bool {
        !self.floating.contains(&pane) && self.pane_ids().contains(&pane)
    }

    /// Exchange two TILED panes' positions in this window — the same-window half of
    /// [`SessionRegistry::swap_panes`]. Answers whether both were there to exchange.
    ///
    /// RECONCILES FIRST, in [`place_pane`](Self::place_pane)'s exact shape and for its reason: a
    /// pane that has not been arranged yet holds no leaf to exchange, and the swap must act on the
    /// tiling as it IS rather than as it was when someone last read it. The exchange then goes
    /// through [`bump_if_changed`](Self::bump_if_changed) like every other arrangement write, so a
    /// swap that moved something bumps the revision and one that found nothing does not.
    ///
    /// [`active`](Self::active) is deliberately untouched: it names a PANE, and the pane the user
    /// was on is still in this window. tmux and herdr both keep the user on the pane they swapped.
    fn swap_tiled(&mut self, a: PaneId, b: PaneId) -> bool {
        self.reconcile_own();
        let mut swapped = false;
        self.bump_if_changed(|window| {
            swapped = window.layout.swap_panes(a, b);
        });
        swapped
    }

    /// Take in `arriving` at the place `leaving` just vacated — the destination half of a
    /// CROSS-WINDOW [`swap`](SessionRegistry::swap_panes), where `home` was captured from this
    /// window's own tree before `leaving`'s leaf collapsed.
    ///
    /// Reconciles first (which prunes the departed pane and would otherwise APPEND the arriving
    /// one), then honors the home. `home` is `None` when `leaving` was this window's sole tiled
    /// pane — nothing to be beside — and the reconcile's append is then the same arrangement by a
    /// shorter route.
    ///
    /// **A window that was ON the departing pane lands on the arriving one**, and this is the one
    /// place that answer is available: [`heal_active`](Self::heal_active) would hand off to a
    /// neighbour, which is right when a pane merely closed and wrong here, because the cell the
    /// user was looking at still exists and something specific took it.
    fn adopt_at(&mut self, arriving: PaneId, home: Option<LeafHome>, leaving: PaneId) {
        if let Some(home) = home {
            self.homes.insert(arriving, home);
        }
        let was_on_it = self.active == Some(leaving);
        self.reconcile_own();
        if was_on_it {
            self.active = Some(arriving);
        }
    }

    /// Self-heal the arrangement against this window's OWN live pool — the caller-less form of
    /// [`reconcile_layout`](Self::reconcile_layout), for the paths that change a window's pane set
    /// from INSIDE the registry (a cross-window move) rather than from a host that already holds
    /// the pane ids.
    ///
    /// Keeps the [`layout`](crate::layout) lock discipline exactly: the pool ids are collected
    /// under the workspace lock, which is RELEASED before [`reconcile_layout`](Self::reconcile_layout)
    /// runs, so the registry lock the caller holds and this window's workspace lock are never both
    /// held at once (registry-then-workspace, released, then the lock-free reconcile).
    fn reconcile_own(&mut self) {
        let ids: Vec<PaneId> = {
            let pool = self
                .workspace
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            pool.panes().iter().map(Pane::id).collect()
        };
        self.reconcile_layout(&ids);
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

    /// Divide `target`'s cell and put `pane` in the half on `side`, along `dir` — tmux
    /// `split-window -h` / `-v` for a pane that was just born, tmux `move-pane` for one that is
    /// already tiled here. Returns whether `target` was there to divide.
    ///
    /// ONE method for both because the two differ only in where the pane came from, which is a
    /// fact about the CALLER: `LayoutTree::place_beside` moves an already-arranged pane rather
    /// than duplicating it, so a split of a fresh pane and a re-placement of an existing one reach
    /// the same insertion by the same path. A second method would be a second positioning rule to
    /// keep in step with this one.
    ///
    /// RECONCILES FIRST, against the `panes` the caller resolved under the workspace lock, for
    /// two reasons that are really one: the pane being placed may have been spawned a moment ago
    /// and not be in the tree yet, and the target must be judged against the tiling as it IS
    /// rather than as it was when someone last read it. Doing both inside this one `&mut Window`
    /// call is what makes the placement atomic — [`reconcile_layout`](Self::reconcile_layout) is
    /// the only other thing that moves a leaf, and it needs the same borrow, so no reconcile can
    /// land in between and append the pane behind the placement's back.
    ///
    /// `false` leaves the window's arrangement exactly as the reconcile left it: the target is
    /// gone, floating, or another window's. The caller REFUSES on that — a direction the user
    /// spelled is a request, not a hint, so a placement that cannot reach its target must not
    /// quietly become an append.
    pub fn place_pane(
        &mut self,
        pane: PaneId,
        target: PaneId,
        side: SplitSide,
        dir: SplitDir,
        panes: &[PaneId],
    ) -> bool {
        self.reconcile_layout(panes);
        let mut placed = false;
        self.bump_if_changed(|window| {
            placed = window.layout.place_beside(pane, target, side, dir);
        });
        placed
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
    /// **Floating CAPTURES the pane's place** ([`LeafHome`]) before the leaf collapses, so
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
    /// Compares the tree, the float set AND the zoom, because all three are state a client
    /// projects: a pane that stops floating changes what the client must draw even on the rare
    /// path where the tiling comes out identical, and a zoom changes it far more than that while
    /// leaving the tiling untouched by construction. It does NOT compare `homes` — a home is not
    /// served and not projected, so capturing one changes nothing a client could re-read. Bumping
    /// on it would wake every client to fetch an arrangement identical to the one it holds.
    ///
    /// It does not compare `active` either, and the asymmetry with the zoom is the point: which
    /// pane a window is ON reaches clients as its own fact and does not re-divide anything, while
    /// which pane FILLS it decides what there is to draw at all. So `select_pane` still bumps
    /// nothing on the ordinary path, and bumps when — and only when — it ended a zoom.
    fn bump_if_changed(&mut self, change: impl FnOnce(&mut Self)) {
        let tree = self.layout.clone();
        let floating = self.floating.clone();
        let zoomed = self.zoomed;
        change(self);
        if self.layout != tree || self.floating != floating || self.zoomed != zoomed {
            self.layout_revision += 1;
        }
    }

    /// Close every pane in this window's pool and RETURN them, so the caller runs each pane's
    /// blocking [`PanePty`](crate::PanePty) `Drop` (kill / wait / join the reader) OFF the
    /// registry lock — the discipline [`KillOutcome`] exists to keep. Used when one window is
    /// killed ([`SessionRegistry::kill_window`]) and, per window, when a whole session is
    /// ([`Session::drain_panes`]).
    ///
    /// Closing removes each pane from the pool first, so the window already counts as idle
    /// (empty pool) before the returned panes are dropped.
    fn drain(&self) -> Vec<Pane> {
        let mut pool = self
            .workspace
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let ids: Vec<PaneId> = pool.panes().iter().map(Pane::id).collect();
        ids.into_iter().flat_map(|id| pool.close(id)).collect()
    }
}

/// What a zoom request did — the answer of [`Window::zoom_pane`].
///
/// Two bools with names, rather than a `(bool, bool)`: at a call site the pair would say nothing
/// about which is which, and they are not interchangeable — one is a state and the other is a claim
/// about a transition.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ZoomOutcome {
    /// Whether ONE pane is filling the window after the call.
    pub zoomed: bool,
    /// Whether that differs from what was in force before it — what decides whether the daemon
    /// wakes every parked client. False exactly for a re-assertion of the state already in force,
    /// which is the one way a well-formed zoom request moves nothing.
    ///
    /// The pair is therefore TOTAL over four cases and every one of them is distinct, which is the
    /// property that makes an operator-facing sentence exact rather than a list of causes: now
    /// filling / already filling / arrangement back / arrangement already showing. A floating
    /// target used to be a fifth reading of the third, and is a refusal now
    /// ([`Window::zoom_pane`]).
    pub changed: bool,
}

/// Why a session operation was refused. The registry is unchanged in either case.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SessionError {
    /// The name is already taken ([`SessionRegistry::new_session`]).
    Duplicate(String),
    /// No session carries the name ([`SessionRegistry::kill_session`]).
    Unknown(String),
    /// The name breaks the grammar an ADDRESS has to satisfy ([`SessionName`]) — carried whole so
    /// an in-process caller can say WHICH rule rather than listing all of them.
    Malformed(SessionNameError),
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Duplicate(name) => write!(f, "a session named {name:?} already exists"),
            Self::Unknown(name) => write!(f, "no session named {name:?}"),
            Self::Malformed(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for SessionError {}

/// Why a pane MOVE between windows was refused ([`break_pane`](SessionRegistry::break_pane) /
/// [`join_pane`](SessionRegistry::join_pane)). Its own class rather than a [`SessionError`]
/// arm, because a move addresses THREE things a session op does not — a source window, a
/// destination, and a specific pane by id — and each has a distinct way to be wrong. Every
/// variant leaves the registry UNCHANGED: the pane is taken out of its pool only after every
/// check has passed, so a refusal never strands a pane between two windows.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum PaneMoveError {
    /// No session carries the name.
    UnknownSession(String),
    /// The session has no window with the (source or destination) name.
    UnknownWindow(String),
    /// The named window does not hold the pane — a client naming a pane that has since exited or
    /// that lives in another window. Refused rather than silently retargeted.
    UnknownPane(PaneId),
    /// `break-pane` on a window that tiles only ONE pane: moving it to a new window would empty
    /// and close the source, a rename dressed as a move. tmux refuses the same ("can't break the
    /// only pane in a window").
    LastPane,
    /// `break-pane` with an explicit new-window name already taken in the session — a name is an
    /// address, so it must stay unique.
    DuplicateWindow(String),
    /// `join-pane` with the source and destination window being the SAME one — a no-op move.
    SameWindow(String),
    /// `move-pane` with the pane and the target being the SAME pane. Refused rather than answered
    /// "nothing moved", because unlike a swap this request has no reading at all: a pane cannot be
    /// placed beside itself, and the division it asks for does not exist.
    SamePane(PaneId),
}

impl std::fmt::Display for PaneMoveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownSession(name) => write!(f, "no session named {name:?}"),
            Self::UnknownWindow(name) => write!(f, "no window named {name:?}"),
            Self::UnknownPane(id) => write!(f, "no pane with id {} in that window", id.0),
            Self::LastPane => write!(f, "cannot break the only pane in a window"),
            Self::DuplicateWindow(name) => write!(f, "a window named {name:?} already exists"),
            Self::SameWindow(name) => write!(f, "source and destination window are both {name:?}"),
            Self::SamePane(id) => write!(f, "cannot place pane {} beside itself", id.0),
        }
    }
}

impl std::error::Error for PaneMoveError {}

/// What a [`kill_session`](SessionRegistry::kill_session) did — carrying the reaped owners so the
/// CALLER drops them (running each pane's blocking [`PanePty`](crate::PanePty) `Drop`: kill,
/// wait, join the reader) OUTSIDE the registry lock. That is the discipline the `close` action
/// keeps; holding it here keeps the same "no blocking pane teardown under a scene lock" shape
/// rather than re-introducing the one `close` pays to avoid.
pub enum KillOutcome {
    /// A non-last session was removed. The daemon keeps serving IFF a surviving session still
    /// holds a live pane; if the removed one held the LAST live pane and the survivors are empty,
    /// the reaper finds none and exits the daemon (the owner's "zero live panes ⇒ exit" policy,
    /// unchanged by this path). So this is a removal, not an unconditional "the server stays up" —
    /// liveness decides the rest. The removed [`Session`] rides here to be dropped off-lock.
    Removed(Session),
    /// The LAST session was killed: its panes were DRAINED (they ride here to drop off-lock) and
    /// the caller must EXIT the daemon (tmux's "killing the last session ends the server"). The
    /// empty session shell is kept so [`default_session`](SessionRegistry::default_session) stays
    /// total for the brief window before the process actually dies.
    KilledServer(Vec<Pane>),
}

/// What a [`kill_window`](SessionRegistry::kill_window) did.
///
/// Like [`KillOutcome`], it carries the reaped panes so the CALLER drops them (running each
/// pane's blocking [`PanePty`](crate::PanePty) `Drop`) OFF the registry lock.
pub enum WindowKillOutcome {
    /// A non-last window was removed from its session; its drained panes ride here to drop
    /// off-lock. The session (and the daemon) keep running.
    Removed(Vec<Pane>),
    /// The window was the session's LAST, so killing it ended the SESSION (tmux "kill the last
    /// window ⇒ the session is gone"). The escalation's own [`KillOutcome`] rides here — the
    /// caller handles it exactly as a [`kill_session`](SessionRegistry::kill_session) result (a
    /// non-last session removed, or the last one drained and the daemon ended).
    Session(KillOutcome),
}

/// A window's public identity for a display client — the mux `windows` slot and the tabbed
/// client that draws from it: the window's NAME and whether it is its session's CURRENT window.
///
/// A view over the tree, not part of it: built on demand by [`Session::window_infos`], serialised
/// over the wire, and returned by the `HostClient` window read — one shape the wire slot, a
/// client's mirror, and the in-process arm all speak, so none can drift.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WindowInfo {
    /// The window's display name (a tab label).
    pub name: String,
    /// Whether this is the session's current window (the active tab).
    pub current: bool,
}

/// A session's public identity for a display client — the registry-WIDE mux `sessions` slot and a
/// session-switcher sidebar that draws from it: the session's NAME (its attach address), its
/// window COUNT, and whether it is the registry DEFAULT (where an unscoped request lands).
///
/// The `default` flag is NOT "is this the client's attached session" — nothing is attached at this
/// layer; a switcher highlights its OWN session via a client-local fact (`sprag_host`'s
/// `HostClient::current_session`) that the wire never carries. Like [`WindowInfo`], it is a view
/// over the registry, not part of it: built on demand, serialised over the wire, and returned by
/// the session read — one shape the wire slot, a client's mirror, and the in-process arm all
/// speak, so none can drift.
///
/// Every field here is the REGISTRY's own: it moves when this daemon performs an event, and the
/// scene revision already announces that it did. Where a session is working, what it is serving, and
/// on what branch are NOT here — they are SAMPLED from the operating system, they move with nothing
/// this daemon can see, and R282 gave them their own shape
/// ([`SessionActivity`](crate::SessionActivity)), asked for separately and answered with the age of
/// its sample. Serving both from here made the cheapest question in the mux cost a `/proc` walk of
/// the whole box on every poll wake of every attached client; that type's module docs carry the
/// measurement.
///
/// The structural fields ([`name`](Self::name) / [`windows`](Self::windows) /
/// [`default`](Self::default)) come from [`SessionRegistry::session_infos`], read under the registry
/// lock alone. [`panes`](Self::panes) needs every window's pool lock, so it is filled by
/// [`SessionRegistry::session_infos_live`] off the registry lock; [`attached`](Self::attached) is
/// dispatch-layer state only a host knows. `#[serde(default)]` on both keeps a peer that omits them
/// readable.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SessionInfo {
    /// The session's display name — the address a client names to attach / switch.
    pub name: String,
    /// How many windows the session holds.
    pub windows: usize,
    /// How many panes the session holds across ALL its windows — the live count that tells a
    /// resting empty anchor (0 panes) from a working session. Filled only by
    /// [`SessionRegistry::session_infos_live`] (it needs each window's pool lock, which the
    /// structural [`session_infos`](SessionRegistry::session_infos) must not take under the
    /// registry lock); a registry-only list carries `0`. Consumed by [`is_listable`](Self::is_listable).
    ///
    /// TRULY additive like the enrichment fields below: `skip_serializing_if` keeps a paneless
    /// session at its prior wire shape, and `#[serde(default)]` reads a peer that omits it as `0`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub panes: usize,
    /// Whether this is the registry default (where an unscoped request lands).
    pub default: bool,
    /// How many distinct clients are currently ATTACHED to this session (R-PR67 Stage 1) — the
    /// tmux `list-clients` / cmux "N viewing this workspace" count. Unlike the other enrichment
    /// fields this is NOT derived from the registry (a session has no idea who is watching it):
    /// it lives in the daemon's dispatch layer ([`crate`]-external `AttachmentRegistry`), filled
    /// in HOST-side when the session list is served, so a session built off the registry alone
    /// carries `0`. Zero also means "not a daemon" (an in-process host has no wire clients).
    ///
    /// TRULY additive like the fields above: `skip_serializing_if` keeps an unattached session at
    /// its prior wire shape, and `#[serde(default)]` reads a peer that omits it back as `0`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub attached: usize,
}

impl SessionInfo {
    /// Whether a HUMAN-facing session list should show this session — the SSOT rule every
    /// listing surface (`sprag ls`, the GUI session rail) applies so they cannot disagree on the
    /// resting anchor. A session lists iff it holds a pane OR a client is attached to it:
    ///
    /// * `panes > 0` — a working session, always shown.
    /// * `attached > 0` — an EMPTY session a client is currently viewing (all its panes closed
    ///   while it stays attached). Shown so a client can see where it is; tmux cannot represent
    ///   this state at all (an empty session does not exist there), so honestly listing it is a
    ///   sprag-superior refinement, not a divergence.
    ///
    /// The daemon keeps an empty resting anchor for `default_session` totality + reattach
    /// durability (unlike tmux, whose server exits when its last session dies); that anchor holds
    /// no pane and, at rest, no attachment — so it is hidden, matching `tmux ls` at rest while the
    /// daemon (and its durable layout) live on. Both facts are known only host-side (`panes` from
    /// the registry, `attached` from the dispatch layer), so this runs there, once, after both are
    /// filled — never in the wire producer alone, or the in-process arm would drift from it.
    #[must_use]
    pub fn is_listable(&self) -> bool {
        self.panes > 0 || self.attached > 0
    }
}

/// `skip_serializing_if` predicate for [`SessionInfo::attached`] / [`SessionInfo::panes`] — a
/// `usize` has no `is_empty`, so the "omit the default" rule the other enrichment fields get from
/// `Option`/`Vec` is spelled out here, keeping a paneless / unattached session byte-identical to
/// the pre-enrichment wire shape.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// One session: a named attach unit owning an ordered, non-empty set of [`Window`]s
/// with exactly one current window.
///
/// A client attaches to a session and views its current window. A session boots with a single
/// window; [`new_window`](Self::new_window) / [`select_window`](Self::select_window) /
/// [`rename_window`](Self::rename_window) and the registry's
/// [`kill_window`](SessionRegistry::kill_window) are the ops on this shape (tmux's windows).
pub struct Session {
    /// This session's IDENTITY — what a [`rename`](SessionRegistry::rename_session) does not move.
    /// See [`SessionId`].
    id: SessionId,
    name: String,
    windows: Vec<Window>,
    current_window: usize,
    /// The counter this session's windows mint their [`WindowId`]s from — the registry's, cloned,
    /// so every window in the registry draws from ONE source exactly as every pane does.
    ///
    /// It is here rather than on the registry because a window is born inside a `&mut Session`
    /// ([`new_window`](Self::new_window), [`break_pane`](Self::break_pane)), which cannot reach the
    /// registry. Same shape and same reason as [`Workspace`]'s pane counter.
    ids: Arc<AtomicU64>,
}

impl Session {
    /// A session named `name` holding one empty window `"0"` — a session always has at
    /// least one window, which is what makes [`current_window`](Self::current_window)
    /// total.
    fn new(name: &str, pool: Workspace, ids: Arc<AtomicU64>) -> Self {
        let id = SessionId(ids.fetch_add(1, Ordering::Relaxed));
        let window = Window::new("0", pool, WindowId(ids.fetch_add(1, Ordering::Relaxed)));
        Self {
            id,
            name: name.to_owned(),
            windows: vec![window],
            current_window: 0,
            ids,
        }
    }

    /// The next [`WindowId`] this session's counter hands out.
    fn mint_window(&self) -> WindowId {
        WindowId(self.ids.fetch_add(1, Ordering::Relaxed))
    }

    /// The session's display name (default `"0"`; the tmux `-s` name later).
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The session's IDENTITY — unchanged by a rename, where [`name`](Self::name) is not.
    #[must_use]
    pub const fn id(&self) -> SessionId {
        self.id
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

    /// This session's windows as [`WindowInfo`]s, in order, with the current one marked — the
    /// list the mux `windows` slot serves and a tabbed client draws.
    #[must_use]
    pub fn window_infos(&self) -> Vec<WindowInfo> {
        let current = self.current_window().name();
        self.windows
            .iter()
            .map(|window| WindowInfo {
                name: window.name.clone(),
                current: window.name == current,
            })
            .collect()
    }

    /// Remove every pane from every window and RETURN them — used when the LAST session is killed
    /// ([`SessionRegistry::kill_session`]), so no live pane keeps the daemon alive, WITHOUT
    /// removing the session (which would empty the registry and unresolve the default).
    ///
    /// The panes are RETURNED, not dropped here, so the caller runs each pane's blocking
    /// `PanePty::Drop` (kill / wait / join the reader) OFF the registry lock. Closing removes each
    /// pane from the pool first, so the session already counts as idle (empty pool) before the
    /// returned panes are dropped — and each drop then SIGHUPs the child and fires its `on_exit`,
    /// nudging the reaper.
    fn drain_panes(&self) -> Vec<Pane> {
        self.windows.iter().flat_map(Window::drain).collect()
    }

    /// The lowest non-negative integer name not currently in use by a window of this session,
    /// as a string — how [`new_window`](Self::new_window) allocates, mirroring tmux's
    /// `new-window` picking the lowest free index and the registry's own
    /// [`lowest_free_name`](SessionRegistry::lowest_free_name) one level up.
    ///
    /// Total by the same argument: at most `windows.len()` names are taken, so one of the
    /// `len + 1` candidates in `0..=len` is free.
    fn lowest_free_window_name(&self) -> String {
        (0u64..)
            .map(|n| n.to_string())
            .find(|candidate| !self.windows.iter().any(|w| w.name == *candidate))
            .expect("some name in 0..=len is always free")
    }

    /// Create a window, holding an empty pool, SELECT it, and return the name it got — tmux
    /// `new-window`, which appends a window and makes it current.
    ///
    /// `name` is the caller's choice; `None` allocates the lowest free integer name
    /// (`lowest_free_window_name`), the way tmux's
    /// `new-window` with no `-n` does. The pool clones the ONE registry-wide id counter out of
    /// an existing window ([`Workspace::sibling`]), so a [`PaneId`] stays unique across every
    /// window of every session (the module's load-bearing invariant).
    ///
    /// The window is born EMPTY here; the host births its first pane (the D4 seam — a birth
    /// pane must carry the daemon's `on_pane_exit` death-signal, which the pinion-free registry
    /// does not hold). Selecting it is the tmux behaviour and is session state: every client
    /// attached to this session follows the current window, so a `new-window` moves them all,
    /// exactly as tmux does.
    ///
    /// # Errors
    ///
    /// [`SessionError::Duplicate`] if an explicit `name` is already a window of this session —
    /// a name is how a window is addressed, so two of them would make the address ambiguous.
    /// The allocated path cannot fail: it picks a name free by construction.
    pub fn new_window(&mut self, name: Option<&str>) -> Result<String, SessionError> {
        let name = match name {
            Some(name) => {
                if self.windows.iter().any(|w| w.name == name) {
                    return Err(SessionError::Duplicate(name.to_owned()));
                }
                name.to_owned()
            }
            None => self.lowest_free_window_name(),
        };
        let pool = self
            .current_window()
            .workspace()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .sibling();
        let id = self.mint_window();
        self.windows.push(Window::new(&name, pool, id));
        self.current_window = self.windows.len() - 1;
        Ok(name)
    }

    /// Make the window named `name` current — tmux `select-window`. Session state: every
    /// attached client follows it.
    ///
    /// # Errors
    ///
    /// [`SessionError::Unknown`] if no window of this session carries `name`. The current
    /// window is unchanged.
    pub fn select_window(&mut self, name: &str) -> Result<(), SessionError> {
        let idx = self
            .windows
            .iter()
            .position(|w| w.name == name)
            .ok_or_else(|| SessionError::Unknown(name.to_owned()))?;
        self.current_window = idx;
        Ok(())
    }

    /// Make the NEXT or PREVIOUS window current, WRAPPING — tmux `next-window` /
    /// `previous-window`, and `select-window -n` / `-p`. Answers the name it landed on.
    ///
    /// # Why this wraps where the PANE walk does not
    ///
    /// A pane walk is SPATIAL: `select-pane -L` measures against an arrangement, an edge really
    /// exists, and `at_edge` is the honest answer there. A window list is an ORDINAL RING — the
    /// order the sidebar draws and the order `windows` publishes — so "the next one" is always
    /// defined and wrapping is what the ring means. tmux wraps for the same reason, and this
    /// session's own `Vec` order is what both ends already agree on.
    ///
    /// TOTAL, and that is a property rather than an accident: a session always holds at least one
    /// window, so there is always somewhere to land. With ONE window it lands on itself and answers
    /// its name — the honest "you are where you were", which a caller can compare against what it
    /// had. No error case, so unlike [`select_window`](Self::select_window) it cannot refuse.
    pub fn select_window_relative(&mut self, step: WindowStep) -> &str {
        let len = self.windows.len();
        // `current_window` is an index into a `Vec` that is never empty, so the rem_euclid below is
        // total; the `max(1)` says so to a reader rather than relying on it.
        let len = len.max(1);
        let here = self.current_window as isize;
        let next = (here + step.offset()).rem_euclid(len as isize) as usize;
        self.current_window = next.min(len - 1);
        self.windows[self.current_window].name.as_str()
    }

    /// Rename the window named `name` to `new` — tmux `rename-window`.
    ///
    /// # Errors
    ///
    /// [`SessionError::Unknown`] if no window carries `name`; [`SessionError::Duplicate`] if
    /// `new` is already another window's name (a name is an address, so it must stay unique).
    /// Renaming a window to the name it already has is a no-op, not a duplicate.
    pub fn rename_window(&mut self, name: &str, new: &str) -> Result<(), SessionError> {
        let idx = self
            .windows
            .iter()
            .position(|w| w.name == name)
            .ok_or_else(|| SessionError::Unknown(name.to_owned()))?;
        if new != name && self.windows.iter().any(|w| w.name == new) {
            return Err(SessionError::Duplicate(new.to_owned()));
        }
        self.windows[idx].name = new.to_owned();
        Ok(())
    }

    /// Pin the size of the window named `name`, or un-pin it with `None` — tmux `resize-window`.
    /// See [`Window::set_manual_size`].
    ///
    /// # Errors
    ///
    /// [`SessionError::Unknown`] if no window of this session is called `name`.
    pub fn resize_window(
        &mut self,
        name: &str,
        size: Option<(u16, u16)>,
    ) -> Result<(), SessionError> {
        let window = self
            .windows
            .iter_mut()
            .find(|w| w.name == name)
            .ok_or_else(|| SessionError::Unknown(name.to_owned()))?;
        window.set_manual_size(size);
        Ok(())
    }

    /// The index of the window whose pool holds `pane`, or `None` if no window of this session
    /// does — how [`break_pane`](Self::break_pane) / [`join_pane`](Self::join_pane) find a pane's
    /// SOURCE window from its id ALONE.
    ///
    /// A [`PaneId`] is unique across the whole registry (the module's load-bearing invariant), so
    /// at most one window holds it and the answer is unambiguous — the caller never has to name
    /// the source window, and cannot mis-name it (tmux requires `-s src-window.pane`; the unique
    /// id makes the window part redundant). Scans each window's pool under its own lock, released
    /// before the next — one lock at a time, registry-then-workspace order.
    fn window_index_of_pane(&self, pane: PaneId) -> Option<usize> {
        self.windows.iter().position(|w| {
            w.workspace()
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .pane(pane)
                .is_some()
        })
    }

    /// Break `pane` out of the window that holds it into a NEW window, select the new window, and
    /// return its name — tmux `break-pane`.
    ///
    /// The pane is MOVED whole (its PTY, emulator, scrollback, and running program ride along —
    /// see [`Workspace::adopt`](crate::Workspace::adopt)); nothing is re-spawned. The new window's
    /// pool siblings off the source's, so the moved pane's id stays unique across the registry.
    ///
    /// The SOURCE window is derived from `pane` alone (the window whose pool holds it — a
    /// [`PaneId`] is registry-unique, so at most one does), so there is no window arg to
    /// mis-name. `new_name` is the caller's choice for the new window; `None` allocates the lowest
    /// free integer window name (as [`new_window`](Self::new_window) does), the way tmux's
    /// `break-pane` with no `-n` picks the next index.
    ///
    /// Every check runs BEFORE the pane leaves its pool, so a refusal moves nothing.
    ///
    /// # Errors
    ///
    /// [`PaneMoveError::UnknownPane`] if no window of the session holds `pane`;
    /// [`PaneMoveError::DuplicateWindow`] if an explicit `new_name` is taken;
    /// [`PaneMoveError::LastPane`] if the source window tiles only that one pane (breaking it would
    /// just rename the window — tmux refuses the same).
    pub fn break_pane(
        &mut self,
        pane: PaneId,
        new_name: Option<&str>,
    ) -> Result<String, PaneMoveError> {
        let widx = self
            .window_index_of_pane(pane)
            .ok_or(PaneMoveError::UnknownPane(pane))?;
        // Resolve the new window name (and reject a duplicate) BEFORE touching the pane.
        let name = match new_name {
            Some(n) => {
                if self.windows.iter().any(|w| w.name == n) {
                    return Err(PaneMoveError::DuplicateWindow(n.to_owned()));
                }
                n.to_owned()
            }
            None => self.lowest_free_window_name(),
        };
        // Take the pane out and mint the new window's pool under ONE source-pool lock, with the
        // last-pane guard checked first so a refusal leaves the pool untouched. Membership is
        // already known (window_index_of_pane found it in this pool).
        let src_ws = Arc::clone(self.windows[widx].workspace());
        let (taken, mut new_pool) = {
            let mut pool = src_ws.lock().unwrap_or_else(PoisonError::into_inner);
            if pool.panes().len() <= 1 {
                return Err(PaneMoveError::LastPane);
            }
            let taken = pool
                .close(pane)
                .expect("window_index_of_pane found it in this pool");
            let new_pool = pool.sibling();
            (taken, new_pool)
        };
        // The new window is born ALREADY holding the moved pane; heal its tree to the single leaf
        // and select it (tmux's break-pane makes the new window current).
        new_pool.adopt(taken);
        let mut win = Window::new(&name, new_pool, self.mint_window());
        win.reconcile_own();
        self.windows.push(win);
        self.current_window = self.windows.len() - 1;
        // The source window lost a leaf: heal its tree (prunes the gone pane, bumps its revision).
        self.windows[widx].reconcile_own();
        Ok(name)
    }

    /// Move `pane` into the window named `dst` of THIS session, appending it as a new tiled leaf —
    /// tmux `join-pane`. Returns whether the SOURCE window was CLOSED (it is when the join emptied
    /// it).
    ///
    /// The pane is MOVED whole, as in [`break_pane`](Self::break_pane), and its SOURCE window is
    /// derived from its id (the window whose pool holds it) — the caller names only the
    /// destination. Placement is the arrangement's append (the destination's
    /// [`reconcile_layout`](Window::reconcile_layout) folds the new leaf in); a client that wants
    /// it at a specific split drops it there and writes the tree ([`Window::set_layout`]), the same
    /// "a gesture outranks a default" rule floating uses.
    ///
    /// A join that empties the source window CLOSES it (tmux's behaviour). The destination is a
    /// DIFFERENT window of this session, so at least two windows exist and removing the emptied
    /// source always leaves the session with at least one — [`current_window`](Self::current_window)
    /// is kept valid and, if it WAS the closed source, moved to the neighbour that takes its place.
    ///
    /// Every check runs BEFORE the pane leaves its pool, so a refusal moves nothing.
    ///
    /// # Errors
    ///
    /// [`PaneMoveError::UnknownWindow`] if the session has no window named `dst`;
    /// [`PaneMoveError::UnknownPane`] if no window of the session holds `pane`;
    /// [`PaneMoveError::SameWindow`] if `pane` already lives in `dst` (a no-op move).
    pub fn join_pane(&mut self, pane: PaneId, dst: &str) -> Result<bool, PaneMoveError> {
        let dst_idx = self
            .windows
            .iter()
            .position(|w| w.name == dst)
            .ok_or_else(|| PaneMoveError::UnknownWindow(dst.to_owned()))?;
        let src_idx = self
            .window_index_of_pane(pane)
            .ok_or(PaneMoveError::UnknownPane(pane))?;
        if src_idx == dst_idx {
            return Err(PaneMoveError::SameWindow(
                self.windows[dst_idx].name.clone(),
            ));
        }
        let src_ws = Arc::clone(self.windows[src_idx].workspace());
        let dst_ws = Arc::clone(self.windows[dst_idx].workspace());
        // Take from the source, then adopt into the destination under a SEPARATE lock — never both
        // pools held at once. Membership is known (window_index_of_pane found it in the source).
        let taken = src_ws
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .close(pane)
            .expect("window_index_of_pane found it in this pool");
        dst_ws
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .adopt(taken);
        self.windows[dst_idx].reconcile_own();
        // tmux closes a source window a join emptied.
        let src_empty = src_ws
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .panes()
            .is_empty();
        if src_empty {
            self.windows.remove(src_idx);
            if self.current_window > src_idx {
                self.current_window -= 1;
            } else if self.current_window == src_idx {
                self.current_window = src_idx.min(self.windows.len() - 1);
            }
            Ok(true)
        } else {
            self.windows[src_idx].reconcile_own();
            Ok(false)
        }
    }

    /// Put `pane` beside `target`, in the half on `side` along `dir` — tmux `move-pane`. Returns
    /// whether the move CLOSED the source window (it does when the move emptied it).
    ///
    /// **Neither window is named**, and that is the design. A [`PaneId`] is registry-unique, so
    /// `pane` implies its source window and `target` implies its destination — the rule
    /// [`break_pane`](Self::break_pane) already applies to the source, applied to both ends. The
    /// same call therefore re-places a pane inside its own window and moves it into another one,
    /// with no mode flag and no second verb: whether the two windows differ is an OBSERVATION
    /// about the two ids, never a choice the caller has to spell.
    ///
    /// This is to [`join_pane`](Self::join_pane) what a directional split is to a spawn — the same
    /// move with a PLACE. `join_pane` remains the verb for a caller who knows the destination
    /// window but not what is in it, and appends there; the two cover the two things a caller can
    /// actually know, with nothing in between them.
    ///
    /// Every check runs BEFORE the pane leaves its pool, so a refusal moves nothing.
    ///
    /// # Errors
    ///
    /// [`PaneMoveError::UnknownPane`] if no window of the session holds `pane` or `target`, or if
    /// the destination window does not TILE `target` (it is floating there) — the placement rule
    /// [`Window::place_pane`] states: a move that cannot reach its target must not quietly become
    /// an append. [`PaneMoveError::SamePane`] if `pane` IS `target`.
    pub fn move_pane(
        &mut self,
        pane: PaneId,
        target: PaneId,
        side: SplitSide,
        dir: SplitDir,
    ) -> Result<bool, PaneMoveError> {
        if pane == target {
            return Err(PaneMoveError::SamePane(pane));
        }
        let src_idx = self
            .window_index_of_pane(pane)
            .ok_or(PaneMoveError::UnknownPane(pane))?;
        let dst_idx = self
            .window_index_of_pane(target)
            .ok_or(PaneMoveError::UnknownPane(target))?;
        // The target must be TILED where it lives, checked before anything moves.
        if !self.windows[dst_idx].tiles(target) {
            return Err(PaneMoveError::UnknownPane(target));
        }
        if src_idx == dst_idx {
            let panes = self.windows[dst_idx].pane_ids();
            self.windows[dst_idx].place_pane(pane, target, side, dir, &panes);
            return Ok(false);
        }
        let src_ws = Arc::clone(self.windows[src_idx].workspace());
        let dst_ws = Arc::clone(self.windows[dst_idx].workspace());
        // Take from the source, then adopt into the destination under a SEPARATE lock — never both
        // pools held at once, the discipline `join_pane` keeps.
        let taken = src_ws
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .close(pane)
            .expect("window_index_of_pane found it in this pool");
        dst_ws
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .adopt(taken);
        let panes = self.windows[dst_idx].pane_ids();
        self.windows[dst_idx].place_pane(pane, target, side, dir, &panes);
        // tmux closes a source window a move emptied — `join_pane`'s rule, and reached the same
        // way: the destination is a DIFFERENT window of this session, so removing the emptied
        // source always leaves at least one behind.
        let src_empty = src_ws
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .panes()
            .is_empty();
        if src_empty {
            self.windows.remove(src_idx);
            if self.current_window > src_idx {
                self.current_window -= 1;
            } else if self.current_window == src_idx {
                self.current_window = src_idx.min(self.windows.len() - 1);
            }
            Ok(true)
        } else {
            self.windows[src_idx].reconcile_own();
            Ok(false)
        }
    }

    /// Where a step from `pane` in `dir` lands, IN THE WINDOW THAT HOLDS IT — the direction half of
    /// [`swap_panes`](Self::swap_panes), and the reason it is here rather than on the host's
    /// scope-bound reader.
    ///
    /// A pane's neighbour is a well-defined question wherever the pane lives, and this verb accepts
    /// a pane the caller named rather than the one a client is looking at. Resolving it against the
    /// SCOPED window would answer "no neighbour" for a pane in another window — which is not a
    /// conservative answer but a false one, since there plainly is something to its left. So the
    /// window is derived from the pane id, the same rule the two move verbs follow at both ends.
    ///
    /// Reconciles that window first, so a pane that has just spawned is a candidate and one that has
    /// just exited is not.
    ///
    /// **The outer [`None`] and the inner [`PaneStep::Untiled`] are different facts, and the whole
    /// reason this answers a [`PaneStep`] rather than an [`Option<PaneId>`]**: `None` means NO
    /// WINDOW OF THIS SESSION HOLDS THE PANE — a caller's mistake, which its verb refuses — while
    /// `Untiled` means this session holds it and the arrangement does not, so there is no adjacency
    /// to walk. Collapsed into one `Option`, an id naming nothing at all reads exactly like a
    /// floating pane and exactly like an edge, and the verb above answered "nothing that way" to all
    /// three ([`crate::layout::PaneStep`] states the same rule one level down).
    pub fn step_of(&mut self, pane: PaneId, dir: PaneDir) -> Option<PaneStep> {
        let idx = self.window_index_of_pane(pane)?;
        self.windows[idx].reconcile_own();
        Some(self.windows[idx].layout().step(pane, dir))
    }

    /// Fill the window that HOLDS `pane` with it alone, or end that window's zoom — tmux
    /// `resize-pane -Z`. Answers whether that window is zoomed after the call, `None` for a pane no
    /// window of this session holds.
    ///
    /// The window is DERIVED from the pane for [`step_of`](Self::step_of)'s reason and by
    /// the rule both move verbs follow at both ends: a [`PaneId`] is registry-unique, so the caller
    /// never has to name a window, and zooming a pane of a window nobody is looking at is a
    /// well-formed request rather than one to refuse. herdr's `pane.zoom` takes a tab-scoped target
    /// and a per-tab flag, which cannot express it at all.
    ///
    /// Reconciles that window first, for [`step_of`](Self::step_of)'s reason: a pane
    /// spawned since anyone last read is a candidate, and one that has exited is not.
    pub fn zoom_pane(&mut self, pane: PaneId, on: Option<bool>) -> Option<ZoomOutcome> {
        let idx = self.window_index_of_pane(pane)?;
        self.windows[idx].reconcile_own();
        let panes = self.windows[idx].pane_ids();
        self.windows[idx].zoom_pane(pane, on, &panes)
    }

    /// Exchange the POSITIONS of `a` and `b` — tmux `swap-pane`. Returns whether anything moved.
    ///
    /// Within ONE window this is a leaf exchange ([`LayoutTree::swap_panes`]) and every division
    /// keeps its id, direction and ratio by construction. ACROSS two windows the panes trade pools
    /// as well as places, which herdr refuses outright (`PaneSwapReason::CrossTab`) and tmux
    /// allows; sprag allows it because [`move_pane`](Self::move_pane) already crosses a window, and
    /// a swap that could not would be the same asymmetry in the other verb.
    ///
    /// The cross-window case is built from the float path's capture: each pane's
    /// [`LeafHome`] is read while it still holds its leaf, and the other pane is inserted at it.
    /// The two homes name siblings in DIFFERENT trees, so removing one cannot invalidate the
    /// other — which is exactly why this is not two [`move_pane`](Self::move_pane) calls. A window
    /// holding only the swapped pane has no home to give ([`LayoutTree::leaf_home`] answers `None`
    /// for the sole leaf) and the incoming pane becomes its sole leaf, which is the same
    /// arrangement by a shorter route.
    ///
    /// **The ACTIVE pane follows the pane within a window and the POSITION across two.** Within
    /// one window nothing has to happen: `active` is a [`PaneId`], and the pane the user was on is
    /// still in the window, merely elsewhere — tmux and herdr both keep the user on the pane they
    /// swapped. Across two, the window whose active pane just LEFT cannot follow it, so it follows
    /// the cell instead and lands on the arriving pane. That is a strictly better answer than
    /// [`Window::reconcile_layout`]'s closed-pane successor (a neighbour), and it is available only
    /// here, because only a swap knows what took the place.
    ///
    /// # Errors
    ///
    /// [`PaneMoveError::UnknownPane`] if no window of the session holds one of them, or if the
    /// window that holds it does not TILE it. `a == b` is NOT an error: it is a well-formed request
    /// that requires no motion, answered `false` like a direction with no neighbour.
    pub fn swap_panes(&mut self, a: PaneId, b: PaneId) -> Result<bool, PaneMoveError> {
        if a == b {
            return Ok(false);
        }
        let a_idx = self
            .window_index_of_pane(a)
            .ok_or(PaneMoveError::UnknownPane(a))?;
        let b_idx = self
            .window_index_of_pane(b)
            .ok_or(PaneMoveError::UnknownPane(b))?;
        for (idx, pane) in [(a_idx, a), (b_idx, b)] {
            if !self.windows[idx].tiles(pane) {
                return Err(PaneMoveError::UnknownPane(pane));
            }
        }
        if a_idx == b_idx {
            return Ok(self.windows[a_idx].swap_tiled(a, b));
        }
        // Settle BOTH trees before reading a home off either: a home is a fact about where a leaf
        // sits, and a pane arranged only in the pool has no leaf to describe yet.
        self.windows[a_idx].reconcile_own();
        self.windows[b_idx].reconcile_own();
        // Capture BOTH homes before either leaf collapses — the moment they exist, exactly as the
        // float path reads one. Different trees, so neither capture disturbs the other.
        let (home_a, home_b) = (
            self.windows[a_idx].layout().leaf_home(a),
            self.windows[b_idx].layout().leaf_home(b),
        );
        let a_ws = Arc::clone(self.windows[a_idx].workspace());
        let b_ws = Arc::clone(self.windows[b_idx].workspace());
        // Take both out, then adopt both in: one pool lock at a time, never nested.
        let taken_a = a_ws
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .close(a)
            .expect("window_index_of_pane found it in this pool");
        let taken_b = b_ws
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .close(b)
            .expect("window_index_of_pane found it in this pool");
        a_ws.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .adopt(taken_b);
        b_ws.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .adopt(taken_a);
        self.windows[a_idx].adopt_at(b, home_a, a);
        self.windows[b_idx].adopt_at(a, home_b, b);
        Ok(true)
    }
}

/// The durable server's whole state: every [`Session`].
///
/// The default pane size is NOT held here — each window's [`Workspace`] owns it, and that
/// is the only copy production reads, so there is nothing to drift.
///
/// The SINGLE global [`PaneId`] counter is not held here separately — it
/// lives with the thing it counts, shared (`Arc`) by every window's [`Workspace`] and
/// seeded once at [`new`](Self::new). The [`new_window`](Session::new_window) /
/// [`new_session`](Self::new_session) paths clone it out of an existing window's workspace, so
/// there is no duplicated handle to keep in sync.
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
/// switching is purely a client-side change — it sends a different name. The default (the only
/// scope not named by the caller) is `sessions[0]`, and it is no longer immutable:
/// [`kill_session`](Self::kill_session) can remove the first session, which re-points the
/// default at the next one. That is the honest consequence of a removal path, not a maintained
/// pointer — the list order IS the default (see [`default_session`](Self::default_session)).
pub struct SessionRegistry {
    /// Never EMPTY, though it can shrink: [`new`](Self::new) seeds one, and
    /// [`kill_session`](Self::kill_session) removes a non-last session but DRAINS (rather than
    /// removes) the last — so at least one always remains, which is what makes
    /// [`default_session`](Self::default_session) total.
    sessions: Vec<Session>,
    /// The ONE source of [`SessionId`]s and [`WindowId`]s for everything this registry holds —
    /// cloned into each [`Session`] so a window born inside one mints from it too.
    ///
    /// Its counterpart for panes lives on the [`Workspace`] and is shared the same way (the
    /// module's load-bearing invariant); this is that invariant applied to the two shapes that
    /// had no identity at all.
    ids: Arc<AtomicU64>,
    /// How many BIRTHS are in flight — sessions (or a whole restored registry) that exist here
    /// while the panes that populate them do not exist yet. See [`pin_birth`](Self::pin_birth).
    births: usize,
}

impl SessionRegistry {
    /// A registry with one empty session (`"0"`) holding one empty window (`"0"`) — the
    /// behaviour-preserving boot state that mirrors the single [`Workspace`] the host
    /// owned before this layer existed. The boot window's workspace is seeded with a
    /// fresh global id counter (which later windows will share).
    #[must_use]
    pub fn new(default_size: (u16, u16)) -> Self {
        let ids = Arc::new(AtomicU64::new(0));
        Self {
            sessions: vec![Session::new(
                "0",
                Workspace::new(default_size),
                Arc::clone(&ids),
            )],
            ids,
            births: 0,
        }
    }

    /// Claim that a BIRTH is in flight: a session exists here whose first pane does not exist yet.
    ///
    /// The claim answers a question the registry alone can answer and the pane pools cannot. A
    /// daemon's life is tied to its LIVE PANES ("zero live panes ⇒ exit"), and an empty session
    /// holds none — vacuously so. That is the right reading at rest and the wrong one mid-create:
    /// between a session being made and its shell being spawned, an unrelated last pane dying
    /// elsewhere finds every pool empty and ends the daemon under the very client that asked for
    /// the session. The claim says "one is coming", and only the registry knows it, because the
    /// pane that would prove it has not been born.
    ///
    /// It must be taken under the SAME lock as the create it covers. A claim taken after that lock
    /// is released leaves exactly the gap it exists to close, only narrower.
    ///
    /// Counted rather than boolean: two clients may be creating at once, and a shared flag would
    /// let the first one to finish drop the second one's claim. Paired with
    /// [`release_birth`](Self::release_birth) — the host wraps the pair in a guard so a birth that
    /// FAILS releases it too, since a claim nothing ever drops is a daemon that never exits.
    pub fn pin_birth(&mut self) {
        self.births += 1;
    }

    /// Release one claim taken by [`pin_birth`](Self::pin_birth).
    ///
    /// Saturating, so an unbalanced release can only ever under-claim (a daemon that exits when it
    /// should) rather than wrap to a permanent claim (a daemon that never does). Neither is
    /// reachable through the host's guard; the saturation is what keeps the failure mode the
    /// recoverable one if some future caller pairs them by hand.
    pub fn release_birth(&mut self) {
        self.births = self.births.saturating_sub(1);
    }

    /// Whether any birth claimed by [`pin_birth`](Self::pin_birth) is still in flight — read by
    /// the daemon's reaper before it concludes that nothing is live.
    #[must_use]
    pub fn birth_in_flight(&self) -> bool {
        self.births > 0
    }

    /// Rebuild a registry's STRUCTURE from a durability [`Snapshot`], returning it paired with the
    /// [`RestorePlan`] of panes the caller must re-spawn.
    ///
    /// Pinion-free and PANE-FREE: the sessions, windows, layout trees, float sets and the seeded
    /// id counter are all rebuilt here, but the pools are EMPTY — a pane is born at the HOST so it
    /// carries the daemon's death-signal (the D4 seam this crate does not hold). The plan names,
    /// per pane, the window it docks into and the facts to spawn its shell with; the host spawns
    /// each under its old id ([`Workspace::spawn_restored`](crate::Workspace)) so the trees,
    /// already referencing those ids, resolve, and the first reconcile heals any that fail to spawn.
    ///
    /// Every pool shares ONE id counter seeded to the snapshot's high-water mark, so a restore
    /// never reissues a retired id — even a gap left by a pane closed pre-reboot.
    ///
    /// # Errors
    ///
    /// [`SnapshotError`] — and the caller boots EMPTY rather than corrupt — if the version is
    /// unsupported, the shape is malformed (no sessions, a session with no windows, a
    /// `current_window` naming no window, or a duplicate session/window name), or a stored layout
    /// is not well-formed. A bad snapshot never bricks the daemon.
    pub fn from_snapshot(snapshot: Snapshot) -> Result<(Self, RestorePlan), SnapshotError> {
        // A RANGE, not an equality: this build restores every format back to
        // `MIN_READABLE_SNAPSHOT_VERSION` and rewrites the file at `SNAPSHOT_VERSION` on the next
        // save, so a user's sessions migrate by being loaded rather than being thrown away once.
        // The upper bound still bites — a snapshot from a NEWER build is refused by name.
        if !(MIN_READABLE_SNAPSHOT_VERSION..=SNAPSHOT_VERSION).contains(&snapshot.version) {
            return Err(SnapshotError::Version {
                found: snapshot.version,
                expected: SNAPSHOT_VERSION,
            });
        }
        if snapshot.sessions.is_empty() {
            return Err(SnapshotError::Malformed("no sessions".to_owned()));
        }
        // One counter for the whole registry, seeded to the stored mark; every window's pool
        // siblings off this seed (which is itself only a counter holder — never a live pool).
        let seed = Workspace::with_seeded_counter(snapshot.default_size, snapshot.next_id);
        // A FRESH identity counter: ids are within-run and are not in the file ([`SessionId`]).
        let ids = Arc::new(AtomicU64::new(0));
        let mut sessions = Vec::with_capacity(snapshot.sessions.len());
        let mut plan = Vec::new();
        let mut seen_sessions = HashSet::new();
        // A PaneId is unique across the WHOLE registry (the load-bearing invariant), so a snapshot
        // with two panes claiming one id is malformed. sprag's own writer cannot produce this
        // (`snapshot()` reads ids unique by construction), but a hand-edited state file could — and
        // `spawn_restored` would push both, leaving two live panes sharing an id that
        // id-addressed reads then resolve ambiguously. Reject it so the fail-safe holds: a corrupt
        // snapshot boots EMPTY, never into an id-colliding registry.
        let mut seen_panes = HashSet::new();
        // And a pane NAME is unique registry-wide for the same reason (it stands in for the id), so
        // the same fail-safe applies to it. Kept as a separate set rather than folded into the id
        // check because the two say different things about a corrupt file, and an operator reading
        // the refusal should learn which invariant the file broke.
        let mut seen_names: HashSet<crate::PaneName> = HashSet::new();
        for s in snapshot.sessions {
            if !seen_sessions.insert(s.name.clone()) {
                return Err(SnapshotError::Malformed(format!(
                    "duplicate session {:?}",
                    s.name
                )));
            }
            if s.windows.is_empty() {
                return Err(SnapshotError::Malformed(format!(
                    "session {:?} has no windows",
                    s.name
                )));
            }
            let mut windows = Vec::with_capacity(s.windows.len());
            let mut seen_windows = HashSet::new();
            for w in s.windows {
                if !seen_windows.insert(w.name.clone()) {
                    return Err(SnapshotError::Malformed(format!(
                        "session {:?} has duplicate window {:?}",
                        s.name, w.name
                    )));
                }
                // Record the panes to re-spawn before the window's fields are moved into it.
                for p in &w.panes {
                    if !seen_panes.insert(p.id) {
                        return Err(SnapshotError::Malformed(format!(
                            "pane id {} appears twice",
                            p.id
                        )));
                    }
                    // A NAME is registry-unique for the same reason an id is — it stands in for
                    // one — so a file carrying the same name twice is malformed on exactly the
                    // grounds above, and refused here rather than restored into a set where a
                    // caller's `--pane build` can no longer be answered.
                    if let Some(name) = &p.name
                        && !seen_names.insert(name.clone())
                    {
                        return Err(SnapshotError::Malformed(format!(
                            "pane name {name:?} appears twice"
                        )));
                    }
                    plan.push(PaneRestore {
                        session: s.name.clone(),
                        window: w.name.clone(),
                        id: p.id,
                        cwd: p.cwd.clone(),
                        argv: p.argv.clone(),
                        remote: p.remote.clone(),
                        opened_by: p.opened_by,
                        name: p.name.clone(),
                        cols: p.cols,
                        rows: p.rows,
                    });
                }
                let window = Window::restore(
                    seed.sibling(),
                    WindowId(ids.fetch_add(1, Ordering::Relaxed)),
                    w,
                )
                .map_err(|e| SnapshotError::Layout(e.to_string()))?;
                windows.push(window);
            }
            let current_window = windows
                .iter()
                .position(|win| win.name == s.current_window)
                .ok_or_else(|| {
                    SnapshotError::Malformed(format!(
                        "session {:?} current window {:?} names no window",
                        s.name, s.current_window
                    ))
                })?;
            sessions.push(Session {
                id: SessionId(ids.fetch_add(1, Ordering::Relaxed)),
                name: s.name,
                windows,
                current_window,
                ids: Arc::clone(&ids),
            });
        }
        // `births: 0` — a rebuilt registry claims nothing on its own. The restore that ADOPTS it
        // holds the claim across its re-spawn loop, because it is the caller that knows the plan's
        // panes are still coming; this constructor only knows the shape.
        Ok((
            Self {
                sessions,
                ids,
                births: 0,
            },
            RestorePlan { panes: plan },
        ))
    }

    /// All sessions, in creation order.
    #[must_use]
    pub fn sessions(&self) -> &[Session] {
        &self.sessions
    }

    /// Install `source` as the [`HistoryLimitSource`] every pane born from here on consults, across
    /// every window this registry currently holds.
    ///
    /// Applied to the whole registry rather than to one pool because a new window's workspace is a
    /// [`sibling`](Workspace::sibling) of an existing one and inherits the source from it — so
    /// seeding what is here now covers every window that will ever be made from it. A registry
    /// REPLACED wholesale (a restore) is a new set of pools and needs its own call, which is why
    /// this is a method and not a constructor argument.
    ///
    /// Takes `&self`: each pool is already behind its own lock, and nothing about the registry's own
    /// shape changes.
    pub fn set_history_limit_source(&self, source: HistoryLimitSource) {
        for session in self.sessions() {
            for window in session.windows() {
                window
                    .workspace()
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .set_history_limit_source(Arc::clone(&source));
            }
        }
    }

    /// Install `source` as the [`PaneEnvSource`] every pane born from here on consults, across every
    /// window this registry currently holds — the registry-wide counterpart of
    /// [`Workspace::set_pane_env_source`].
    ///
    /// Whole-registry and `&self` for [`set_history_limit_source`](Self::set_history_limit_source)'s
    /// reasons, and a REPLACED registry (a restore) needs its own call for the same reason: its
    /// pools are new, and panes coming back into them would otherwise be the only ones in the
    /// daemon born unable to name themselves.
    pub fn set_pane_env_source(&self, source: PaneEnvSource) {
        for session in self.sessions() {
            for window in session.windows() {
                window
                    .workspace()
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .set_pane_env_source(Arc::clone(&source));
            }
        }
    }

    /// A [`SessionInfo`] for every session, in creation order — the STRUCTURAL list a switcher
    /// draws, marking the DEFAULT (where an unscoped request lands). The ONE builder for the
    /// structural fields, so the wire `sessions` slot and the in-process arm cannot drift in what
    /// `name`/`windows`/`default` mean.
    ///
    /// [`panes`](SessionInfo::panes) is left `0` here: counting a session's panes needs every
    /// window's pool lock, which must NOT be taken under the registry lock this runs beneath (the
    /// module's registry-then-workspace, never-nested discipline).
    /// [`SessionRegistry::session_infos_live`] fills it off the lock.
    #[must_use]
    pub fn session_infos(&self) -> Vec<SessionInfo> {
        let default = self.default_session().name();
        self.sessions
            .iter()
            .map(|session| SessionInfo {
                name: session.name().to_owned(),
                windows: session.windows().len(),
                // Live count; the structural builder cannot take the pool locks it needs, so it
                // is 0 here and filled by `session_infos_live` (which locks the pools off the
                // registry lock). A registry-only list therefore reports every session paneless.
                panes: 0,
                default: session.name() == default,
                // The registry has no idea who is watching a session; the daemon fills this in
                // host-side ([`SessionInfo::attached`]). A registry-only list carries 0.
                attached: 0,
            })
            .collect()
    }

    /// The [`session_infos`](Self::session_infos) list with each session's live
    /// [`pane count`](SessionInfo::panes) filled in — the registry-wide read the wire `sessions`
    /// slot and the in-process arm both call, so neither the shape nor what a field means can drift
    /// between them.
    ///
    /// TWO-PHASE, exactly like [`snapshot`](crate::snapshot::snapshot), so the registry lock and a
    /// workspace lock are held SEQUENTIALLY, never nested (the module's registry-then-workspace
    /// discipline):
    ///  1. under the registry lock: the structural infos, plus (in the SAME pass, so both Vecs share
    ///     the session order) all of each session's windows' pool `Arc`s;
    ///  2. lock RELEASED — every window pool locked on its OWN to count its panes.
    ///
    /// # What this deliberately does NOT read (R282)
    ///
    /// No filesystem, and no `/proc`. Where a session is working, on what branch, and what it is
    /// serving used to be filled here, which made this — the question a display client asks on every
    /// poll wake, and a wake is a batch of PTY output — cost a `/proc` walk of every process on the
    /// box. Those three are sampled by [`ActivitySampler`](crate::ActivitySampler) now, asked for
    /// separately with the staleness the caller will accept. This answers only what the registry and
    /// its pools already know, which is why it can be asked as often as anyone likes.
    #[must_use]
    pub fn session_infos_live(registry: &Arc<Mutex<Self>>) -> Vec<SessionInfo> {
        // Phase 1 — registry lock ONLY: the structural infos and ALL of each session's windows'
        // pools, in ONE pass so entry `i` of both Vecs names the same session.
        let (mut infos, window_pools) = {
            let reg = registry.lock().unwrap_or_else(PoisonError::into_inner);
            let infos = reg.session_infos();
            let windows = reg.window_pools();
            (infos, windows)
        };

        // Phase 2 (each pool under its OWN lock, never nested with the registry): each session's
        // live pane count across ALL its windows — the signal that tells a resting empty anchor (0)
        // from a working session (see [`SessionInfo::is_listable`]).
        for (info, pools) in infos.iter_mut().zip(&window_pools) {
            info.panes = Self::window_pane_count(pools);
        }
        infos
    }

    /// Every session's windows' pane pools, in registry order — the handles a caller needs to read
    /// pane state OFF the registry lock (the module's registry-then-workspace discipline).
    ///
    /// Taken under the registry lock by whoever holds it and used after releasing it. Shared by
    /// [`session_infos_live`](Self::session_infos_live) and the
    /// [`ActivitySampler`](crate::ActivitySampler), so the two cannot come to disagree about which
    /// pools belong to which session.
    #[must_use]
    pub(crate) fn window_pools(&self) -> Vec<Vec<Arc<Mutex<Workspace>>>> {
        self.sessions
            .iter()
            .map(|session| {
                session
                    .windows()
                    .iter()
                    .map(|window| Arc::clone(window.workspace()))
                    .collect()
            })
            .collect()
    }

    /// Every session's NAME and its current window's pane pool, in registry order — what the
    /// [`ActivitySampler`](crate::ActivitySampler) needs from the registry, taken under its lock in
    /// the same pass as [`window_pools`](Self::window_pools) so the two Vecs share the session order.
    ///
    /// The CURRENT window's pool, because a session's working directory is the one a reader would
    /// see on attach; the pane chosen within it is the sampler's decision, not this one's.
    #[must_use]
    pub(crate) fn current_pools(&self) -> (Vec<String>, Vec<Arc<Mutex<Workspace>>>) {
        self.sessions
            .iter()
            .map(|session| {
                (
                    session.name().to_owned(),
                    Arc::clone(session.current_window().workspace()),
                )
            })
            .unzip()
    }

    /// The child pids of every pane across `pools` (a session's windows), each pool locked on its
    /// OWN — never nested with the registry lock (the module's registry-then-workspace discipline;
    /// [`session_infos_live`](Self::session_infos_live) runs this only after releasing it). These are
    /// the roots [`ProcScan::listening_ports`](crate::ports::ProcScan::listening_ports) walks: a
    /// session's listening servers live in the pane process subtrees, not the pane pids themselves.
    ///
    /// Only pids of STILL-POOLED panes are read here, and every pid returned belongs to a child that
    /// has not yet been waited — so it is live or a zombie, never recycled to an unrelated process,
    /// and the `/proc` fd walk cannot stray into a foreign process's sockets.
    ///
    /// A still-pooled pane IS reaped in place now (its reader thread waits the moment the child's
    /// output ends, so an exit status can be reported), which is exactly the case the earlier note
    /// here warned would break the property. It does not, because the reap and the withholding are
    /// the same event: [`PanePty::pid`](crate::pane_pty::PanePty::pid) answers `None` from the
    /// instant the status is published, so a reaped pid never reaches this filter.
    pub(crate) fn window_pids(pools: &[Arc<Mutex<Workspace>>]) -> Vec<u32> {
        pools
            .iter()
            .flat_map(|pool| {
                let pool = pool.lock().unwrap_or_else(PoisonError::into_inner);
                pool.panes()
                    .iter()
                    .filter_map(|pane| pane.pty().pid())
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// The total pane count across `pools` (a session's windows) — the STRUCTURAL count (every
    /// pooled pane, whether or not its child still has a live pid), so a session whose processes
    /// have exited but whose panes remain still reads as non-empty. Each pool locked on its OWN,
    /// never nested with the registry lock, exactly like [`window_pids`](Self::window_pids).
    fn window_pane_count(pools: &[Arc<Mutex<Workspace>>]) -> usize {
        pools
            .iter()
            .map(|pool| {
                pool.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .panes()
                    .len()
            })
            .sum()
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

    /// Create a session, holding one empty window, and return the name it got.
    ///
    /// `name` is the caller's choice; `None` asks the registry to ALLOCATE the lowest free
    /// name, the way tmux's `new-session` with no `-s` does. Allocation belongs here rather
    /// than in the caller for the reason [`session`](Self::session) gives about an index
    /// supplied from outside: a client that invents a name and retries on
    /// [`Duplicate`](SessionError::Duplicate) is doing check-then-act against a namespace it
    /// does not own, and two such clients race. Here the check and the act are one, under the
    /// one lock that owns the namespace — the same reason nothing else in this type is
    /// addressed by a caller-chosen index.
    ///
    /// The returned name is what the caller scopes its next request with — indispensable for
    /// the allocated case (the caller did not choose it), and harmlessly the same string back
    /// for the explicit one.
    ///
    /// Its pane pool clones the id counter out of a pool that already exists, so ids stay
    /// unique across the WHOLE registry (the module's load-bearing invariant) with no second
    /// home to keep in step. Size is inherited from the default session's pool, which is the
    /// only copy production reads.
    ///
    /// Does NOT change any other client's scope: creating and attaching are separate acts,
    /// and a client that creates a session for someone else must not yank the scope out from
    /// under whoever is attached now. Nothing here can — `new_session` APPENDS, so it never moves
    /// the default (`sessions[0]`); only [`kill_session`](Self::kill_session) of the default can,
    /// and every other client names its own scope.
    ///
    /// # Errors
    ///
    /// [`SessionError::Duplicate`] if an explicit `name` is already taken — a name is how a
    /// session is addressed, so two of them would make the address ambiguous and let one
    /// client's request silently land in another's session. The allocated path cannot fail:
    /// it picks a name that is free by construction.
    pub fn new_session(&mut self, name: Option<&str>) -> Result<String, SessionError> {
        let name = match name {
            // Parsed BEFORE the duplicate check, so a caller offering an unusable name is told
            // which rule it broke rather than that the name is free. See [`SessionName`] for the
            // grammar and for why an all-digit name is not one of the refusals — this method's own
            // allocation below mints exactly those.
            Some(name) => {
                let name = String::from(SessionName::parse(name).map_err(SessionError::Malformed)?);
                if self.session(&name).is_some() {
                    return Err(SessionError::Duplicate(name));
                }
                name
            }
            None => self.lowest_free_name(),
        };
        let seed = Arc::clone(self.default_session().current_window().workspace());
        let pool = seed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .sibling();
        self.sessions
            .push(Session::new(&name, pool, Arc::clone(&self.ids)));
        Ok(name)
    }

    /// The lowest non-negative integer name not currently in use, as a string.
    ///
    /// tmux allocates the same way (`new-session` with no `-s` picks the lowest free number).
    /// The boot session is `"0"`, so this returns `"1"` first; a session a user explicitly
    /// named `"3"` is stepped over, never handed out again while it lives.
    ///
    /// Total: at most `sessions.len()` names are taken, so at least one of the `len + 1`
    /// candidates in `0..=len` is free — the scan cannot run past the end.
    fn lowest_free_name(&self) -> String {
        (0u64..)
            .map(|n| n.to_string())
            .find(|candidate| self.session(candidate).is_none())
            .expect("some name in 0..=len is always free")
    }

    /// Kill the session named `name` — tmux `kill-session`.
    ///
    /// A NON-last session is REMOVED: its windows and their panes drop, closing every PTY master
    /// so the child shells receive SIGHUP, and the registry shrinks. If the removed one was the
    /// default (first) session, the next becomes the default — an unscoped request now lands
    /// there. That the default can MOVE is new: it was immutable only because nothing could
    /// remove a session; killing the one an unscoped request happens to land in re-points it,
    /// which is the honest consequence, not a bug (a client that wants a specific session names
    /// it).
    ///
    /// The LAST session is NOT removed but DRAINED (its panes closed), and
    /// [`KillOutcome::KilledServer`] is returned so the caller exits the daemon. Draining rather
    /// than removing is what keeps
    /// [`default_session`](Self::default_session) total: an empty registry still answering
    /// requests would leave the unscoped path unresolvable, and the daemon is about to exit
    /// anyway, so the emptied shell simply outlives the last request by the width of a shutdown.
    ///
    /// Both arms hand the reaped owners BACK in the [`KillOutcome`] so the caller drops them off
    /// the registry lock, rather than running their blocking `PanePty::Drop` (kill / wait / join)
    /// under it — the same discipline the `close` action keeps.
    ///
    /// # Errors
    ///
    /// [`SessionError::Unknown`] if no session carries `name`.
    pub fn kill_session(&mut self, name: &str) -> Result<KillOutcome, SessionError> {
        let idx = self
            .sessions
            .iter()
            .position(|session| session.name == name)
            .ok_or_else(|| SessionError::Unknown(name.to_owned()))?;
        if self.sessions.len() == 1 {
            // The last session: drain it (no live pane remains, so the reaper exits the daemon)
            // but keep the shell so `default_session` stays total until the process dies.
            return Ok(KillOutcome::KilledServer(self.sessions[idx].drain_panes()));
        }
        // Removing the session takes its windows -> workspaces -> panes out of the registry; the
        // returned Session carries them so the caller drops it (SIGHUP + reader join) off-lock.
        Ok(KillOutcome::Removed(self.sessions.remove(idx)))
    }

    /// Rename the session named `name` to `new` — tmux `rename-session`.
    ///
    /// This MOVES AN ADDRESS, which is what makes it different from
    /// [`rename_window`](Self::rename_window) one level down and from
    /// a pane's rename beside it. A window is addressed inside its session and a
    /// pane by an id its name only stands in for; a SESSION name is what every `-t`, every scoped
    /// connection and every attached client holds. So the registry's half is only the first half:
    /// the daemon must also carry the session's change CHANNEL and its clients' ATTACHMENTS across
    /// with it, or a rename orphans everyone parked on the old name (`sprag_host::workspace`'s
    /// `rename_session` action is where that happens, and it is the caller of this).
    ///
    /// The IDENTITY does not move ([`Session::id`]), which is what lets the change funnel report
    /// this as one rename instead of a session dying and another being born.
    ///
    /// # Errors
    ///
    /// [`SessionError::Unknown`] if no session carries `name`; [`SessionError::Duplicate`] if `new`
    /// is another session's name — an address must stay unique or one client's request silently
    /// lands in another's session, which is the failure [`new_session`](Self::new_session) refuses a
    /// duplicate for. Renaming a session to the name it already has is a NO-OP rather than a
    /// duplicate, exactly as `rename_window` treats its own.
    pub fn rename_session(&mut self, name: &str, new: &str) -> Result<String, SessionError> {
        let idx = self
            .sessions
            .iter()
            .position(|session| session.name == name)
            .ok_or_else(|| SessionError::Unknown(name.to_owned()))?;
        // The grammar an ADDRESS has to satisfy, and the reason this verb had to bring it: until a
        // session could be renamed, a name entered the registry only at birth. MEASURED before it
        // was written — a rename accepted `""`, a newline that printed as two rows of `sprag ls`,
        // and an escape sequence that reached the reader's terminal.
        let new = String::from(SessionName::parse(new).map_err(SessionError::Malformed)?);
        if new != name && self.sessions.iter().any(|session| session.name == new) {
            return Err(SessionError::Duplicate(new));
        }
        self.sessions[idx].name.clone_from(&new);
        Ok(new)
    }

    /// What the session with this IDENTITY is called NOW, or `None` if it is gone.
    ///
    /// The lookup a caller holding a resolved scope makes after a dispatch that may have moved the
    /// name it resolved: see `sprag_host`'s derive site, which observes a session's changes at the
    /// address the session has rather than the one the request carried.
    #[must_use]
    pub fn name_of(&self, id: SessionId) -> Option<&str> {
        self.sessions
            .iter()
            .find(|session| session.id == id)
            .map(|session| session.name.as_str())
    }

    /// The session named `name`, mutably, or [`SessionError::Unknown`] — the resolution the
    /// window wrappers below share, so "no such session" is one refusal carrying its name.
    fn session_named_mut(&mut self, name: &str) -> Result<&mut Session, SessionError> {
        self.sessions
            .iter_mut()
            .find(|s| s.name == name)
            .ok_or_else(|| SessionError::Unknown(name.to_owned()))
    }

    /// Create a window in the session named `session`, select it, and return its name — the
    /// registry-level entry the wire handler uses (it resolves the session, then delegates to
    /// [`Session::new_window`]). The host births its first pane; see that primitive.
    ///
    /// # Errors
    ///
    /// [`SessionError::Unknown`] if no session carries `session`; [`SessionError::Duplicate`]
    /// if an explicit window `name` is already taken in it.
    pub fn new_window(
        &mut self,
        session: &str,
        name: Option<&str>,
    ) -> Result<String, SessionError> {
        self.session_named_mut(session)?.new_window(name)
    }

    /// Make the window named `name` current in the session named `session` — tmux
    /// `select-window`. See [`Session::select_window`].
    ///
    /// # Errors
    ///
    /// [`SessionError::Unknown`] for an unknown session OR window.
    pub fn select_window(&mut self, session: &str, name: &str) -> Result<(), SessionError> {
        self.session_named_mut(session)?.select_window(name)
    }

    /// Walk `session`'s windows one step, wrapping, and answer the name it landed on — tmux
    /// `next-window` / `previous-window`. See [`Session::select_window_relative`] for why this
    /// wraps where the pane walk does not, and why it cannot fail once the session resolves.
    ///
    /// # Errors
    ///
    /// [`SessionError::Unknown`] if no session carries `session` — the ONLY way this refuses.
    pub fn select_window_relative(
        &mut self,
        session: &str,
        step: WindowStep,
    ) -> Result<String, SessionError> {
        Ok(self
            .session_named_mut(session)?
            .select_window_relative(step)
            .to_owned())
    }

    /// Rename the window named `name` of the session named `session` to `new` — tmux
    /// `rename-window`. See [`Session::rename_window`].
    ///
    /// # Errors
    ///
    /// [`SessionError::Unknown`] for an unknown session OR window; [`SessionError::Duplicate`]
    /// if `new` is already another window's name.
    pub fn rename_window(
        &mut self,
        session: &str,
        name: &str,
        new: &str,
    ) -> Result<(), SessionError> {
        self.session_named_mut(session)?.rename_window(name, new)
    }

    /// Pin the size of the window named `name` of the session named `session`, or un-pin it with
    /// `None` — tmux `resize-window`. See [`Session::resize_window`].
    ///
    /// # Errors
    ///
    /// [`SessionError::Unknown`] for an unknown session OR window.
    pub fn resize_window(
        &mut self,
        session: &str,
        name: &str,
        size: Option<(u16, u16)>,
    ) -> Result<(), SessionError> {
        self.session_named_mut(session)?.resize_window(name, size)
    }

    /// Break `pane` out of the window that holds it, within the session named `session`, into a new
    /// window, returning its name — the registry-level entry the wire handler uses (resolve the
    /// session, then delegate to [`Session::break_pane`], which derives the pane's source window).
    /// tmux `break-pane`.
    ///
    /// # Errors
    ///
    /// [`PaneMoveError::UnknownSession`] if no session carries `session`; otherwise the refusals
    /// [`Session::break_pane`] gives.
    pub fn break_pane(
        &mut self,
        session: &str,
        pane: PaneId,
        new_name: Option<&str>,
    ) -> Result<String, PaneMoveError> {
        self.sessions
            .iter_mut()
            .find(|s| s.name == session)
            .ok_or_else(|| PaneMoveError::UnknownSession(session.to_owned()))?
            .break_pane(pane, new_name)
    }

    /// Move `pane` into the window named `dst` of the session named `session`, returning whether the
    /// source window was closed — the registry-level entry the wire handler uses ([`Session::join_pane`]
    /// derives the pane's source window). tmux `join-pane`.
    ///
    /// # Errors
    ///
    /// [`PaneMoveError::UnknownSession`] if no session carries `session`; otherwise the refusals
    /// [`Session::join_pane`] gives.
    pub fn join_pane(
        &mut self,
        session: &str,
        pane: PaneId,
        dst: &str,
    ) -> Result<bool, PaneMoveError> {
        self.sessions
            .iter_mut()
            .find(|s| s.name == session)
            .ok_or_else(|| PaneMoveError::UnknownSession(session.to_owned()))?
            .join_pane(pane, dst)
    }

    /// Put `pane` beside `target` in the session named `session`, returning whether the source
    /// window was closed — the registry-level entry the wire handler uses ([`Session::move_pane`]
    /// derives BOTH windows from the two pane ids). tmux `move-pane`.
    ///
    /// # Errors
    ///
    /// [`PaneMoveError::UnknownSession`] if no session carries `session`; otherwise the refusals
    /// [`Session::move_pane`] gives.
    pub fn move_pane(
        &mut self,
        session: &str,
        pane: PaneId,
        target: PaneId,
        side: SplitSide,
        dir: SplitDir,
    ) -> Result<bool, PaneMoveError> {
        self.sessions
            .iter_mut()
            .find(|s| s.name == session)
            .ok_or_else(|| PaneMoveError::UnknownSession(session.to_owned()))?
            .move_pane(pane, target, side, dir)
    }

    /// Where a step from `pane` in `dir` lands, in whichever window of `session` holds it — the
    /// registry-level entry the `swap_pane` handler's direction form uses. See
    /// [`Session::step_of`]. `None` for an unknown session or a pane it does not hold; the reason a
    /// held pane went nowhere is the [`PaneStep`] inside.
    pub fn step_of(&mut self, session: &str, pane: PaneId, dir: PaneDir) -> Option<PaneStep> {
        self.sessions
            .iter_mut()
            .find(|s| s.name == session)?
            .step_of(pane, dir)
    }

    /// Zoom `pane` — or un-zoom its window — in whichever window of `session` holds it: the
    /// registry-level entry the `zoom_pane` handler uses. See [`Session::zoom_pane`]. `None` for an
    /// unknown session or a pane no window of it holds.
    pub fn zoom_pane(
        &mut self,
        session: &str,
        pane: PaneId,
        on: Option<bool>,
    ) -> Option<ZoomOutcome> {
        self.sessions
            .iter_mut()
            .find(|s| s.name == session)?
            .zoom_pane(pane, on)
    }

    /// Exchange the positions of `a` and `b` in the session named `session`, returning whether
    /// anything moved — the registry-level entry the wire handler uses ([`Session::swap_panes`]
    /// derives both windows). tmux `swap-pane`.
    ///
    /// # Errors
    ///
    /// [`PaneMoveError::UnknownSession`] if no session carries `session`; otherwise the refusals
    /// [`Session::swap_panes`] gives.
    pub fn swap_panes(
        &mut self,
        session: &str,
        a: PaneId,
        b: PaneId,
    ) -> Result<bool, PaneMoveError> {
        self.sessions
            .iter_mut()
            .find(|s| s.name == session)
            .ok_or_else(|| PaneMoveError::UnknownSession(session.to_owned()))?
            .swap_panes(a, b)
    }

    /// Kill the window named `window` of the session named `session` — tmux `kill-window`.
    ///
    /// A NON-last window is removed and its panes drained ([`WindowKillOutcome::Removed`]),
    /// which keeps the session's [`current_window`](Session::current_window) valid and, tmux-like,
    /// on the window that took the killed one's place (the next; the previous if the last was
    /// killed). The LAST window of a session cannot be removed without emptying it, and tmux ends
    /// the session with its last window — so this delegates to [`kill_session`](Self::kill_session)
    /// and reports [`WindowKillOutcome::Session`], which also folds in the last-SESSION case
    /// (draining the panes and ending the daemon).
    ///
    /// The reaped panes ride back in the outcome so the caller drops them off the registry lock,
    /// the same discipline [`kill_session`](Self::kill_session) keeps.
    ///
    /// # Errors
    ///
    /// [`SessionError::Unknown`] carrying the session name if none exists, or the window name if
    /// the session has no such window.
    pub fn kill_window(
        &mut self,
        session: &str,
        window: &str,
    ) -> Result<WindowKillOutcome, SessionError> {
        let sidx = self
            .sessions
            .iter()
            .position(|s| s.name == session)
            .ok_or_else(|| SessionError::Unknown(session.to_owned()))?;
        let widx = self.sessions[sidx]
            .windows
            .iter()
            .position(|w| w.name == window)
            .ok_or_else(|| SessionError::Unknown(window.to_owned()))?;
        if self.sessions[sidx].windows.len() == 1 {
            // The session's last window: tmux ends the session with it. Escalating also handles
            // the last-SESSION case (drain + end the daemon) in one place — this never removes a
            // window such that the session is left with zero.
            return Ok(WindowKillOutcome::Session(self.kill_session(session)?));
        }
        let sess = &mut self.sessions[sidx];
        // Drain BEFORE removing, so the returned panes' blocking Drop runs off-lock (the caller
        // drops the Vec); the emptied window then drops with nothing left to tear down.
        let reaped = sess.windows[widx].drain();
        sess.windows.remove(widx);
        // Keep current_window in range and on the neighbour that takes the killed window's place.
        if sess.current_window > widx {
            sess.current_window -= 1;
        } else if sess.current_window == widx {
            sess.current_window = widx.min(sess.windows.len() - 1);
        }
        Ok(WindowKillOutcome::Removed(reaped))
    }

    /// The session an UNSCOPED request acts on — the first in the list.
    ///
    /// Total: `sessions` is seeded non-empty and NEVER becomes empty — [`kill_session`] removes
    /// a non-last session but DRAINS (rather than removes) the last one, so at least one shell
    /// always remains. So this is not a pointer that must be maintained; it is the first
    /// session, for the life of the registry.
    ///
    /// It is no longer IMMUTABLE, though: since [`kill_session`] can remove the first session,
    /// killing the current default re-points this at the next one. That is the honest
    /// consequence of a removal path (a client that wants a specific session names it); the
    /// answer the registry's own earlier bound note called for ("re-establish a default") is
    /// taken structurally — the list order IS the default, and removal just shifts it.
    ///
    /// [`kill_session`]: Self::kill_session
    #[must_use]
    pub fn default_session(&self) -> &Session {
        &self.sessions[0]
    }

    /// The window named `window` of the session named `session`, mutably — the seam a caller
    /// reconciles the arrangement through ([`Window::reconcile_layout`]). `None` if no session
    /// carries the session name OR no window of it carries the window name.
    ///
    /// Name-addressed on BOTH dimensions, and that is what closes the window-switch bound
    /// [`crate::layout`] flagged: a request's `SessionScope` (in `sprag-host`) pins the
    /// window it was assembled for, so the layout paths act on THAT window rather than
    /// "whichever is current at the moment of use" — the two agree even if the current window
    /// moved between a request's resolve and its use.
    ///
    /// The `Option` is what makes a vanished scope a REFUSAL at the caller rather than a
    /// panic here: a scope is validated when a request arrives, but the authority for "does
    /// this session / window exist" is this type, and asking it again at the moment of use is
    /// what keeps the two from drifting once a removal path exists.
    pub fn window_mut(&mut self, session: &str, window: &str) -> Option<&mut Window> {
        let session = self.sessions.iter_mut().find(|s| s.name == session)?;
        session.windows.iter_mut().find(|w| w.name == window)
    }

    /// The window named `window` of the session named `session` — [`window_mut`](Self::window_mut)'s
    /// read half, name-addressed on both dimensions for the same reason and `None` in the same two
    /// cases.
    ///
    /// A reader wants the same "THAT window, not whichever is current now" guarantee a writer does:
    /// the host arbitrates a session's window size against the arrangement of the window its scope
    /// was assembled for, and reading a pinned size off a different window than the one being tiled
    /// would lay one window's tree out over another window's rectangle.
    #[must_use]
    pub fn window(&self, session: &str, window: &str) -> Option<&Window> {
        let session = self.sessions.iter().find(|s| s.name == session)?;
        session.windows.iter().find(|w| w.name == window)
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

    /// The pane pool of a SPECIFIC window, by session AND window name — cloned so the registry
    /// lock releases before a workspace lock is taken. `None` if no session carries the session
    /// name or no window of it carries the window name.
    ///
    /// Unlike [`workspace_of`](Self::workspace_of) (which resolves the CURRENT window), this
    /// addresses an arbitrary window — how a restore re-spawns each recorded pane into the exact
    /// window it belonged to, current or not.
    #[must_use]
    pub fn window_workspace(&self, session: &str, window: &str) -> Option<Arc<Mutex<Workspace>>> {
        let session = self.sessions.iter().find(|s| s.name == session)?;
        session
            .windows
            .iter()
            .find(|w| w.name == window)
            .map(|w| Arc::clone(w.workspace()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CommandBuilder, LayoutNode, Pane, SplitDir};

    /// Whether a zoom answer says one pane is filling the window — for the cases that assert the
    /// STATE and leave `changed` to the test that is about it.
    fn zoom(outcome: Option<ZoomOutcome>) -> bool {
        outcome.expect("the pane is one of the window's").zoomed
    }

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

    /// The default session's CURRENT window, mutably.
    fn default_window(reg: &mut SessionRegistry) -> &mut Window {
        let name = default_name(reg);
        let window = reg.default_session().current_window().name().to_owned();
        reg.window_mut(&name, &window)
            .expect("the default session always resolves")
    }

    /// A long-lived `cat` child in `dir` — so a spawned pane's PTY (and its `/proc` cwd) stay open
    /// across the [`SessionRegistry::session_infos_live`] read. Linux-only (cwd via `/proc`).
    #[cfg(target_os = "linux")]
    fn cmd_in(dir: &std::path::Path) -> CommandBuilder {
        let mut c = CommandBuilder::new("/bin/sh");
        c.arg("-c");
        c.arg("cat");
        c.cwd(dir);
        c.env("TERM", "dumb");
        c
    }

    /// A unique temp directory removed on drop — the test leaves nothing behind even if it panics.
    #[cfg(target_os = "linux")]
    struct TmpDir(std::path::PathBuf);

    #[cfg(target_os = "linux")]
    impl TmpDir {
        fn new(tag: &str) -> Self {
            use std::sync::atomic::{AtomicU32, Ordering};
            static N: AtomicU32 = AtomicU32::new(0);
            let n = N.fetch_add(1, Ordering::Relaxed);
            let d =
                std::env::temp_dir().join(format!("sprag-sinfo-{tag}-{}-{n}", std::process::id()));
            std::fs::create_dir_all(&d).expect("create temp dir");
            Self(d)
        }
    }

    #[cfg(target_os = "linux")]
    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The listability rule ([`SessionInfo::is_listable`]) shows a session iff it holds a pane OR a
    /// client is attached — so the resting empty anchor (neither) is hidden while an empty session a
    /// client is viewing still lists. Deterministic + revert-proof: flipping the rule's `||` to `&&`
    /// would drop the attached-empty case, and dropping the pane check would drop a working session.
    #[test]
    fn is_listable_shows_working_or_attached_and_hides_the_resting_anchor() {
        let si = |panes: usize, attached: usize| SessionInfo {
            name: "s".to_owned(),
            windows: 1,
            panes,
            default: false,
            attached,
        };
        assert!(si(1, 0).is_listable(), "a working session lists");
        assert!(si(3, 2).is_listable(), "a working, watched session lists");
        // tmux-superior: an EMPTY session a client is attached to still lists, so the client can
        // see where it is — a state tmux cannot represent (an empty session cannot exist there).
        assert!(
            si(0, 1).is_listable(),
            "an empty but attached session lists"
        );
        // The resting anchor: no pane, nobody attached — hidden, matching `tmux ls` at rest.
        assert!(
            !si(0, 0).is_listable(),
            "the resting empty anchor is hidden"
        );
    }

    /// The [`ActivitySampler`](crate::ActivitySampler) carries EACH session's own live cwd and git
    /// branch, derived host-side from the current window's first pane. A pane in a (fake) repo
    /// reports its branch; a pane in a plain dir reports a cwd but no branch — proving the
    /// derivation is per-session, not global. Linux-only: the cwd comes from `/proc/<pid>/cwd`.
    ///
    /// The subject moved to `crate::activity` in R282; this test did not, because the harness it
    /// needs is here — a real PTY pane spawned in a chosen directory (`cmd_in`, [`TmpDir`]) against
    /// a real registry. Duplicating that into the other module to keep the test beside its subject
    /// would buy tidiness with two copies of the fixture. What DOES live beside the subject is the
    /// sampler's own logic (what it holds, what it re-takes, what age it reports), which needs no
    /// PTY at all.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_sampler_carries_each_sessions_cwd_and_branch() {
        // A FAKE repo: `git::branch` reads `.git/HEAD`, so no real `git` is needed.
        let repo = TmpDir::new("repo");
        std::fs::create_dir_all(repo.0.join(".git")).unwrap();
        std::fs::write(repo.0.join(".git/HEAD"), "ref: refs/heads/slice2\n").unwrap();
        let plain = TmpDir::new("plain");

        let reg = Arc::new(Mutex::new(SessionRegistry::new((80, 24))));
        let (def_pool, plain_pool) = {
            let mut r = reg.lock().unwrap_or_else(PoisonError::into_inner);
            let default = r.default_session().name().to_owned();
            r.new_session(Some("plain")).unwrap();
            (
                r.workspace_of(&default).unwrap(),
                r.workspace_of("plain").unwrap(),
            )
        };
        lock(&def_pool)
            .spawn(cmd_in(&repo.0), "sh".to_owned(), 80, 24)
            .unwrap();
        lock(&plain_pool)
            .spawn(cmd_in(&plain.0), "sh".to_owned(), 80, 24)
            .unwrap();

        let reading = crate::ActivitySampler::new().read(&reg, std::time::Duration::ZERO);
        let infos = SessionRegistry::session_infos_live(&reg);

        let def_name = infos
            .iter()
            .find(|i| i.default)
            .expect("the default session")
            .name
            .clone();
        assert_eq!(
            infos.iter().find(|i| i.default).expect("default").panes,
            1,
            "session_infos_live counts the default session's one live pane",
        );
        let def_row = reading
            .value
            .iter()
            .find(|row| row.name == def_name)
            .expect("an activity row for the default session");
        assert_eq!(
            def_row.branch.as_deref(),
            Some("slice2"),
            "the default session's branch came from its pane's repo",
        );
        assert_eq!(
            def_row
                .cwd
                .as_deref()
                .map(|c| std::path::Path::new(c).canonicalize().ok()),
            Some(repo.0.canonicalize().ok()),
            "and its cwd is the repo it spawned in",
        );

        let plain_row = reading
            .value
            .iter()
            .find(|row| row.name == "plain")
            .expect("an activity row for the plain session");
        assert_eq!(plain_row.branch, None, "a non-repo pane reports no branch");
        assert!(plain_row.cwd.is_some(), "but its cwd is still reported");
        // A cat pane serves nothing, so ports comes back empty. This only proves the sampler runs
        // the real `/proc` path for a real session without panicking (empty in, empty out); the
        // POSITIVE attribution + descendant-walk proof lives in `ports.rs`
        // (`a_real_listener_is_attributed_only_to_the_pid_that_holds_it`,
        // `read_children_map_links_a_real_child_into_our_subtree`).
        assert!(def_row.ports.is_empty(), "a cat pane listens on no ports");
        // And the session list itself now says nothing about any of it — the split R282 made, in the
        // one test that holds both answers at once. A `SessionInfo` has no field to be empty here.
        assert_eq!(
            serde_json::to_value(infos.iter().find(|i| i.default).expect("default"))
                .expect("a session info serialises")
                .as_object()
                .expect("an object")
                .keys()
                .filter(|key| ["cwd", "branch", "ports"].contains(&key.as_str()))
                .count(),
            0,
            "the session list carries no sampled field",
        );
    }

    /// [`SessionRegistry::window_pids`] gathers the child pid of every pane across every window pool
    /// it is GIVEN — the roots the `/proc` scan walks. REVERT-PROOF for the helper: given both
    /// windows' pools it finds two pids, given only the first window's it finds one. (That
    /// [`session_infos_live`](SessionRegistry::session_infos_live) feeds it ALL a session's window
    /// pools — the all-windows port scope — is a separate fact, visible in its phase-1 loop over
    /// `session.windows()`; this test covers only the helper.)
    #[test]
    fn window_pids_gathers_every_pane_across_all_windows() {
        let mut reg = SessionRegistry::new((80, 24));
        let default = default_name(&reg);
        reg.new_window(&default, None).unwrap(); // a second window in the default session
        let pools: Vec<_> = reg
            .default_session()
            .windows()
            .iter()
            .map(|w| Arc::clone(w.workspace()))
            .collect();
        assert_eq!(pools.len(), 2, "the default session now has two windows");
        for pool in &pools {
            lock(pool).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap();
        }

        let all = SessionRegistry::window_pids(&pools);
        assert_eq!(
            all.len(),
            2,
            "one live child pid per pane, across BOTH windows"
        );
        let first_only = SessionRegistry::window_pids(&pools[..1]);
        assert_eq!(
            first_only.len(),
            1,
            "the current window alone would find only its own pane's pid",
        );
        assert!(
            all.contains(&first_only[0]),
            "the wider scope is a superset"
        );
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

    /// A split places a pane the tree has never seen — the case that exists because a spawn
    /// puts a pane in the POOL and only a reconcile puts it in the tiling. Placing it and
    /// reconciling it are one call, so nothing can land in between.
    #[test]
    fn a_split_places_a_pane_the_tiling_has_not_seen_yet() {
        let mut reg = SessionRegistry::new((80, 24));
        let ws = pool(&reg);
        let ids: Vec<_> = (0..2)
            .map(|_| lock(&ws).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap())
            .collect();
        let panes: Vec<_> = lock(&ws).panes().iter().map(Pane::id).collect();
        let window = default_window(&mut reg);
        window.reconcile_layout(&panes);
        assert_eq!(
            window.layout().panes(),
            ids,
            "the tiling starts as the pool"
        );

        // The spawn a split follows: in the pool, absent from the tree.
        let fresh = lock(&ws).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap();
        let panes: Vec<_> = lock(&ws).panes().iter().map(Pane::id).collect();
        let window = default_window(&mut reg);
        let before = window.layout_revision();

        assert!(window.place_pane(fresh, ids[0], SplitSide::Second, SplitDir::Vertical, &panes));

        assert_eq!(
            window.layout().panes(),
            vec![ids[0], fresh, ids[1]],
            "the fresh pane landed BELOW pane 0, not appended after pane 1",
        );
        assert!(
            window.layout_revision() > before,
            "the tiling moved, so every attached client must re-read it",
        );
    }

    /// The reconcile inside `place_pane` is what makes the target judged against the tiling as
    /// it IS: a pane that has EXITED still holds a leaf until something reconciles, and a split
    /// aimed at that stale leaf must be refused rather than dividing a ghost.
    ///
    /// Revert-proof: drop `place_pane`'s `reconcile_layout` call and this split SUCCEEDS,
    /// putting the new pane beside a pane that is gone.
    #[test]
    fn a_split_refuses_a_target_that_has_exited_even_before_anyone_reconciled() {
        let mut reg = SessionRegistry::new((80, 24));
        let ws = pool(&reg);
        let ids: Vec<_> = (0..3)
            .map(|_| lock(&ws).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap())
            .collect();
        let panes: Vec<_> = lock(&ws).panes().iter().map(Pane::id).collect();
        let window = default_window(&mut reg);
        window.reconcile_layout(&panes);
        assert!(window.layout().panes().contains(&ids[1]), "pane 1 is tiled");

        // Pane 1 exits. Nothing has reconciled since, so its leaf is still in the tree.
        let gone = lock(&ws).close(ids[1]);
        drop(gone);
        let fresh = lock(&ws).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap();
        let panes: Vec<_> = lock(&ws).panes().iter().map(Pane::id).collect();
        let window = default_window(&mut reg);

        assert!(
            !window.place_pane(fresh, ids[1], SplitSide::Second, SplitDir::Vertical, &panes),
            "a split cannot divide a pane that is gone",
        );
        assert!(
            !window.layout().panes().contains(&ids[1]),
            "and the reconcile it ran first dropped the ghost's leaf",
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

        reg.new_session(Some("work")).expect("a free name");
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
        reg.new_session(Some("work")).unwrap();
        assert_eq!(
            reg.new_session(Some("work")).unwrap_err(),
            SessionError::Duplicate("work".to_owned()),
        );
        assert_eq!(reg.sessions().len(), 2, "the refused create added nothing");
    }

    /// With no name, the registry ALLOCATES the lowest free one — tmux's `new-session`
    /// without `-s`. The caller learns the name it got (it did not choose it), and because the
    /// allocation happens under the registry lock, two clients cannot invent the same name and
    /// race for it.
    #[test]
    fn an_unnamed_new_session_allocates_the_lowest_free_name() {
        let mut reg = SessionRegistry::new((80, 24));

        // The boot session is "0", so the first allocation is "1", the next "2".
        assert_eq!(reg.new_session(None).unwrap(), "1");
        assert_eq!(reg.new_session(None).unwrap(), "2");

        // An explicit numeric name is STEPPED OVER, never reused: name "4" by hand, and the
        // next allocation fills the "3" gap, then continues at "5".
        reg.new_session(Some("4")).unwrap();
        assert_eq!(reg.new_session(None).unwrap(), "3");
        assert_eq!(reg.new_session(None).unwrap(), "5");

        for name in ["1", "2", "3", "4", "5"] {
            assert!(reg.session(name).is_some(), "{name} is its own session");
        }
        assert_eq!(
            reg.sessions().len(),
            6,
            "the boot session plus the five created"
        );
    }

    /// The window RING: `next` walks forward and WRAPS, `previous` walks back and wraps, and each
    /// answers the name it landed on.
    ///
    /// The wrap is asserted from BOTH ends rather than once, because a walk that clamped instead of
    /// wrapping would be indistinguishable from a correct one anywhere in the middle — and the two
    /// ends fail differently (a clamped `next` sticks on the last, a clamped `previous` on the
    /// first).
    ///
    /// REVERT-PROOF: drop the `rem_euclid` for a saturating step and the two wrap rows fail; swap
    /// `WindowStep::offset`'s signs and every row lands one window the wrong way.
    #[test]
    fn the_window_walk_wraps_in_both_directions_and_answers_where_it_landed() {
        let mut reg = SessionRegistry::new((80, 24));
        let session = reg.default_session().name().to_owned();
        for name in ["a", "b"] {
            reg.new_window(&session, Some(name)).expect("a window");
        }
        // Three windows: "0" (the boot window), "a", "b" — and `new_window` selected the last.
        reg.select_window(&session, "0")
            .expect("start at the first");

        let step = |reg: &mut SessionRegistry, step| {
            reg.select_window_relative(&session, step)
                .expect("the session resolves")
        };
        assert_eq!(step(&mut reg, WindowStep::Next), "a");
        assert_eq!(step(&mut reg, WindowStep::Next), "b");
        assert_eq!(
            step(&mut reg, WindowStep::Next),
            "0",
            "the last wraps onto the first, which is what makes it a ring",
        );
        assert_eq!(
            step(&mut reg, WindowStep::Previous),
            "b",
            "and the first wraps onto the last, going the other way",
        );
        assert_eq!(step(&mut reg, WindowStep::Previous), "a");

        // The walk really MOVED the session's current window, not just answered a name — read back
        // through the listing every client projects from.
        let current: Vec<(String, bool)> = reg
            .session(&session)
            .expect("the session")
            .window_infos()
            .into_iter()
            .map(|info| (info.name, info.current))
            .collect();
        assert_eq!(
            current,
            vec![
                ("0".to_owned(), false),
                ("a".to_owned(), true),
                ("b".to_owned(), false),
            ],
        );
    }

    /// A session with ONE window walks to itself, both ways — the property that makes the step
    /// TOTAL, so the action can answer without an error case where the named form has one.
    #[test]
    fn the_window_walk_is_total_for_a_session_with_one_window() {
        let mut reg = SessionRegistry::new((80, 24));
        let session = reg.default_session().name().to_owned();
        for step in WindowStep::ALL {
            assert_eq!(
                reg.select_window_relative(&session, step)
                    .expect("the session resolves"),
                "0",
                "{step:?} on a one-window session lands where it was",
            );
        }
        assert!(
            reg.select_window_relative("nosuch", WindowStep::Next)
                .is_err(),
            "the ONLY way this refuses is a session that does not exist",
        );
    }

    /// The two wire words are ONE vocabulary, read and written by the same table — so a spelling
    /// cannot drift between the action, the CLI flags and the keybinding.
    #[test]
    fn a_window_step_round_trips_through_its_wire_word() {
        for step in WindowStep::ALL {
            assert_eq!(WindowStep::from_wire(step.wire_str()), Some(step));
        }
        assert_eq!(WindowStep::from_wire("sideways"), None);
        // The words themselves, pinned: they cross the wire, so renaming one is a protocol change
        // rather than a refactor.
        assert_eq!(WindowStep::Next.wire_str(), "next");
        assert_eq!(WindowStep::Previous.wire_str(), "previous");
    }

    /// Killing a NON-last session removes it; killing the DEFAULT (first) re-points the default
    /// at the next — the honest consequence of a removal path, which the old immutable-default
    /// doc could promise only because nothing could remove a session.
    #[test]
    fn kill_session_removes_a_non_last_session_and_can_move_the_default() {
        let mut reg = SessionRegistry::new((80, 24));
        reg.new_session(Some("work")).unwrap();
        reg.new_session(Some("play")).unwrap();
        assert_eq!(reg.sessions().len(), 3);
        assert_eq!(reg.default_session().name(), "0");

        // A non-default session: removed, the default unchanged.
        assert!(matches!(
            reg.kill_session("work").unwrap(),
            KillOutcome::Removed(_)
        ));
        assert!(reg.session("work").is_none());
        assert_eq!(reg.sessions().len(), 2);
        assert_eq!(
            reg.default_session().name(),
            "0",
            "killing another session leaves the default where it was",
        );

        // The DEFAULT session: the next becomes the default.
        assert!(matches!(
            reg.kill_session("0").unwrap(),
            KillOutcome::Removed(_)
        ));
        assert!(reg.session("0").is_none());
        assert_eq!(
            reg.default_session().name(),
            "play",
            "killing the default re-points it at the next session",
        );
    }

    /// Killing an unknown session is refused and changes nothing.
    #[test]
    fn kill_session_refuses_an_unknown_name() {
        let mut reg = SessionRegistry::new((80, 24));
        reg.new_session(Some("work")).unwrap();
        assert!(
            matches!(reg.kill_session("ghost"), Err(SessionError::Unknown(name)) if name == "ghost"),
            "an unknown name is refused as Unknown, carrying the name asked for",
        );
        assert_eq!(reg.sessions().len(), 2, "the refused kill removed nothing");
        assert!(reg.session("work").is_some());
    }

    /// Killing the LAST session does NOT remove it — that would empty the registry and unresolve
    /// the default — but DRAINS its panes and reports [`KillOutcome::KilledServer`] so the caller
    /// ends the daemon. The shell is kept, so `default_session` stays total.
    #[test]
    fn kill_session_on_the_last_drains_it_and_keeps_the_default_total() {
        let mut reg = SessionRegistry::new((80, 24));
        let ws = pool(&reg);
        let _a = lock(&ws).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap();
        let _b = lock(&ws).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap();
        assert_eq!(lock(&ws).panes().len(), 2);

        let name = default_name(&reg);
        assert!(matches!(
            reg.kill_session(&name).unwrap(),
            KillOutcome::KilledServer(_)
        ));

        assert_eq!(reg.sessions().len(), 1, "the last session is NOT removed");
        assert_eq!(
            reg.default_session().name(),
            name,
            "so the default still resolves — total by construction",
        );
        assert!(
            lock(&ws).panes().is_empty(),
            "but its panes are drained, so no live pane keeps the daemon alive",
        );
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
        reg.new_session(Some("work")).unwrap();

        // Absent at every resolution site — not an error to be handled, just nothing.
        assert!(reg.session("ghost").is_none());
        assert!(reg.workspace_of("ghost").is_none());
        assert!(reg.window_mut("ghost", "0").is_none());

        // ...while a real name resolves at each of them. Without this half, the assertions
        // above would pass just as well against a registry that resolves NOTHING.
        assert_eq!(reg.session("work").map(Session::name), Some("work"));
        assert!(reg.workspace_of("work").is_some());
        assert!(reg.window_mut("work", "0").is_some());
        // A real session but an unknown WINDOW is also nothing — the address is two-dimensional.
        assert!(reg.window_mut("work", "ghost").is_none());

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

        reg.new_session(Some("work")).unwrap();
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
        // it. (Pools are constructed directly here to isolate the counter-sharing the registry
        // relies on; `new_window_appends_selects_and_shares_the_id_counter` proves the same
        // through the registry's real new-window API.)
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

    // ─── windows: new / select / rename / kill ───

    /// tmux `new-window`: it APPENDS a window, MAKES IT CURRENT, and its pool draws from the
    /// ONE registry-wide id counter — a pane spawned there gets a fresh global id, never a
    /// collision with window "0".
    #[test]
    fn new_window_appends_selects_and_shares_the_id_counter() {
        let mut reg = SessionRegistry::new((80, 24));
        let default = default_name(&reg);
        // A pane in window "0" takes id 0 before the new window exists.
        let ws0 = pool(&reg);
        let a = lock(&ws0).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap();
        assert_eq!(a.0, 0);

        assert_eq!(
            reg.new_window(&default, None).unwrap(),
            "1",
            "lowest free name"
        );
        let session = reg.session(&default).unwrap();
        assert_eq!(session.windows().len(), 2);
        assert_eq!(
            session.current_window().name(),
            "1",
            "new-window makes the new one current",
        );
        assert!(
            session.current_window().layout().panes().is_empty(),
            "born empty — the host births its pane",
        );

        // The new window's pool mints the NEXT global id, not a fresh 0.
        let ws1 = reg
            .workspace_of(&default)
            .expect("current = the new window");
        let b = lock(&ws1).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap();
        assert!(
            b > a && b.0 == 1,
            "a shared, monotonic counter: {a} then {b}"
        );
    }

    /// With no name the registry allocates the lowest free integer, tmux-style; an explicit
    /// name is stepped over, and a duplicate is refused with nothing added.
    #[test]
    fn new_window_names_allocate_step_over_and_refuse_duplicates() {
        let mut reg = SessionRegistry::new((80, 24));
        let default = default_name(&reg);

        assert_eq!(reg.new_window(&default, None).unwrap(), "1");
        reg.new_window(&default, Some("3")).unwrap();
        assert_eq!(
            reg.new_window(&default, None).unwrap(),
            "2",
            "fills the gap"
        );
        assert_eq!(
            reg.new_window(&default, None).unwrap(),
            "4",
            "then continues"
        );

        assert_eq!(
            reg.new_window(&default, Some("3")).unwrap_err(),
            SessionError::Duplicate("3".to_owned()),
            "a taken window name is refused",
        );
        assert_eq!(
            reg.session(&default).unwrap().windows().len(),
            5,
            "the boot window plus four created; the refused one added nothing",
        );
        // An unknown session is Unknown, not Duplicate.
        assert!(matches!(
            reg.new_window("ghost", None),
            Err(SessionError::Unknown(name)) if name == "ghost",
        ));
    }

    /// `select-window` moves the current window (session state — every attached client
    /// follows), and an unknown window is refused, leaving the current one put.
    #[test]
    fn select_window_moves_the_current_and_refuses_unknown() {
        let mut reg = SessionRegistry::new((80, 24));
        let default = default_name(&reg);
        reg.new_window(&default, Some("work")).unwrap();
        assert_eq!(
            reg.session(&default).unwrap().current_window().name(),
            "work"
        );

        reg.select_window(&default, "0").unwrap();
        assert_eq!(reg.session(&default).unwrap().current_window().name(), "0");

        assert!(matches!(
            reg.select_window(&default, "ghost"),
            Err(SessionError::Unknown(name)) if name == "ghost",
        ));
        assert_eq!(
            reg.session(&default).unwrap().current_window().name(),
            "0",
            "a refused select leaves the current window unchanged",
        );
    }

    /// `rename-window` renames, refuses a name another window holds, and treats renaming a
    /// window to the name it already has as a no-op (not a duplicate).
    #[test]
    fn rename_window_renames_refuses_a_duplicate_and_allows_a_noop() {
        let mut reg = SessionRegistry::new((80, 24));
        let default = default_name(&reg);
        reg.new_window(&default, Some("1")).unwrap();

        reg.rename_window(&default, "0", "editor").unwrap();
        let names = |reg: &SessionRegistry| -> Vec<String> {
            reg.session(&default)
                .unwrap()
                .windows()
                .iter()
                .map(|w| w.name().to_owned())
                .collect()
        };
        assert_eq!(names(&reg), vec!["editor".to_owned(), "1".to_owned()]);

        // Renaming onto a name another window holds is refused.
        assert_eq!(
            reg.rename_window(&default, "1", "editor").unwrap_err(),
            SessionError::Duplicate("editor".to_owned()),
        );
        assert_eq!(
            names(&reg),
            vec!["editor".to_owned(), "1".to_owned()],
            "unchanged"
        );

        // Renaming a window to its own current name is a no-op, not a duplicate.
        reg.rename_window(&default, "editor", "editor").unwrap();
        assert_eq!(names(&reg), vec!["editor".to_owned(), "1".to_owned()]);

        // Unknown window / session refuse.
        assert!(matches!(
            reg.rename_window(&default, "ghost", "x"),
            Err(SessionError::Unknown(name)) if name == "ghost",
        ));
    }

    /// `rename-session` renames, refuses a name another session holds, treats a rename to the
    /// name it already has as a no-op, and refuses an unknown one — and through all of that the
    /// session's IDENTITY does not move, which is the property the change funnel derives on.
    #[test]
    fn rename_session_moves_the_name_and_never_the_identity() {
        let mut reg = SessionRegistry::new((80, 24));
        let default = default_name(&reg);
        reg.new_session(Some("play")).unwrap();
        let was = reg.session(&default).unwrap().id();

        reg.rename_session(&default, "work").unwrap();
        assert!(
            reg.session(&default).is_none(),
            "the old address stops resolving — that is what makes it an address",
        );
        let renamed = reg.session("work").expect("the new address resolves");
        assert_eq!(
            renamed.id(),
            was,
            "SAME session: an identity that moved with the name would make every rename              indistinguishable from a close plus a create, which is the whole reason it exists",
        );
        assert_eq!(
            reg.name_of(was),
            Some("work"),
            "and it answers to the new one"
        );

        // Onto a name another session holds: refused, and NOTHING moves.
        assert_eq!(
            reg.rename_session("work", "play").unwrap_err(),
            SessionError::Duplicate("play".to_owned()),
        );
        assert_eq!(reg.name_of(was), Some("work"), "unchanged");

        // To its own name: a no-op, not a duplicate.
        reg.rename_session("work", "work").unwrap();
        assert_eq!(reg.name_of(was), Some("work"));

        assert!(matches!(
            reg.rename_session("ghost", "x"),
            Err(SessionError::Unknown(name)) if name == "ghost",
        ));
    }

    /// Every session and every window this registry mints gets a DISTINCT identity — the property
    /// the funnel's whole derivation rests on, and one a per-session counter would break the day a
    /// window can move between sessions.
    #[test]
    fn no_two_sessions_or_windows_share_an_identity() {
        let mut reg = SessionRegistry::new((80, 24));
        let default = default_name(&reg);
        reg.new_window(&default, Some("1")).unwrap();
        reg.new_session(Some("play")).unwrap();
        reg.new_window("play", Some("1")).unwrap();

        let sessions: HashSet<_> = reg.sessions().iter().map(Session::id).collect();
        assert_eq!(sessions.len(), 2, "two sessions, two identities");
        let windows: HashSet<_> = reg
            .sessions()
            .iter()
            .flat_map(|session| session.windows().iter().map(Window::id))
            .collect();
        assert_eq!(
            windows.len(),
            4,
            "four windows across two sessions, four identities — registry-wide, not per-session",
        );
    }

    /// Killing a NON-last window removes it, drains its panes, and keeps `current_window` valid
    /// and on the neighbour that took its place — the next window, or the previous if the last
    /// was killed. The session and daemon keep running.
    #[test]
    fn kill_window_removes_a_non_last_and_keeps_current_on_a_neighbour() {
        let mut reg = SessionRegistry::new((80, 24));
        let default = default_name(&reg);
        // Windows "0", "1", "2"; a live pane in "1" so its kill actually drains something.
        reg.new_window(&default, Some("1")).unwrap();
        reg.new_window(&default, Some("2")).unwrap();
        let ws1 = {
            reg.select_window(&default, "1").unwrap();
            reg.workspace_of(&default).unwrap()
        };
        let _p = lock(&ws1).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap();
        assert_eq!(lock(&ws1).panes().len(), 1);

        // Current is "1" (the middle). Killing it drops to the window that took its slot ("2").
        assert!(matches!(
            reg.kill_window(&default, "1").unwrap(),
            WindowKillOutcome::Removed(panes) if panes.len() == 1,
        ));
        let session = reg.session(&default).unwrap();
        assert_eq!(session.windows().len(), 2);
        assert!(
            session.windows().iter().all(|w| w.name() != "1"),
            "1 is gone"
        );
        assert_eq!(
            session.current_window().name(),
            "2",
            "current follows to the window that took the killed one's index",
        );
        assert!(
            lock(&ws1).panes().is_empty(),
            "the killed window's pane was drained"
        );

        // Killing the LAST window (by index) when it is current lands the current on the
        // previous: select "2" (now last), kill it, current becomes "0".
        reg.select_window(&default, "2").unwrap();
        assert!(matches!(
            reg.kill_window(&default, "2").unwrap(),
            WindowKillOutcome::Removed(_),
        ));
        assert_eq!(
            reg.session(&default).unwrap().current_window().name(),
            "0",
            "killing the last (current) window falls back to the previous",
        );
    }

    /// Killing the session's LAST window ends the SESSION (tmux) — it escalates to
    /// `kill_session`, reported as [`WindowKillOutcome::Session`]. A non-last session removed;
    /// the last one drains and ends the daemon.
    #[test]
    fn killing_the_last_window_escalates_to_killing_the_session() {
        let mut reg = SessionRegistry::new((80, 24));
        reg.new_session(Some("work")).unwrap();
        assert_eq!(reg.sessions().len(), 2);

        // "work" has one window; killing it removes the whole session (a non-last session).
        assert!(matches!(
            reg.kill_window("work", "0").unwrap(),
            WindowKillOutcome::Session(KillOutcome::Removed(_)),
        ));
        assert!(
            reg.session("work").is_none(),
            "the session went with its last window"
        );
        assert_eq!(reg.sessions().len(), 1);

        // The default now has one window; killing it is the LAST session ⇒ end the daemon.
        let default = default_name(&reg);
        assert!(matches!(
            reg.kill_window(&default, "0").unwrap(),
            WindowKillOutcome::Session(KillOutcome::KilledServer(_)),
        ));
        assert_eq!(
            reg.sessions().len(),
            1,
            "the last session is drained, not removed"
        );
    }

    /// Killing a window at an index BELOW the current one decrements `current_window` so it keeps
    /// pointing at the SAME window (the `> widx` branch, which every other kill test — all killing
    /// the current window — leaves unexercised).
    #[test]
    fn kill_window_below_the_current_keeps_current_on_the_same_window() {
        let mut reg = SessionRegistry::new((80, 24));
        let default = default_name(&reg);
        // Windows "0","a","b","c" (indices 0..3); make "c" (index 3) current.
        for name in ["a", "b", "c"] {
            reg.new_window(&default, Some(name)).unwrap();
        }
        reg.select_window(&default, "c").unwrap();
        assert_eq!(reg.session(&default).unwrap().current_window().name(), "c");

        // Kill "a" (index 1), which is BELOW current (index 3): current decrements to stay on "c".
        assert!(matches!(
            reg.kill_window(&default, "a").unwrap(),
            WindowKillOutcome::Removed(_),
        ));
        assert_eq!(
            reg.session(&default).unwrap().current_window().name(),
            "c",
            "killing a window below the current one keeps current on the SAME window",
        );
        let names: Vec<_> = reg
            .session(&default)
            .unwrap()
            .windows()
            .iter()
            .map(|w| w.name().to_owned())
            .collect();
        assert_eq!(names, vec!["0".to_owned(), "b".to_owned(), "c".to_owned()]);
    }

    /// kill-window refuses an unknown session or window, carrying the missing name, and removes
    /// nothing.
    #[test]
    fn kill_window_refuses_an_unknown_session_or_window() {
        let mut reg = SessionRegistry::new((80, 24));
        let default = default_name(&reg);
        reg.new_window(&default, Some("1")).unwrap();

        assert!(matches!(
            reg.kill_window("ghost", "0"),
            Err(SessionError::Unknown(name)) if name == "ghost",
        ));
        assert!(matches!(
            reg.kill_window(&default, "ghost"),
            Err(SessionError::Unknown(name)) if name == "ghost",
        ));
        assert_eq!(
            reg.session(&default).unwrap().windows().len(),
            2,
            "a refused kill removed nothing",
        );
    }

    /// Spawn `n` live panes into the window named `w` of the default session, returning their ids
    /// (spawned straight into the pool, as the host does; the window's layout lags until a read
    /// reconciles it, which the move paths do).
    fn spawn_into(reg: &SessionRegistry, w: &str, n: usize) -> Vec<PaneId> {
        let ws = reg
            .window_workspace(&default_name(reg), w)
            .expect("the window exists");
        (0..n)
            .map(|_| lock(&ws).spawn(cmd(), "sh".to_owned(), 80, 24).unwrap())
            .collect()
    }

    /// The pane ids the window named `w` currently pools, in order.
    fn pool_ids(reg: &SessionRegistry, w: &str) -> Vec<PaneId> {
        let ws = reg
            .window_workspace(&default_name(reg), w)
            .expect("the window exists");
        let pool = lock(&ws);
        pool.panes().iter().map(Pane::id).collect()
    }

    /// `break-pane` moves the pane WHOLE into a new window (same id — not re-spawned), selects the
    /// new window, and leaves the source with its remaining panes. The tmux-superior claim: the
    /// pane's identity survives the move, so its PTY / emulator / history ride along.
    #[test]
    fn break_pane_moves_a_pane_whole_into_a_new_selected_window() {
        let mut reg = SessionRegistry::new((80, 24));
        let default = default_name(&reg);
        let ids = spawn_into(&reg, "0", 2);
        let (a, b) = (ids[0], ids[1]);

        assert_eq!(
            reg.break_pane(&default, b, None).unwrap(),
            "1",
            "the new window gets the lowest free name",
        );
        let session = reg.session(&default).unwrap();
        assert_eq!(session.windows().len(), 2);
        assert_eq!(
            session.current_window().name(),
            "1",
            "break-pane makes the new window current",
        );
        // The moved pane kept its exact id in the new window; the source kept the other.
        assert_eq!(pool_ids(&reg, "1"), vec![b], "moved pane, same id");
        assert_eq!(
            pool_ids(&reg, "0"),
            vec![a],
            "source keeps its remaining pane"
        );
        // The new (current) window's tree reconciled to the moved pane.
        assert_eq!(
            reg.session(&default)
                .unwrap()
                .current_window()
                .layout()
                .panes(),
            vec![b],
            "the new window's tree reconciled to the moved pane",
        );

        // The id counter is shared and monotonic: the next spawn is 2, never a reused 0/1.
        let next = spawn_into(&reg, "1", 1)[0];
        assert_eq!(next.0, 2, "shared, monotonic id counter across the move");
    }

    /// `break-pane` refuses without moving anything: the only pane of a window (a rename dressed as
    /// a move), a taken new-window name, an unknown window, and a pane the window does not hold.
    #[test]
    fn break_pane_refuses_and_moves_nothing() {
        let mut reg = SessionRegistry::new((80, 24));
        let default = default_name(&reg);

        // The only pane cannot be broken out.
        let solo = spawn_into(&reg, "0", 1)[0];
        assert_eq!(
            reg.break_pane(&default, solo, None).unwrap_err(),
            PaneMoveError::LastPane,
        );
        assert_eq!(
            reg.session(&default).unwrap().windows().len(),
            1,
            "no window added"
        );
        assert_eq!(pool_ids(&reg, "0"), vec![solo], "the pane stayed put");

        // Two panes now; an explicit name that is taken is refused.
        let more = spawn_into(&reg, "0", 1)[0];
        reg.new_window(&default, Some("keep")).unwrap();
        assert_eq!(
            reg.break_pane(&default, more, Some("keep")).unwrap_err(),
            PaneMoveError::DuplicateWindow("keep".to_owned()),
        );
        assert_eq!(
            pool_ids(&reg, "0"),
            vec![solo, more],
            "nothing moved on a refusal"
        );

        // A pane no window holds refuses (the source window is derived from the id).
        assert_eq!(
            reg.break_pane(&default, PaneId(999), None).unwrap_err(),
            PaneMoveError::UnknownPane(PaneId(999)),
        );
        // An unknown SESSION refuses at the registry wrapper.
        assert_eq!(
            reg.break_pane("nope", more, None).unwrap_err(),
            PaneMoveError::UnknownSession("nope".to_owned()),
        );
    }

    /// `join-pane` appends a pane from one window into another as a new leaf; the source keeps its
    /// remaining panes and the current window does not move.
    #[test]
    fn join_pane_appends_into_the_destination_and_keeps_a_nonempty_source() {
        let mut reg = SessionRegistry::new((80, 24));
        let default = default_name(&reg);
        let src = spawn_into(&reg, "0", 2);
        let (a, b) = (src[0], src[1]);
        reg.new_window(&default, Some("1")).unwrap();
        let c = spawn_into(&reg, "1", 1)[0];
        // Selecting "1" then back to "0" leaves current on "0" — the join must not move it.
        reg.select_window(&default, "0").unwrap();

        assert!(
            !reg.join_pane(&default, b, "1").unwrap(),
            "the source kept a pane, so it was not closed",
        );
        assert_eq!(
            pool_ids(&reg, "1"),
            vec![c, b],
            "appended after the destination's pane"
        );
        assert_eq!(
            pool_ids(&reg, "0"),
            vec![a],
            "source keeps its remaining pane"
        );
        assert_eq!(
            reg.session(&default).unwrap().current_window().name(),
            "0",
            "a join that keeps the source open leaves the current window put",
        );
    }

    /// A join that EMPTIES the source window closes it (tmux), and when that source was the CURRENT
    /// window the current moves to the neighbour that takes its place — the kill_window clamp.
    #[test]
    fn join_pane_that_empties_the_source_closes_it_and_reclamps_current() {
        let mut reg = SessionRegistry::new((80, 24));
        let default = default_name(&reg);
        let a = spawn_into(&reg, "0", 1)[0];
        reg.new_window(&default, Some("1")).unwrap();
        let b = spawn_into(&reg, "1", 1)[0];
        // Current is the SOURCE window "0" (index 0).
        reg.select_window(&default, "0").unwrap();

        assert!(
            reg.join_pane(&default, a, "1").unwrap(),
            "the source emptied, so it was closed",
        );
        let session = reg.session(&default).unwrap();
        assert_eq!(
            session.windows().len(),
            1,
            "the emptied source window is gone"
        );
        assert_eq!(
            session.current_window().name(),
            "1",
            "current re-clamped onto the window that took the closed one's place",
        );
        assert_eq!(
            pool_ids(&reg, "1"),
            vec![b, a],
            "both panes now live in the survivor"
        );
    }

    /// The other clamp branch: when the CURRENT window sits ABOVE the source that a join closes,
    /// its index must DECREMENT to keep naming the same window — without it, the removal shifts the
    /// list under a now-out-of-range `current_window`, which `current_window()` would panic on.
    #[test]
    fn join_pane_closing_a_source_below_the_current_decrements_it() {
        let mut reg = SessionRegistry::new((80, 24));
        let default = default_name(&reg);
        let a = spawn_into(&reg, "0", 1)[0]; // source at index 0
        reg.new_window(&default, Some("1")).unwrap(); // destination at index 1
        spawn_into(&reg, "1", 1);
        reg.new_window(&default, Some("2")).unwrap(); // index 2
        spawn_into(&reg, "2", 1);
        // Current is "2" (index 2), ABOVE the source "0" that the join will close.
        reg.select_window(&default, "2").unwrap();

        assert!(
            reg.join_pane(&default, a, "1").unwrap(),
            "source emptied ⇒ closed"
        );
        let session = reg.session(&default).unwrap();
        assert_eq!(
            session.windows().len(),
            2,
            "\"0\" gone; \"1\" and \"2\" remain"
        );
        assert_eq!(
            session.current_window().name(),
            "2",
            "current still names \"2\" after the list shifted down",
        );
    }

    /// `join-pane` refuses without moving anything: the same window as source and destination, an
    /// unknown source or destination window, and a pane the source does not hold.
    #[test]
    fn join_pane_refuses_and_moves_nothing() {
        let mut reg = SessionRegistry::new((80, 24));
        let default = default_name(&reg);
        let a = spawn_into(&reg, "0", 1)[0];
        reg.new_window(&default, Some("1")).unwrap();
        spawn_into(&reg, "1", 1);

        // The pane already lives in "0", so joining it INTO "0" is a no-op move.
        assert_eq!(
            reg.join_pane(&default, a, "0").unwrap_err(),
            PaneMoveError::SameWindow("0".to_owned()),
        );
        // An unknown DESTINATION window refuses (the source is derived from the pane id).
        assert_eq!(
            reg.join_pane(&default, a, "ghost").unwrap_err(),
            PaneMoveError::UnknownWindow("ghost".to_owned()),
        );
        assert_eq!(
            reg.join_pane(&default, PaneId(999), "1").unwrap_err(),
            PaneMoveError::UnknownPane(PaneId(999)),
        );
        assert_eq!(
            reg.join_pane("nope", a, "1").unwrap_err(),
            PaneMoveError::UnknownSession("nope".to_owned()),
        );
        assert_eq!(
            pool_ids(&reg, "0"),
            vec![a],
            "every refusal left the pane in place"
        );
    }

    /// The arrangement the window named `w` tiles, in paint order — what a `move` / `swap` has to
    /// be asserted against, since a pool order says only WHO is in a window, never where.
    ///
    /// Settles the window first, because the arrangement reconciles LAZILY and every production
    /// read goes through one (`sprag_host::host::reconciled_layout`). A test reading the raw tree
    /// would be reading a state no client can observe, and would call a freshly spawned pane
    /// unarranged.
    fn tiled(reg: &mut SessionRegistry, w: &str) -> Vec<PaneId> {
        let name = default_name(reg);
        let window = reg.window_mut(&name, w).expect("the window exists");
        let panes: Vec<PaneId> = {
            let pool = lock(window.workspace());
            pool.panes().iter().map(Pane::id).collect()
        };
        window.reconcile_layout(&panes).panes()
    }

    /// The hole in herdr's pair of verbs, closed: `move-pane` re-places a pane INSIDE its own
    /// window. herdr's `pane.move` answers `PaneMoveReason::SameTab` unchanged for this
    /// (`src/app/api/panes.rs:685-707` at `9a4ce5e1`) and its `pane.swap` cannot express a
    /// position, so the only way to say it there is a whole-tree `layout.apply`.
    #[test]
    fn move_pane_replaces_a_pane_inside_its_own_window() {
        let mut reg = SessionRegistry::new((80, 24));
        let default = default_name(&reg);
        let ids = spawn_into(&reg, "0", 3);
        let (a, b, c) = (ids[0], ids[1], ids[2]);
        assert_eq!(
            tiled(&mut reg, "0"),
            vec![a, b, c],
            "the append order it starts in"
        );

        assert!(
            !reg.move_pane(&default, c, a, SplitSide::First, SplitDir::Horizontal)
                .unwrap(),
            "a within-window move closes no source window",
        );

        assert_eq!(
            tiled(&mut reg, "0"),
            vec![c, a, b],
            "pane c landed LEFT of pane a, which is what was asked",
        );
        assert_eq!(
            pool_ids(&reg, "0").len(),
            3,
            "and it moved rather than being duplicated",
        );
    }

    /// The other cell of the same table: the SAME verb crosses a window, placing the pane beside a
    /// target whose window it never had to name. herdr's `pane.swap` refuses to cross a tab and its
    /// `pane.move` needs the destination spelled as a `tab_id`.
    #[test]
    fn move_pane_crosses_a_window_and_lands_where_it_was_asked() {
        let mut reg = SessionRegistry::new((80, 24));
        let default = default_name(&reg);
        let src = spawn_into(&reg, "0", 2);
        let (a, b) = (src[0], src[1]);
        reg.new_window(&default, Some("1")).unwrap();
        let dst = spawn_into(&reg, "1", 2);
        let (c, d) = (dst[0], dst[1]);
        reg.select_window(&default, "0").unwrap();
        assert_eq!(tiled(&mut reg, "1"), vec![c, d]);

        assert!(
            !reg.move_pane(&default, b, c, SplitSide::Second, SplitDir::Vertical)
                .unwrap(),
            "the source kept a pane, so it was not closed",
        );

        assert_eq!(
            tiled(&mut reg, "1"),
            vec![c, b, d],
            "b landed BELOW c — inside the row, not appended at the end join-pane would have chosen",
        );
        assert_eq!(tiled(&mut reg, "0"), vec![a], "and it left the source");
        assert_eq!(
            pool_ids(&reg, "1"),
            vec![c, d, b],
            "the pane moved pools whole"
        );
    }

    /// A cross-window move that empties its source CLOSES it and re-clamps the current window —
    /// `join_pane`'s rule, reached by the same path.
    #[test]
    fn move_pane_that_empties_the_source_closes_it() {
        let mut reg = SessionRegistry::new((80, 24));
        let default = default_name(&reg);
        let a = spawn_into(&reg, "0", 1)[0];
        reg.new_window(&default, Some("1")).unwrap();
        let b = spawn_into(&reg, "1", 1)[0];
        reg.select_window(&default, "0").unwrap();

        assert!(
            reg.move_pane(&default, a, b, SplitSide::Second, SplitDir::Horizontal)
                .unwrap(),
            "the source emptied, so it was closed",
        );

        let session = reg.session(&default).unwrap();
        assert_eq!(session.windows().len(), 1, "only the destination remains");
        assert_eq!(session.current_window().name(), "1");
        assert_eq!(tiled(&mut reg, "1"), vec![b, a]);
    }

    /// `move-pane` refuses without moving anything: a pane beside ITSELF, an unknown pane in either
    /// position, an unknown session, and — the placement rule — a target that is not TILED.
    #[test]
    fn move_pane_refuses_and_moves_nothing() {
        let mut reg = SessionRegistry::new((80, 24));
        let default = default_name(&reg);
        let ids = spawn_into(&reg, "0", 3);
        let (a, b, c) = (ids[0], ids[1], ids[2]);
        let before = tiled(&mut reg, "0");

        assert_eq!(
            reg.move_pane(&default, a, a, SplitSide::Second, SplitDir::Horizontal)
                .unwrap_err(),
            PaneMoveError::SamePane(a),
        );
        assert_eq!(
            reg.move_pane(
                &default,
                PaneId(999),
                a,
                SplitSide::Second,
                SplitDir::Horizontal
            )
            .unwrap_err(),
            PaneMoveError::UnknownPane(PaneId(999)),
        );
        assert_eq!(
            reg.move_pane(
                &default,
                a,
                PaneId(999),
                SplitSide::Second,
                SplitDir::Horizontal
            )
            .unwrap_err(),
            PaneMoveError::UnknownPane(PaneId(999)),
        );
        assert_eq!(
            reg.move_pane("nope", a, b, SplitSide::Second, SplitDir::Horizontal)
                .unwrap_err(),
            PaneMoveError::UnknownSession("nope".to_owned()),
        );

        // A FLOATING target holds no leaf, so there is no cell to divide. Refused rather than
        // silently appended — the rule `place_pane` states.
        let panes = pool_ids(&reg, "0");
        let window = default_window(&mut reg);
        assert!(window.set_floating(c, true, &panes));
        window.reconcile_layout(&panes);
        assert_eq!(
            reg.move_pane(&default, a, c, SplitSide::Second, SplitDir::Horizontal)
                .unwrap_err(),
            PaneMoveError::UnknownPane(c),
        );

        assert_eq!(
            tiled(&mut reg, "0"),
            before
                .iter()
                .copied()
                .filter(|pane| *pane != c)
                .collect::<Vec<_>>(),
            "every refusal left the arrangement as the float found it",
        );
    }

    /// A cross-window swap — which herdr refuses outright (`PaneSwapReason::CrossTab`) and tmux
    /// allows. Each pane takes the other's PLACE, not merely the other's window: pane `b` lands
    /// where `d` sat inside the destination's row rather than appended after it.
    #[test]
    fn swap_panes_across_windows_trades_places_not_just_pools() {
        let mut reg = SessionRegistry::new((80, 24));
        let default = default_name(&reg);
        let src = spawn_into(&reg, "0", 2);
        let (a, b) = (src[0], src[1]);
        reg.new_window(&default, Some("1")).unwrap();
        let dst = spawn_into(&reg, "1", 3);
        let (c, d, e) = (dst[0], dst[1], dst[2]);
        assert_eq!(tiled(&mut reg, "1"), vec![c, d, e]);

        assert!(reg.swap_panes(&default, b, d).unwrap());

        assert_eq!(
            tiled(&mut reg, "1"),
            vec![c, b, e],
            "b took d's place in the MIDDLE of the row, not the end",
        );
        assert_eq!(tiled(&mut reg, "0"), vec![a, d], "and d took b's");
        assert_eq!(
            reg.session(&default).unwrap().windows().len(),
            2,
            "no window closed"
        );
    }

    /// The active pane follows the PANE within one window and the POSITION across two — the only
    /// answer available to a window whose active pane just left, and a better one than the
    /// closed-pane successor (a neighbour) `reconcile_layout` would otherwise reach for.
    ///
    /// Revert-proof: drop `adopt_at`'s `was_on_it` restore and the first assertion reads `a`,
    /// the neighbour `heal_active` hands off to.
    #[test]
    fn a_swap_carries_the_user_with_the_pane_at_home_and_with_the_cell_abroad() {
        let mut reg = SessionRegistry::new((80, 24));
        let default = default_name(&reg);
        let src = spawn_into(&reg, "0", 2);
        let (a, b) = (src[0], src[1]);
        reg.new_window(&default, Some("1")).unwrap();
        let dst = spawn_into(&reg, "1", 2);
        let (c, d) = (dst[0], dst[1]);

        // Put each window ON the pane that is about to leave it.
        for (window, pane) in [("0", b), ("1", d)] {
            let panes = pool_ids(&reg, window);
            let name = default_name(&reg);
            assert!(
                reg.window_mut(&name, window)
                    .expect("the window exists")
                    .select_pane(pane, &panes)
            );
        }

        assert!(reg.swap_panes(&default, b, d).unwrap());

        let session = reg.session(&default).unwrap();
        let active = |w: &str| {
            session
                .windows()
                .iter()
                .find(|window| window.name() == w)
                .expect("the window exists")
                .active_pane()
        };
        assert_eq!(
            active("0"),
            Some(d),
            "window 0 stayed on the cell it was on"
        );
        assert_eq!(active("1"), Some(b), "and so did window 1");

        // Within ONE window nothing has to happen: the user stays on the pane they swapped.
        let mut reg = reg;
        let panes = pool_ids(&reg, "0");
        let name = default_name(&reg);
        assert!(
            reg.window_mut(&name, "0")
                .expect("the window exists")
                .select_pane(a, &panes)
        );
        assert!(reg.swap_panes(&default, a, d).unwrap());
        assert_eq!(
            reg.session(&default)
                .unwrap()
                .windows()
                .iter()
                .find(|window| window.name() == "0")
                .expect("the window exists")
                .active_pane(),
            Some(a),
            "a same-window swap leaves the user on the pane, which merely moved",
        );
        assert_eq!(
            tiled(&mut reg, "0"),
            vec![d, a],
            "and the two did trade places"
        );
        let _ = c;
    }

    /// The zoom invariant, from both directions: it names the pane the window is ON, or it is off.
    ///
    /// The first half is the reason a zoom SELECTS, the second the reason a select can END one —
    /// together they are what makes "the pane you type into is a pane you can see" a property
    /// rather than a habit. herdr keeps the same safety by RETARGETING (`ui/panes.rs:179-180` at
    /// `9a4ce5e1` paints whatever is focused), which is a coherent different feature: its zoom is a
    /// mode over the tab, sprag's is a fact about a pane.
    ///
    /// Revert-proof: drop `heal_zoom`'s call from `set_active` and the last assertion still reads
    /// `Some(b)` while the window is on `a` — a hidden pane taking every keystroke.
    #[test]
    fn a_zoom_names_the_active_pane_or_there_is_no_zoom() {
        let mut reg = SessionRegistry::new((80, 24));
        let ids = spawn_into(&reg, "0", 3);
        let (a, b, _c) = (ids[0], ids[1], ids[2]);
        let panes = pool_ids(&reg, "0");
        let window = default_window(&mut reg);
        window.reconcile_layout(&panes);

        assert_eq!(
            window.zoom_pane(b, None, &panes),
            Some(ZoomOutcome {
                zoomed: true,
                changed: true
            }),
            "no mode toggles a pane that is not zoomed ON"
        );
        assert_eq!(window.zoomed(), Some(b));
        assert_eq!(
            window.active_pane(),
            Some(b),
            "zooming a pane is also going to it — the invariant leaves no other reading",
        );

        assert!(window.select_pane(a, &panes));
        assert_eq!(
            window.zoomed(),
            None,
            "moving to another pane ends the zoom rather than dragging it along",
        );
    }

    /// A structural verb does NOT end the zoom, and that is the design: the arrangement moved, the
    /// filter did not. Because the zoom names a PANE rather than a position it rides the swap.
    ///
    /// This is the hole in herdr's set, in the other verb R284 already found one in: herdr refuses
    /// `pane.move` outright when either tab is zoomed (`PaneMoveReason::ZoomedTab`,
    /// `src/app/api/panes.rs:656` and `:708` at `9a4ce5e1`), so a zoomed tab is a tab whose panes
    /// cannot be re-arranged at all.
    #[test]
    fn a_zoom_survives_a_move_and_a_swap_and_follows_its_own_pane() {
        let mut reg = SessionRegistry::new((80, 24));
        let default = default_name(&reg);
        let ids = spawn_into(&reg, "0", 3);
        let (a, b, c) = (ids[0], ids[1], ids[2]);
        let panes = pool_ids(&reg, "0");
        assert_eq!(tiled(&mut reg, "0"), vec![a, b, c]);
        assert!(zoom(default_window(&mut reg).zoom_pane(b, None, &panes)));

        // Re-place a pane the zoom is NOT on: herdr cannot express this request at all.
        assert!(
            !reg.move_pane(&default, c, a, SplitSide::First, SplitDir::Horizontal)
                .unwrap(),
            "a within-window move closes no source window",
        );
        assert_eq!(tiled(&mut reg, "0"), vec![c, a, b], "the arrangement moved");
        assert_eq!(
            default_window(&mut reg).zoomed(),
            Some(b),
            "and the zoom did not notice, because it names a pane",
        );

        // Now trade the zoomed pane itself. It ends up somewhere else in the row, still filling
        // the window: a swap exchanges positions, and the zoom was never about a position.
        assert!(reg.swap_panes(&default, b, c).unwrap());
        assert_eq!(tiled(&mut reg, "0"), vec![b, a, c]);
        assert_eq!(default_window(&mut reg).zoomed(), Some(b));
    }

    /// The half of the invariant no verb triggers: a zoomed pane that EXITS, or that a client
    /// floats out, stops being a tiled pane of the window — and the zoom ends at the reconcile,
    /// which is the one place both of those facts are already settled.
    ///
    /// herdr writes this by hand at four sites (`src/workspace/tab.rs:414` `:483` `:505` `:527`),
    /// one per structural verb, which is four chances for a fifth verb to forget.
    ///
    /// Revert-proof: delete `heal_zoom` from `reconcile_layout` and both halves keep reading
    /// `Some(b)` — a zoom on a pane the window no longer shows.
    #[test]
    fn a_zoom_ends_when_its_pane_stops_being_tiled() {
        // A pane that exits.
        let mut reg = SessionRegistry::new((80, 24));
        let ids = spawn_into(&reg, "0", 2);
        let (a, b) = (ids[0], ids[1]);
        let panes = pool_ids(&reg, "0");
        assert!(zoom(default_window(&mut reg).zoom_pane(b, None, &panes)));
        let ws = pool(&reg);
        drop(lock(&ws).close(b));
        assert_eq!(tiled(&mut reg, "0"), vec![a]);
        assert_eq!(
            default_window(&mut reg).zoomed(),
            None,
            "the zoomed pane is gone, so the zoom is",
        );

        // A pane a client floats OUT. It is alive and can still be the active pane, but it has no
        // leaf, so there is nothing for it to fill the window with.
        let mut reg = SessionRegistry::new((80, 24));
        let ids = spawn_into(&reg, "0", 2);
        let b = ids[1];
        let panes = pool_ids(&reg, "0");
        let window = default_window(&mut reg);
        window.reconcile_layout(&panes);
        assert!(zoom(window.zoom_pane(b, None, &panes)));
        assert!(window.set_floating(b, true, &panes));
        window.reconcile_layout(&panes);
        assert_eq!(window.zoomed(), None, "a floated pane has no leaf to zoom");
        assert_eq!(
            window.active_pane(),
            Some(b),
            "but it is still a pane the user can be on — floating is not leaving",
        );
    }

    /// Both refusals, and they are the SAME refusal: a zoom acts on the tiling, so its target has
    /// to be a pane the window tiles. An id naming nothing fails that, and so does a pane a client
    /// floated out — which is `SPLIT_ACTION`'s rule and `move_pane`'s, not `swap_pane`'s edge rule.
    #[test]
    fn a_zoom_refuses_a_pane_it_cannot_fill_a_window_with() {
        let mut reg = SessionRegistry::new((80, 24));
        let ids = spawn_into(&reg, "0", 2);
        let (a, b) = (ids[0], ids[1]);
        let panes = pool_ids(&reg, "0");
        let window = default_window(&mut reg);
        window.reconcile_layout(&panes);

        assert_eq!(
            window.zoom_pane(PaneId(9999), None, &panes),
            None,
            "a pane id naming nothing is refused, and nothing moves",
        );
        assert_eq!(window.active_pane(), Some(a), "not even the active pane");

        // A FLOATED pane is the second kind of typo, not an edge: it is a real pane, but it has no
        // leaf, so it cannot fill a window any more than it can be split or placed beside. Refused
        // for `SPLIT_ACTION`'s and `move_pane`'s reason, and nothing moves — not even the active
        // pane, which an ACCEPTED-but-did-nothing reading would have moved as a side effect of a
        // request that then declined to do the thing it was for.
        assert!(window.set_floating(b, true, &panes));
        window.reconcile_layout(&panes);
        assert_eq!(
            window.zoom_pane(b, Some(true), &panes),
            None,
            "a pane with no leaf is refused, like a split's target that is not tiled",
        );
        assert_eq!(
            window.active_pane(),
            Some(a),
            "and a refusal moves nothing, so the user is where they were",
        );
    }

    /// The four answers a zoom can give are DISTINCT, which is what lets an operator-facing sentence
    /// name each one instead of listing the causes it is consistent with (R283's shape).
    ///
    /// This is the property the floating-target refusal buys, and it is asserted rather than
    /// argued: accepted, a toggle aimed at a floated pane while another was zoomed answered
    /// `{zoomed: false, changed: true}` — byte for byte what toggling a zoom OFF answers.
    #[test]
    fn the_four_zoom_answers_are_distinct() {
        let mut reg = SessionRegistry::new((80, 24));
        let ids = spawn_into(&reg, "0", 2);
        let (a, b) = (ids[0], ids[1]);
        let panes = pool_ids(&reg, "0");
        let window = default_window(&mut reg);
        window.reconcile_layout(&panes);

        let seen = [
            window.zoom_pane(b, Some(true), &panes),  // now filling
            window.zoom_pane(b, Some(true), &panes),  // already filling
            window.zoom_pane(b, Some(false), &panes), // arrangement back
            window.zoom_pane(b, Some(false), &panes), // arrangement already showing
        ];
        assert_eq!(
            seen,
            [
                Some(ZoomOutcome {
                    zoomed: true,
                    changed: true
                }),
                Some(ZoomOutcome {
                    zoomed: true,
                    changed: false
                }),
                Some(ZoomOutcome {
                    zoomed: false,
                    changed: true
                }),
                Some(ZoomOutcome {
                    zoomed: false,
                    changed: false
                }),
            ],
        );

        // The case that USED to collide with the third: a toggle at a pane with no leaf. It is a
        // refusal now, so no answer above can be read two ways.
        assert!(window.zoom_pane(b, Some(true), &panes).is_some());
        assert!(window.set_floating(a, true, &panes));
        window.reconcile_layout(&panes);
        assert_eq!(window.zoom_pane(a, None, &panes), None);
        assert_eq!(
            window.zoomed(),
            Some(b),
            "and a refused request left the zoom that was in force alone",
        );
    }

    /// A zoom moves the layout REVISION and a plain select does not — the two facts a client acts
    /// on differently. Which pane a window is ON reaches it as its own fact and re-divides nothing;
    /// which pane FILLS it decides what there is to draw at all, so a client that did not re-read
    /// would keep painting an arrangement that is no longer on screen.
    ///
    /// The float set is bumped for the weaker version of exactly this reason
    /// (`bump_if_changed`'s own docs), so this is that rule applied, not a new one.
    #[test]
    fn a_zoom_bumps_the_revision_and_a_bare_select_does_not() {
        let mut reg = SessionRegistry::new((80, 24));
        let ids = spawn_into(&reg, "0", 2);
        let (a, b) = (ids[0], ids[1]);
        let panes = pool_ids(&reg, "0");
        let window = default_window(&mut reg);
        window.reconcile_layout(&panes);

        let settled = window.layout_revision();
        assert!(window.select_pane(b, &panes));
        assert_eq!(
            window.layout_revision(),
            settled,
            "moving the user re-divides nothing",
        );

        assert!(zoom(window.zoom_pane(b, None, &panes)));
        let zoomed_at = window.layout_revision();
        assert_eq!(zoomed_at, settled + 1, "but filling the window does");

        // Ending it the two ways it can end, each once: by mode, and by going elsewhere.
        assert!(!zoom(window.zoom_pane(b, Some(false), &panes)));
        assert_eq!(window.layout_revision(), zoomed_at + 1);
        assert!(zoom(window.zoom_pane(b, Some(true), &panes)));
        assert!(window.select_pane(a, &panes));
        assert_eq!(
            window.layout_revision(),
            zoomed_at + 3,
            "a select that ENDS a zoom bumps — it is the zoom that moved, not the user",
        );
    }

    /// Naming the pane is what makes a zoom survive a reboot EXACTLY. A stored flag could only come
    /// back bound to whichever pane the restore happened to make active; an id either finds its
    /// pane or the first reconcile ends the zoom.
    ///
    /// Both halves here, because only the pair discriminates: a build that restored the flag and
    /// re-bound it would pass the first and fail the second.
    #[test]
    fn a_restored_zoom_finds_its_own_pane_or_ends() {
        let mut reg = SessionRegistry::new((80, 24));
        let ids = spawn_into(&reg, "0", 2);
        let (a, b) = (ids[0], ids[1]);
        let panes = pool_ids(&reg, "0");
        default_window(&mut reg).reconcile_layout(&panes);
        assert!(zoom(default_window(&mut reg).zoom_pane(b, None, &panes)));

        let saved = crate::snapshot::snapshot(&Arc::new(Mutex::new(reg)));
        assert_eq!(
            saved.sessions[0].windows[0].zoomed,
            Some(b),
            "the pane travels, not a flag",
        );

        // Restored with both panes back: the zoom is exactly where it was.
        let (mut back, _plan) =
            SessionRegistry::from_snapshot(saved.clone()).expect("the snapshot restores");
        let window = default_window(&mut back);
        window.reconcile_layout(&[a, b]);
        assert_eq!(window.zoomed(), Some(b));
        assert_eq!(window.active_pane(), Some(b));

        // Restored with the zoomed pane MISSING (its shell failed to come back): the first
        // reconcile ends the zoom rather than handing it to the survivor.
        let (mut orphaned, _plan) =
            SessionRegistry::from_snapshot(saved).expect("the snapshot restores");
        let window = default_window(&mut orphaned);
        window.reconcile_layout(&[a]);
        assert_eq!(window.zoomed(), None, "no pane, no zoom");
        assert_eq!(
            window.active_pane(),
            Some(a),
            "and the user lands on what is there"
        );
    }

    /// A direction is resolved in the window that HOLDS the pane, not in the session's current one.
    /// Against the current window the answer for a pane living elsewhere would be "no neighbour" —
    /// which is not conservative but FALSE, since there plainly is something beside it.
    ///
    /// Revert-proof: resolve through the current window's tree instead and the second assertion
    /// reads `None` while pane 2 is sitting right next to pane 3.
    #[test]
    fn a_direction_is_answered_by_the_window_that_holds_the_pane() {
        let mut reg = SessionRegistry::new((80, 24));
        let default = default_name(&reg);
        spawn_into(&reg, "0", 2);
        reg.new_window(&default, Some("1")).unwrap();
        let far = spawn_into(&reg, "1", 2);
        let (c, d) = (far[0], far[1]);
        // The session is CURRENT on "1"; select "0" so the far panes are in a non-current window.
        reg.select_window(&default, "0").unwrap();
        assert_eq!(tiled(&mut reg, "1"), vec![c, d]);

        assert_eq!(
            reg.step_of(&default, c, PaneDir::Right),
            Some(PaneStep::To(d)),
            "pane d is to c's right in the window that holds them both",
        );
        assert_eq!(
            reg.step_of(&default, d, PaneDir::Left),
            Some(PaneStep::To(c)),
            "and the walk runs both ways in a window nobody is currently looking at",
        );
        assert_eq!(
            reg.step_of(&default, c, PaneDir::Left),
            Some(PaneStep::Edge),
            "c is at that edge — an ANSWER, and the whole reason this is a `PaneStep`",
        );
        // ...where a pane no window of this session holds is a REFUSAL, and the two used to be one
        // `None`. A caller cannot tell "there is nothing to your left" from "you do not exist"
        // unless this says so, and `swap_pane` answered success for the second until R301.
        assert_eq!(reg.step_of(&default, PaneId(999), PaneDir::Left), None);
        assert_eq!(reg.step_of("nope", c, PaneDir::Right), None);
    }

    /// `swap-pane` refuses an unreachable pane and answers "nothing moved" for a legal request that
    /// needs no motion — the `select_pane` split between a typo and an edge.
    #[test]
    fn swap_panes_refuses_the_unreachable_and_answers_nothing_moved_for_the_idle() {
        let mut reg = SessionRegistry::new((80, 24));
        let default = default_name(&reg);
        let ids = spawn_into(&reg, "0", 2);
        let (a, b) = (ids[0], ids[1]);
        let before = tiled(&mut reg, "0");

        assert!(
            !reg.swap_panes(&default, a, a).unwrap(),
            "a pane swapped with itself is legal and moves nothing",
        );
        assert_eq!(
            reg.swap_panes(&default, a, PaneId(999)).unwrap_err(),
            PaneMoveError::UnknownPane(PaneId(999)),
        );
        assert_eq!(
            reg.swap_panes("nope", a, b).unwrap_err(),
            PaneMoveError::UnknownSession("nope".to_owned()),
        );
        assert_eq!(
            tiled(&mut reg, "0"),
            before,
            "and nothing moved on any of them"
        );
    }

    /// A window is on a pane as soon as it HAS one, and nobody had to select it. The birth path
    /// spawns into the pool and the arrangement reconciles lazily, so the first reconcile is the
    /// only place that could establish this — which is why the healing lives there rather than in
    /// each of the seven callers that create a pane.
    #[test]
    fn a_windows_first_pane_becomes_active_without_anyone_selecting_it() {
        let mut reg = SessionRegistry::new((80, 24));
        let window = default_window(&mut reg);
        assert_eq!(window.active_pane(), None, "an empty window is on no pane");

        window.reconcile_layout(&[PaneId(0), PaneId(1)]);

        assert_eq!(window.active_pane(), Some(PaneId(0)));
    }

    /// Selecting names a pane of the POOL, not of the tiling: a FLOATED pane is still a pane the
    /// user can be on. A pane the pool does not hold is refused whole — nothing moves, so a typo
    /// cannot cost the user their place.
    #[test]
    fn select_takes_any_pane_of_the_pool_including_a_floated_one_and_refuses_the_rest() {
        let mut reg = SessionRegistry::new((80, 24));
        let window = default_window(&mut reg);
        let panes = [PaneId(0), PaneId(1), PaneId(2)];
        window.reconcile_layout(&panes);
        assert!(window.set_floating(PaneId(2), true, &panes));
        // The float set collapses the leaf on the next reconcile (the tiling is reconciled over
        // `panes − floating`), so take one before reading the tiling below.
        window.reconcile_layout(&panes);

        assert!(window.select_pane(PaneId(2), &panes));
        assert_eq!(window.active_pane(), Some(PaneId(2)));
        assert!(
            !window.layout().panes().contains(&PaneId(2)),
            "THE CONTROL: pane 2 really is out of the tiling, so this is not the tiled case \
             passing by accident",
        );

        assert!(!window.select_pane(PaneId(9), &panes));
        assert_eq!(
            window.active_pane(),
            Some(PaneId(2)),
            "a refused select leaves the active pane exactly where it was",
        );
    }

    /// Closing the active pane hands off to its NEIGHBOUR in the arrangement, not to the first pane
    /// in the window. The control is the second half: closing a pane the user was NOT on moves
    /// nothing, so the successor rule cannot be mistaken for "reset on every reconcile".
    #[test]
    fn a_closed_active_pane_hands_off_to_its_neighbour_not_to_the_first() {
        let mut reg = SessionRegistry::new((80, 24));
        let window = default_window(&mut reg);
        let all = [PaneId(0), PaneId(1), PaneId(2), PaneId(3)];
        window.reconcile_layout(&all);
        assert!(window.select_pane(PaneId(2), &all));

        window.reconcile_layout(&[PaneId(0), PaneId(1), PaneId(3)]);

        assert_eq!(
            window.active_pane(),
            Some(PaneId(3)),
            "the pane AFTER it in paint order inherits — pane 0 would be the cheap answer and \
             would throw a user across the whole window",
        );

        window.reconcile_layout(&[PaneId(0), PaneId(3)]);
        assert_eq!(
            window.active_pane(),
            Some(PaneId(3)),
            "THE CONTROL: closing a pane the user was not on moves nothing",
        );

        // The LAST pane has nobody after it, so the hand-off goes backwards rather than nowhere.
        window.reconcile_layout(&[PaneId(0)]);
        assert_eq!(window.active_pane(), Some(PaneId(0)));
        window.reconcile_layout(&[]);
        assert_eq!(
            window.active_pane(),
            None,
            "and a window with no panes is on no pane, rather than on a ghost",
        );
    }

    /// Selecting is not an ARRANGEMENT change: the revision a client watches to know its projection
    /// is stale must not move, or every `select-pane -L` would make every attached client re-fetch
    /// and re-project a tiling that is byte-identical to the one it holds.
    #[test]
    fn selecting_a_pane_does_not_move_the_layout_revision() {
        let mut reg = SessionRegistry::new((80, 24));
        let window = default_window(&mut reg);
        let panes = [PaneId(0), PaneId(1)];
        window.reconcile_layout(&panes);
        let settled = window.layout_revision();

        assert!(window.select_pane(PaneId(1), &panes));

        assert_eq!(window.layout_revision(), settled);
        assert_eq!(
            window.active_pane(),
            Some(PaneId(1)),
            "THE CONTROL: the select really happened, so an unmoved revision is the claim and \
             not a no-op",
        );
    }

    /// The active pane is a DECISION, so it survives a reboot the way a pinned size does — and a
    /// decision naming a pane that did not come back is healed rather than kept as a ghost.
    #[test]
    fn the_active_pane_survives_a_snapshot_and_heals_when_its_pane_does_not_come_back() {
        let mut reg = SessionRegistry::new((80, 24));
        let default = default_name(&reg);
        let panes = [PaneId(0), PaneId(1), PaneId(2)];
        {
            let window = default_window(&mut reg);
            window.reconcile_layout(&panes);
            assert!(window.select_pane(PaneId(1), &panes));
        }
        let snap = crate::snapshot::snapshot(&Arc::new(Mutex::new(reg)));

        assert_eq!(
            snap.sessions[0].windows[0].active,
            Some(PaneId(1)),
            "the file records the pane the user was on",
        );

        let (mut back, _) = SessionRegistry::from_snapshot(snap.clone()).unwrap();
        let window = back
            .window_mut(&default, "0")
            .expect("the restored default session's window");
        window.reconcile_layout(&panes);
        assert_eq!(window.active_pane(), Some(PaneId(1)));

        // The same snapshot restored into a world where pane 1's shell failed to re-spawn.
        let (mut lossy, _) = SessionRegistry::from_snapshot(snap).unwrap();
        let window = lossy
            .window_mut(&default, "0")
            .expect("the restored default session's window");
        window.reconcile_layout(&[PaneId(0), PaneId(2)]);
        assert_eq!(
            window.active_pane(),
            Some(PaneId(2)),
            "the neighbour inherits, exactly as a live close would hand off",
        );
    }
}
