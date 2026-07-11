//! Input routing: a focused keystroke / IME commit -> the FOCUSED pane's PTY
//! through the host client (`Host::send_key` / `send_text`), the focus-cycle
//! chord that moves between tiled panes, and the per-pane scrollback-view offset
//! those keys snap.
//! The [`TerminalViewer`](crate::TerminalViewer) `apply_key` / `apply_composition`
//! trait methods delegate here. See the crate-root "Input" / "Scrollback" docs.

use crate::terminal::{pane_cache_key, pane_index_of, pane_tag, use_terminal};
use pinion_core::reactive::{Owner, Signal};
use pinion_core::{CompositionEvent, Modifiers};

/// `Owner::cache` key for pane `pane`'s IME preedit overlay. Minted via the one
/// per-pane key site [`pane_cache_key`] so the index suffix cannot drift.
fn preedit_key(pane: usize) -> String {
    pane_cache_key("preedit", pane)
}

/// Pane `pane`'s IME preedit (in-progress composition) string, an
/// `Owner::cache`-backed [`Signal`] that [`route_composition`] writes on each
/// composition event and `view` reads every frame to overlay at that pane's
/// cursor (see [`sprag_grid::overlay_preedit`] for why a terminal renders the
/// preedit itself — the CLIENT-side step over the host's cells). Empty = not composing;
/// display-only (the preedit never reaches the PTY — only a `Commit` writes).
/// Because `view` reads it every frame, a `set` flips the root owner dirty so the
/// shell's R705.1 reactive bridge arms a redraw — the composition repaints live
/// as you type. **Per-pane**: composition targets the focused pane only.
pub(crate) fn use_preedit(pane: usize) -> Signal<String> {
    let owner = Owner::current().expect("use_preedit() requires an active Owner scope");
    owner
        .cache(preedit_key(pane), || Signal::new(String::new()))
        .as_ref()
        .clone()
}

/// The signed row delta a `Shift+PageUp` / `Shift+PageDown` applies to the
/// top-anchored `offset_y` ([`crate::scrollbar`]: `0` = oldest top, `max` = live
/// bottom). `PageUp` walks toward older history (DECREASE `offset_y`), `PageDown`
/// back toward the live bottom (INCREASE) — `ScrollState::scroll_by` clamps to
/// `[0, max]`, so this only carries the direction. Pure / unit-testable.
fn page_delta(key: &str, page: i32) -> i32 {
    match key {
        "PageUp" => -page,
        "PageDown" => page,
        _ => 0,
    }
}

/// Scroll pane `pane`'s history for a `Shift+PageUp` / `Shift+PageDown` by one
/// page on its row-unit [`ScrollState`](crate::scrollbar::use_pane_scroll). A page
/// is the viewport height less one row (one row of overlap for continuity);
/// `scroll_by` clamps to the reconciled `[0, max]` depth. Reads the live pane for
/// the row count; called from `apply_key` (outside any cache factory).
fn scroll_view(pane: usize, key: &str) {
    let scroll = crate::scrollbar::use_pane_scroll(pane);
    let rows = use_terminal().host.pane_scroll_facts(pane).visible_rows;
    let page = i32::from(rows).saturating_sub(1).max(1);
    scroll.scroll_by(0, page_delta(key, page));
}

/// The slot to focus after a `Ctrl+PageUp` (previous) / `Ctrl+PageDown` (next) from
/// `active`, wrapping over the `occupied` display slots. Cycles over the OCCUPIED set
/// (skipping any hole a closed pane left — Round 2b), not a contiguous `0..count`; at
/// boot the set is contiguous so this is the former modular wrap. `None` when there is
/// nowhere to switch (`occupied.len() <= 1`, `active` not in the set, or a non-cycle
/// key). Pure, so it is unit-testable; up=previous / down=next mirrors the scrollback
/// chord.
fn next_focus(active: usize, key: &str, occupied: &[usize]) -> Option<usize> {
    if occupied.len() <= 1 {
        return None;
    }
    let pos = occupied.iter().position(|&slot| slot == active)?;
    let n = occupied.len();
    let next = match key {
        "PageDown" => (pos + 1) % n,
        "PageUp" => (pos + n - 1) % n,
        _ => return None,
    };
    Some(occupied[next])
}

