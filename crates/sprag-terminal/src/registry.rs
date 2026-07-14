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
//! A [`Window`] holds a [`Workspace`] (its panes) and a [`LayoutTree`] (how they are
//! arranged). **v1 bound:** that tree is only a boot seed for the client, not yet the
//! arrangement authority — see [`crate::layout`]. This layer is
//! deliberately pinion-free (producer concern) and keeps the plugin/control surfaces
//! speaking `Arc<Mutex<Workspace>>` — a plugin operates on a *workspace*, not a session
//! tree (Interface Segregation). The host resolves "which workspace is current" through
//! this registry and hands that one workspace down, so the surfaces above never learn
//! about sessions or windows until they must.
//!
//! ## The load-bearing invariant
//!
//! Every window's [`Workspace`] shares ONE `Arc<AtomicU64>` id counter
//! ([`Workspace::with_id_source`]), so a [`PaneId`](crate::PaneId) is unique across the
//! WHOLE registry, monotonic, and never reused. That is what lets a pane be addressed
//! by id alone regardless of which window/session holds it — the per-pane wire path
//! stays window-free, and adding windows later needs no address migration.

use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use crate::layout::LayoutTree;
use crate::workspace::Workspace;

/// One window: a named layout unit owning a pane pool and how those panes are ARRANGED.
///
/// The [`LayoutTree`] is the logical arrangement only (no pixels — see
/// [`layout`](crate::layout)); it lives here, client-independently, so that a detached
/// session CAN keep the user's layout — though the write path that would put the user's
/// intent into it is not built yet (v1 bound, see [`crate::layout`]). Membership stays
/// the [`Workspace`]'s: the arrangement self-heals against the pane set via
/// [`LayoutTree::reconcile`], since pane lifecycle runs through the workspace directly.
pub struct Window {
    name: String,
    workspace: Arc<Mutex<Workspace>>,
    layout: LayoutTree,
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

    /// How this window's panes are arranged (logical only, never pixels).
    ///
    /// May lag the pane set until [`reconcile_layout`](Self::reconcile_layout) folds in
    /// a spawn/close that went straight to the [`Workspace`] — read it through the host,
    /// which reconciles first.
    #[must_use]
    pub fn layout(&self) -> &LayoutTree {
        &self.layout
    }

    /// Self-heal the arrangement against `panes` (the workspace's live ids) and return
    /// it. The caller resolves `panes` under the WORKSPACE lock and calls this under the
    /// registry lock, so the two locks are never nested (see [`crate::layout`]).
    pub fn reconcile_layout(&mut self, panes: &[crate::PaneId]) -> &LayoutTree {
        self.layout.reconcile(panes);
        &self.layout
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
/// The SINGLE global [`PaneId`](crate::PaneId) counter is not held here separately — it
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
    use crate::{CommandBuilder, Pane};

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
