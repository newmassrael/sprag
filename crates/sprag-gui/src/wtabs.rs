//! The window TAB STRIP (tmux "windows"): a horizontal strip above the pane area, one tab per
//! window of the session, plus a "+" (new window) and "×" (close current window) — click a tab to
//! select it (tmux `select-window`).
//!
//! The "×" ASKS before it closes ([`crate::confirm`]), because closing a window is irreversible and
//! closing the session's LAST window ends the session. It does not ask here, in a 30px band with
//! nowhere to put a prompt: it activates the catalog's `kill-window` command, which carries its own
//! question, and the client's one confirmation surface poses it.
//!
//! Modeled on [`ctxmenu`](crate::ctxmenu): sprag registers pinion
//! [`ButtonExternal`]s as EXTRA externals at FIXED tags (preserved across the dynamic-external
//! reconcile by tag, pinion R689), paints tagged clickable nodes, and the binding reducer routes
//! each button's "click" intent to a [`SlotView`] window action. The strip reads the window list
//! off the mirror ([`SlotView::windows`]) — no socket call on the paint path — and the host is the
//! single source of truth for which window is current (the highlighted tab).
//!
//! ## Why per-tab BUTTONS, not one `RadioGroupExternal` + composite `#` tags
//!
//! A [`ButtonExternal`] fires "click" on EVERY press, so a tab click always reaches the host. A
//! `RadioGroupExternal` fires only on a change of its OWN selection state — which would SILENCE a
//! click on the tab it (stalely) believes is already selected. The current window can move OUT OF
//! BAND of any one client's clicks (a "+" auto-selects the new window; another attached client
//! switches), so a fire-only-on-change selector would drop exactly the "switch back to the tab
//! I was on" click. Per-tab buttons keep the host authoritative and every click live.

use pinion_core::scene::{ContainerNode, Rect, TextNode};
use pinion_core::style::{
    AlignItems, BoxStyle, FlexDirection, JustifyContent, LayoutStyle, Size, SizeValue, TextStyle,
};
use pinion_core::theme::{ColorRole, Theme};
use pinion_core::widget_core::ExtraExternal;
use pinion_core::widgets::button::ButtonExternal;
use pinion_core::{Color, Intent, Scene};

use crate::command::Command;
use crate::slotview::SlotView;

/// The strip container tag (the Row of tabs + action buttons).
const WINDOW_STRIP_TAG: &str = "sprag_gui.wstrip";
/// The "+" (new window) button tag.
const NEW_WINDOW_TAG: &str = "sprag_gui.wnew";
/// The "×" (close CURRENT window) button tag.
const CLOSE_WINDOW_TAG: &str = "sprag_gui.wclose";
/// The per-tab tag prefix; tab `i` is tagged `{TAB_TAG_PREFIX}{i}`.
const TAB_TAG_PREFIX: &str = "sprag_gui.wtab.";
/// The event a [`ButtonExternal`] emits on activation — pinion scopes it as `{tag}.click`.
const CLICK_EVENT: &str = "click";

/// The fixed cap on tabs the strip can route. The per-tab [`ButtonExternal`]s are registered ONCE
/// at fixed tags `{TAB_TAG_PREFIX}0..CAP` — a count that changed per window would have its rebuilt
/// externals discarded by the tag-keyed dynamic-external reconcile (pinion R689) — and the strip
/// paints only the LIVE windows. A window past the cap gets no tab (an honest bound, like
/// [`MAX_PANES`](crate::terminal)); the `sprag` CLI still reaches it.
pub(crate) const MAX_WINDOW_TABS: usize = 16;

/// The tab strip height in logical pixels — the fixed band it takes above the pane area (the
/// panes reflow to the window height minus this, like the chrome + dock-header strips).
pub(crate) const STRIP_HEIGHT: u32 = 30;

/// The tab-button tag for tab `i`.
fn tab_tag(i: usize) -> String {
    format!("{TAB_TAG_PREFIX}{i}")
}

/// The window-strip EXTRA externals: one [`ButtonExternal`] per possible tab plus the "+" and "×"
/// action buttons, all at FIXED tags (preserved across the dynamic-external reconcile by tag, like
/// the context menu and dock panels). See the module docs for why they are per-tab buttons.
pub(crate) fn create_window_externals() -> Vec<ExtraExternal> {
    let mut externals = Vec::with_capacity(MAX_WINDOW_TABS + 2);
    for i in 0..MAX_WINDOW_TABS {
        externals.push(ExtraExternal::new(
            tab_tag(i),
            Box::new(ButtonExternal::new()),
        ));
    }
    externals.push(ExtraExternal::new(
        NEW_WINDOW_TAG.to_owned(),
        Box::new(ButtonExternal::new()),
    ));
    externals.push(ExtraExternal::new(
        CLOSE_WINDOW_TAG.to_owned(),
        Box::new(ButtonExternal::new()),
    ));
    externals
}