/// Move focus to the next / previous tiled pane (wrapping) via a pinion
/// [`focus_request`](pinion_core::focus_request) — the framework focus ring and
/// the `apply_key` routing both follow the focus manager, so requesting the new
/// pane's tag is the whole switch. A single-pane window is a no-op.
fn cycle_focus(active: usize, key: &str) {
    if let Some(next) = next_focus(active, key, &use_terminal().host.occupied_slots()) {
        pinion_core::focus_request::request(pane_tag(next));
    }
}

/// A reserved chord that acts on the window / layout, NOT the focused pane's PTY.
/// `route_key` recognizes one via [`window_chord`] and dispatches it instead of
/// injecting.
#[derive(Debug, PartialEq, Eq)]
enum WindowChord {
    /// `Ctrl+PageUp/Down` — cycle focus between tiles (the `key` carries the
    /// direction at dispatch).
    CycleFocus,
    /// `Shift+PageUp/Down` — scroll the focused pane's history.
    Scroll,
    /// `Ctrl+Shift+Enter` — toggle the focused pane's dock state.
    ToggleDock,
}

impl WindowChord {
    /// A stable, allocation-free name for the diagnostic trace ([`crate::diag`]).
    fn label(&self) -> &'static str {
        match self {
            Self::CycleFocus => "CycleFocus",
            Self::Scroll => "Scroll",
            Self::ToggleDock => "ToggleDock",
        }
    }
}

/// Recognize a reserved window chord from `key` + `modifiers`, or `None` for a
/// normal keystroke (which injects). Pure — the chord-decision is separated from
/// the side-effecting inject path and unit-tested directly. Precedence matches the
/// historical if-ladder (Ctrl+Page before Shift+Page); `Ctrl+Shift+Enter` is
/// essentially unbound in TUIs (terminals cannot encode it distinctly), so it
/// steals no app key.
fn window_chord(key: &str, modifiers: Modifiers) -> Option<WindowChord> {
    let is_page = matches!(key, "PageUp" | "PageDown");
    if modifiers.ctrl && is_page {
        Some(WindowChord::CycleFocus)
    } else if modifiers.shift && is_page {
        Some(WindowChord::Scroll)
    } else if modifiers.ctrl && modifiers.shift && key == "Enter" {
        Some(WindowChord::ToggleDock)
    } else {
        None
    }
}

