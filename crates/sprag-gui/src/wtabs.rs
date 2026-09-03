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

use pinion_a11y::{AccessNode, AriaRole};
use pinion_core::scene::{ContainerNode, Rect, TextNode};
use pinion_core::style::{
    AlignItems, BoxStyle, FlexDirection, JustifyContent, LayoutStyle, Size, SizeValue, TextStyle,
};
use pinion_core::theme::{ColorRole, Theme};
use pinion_core::widget_core::ExtraExternal;
use pinion_core::widgets::button::ButtonExternal;
use pinion_core::{Color, Intent, Scene};

use pinion_core::reactive::{Owner, Signal};
use sprag_host::report::Report;

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
/// The strip's WAI-ARIA `tablist` container — see [`window_strip_access_nodes`].
///
/// ⚠ Named like the session rail's (`sprag_gui.stablist`) because it is the same kind of thing on
/// the other axis, and an AT meeting both should not have to learn two conventions.
const TABLIST_TAG: &str = "sprag_gui.wtablist";
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

/// **THE WINDOW STRIP AS A WAI-ARIA TABLIST** — register item 582, and the thing a person could not
/// be told.
///
/// # ⚠⚠⚠⚠⚠ Why a strip that PAINTS the answer still has to publish it
///
/// [`tab_node`] marks the current window with two colours and nothing else. That is enough for eyes
/// and it is the whole answer for nobody else: a screen reader can say which SESSION this client is
/// on ([`crate::stabs::session_sidebar_access_nodes`], a tablist since R179) and could not say which
/// WINDOW. Two strips, one axis each, and only one of them speaks.
///
/// ⚠⚠ It is also what made the owner's symptom ungateable. The report was *the chrome says pinion
/// and the pixels are sce*; comparing those needs the chrome's claim as a VALUE, and a colour in a
/// frame buffer is not one. So this is published for a person first and a gate second — the order
/// matters, because an address minted so a test can see something is the shape register item 642
/// measured this workspace refusing.
///
/// ⚠ Built like the session rail deliberately: a `TabList` container naming its children, then one
/// `Tab` per window carrying `selected` and its position in the set. A second spelling of *what a
/// tab strip is* would drift from the one an AT already meets on the other axis.
pub(crate) fn window_strip_access_nodes(slots: &crate::slotview::SlotView) -> Vec<AccessNode> {
    let windows = slots.windows();
    let count = windows.len().min(MAX_WINDOW_TABS);
    let mut nodes: Vec<AccessNode> = Vec::with_capacity(count + 3);
    let mut tablist = AccessNode::new(TABLIST_TAG, AriaRole::TabList).with_name("Windows");
    for i in 0..count {
        tablist = tablist.with_child(tab_tag(i));
    }
    nodes.push(tablist);
    for (i, window) in windows.iter().enumerate().take(MAX_WINDOW_TABS) {
        nodes.push(
            AccessNode::new(tab_tag(i), AriaRole::Tab)
                .with_name(window.name.clone())
                // ⚠⚠⚠ READ FROM THE WINDOW, never from an index. `SlotView`'s mirror is the single
                // source of truth for which window is current (this module's own doc says so), and
                // the strip's FIRST TAB IS NOT THE CURRENT ONE — the pixel smoke paid a round for
                // that assumption before this existed.
                .with_selected(window.current)
                .with_set_position(i, count),
        );
    }
    nodes.push(AccessNode::new(NEW_WINDOW_TAG, AriaRole::Button).with_name("New window"));
    nodes.push(AccessNode::new(CLOSE_WINDOW_TAG, AriaRole::Button).with_name("Close window"));
    nodes
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
/// the "×"), run the corresponding window action against `slots` and answer WHAT IT DID. [`None`]
/// for any other intent, which is left for the caller's own reducer arms.
///
/// # It answered `bool` until R852, and that is the defect this signature removes
///
/// `true` meant *this strip routed the intent*, and the caller — the only reducer that could say
/// anything to anybody — read it as *this strip did something*. Those are different claims and the
/// tab arm was making the wrong one: a click it could not address, and a click whose select landed
/// nowhere, both returned `true` and left no trace on the screen or in the log. The owner pressed a
/// tab **dozens of times** against exactly that.
///
/// [`Report`] is the vocabulary this client already answers a key, a palette row and a confirmed
/// command in, so every arm here now states its outcome in the words a person reads — and the
/// silent outcome has a NAME ([`Report::on_screen`]) that a reader can check against what the strip
/// paints, rather than a bare `return true` nobody can disagree with.
pub(crate) fn handle_window_intent(intent: &Intent, slots: &SlotView) -> Option<Report> {
    let (who, event) = intent.tag_str().rsplit_once('.')?;
    if event != CLICK_EVENT {
        return None;
    }
    if who == NEW_WINDOW_TAG {
        // Creates AND selects, so the strip repainting one tab wider IS the answer — the same claim
        // the palette's `New window` row makes.
        let _born = slots.new_window();
        return Some(Report::on_screen());
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
        let Some(current) = slots
            .windows()
            .into_iter()
            .find(|window| window.current && window.id.is_some())
        else {
            // ⚠ THE SAME SILENCE THE TAB ARM HAD, on the same strip: a daemon that publishes no
            // identity leaves this button with nothing to arm, and until R852 that was a `true`
            // claiming the click had been dealt with. It is `no_window` and not `on_screen` because
            // nothing appeared — the prompt this button exists to raise did not come up, and a
            // person who pressed "×" and saw no question is owed the reason.
            return Some(Report::no_window());
        };
        crate::confirm::run_or_arm(
            Command::KillWindow {
                window: current.id.expect("filtered to the rows that have one"),
                label: current.name,
            },
            None,
            slots,
        );
        // The prompt (or the command `run_or_arm` performed and reported through
        // `crate::message::show` itself) is on screen; this arm has nothing to add over it.
        return Some(Report::on_screen());
    }
    if let Some(idx) = tab_index(who) {
        // Resolve the clicked tab's slot to the IDENTITY it was painted from and send THAT — a
        // window that has gone selects nothing, and one that moved position or was renamed is still
        // the one on the tab. See [`select_painted_tab`], which owns the address and the answer.
        let outcome = select_painted_tab(idx, slots);
        // The LOG half of R852's "say so". The intent's arrival was already logged by the shell —
        // the owner's report was 29 `sprag_gui.wtab.N.click` lines with NOTHING after them — so what
        // was missing is a line per click saying what became of it. One event, whose word is read
        // off the SAME value the sentence is, so the log and the strip cannot come to disagree.
        crate::diag::tab_click(idx, outcome.verdict());
        return Some(outcome.report());
    }
    None
}

/// WHAT A TAB CLICK DID — the one value both the person's sentence and the log line are read off.
///
/// # Why the two silent arms are TWO and not one
///
/// They have DIFFERENT PRESCRIPTIONS, which is the whole of R852's third condition. [`Unaddressed`]
/// means the request never left this client, so the thing to look at is what the daemon publishes in
/// its `windows` slot (or whether the strip was painted at all). [`Gone`] means the request left,
/// arrived, and was refused, so the thing to look at is the window list — a race a person can lose
/// honestly by clicking a tab after its window closed. A single "the click did nothing" would send
/// a reader into the wrong process, which is exactly the state this item was opened in: the ledger
/// could not say which of the two the owner had hit, because neither left a mark.
///
/// [`Unaddressed`]: TabOutcome::Unaddressed
/// [`Gone`]: TabOutcome::Gone
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TabOutcome {
    /// The tab resolved to an identity, the select was sent, and the host answered the window it
    /// landed on.
    Landed,
    /// The slot this tab was painted from carries NO identity, so no select was sent at all — a
    /// daemon that publishes no [`WindowInfo::id`](sprag_terminal::WindowInfo::id), or a strip whose
    /// painter never ran. Offering no act is the right behaviour ([`painted_tabs`] states why);
    /// offering it in silence was the defect.
    Unaddressed,
    /// The identity was sent and the host answered [`None`] — the window this tab was painted from
    /// is no longer there. The answer has been available since R316 and this arm is what reads it.
    Gone,
}

impl TabOutcome {
    /// The sentence this outcome puts on the message strip.
    ///
    /// An exhaustive match with no catch-all, deliberately: a fourth outcome must be given words
    /// here rather than inheriting a default, which is how a new silence would get in.
    fn report(self) -> Report {
        match self {
            // The window changed under the person's eyes and the strip repainted, which is the
            // answer. `on_screen` is a CLAIM a reader can check, not the old bare `return true`.
            Self::Landed => Report::on_screen(),
            Self::Unaddressed => Report::no_window(),
            Self::Gone => Report::window_gone(),
        }
    }

    /// The word the log line carries — a `&'static str` per arm, like
    /// [`diag::redock_resolution`](crate::diag::redock_resolution)'s verdict.
    ///
    /// Read off the same value [`report`](Self::report) is, so the two halves of *"say so — to a
    /// person or to the log"* cannot drift apart, and exhaustive for that method's reason.
    const fn verdict(self) -> &'static str {
        match self {
            Self::Landed => "landed",
            Self::Unaddressed => "unaddressed",
            Self::Gone => "gone",
        }
    }
}

