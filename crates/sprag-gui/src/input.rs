//! Input routing: a focused keystroke / IME commit -> the pane's PTY through
//! the one `invoke(...)` wire, plus the scrollback-view offset those keys snap.
//! The [`TerminalViewer`](crate::TerminalViewer) `apply_key` / `apply_composition`
//! trait methods delegate here. See the crate-root "Input" / "Scrollback" docs.

use crate::terminal::use_terminal;
use crate::ROOT_TAG;
use pinion_core::external::IntrospectValue;
use pinion_core::reactive::{Owner, Signal};
use pinion_core::{CompositionEvent, Modifiers, Scene};

/// `Owner::cache` key for the scrollback view offset (lines scrolled up from
/// the live bottom; `0` = live).
const SCROLL_KEY: &str = "sprag_gui.scroll";

/// The scrollback view offset (lines scrolled up from the live bottom), an
/// `Owner::cache`-backed [`Signal`] so `apply_key` writes it and `view` reads it
/// reactively (a `set` re-renders the view). `0` = live (follow the bottom).
pub(crate) fn use_scroll_offset() -> Signal<usize> {
    let owner = Owner::current().expect("use_scroll_offset() requires an active Owner scope");
    owner.cache(SCROLL_KEY, || Signal::new(0_usize)).as_ref().clone()
}

/// `Owner::cache` key for the IME preedit (in-progress composition) overlay.
const PREEDIT_KEY: &str = "sprag_gui.preedit";

/// The IME preedit (in-progress composition) string, an `Owner::cache`-backed
/// [`Signal`] that [`route_composition`] writes on each composition event and
/// `view` reads every frame to overlay at the cursor (see
/// [`sprag_host::pane_view_scene_scrolled_with_preedit`] /
/// `sprag_grid::overlay_preedit` for why a terminal renders the preedit itself).
/// Empty = not composing; display-only (the preedit never reaches the PTY — only
/// a `Commit` writes). Because `view` reads it every frame, a `set` flips the root
/// owner dirty so the shell's R705.1 reactive bridge arms a redraw — the
/// composition repaints live as you type.
pub(crate) fn use_preedit() -> Signal<String> {
    let owner = Owner::current().expect("use_preedit() requires an active Owner scope");
    owner.cache(PREEDIT_KEY, || Signal::new(String::new())).as_ref().clone()
}

/// The scrollback offset after a `PageUp` / `PageDown` of `page` rows from
/// `current`, clamped to `[0, scrollback_len]`. Pure, so it is unit-testable;
/// `PageUp` walks into history (clamped at the depth), `PageDown` back toward
/// the live bottom (saturating at `0`).
fn next_scroll_offset(key: &str, current: usize, page: usize, scrollback_len: usize) -> usize {
    match key {
        "PageUp" => (current + page).min(scrollback_len),
        "PageDown" => current.saturating_sub(page),
        _ => current,
    }
}

/// Adjust the scrollback offset for a `Shift+PageUp` / `Shift+PageDown`, clamped
/// to the pane's retained scrollback depth. A page is the viewport height less
/// one row (one row of overlap for continuity). Reads the live pane for the
/// depth + row count; called from `apply_key` (outside any cache factory).
fn scroll_view(key: &str) {
    let terminal = use_terminal();
    let offset = use_scroll_offset();
    let (scrollback_len, rows) = terminal
        .boot_pane()
        .session()
        .with_screen(|screen| (screen.scrollback_len(), screen.rows()));
    let page = usize::from(rows).saturating_sub(1).max(1);
    offset.set(next_scroll_offset(key, offset.get(), page, scrollback_len));
}

