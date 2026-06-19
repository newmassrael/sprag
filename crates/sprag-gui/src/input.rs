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

/// Route committed IME text (Hangul / CJK / any composed input) to the boot
/// pane's PTY. The platform IME composes off-grid (its own preedit popup) and
/// emits [`CompositionEvent::Commit`] with the finished text; we write it
/// **literally** via the root [`SpragPaneExternal`](sprag_host::SpragPaneExternal)'s
/// `invoke("text", …)` — composed text is not a keystroke, so it bypasses the
/// key encoder (the same `scene/invoke` wire the AI peer drives, §2 #2).
/// `Start`/`Update` (preedit) and `Cancel` are not consumed here: the IME
/// renders the in-progress composition itself; an inline-preedit overlay on the
/// grid is a later round. Focus-gated on [`ROOT_TAG`] like [`route_key`].
pub(crate) fn route_composition(
    scene: &mut Scene,
    focused: Option<&str>,
    event: &CompositionEvent,
) -> bool {
    if focused != Some(ROOT_TAG) {
        return false;
    }
    let CompositionEvent::Commit(text) = event else {
        return false; // preedit / cancel: nothing to insert into the PTY
    };
    if text.is_empty() {
        return false; // empty commit == cancel (the no-data compositionend shape)
    }
    // Committing text is a live interaction — snap the scrollback view back
    // to the live bottom (you type at the prompt), matching route_key.
    use_scroll_offset().set(0);
    let Scene::External(node) = scene else {
        return false;
    };
    let Some(intro) = node.handle.introspect_mut() else {
        return false;
    };
    intro
        .invoke("text", IntrospectValue::Text(text.clone()))
        .is_ok()
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

    /// End-to-end IME input: drive `apply_composition` with a committed Hangul
    /// string over a live `cat` pane and confirm the literal UTF-8 echoes back
    /// through the cooked-mode PTY (the apply_key test idiom). The focus gate,
    /// preedit (`Update`), and empty-commit cases are deterministic no-ops.
    #[test]
    fn apply_composition_routes_committed_ime_text_to_the_pane() {
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

        // apply_composition reads the scrollback-offset Signal, so run it inside
        // a root Owner scope (mirrors the shell).
        let owner = Owner::new();
        owner.run(|| {
            let commit = |t: &str| CompositionEvent::Commit(t.to_owned());
            // Focus gate: a wrong-tag commit injects nothing.
            assert!(!TerminalViewer::apply_composition(&mut scene, None, &commit("한")));
            // Preedit (Update) and an empty commit are not written to the PTY —
            // the IME renders the in-progress composition itself.
            let preedit = CompositionEvent::Update("ㅎ".to_owned());
            assert!(!TerminalViewer::apply_composition(&mut scene, Some(ROOT_TAG), &preedit));
            assert!(!TerminalViewer::apply_composition(&mut scene, Some(ROOT_TAG), &commit("")));
            // Focused commit: the literal Hangul is written and echoes back.
            assert!(TerminalViewer::apply_composition(&mut scene, Some(ROOT_TAG), &commit("한글")));
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