/// Resolve tab `idx` to the IDENTITY it was painted from, send THAT, and answer what became of it.
///
/// There is no second hop and no name: the select ACTION takes a reference, so the address a person
/// pointed at is the address that crosses the wire. The first version of this fix resolved the
/// identity back to a live NAME because the ask was shared with the keybinding vocabulary — a gap of
/// one reducer call, which was still a gap.
///
/// Split out of the reducer arm so the CLASSIFICATION is a value a test can name, rather than a
/// branch whose only trace was a `true` both halves of the failure also returned.
fn select_painted_tab(idx: usize, slots: &SlotView) -> TabOutcome {
    let Some(window) = painted_tabs().get().get(idx).copied().flatten() else {
        return TabOutcome::Unaddressed;
    };
    // ⚠ THE ANSWER IS THE POINT. `SlotView::select_window` carries a `#[must_use]` of its own now,
    // so this is also the shape `-D warnings` keeps: the previous line here dropped the landing and
    // the lint on the TRAIT method could not see through the wrapper.
    match slots.select_window(&sprag_host::wire::WindowRef::Picked(window)) {
        Some(_) => TabOutcome::Landed,
        None => TabOutcome::Gone,
    }
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
            // The landing is discarded ON PURPOSE here and at the two other setup selects in this
            // module: the assertion below re-reads the current window, which is the fact this
            // arrangement is about. Written `let _ =` rather than left bare so the wrapper's
            // `#[must_use]` still catches the drop that mattered.
            let _ = slots.select_window(&sprag_host::wire::WindowRef::Named(second.clone()));
            assert_eq!(
                current_name(slots),
                second,
                "the control: not on the answer yet"
            );

            let said = handle_window_intent(&tab_click(2), slots).expect("the click is handled");
            assert_eq!(
                said.says(),
                None,
                "a tab click that LANDED is the control for R852's sentences: the window changed \
                 under the person's eyes, so there is nothing to say — and a strip that warned here \
                 too would satisfy the two failure gates by warning always: {said:?}",
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
            let _ = slots.select_window(&sprag_host::wire::WindowRef::Named(boot.clone()));
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

            let said = handle_window_intent(&tab_click(1), slots).expect("the click is handled");
            assert_eq!(
                current_name(slots),
                boot,
                "a tab for a window that is gone moves nobody",
            );
            // R852's second half: moving nobody is right, and doing it in SILENCE was the defect.
            assert_eq!(
                said.says(),
                Some("that window is gone"),
                "⚠⚠⚠⚠⚠ the select was SENT and the host answered none — the answer \
                 `HostClient::select_window` has carried since R316 — and the caller that painted \
                 the row dropped it. REVERT-PROOF: put back \
                 `slots.select_window(&…Picked(window));` with its answer discarded and this says \
                 nothing: {said:?}",
            );
        });
    }

    /// ⚠⚠⚠⚠⚠ **R852's OTHER half: a tab that carries NO ADDRESS says so too.**
    ///
    /// The owner pressed three tabs 29 times between them, the log recorded every
    /// `sprag_gui.wtab.N.click` intent arriving, and after them there was nothing — no window
    /// change, no error, no line. Two branches of ONE arm could produce that, and both answered
    /// `true`: this one, where the painted slot holds no [`sprag_terminal::WindowId`] so no select
    /// is ever sent, and `a_tab_whose_window_is_gone_says_so`, where one is sent and refused.
    ///
    /// # ⚠ The map is written through the PAINTER'S OWN door, and it has to be
    ///
    /// The in-process `Host` fills `id` on EVERY row — `Session::window_infos_marking` does it
    /// unconditionally — so a daemon that publishes none is unreachable over it, which is also why
    /// every live test in this file could pass while this branch was silent. The address-less slot
    /// is therefore written with `painted_tabs().set`, the one call `view_window_strip` itself makes
    /// (a `Vec<Option<WindowId>>`, absent exactly as the wire's absent `id` key arrives), rather
    /// than by a second thirty-five-method `HostClient` fake beside `stabs`'s.
    ///
    /// The host under it is LIVE, so the claim is not only about words: a branch that sent a select
    /// anyway would move the session, and the second assertion is what says it did not.
    ///
    /// REVERT-PROOF: answer `Report::on_screen()` for the unaddressed branch — the old `return true`
    /// in the words this signature made available — and this fails with nothing said.
    #[test]
    fn a_tab_that_addresses_no_window_says_so() {
        Owner::new().run(|| {
            seed_live_host();
            let slots = &use_terminal().slots;
            let second = slots.new_window();
            let _ = view_window_strip(slots, &Theme::default());
            assert!(
                painted_tabs().get().iter().all(Option::is_some),
                "the control: over a live host every painted tab HAS an address, which is why the \
                 next line is the only way to reach the branch this gate is about",
            );

            // A daemon that publishes no identity for the window on tab 1.
            let mut painted = painted_tabs().get();
            painted[1] = None;
            painted_tabs().set(painted);
            let before = current_name(slots);
            assert_eq!(before, second, "the control: on the window tab 1 was painted from");

            let said = handle_window_intent(&tab_click(1), slots).expect("the click is handled");
            assert_eq!(
                said.says(),
                Some("no window to select here"),
                "⚠⚠⚠⚠⚠ a tab click that could not be addressed must SAY so: offering no act is \
                 right — `WindowInfo::id` is an Option because \"a client that needs an identity \
                 offers no act rather than a wrong one\" — and offering it in SILENCE is what the \
                 owner met as a tab that would not switch: {said:?}",
            );
            assert_eq!(
                current_name(slots),
                before,
                "and it really sent nothing: an address-less tab must not fall back to a name or a \
                 position, which is the stranger-selecting defect this strip's map exists to remove",
            );
        });
    }

    /// ⚠⚠⚠⚠⚠ **THE SPLIT — R852's third condition, and the one neither gate above can state.**
    ///
    /// The two silences have DIFFERENT PRESCRIPTIONS: `unaddressed` points at what the daemon
    /// publishes in its `windows` slot, `gone` at a window that closed under a painted strip. The
    /// ledger could not say which of the two the owner hit, and a fix that made both clicks warn
    /// with ONE sentence would leave it exactly as unable to.
    ///
    /// So this fires the strip's two failures and requires the outcome to be TOLD APART, on both
    /// halves of *"say so — to a person or to the log"*: the sentences differ, the log verdicts
    /// differ, and neither is the silent [`Report::on_screen`] the landing case correctly is.
    ///
    /// REVERT-PROOF: give `TabOutcome::Unaddressed` the same `Report` as `Gone` (or the same
    /// verdict word) and this fails — while both single-face gates above stay green.
    #[test]
    fn the_two_silent_tab_clicks_are_told_apart() {
        let (unaddressed, gone, landed) = (
            TabOutcome::Unaddressed,
            TabOutcome::Gone,
            TabOutcome::Landed,
        );
        // The PERSON's half.
        let (a, b) = (unaddressed.report(), gone.report());
        assert!(
            a.says().is_some() && b.says().is_some(),
            "both silent outcomes must speak: {a:?} / {b:?}",
        );
        assert_ne!(
            a.says(),
            b.says(),
            "⚠⚠⚠⚠⚠ a click that never left the client and a click the host refused need DIFFERENT \
             sentences — one sends a reader to the daemon's `windows` slot, the other to a window \
             that closed, and one wording for both is the ledger's own \"⑴ 인지 ⑵ 인지는 아직 안 \
             갈렸다\" reproduced in the product",
        );
        assert_eq!(
            landed.report().says(),
            None,
            "the control: the LANDING says nothing, so this gate cannot be passed by warning always",
        );
        // The LOG's half, read off the same value.
        let words = [
            unaddressed.verdict(),
            gone.verdict(),
            landed.verdict(),
            unaddressed.verdict(),
        ];
        assert_eq!(
            words
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            3,
            "three outcomes, three verdict words, and the same outcome twice is the same word: \
             {words:?}",
        );
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
            let said = handle_window_intent(&click, &terminal.slots).expect("the click is handled");
            assert_eq!(
                said.says(),
                None,
                "the prompt IS the answer, so this arm has nothing to add over it: {said:?}",
            );

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

    /// **WHICH WINDOW A CLIENT IS ON IS A FACT IT PUBLISHES, NOT A COLOUR** — register item 582.
    ///
    /// # ⚠⚠⚠⚠⚠ The owner's symptom, and why nothing could gate it
    ///
    /// The report was *the chrome says pinion and the pixels are sce* — a header naming one window
    /// while the panes of another are painted. Four gates now drive that gesture and every one of
    /// them is green, because each asks about ONE side: the window LIST moves (it always did), or
    /// the PANES move (they do now). **Nothing asks whether the two agree**, and the only way to ask
    /// is to read what the chrome claims.
    ///
    /// That could not be read. [`tab_node`] renders currentness as `SurfaceContainerHighest` +
    /// `Accent` and nothing else — so *which window is current* existed, in this client, **only as
    /// two colours in a frame buffer**.
    ///
    /// # ⚠⚠ And that is a defect of its own, which is why this is fixed BEFORE the comparison gate
    ///
    /// The session rail publishes exactly this (`AriaRole::Tab` + `with_selected`), so a screen
    /// reader can say which SESSION it is on and cannot say which WINDOW. Publishing it is therefore
    /// worth doing for a person — and doing it in that order is what keeps this from being an
    /// address minted so a test could see something, which is the shape register item 642 measured
    /// this workspace refusing.
    #[test]
    fn the_window_strip_publishes_which_window_is_current() {
        Owner::new().run(|| {
            seed_live_host();
            let slots = &use_terminal().slots;
            slots.new_window();
            let windows = slots.windows();
            assert!(
                windows.len() > 1,
                "two windows, so 'which one' is a real question: {windows:?}",
            );

            let nodes = window_strip_access_nodes(slots);
            let tabs: Vec<_> = nodes
                .iter()
                .filter(|node| node.role == AriaRole::Tab)
                .collect();
            assert_eq!(
                tabs.len(),
                windows.len(),
                "one tab node per live window: {nodes:?}",
            );

            // ⚠ THE CLAIM: exactly one tab says it is the current one, and it is the one the window
            // list says is current. A strip that marked none would leave a screen reader where it
            // was; one that marked several would be the divergence this item is about, published.
            let current = windows
                .iter()
                .position(|w| w.current)
                .expect("the window list names a current window");
            let said: Vec<usize> = tabs
                .iter()
                .enumerate()
                .filter(|(_, node)| node.selected == Some(true))
                .map(|(i, _)| i)
                .collect();
            assert_eq!(
                said,
                vec![current],
                "⚠⚠⚠⚠⚠ the strip must say WHICH window is current, and say it about the same one \
                 the window list does — the owner's symptom was a chrome naming one window while \
                 another's panes were painted, and a fact kept only as a colour cannot be compared \
                 with anything: {nodes:?}",
            );
        });
    }

    /// ⚠⚠⚠⚠⚠ **THE CONTROL: THE ANSWER MUST FOLLOW THE WINDOW, NOT AN INDEX.** The gate above
    /// passes for a strip that hard-codes `selected` on tab 0 — which is exactly the stale-first-tab
    /// assumption the pixel smoke already paid a round for. This selects a different window and
    /// requires the answer to move with it.
    #[test]
    fn the_published_current_window_moves_when_the_window_does() {
        Owner::new().run(|| {
            seed_live_host();
            let slots = &use_terminal().slots;
            slots.new_window();

            let first = window_strip_access_nodes(slots)
                .iter()
                .position(|n| n.selected == Some(true))
                .expect("some tab is current to begin with");

            let names: Vec<String> = slots.windows().iter().map(|w| w.name.clone()).collect();
            let other = names
                .iter()
                .enumerate()
                .find(|(i, _)| *i != first)
                .map(|(_, name)| name.clone())
                .expect("a second window to move to");
            let _ = slots.select_window(&sprag_host::wire::WindowRef::Named(other));

            let moved = window_strip_access_nodes(slots)
                .iter()
                .position(|n| n.selected == Some(true))
                .expect("some tab is current after the switch");
            assert_ne!(
                moved, first,
                "⚠⚠⚠ selecting another window must move what the strip SAYS is current — an answer \
                 pinned to an index would be green here and wrong in exactly the way the pixel \
                 smoke's first cut was (its 'first tab' was not the window the client was on)",
            );
        });
    }
}
