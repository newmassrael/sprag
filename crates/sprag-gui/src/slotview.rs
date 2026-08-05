//! `SlotView` — the GUI-side slot↔[`PaneId`] adapter over any [`HostClient`].
//!
//! The host (`sprag-term` or the in-process `Host`) addresses panes by [`PaneId`] — its
//! OWN stable identity, with no notion of display "slots". A "slot" is a pure GUI
//! concept: the fixed `PANE_SLOTS` `&'static` tag table ([`crate::terminal`],
//! [`MAX_PANES`] wide) and the per-slot [`Owner::cache`](pinion_core::reactive::Owner)
//! state (scroll offset, IME preedit, focus). `SlotView` is the ONE place that maps
//! between the two: it wraps ANY [`HostClient`] — the out-of-process wire client
//! (`WireHost`) OR the in-process [`Host`](sprag_host::Host) — so both stay pure
//! identity clients and the slot concept never leaks into `sprag-host`.
//!
//! ## Slot stability + reuse (live deltas — Round 2b)
//!
//! A slot is STABLE for a pane's life: [`reconcile`](SlotView::reconcile) keeps a mapped
//! `PaneId` in its slot and frees a slot only when its pane leaves the host set, so a
//! survivor's per-slot GUI state never migrates onto a different pane. A freed slot may
//! be REUSED by a later pane (the compact-slot allocator), so a reused slot's per-slot
//! GUI state — keyed by slot index in `Owner::cache`, OUTSIDE this map — MUST be reset by
//! the caller when the slot frees. `reconcile` returns a [`SlotDelta`] (the slots FREED +
//! the slots ADDED) so the caller can, on the SAME pre-view frame, reset each freed slot's
//! per-slot state + evict its dock leaf/window, and admit a dock leaf for each added slot.
//!
//! The map is behind a [`RefCell`]: `reconcile` runs each frame through the shared
//! `Rc<TerminalView>` (from the pinion `reconcile_frame` pre-view hook — the sanctioned
//! place to reconcile off-thread-producer state into the reactive graph), so it mutates
//! the mapping via UI-thread interior mutability, matching `WireHost`'s own single-thread
//! `RefCell`. Boot is just the first `reconcile` (all-added, no frees).

use std::cell::RefCell;
use std::collections::HashSet;

use pinion_core::GridBuffer;
use sprag_host::{
    HostClient, PaneAgent, PaneClipboardQuery, PaneClipboardWrite, PaneFind, PaneNotification,
    PaneScrollFacts, Project, UserConfig,
};
use sprag_input::{Modifiers, MouseInput};
use sprag_terminal::{
    Ended, LayoutSnapshot, LayoutWire, PaneDir, PaneExit, PaneId, SessionInfo, SplitDir, WindowInfo,
};
use sprag_vt::{ClipboardTarget, MouseProtocol};

use crate::terminal::MAX_PANES;

/// The membership change one [`SlotView::reconcile`] applied: the display slots FREED
/// (their pane left the host set), ascending. The Round 2b live-delta hook the caller acts
/// on — a freed slot gets its per-slot GUI state reset and its floating window dropped. A
/// steady frame yields an empty one.
///
/// An ADDED slot needs no hook, which is why none is reported (R149): a new pane appears in
/// the HOST's arrangement, so the client's projection admits its leaf on the same frame
/// ([`crate::split::sync_layout`]). A freed slot still needs one because the state it must
/// clear — scroll offset, preedit, its OS window — is this client's own, and the host knows
/// nothing about it.
pub(crate) struct SlotDelta {
    /// Slots whose pane vanished this reconcile (ascending).
    pub(crate) freed: Vec<usize>,
}

/// The GUI's display-slot mapping over a host client (see the module docs). Consumers
/// address panes by display SLOT; this translates each to the host's [`PaneId`] and
/// delegates to the wrapped [`HostClient`]. An empty slot yields each method's graceful
/// default, so a hole never panics.
pub(crate) struct SlotView {
    host: Box<dyn HostClient>,
    /// slot -> the `PaneId` occupying it (`None` = a hole). Length [`MAX_PANES`]. Behind a
    /// [`RefCell`] because [`reconcile`](Self::reconcile) mutates the mapping each frame
    /// through the shared `Rc<TerminalView>` (UI-thread only — see the module docs).
    slots: RefCell<Vec<Option<PaneId>>>,
    /// Slots freed by every [`remap`](Self::remap) since the last [`reconcile`](Self::reconcile)
    /// claimed them, ascending and without repeats.
    ///
    /// The delta is ACCUMULATED rather than returned by whoever happens to re-map, because the two
    /// are separate concerns with separate owners: any caller may need the map current (a window op
    /// does, before it can name the slot the new pane will use), but only the pre-view frame hook
    /// owns what FREEING a slot entails — dropping its floating window, resetting its per-slot
    /// reactive state. Accumulating means an extra re-map can never swallow a cleanup the way a
    /// second `reconcile` used to; the hook still sees every slot that freed.
    freed: RefCell<Vec<usize>>,
}

impl SlotView {
    /// Wrap `host` and map its current panes to slots (boot = the all-new path: host
    /// order -> contiguous slots `0..N`).
    pub(crate) fn new(host: Box<dyn HostClient>) -> Self {
        let view = Self {
            host,
            slots: RefCell::new((0..MAX_PANES).map(|_| None).collect()),
            freed: RefCell::new(Vec::new()),
        };
        let _boot = view.reconcile(); // boot is all-added, no frees; nothing to reset yet
        view
    }

    /// Re-map slots to the host's current pane set and CLAIM the accumulated membership delta —
    /// what the pre-view frame hook calls, because it is the one that owns freeing a slot.
    ///
    /// The returned [`SlotDelta`] covers every slot freed since the last claim, not merely the ones
    /// this call freed, so a [`remap`](Self::remap) between two frames cannot lose a cleanup.
    pub(crate) fn reconcile(&self) -> SlotDelta {
        self.remap();
        SlotDelta {
            freed: std::mem::take(&mut self.freed.borrow_mut()),
        }
    }

    /// Re-map slots to the host's current pane set — the ONE place slot membership
    /// changes. Frees the slot of every mapped pane no longer present, allocates the
    /// lowest free slot to each new host pane, and RECORDS the freed slots for the next
    /// [`reconcile`](Self::reconcile) to claim. No IO: the host owns the frame data,
    /// this owns only the mapping. `&self` (interior-mutable) so it runs through the shared
    /// `Rc<TerminalView>`.
    ///
    /// Separate from `reconcile` so a caller that needs the map CURRENT — a window op, which
    /// cannot name the slot the incoming pane will take until the map has moved — does not have to
    /// claim a delta it has no business acting on.
    pub(crate) fn remap(&self) {
        let host_ids = self.host.pane_ids();
        let mut slots = self.slots.borrow_mut();
        let (freed, adds, overflow) = plan_slots(&slots, &host_ids);
        for &slot in &freed {
            slots[slot] = None;
        }
        for (slot, id) in adds {
            slots[slot] = Some(id);
        }
        if !overflow.is_empty() {
            tracing::warn!(
                target: "sprag_gui::slotview",
                dropped = overflow.len(),
                cap = MAX_PANES,
                "host pane set exceeds the slot cap; extra panes not shown",
            );
        }
        if !freed.is_empty() {
            let mut pending = self.freed.borrow_mut();
            pending.extend(freed);
            pending.sort_unstable();
            pending.dedup();
        }
    }

    /// The occupied display slots, ascending — the set consumers ITERATE instead of
    /// assuming a contiguous `0..pane_count()` (a closed pane leaves a hole).
    pub(crate) fn occupied_slots(&self) -> Vec<usize> {
        self.slots
            .borrow()
            .iter()
            .enumerate()
            .filter_map(|(slot, id)| id.map(|_| slot))
            .collect()
    }

    /// Whether display slot `slot` currently holds a pane (O(1), alloc-free — the paint
    /// hot path calls it per leaf per frame).
    pub(crate) fn is_pane_occupied(&self, slot: usize) -> bool {
        self.slots.borrow().get(slot).is_some_and(Option::is_some)
    }

    /// The `PaneId` at `slot`, if occupied — the ONE slot->id resolver the delegating
    /// methods share; a hole yields each method's graceful default.
    pub(crate) fn id(&self, slot: usize) -> Option<PaneId> {
        self.slots.borrow().get(slot).copied().flatten()
    }

    /// The pane the SESSION is on, as the host mirror reports it — the daemon's active pane
    /// ([`crate::active_pane`]), or `None` for a window holding none.
    ///
    /// A mirror read, no socket call: this is on the paint path, and the client reconciles its own
    /// focus against it every frame.
    pub(crate) fn active_pane(&self) -> Option<PaneId> {
        self.host.active_pane()
    }

    /// Tell the daemon the user moved to `pane` — the publish half of [`crate::active_pane`].
    ///
    /// The answer is deliberately dropped: a refusal means the pane has left, and the next mirror
    /// read reports what IS rather than what this client hoped.
    pub(crate) fn select_pane(&self, pane: PaneId) {
        let _ = self.host.select_pane(pane);
    }

    /// Ask the daemon to move the session's active pane one step in `dir` — the DIRECTIONAL half of
    /// the same wire action, resolved against the arrangement rather than against this client's
    /// tiles (see [`HostClient::select_toward`]).
    ///
    /// The answer is dropped for [`Self::select_pane`]'s reason and one more: which pane the session
    /// is on is exactly what [`crate::active_pane`] already follows, so adopting it here would be a
    /// second path to a fact this client has one path to.
    pub(crate) fn select_toward(&self, dir: PaneDir) {
        let _ = self.host.select_toward(dir);
    }

    /// Ask the daemon to TRADE the active pane with its neighbour one step in `dir` — the swap's
    /// directional arm, resolved against the same arrangement (see [`HostClient::swap_toward`]).
    ///
    /// The answer is dropped for [`Self::select_toward`]'s reason: the daemon announces an
    /// arrangement that moved, and the layout mirror this client projects from is what re-reads it.
    /// Adopting the answer here would be a second path to a fact this client has one path to.
    pub(crate) fn swap_toward(&self, dir: PaneDir) {
        let _ = self.host.swap_toward(dir);
    }