/// Route a focused keystroke to the **focused pane's** PTY. The roving-tabindex
/// gate maps `focused` to a pane tile ([`pane_index_of`]); a non-pane / absent
/// focus is a no-op (falls through to the shell default). A reserved
/// [`WindowChord`] ([`window_chord`]) acts on the window/layout and never reaches
/// the PTY (focus-cycle / scrollback / dock-toggle); a DISCRETE chord (dock-toggle /
/// focus-cycle) is dropped on an OS auto-repeat (`repeat`) so it acts once per press
/// (pinion R1071 / PINION-PR27 — a held `Ctrl+Shift+Enter` no longer dock-then-undocks),
/// while scrollback + PTY keys repeat normally. Otherwise the key + W3C modifiers
/// are SENT to the focused pane through the host client
/// ([`HostClient::send_key`](sprag_host::HostClient::send_key)), which encodes them
/// to PTY bytes via the shared host SSOT ([`sprag_host::send_key`]) — the same
/// key->PTY encoder the AI `scene/invoke` path uses (§2 #2; encoding is sprag's,
/// R2.6). Topology B: the GUI's keyboard is a client SEND, not a mutation of its
/// own paint scene. An unencodable key returns `false`, so it falls through rather
/// than injecting nothing. Returning `true` for the encodable keys swallows
/// Escape/Tab from the shell's quit/traverse defaults so a full-screen TUI receives them.
pub(crate) fn route_key(
    focused: Option<&str>,
    key: &str,
    modifiers: Modifiers,
    repeat: bool,
) -> bool {
    let Some(tag) = focused else {
        return false;
    };
    let Some(active) = pane_index_of(tag) else {
        return false;
    };
    // Reserved window chords act on the layout, not the PTY.
    if let Some(chord) = window_chord(key, modifiers) {
        // Discrete chords (dock-toggle, focus-cycle) act once per press: drop an OS
        // auto-repeat re-send (pinion R1071 / PINION-PR27 toggle-class contract), or a
        // held `Ctrl+Shift+Enter` dock-then-undocks in the multi-window state. Scrollback
        // is continuous, so a held `Shift+PageUp` keeps scrolling.
        if repeat && !matches!(chord, WindowChord::Scroll) {
            crate::diag::chord(chord.label(), "drop-repeat", active);
            return false;
        }
        crate::diag::chord(chord.label(), "act", active);
        match chord {
            WindowChord::CycleFocus => cycle_focus(active, key),
            WindowChord::Scroll => scroll_view(active, key),
            WindowChord::ToggleDock => {
                crate::dock::toggle_pane_floating(active);
                // Keep typing in the same pane: focus it (a no-op on undock —
                // already focused; correct on dock-back so a dropped window cannot
                // strand focus).
                pinion_core::focus_request::request(pane_tag(active));
            }
        }
        return true;
    }
    // Any other key is a live interaction with the focused pane: snap its view to
    // the live bottom (offset_y == max), then SEND it to that pane through the host
    // client (topology B: the GUI's keyboard is a client SEND, not a mutation of its
    // own paint scene — the same key->PTY encoder the AI `scene/invoke` path uses,
    // and over the wire this becomes an RPC send to the host). An unencodable key
    // returns `false`, so it falls through to the shell default rather than swallowing.
    let scroll = crate::scrollbar::use_pane_scroll(active);
    scroll.scroll_to(0, scroll.max().1);
    use_terminal()
        .host
        .send_key(active, key, to_input_mods(modifiers))
}

/// Map pinion's key [`Modifiers`] to the encoder's [`sprag_input::Modifiers`]:
/// pinion's `meta` (Cmd / Super / Win) is the encoder's `sup` (the only rename).
fn to_input_mods(m: Modifiers) -> sprag_input::Modifiers {
    sprag_input::Modifiers {
        ctrl: m.ctrl,
        alt: m.alt,
        shift: m.shift,
        sup: m.meta,
    }
}

