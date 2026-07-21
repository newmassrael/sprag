//! OSC 52 clipboard integration — the client-local half of the clipboard feature.
//!
//! A program in a pane can SET the system clipboard (`OSC 52 ; c ; <base64>`) or READ it back
//! (`OSC 52 ; c ; ?`). The emulator only RECORDS these (the clipboard is the display client's, not
//! the host's — see [`sprag_vt::VtPort::clipboard_write`]); this module is where a GUI client
//! actually touches its system clipboard, governed by a [`ClipboardPolicy`].
//!
//! ## Why a policy, and its default (the security posture)
//!
//! A READ lets a program in a pane — possibly remote over SSH — exfiltrate whatever is on your
//! clipboard, so Ghostty / kitty / xterm all DENY reads by default. A WRITE only lets a program
//! set your clipboard (paste-injection), the common and lower-risk case (copying from a remote
//! vim / tmux). So the default when `SPRAG_OSC52` is unset is WRITES ALLOWED, READS DENIED. This
//! is already tmux-superior: tmux's `set-clipboard` is on/off with no read at all, and one server
//! buffer; sprag gives per-selection (clipboard vs primary), per-direction control, and each
//! attached client applies to ITS OWN system clipboard.
//!
//! ## Why acks are primed, not zero (no stale-clobber)
//!
//! Each slot tracks the last write it APPLIED and the last read it ANSWERED. Unlike the attention
//! marker (which starts at 0 — showing a marker for a pre-attach notification is harmless), a
//! clipboard write is an EFFECT: replaying a stale latched write on attach would clobber your
//! current clipboard with an old copy. So an ack starts as `None` and is PRIMED to the pane's
//! current seq on first observation (a slot bind / attach) WITHOUT applying — only writes and
//! reads witnessed AFTER that take effect. A freed slot resets the acks ([`reset_pane_clip_acks`],
//! called from `reset_freed_slot`), so a reused slot re-primes against its new pane.
//!
//! ## Multi-client reads are host-arbitrated
//!
//! When several clients are attached, each has its own system clipboard, so a READ answer would be
//! ambiguous. A client that its policy PERMITS reads its clipboard and offers the answer to the
//! host, which admits EXACTLY ONE reply per query (see [`sprag_host::HostClient::answer_clipboard_query`]);
//! if every client's policy denies, the query goes unanswered. Writes need no arbitration — every
//! client applies to its own clipboard.

use std::sync::LazyLock;

use pinion_core::ClipboardSelection;
use pinion_core::reactive::{Owner, Signal};
use sprag_vt::ClipboardTarget;

use crate::selection::clipboard;
use crate::slotview::SlotView;
use crate::terminal::pane_cache_key;

/// The [`Owner::cache`] namespace for a slot's last-APPLIED clipboard-write seq.
const WRITE_ACK_NAMESPACE: &str = "clip_write_ack";
/// The [`Owner::cache`] namespace for a slot's last-ANSWERED clipboard-read seq.
const READ_ACK_NAMESPACE: &str = "clip_read_ack";

/// The OSC 52 clipboard access policy — which directions (read / write) are permitted on which
/// selection (clipboard / primary). Parsed once at boot from `SPRAG_OSC52`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct ClipboardPolicy {
    write_clipboard: bool,
    read_clipboard: bool,
    write_primary: bool,
    read_primary: bool,
}

impl ClipboardPolicy {
    /// The default when `SPRAG_OSC52` is unset: WRITES allowed, READS denied — the exfiltration
    /// guard Ghostty / kitty ship.
    const fn secure_default() -> Self {
        Self {
            write_clipboard: true,
            write_primary: true,
            read_clipboard: false,
            read_primary: false,
        }
    }

    /// Everything permitted.
    const fn all() -> Self {
        Self {
            write_clipboard: true,
            write_primary: true,
            read_clipboard: true,
            read_primary: true,
        }
    }

    /// Nothing permitted.
    const fn none() -> Self {
        Self {
            write_clipboard: false,
            write_primary: false,
            read_clipboard: false,
            read_primary: false,
        }
    }