/// The tab strip: a Row of one tab per live window (the current one highlighted) followed by the
/// "+" new-window and "×" close-current buttons. Reads the window list off the [`SlotView`] mirror
/// (no socket call — the paint path). Mounted ONLY on the main window (via
/// [`view::compose`](crate::view)).
pub(crate) fn view_window_strip(slots: &SlotView, theme: &Theme) -> Scene {
    let windows = slots.windows();
    let mut children: Vec<Scene> = Vec::with_capacity(windows.len() + 2);
    for (i, window) in windows.iter().enumerate().take(MAX_WINDOW_TABS) {
        children.push(tab_node(i, &window.name, window.current, theme));
    }
    children.push(action_button(NEW_WINDOW_TAG, "+", theme));
    children.push(action_button(CLOSE_WINDOW_TAG, "×", theme));
    Scene::Container(
        ContainerNode::new(children)
            .with_tag(WINDOW_STRIP_TAG)
            .with_style(BoxStyle::filled(theme.resolve(ColorRole::Surface)))
            .with_layout(
                LayoutStyle::new()
                    .flex(FlexDirection::Row)
                    .with_align_items(AlignItems::Stretch)
                    .with_justify(JustifyContent::Start)
                    .with_size(Size::auto().with_height(SizeValue::Px(STRIP_HEIGHT))),
            ),
    )
}

/// One clickable tab: the window's name, highlighted when it is current. Tagged so a click routes
/// to tab `i`'s [`ButtonExternal`].
fn tab_node(i: usize, name: &str, current: bool, theme: &Theme) -> Scene {
    let (fill, fg) = if current {
        (
            theme.resolve(ColorRole::SurfaceContainerHighest),
            theme.resolve(ColorRole::Accent),
        )
    } else {
        (Color::TRANSPARENT, theme.resolve(ColorRole::OnSurfaceMuted))
    };
    clickable(tab_tag(i), name, fill, fg)
}

/// One clickable action button (the "+" / "×"), tagged so its click routes to its [`ButtonExternal`].
fn action_button(tag: &str, glyph: &str, theme: &Theme) -> Scene {
    clickable(
        tag.to_owned(),
        glyph,
        Color::TRANSPARENT,
        theme.resolve(ColorRole::OnSurface),
    )
}

/// A tagged, clickable cell: a centered `label` over `fill`, hit-tested by `tag` (the pinion input
/// router drives the [`ButtonExternal`] registered at that tag on a press — mouse hit-testing is by
/// tag + rect, independent of keyboard focus).
///
/// NOT `with_focusable`: the strip is mouse-first for v1 (like the context menu, which also defers
/// keyboard nav), so a click still routes but the tabs do not enter the pane Tab-order. Keyboard /
/// a11y for the tabs is a tracked follow-up.
fn clickable(tag: String, label: &str, fill: Color, fg: Color) -> Scene {
    let text = Scene::Text(TextNode::styled(
        label.to_owned(),
        Rect::default(),
        TextStyle::new().with_size_px(13).with_fg(fg),
    ));
    Scene::Container(
        ContainerNode::new(vec![text])
            .with_tag(tag)
            .with_style(BoxStyle::filled(fill))
            .with_layout(
                LayoutStyle::new()
                    .flex(FlexDirection::Row)
                    .with_align_items(AlignItems::Center)
                    .with_justify(JustifyContent::Center)
                    .with_padding(Rect::new(12, 0, 12, 0)),
            ),
    )
}