    /// Ask the daemon to MOVE THE BOUNDARY beside the active pane `cells` cells in `dir` — see
    /// [`HostClient::resize_toward`].
    ///
    /// The answer is dropped for [`Self::select_toward`]'s reason, and the DISTANCE is sent rather
    /// than resolved here for a reason of this verb's own: a cell becomes a share against the
    /// arbitrated window, which is derived from every attached client's report and not from this
    /// one's surface.
    pub(crate) fn resize_toward(&self, dir: PaneDir, cells: u16) {
        let _ = self.host.resize_toward(dir, cells);
    }

    /// The display slot holding `pane` — the inverse of [`Self::id`], for PROJECTING
    /// host-side state that names panes by identity (the window's arrangement) onto this
    /// client's slots.
    ///
    /// `None` for a host pane this client has not admitted yet (the "renderable now"
    /// contract, e.g. a first frame not fetched). A projection then simply omits it, the
    /// same way a slot hole is omitted — never a wrong slot.
    pub(crate) fn slot_of(&self, pane: PaneId) -> Option<usize> {
        self.slots.borrow().iter().position(|id| *id == Some(pane))
    }

    /// The `PaneId` at display slot `slot`, if occupied — for UN-projecting this client's
    /// dock surface back into the host's language (the inverse direction of
    /// [`slot_of`](Self::slot_of)). `None` for a hole, which makes the surface
    /// unrepresentable rather than silently mis-addressed.
    pub(crate) fn pane_at(&self, slot: usize) -> Option<PaneId> {
        self.id(slot)
    }

    /// Write this client's settled arrangement back, adopting the host's canonical answer —
    /// the gesture-to-session-state path (see [`sprag_terminal::layout`]).
    ///
    /// `expected` is the revision the gesture was authored against; a write against an
    /// arrangement that has since moved on is refused, and the answer carries the truth.
    pub(crate) fn set_layout(&self, tree: LayoutWire, expected: u64) -> LayoutSnapshot {
        self.host.set_layout(tree, expected)
    }

    /// Take slot `slot`'s pane out of the tiling (`floating == true`) or put it back,
    /// answering with the resulting arrangement. `None` for a hole — a slot with no pane has
    /// nothing to float.
    pub(crate) fn set_floating(&self, slot: usize, floating: bool) -> Option<LayoutSnapshot> {
        self.id(slot).map(|id| self.host.set_floating(id, floating))
    }

    /// The host's LOGICAL arrangement of the current window's tiled panes, and the revision
    /// it is at — what this client PROJECTS into its dock surface (the host owns it so it
    /// survives a detach).
    ///
    /// Read every frame to notice the projection is stale; the wire client mirrors it, so
    /// this costs a lock and a clone rather than a round trip.
    pub(crate) fn layout(&self) -> LayoutSnapshot {
        self.host.layout()
    }

    /// The scoped session's windows (tmux "windows") — each name and whether it is current: the
    /// list the tab strip draws. NOT slot-mapped: windows are session-scoped, not pane-addressed,
    /// so this passes straight through (the reducer addresses windows by NAME, never by slot).
    pub(crate) fn windows(&self) -> Vec<WindowInfo> {
        self.host.windows()
    }

    /// Make the window named `name` current (tmux `select-window`) — a tab click.
    pub(crate) fn select_window(&self, name: &str) {
        self.host.select_window(name);
        self.reseed_pane_focus();
    }

    /// Walk the window RING one step, wrapping (tmux `next-window` / `previous-window`) — the
    /// `prefix n` / `prefix p` keys. The step is resolved by the daemon; this reseeds the focus for
    /// the same reason a tab click does, because the window it projects has changed.
    pub(crate) fn select_window_toward(&self, step: sprag_terminal::OrderStep) -> Option<String> {
        let landed = self.host.select_window_toward(step);
        self.reseed_pane_focus();
        landed
    }

    /// Move a window's PLACE in the session's order (tmux `move-window`), answering the window
    /// that was placed and WHAT happened.
    ///
    /// A reseed for [`select_window_toward`](Self::select_window_toward)'s reason, even though a
    /// reorder moves no pane: the daemon may resolve `None` to a window this client's mirror does
    /// not think is current, and one authority for "where the session is" is the whole discipline
    /// these window verbs keep. It costs a mirror read on a gesture a user makes by hand.
    pub(crate) fn move_window(
        &self,
        window: Option<&str>,
        place: &sprag_terminal::WindowPlace,
    ) -> Option<(String, sprag_terminal::PlaceHow)> {
        let moved = self.host.move_window(window, place);
        self.reseed_pane_focus();
        moved
    }

    /// Create + select a window, born with a shell (tmux `new-window`), returning its name — the
    /// "+" tab.
    pub(crate) fn new_window(&self) -> String {
        let name = self.host.new_window();
        self.reseed_pane_focus();
        name
    }

    /// Kill the window named `name` (tmux `kill-window`) — a tab's close affordance. The
    /// session's last window ends the session.
    pub(crate) fn kill_window(&self, name: &str) {
        self.host.kill_window(name);
        self.reseed_pane_focus();
    }

    /// The host this view reads, for a caller that needs the PROTOCOL rather than a projection.
    ///
    /// One caller: [`crate::prompt`], which hands the client's host to
    /// [`Subject::commit`](sprag_host::prompt::Subject::commit) — the shared function that carries a
    /// prompt's answer to the daemon. Delegating three renames through this view instead would put a
    /// second copy of that decision in the client, which is the one thing the shared module exists
    /// to prevent. No reseed either: a rename changes what a thing is CALLED, so this client's pane
    /// set and its focus are untouched, unlike every window verb above.
    pub(crate) fn host(&self) -> &dyn HostClient {
        self.host.as_ref()
    }

    /// Leave the keyboard on a live pane after an op that may have REPLACED this client's pane set.
    ///
    /// A window's panes belong to that window alone, so selecting another one swaps every pane this
    /// client shows — and pinion drops the focus ring the moment the focused tag stops being
    /// painted (`FocusManager::update_focusable_tags`), with nothing on the window path asking for
    /// it back. The window then arrives looking perfectly normal and simply does not answer the
    /// keyboard until the user clicks a pane. tmux always has an active pane; so must this.
    ///
    /// **Why on the ops and not on the per-frame reconcile.** pinion drains a focus request at the
    /// end of the DISPATCH that wrote it, so a request written from the paint path sits in the
    /// mailbox until the next input arrives — long enough to swallow the user's first keystroke,
    /// which is the bug in a quieter costume. Every caller of these ops is inside a dispatch (a tab
    /// click, a palette row, a keyboard chord), so a request made here is applied before the next
    /// frame.
    ///
    /// **Why [`remap`](Self::remap) first.** pinion resolves a focus request against the painted
    /// scene, and the paint that adopts the new pane set has not run. Re-mapping means the
    /// side-effect-free view pinion re-derives the enumeration from (its retry for a node this
    /// dispatch just made paintable) already paints the incoming panes, so the requested tag is one
    /// it can find. Without it the request is silently dropped whenever the target slot is a hole.
    /// The BACKSTOP for the invariant [`reseed_pane_focus`](Self::reseed_pane_focus) keeps on the
    /// ops: if NOTHING holds the keyboard and this client has a pane, ask for one. Called from the
    /// pre-view frame hook, where the slot map is already current.
    ///
    /// **Its own reason to exist** is the pane-set change that happens on the PAINT path, where
    /// there is no dispatch for an op-site re-seed to live in: the poll thread flags a session lost
    /// out of band, and [`reconcile_lost_session`](Self::reconcile_lost_session) resolves it by
    /// switching the client to another session — a total pane swap, reached through the host client
    /// directly rather than through [`switch_session`](Self::switch_session). No shell change can
    /// give that path a dispatch, so this is the only seam it has.
    ///
    /// **It no longer covers the palette path, and how that ended is worth the paragraph.** PINION-
    /// PR78 landed as pinion R1462: the dispatch tail now applies the modal batch first and the
    /// focus request last, so an op's request outranks the pop's restore. That did NOT by itself
    /// fix anything here — measured with this backstop disabled, the smoke's palette leg still timed
    /// out, 3 runs of 3, at the pin carrying the fix. The reason is that the op was not asking: a
    /// palette row closes its dialog before running its command, so the op read a ring parked on a
    /// field that no longer existed, called it `Elsewhere`, and declined ([`ring_of`] now draws that
    /// distinction). Fixing the classifier is what closes the path, and R1462 is what makes fixing
    /// it WORK — with the same fix on the previous pin the leg still failed, because the restore was
    /// drained after the request and overwrote it. Upstream removed the veto; sprag had to start
    /// asking. This method stays for the paragraph above.
    ///
    /// The cost is the ONE input event a request written from the paint path cannot beat: pinion
    /// drains the mailbox at the end of a DISPATCH, never after a paint, so the ring lands on the
    /// next event to arrive — a keystroke, a pointer move, an RPC call — instead of before it. On
    /// every path sprag CAN reach in-dispatch the op re-seeds instead and nothing is lost, which is
    /// why this is the backstop and not the mechanism.
    pub(crate) fn reseed_pane_focus_if_idle(&self) {
        if pinion_core::focus_state::focused().is_none()
            && let Some(slot) = self.occupied_slots().first().copied()
        {
            pinion_core::focus_request::request(crate::terminal::pane_tag(slot));
        }
    }

    fn reseed_pane_focus(&self) {
        self.remap();
        let focused = pinion_core::focus_state::focused();
        let ring = ring_of(
            focused.as_deref(),
            focused.as_deref().and_then(crate::terminal::pane_index_of),
            focused.as_deref().is_none_or(surface_is_up),
        );
        if let Some(slot) = reseed_target(ring, &self.occupied_slots()) {
            pinion_core::focus_request::request(crate::terminal::pane_tag(slot));
        }
    }