    /// Parse `SPRAG_OSC52`. `None` (unset) is the [`secure_default`](Self::secure_default). A
    /// PRESENT value is an explicit ALLOWLIST, parsed from all-denied — a typo denies rather than
    /// silently widening. Tokens are whitespace/comma separated, case-insensitive (kitty's
    /// `clipboard_control` vocabulary plus convenience shortcuts):
    ///
    /// * `off` / `none` (or empty) — permit nothing (a kill switch: it wins regardless of order);
    /// * `all` / `rw` — permit everything;
    /// * `write` / `read` — both selections in that direction;
    /// * `write-clipboard` / `read-clipboard` / `write-primary` / `read-primary` — one bit.
    pub(crate) fn parse(spec: Option<&str>) -> Self {
        let Some(spec) = spec else {
            return Self::secure_default();
        };
        let mut p = Self::none();
        for token in spec.split([',', ' ', '\t', '\n']) {
            match token.trim().to_ascii_lowercase().as_str() {
                "" => {}
                "off" | "none" => return Self::none(), // absolute kill switch
                "all" | "rw" => p = Self::all(),
                "write" => {
                    p.write_clipboard = true;
                    p.write_primary = true;
                }
                "read" => {
                    p.read_clipboard = true;
                    p.read_primary = true;
                }
                "write-clipboard" => p.write_clipboard = true,
                "read-clipboard" => p.read_clipboard = true,
                "write-primary" => p.write_primary = true,
                "read-primary" => p.read_primary = true,
                other => {
                    tracing::warn!(target: "sprag_gui::clipboard", token = other, "unknown SPRAG_OSC52 token ignored (denies)");
                }
            }
        }
        p
    }

    /// Whether a WRITE to `target` is permitted.
    fn allows_write(self, target: ClipboardTarget) -> bool {
        match target {
            ClipboardTarget::Clipboard => self.write_clipboard,
            ClipboardTarget::Primary => self.write_primary,
        }
    }

    /// Whether a READ of `target` is permitted.
    fn allows_read(self, target: ClipboardTarget) -> bool {
        match target {
            ClipboardTarget::Clipboard => self.read_clipboard,
            ClipboardTarget::Primary => self.read_primary,
        }
    }
}

/// The process-wide clipboard policy, parsed once from `SPRAG_OSC52` (one GUI process = one
/// client = one posture).
static POLICY: LazyLock<ClipboardPolicy> =
    LazyLock::new(|| ClipboardPolicy::parse(std::env::var("SPRAG_OSC52").ok().as_deref()));

/// Map an OSC 52 [`ClipboardTarget`] to pinion's [`ClipboardSelection`] (the clipboard handle's
/// vocabulary) — a 1:1 correspondence.
fn to_selection(target: ClipboardTarget) -> ClipboardSelection {
    match target {
        ClipboardTarget::Clipboard => ClipboardSelection::Clipboard,
        ClipboardTarget::Primary => ClipboardSelection::Primary,
    }
}

/// Slot `slot`'s last-APPLIED clipboard-write seq, client-local in the binding-root [`Owner::cache`]
/// (the [`crate::attention`] / [`crate::scrollbar`] per-slot pattern). `None` until PRIMED on first
/// observation — priming records the pane's current seq WITHOUT applying, so a stale latched write
/// never clobbers the clipboard on attach.
fn use_write_ack(slot: usize) -> Signal<Option<u64>> {
    Owner::current()
        .expect("use_write_ack() requires an active Owner scope")
        .cache(pane_cache_key(WRITE_ACK_NAMESPACE, slot), || {
            Signal::new(None)
        })
        .as_ref()
        .clone()
}

/// Slot `slot`'s last-ANSWERED clipboard-read seq — same shape as [`use_write_ack`], primed on
/// first observation so a stale latched query is never answered on attach.
fn use_read_ack(slot: usize) -> Signal<Option<u64>> {
    Owner::current()
        .expect("use_read_ack() requires an active Owner scope")
        .cache(pane_cache_key(READ_ACK_NAMESPACE, slot), || {
            Signal::new(None)
        })
        .as_ref()
        .clone()
}