/// Route a drained intent: if it is one of the window strip's button "click"s (a tab, the "+", or
/// the "×"), run the corresponding window action against `slots` and report handled. Any other
/// intent is left for the caller's own reducer arms.
pub(crate) fn handle_window_intent(intent: &Intent, slots: &SlotView) -> bool {
    let Some((who, event)) = intent.tag_str().rsplit_once('.') else {
        return false;
    };
    if event != CLICK_EVENT {
        return false;
    }
    if who == NEW_WINDOW_TAG {
        slots.new_window();
        return true;
    }
    if who == CLOSE_WINDOW_TAG {
        // Close the CURRENT window (whichever tab is active) — but ASK first. This is the ONE action
        // of the strip routed through the command catalog, and the asymmetry is deliberate rather than
        // an oversight: the question belongs to the command
        // ([`Command::confirmation`](crate::command::Command::confirmation)), so going through
        // [`confirm::run_or_arm`](crate::confirm::run_or_arm) is the only way this "×" inherits the
        // same prompt the palette's `Kill window` row gets. The "+" and the tabs have no question to
        // inherit; folding them onto the catalog too would be a refactor with no change in behaviour,
        // deliberately not done here.
        //
        // Until this routing landed, this button killed a window on ONE unguarded click — and the
        // session's last window ends it, so the least guarded surface in the client was the one able
        // to destroy the most.
        if let Some(current) = slots.windows().into_iter().find(|window| window.current) {
            crate::confirm::run_or_arm(Command::KillWindow(current.name), None, slots);
        }
        return true;
    }
    if let Some(idx) = tab_index(who) {
        // Resolve the clicked tab's index into the CURRENT window list and select that window by
        // NAME. The index is still positional (from paint time); re-reading the live list at click
        // time just means a list that changed since paint selects a neighbour or no-ops
        // (`.get(idx)` → `None`) rather than acting on a stale name — never a panic, never a dead
        // name. Benign and self-healing.
        if let Some(window) = slots.windows().get(idx) {
            slots.select_window(&window.name);
        }
        return true;
    }
    false
}

/// The tab index a `{TAB_TAG_PREFIX}{i}` button tag names, or `None` for a non-tab tag.
fn tab_index(who: &str) -> Option<usize> {
    who.strip_prefix(TAB_TAG_PREFIX)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_tags_round_trip_through_the_index_parser() {
        for i in [0, 3, MAX_WINDOW_TABS - 1] {
            // The scoped intent tag a tab-button click arrives as: `{tab_tag}.click`.
            let scoped = format!("{}.{CLICK_EVENT}", tab_tag(i));
            let (who, event) = scoped.rsplit_once('.').expect("a scoped tag");
            assert_eq!(event, CLICK_EVENT);
            assert_eq!(tab_index(who), Some(i), "the tab index round-trips");
        }
    }

    #[test]
    fn the_action_button_tags_are_not_mistaken_for_tabs() {
        // The "+" / "×" tags must not parse as a tab index, or a click on them would select a
        // window instead of creating / closing one.
        assert_eq!(tab_index(NEW_WINDOW_TAG), None);
        assert_eq!(tab_index(CLOSE_WINDOW_TAG), None);
        // ...and a tab tag IS a tab.
        assert_eq!(tab_index(&tab_tag(2)), Some(2));
    }

    #[test]
    fn one_button_external_is_registered_per_tab_plus_the_two_actions() {
        // The strip routes at most MAX_WINDOW_TABS tabs plus "+" and "×", each its own external.
        assert_eq!(create_window_externals().len(), MAX_WINDOW_TABS + 2);
    }

    /// The "×" ASKS instead of killing. This is the defect the confirmation front closed: one click on
    /// this button used to end a window — and, on the session's last one, the session.
    ///
    /// REVERT-PROOF: put `slots.kill_window(&current.name)` back in place of the `run_or_arm` call and
    /// this fails with the window already gone and no prompt up.
    #[test]
    fn the_close_button_asks_before_it_closes_the_current_window() {
        use pinion_core::reactive::Owner;
        use sprag_host::Host;
        use sprag_terminal::CommandBuilder;

        Owner::new().run(|| {
            let mut cat = CommandBuilder::new("/bin/sh");
            cat.arg("-c");
            cat.arg("cat");
            cat.env("TERM", "dumb");
            let host = Host::new((40, 6));
            host.spawn(cat, "cat".to_owned(), 40, 6, None, None)
                .unwrap();
            crate::terminal::seed_terminal(host);

            let terminal = crate::terminal::use_terminal();
            terminal.slots.new_window();
            let before = terminal.slots.windows().len();
            assert!(before > 1, "two windows, so a kill would be observable");

            let click = Intent {
                tag: std::borrow::Cow::Owned(format!("{CLOSE_WINDOW_TAG}.{CLICK_EVENT}")),
                payload: pinion_core::external::IntrospectValue::Null,
            };
            assert!(handle_window_intent(&click, &terminal.slots));

            assert_eq!(
                crate::terminal::use_terminal().slots.windows().len(),
                before,
                "the click destroyed NOTHING on its own"
            );
            assert!(
                crate::confirm::is_open(),
                "it armed the confirmation instead"
            );
        });
    }
}