    /// Whether slot `slot`'s child has EXITED — the pane is still here, nothing is running in it.
    /// `false` for a hole, which is the honest answer: an empty slot has no child to have died.
    pub(crate) fn pane_is_dead(&self, slot: usize) -> bool {
        self.id(slot).is_some_and(|id| self.host.pane_is_dead(id))
    }

    /// HOW slot `slot`'s child ended, or `None` while it runs, before the host has reaped it, or
    /// for a hole. Never asked without [`pane_is_dead`](Self::pane_is_dead) first — the two are
    /// separate facts and a `None` here does not mean the pane is alive.
    pub(crate) fn pane_child_exit(&self, slot: usize) -> Option<PaneExit> {
        self.host.pane_child_exit(self.id(slot)?)
    }

    /// Create a pane in the current window (tmux `split-window`), returning whether one was born.
    ///
    /// The only write here that takes NO slot, because it addresses nothing that exists yet. Which
    /// slot the new pane lands in is not this call's answer either: the host appends it to the
    /// arrangement and the next [`reconcile`](Self::reconcile) maps it, exactly as it maps a pane a
    /// second client or a plugin created.
    pub(crate) fn new_pane(&self) -> bool {
        self.host.new_pane().is_some()
    }

    /// Divide slot `slot`'s pane and put a new shell in the half it opens (tmux `split-window -h` /
    /// `-v`), returning whether one was born — `false` for a hole, or a refusal.
    ///
    /// [`new_pane`](Self::new_pane) with a PLACE. This client can rearrange with a POINTER, so an
    /// append plus a drag says everything a direction does and the palette's `New pane` needs no
    /// direction at all. What needs one is a KEYSTROKE: `prefix %` names the arrangement it wants and
    /// there is no gesture in it, so the direction has to travel with the request.
    ///
    /// Which slot the new pane lands in is not this call's answer, exactly as `new_pane`'s is not:
    /// the host places it and the next [`reconcile`](Self::reconcile) maps it.
    pub(crate) fn split(&self, slot: usize, dir: SplitDir, before: bool) -> bool {
        self.id(slot)
            .and_then(|id| self.host.split(id, dir, before))
            .is_some()
    }

    /// Fill the window with slot `slot`'s pane alone, or give the arrangement back (tmux
    /// `resize-pane -Z`) — `on` absent toggles. Returns whether ONE pane fills the window
    /// afterwards, and `None` for a hole or a pane the daemon refused (one it has floated out).
    ///
    /// The answer is carried rather than discarded because `{zoomed, changed}` is total over four
    /// distinct cases; a caller that only wanted "did it work" would have to guess between two of
    /// them.
    pub(crate) fn zoom_pane(&self, slot: usize, on: Option<bool>) -> Option<bool> {
        let outcome = self.host.zoom_pane(self.id(slot)?, on)?;
        Some(outcome.zoomed)
    }

    /// Close slot `slot`'s pane (tmux `kill-pane`), answering how far the kill CASCADED — [`None`]
    /// for a hole, or when the daemon refused. DESTRUCTIVE: the pane's child is killed and its
    /// scrollback goes with it, and its window goes too when it was the last pane
    /// ([`Ended`]). The asking happens above, in [`confirm`](crate::confirm); this is the
    /// performer.
    pub(crate) fn close_pane(&self, slot: usize) -> Option<Ended> {
        self.host.kill_pane(self.id(slot)?)
    }

    /// Break slot `slot`'s pane out into a NEW window (tmux `break-pane`), returning the new
    /// window's name — `None` for a hole, or when the daemon refuses (the pane is its window's
    /// only one). Slot-mapped: the reducer knows the pane the user acted on by its slot, and the
    /// host addresses it by [`PaneId`].
    pub(crate) fn break_pane(&self, slot: usize, name: Option<&str>) -> Option<String> {
        self.host.break_pane(self.id(slot)?, name)
    }

    /// Move slot `slot`'s pane into the window named `dst` (tmux `join-pane`), returning whether the
    /// source window was closed — `None` for a hole or a refusal.
    pub(crate) fn join_pane(&self, slot: usize, dst: &str) -> Option<bool> {
        self.host.join_pane(self.id(slot)?, dst)
    }

    /// The project governing slot `slot`'s pane — the commands its `.sprag.toml` declares. `None`
    /// for a hole, a pane in no project, or a remote pane (whose cwd is on another machine).
    /// `Some(Err(message))` is a project whose config is unusable, whose message already names the
    /// file to fix — rendered host-side, like the user config's beside it, so this client never has
    /// to guess which file a report is about.
    ///
    /// Slot-mapped like every other read here, and asked ON DEMAND (the palette opening) because the
    /// answer costs the host a filesystem walk and this client a socket round trip.
    pub(crate) fn project(&self, slot: usize) -> Option<Result<Project, String>> {
        self.host.project(self.id(slot)?)
    }

    /// The USER's own declared commands — offered in every pane, whatever project it is in (or none).
    /// `None` when no config has been written; `Some(Err(message))` a config that is unusable, whose
    /// message already names the file to fix.
    ///
    /// The one read here that takes NO slot, because the answer depends on no pane.
    pub(crate) fn global_commands(&self) -> Option<Result<UserConfig, String>> {
        self.host.global_commands()
    }

    /// Why the daemon's agent manifests are not the ones the user's `config.toml` declares — `None`
    /// when they are, or when this host detects no agents at all. Already worded to name the file,
    /// like the two reads above.
    ///
    /// Takes no slot for a stronger reason than [`global_commands`](Self::global_commands) does: the
    /// ruleset is the DAEMON's, so a broken block is not a fact about any pane on this window.
    pub(crate) fn agent_manifest_report(&self) -> Option<String> {
        self.host.agent_manifest_report()
    }

    /// Deliver a file dropped on the window to slot `slot`'s pane, returning the path the pane was
    /// handed — `None` for a hole, or when the host refuses the file. The host decides whether that
    /// means pasting the local path or uploading to a remote workspace first; this end only says
    /// WHICH pane the drop landed on.
    pub(crate) fn drop_file(&self, slot: usize, path: &str) -> Option<String> {
        self.host.drop_file(self.id(slot)?, path)
    }

    /// Slot `slot`'s pane's search matches for `needle` — the find bar's read. `None`-safe for a
    /// hole (an empty result), like every other slot-mapped read.
    pub(crate) fn pane_find(&self, slot: usize, needle: &str) -> PaneFind {
        self.id(slot)
            .map(|id| self.host.pane_find(id, needle))
            .unwrap_or_default()
    }

    /// Slot `slot`'s pane's REGEX matches for `pattern` — the find bar's read in the other search
    /// LANGUAGE. A distinct method, not a mode on [`Self::pane_find`], all the way down to the wire
    /// address: the same characters mean different things in the two languages, so which one is being
    /// asked must be carried by the call itself.
    pub(crate) fn pane_find_regex(&self, slot: usize, pattern: &str) -> PaneFind {
        self.id(slot)
            .map(|id| self.host.pane_find_regex(id, pattern))
            .unwrap_or_default()
    }

    /// Every session on the host (registry-wide) — the list the session sidebar draws. NOT
    /// slot-mapped: sessions are not pane-addressed, so this passes straight through (the reducer
    /// addresses sessions by NAME, like it does windows).
    pub(crate) fn sessions(&self) -> Vec<SessionInfo> {
        self.host.sessions()
    }

    /// Every session's live ACTIVITY — the sidebar's subtitle facts, and the ROWS ONLY: the reading's
    /// age is dropped here, deliberately.
    ///
    /// The rail paints no staleness yet, and handing the paint path a number it does not use would
    /// invite a later reader to think it does. The age is on the wire and on
    /// [`HostClient::session_activity`] for whoever needs
    /// it; this accessor states exactly what the sidebar consumes.
    ///
    /// NOT joined onto [`sessions`](Self::sessions) here: the two are separate answers, and a caller
    /// that needs both joins them by NAME (a session's address) rather than by position, which the
    /// sidebar does.
    pub(crate) fn session_activity(&self) -> Vec<sprag_terminal::SessionActivity> {
        self.host.session_activity().value
    }

    /// The session this client is attached to — for the sidebar's current-row highlight.
    pub(crate) fn current_session(&self) -> String {
        self.host.current_session()
    }

    /// Switch this client to the session named `name` in place (tmux `switch-client`) — a sidebar
    /// row click.
    pub(crate) fn switch_session(&self, name: &str) {
        self.host.switch_session(name);
        self.reseed_pane_focus();
    }

    /// Create a fresh session and switch to it (tmux `new-session`), returning its name — the "+"
    /// of the session sidebar.
    pub(crate) fn new_session(&self) -> String {
        let name = self.host.new_session();
        self.reseed_pane_focus();
        name
    }

    /// Kill the session named `name` (tmux `kill-session`) — a sidebar row's "×" close affordance.
    /// Killing this client's OWN attached session detaches the client; killing another leaves this
    /// one serving. NOT slot-mapped: sessions are addressed by NAME, like the other session ops.
    pub(crate) fn kill_session(&self, name: &str) {
        self.host.kill_session(name);
        self.reseed_pane_focus();
    }

    /// Resolve a session lost OUT OF BAND (killed by another client / the CLI) against the
    /// `detach-on-destroy` policy — switch-to-next or detach. A pre-view reconcile passthrough,
    /// like [`sessions`](Self::sessions), addressed by no slot: the wire client flags the loss on
    /// its poll thread and this runs the UI-thread switch. A no-op for the in-process host.
    pub(crate) fn reconcile_lost_session(&self) {
        self.host.reconcile_lost_session();
    }

    /// Switch to the LAST session — the most-recent OTHER session this client visited that is still
    /// live (tmux `switch-client -l`), a `Ctrl+Shift+L` keyboard affordance. A no-op for the
    /// in-process host (no visit history).
    pub(crate) fn switch_to_last_session(&self) {
        self.host.switch_to_last_session();
        self.reseed_pane_focus();
    }