/// Apply any newly-witnessed OSC 52 clipboard writes and answer any newly-witnessed read queries,
/// for every occupied slot — called once per frame from the reconcile pass. Loop-safe: each ack
/// [`Signal::set`] EQUALITY-SKIPS, and after acting the seq no longer exceeds the ack, so a slot
/// with nothing new is inert (no repaint loop), exactly like [`crate::scrollbar::reconcile_scroll`].
pub(crate) fn reconcile_clipboard(slots: &SlotView) {
    for slot in slots.occupied_slots() {
        apply_pending_write(slots, slot);
        answer_pending_read(slots, slot);
    }
}

/// What to do with a pane's clipboard signal (write or read) given its current `seq` and the last
/// seq this client handled (`ack`). The PURE core of the no-stale-clobber rule: a `None` ack means
/// never observed, so PRIME (record the baseline, act on nothing) rather than replay a latched
/// signal from before attach; a `seq` that GREW past the ack is a genuine new signal to act on;
/// anything else is already handled. Unit-tested — the subtle bit of the whole module.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SeqAction {
    /// First observation — record `seq` as the baseline ack, take no action.
    Prime(u64),
    /// A new signal — act on it, then ack to its seq.
    Act,
    /// Already handled (or nothing newer) — do nothing.
    Idle,
}

fn seq_action(seq: u64, ack: Option<u64>) -> SeqAction {
    match ack {
        None => SeqAction::Prime(seq),
        Some(acked) if seq > acked => SeqAction::Act,
        _ => SeqAction::Idle,
    }
}

/// Apply slot `slot`'s pending clipboard WRITE, if newly witnessed. On a genuine new write, fetches
/// the payload ON DEMAND and copies its text to each policy-permitted target selection.
fn apply_pending_write(slots: &SlotView, slot: usize) {
    let seq = slots.pane_clipboard_write_seq(slot);
    let ack = use_write_ack(slot);
    match seq_action(seq, ack.get()) {
        SeqAction::Prime(baseline) => ack.set(Some(baseline)),
        SeqAction::Idle => {}
        SeqAction::Act => {
            // Fetch the (potentially large) payload on demand.
            let Some(write) = slots.pane_clipboard_write(slot) else {
                return; // fetch missed (transient) — retry next frame, don't ack past it
            };
            let cb = clipboard();
            if write.targets.clipboard && POLICY.allows_write(ClipboardTarget::Clipboard) {
                cb.copy_to(to_selection(ClipboardTarget::Clipboard), write.text.clone());
            }
            if write.targets.primary && POLICY.allows_write(ClipboardTarget::Primary) {
                cb.copy_to(to_selection(ClipboardTarget::Primary), write.text.clone());
            }
            // Ack the FETCHED seq (may be newer than the detected one if a write landed mid-fetch).
            ack.set(Some(write.seq));
        }
    }
}

/// Answer slot `slot`'s pending clipboard READ query, if newly witnessed. On a genuine new query,
/// if policy PERMITS the read, reads that selection off this client's clipboard and offers it to
/// the host (which arbitrates one reply across clients). Acks regardless of the policy decision (a
/// denied read is handled once — it simply sends no reply).
fn answer_pending_read(slots: &SlotView, slot: usize) {
    let Some(query) = slots.pane_clipboard_query(slot) else {
        return; // no read pending
    };
    let ack = use_read_ack(slot);
    match seq_action(query.seq, ack.get()) {
        SeqAction::Prime(baseline) => ack.set(Some(baseline)),
        SeqAction::Idle => {}
        SeqAction::Act => {
            if POLICY.allows_read(query.target) {
                let text = clipboard()
                    .paste_from(to_selection(query.target))
                    .unwrap_or_default();
                let _ = slots.answer_clipboard_query(slot, query.seq, query.target, &text);
            }
            ack.set(Some(query.seq));
        }
    }
}

