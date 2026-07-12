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
//! ## Slot stability + reuse
//!
//! A slot is STABLE for a pane's life: [`reconcile`](SlotView::reconcile) keeps a mapped
//! `PaneId` in its slot and frees a slot only when its pane leaves the host set, so a
//! survivor's per-slot GUI state never migrates onto a different pane. A freed slot may
//! be REUSED by a later pane (the compact-slot allocator), so a reused slot's per-slot
//! GUI state — keyed by slot index in `Owner::cache`, OUTSIDE this map — MUST be reset by
//! the caller when the slot frees. `reconcile` returns the freed slots for exactly that
//! (Round 2b, when live add/remove deltas arrive; boot never frees, so the reset is a
//! documented Round 2b hook, not a Round 2a operation).

use std::collections::HashSet;

use pinion_core::GridBuffer;
use sprag_host::{HostClient, PaneScrollFacts};
use sprag_input::Modifiers;
use sprag_terminal::PaneId;

use crate::terminal::MAX_PANES;

/// The GUI's display-slot mapping over a host client (see the module docs). Consumers
/// address panes by display SLOT; this translates each to the host's [`PaneId`] and
/// delegates to the wrapped [`HostClient`]. An empty slot yields each method's graceful
/// default, so a hole never panics.
pub(crate) struct SlotView {
    host: Box<dyn HostClient>,
    /// slot -> the `PaneId` occupying it (`None` = a hole). Length [`MAX_PANES`].
    slots: Vec<Option<PaneId>>,
}

impl SlotView {
    /// Wrap `host` and map its current panes to slots (boot = the all-new path: host
    /// order -> contiguous slots `0..N`).
    pub(crate) fn new(host: Box<dyn HostClient>) -> Self {
        let mut view = Self {
            host,
            slots: (0..MAX_PANES).map(|_| None).collect(),
        };
        let _freed = view.reconcile();
        view
    }

    /// Re-map slots to the host's current pane set — the ONE place slot membership
    /// changes. Frees the slot of every mapped pane no longer present, allocates the
    /// lowest free slot to each new host pane, and returns the FREED slots so the caller
    /// resets their per-slot GUI state before reuse (the module-docs Round 2b hook; boot
    /// frees nothing). No IO: the host owns the frame data, this owns only the mapping.
    pub(crate) fn reconcile(&mut self) -> Vec<usize> {
        let host_ids = self.host.pane_ids();
        let (frees, adds, overflow) = plan_slots(&self.slots, &host_ids);
        for &slot in &frees {
            self.slots[slot] = None;
        }
        for (slot, id) in adds {
            self.slots[slot] = Some(id);
        }
        if !overflow.is_empty() {
            tracing::warn!(
                target: "sprag_gui::slotview",
                dropped = overflow.len(),
                cap = MAX_PANES,
                "host pane set exceeds the slot cap; extra panes not shown",
            );
        }
        frees
    }

    /// The occupied display slots, ascending — the set consumers ITERATE instead of
    /// assuming a contiguous `0..pane_count()` (a closed pane leaves a hole).
    pub(crate) fn occupied_slots(&self) -> Vec<usize> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(slot, id)| id.map(|_| slot))
            .collect()
    }

    /// Whether display slot `slot` currently holds a pane (O(1), alloc-free — the paint
    /// hot path calls it per leaf per frame).
    pub(crate) fn is_pane_occupied(&self, slot: usize) -> bool {
        self.slots.get(slot).is_some_and(Option::is_some)
    }

    /// The `PaneId` at `slot`, if occupied — the ONE slot->id resolver the delegating
    /// methods share; a hole yields each method's graceful default.
    fn id(&self, slot: usize) -> Option<PaneId> {
        self.slots.get(slot).copied().flatten()
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
}