/// Route an IME composition (Hangul / CJK / any composed input) to the **focused
/// pane**: mirror the in-progress preedit into that pane's [`use_preedit`] overlay
/// Signal and write only the committed text to its PTY. See
/// `sprag_grid::overlay_preedit` for why a terminal must render the preedit itself
/// (winit + XIM) and the display-only contract.
///
/// - [`Start`](CompositionEvent::Start) / [`Cancel`](CompositionEvent::Cancel):
///   clear the focused pane's overlay (begin / abort).
/// - [`Update`](CompositionEvent::Update): mirror the preedit text into that
///   pane's overlay — NOT written to the PTY.
/// - [`Commit`](CompositionEvent::Commit): clear the overlay and write the text
///   **literally** to the focused pane through the host client
///   ([`HostClient::send_text`](sprag_host::HostClient::send_text)) — the same
///   text->PTY seam the AI peer drives, bypassing the key encoder. An empty Commit
///   is the cancel-shaped end (clearing is the whole job; no write).
///
/// Focus-gated to a pane tile like [`route_key`]. Returning `true` reports the
/// event handled AND — because `view` subscribes to [`use_preedit`] — arms a
/// repaint via pinion's R705.1 reactive-dirty bridge, so the composition repaints
/// live. (`WidgetView::ime_caret_rect`, Hanja-candidate positioning, is still
/// unwired — polish, not shown during plain Hangul.)
pub(crate) fn route_composition(focused: Option<&str>, event: &CompositionEvent) -> bool {
    let Some(tag) = focused else {
        return false;
    };
    let Some(active) = pane_index_of(tag) else {
        return false;
    };
    // Composition happens at the live prompt, so snap the focused pane's
    // scrollback view to the live bottom once for any composition activity — one
    // site, and it also covers a Commit that arrives without a preceding Start.
    let scroll = crate::scrollbar::use_pane_scroll(active);
    scroll.scroll_to(0, scroll.max().1);
    match event {
        // Begin / abort: clear any (stale) overlay. Update carries the live text.
        CompositionEvent::Start | CompositionEvent::Cancel => {
            use_preedit(active).set(String::new());
            true
        }
        // Preedit progresses (ㅎ -> 하 -> 한): mirror it into the pane's overlay.
        CompositionEvent::Update(text) => {
            use_preedit(active).set(text.clone());
            true
        }
        // Finished: clear the overlay, then write the literal committed text to the
        // focused pane through the host client (the same text->PTY seam the AI
        // `scene/invoke` path uses, bypassing the key encoder). An empty commit is a
        // no-op (send_text no-ops it).
        CompositionEvent::Commit(text) => {
            use_preedit(active).set(String::new());
            if !text.is_empty() {
                // The committed-text write. A PTY-write failure has nothing for an
                // IME commit to fall through to (unlike a keystroke, which returns
                // false to the shell), so the result is intentionally discarded —
                // made explicit now that `send_text` is `#[must_use]`.
                let _ = use_terminal().host.send_text(active, text);
            }
            true
        }
        // `CompositionEvent` is `#[non_exhaustive]`: a future variant is
        // unhandled (falls through to the shell default), not silently consumed.
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TerminalViewer;
    use crate::terminal::seed_terminal;
    use pinion_core::Scene;
    use pinion_core::WidgetCore;
    use pinion_core::scene::ContainerNode;
    use sprag_host::Host;
    use sprag_terminal::{CommandBuilder, SessionHandle};
    use std::thread::sleep;
    use std::time::{Duration, Instant};

    /// A long-lived `cat` pane (echoes stdin, keeps the PTY open across keys).
    fn cat() -> CommandBuilder {
        let mut c = CommandBuilder::new("/bin/sh");
        c.arg("-c");
        c.arg("cat");
        c.env("TERM", "dumb");
        c
    }

    /// Poll a handle's row 0 until it contains `needle` or the deadline passes.
    fn wait_for_row0(handle: &SessionHandle, needle: &str) -> String {
        let start = Instant::now();
        let mut row0 = String::new();
        while start.elapsed() < Duration::from_secs(5) {
            row0 = handle.with_screen(|screen| screen.row_text(0));
            if row0.contains(needle) {
                break;
            }
            sleep(Duration::from_millis(20));
        }
        row0
    }

    /// End-to-end multi-pane routing THROUGH `apply_key`: a focused keystroke reaches
    /// ONLY the focused pane's PTY (route_key resolves the focus tag -> pane index ->
    /// `host.send_key(active)`), and a sibling pane receives nothing. A seeded 2-pane
    /// `cat` terminal makes the echo deterministic. This is the full public path
    /// (apply_key -> route_key -> client send), restoring the pre-R110 net that R110's
    /// direct `host.send_key` + negative-only gate tests had dropped (session review).
    #[test]
    fn apply_key_routes_to_the_focused_pane_only() {
        let host = Host::new((40, 6));
        host.spawn(cat(), "cat".to_owned(), 40, 6, None).unwrap();
        host.spawn(cat(), "cat".to_owned(), 40, 6, None).unwrap();
        let h0 = host.pane_handle(0);
        let h1 = host.pane_handle(1);
        let owner = Owner::new();
        owner.run(|| {
            seed_terminal(host); // use_terminal() now returns these two cat panes
            let mut scene = Scene::Container(ContainerNode::new(Vec::new()));
            // Focus pane 1: each key routes to pane 1's PTY only.
            for ch in ["h", "i"] {
                assert!(TerminalViewer::apply_key(
                    &mut scene,
                    Some(pane_tag(1)),
                    ch,
                    Modifiers::default()
                ));
            }
        });
        assert!(
            wait_for_row0(&h1, "hi").contains("hi"),
            "the focused pane echoes the keys"
        );
        assert!(
            !h0.with_screen(|s| s.row_text(0)).contains("hi"),
            "the unfocused pane received nothing",
        );
    }

    /// `route_key`'s focus gate (via `apply_key`): a non-pane focus (the cosmetic
    /// root tag) and no focus are no-ops that fall through to the shell default —
    /// without resolving a pane or touching a PTY. The inject leg is
    /// [`send_key_routes_to_the_named_pane_only`].
    #[test]
    fn apply_key_gates_on_a_pane_focus() {
        let owner = Owner::new();
        owner.run(|| {
            let mut scene = Scene::Container(ContainerNode::new(Vec::new()));
            assert!(!TerminalViewer::apply_key(
                &mut scene,
                Some("sprag_gui"),
                "x",
                Modifiers::default()
            ));
            assert!(!TerminalViewer::apply_key(
                &mut scene,
                None,
                "x",
                Modifiers::default()
            ));
        });
    }

    /// The composition overlay + focus gate (PTY-free): `Update` mirrors into the
    /// focused pane's [`use_preedit`] overlay WITHOUT touching any PTY, a non-pane
    /// composition is a no-op, and an empty commit clears the overlay (the empty
    /// guard skips `send_text`, so this spawns no session). The committed-text WRITE
    /// is [`send_text_writes_committed_text_to_the_named_pane`]. `apply_composition`
    /// ignores its scene arg now (input is a client send), so a trivial scene suffices.
    ///
    /// Scope (honest): synthetic `CompositionEvent`s exercise sprag's preedit-overlay
    /// seam, NOT the live platform IME's `Start`/`Update`/`Commit` sequencing
    /// (verified separately against ibus-hangul in a live window).
    #[test]
    fn apply_composition_overlays_preedit_without_touching_the_pty() {
        let owner = Owner::new();
        owner.run(|| {
            let mut scene = Scene::Container(ContainerNode::new(Vec::new()));
            let commit = |t: &str| CompositionEvent::Commit(t.to_owned());
            // Focus gate: a non-pane composition is a no-op.
            assert!(!TerminalViewer::apply_composition(
                &mut scene,
                None,
                &commit("한")
            ));
            // Update mirrors into pane 0's overlay (NOT written to any PTY).
            assert!(TerminalViewer::apply_composition(
                &mut scene,
                Some(pane_tag(0)),
                &CompositionEvent::Update("ㅎ".to_owned())
            ));
            assert_eq!(
                use_preedit(0).get(),
                "ㅎ",
                "the overlay mirrors the in-progress composition"
            );
            // An empty commit clears the overlay and writes nothing.
            assert!(TerminalViewer::apply_composition(
                &mut scene,
                Some(pane_tag(0)),
                &commit("")
            ));
            assert_eq!(
                use_preedit(0).get(),
                "",
                "an empty commit clears the overlay"
            );
        });
    }

    /// End-to-end IME commit THROUGH `apply_composition`: a non-empty Commit on the
    /// focused pane clears the overlay AND writes the literal UTF-8 to that pane's PTY
    /// (route_composition's Commit arm -> `host.send_text`), echoed through the
    /// cooked-mode `cat`. Uses a seeded `cat` terminal for a deterministic echo. This
    /// restores the pre-R110 commit-write net that R110's direct `host.send_text`
    /// (which bypassed route_composition's Commit arm) had dropped (session review).
    #[test]
    fn apply_composition_commit_writes_to_the_focused_pane() {
        let host = Host::new((40, 6));
        host.spawn(cat(), "cat".to_owned(), 40, 6, None).unwrap();
        let handle = host.pane_handle(0);
        let owner = Owner::new();
        owner.run(|| {
            seed_terminal(host); // use_terminal() now returns this cat pane
            let mut scene = Scene::Container(ContainerNode::new(Vec::new()));
            assert!(TerminalViewer::apply_composition(
                &mut scene,
                Some(pane_tag(0)),
                &CompositionEvent::Commit("한글".to_owned())
            ));
            assert_eq!(use_preedit(0).get(), "", "the commit clears the overlay");
        });
        assert!(
            wait_for_row0(&handle, "한글").contains("한글"),
            "the committed text reached the focused pane's PTY",
        );
    }

    /// The R705.1 contract sprag relies on for live preedit: a composing `set` of
    /// a pane's preedit Signal — which `view` subscribes to by reading it every
    /// frame — flips the root owner dirty, which the shell's reactive-dirty bridge
    /// turns into a repaint. Pins the coupling so a pinion bridge regression fails
    /// HERE rather than silently degrading live IME.
    #[test]
    fn preedit_set_flips_owner_dirty_for_repaint() {
        let owner = Owner::new();
        // Mirror `view`: subscribe the owner by reading the preedit Signal.
        owner.run(|| {
            let _ = use_preedit(0).get();
        });
        owner.clear_dirty();
        assert!(!owner.is_dirty(), "clean after a subscribing read");
        // A composing Update sets the subscribed Signal.
        owner.run(|| use_preedit(0).set("ㅎ".to_owned()));
        assert!(
            owner.is_dirty(),
            "a preedit set flips the owner dirty (R705.1 repaint arm)"
        );
    }

    #[test]
    fn page_delta_signs_match_scroll_direction() {
        // PageUp walks toward older history -> DECREASE offset_y (top-anchored).
        assert_eq!(page_delta("PageUp", 13), -13);
        // PageDown walks back toward the live bottom -> INCREASE offset_y.
        assert_eq!(page_delta("PageDown", 13), 13);
        // A non-page key does not scroll.
        assert_eq!(page_delta("a", 13), 0);
    }

    #[test]
    fn window_chord_recognizes_the_reserved_chords() {
        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        let ctrl_shift = Modifiers {
            ctrl: true,
            shift: true,
            ..Modifiers::default()
        };
        assert_eq!(window_chord("PageUp", ctrl), Some(WindowChord::CycleFocus));
        assert_eq!(
            window_chord("PageDown", ctrl),
            Some(WindowChord::CycleFocus)
        );
        assert_eq!(window_chord("PageUp", shift), Some(WindowChord::Scroll));
        assert_eq!(
            window_chord("Enter", ctrl_shift),
            Some(WindowChord::ToggleDock)
        );
        // Precedence: Ctrl wins over Shift on Page (the historical if-ladder order).
        assert_eq!(
            window_chord("PageUp", ctrl_shift),
            Some(WindowChord::CycleFocus)
        );
        // A normal keystroke is not a chord (it injects).
        assert_eq!(window_chord("a", Modifiers::default()), None);
        assert_eq!(window_chord("Enter", Modifiers::default()), None);
        assert_eq!(window_chord("PageUp", Modifiers::default()), None);
    }

    #[test]
    fn next_focus_wraps_between_panes() {
        // Down = next, Up = previous; both wrap over the occupied slots. Contiguous
        // (the boot set) behaves as the former modular wrap.
        let all = [0, 1, 2];
        assert_eq!(next_focus(0, "PageDown", &all), Some(1));
        assert_eq!(next_focus(2, "PageDown", &all), Some(0)); // wrap forward
        assert_eq!(next_focus(0, "PageUp", &all), Some(2)); // wrap backward
        assert_eq!(next_focus(1, "PageUp", &all), Some(0));
        // A single pane has nowhere to switch to; a non-cycle key is None.
        assert_eq!(next_focus(0, "PageDown", &[0]), None);
        assert_eq!(next_focus(0, "Enter", &all), None);
        // Non-contiguous slots (a closed pane left a hole at slot 1): cycling STEPS
        // OVER the hole rather than landing on it (the Round 2b live-correct path).
        let holed = [0, 2, 3];
        assert_eq!(next_focus(0, "PageDown", &holed), Some(2));
        assert_eq!(next_focus(3, "PageDown", &holed), Some(0)); // wrap forward
        assert_eq!(next_focus(0, "PageUp", &holed), Some(3)); // wrap backward
        // `active` not among the occupied slots (a just-closed slot) -> nowhere.
        assert_eq!(next_focus(1, "PageDown", &holed), None);
    }

    /// The `Ctrl+PageUp/Down` focus-cycle chord is handled (`true`), reaches no
    /// PTY, and fires a pinion [`focus_request`](pinion_core::focus_request) for
    /// the correct neighbour pane — the end-to-end focus-switch wiring
    /// (`route_key` -> `cycle_focus` -> `focus_request`), short of the winit event
    /// loop draining it (pinion's own, proven). The Ctrl branch returns before
    /// touching the scene's External, so a minimal model scene suffices;
    /// `cycle_focus` reads `use_terminal()`'s pane count (the 2 boot panes).
    #[test]
    fn ctrl_page_chord_cycles_focus_without_touching_the_pty() {
        let owner = Owner::new();
        owner.run(|| {
            // apply_key ignores the scene now (input is a client send), so a
            // trivial scene suffices — these tests exercise chords, not the PTY.
            let mut scene = Scene::Container(ContainerNode::new(Vec::new()));
            let ctrl = Modifiers {
                ctrl: true,
                ..Modifiers::default()
            };
            let _ = pinion_core::focus_request::drain(); // clear any stale request
            // Ctrl+PageDown from pane 0 requests focus on the next pane.
            assert!(TerminalViewer::apply_key(
                &mut scene,
                Some(pane_tag(0)),
                "PageDown",
                ctrl
            ));
            assert_eq!(
                pinion_core::focus_request::drain().as_deref(),
                Some(pane_tag(1)),
                "Ctrl+PageDown cycles focus to the next pane",
            );
            // Ctrl+PageUp from pane 0 wraps backward to the last pane (pane 1 of 2).
            assert!(TerminalViewer::apply_key(
                &mut scene,
                Some(pane_tag(0)),
                "PageUp",
                ctrl
            ));
            assert_eq!(
                pinion_core::focus_request::drain().as_deref(),
                Some(pane_tag(1)),
                "Ctrl+PageUp wraps focus backward",
            );
        });
    }

    /// `Ctrl+Shift+Enter` toggles the focused pane's dock state — handled
    /// (`true`), reaches no PTY (the branch returns before the External invoke),
    /// and round-trips the dock topology Signal (undock adds a window, dock-back
    /// removes it). The window-management sibling of the focus-cycle chord.
    #[test]
    fn ctrl_shift_enter_toggles_dock_without_touching_the_pty() {
        let owner = Owner::new();
        owner.run(|| {
            // apply_key ignores the scene now (input is a client send), so a
            // trivial scene suffices — these tests exercise chords, not the PTY.
            let mut scene = Scene::Container(ContainerNode::new(Vec::new()));
            let windows = crate::dock::use_windows_topology();
            assert_eq!(windows.get().len(), 1, "starts with the main window only");
            let ctrl_shift = Modifiers {
                ctrl: true,
                shift: true,
                ..Modifiers::default()
            };
            // Undock the focused pane (handled, not to the PTY).
            assert!(TerminalViewer::apply_key(
                &mut scene,
                Some(pane_tag(0)),
                "Enter",
                ctrl_shift
            ));
            assert_eq!(
                windows.get().len(),
                2,
                "the focused pane undocked into its own window"
            );
            // The same chord docks it back.
            assert!(TerminalViewer::apply_key(
                &mut scene,
                Some(pane_tag(0)),
                "Enter",
                ctrl_shift
            ));
            assert_eq!(windows.get().len(), 1, "docked back");
        });
    }

    /// Auto-repeat of the DISCRETE dock chord is dropped (consumes pinion R1071 /
    /// PINION-PR27): the live shell drives `apply_key_repeat` with the platform
    /// `KeyEvent.repeat` flag, and a held `Ctrl+Shift+Enter` must toggle ONCE per
    /// press, not on every OS auto-repeat — that re-send is what made it
    /// "dock-then-undock" in the multi-window state. The leading press
    /// (`repeat == false`) toggles; an auto-repeat (`repeat == true`) is a no-op. A
    /// scrollback chord is CONTINUOUS, so it is NOT dropped (a held `Shift+PageUp`
    /// keeps scrolling) — `route_key` drops only the discrete chords.
    #[test]
    fn dock_chord_auto_repeat_is_dropped_scroll_repeats() {
        let owner = Owner::new();
        owner.run(|| {
            // apply_key ignores the scene now (input is a client send), so a
            // trivial scene suffices — these tests exercise chords, not the PTY.
            let mut scene = Scene::Container(ContainerNode::new(Vec::new()));
            let windows = crate::dock::use_windows_topology();
            let ctrl_shift = Modifiers {
                ctrl: true,
                shift: true,
                ..Modifiers::default()
            };
            let shift = Modifiers {
                shift: true,
                ..Modifiers::default()
            };

            // An OS auto-repeat re-send of the dock chord is dropped: no-op, no window.
            assert!(
                !TerminalViewer::apply_key_repeat(
                    &mut scene,
                    Some(pane_tag(0)),
                    "Enter",
                    ctrl_shift,
                    true
                ),
                "a held dock chord's auto-repeat is a no-op",
            );
            assert_eq!(windows.get().len(), 1, "auto-repeat did not undock");

            // The leading press (repeat = false) toggles once.
            assert!(TerminalViewer::apply_key_repeat(
                &mut scene,
                Some(pane_tag(0)),
                "Enter",
                ctrl_shift,
                false
            ));
            assert_eq!(windows.get().len(), 2, "the leading press undocked once");

            // A scrollback chord is continuous — a held Shift+PageUp keeps scrolling
            // (NOT dropped), so only the discrete chords are repeat-gated.
            assert!(
                TerminalViewer::apply_key_repeat(
                    &mut scene,
                    Some(pane_tag(0)),
                    "PageUp",
                    shift,
                    true
                ),
                "scrollback repeats on a held key",
            );
        });
    }

    #[test]
    fn scroll_state_is_per_pane_and_defaults_live() {
        use crate::scrollbar::use_pane_scroll;
        let owner = Owner::new();
        // Each pane boots at the live bottom (offset_y 0 == max 0) and scrolls
        // independently.
        assert_eq!(
            owner.run(|| use_pane_scroll(0).offset_y()),
            0,
            "pane 0 boots live"
        );
        assert_eq!(
            owner.run(|| use_pane_scroll(1).offset_y()),
            0,
            "pane 1 boots live"
        );
        owner.run(|| {
            let s = use_pane_scroll(1);
            s.set_max(0, 20);
            s.scroll_to(0, 7);
        });
        assert_eq!(owner.run(|| use_pane_scroll(1).offset_y()), 7);
        assert_eq!(
            owner.run(|| use_pane_scroll(0).offset_y()),
            0,
            "pane 0 is unaffected (per-pane slots)"
        );
    }

    /// `apply_key` treats Shift+PageUp as a scroll (handled, not sent to the PTY)
    /// of the focused pane and snaps that pane's view back to the live bottom
    /// (offset_y == max) on any other (typed) key.
    #[test]
    fn apply_key_scrolls_and_snaps_to_bottom() {
        use crate::scrollbar::use_pane_scroll;
        let owner = Owner::new();
        owner.run(|| {
            // The model scene over the live boot pane 0 (same pane use_terminal
            // caches), so scroll_view reads the pane the routing addresses.
            // apply_key ignores the scene now (input is a client send), so a
            // trivial scene suffices — these tests exercise chords, not the PTY.
            let mut scene = Scene::Container(ContainerNode::new(Vec::new()));
            // Shift+PageUp is consumed as a scroll (true = handled, not the PTY).
            let shift = Modifiers {
                shift: true,
                ..Modifiers::default()
            };
            assert!(TerminalViewer::apply_key(
                &mut scene,
                Some(pane_tag(0)),
                "PageUp",
                shift
            ));
            // Pause partway up history (offset_y 3 of a 10-row depth), then a typed
            // key snaps to the live bottom (offset_y == max).
            let scroll = use_pane_scroll(0);
            scroll.set_max(0, 10);
            scroll.scroll_to(0, 3);
            assert!(TerminalViewer::apply_key(
                &mut scene,
                Some(pane_tag(0)),
                "a",
                Modifiers::default()
            ));
            assert_eq!(
                scroll.offset_y(),
                scroll.max().1,
                "typing snaps to the live bottom"
            );
            assert_eq!(
                scroll.offset_y(),
                10,
                "the live bottom is the reconciled max"
            );
        });
    }
}