    /// Slot `slot`'s cell DATA at `offset_lines` (a `1x1` placeholder for a hole).
    pub(crate) fn pane_cells(&self, slot: usize, offset_lines: usize) -> GridBuffer {
        self.id(slot).map_or_else(
            || GridBuffer::new(1, 1),
            |id| self.host.pane_cells(id, offset_lines),
        )
    }

    /// Slot `slot`'s per-frame scroll facts (a zero-depth default for a hole).
    pub(crate) fn pane_scroll_facts(&self, slot: usize) -> PaneScrollFacts {
        self.id(slot).map_or(
            PaneScrollFacts {
                scrollback_len: 0,
                visible_rows: 1,
            },
            |id| self.host.pane_scroll_facts(id),
        )
    }

    /// Slot `slot`'s OSC 133 prompt-mark positions (the jump-to-prompt targets), empty for a
    /// hole. On demand (a keyboard jump), never per frame.
    pub(crate) fn pane_prompt_positions(&self, slot: usize) -> Vec<usize> {
        self.id(slot)
            .map_or_else(Vec::new, |id| self.host.pane_prompt_positions(id))
    }

    /// Slot `slot`'s grid `(cols, rows)` (`(1, 1)` for a hole).
    pub(crate) fn pane_grid_size(&self, slot: usize) -> (u16, u16) {
        self.id(slot)
            .map_or((1, 1), |id| self.host.pane_grid_size(id))
    }

    /// Resize slot `slot`'s pane (a no-op for a hole). `cell_px` is the display's
    /// `(cell_width, cell_height)` in logical pixels, forwarded so the PTY winsize is truthful.
    pub(crate) fn resize(&self, slot: usize, cols: u16, rows: u16, cell_px: (u16, u16)) {
        if let Some(id) = self.id(slot) {
            self.host.resize(id, cols, rows, cell_px);
        }
    }

    /// Tell the host how many CELLS this client has to give the session's arrangement — the input
    /// its `window-size` policy arbitrates over ([`crate::reflow`]).
    ///
    /// Not a slot's business, so it takes none: the report is about this CLIENT, and the number is
    /// folded from every tiled pane's measured surface rather than read off any one of them.
    pub(crate) fn report_client_size(&self, cols: u16, rows: u16) {
        self.host.report_client_size(cols, rows);
    }

    /// The session's arbitrated window in cells, or `None` when nothing has been arbitrated — an
    /// in-process host (one surface) or a daemon no client has reported an area to yet.
    pub(crate) fn window_size(&self) -> Option<(u16, u16)> {
        self.host.window_size()
    }

    /// Send a key to slot `slot`'s pane; `false` for a hole / unencodable / failed send.
    #[must_use]
    pub(crate) fn send_key(&self, slot: usize, key: &str, mods: Modifiers) -> bool {
        self.id(slot)
            .is_some_and(|id| self.host.send_key(id, key, mods))
    }

    /// Write committed text to slot `slot`'s pane; `false` for a hole / failed send.
    #[must_use]
    pub(crate) fn send_text(&self, slot: usize, text: &str) -> bool {
        self.id(slot)
            .is_some_and(|id| self.host.send_text(id, text))
    }

    /// PASTE text into slot `slot`'s pane — bracketed at the host when the child enabled DEC
    /// private mode 2004, raw otherwise. `false` for a hole / failed send. Distinct from
    /// [`Self::send_text`] so only a clipboard paste is bracketed, never typed / IME text.
    #[must_use]
    pub(crate) fn paste(&self, slot: usize, text: &str) -> bool {
        self.id(slot).is_some_and(|id| self.host.paste(id, text))
    }

    /// REPORT a mouse `event` to slot `slot`'s pane — the host gates it against the pane's live
    /// tracking mode and encodes an X10 / SGR report at the PTY boundary. `false` for a hole /
    /// failed send (an event the mode does not want is a legitimate `true` no-op host-side).
    #[must_use]
    pub(crate) fn mouse(&self, slot: usize, event: MouseInput) -> bool {
        self.id(slot).is_some_and(|id| self.host.mouse(id, event))
    }

    /// REPORT a pane FOCUS change to slot `slot`'s pane — the host sends `ESC [ I` / `ESC [ O` when
    /// the child enabled DEC 1004, a no-op otherwise. `false` for a hole. Called on the focus edge
    /// (the newly-focused pane gets `true`, the pane it left gets `false`).
    #[must_use]
    pub(crate) fn focus(&self, slot: usize, focused: bool) -> bool {
        self.id(slot).is_some_and(|id| self.host.focus(id, focused))
    }

    /// Whether slot `slot`'s pane has a mouse-tracking mode active — the pane pointer oracle's
    /// per-frame capture gate. `false` for a hole.
    #[must_use]
    pub(crate) fn pane_mouse_active(&self, slot: usize) -> bool {
        self.id(slot)
            .is_some_and(|id| self.host.pane_mouse_active(id))
    }

    /// Slot `slot`'s live mouse-tracking protocol LEVEL — the pane pointer oracle reads it per frame
    /// to gate capture AND, from the level, whether to forward drag / bare motion. `None` for a hole.
    #[must_use]
    pub(crate) fn pane_mouse_protocol(&self, slot: usize) -> MouseProtocol {
        self.id(slot)
            .map_or(MouseProtocol::None, |id| self.host.pane_mouse_protocol(id))
    }

    /// Slot `slot`'s full text (empty for a hole).
    pub(crate) fn pane_full_text(&self, slot: usize) -> String {
        self.id(slot)
            .map(|id| self.host.pane_full_text(id))
            .unwrap_or_default()
    }

    /// Slot `slot`'s command label (empty for a hole).
    pub(crate) fn pane_command_label(&self, slot: usize) -> String {
        self.id(slot)
            .map(|id| self.host.pane_command_label(id))
            .unwrap_or_default()
    }

    /// Slot `slot`'s child-reported window title (`OSC 0`/`OSC 2`), `None` for a hole or
    /// a child that has set none. A DISPLAY name only — see [`HostClient::pane_title`].
    pub(crate) fn pane_title(&self, slot: usize) -> Option<String> {
        self.id(slot).and_then(|id| self.host.pane_title(id))
    }

    /// Slot `slot`'s operator-given NAME, `None` for a hole or a pane nobody named. Preferred OVER
    /// the title by every display surface — see [`HostClient::pane_name`].
    pub(crate) fn pane_name(&self, slot: usize) -> Option<String> {
        self.id(slot).and_then(|id| self.host.pane_name(id))
    }

    /// Slot `slot`'s most recent attention notification (`OSC 9` / `OSC 777;notify` / `OSC 99`),
    /// `None` for a hole or a pane that raised none. A DISPLAY signal — the GUI's per-pane
    /// attention marker reads its [`seq`](PaneNotification::seq) against the acked one (see
    /// [`crate::attention`]).
    pub(crate) fn pane_notification(&self, slot: usize) -> Option<PaneNotification> {
        self.id(slot).and_then(|id| self.host.pane_notification(id))
    }

    /// What the AGENT in slot `slot` is doing (H3), `None` for a hole or a pane no manifest claims.
    ///
    /// A DISPLAY fact like [`pane_title`](Self::pane_title), and read by the title SSOT
    /// ([`crate::view::pane_display_title`]) so one verdict reaches every surface a title reaches —
    /// the dock header, the tab label, a floater's OS title, and the a11y name. Never identity: the
    /// state moves under a pane that keeps its address.
    pub(crate) fn pane_agent(&self, slot: usize) -> Option<PaneAgent> {
        self.id(slot).and_then(|id| self.host.pane_agent(id))
    }

    /// Slot `slot`'s tmux monitor-bell count (`\a`), `0` for a hole or a pane that rang none. A
    /// DISPLAY signal the attention marker combines with the notification `seq` (see
    /// [`crate::attention`]) — kept SEPARATE because a bell carries no text.
    pub(crate) fn pane_bell_seq(&self, slot: usize) -> u64 {
        self.id(slot).map_or(0, |id| self.host.pane_bell_seq(id))
    }

    /// Slot `slot`'s inline image SUMMARIES (`{id,width,height,anchor,seq}`, RGBA empty — R1404
    /// Stage 5), empty for a hole or a pane with none. The GUI composites each over the pane grid at
    /// its anchor cell and fetches the RGBA on demand via [`Self::pane_image_rgba`] (see
    /// [`crate::view`]).
    pub(crate) fn pane_images(&self, slot: usize) -> Vec<sprag_vt::Image> {
        self.id(slot)
            .map(|id| self.host.pane_images(id))
            .unwrap_or_default()
    }

    /// Slot `slot`'s image `image_id` RGBA, fetched ON DEMAND (R1404 Stage 5) — `None` for a hole or
    /// an id the pane no longer shows. The compositor calls this once per `(id, seq)` change.
    pub(crate) fn pane_image_rgba(&self, slot: usize, image_id: u32) -> Option<Vec<u8>> {
        self.id(slot)
            .and_then(|id| self.host.pane_image_rgba(id, image_id))
    }

    /// Slot `slot`'s cheap OSC 52 clipboard-WRITE count (`0` for a hole) — [`crate::clipboard_osc`]
    /// polls it each frame and fetches the payload only when it grows.
    pub(crate) fn pane_clipboard_write_seq(&self, slot: usize) -> u64 {
        self.id(slot)
            .map_or(0, |id| self.host.pane_clipboard_write_seq(id))
    }

    /// Slot `slot`'s pending OSC 52 read query (selection + seq), `None` for a hole or no query.
    pub(crate) fn pane_clipboard_query(&self, slot: usize) -> Option<PaneClipboardQuery> {
        self.id(slot)
            .and_then(|id| self.host.pane_clipboard_query(id))
    }

    /// Slot `slot`'s most recent OSC 52 clipboard WRITE payload (targets + text + seq), fetched ON
    /// DEMAND. `None` for a hole or no write.
    pub(crate) fn pane_clipboard_write(&self, slot: usize) -> Option<PaneClipboardWrite> {
        self.id(slot)
            .and_then(|id| self.host.pane_clipboard_write(id))
    }