/// Reset slot `slot`'s clipboard acks to `None` — called from `reset_freed_slot` so a slot reused
/// by a NEW pane RE-PRIMES against that pane's current seqs (never inheriting the dead pane's
/// applied/answered watermark, which would let a new pane's early write be missed or a stale one
/// replay).
pub(crate) fn reset_pane_clip_acks(slot: usize) {
    use_write_ack(slot).set(None);
    use_read_ack(slot).set(None);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_policy_is_write_allowed_read_denied() {
        let p = ClipboardPolicy::parse(None);
        assert!(p.allows_write(ClipboardTarget::Clipboard));
        assert!(p.allows_write(ClipboardTarget::Primary));
        assert!(
            !p.allows_read(ClipboardTarget::Clipboard),
            "reads denied by default (exfiltration guard)"
        );
        assert!(!p.allows_read(ClipboardTarget::Primary));
    }

    #[test]
    fn a_present_value_is_an_explicit_allowlist_from_denied() {
        // Only what is named is permitted — a lone read-clipboard does NOT re-enable writes.
        let p = ClipboardPolicy::parse(Some("read-clipboard"));
        assert!(p.allows_read(ClipboardTarget::Clipboard));
        assert!(
            !p.allows_write(ClipboardTarget::Clipboard),
            "not in the allowlist, so denied"
        );
        assert!(!p.allows_read(ClipboardTarget::Primary));
    }

    #[test]
    fn shortcuts_and_granular_tokens() {
        assert_eq!(ClipboardPolicy::parse(Some("all")), ClipboardPolicy::all());
        assert_eq!(ClipboardPolicy::parse(Some("rw")), ClipboardPolicy::all());
        assert_eq!(ClipboardPolicy::parse(Some("off")), ClipboardPolicy::none());
        // `write` = both write selections, no reads.
        let w = ClipboardPolicy::parse(Some("write"));
        assert!(
            w.allows_write(ClipboardTarget::Clipboard) && w.allows_write(ClipboardTarget::Primary)
        );
        assert!(!w.allows_read(ClipboardTarget::Clipboard));
        // A combination, comma + space separated, case-insensitive.
        let c = ClipboardPolicy::parse(Some("Write-Clipboard, read-primary"));
        assert!(c.allows_write(ClipboardTarget::Clipboard));
        assert!(c.allows_read(ClipboardTarget::Primary));
        assert!(!c.allows_read(ClipboardTarget::Clipboard));
        assert!(!c.allows_write(ClipboardTarget::Primary));
    }

    #[test]
    fn off_is_an_absolute_kill_switch_regardless_of_order() {
        assert_eq!(
            ClipboardPolicy::parse(Some("all off")),
            ClipboardPolicy::none()
        );
        assert_eq!(
            ClipboardPolicy::parse(Some("")),
            ClipboardPolicy::none(),
            "explicit empty permits nothing"
        );
        // An unknown token denies (no widening) but does not error.
        assert_eq!(
            ClipboardPolicy::parse(Some("bogus")),
            ClipboardPolicy::none()
        );
    }

    /// The no-stale-clobber core: the FIRST observation of a pane's clipboard seq PRIMES (records
    /// the baseline, acts on nothing) — so a write/query latched from before this client attached
    /// is never replayed onto the clipboard / answered. Only a seq that grows AFTER the baseline
    /// acts.
    #[test]
    fn first_observation_primes_and_only_a_later_increase_acts() {
        // Never seen (attach with a stale latched seq 7): prime to 7, do NOT act — this is what
        // stops an old copy from clobbering the clipboard on attach.
        assert_eq!(seq_action(7, None), SeqAction::Prime(7));
        // After priming to 7, the same 7 is already handled.
        assert_eq!(seq_action(7, Some(7)), SeqAction::Idle);
        // A genuine NEW write/query (seq grew past the baseline) acts.
        assert_eq!(seq_action(8, Some(7)), SeqAction::Act);
        // A pane that has never written (seq 0) primes to 0 and stays idle — never acts.
        assert_eq!(seq_action(0, None), SeqAction::Prime(0));
        assert_eq!(seq_action(0, Some(0)), SeqAction::Idle);
        // A seq that somehow went backwards (can't happen — monotonic — but be safe) is idle.
        assert_eq!(seq_action(3, Some(5)), SeqAction::Idle);
    }
}