/// Route a focused keystroke to the boot pane's PTY. The roving-tabindex gate
/// (`focused == Some(ROOT_TAG)`) keeps keys scoped to the terminal, and the
/// key + W3C modifiers go to the root [`SpragPaneExternal`](sprag_host::SpragPaneExternal)'s
/// `invoke("key", {key, ctrl, alt, shift, super})` — the same `scene/invoke`
/// wire the RPC client uses (§2 #2), where the sprag-owned encoder turns the key
/// into PTY bytes (R2.6). An unencodable key (a bare modifier press, an
/// `Fn`-style key the encoder does not map) returns `Err` -> `false`, so it
/// falls through to the shell default rather than injecting nothing silently.
/// The terminal keys that matter all encode: returning `true` for them swallows
/// the key from the shell's Escape-quits / Tab-traverses defaults, so Escape and
/// Tab reach a full-screen TUI (vim) instead of the window.
pub(crate) fn route_key(scene: &mut Scene, focused: Option<&str>, key: &str, modifiers: Modifiers) -> bool {
    if focused != Some(ROOT_TAG) {
        return false;
    }
    // Scrollback: Shift+PageUp / Shift+PageDown scroll the history view and do
    // NOT reach the PTY (a terminal app sees an unmodified PageUp). Every
    // other key is a live interaction, so it first snaps the view back to the
    // bottom — you type at the prompt, which is at the live bottom.
    if modifiers.shift && matches!(key, "PageUp" | "PageDown") {
        scroll_view(key);
        return true;
    }
    use_scroll_offset().set(0);
    let Scene::External(node) = scene else {
        return false;
    };
    let Some(intro) = node.handle.introspect_mut() else {
        return false;
    };
    let args = serde_json::json!({
        "key": key,
        "ctrl": modifiers.ctrl,
        "alt": modifiers.alt,
        "shift": modifiers.shift,
        // pinion's `meta` (Cmd/Super/Win) maps to the encoder's "super".
        "super": modifiers.meta,
    });
    intro.invoke("key", IntrospectValue::Json(args)).is_ok()
}