    /// Answer slot `slot`'s pending OSC 52 read query `seq` with `text` for `target`; `true` if
    /// THIS client's reply reached the PTY (the host arbitrates exactly-once across clients).
    /// `false` for a hole.
    #[must_use]
    pub(crate) fn answer_clipboard_query(
        &self,
        slot: usize,
        seq: u64,
        target: ClipboardTarget,
        text: &str,
    ) -> bool {
        self.id(slot)
            .is_some_and(|id| self.host.answer_clipboard_query(id, seq, target, text))
    }
}

/// Where this client's keyboard focus ring sits when a window / session op finishes — the input to
/// [`reseed_target`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ring {
    /// On nothing at all: pinion dropped it when the tag it was on stopped being painted, or it was
    /// never seeded (a client that has not been typed into yet).
    Nowhere,
    /// On the pane tile at this display slot.
    Pane(usize),
    /// On some other widget of this client that is STILL UP — the find bar's field, the session
    /// rail. A pane is not what has the keyboard, and something the user is using still wants it.
    Elsewhere,
}

/// Classify the focus ring for [`reseed_target`] from the tag that holds it, the pane slot that tag
/// names (if any), and whether the surface owning it is still up.
///
/// Split out from [`SlotView::reseed_pane_focus`] and given the third argument as DATA because the
/// distinction it draws is the whole point, and it was wrong for as long as this code existed:
/// `Elsewhere` used to mean nothing more than "the tag is not a pane", which quietly fused two
/// different facts — *a live widget holds the caret* (decline; yanking it mid-search is a bug) and
/// *the tag is not a pane* (which a dismissed modal also satisfies, one frame before pinion notices).
///
/// The palette is the case that exposed it. A row CLOSES the palette and then runs its command in
/// the same dispatch, so a window op reads a ring still parked on `sprag_palette_query` — a field
/// that stops being painted at the very next frame. Reading that as `Elsewhere` made the op decline
/// to ask for anything, so the window changed with no focus request in flight at all, and the client
/// came up answering no keys. MEASURED, not reasoned: with the frame-hook backstop disabled the live
/// smoke's palette leg timed out 3 runs out of 3, and the probe that found it read
/// `ring=Elsewhere occupied=[0] target=None` on exactly that op.
fn ring_of(focused: Option<&str>, pane: Option<usize>, holder_is_up: bool) -> Ring {
    match (focused, pane) {
        (None, _) => Ring::Nowhere,
        (Some(_), Some(slot)) => Ring::Pane(slot),
        // A ring on a surface that has already gone is not a caret to protect. `Nowhere` rather than
        // a new variant: it IS nowhere — pinion drops the ring to `None` at the next paint, and this
        // only declines to wait for that paint to say so.
        (Some(_), None) if !holder_is_up => Ring::Nowhere,
        (Some(_), None) => Ring::Elsewhere,
    }
}

/// Whether the client surface that owns `tag` is still up.
///
/// Only the palette is asked, because it is the only surface that dismisses itself in the same
/// dispatch as the op that reads the ring — `run_cursor_row` closes and then activates, deliberately
/// ([`crate::palette`] documents why the order is that way round). The find bar and the session rail
/// are still on screen when a window op runs, so they answer `true` here by falling through, which
/// is the same answer this predicate would give if it knew about them. A tag this does not recognise
/// is therefore treated as a LIVE holder — the conservative arm, since declining to re-seed leaves
/// the keyboard where the user put it.
fn surface_is_up(tag: &str) -> bool {
    !crate::palette::is_palette_focus(tag) || crate::palette::is_open()
}

/// The display slot to RE-SEED the focus ring on after an op that may have replaced this client's
/// pane set, or `None` to leave the ring where it is.
///
/// `occupied` is the slot set the op left behind (ascending), so the answer is the client's first
/// live pane — the seat a swapped-in window fills first, and the same one the boot seed uses.
///
/// Re-seeds in exactly two cases: the ring is on NOTHING (pinion dropped it, or nothing ever
/// seeded it), or it was on a pane the op did not leave standing. It deliberately does NOT re-seed
/// when the ring sits `Elsewhere` — switching windows is no reason to yank the caret out of the
/// find bar mid-search, and a client with no panes left has nothing to offer either.
fn reseed_target(ring: Ring, occupied: &[usize]) -> Option<usize> {
    match ring {
        Ring::Elsewhere => None,
        Ring::Pane(slot) if occupied.contains(&slot) => None,
        Ring::Nowhere | Ring::Pane(_) => occupied.first().copied(),
    }
}

