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
use sprag_host::{HostClient, PaneScrollFacts};
use sprag_input::Modifiers;
use sprag_terminal::{LayoutSnapshot, LayoutWire, PaneId};

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
}

impl SlotView {
    /// Wrap `host` and map its current panes to slots (boot = the all-new path: host
    /// order -> contiguous slots `0..N`).
    pub(crate) fn new(host: Box<dyn HostClient>) -> Self {
        let view = Self {
            host,
            slots: RefCell::new((0..MAX_PANES).map(|_| None).collect()),
        };
        let _boot = view.reconcile(); // boot is all-added, no frees; nothing to reset yet
        view
    }

    /// Re-map slots to the host's current pane set — the ONE place slot membership
    /// changes. Frees the slot of every mapped pane no longer present, allocates the
    /// lowest free slot to each new host pane, and returns the [`SlotDelta`] so the caller
    /// resets each freed slot's per-slot GUI state. No IO: the host owns the frame data,
    /// this owns only the mapping. `&self` (interior-mutable) so it runs through the shared
    /// `Rc<TerminalView>`.
    pub(crate) fn reconcile(&self) -> SlotDelta {
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
        SlotDelta { freed }
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
    fn id(&self, slot: usize) -> Option<PaneId> {
        self.slots.borrow().get(slot).copied().flatten()
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
    pub(crate) fn set_layout(&self, tree: LayoutWire) -> LayoutSnapshot {
        self.host.set_layout(tree)
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

    /// Slot `slot`'s grid `(cols, rows)` (`(1, 1)` for a hole).
    pub(crate) fn pane_grid_size(&self, slot: usize) -> (u16, u16) {
        self.id(slot)
            .map_or((1, 1), |id| self.host.pane_grid_size(id))
    }

    /// Resize slot `slot`'s pane (a no-op for a hole).
    pub(crate) fn resize(&self, slot: usize, cols: u16, rows: u16) {
        if let Some(id) = self.id(slot) {
            self.host.resize(id, cols, rows);
        }
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

    /// A [`HostClient`] whose pane-id list the test controls (shared via `Rc<RefCell<..>>`),
    /// so a live `reconcile` delta is driven without a real host. Every other method returns
    /// its graceful default — the slot map / delta logic reads only `pane_ids` — except
    /// `pane_title`, which serves `titles` so the R128 display-title policy is testable.
    struct FakeHost {
        ids: std::rc::Rc<RefCell<Vec<PaneId>>>,
        /// Per-pane-id OSC title, for the `pane_title` / display-title tests.
        titles: std::collections::HashMap<PaneId, String>,
    }

    impl HostClient for FakeHost {
        fn pane_ids(&self) -> Vec<PaneId> {
            self.ids.borrow().clone()
        }
        /// Inert: these tests drive the slot map / delta logic, which reads only
        /// `pane_ids` — the arrangement is a separate authority.
        fn layout(&self) -> LayoutSnapshot {
            LayoutSnapshot::default()
        }
        fn set_layout(&self, _tree: LayoutWire) -> LayoutSnapshot {
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
        fn pane_grid_size(&self, _id: PaneId) -> (u16, u16) {
            (1, 1)
        }
        fn resize(&self, _id: PaneId, _cols: u16, _rows: u16) {}
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
    }

    fn view_over(ids: &std::rc::Rc<RefCell<Vec<PaneId>>>) -> SlotView {
        SlotView::new(Box::new(FakeHost {
            ids: std::rc::Rc::clone(ids),
            titles: std::collections::HashMap::new(),
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
        }));

        // Slot 0's child set a title -> it is displayed.
        assert_eq!(crate::view::pane_display_title(&view, 0), "vim README");
        // Slot 1's child set a blank one -> fall back, never an empty header.
        assert_eq!(crate::view::pane_display_title(&view, 1), "terminal-1");
        // Slot 2's child set none -> the stable panel id.
        assert_eq!(crate::view::pane_display_title(&view, 2), "terminal-2");
        // A hole (no pane) still yields its stable panel id, never a panic.
        assert_eq!(crate::view::pane_display_title(&view, 7), "terminal-7");
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