/// Route an IME composition (Hangul / CJK / any composed input): mirror the
/// in-progress preedit into the [`use_preedit`] overlay Signal and write only the
/// committed text to the PTY. See `sprag_grid::overlay_preedit` for why a terminal
/// must render the preedit itself (winit + XIM) and the display-only contract.
///
/// - [`Start`](CompositionEvent::Start) / [`Cancel`](CompositionEvent::Cancel):
///   clear the overlay (begin / abort).
/// - [`Update`](CompositionEvent::Update): mirror the preedit text into the
///   overlay — NOT written to the PTY.
/// - [`Commit`](CompositionEvent::Commit): clear the overlay and write the text
///   **literally** via the root [`SpragPaneExternal`](sprag_host::SpragPaneExternal)'s
///   `invoke("text", …)` — composed text is not a keystroke, so it bypasses the
///   key encoder (the same `scene/invoke` wire the AI peer drives, §2 #2). For a
///   cooked-mode program the committed glyphs then arrive echoed through the PTY;
///   a non-echoing program (a password prompt, a TUI) shows them only when it next
///   redraws. An empty Commit is the cancel-shaped end (clearing is the whole job).
///
/// Focus-gated on [`ROOT_TAG`] like [`route_key`]. Returning `true` does double
/// duty: it reports the event handled AND — because `view` subscribes to
/// [`use_preedit`] — arms a repaint via pinion's R705.1 reactive-dirty bridge, so
/// the composition repaints live. (`WidgetView::ime_caret_rect`, Hanja-candidate
/// positioning, is still unwired — polish, not shown during plain Hangul.)
pub(crate) fn route_composition(
    scene: &mut Scene,
    focused: Option<&str>,
    event: &CompositionEvent,
) -> bool {
    if focused != Some(ROOT_TAG) {
        return false;
    }
    // Composition happens at the live prompt, so snap the scrollback view to the
    // bottom once for any composition activity — one site (was duplicated across
    // Start and Commit), and it now also covers a Commit that arrives without a
    // preceding Start (some IMEs do that).
    use_scroll_offset().set(0);
    match event {
        // Begin / abort: clear any (stale) overlay. Update carries the live text.
        CompositionEvent::Start | CompositionEvent::Cancel => {
            use_preedit().set(String::new());
            true
        }
        // Preedit progresses (ㅎ -> 하 -> 한): mirror it into the overlay.
        CompositionEvent::Update(text) => {
            use_preedit().set(text.clone());
            true
        }
        // Finished: clear the overlay, then write the literal committed text.
        CompositionEvent::Commit(text) => {
            use_preedit().set(String::new());
            if !text.is_empty()
                && let Scene::External(node) = scene
                && let Some(intro) = node.handle.introspect_mut()
            {
                let _ = intro.invoke("text", IntrospectValue::Text(text.clone()));
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
    use pinion_core::scene::ExternalNode;
    use pinion_core::WidgetCore;
    use sprag_host::SpragPaneExternal;
    use sprag_terminal::{CommandBuilder, Workspace};
    use std::thread::sleep;
    use std::time::{Duration, Instant};

    /// End-to-end keyboard input: build the model scene the shell assembles
    /// (`Scene::External(SpragPaneExternal, ROOT_TAG)`) over a live `cat` pane
    /// and drive `apply_key`. The focus gate is deterministic; the typed text
    /// echoes back through the cooked-mode PTY (bounded poll, the sprag-terminal
    /// test idiom).
    #[test]
    fn apply_key_routes_focused_keystrokes_to_the_pane() {
        let mut ws = Workspace::new((40, 6));
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg("cat"); // echoes stdin; keeps the PTY open across the keys
        command.env("TERM", "dumb");
        let id = ws.spawn(command, "cat".to_owned(), 40, 6).unwrap();
        let handle = ws.pane(id).unwrap().handle();
        let mut scene = Scene::External(
            ExternalNode::new(Box::new(SpragPaneExternal::new(handle.clone()))).with_tag(ROOT_TAG),
        );

        // apply_key runs inside the shell's root Owner scope (it reads the
        // scrollback-offset Signal); mirror that here.
        let owner = Owner::new();
        owner.run(|| {
            // Focus gate: an unfocused / wrong-tag keystroke is a no-op.
            assert!(!TerminalViewer::apply_key(&mut scene, None, "a", Modifiers::default()));
            assert!(!TerminalViewer::apply_key(&mut scene, Some("other"), "a", Modifiers::default()));
            // Focused: each key is injected and the cooked-mode PTY echoes it back.
            for ch in ["h", "i"] {
                assert!(TerminalViewer::apply_key(&mut scene, Some(ROOT_TAG), ch, Modifiers::default()));
            }
        });
        let start = Instant::now();
        let mut row0 = String::new();
        while start.elapsed() < Duration::from_secs(5) {
            row0 = handle.with_screen(|screen| screen.row_text(0));
            if row0.contains("hi") {
                break;
            }
            sleep(Duration::from_millis(20));
        }
        assert!(row0.contains("hi"), "typed keys echo to the pane screen; row0 = {row0:?}");
    }

    /// Verify the composition lifecycle: `Update` (preedit) mirrors into the
    /// [`use_preedit`] overlay Signal WITHOUT touching the PTY, and `Commit`
    /// clears the overlay and WRITES the literal UTF-8 (the R31 `Commit` ->
    /// `invoke("text")` -> `session.write` seam), confirmed by the text echoing
    /// back through the cooked-mode `cat` PTY.
    ///
    /// Scope (honest): this drives synthetic `CompositionEvent`s, so it exercises
    /// sprag's preedit-overlay + commit-write seams, NOT the live platform IME's
    /// `Start`/`Update`/`Commit` sequencing (the IME's own, reproducible only in
    /// a live window — verified separately against ibus-hangul). The focus gate
    /// is a deterministic no-op.
    #[test]
    fn apply_composition_overlays_preedit_and_writes_commit() {
        let mut ws = Workspace::new((40, 6));
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg("cat"); // echoes stdin; keeps the PTY open
        command.env("TERM", "dumb");
        let id = ws.spawn(command, "cat".to_owned(), 40, 6).unwrap();
        let handle = ws.pane(id).unwrap().handle();
        let mut scene = Scene::External(
            ExternalNode::new(Box::new(SpragPaneExternal::new(handle.clone()))).with_tag(ROOT_TAG),
        );

        // apply_composition reads the scrollback-offset + preedit Signals, so run
        // it inside a root Owner scope (mirrors the shell).
        let owner = Owner::new();
        owner.run(|| {
            let commit = |t: &str| CompositionEvent::Commit(t.to_owned());
            // Focus gate: a wrong-tag composition is a no-op.
            assert!(!TerminalViewer::apply_composition(&mut scene, None, &commit("한")));
            // Preedit (Update) is handled (mirrored into the overlay) but is NOT
            // written to the PTY — only a Commit writes.
            let preedit = CompositionEvent::Update("ㅎ".to_owned());
            assert!(TerminalViewer::apply_composition(&mut scene, Some(ROOT_TAG), &preedit));
            assert_eq!(use_preedit().get(), "ㅎ", "the overlay mirrors the in-progress composition");
            // An empty commit is the cancel-shaped end: it clears the overlay and
            // writes nothing.
            assert!(TerminalViewer::apply_composition(&mut scene, Some(ROOT_TAG), &commit("")));
            assert_eq!(use_preedit().get(), "", "an empty commit clears the overlay");
            // A real commit clears the overlay and writes the literal Hangul.
            let update_han = CompositionEvent::Update("한".to_owned());
            assert!(TerminalViewer::apply_composition(&mut scene, Some(ROOT_TAG), &update_han));
            assert!(TerminalViewer::apply_composition(&mut scene, Some(ROOT_TAG), &commit("한글")));
            assert_eq!(use_preedit().get(), "", "the commit clears the overlay");
        });
        let start = Instant::now();
        let mut row0 = String::new();
        while start.elapsed() < Duration::from_secs(5) {
            row0 = handle.with_screen(|screen| screen.row_text(0));
            if row0.contains("한글") {
                break;
            }
            sleep(Duration::from_millis(20));
        }
        assert!(
            row0.contains("한글"),
            "committed IME text echoes to the pane screen; row0 = {row0:?}"
        );
    }

    /// The R705.1 contract sprag relies on for live preedit: a composing `set` of
    /// the preedit Signal — which `view` subscribes to by reading it every frame —
    /// flips the root owner dirty, which is what the shell's reactive-dirty bridge
    /// turns into a repaint. Pins the coupling so a pinion bridge regression fails
    /// HERE rather than silently degrading live IME (only otherwise visible in a
    /// real window).
    #[test]
    fn preedit_set_flips_owner_dirty_for_repaint() {
        let owner = Owner::new();
        // Mirror `view`: subscribe the owner by reading the preedit Signal.
        owner.run(|| {
            let _ = use_preedit().get();
        });
        owner.clear_dirty();
        assert!(!owner.is_dirty(), "clean after a subscribing read");
        // A composing Update sets the subscribed Signal.
        owner.run(|| use_preedit().set("ㅎ".to_owned()));
        assert!(owner.is_dirty(), "a preedit set flips the owner dirty (R705.1 repaint arm)");
    }

    #[test]
    fn next_scroll_offset_clamps_and_saturates() {
        // PageUp accumulates up to the scrollback depth (clamped).
        assert_eq!(next_scroll_offset("PageUp", 0, 10, 25), 10);
        assert_eq!(next_scroll_offset("PageUp", 20, 10, 25), 25); // clamp at depth
        // PageDown walks back toward the live bottom (saturating at 0).
        assert_eq!(next_scroll_offset("PageDown", 25, 10, 25), 15);
        assert_eq!(next_scroll_offset("PageDown", 5, 10, 25), 0); // saturate
        // No scrollback -> stays live regardless of the key.
        assert_eq!(next_scroll_offset("PageUp", 0, 10, 0), 0);
    }

    #[test]
    fn scroll_offset_signal_defaults_live_and_round_trips() {
        let owner = Owner::new();
        assert_eq!(owner.run(|| use_scroll_offset().get()), 0, "boots at the live bottom");
        owner.run(|| use_scroll_offset().set(7));
        assert_eq!(owner.run(|| use_scroll_offset().get()), 7);
    }

    /// `apply_key` treats Shift+PageUp as a scroll (handled, not sent to the PTY)
    /// and snaps the view back to the live bottom on any other (typed) key.
    #[test]
    fn apply_key_scrolls_and_snaps_to_bottom() {
        let owner = Owner::new();
        owner.run(|| {
            // The model scene over the live boot pane (same pane use_terminal
            // caches), so the PTY routing reaches the pane scroll_view reads.
            let handle = use_terminal().pane_handle();
            let mut scene = Scene::External(
                ExternalNode::new(Box::new(SpragPaneExternal::new(handle))).with_tag(ROOT_TAG),
            );
            // Shift+PageUp is consumed as a scroll (true = handled, not the PTY).
            let shift = Modifiers { shift: true, ..Modifiers::default() };
            assert!(TerminalViewer::apply_key(&mut scene, Some(ROOT_TAG), "PageUp", shift));
            // A scrolled-up view snaps to the live bottom when the user types.
            use_scroll_offset().set(5);
            assert!(TerminalViewer::apply_key(&mut scene, Some(ROOT_TAG), "a", Modifiers::default()));
            assert_eq!(use_scroll_offset().get(), 0, "typing snaps to the live bottom");
        });
    }
}