/// The PURE slot-allocation plan behind [`SlotView::reconcile`] (so the allocator is
/// unit-tested without a host): from each slot's current occupant (`None` = a hole) and
/// the host's live id list (host order), compute the slots to FREE (occupant vanished),
/// the `(slot, id)` ADDS (a host id with no slot yet, placed at the LOWEST free slot —
/// reusing a slot freed in this same plan, so slot usage stays compact), and the OVERFLOW
/// ids (a host id past the [`MAX_PANES`] slot cap — the ONE place the cap is decided). A
/// survivor (an id still present) keeps its existing slot and appears in none of the
/// three lists. Written against the delta case so Round 2b feeds it with no rework; boot
/// exercises only its all-new path (contiguous `0..N`).
fn plan_slots(
    current: &[Option<PaneId>],
    host_ids: &[PaneId],
) -> (Vec<usize>, Vec<(usize, PaneId)>, Vec<PaneId>) {
    let live: HashSet<PaneId> = host_ids.iter().copied().collect();
    let mut taken: Vec<bool> = current.iter().map(Option::is_some).collect();
    let mut frees = Vec::new();
    for (slot, occupant) in current.iter().enumerate() {
        if let Some(id) = occupant
            && !live.contains(id)
        {
            frees.push(slot);
            taken[slot] = false; // available for an add below (hole reuse)
        }
    }
    let survivors: HashSet<PaneId> = current
        .iter()
        .flatten()
        .copied()
        .filter(|id| live.contains(id))
        .collect();
    let mut adds = Vec::new();
    let mut overflow = Vec::new();
    for &id in host_ids {
        if survivors.contains(&id) {
            continue; // keeps its existing slot
        }
        if let Some(free) = taken.iter().position(|slot_taken| !slot_taken) {
            taken[free] = true;
            adds.push((free, id));
        } else {
            overflow.push(id); // no free slot (host set > MAX_PANES)
        }
    }
    (frees, adds, overflow)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pinion_core::reactive::Owner;

    fn pid(n: u64) -> PaneId {
        PaneId(n)
    }

    #[test]
    fn plan_slots_boot_is_contiguous_from_empty() {
        // Boot = the all-new path: an empty map + host ids -> contiguous slots 0..N in
        // host order, no frees, no overflow.
        let (frees, adds, overflow) =
            plan_slots(&[None, None, None, None], &[pid(10), pid(11), pid(12)]);
        assert!(frees.is_empty());
        assert_eq!(adds, vec![(0, pid(10)), (1, pid(11)), (2, pid(12))]);
        assert!(overflow.is_empty());
    }

    #[test]
    fn plan_slots_survivors_keep_their_slots() {
        // Ids already mapped and still live keep their slots (neither freed nor re-added),
        // so no per-slot GUI state migrates.
        let (frees, adds, overflow) = plan_slots(
            &[Some(pid(10)), Some(pid(11)), None, None],
            &[pid(10), pid(11)],
        );
        assert!(frees.is_empty());
        assert!(adds.is_empty());
        assert!(overflow.is_empty());
    }

    #[test]
    fn plan_slots_frees_a_closed_pane_and_reuses_the_hole() {
        // Pane at slot 1 closed, a new pane (20) appeared: slot 1 frees, the survivors (10,
        // 12) keep slots 0 and 2, and the newcomer takes the LOWEST free slot — the reused
        // hole at slot 1 — so slot usage stays compact.
        let (frees, adds, overflow) = plan_slots(
            &[Some(pid(10)), Some(pid(11)), Some(pid(12)), None],
            &[pid(10), pid(12), pid(20)],
        );
        assert_eq!(frees, vec![1]);
        assert_eq!(adds, vec![(1, pid(20))]);
        assert!(overflow.is_empty());
    }

    #[test]
    fn plan_slots_drops_ids_past_the_slot_cap() {
        // A full map (no holes) with an extra host id: the newcomer gets NO slot (absent
        // from adds, present in overflow by its exact id) — the honest MAX_PANES bound.
        let full: Vec<Option<PaneId>> = (0..MAX_PANES as u64).map(|n| Some(pid(n))).collect();
        let mut host: Vec<PaneId> = (0..MAX_PANES as u64).map(pid).collect();
        host.push(pid(999));
        let (frees, adds, overflow) = plan_slots(&full, &host);
        assert!(frees.is_empty());
        assert!(adds.is_empty(), "no free slot -> the extra id is dropped");
        assert_eq!(
            overflow,
            vec![pid(999)],
            "the specific overflowed id is reported"
        );
    }

    #[test]
    fn a_remap_between_frames_does_not_swallow_the_freed_slot() {
        // The delta belongs to whoever OWNS freeing a slot (dropping its floating window, resetting
        // its per-slot state), not to whoever happened to move the map. A window op re-maps mid
        // dispatch so it can name the incoming pane's slot; the frame hook must still be told what
        // freed, or a torn-off window outlives the pane it showed.
        //
        // REVERT-PROOF: have `reconcile` compute its own frees instead of claiming the accumulator
        // and this reports an empty delta — the shape the bug takes.
        let ids = std::rc::Rc::new(RefCell::new(vec![pid(10), pid(11)]));
        let view = view_over(&ids);
        assert_eq!(view.occupied_slots(), vec![0, 1]);

        *ids.borrow_mut() = vec![pid(10)];
        view.remap(); // the window op's re-map — it claims nothing
        assert_eq!(view.occupied_slots(), vec![0], "the map moved");

        let delta = view.reconcile(); // the frame hook, one paint later
        assert_eq!(
            delta.freed,
            vec![1],
            "the frame hook is still told slot 1 freed",
        );
        assert!(
            view.reconcile().freed.is_empty(),
            "and claiming it once is enough — a steady frame reports nothing",
        );
    }

    #[test]
    fn a_swapped_out_pane_hands_the_ring_to_the_first_live_one() {
        // The window-change case: the ring was on slot 2, whose pane left with its window, and the
        // incoming window filled slots 0..1. The ring goes to the first live pane, not nowhere.
        assert_eq!(reseed_target(Ring::Pane(2), &[0, 1]), Some(0));
    }

    #[test]
    fn a_ring_on_nothing_is_seeded() {
        // The rescue case, and the reason the predicate reads the ring rather than only comparing
        // pane sets: pinion has ALREADY dropped focus by the time some paths get here, so there is
        // no pane left to notice the loss of.
        assert_eq!(reseed_target(Ring::Nowhere, &[1, 3]), Some(1));
    }

    #[test]
    fn a_surviving_pane_keeps_the_ring() {
        // Killing some OTHER window leaves this client's panes alone; moving the ring off the pane
        // the user is typing in would be a bug of its own.
        assert_eq!(reseed_target(Ring::Pane(1), &[0, 1, 2]), None);
    }

    #[test]
    fn the_find_bar_keeps_the_ring_through_a_window_change() {
        // `Elsewhere` is not a pane that vanished — it is the find field or the session rail, STILL
        // UP. Seeding here would yank the caret out mid-search. The palette's query field used to be
        // named in this list too, and that was the defect `ring_of` fixes: a palette that has closed
        // is not a widget with a claim on the keyboard.
        assert_eq!(reseed_target(Ring::Elsewhere, &[0, 1]), None);
    }

    #[test]
    fn a_ring_on_a_dismissed_surface_is_nowhere_not_elsewhere() {
        // THE case the live smoke measured: a palette row closes the palette and runs a window op in
        // one dispatch, so the op reads a ring on a field that is already gone. Classified
        // `Elsewhere` the op asks for nothing and the new window answers no keys.
        assert_eq!(
            ring_of(Some("sprag_palette_query"), None, false),
            Ring::Nowhere
        );
        // ...and the rescue is real: `Nowhere` is what makes the op name a pane.
        assert_eq!(
            reseed_target(ring_of(Some("sprag_palette_query"), None, false), &[0]),
            Some(0)
        );
    }

    #[test]
    fn a_ring_on_a_live_surface_is_left_alone() {
        // The find bar mid-search: same shape as above — not a pane, focus held — and the opposite
        // answer, which is why the holder's liveness is an argument rather than an assumption.
        assert_eq!(
            ring_of(Some("sprag_find_query"), None, true),
            Ring::Elsewhere
        );
    }

    #[test]
    fn a_ring_on_a_pane_names_its_slot_whatever_the_holder_says() {
        // A pane IS the holder, so the third argument cannot reach this arm — asserted so a future
        // edit that reorders the match has to notice.
        assert_eq!(
            ring_of(Some("sprag_gui.pane.2"), Some(2), true),
            Ring::Pane(2)
        );
        assert_eq!(
            ring_of(Some("sprag_gui.pane.2"), Some(2), false),
            Ring::Pane(2)
        );
    }

    #[test]
    fn no_focus_at_all_is_nowhere() {
        assert_eq!(ring_of(None, None, true), Ring::Nowhere);
    }

    #[test]
    fn a_client_with_no_panes_left_seeds_nothing() {
        // The last window of a session closing: there is no pane to hand the keyboard to, and
        // naming one anyway would request a tag nothing paints.
        assert_eq!(reseed_target(Ring::Nowhere, &[]), None);
        assert_eq!(reseed_target(Ring::Pane(0), &[]), None);
    }

    /// A [`HostClient`] whose pane-id list the test controls (shared via `Rc<RefCell<..>>`),
    /// so a live `reconcile` delta is driven without a real host. Every other method returns
    /// its graceful default — the slot map / delta logic reads only `pane_ids` — except
    /// `pane_title`, which serves `titles` so the R128 display-title policy is testable.
    struct FakeHost {
        ids: std::rc::Rc<RefCell<Vec<PaneId>>>,
        /// Per-pane-id OSC title, for the `pane_title` / display-title tests.
        titles: std::collections::HashMap<PaneId, String>,
        /// Per-pane-id attention notification (STATIC), for a fixed-notification test.
        notifications: std::collections::HashMap<PaneId, PaneNotification>,
        /// A SHARED, mutable notification map the test still holds after construction, so it can
        /// raise a NEW notification mid-run (the live-mirror `seq`-grows case). Takes precedence
        /// over `notifications` when present.
        notes: Option<std::rc::Rc<RefCell<std::collections::HashMap<PaneId, PaneNotification>>>>,
        /// Per-pane-id BELL count (tmux monitor-bell), for the bell-drives-the-marker test.
        bells: std::collections::HashMap<PaneId, u64>,
        /// The pane ids whose child has EXITED, for the display-title / liveness tests.
        dead: std::collections::HashSet<PaneId>,
        /// Per-pane-id exit STATUS, for the title tests that need a code. Deliberately independent
        /// of `dead` so a test can build the real "died but not yet reaped" state — the one a
        /// combined field would make unrepresentable.
        /// Per-pane-id AGENT verdict (H3), for the display-title / a11y tests. A pane
        /// absent from the map is one no manifest claims, which is what the wire's absent
        /// `agent` key means and what a title must render as silence.
        agents: std::collections::HashMap<PaneId, PaneAgent>,
        exits: std::collections::HashMap<PaneId, PaneExit>,
    }

    impl HostClient for FakeHost {
        fn pane_ids(&self) -> Vec<PaneId> {
            self.ids.borrow().clone()
        }
        fn pane_is_dead(&self, id: PaneId) -> bool {
            self.dead.contains(&id)
        }
        fn pane_child_exit(&self, id: PaneId) -> Option<PaneExit> {
            self.exits.get(&id).cloned()
        }
        /// Inert: these tests drive the slot map / delta logic, which reads only
        /// `pane_ids` — the arrangement is a separate authority.
        fn layout(&self) -> LayoutSnapshot {
            LayoutSnapshot::default()
        }
        fn set_layout(&self, _tree: LayoutWire, _expected: u64) -> LayoutSnapshot {
            LayoutSnapshot::default()
        }
        fn set_floating(&self, _id: PaneId, _floating: bool) -> LayoutSnapshot {
            LayoutSnapshot::default()
        }
        fn pane_cells(&self, _id: PaneId, _offset_lines: usize) -> GridBuffer {
            GridBuffer::new(1, 1)
        }
        fn pane_scroll_facts(&self, _id: PaneId) -> PaneScrollFacts {
            PaneScrollFacts {
                scrollback_len: 0,
                visible_rows: 1,
            }
        }
        fn pane_prompt_positions(&self, _id: PaneId) -> Vec<usize> {
            Vec::new()
        }
        fn pane_grid_size(&self, _id: PaneId) -> (u16, u16) {
            (1, 1)
        }
        fn resize(&self, _id: PaneId, _cols: u16, _rows: u16, _cell_px: (u16, u16)) {}
        fn send_key(&self, _id: PaneId, _key: &str, _mods: Modifiers) -> bool {
            false
        }
        fn send_text(&self, _id: PaneId, _text: &str) -> bool {
            false
        }
        fn pane_full_text(&self, _id: PaneId) -> String {
            String::new()
        }
        fn pane_command_label(&self, _id: PaneId) -> String {
            String::new()
        }
        fn pane_title(&self, id: PaneId) -> Option<String> {
            self.titles.get(&id).cloned()
        }
        fn pane_notification(&self, id: PaneId) -> Option<PaneNotification> {
            match &self.notes {
                Some(notes) => notes.borrow().get(&id).cloned(),
                None => self.notifications.get(&id).cloned(),
            }
        }
        fn pane_bell_seq(&self, id: PaneId) -> u64 {
            self.bells.get(&id).copied().unwrap_or(0)
        }
        fn pane_agent(&self, id: PaneId) -> Option<PaneAgent> {
            self.agents.get(&id).cloned()
        }
        /// Inert: these tests drive the pane slot map, not the window / session surface.
        fn windows(&self) -> Vec<WindowInfo> {
            Vec::new()
        }
        fn select_window(&self, _name: &str) {}
        fn select_window_toward(&self, _step: sprag_terminal::OrderStep) -> Option<String> {
            None
        }
        /// Inert, like the walk above it: this fixture drives what is PAINTED, not the order.
        fn move_window(
            &self,
            _window: Option<&str>,
            _place: &sprag_terminal::WindowPlace,
        ) -> Option<(String, sprag_terminal::PlaceHow)> {
            None
        }
        fn new_window(&self) -> String {
            String::new()
        }
        fn kill_window(&self, _name: &str) {}
        /// These fixtures exercise slot ROUTING, not renaming — refused honestly rather than
        /// answered with a name nothing recorded.
        fn rename_window(&self, _name: &str) -> Option<String> {
            None
        }
        fn rename_session(&self, _name: &str) -> Option<String> {
            None
        }
        fn rename_pane(&self, _id: PaneId, _name: &str) -> Option<String> {
            None
        }
        /// No sample: these fixtures exercise the ROUTING over a session list, not the facts a row
        /// paints. An empty reading of age zero is the honest "nothing sampled here" (see
        /// `HostClient::session_activity`), and it keeps every subtitle out of the fixture's way.
        fn session_activity(&self) -> sprag_terminal::ActivityReading {
            sprag_terminal::ActivityReading {
                age: std::time::Duration::ZERO,
                value: Vec::new(),
            }
        }
        fn sessions(&self) -> Vec<SessionInfo> {
            Vec::new()
        }
        fn current_session(&self) -> String {
            String::new()
        }
        fn switch_session(&self, _name: &str) {}
        fn switch_session_toward(&self, _step: sprag_terminal::OrderStep) -> Option<String> {
            None
        }
        fn switch_session_last(&self) -> Option<String> {
            None
        }
        fn switch_session_named(&self, _name: &str) -> Option<String> {
            None
        }
        fn new_session(&self) -> String {
            String::new()
        }
        fn kill_session(&self, _name: &str) {}
    }

    fn view_over(ids: &std::rc::Rc<RefCell<Vec<PaneId>>>) -> SlotView {
        SlotView::new(Box::new(FakeHost {
            ids: std::rc::Rc::clone(ids),
            titles: std::collections::HashMap::new(),
            notifications: std::collections::HashMap::new(),
            notes: None,
            bells: std::collections::HashMap::new(),
            dead: std::collections::HashSet::new(),
            agents: std::collections::HashMap::new(),
            exits: std::collections::HashMap::new(),
        }))
    }

    /// The R128 DISPLAY-title policy ([`crate::view::pane_display_title`]): prefer the
    /// child's live OSC title, fall back to the stable `panel_id` when it has set none —
    /// or set a BLANK one, which must not blank the header. Identity is never affected.
    #[test]
    fn display_title_prefers_the_osc_title_and_falls_back_to_the_panel_id() {
        let ids = std::rc::Rc::new(RefCell::new(vec![pid(10), pid(11), pid(12)]));
        let view = SlotView::new(Box::new(FakeHost {
            ids: std::rc::Rc::clone(&ids),
            titles: [
                (pid(10), "vim README".to_owned()),
                (pid(11), "   ".to_owned()), // child set a BLANK title
            ]
            .into_iter()
            .collect(),
            notifications: std::collections::HashMap::new(),
            notes: None,
            bells: std::collections::HashMap::new(),
            dead: std::collections::HashSet::new(),
            agents: std::collections::HashMap::new(),
            exits: std::collections::HashMap::new(),
        }));

        // `pane_display_title` reads the per-slot attention-ack Signal, so it runs in an Owner
        // scope (as every paint / reconcile caller does); no notification ⇒ no marker prefix.
        Owner::new().run(|| {
            // Slot 0's child set a title -> it is displayed.
            assert_eq!(crate::view::pane_display_title(&view, 0), "vim README");
            // Slot 1's child set a blank one -> fall back, never an empty header.
            assert_eq!(crate::view::pane_display_title(&view, 1), "terminal-1");
            // Slot 2's child set none -> the stable panel id.
            assert_eq!(crate::view::pane_display_title(&view, 2), "terminal-2");
            // A hole (no pane) still yields its stable panel id, never a panic.
            assert_eq!(crate::view::pane_display_title(&view, 7), "terminal-7");
        });
    }

    /// A pane whose child has EXITED wears the marker on its title, on every surface that reads
    /// [`crate::view::pane_display_title`] — the one thing that distinguishes a finished command
    /// from a hung one, since sprag keeps the dead pane and its final screen either way.
    ///
    /// REVERT-PROOF: drop the `pane_is_dead` branch in `pane_display_title` and the exited pane's
    /// title is indistinguishable from its live sibling's, which is the whole defect.
    #[test]
    fn an_exited_pane_wears_the_marker_and_a_live_one_does_not() {
        use crate::attention::{ATTENTION_MARKER, ack_focused};
        use crate::view::DEAD_MARKER;

        let ids = std::rc::Rc::new(RefCell::new(vec![pid(10), pid(11)]));
        let view = SlotView::new(Box::new(FakeHost {
            ids: std::rc::Rc::clone(&ids),
            titles: [(pid(10), "cargo test".to_owned())].into_iter().collect(),
            notifications: std::collections::HashMap::new(),
            notes: None,
            bells: [(pid(10), 1u64)].into_iter().collect(), // pane 10 also rang, so both markers ride
            dead: [pid(10)].into_iter().collect(),          // ...and its child has exited
            agents: std::collections::HashMap::new(),
            exits: std::collections::HashMap::new(),
        }));

        Owner::new().run(|| {
            let title = |i| crate::view::pane_display_title(&view, i);
            assert!(
                title(0).ends_with(DEAD_MARKER),
                "the exited pane says so: {:?}",
                title(0)
            );
            assert!(
                title(0).starts_with(ATTENTION_MARKER),
                "and the two markers COMPOSE, each at its own end: {:?}",
                title(0)
            );
            assert!(
                title(0).contains("cargo test"),
                "without displacing the child's own title: {:?}",
                title(0)
            );
            assert!(
                !title(1).ends_with(DEAD_MARKER),
                "a live sibling wears nothing: {:?}",
                title(1)
            );

            // Viewing the pane clears the ATTENTION marker; the exited one is not a flag to clear.
            ack_focused(&view, Some(0));
            assert!(
                !title(0).starts_with(ATTENTION_MARKER),
                "attention is acknowledged by looking: {:?}",
                title(0)
            );
            assert!(
                title(0).ends_with(DEAD_MARKER),
                "but looking at a dead pane does not bring it back: {:?}",
                title(0)
            );
        });

        // A HOLE has no child to have died — the graceful default every slot-mapped read keeps.
        assert!(!view.pane_is_dead(7));
        assert_eq!(view.pane_child_exit(7), None, "...and no status either");
    }

    /// The AGENT verdict reaches every title surface through the same SSOT the other two markers do
    /// (H3 slice 5), and it reaches them as WORDS a person can act on: the pane running a blocked
    /// agent is the one the user has to go to, and it says so beside the pane's own title rather than
    /// only on a wire nobody reads.
    ///
    /// Four panes, because four things had to hold at once and only a composition test can see them
    /// together: a claimed pane wears its state, an UNCLAIMED one wears nothing (D3 — "this is not an
    /// agent" and "this agent wants you" must not collapse), `blocked` is rendered as a REQUEST rather
    /// than as a fault, and a DEAD pane drops the verdict its final screen still shows.
    ///
    /// REVERT-PROOF: drop the `agent_marker` call from `pane_display_title` and every assertion but
    /// the unclaimed pane's fails — the pane list goes back to being unable to say which of two
    /// identical-looking panes is waiting for an answer.
    #[test]
    fn the_agent_state_rides_the_display_title_and_a_dead_pane_drops_it() {
        let agent = |state: &str, name: &str| PaneAgent {
            state: state.to_owned(),
            name: Some(name.to_owned()),
            rule: Some("idle-glyph".to_owned()),
            seq: 3,
        };
        let ids = std::rc::Rc::new(RefCell::new(vec![pid(10), pid(11), pid(12), pid(13)]));
        let view = SlotView::new(Box::new(FakeHost {
            ids: std::rc::Rc::clone(&ids),
            titles: [
                (pid(10), "claude".to_owned()),
                (pid(11), "claude".to_owned()),
                (pid(12), "bash".to_owned()),
                (pid(13), "claude".to_owned()),
            ]
            .into_iter()
            .collect(),
            notifications: std::collections::HashMap::new(),
            notes: None,
            bells: std::collections::HashMap::new(),
            // The fourth pane's child is GONE while its screen still shows the agent that was there.
            dead: [pid(13)].into_iter().collect(),
            agents: [
                (pid(10), agent("working", "claude")),
                (pid(11), agent("blocked", "claude")),
                // pid(12) is a shell: no manifest claims it, so the wire carries no key at all.
                (pid(13), agent("idle", "claude")),
            ]
            .into_iter()
            .collect(),
            exits: std::collections::HashMap::new(),
        }));

        Owner::new().run(|| {
            let title = |i| crate::view::pane_display_title(&view, i);
            assert_eq!(
                title(0),
                "claude (claude working)",
                "the state rides the title the dock header and the tab both read",
            );
            assert_eq!(
                title(1),
                "claude (claude needs an answer)",
                "`blocked` is a REQUEST, not a fault — the wire token is not what a person reads",
            );
            assert_eq!(
                title(2),
                "bash",
                "a pane no manifest claims says nothing: absence is the answer, never `idle`",
            );
            assert_eq!(
                title(3),
                format!("claude{}", crate::view::DEAD_MARKER),
                "a dead pane's final screen may still show an agent; the pane is not running one",
            );
        });
    }

    /// A pane whose child FAILED names its code on the title, and one that a signal killed names
    /// the signal — the difference between "this finished badly" and "something took it", neither
    /// of which a stopped screen can express on its own.
    ///
    /// The third pane is the state a combined `Option` could not represent: dead, not yet reaped.
    /// It renders exactly like a clean exit, deliberately, so the reap window is invisible for the
    /// common ending rather than a flicker every pane shows.
    ///
    /// REVERT-PROOF: have `pane_display_title` append the bare `DEAD_MARKER` again and the first
    /// two assertions fail — a failing `cargo test` becomes indistinguishable from a passing one,
    /// which is the whole reason to reap at all.
    #[test]
    fn an_exited_panes_title_names_its_code_or_the_signal_that_killed_it() {
        use crate::view::DEAD_MARKER;

        let ids = std::rc::Rc::new(RefCell::new(vec![pid(10), pid(11), pid(12)]));
        let view = SlotView::new(Box::new(FakeHost {
            ids: std::rc::Rc::clone(&ids),
            titles: std::collections::HashMap::new(),
            notifications: std::collections::HashMap::new(),
            notes: None,
            bells: std::collections::HashMap::new(),
            // All three children are gone; only two of them have been reaped.
            dead: [pid(10), pid(11), pid(12)].into_iter().collect(),
            agents: std::collections::HashMap::new(),
            exits: [
                (
                    pid(10),
                    PaneExit {
                        code: 101,
                        signal: None,
                    },
                ),
                (
                    pid(11),
                    PaneExit {
                        code: 1,
                        signal: Some("Terminated".to_owned()),
                    },
                ),
            ]
            .into_iter()
            .collect(),
        }));

        Owner::new().run(|| {
            let title = |i| crate::view::pane_display_title(&view, i);
            assert!(
                title(0).ends_with(" (exited 101)"),
                "a failing command reports its code: {:?}",
                title(0)
            );
            assert!(
                title(1).ends_with(" (killed: Terminated)"),
                "a signalled one names the signal, not the stand-in code 1: {:?}",
                title(1)
            );
            assert!(
                title(2).ends_with(DEAD_MARKER),
                "and one not yet reaped still says it is finished: {:?}",
                title(2)
            );
        });
    }

    /// A CLEAN exit renders as the bare marker, not `(exited 0)`. The status is known — the client
    /// simply declines to shout a zero on the commonest ending there is, and that choice is what
    /// makes the unreaped window above invisible instead of a flicker on every pane.
    #[test]
    fn a_clean_exit_reads_the_same_as_one_not_yet_reaped() {
        let ids = std::rc::Rc::new(RefCell::new(vec![pid(10)]));
        let view = SlotView::new(Box::new(FakeHost {
            ids: std::rc::Rc::clone(&ids),
            titles: std::collections::HashMap::new(),
            notifications: std::collections::HashMap::new(),
            notes: None,
            bells: std::collections::HashMap::new(),
            dead: [pid(10)].into_iter().collect(),
            agents: std::collections::HashMap::new(),
            exits: [(
                pid(10),
                PaneExit {
                    code: 0,
                    signal: None,
                },
            )]
            .into_iter()
            .collect(),
        }));

        Owner::new().run(|| {
            assert!(
                crate::view::pane_display_title(&view, 0).ends_with(crate::view::DEAD_MARKER),
                "a clean exit is reported as finished, with no number to read",
            );
        });
    }

    /// The attention feature end to end through its real consumer ([`crate::view::pane_display_title`]):
    /// a pane whose child raised a notification wears the marker until it is VIEWED, and a fresh
    /// notification (a higher `seq`) re-arms it. REVERT-PROOF: a pane with no notification never
    /// wears the marker, and the ack clears it — so neither half is unconditional. The notes map is
    /// an `Rc<RefCell<..>>` the test still holds after the host takes its clone, so a new
    /// notification can be raised MID-run (the live-mirror case, `seq` growing under the client).
    #[test]
    fn an_unseen_notification_marks_the_title_until_the_pane_is_viewed() {
        use crate::attention::{ATTENTION_MARKER, ack_focused, reset_pane_ack};

        let ids = std::rc::Rc::new(RefCell::new(vec![pid(10), pid(11)]));
        let notes = std::rc::Rc::new(RefCell::new(
            [(pid(10), note(1, "build done"))]
                .into_iter()
                .collect::<std::collections::HashMap<PaneId, PaneNotification>>(),
        ));
        let view = SlotView::new(Box::new(FakeHost {
            ids: std::rc::Rc::clone(&ids),
            titles: std::collections::HashMap::new(),
            notifications: std::collections::HashMap::new(),
            notes: Some(std::rc::Rc::clone(&notes)),
            bells: std::collections::HashMap::new(),
            dead: std::collections::HashSet::new(),
            agents: std::collections::HashMap::new(),
            exits: std::collections::HashMap::new(),
        }));

        Owner::new().run(|| {
            let title = |i| crate::view::pane_display_title(&view, i);
            // Slot 0 raised a notification (seq 1), unviewed ⇒ the marker leads its title.
            assert!(
                title(0).starts_with(ATTENTION_MARKER),
                "unseen ⇒ marked: {}",
                title(0)
            );
            // Slot 1 raised none ⇒ never marked.
            assert!(
                !title(1).starts_with(ATTENTION_MARKER),
                "no notification ⇒ no marker"
            );

            // VIEWING slot 0 (it is focused) acks its seq ⇒ the marker clears.
            ack_focused(&view, Some(0));
            assert!(
                !title(0).starts_with(ATTENTION_MARKER),
                "viewed ⇒ cleared: {}",
                title(0)
            );

            // A NEW notification (seq 2) past the acked seq re-arms the marker.
            notes.borrow_mut().insert(pid(10), note(2, "tests passed"));
            assert!(
                title(0).starts_with(ATTENTION_MARKER),
                "a newer seq re-arms it"
            );

            // Resetting the ack (slot reuse) re-arms it from zero too.
            ack_focused(&view, Some(0));
            assert!(!title(0).starts_with(ATTENTION_MARKER));
            reset_pane_ack(0);
            assert!(
                title(0).starts_with(ATTENTION_MARKER),
                "a reset ack re-shows a live notification"
            );
        });
    }

    /// A BELL (`\a`) drives the SAME attention marker as a notification — the combined attention
    /// `seq` (notification + bell) is what the marker reads. A pane that only rang a bell (NO
    /// notification) still wears the marker until VIEWED. REVERT-PROOF: if the marker read only
    /// the notification seq, this pane (notification-less) would never be marked, so the first
    /// assertion would fail; the ack-clears assertion pins that a bell is view-acknowledged like a
    /// notification.
    #[test]
    fn a_bell_alone_marks_the_title_until_the_pane_is_viewed() {
        use crate::attention::{ATTENTION_MARKER, ack_focused};

        let ids = std::rc::Rc::new(RefCell::new(vec![pid(10), pid(11)]));
        let view = SlotView::new(Box::new(FakeHost {
            ids: std::rc::Rc::clone(&ids),
            titles: std::collections::HashMap::new(),
            notifications: std::collections::HashMap::new(), // NO notification on either pane
            notes: None,
            bells: [(pid(10), 2u64)].into_iter().collect(), // pane 10 rang the bell twice
            dead: std::collections::HashSet::new(),
            agents: std::collections::HashMap::new(),
            exits: std::collections::HashMap::new(),
        }));

        Owner::new().run(|| {
            let title = |i| crate::view::pane_display_title(&view, i);
            // Slot 0 rang a bell (no notification), unviewed ⇒ the marker leads its title.
            assert!(
                title(0).starts_with(ATTENTION_MARKER),
                "a bell alone marks the title: {}",
                title(0)
            );
            // Slot 1 rang none ⇒ never marked.
            assert!(
                !title(1).starts_with(ATTENTION_MARKER),
                "no bell, no notification ⇒ no marker"
            );
            // VIEWING slot 0 acks the combined seq (which includes the bell) ⇒ the marker clears.
            ack_focused(&view, Some(0));
            assert!(
                !title(0).starts_with(ATTENTION_MARKER),
                "a bell is view-acknowledged like a notification: {}",
                title(0)
            );
        });
    }

    /// A [`PaneNotification`] with `seq` and a body, for the attention tests.
    fn note(seq: u64, body: &str) -> PaneNotification {
        PaneNotification {
            title: None,
            body: body.to_owned(),
            seq,
        }
    }

    #[test]
    fn reconcile_boot_maps_all_added_then_a_steady_frame_is_a_no_op() {
        let ids = std::rc::Rc::new(RefCell::new(vec![pid(10), pid(11)]));
        let view = view_over(&ids);
        // Boot mapped both host panes to contiguous slots 0, 1.
        assert_eq!(view.occupied_slots(), vec![0, 1]);
        assert!(view.is_pane_occupied(0) && view.is_pane_occupied(1));
        // A steady reconcile (host set unchanged) frees nothing.
        assert!(view.reconcile().freed.is_empty());
    }

    #[test]
    fn reconcile_frees_a_closed_pane_and_reuses_its_slot_for_a_newcomer() {
        let ids = std::rc::Rc::new(RefCell::new(vec![pid(10), pid(11)]));
        let view = view_over(&ids);
        assert_eq!(view.occupied_slots(), vec![0, 1]);
        // Pane 11 (slot 1) closed host-side, pane 12 opened.
        *ids.borrow_mut() = vec![pid(10), pid(12)];
        let delta = view.reconcile();
        assert_eq!(delta.freed, vec![1], "slot 1 (pane 11) freed");
        assert_eq!(
            view.pane_at(1),
            Some(pid(12)),
            "pane 12 took the reused slot 1"
        );
        assert_eq!(view.occupied_slots(), vec![0, 1]);
    }

    #[test]
    fn reconcile_shrink_leaves_a_hole_survivors_never_migrate() {
        // The load-bearing property: a survivor keeps its slot for life, so a middle-pane
        // close leaves a HOLE (not a compacting shift) and no per-slot GUI state migrates
        // onto a different pane.
        let ids = std::rc::Rc::new(RefCell::new(vec![pid(10), pid(11), pid(12)]));
        let view = view_over(&ids);
        assert_eq!(view.occupied_slots(), vec![0, 1, 2]);
        // The MIDDLE pane (slot 1) closes.
        *ids.borrow_mut() = vec![pid(10), pid(12)];
        let delta = view.reconcile();
        assert_eq!(delta.freed, vec![1]);
        // Pane 12 KEPT slot 2 (it did not slide into the hole at 1); slot 1 is a hole.
        assert_eq!(view.occupied_slots(), vec![0, 2]);
        assert!(!view.is_pane_occupied(1));
    }

    #[test]
    fn reconcile_free_plus_two_adds_reuses_the_hole_then_takes_a_higher_slot() {
        // A single free with TWO newcomers in one reconcile: one reuses the freed hole
        // (lowest), the other takes the next free slot -> freed != added (the
        // multi-add-after-free path the earlier delta tests don't exercise).
        let ids = std::rc::Rc::new(RefCell::new(vec![pid(10), pid(11), pid(12)]));
        let view = view_over(&ids);
        assert_eq!(view.occupied_slots(), vec![0, 1, 2]);
        // Pane 11 (slot 1) closes; panes 20 and 21 open (host order 10, 12, 20, 21).
        *ids.borrow_mut() = vec![pid(10), pid(12), pid(20), pid(21)];
        let delta = view.reconcile();
        assert_eq!(delta.freed, vec![1], "slot 1 (pane 11) freed");
        assert_eq!(
            view.pane_at(1),
            Some(pid(20)),
            "pane 20 reuses the hole at 1"
        );
        assert_eq!(
            view.pane_at(3),
            Some(pid(21)),
            "pane 21 takes the next free slot"
        );
        assert_eq!(view.occupied_slots(), vec![0, 1, 2, 3]);
    }
}
