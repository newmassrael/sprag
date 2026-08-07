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

use pinion_core::reactive::{Owner, Signal};

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

/// The reactive-cache key for [`painted_tabs`].
const PAINTED_TABS_KEY: &str = "sprag_gui.wtabs.painted";

/// **WHICH WINDOW each tab slot was PAINTED FROM**, by identity — the map a click resolves through.
///
/// # The defect this removes, which this file's own comment called benign
///
/// A tab's tag is its POSITION and has to be ([`create_window_externals`] registers a fixed set, so
/// a per-window tag would be discarded by the tag-keyed reconcile). Until R330 the click resolved
/// that position against the LIVE window list — `slots.windows().get(idx)` — and the comment beside
/// it said a list that changed since paint *"selects a neighbour or no-ops … benign and
/// self-healing"*.
///
/// **Selecting a neighbour is landing on a window the person did not click.** With tabs `a b c`
/// painted and `a` closing before the click, tab 1 resolves to `c`. That is the same defect this
/// project cites the rival for — herdr's `NavigatorState::selected` is a `usize` into rows rebuilt
/// on every render (`9a4ce5e1`) — sitting in sprag's own strip, and the claim of benignity was
/// never driven by a test.
///
/// [`crate::ctxmenu`] had already solved exactly this, one surface over: its rows are CAPTURED when
/// the menu opens *"so a window list that changes under an open popup can never make a click run a
/// different row than the one shown"*. N-1 of N doors kept the invariant; this is the odd one out.
///
/// [`None`] in a slot is a window the daemon publishes no identity for — the tab still paints and
/// its click no-ops, which is the same direction `WindowInfo::id`'s other readers take.
fn painted_tabs() -> Signal<Vec<Option<sprag_terminal::WindowId>>> {
    Owner::current()
        .expect("painted_tabs() requires an active Owner scope")
        .cache(PAINTED_TABS_KEY, || Signal::new(Vec::new()))
        .as_ref()
        .clone()
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
    // CAPTURE what each slot is painted from, so a click resolves to the window on the tab rather
    // than to whatever has moved into that position — see `painted_tabs`.
    painted_tabs().set(
        windows
            .iter()
            .take(MAX_WINDOW_TABS)
            .map(|window| window.id)
            .collect(),
    );
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
        // Addressed by IDENTITY (R330). The prompt this arms stands between the click and the act,
        // and a window that took the label in between is not the one the person agreed to kill; a
        // daemon that publishes no identity gets no kill from this button at all, which is the safe
        // direction for the least guarded surface in the client.
        if let Some(current) = slots
            .windows()
            .into_iter()
            .find(|window| window.current && window.id.is_some())
        {
            crate::confirm::run_or_arm(
                Command::KillWindow {
                    window: current.id.expect("filtered to the rows that have one"),
                    label: current.name,
                },
                None,
                slots,
            );
        }
        return true;
    }
    if let Some(idx) = tab_index(who) {
        // Resolve the clicked tab's slot to the IDENTITY it was painted from, then find that window
        // in the live list and select it by the name it carries NOW. A window that has gone selects
        // nothing, and a window that moved position or was renamed is still the one on the tab.
        //
        // The second hop is a name because `select-window` takes one, and the gap it leaves is a
        // microsecond inside one reducer call rather than the unbounded paint-to-click gap this
        // replaces. Closing it entirely needs the select ACTION to take an identity, which is a
        // grammar decision (`SelectWindowAsk` is shared with `BoundAction`, whose spelling has to
        // round-trip through a config file) and is registered rather than done here.
        let painted = painted_tabs().get().get(idx).copied().flatten();
        if let Some(row) = painted.and_then(|window| {
            slots
                .windows()
                .into_iter()
                .find(|row| row.id == Some(window))
        }) {
            slots.select_window(&row.name);
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
    use crate::terminal::{seed_terminal, use_terminal};
    use sprag_host::Host;
    use sprag_terminal::CommandBuilder;

    /// A long-lived `cat` pane, so a window keeps its pane for the length of a test — the shape
    /// every reducer fixture in this client seeds.
    fn cat() -> CommandBuilder {
        let mut c = CommandBuilder::new("/bin/sh");
        c.arg("-c");
        c.arg("cat");
        c.env("TERM", "dumb");
        c
    }

    /// A live in-process host behind the strip, holding one pane — so a test drives the real
    /// window list rather than a fake that cannot shift under it.
    fn seed_live_host() {
        let host = Host::new((40, 6));
        host.spawn(
            cat(),
            "cat".to_owned(),
            40,
            6,
            sprag_terminal::PaneBirthHooks::default(),
        )
        .unwrap();
        seed_terminal(host);
    }

    /// The name of the window the session is CURRENTLY on.
    fn current_name(slots: &SlotView) -> String {
        slots
            .windows()
            .into_iter()
            .find(|row| row.current)
            .expect("a session always has a current window")
            .name
    }

    /// The click intent a tab button delivers, as pinion scopes it.
    fn tab_click(i: usize) -> Intent {
        Intent {
            tag: std::borrow::Cow::Owned(format!("{}.{CLICK_EVENT}", tab_tag(i))),
            payload: pinion_core::external::IntrospectValue::Null,
        }
    }

    /// **A TAB CLICK SELECTS THE WINDOW ON THE TAB, NOT WHATEVER HAS MOVED INTO ITS POSITION.**
    ///
    /// # The claim this replaces
    ///
    /// The code beside the resolve said a list that changed since paint *"selects a neighbour or
    /// no-ops … Benign and self-healing"*, and nothing drove it. Selecting a neighbour is landing
    /// on a window the person did not click — the defect this project cites the rival for, in its
    /// own strip. Driven here over a LIVE host, so the window list changes the way it really does.
    ///
    /// # ⚠ The fixture is built so the two readings LAND ON DIFFERENT WINDOWS
    ///
    /// The first version of this test was VACUOUS and the mutation pass said so: killing the FIRST
    /// window left the current window already ON the expected answer, so the assertion held whether
    /// the click did anything at all. Both mutations came back GREEN.
    ///
    /// So the current window is moved AWAY from the answer before the click. With `0 b c` painted,
    /// `0` closing and `b` selected, tab 2 resolves by IDENTITY to `c` and by POSITION to nothing
    /// (the live list holds two) — one lands, the other leaves `b` current.
    ///
    /// REVERT-PROOF: resolve through `slots.windows().get(idx)` again, or drop the
    /// `painted_tabs().set(...)` from the painter, and the click leaves `b` current.
    #[test]
    fn a_tab_click_selects_the_window_the_tab_was_painted_from() {
        Owner::new().run(|| {
            seed_live_host();
            let slots = &use_terminal().slots;

            // Three windows: the boot one plus two. Each `new_window` selects what it made.
            let second = slots.new_window();
            let third = slots.new_window();
            let painted = slots.windows();
            assert_eq!(painted.len(), 3, "three tabs to paint");
            let boot = painted[0]
                .id
                .expect("the in-process host publishes an identity");

            // PAINT the strip — this is what captures the slot-to-identity map.
            let _ = view_window_strip(slots, &Theme::default());

            // The FIRST window closes out of band, so every tab after it shifts left by one...
            let _ = slots.kill_window(boot);
            assert_eq!(slots.windows().len(), 2, "the list really shifted");
            // ...and the current window is moved OFF the answer, or this test cannot fail.
            slots.select_window(&second);
            assert_eq!(
                current_name(slots),
                second,
                "the control: not on the answer yet"
            );

            assert!(
                handle_window_intent(&tab_click(2), slots),
                "the click is handled"
            );
            assert_eq!(
                current_name(slots),
                third,
                "the click landed on the window that was ON the tab; a positional resolve would \
                 have found nothing at index 2 and left the selection where it was",
            );
        });
    }

    /// A tab whose window has GONE selects nothing — the other half of the same map.
    ///
    /// ⚠ Built so a POSITIONAL resolve would land somewhere: with `0 b c` painted and `b` killed,
    /// index 1 of the live list is `c`. A fixture where the list merely shrank at the end would
    /// no-op under both readings and could not fail.
    ///
    /// REVERT-PROOF: resolve through `slots.windows().get(idx)` and this selects `c`.
    #[test]
    fn a_tab_whose_window_is_gone_selects_nothing() {
        Owner::new().run(|| {
            seed_live_host();
            let slots = &use_terminal().slots;

            let second = slots.new_window();
            let third = slots.new_window();
            let painted = slots.windows();
            let boot = painted[0].name.clone();
            let doomed = painted
                .iter()
                .find(|row| row.name == second)
                .and_then(|row| row.id)
                .expect("the second window has an identity");
            let _ = view_window_strip(slots, &Theme::default());

            let _ = slots.kill_window(doomed);
            slots.select_window(&boot);
            assert_eq!(
                current_name(slots),
                boot,
                "the control: not on the answer yet"
            );
            assert_eq!(
                slots.windows()[1].name,
                third,
                "a positional resolve of tab 1 WOULD find a window, or this cannot fail",
            );

            assert!(
                handle_window_intent(&tab_click(1), slots),
                "the click is handled"
            );
            assert_eq!(
                current_name(slots),
                boot,
                "a tab for a window that is gone moves nobody",
            );
        });
    }

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
            host.spawn(
                cat,
                "cat".to_owned(),
                40,
                6,
                sprag_terminal::PaneBirthHooks::default(),
            )
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
