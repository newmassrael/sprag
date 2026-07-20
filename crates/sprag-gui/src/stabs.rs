//! The session SIDEBAR (cmux "workspaces" / tmux sessions): a fixed-width VERTICAL rail down the
//! left of the window, one row per session of the daemon, the attached one highlighted — click a
//! row's BODY to SWITCH this client to that session IN PLACE (tmux `switch-client`), or its "×" to
//! KILL that session (tmux `kill-session`; killing the ATTACHED session detaches this client), plus
//! a "+" at the bottom (tmux `new-session`).
//!
//! The orthogonal axis to [`wtabs`](crate::wtabs): that draws the current SESSION's windows across
//! the top; this draws every SESSION down the side. Together they mirror tmux's sessions ⊃ windows
//! hierarchy (and cmux's workspace sidebar + tab strip).
//!
//! Built exactly like [`wtabs`](crate::wtabs): sprag registers pinion [`ButtonExternal`]s as EXTRA
//! externals at FIXED tags (preserved across the dynamic-external reconcile by tag, pinion R689),
//! paints tagged clickable nodes, and the binding reducer routes each button's "click" intent to a
//! [`SlotView`] session action. The rail reads the session list off the mirror
//! ([`SlotView::sessions`]) with no socket call on the paint path, and the host is the single
//! source of truth for which sessions exist; WHICH one this client is on is a client-local fact
//! ([`SlotView::current_session`]) — the highlighted row.
//!
//! Per-tab BUTTONS (not one `RadioGroupExternal`) for the SAME reason [`wtabs`](crate::wtabs)
//! documents: a [`ButtonExternal`] fires on EVERY press, so a click always reaches the host, while
//! a fire-only-on-change selector would silence a re-click on the already-selected row — and the
//! attached session can move out of band (another client, the `sprag` CLI creating one).

use pinion_core::scene::{ContainerNode, Rect, TextNode};
use pinion_core::style::{
    AlignItems, BoxStyle, FlexDirection, JustifyContent, LayoutStyle, Size, SizeValue, TextStyle,
};
use pinion_core::theme::{ColorRole, Theme};
use pinion_core::widget_core::ExtraExternal;
use pinion_core::widgets::button::ButtonExternal;
use pinion_core::{Color, Intent, Scene};

use crate::slotview::SlotView;

/// The sidebar container tag (the Column of session rows + the "+" action).
const SESSION_RAIL_TAG: &str = "sprag_gui.srail";
/// The "+" (new session) button tag.
const NEW_SESSION_TAG: &str = "sprag_gui.snew";
/// The per-row SWITCH tag prefix; row `i`'s body (a click switches this client to it) is tagged
/// `{ROW_TAG_PREFIX}{i}`.
const ROW_TAG_PREFIX: &str = "sprag_gui.stab.";
/// The per-row KILL tag prefix; row `i`'s "×" (a click kills that session) is tagged
/// `{KILL_TAG_PREFIX}{i}`. Distinct from [`ROW_TAG_PREFIX`] (`stab` vs `skill`) so the reducer
/// routes a body click to a SWITCH and an "×" click to a KILL, never confusing the two.
const KILL_TAG_PREFIX: &str = "sprag_gui.skill.";
/// The event a [`ButtonExternal`] emits on activation — pinion scopes it as `{tag}.click`.
const CLICK_EVENT: &str = "click";

/// The fixed cap on session rows the rail can route. The per-row [`ButtonExternal`]s are registered
/// ONCE at fixed tags `{ROW_TAG_PREFIX}0..CAP` — a count that changed per session would have its
/// rebuilt externals discarded by the tag-keyed dynamic-external reconcile (pinion R689) — and the
/// rail paints only the LIVE sessions. A session past the cap gets no row (an honest bound, like
/// [`MAX_WINDOW_TABS`](crate::wtabs::MAX_WINDOW_TABS)); the `sprag` CLI still reaches it.
pub(crate) const MAX_SESSION_TABS: usize = 16;

/// The sidebar width in logical pixels — the fixed band it takes down the LEFT of the window (the
/// panes reflow to the window width minus this, like the tab strip takes a band off the height).
pub(crate) const SIDEBAR_WIDTH: u32 = 180;

/// One row's height in logical pixels — tall enough for two lines: the session name and a muted
/// subtitle (its cwd basename + git branch).
const ROW_HEIGHT: u32 = 44;

/// The fixed width in logical pixels of a row's "×" kill hit-target, on the right edge of the row.
/// The switch body flex-grows to fill the rest of the rail, so a click anywhere but the "×"
/// switches and only the "×" kills.
const KILL_WIDTH: u32 = 28;

/// The row-SWITCH button tag for row `i` (its body).
fn row_tag(i: usize) -> String {
    format!("{ROW_TAG_PREFIX}{i}")
}

/// The row-KILL button tag for row `i` (its "×").
fn kill_tag(i: usize) -> String {
    format!("{KILL_TAG_PREFIX}{i}")
}

/// The session-rail EXTRA externals: per possible row a SWITCH button (its body) AND a KILL button
/// (its "×"), plus the "+" new-session action — all at FIXED tags (preserved across the
/// dynamic-external reconcile by tag, like the window tab strip and the context menu). See the
/// module docs for why they are per-row buttons.
pub(crate) fn create_session_externals() -> Vec<ExtraExternal> {
    let mut externals = Vec::with_capacity(2 * MAX_SESSION_TABS + 1);
    for i in 0..MAX_SESSION_TABS {
        externals.push(ExtraExternal::new(
            row_tag(i),
            Box::new(ButtonExternal::new()),
        ));
        externals.push(ExtraExternal::new(
            kill_tag(i),
            Box::new(ButtonExternal::new()),
        ));
    }
    externals.push(ExtraExternal::new(
        NEW_SESSION_TAG.to_owned(),
        Box::new(ButtonExternal::new()),
    ));
    externals
}

/// The session sidebar: a Column of one row per live session (the attached one highlighted)
/// followed by the "+" new-session button. Reads the session list + current session off the
/// [`SlotView`] mirror (no socket call — the paint path). Mounted ONLY on the main window (via
/// [`view::compose`](crate::view)).
pub(crate) fn view_session_sidebar(slots: &SlotView, theme: &Theme) -> Scene {
    let sessions = slots.sessions();
    let current = slots.current_session();
    let mut children: Vec<Scene> = Vec::with_capacity(sessions.len() + 1);
    for (i, session) in sessions.iter().enumerate().take(MAX_SESSION_TABS) {
        let attached = session.name == current;
        children.push(row_node(
            i,
            &session.name,
            session.windows,
            attached,
            session.cwd.as_deref(),
            session.branch.as_deref(),
            &session.ports,
            theme,
        ));
    }
    children.push(new_session_node(theme));
    Scene::Container(
        ContainerNode::new(children)
            .with_tag(SESSION_RAIL_TAG)
            .with_style(BoxStyle::filled(theme.resolve(ColorRole::SurfaceContainer)))
            .with_layout(
                LayoutStyle::new()
                    .flex(FlexDirection::Column)
                    .with_align_items(AlignItems::Stretch)
                    .with_justify(JustifyContent::Start)
                    .with_size(Size::auto().with_width(SizeValue::Px(SIDEBAR_WIDTH))),
            ),
    )
}

/// One session row: a SWITCH body (the session's NAME + window count on the first line, a muted
/// SUBTITLE — cwd basename + git branch + listening ports, [`subtitle`] — on the second) and a "×"
/// KILL target on the right edge — highlighted when it is the ATTACHED session. Two hit-targets in
/// one row: the flex-grown body is tagged so a click SWITCHES this client to row `i`'s session; the
/// fixed-width "×" is tagged so a click KILLS it (killing the attached session detaches this client).
///
/// `cwd` / `branch` (Slice 2) / `ports` (Slice 3) are host-derived facts carried on the
/// [`SessionInfo`](sprag_terminal::SessionInfo): the client only displays them, never reads a path,
/// runs git, or scans `/proc` itself.
#[allow(clippy::too_many_arguments)]
fn row_node(
    i: usize,
    name: &str,
    windows: usize,
    attached: bool,
    cwd: Option<&str>,
    branch: Option<&str>,
    ports: &[u16],
    theme: &Theme,
) -> Scene {
    let (fill, fg) = if attached {
        (
            theme.resolve(ColorRole::SurfaceContainerHighest),
            theme.resolve(ColorRole::Accent),
        )
    } else {
        (Color::TRANSPARENT, theme.resolve(ColorRole::OnSurface))
    };
    // "name  ·  Nw" — the session name with its window count, the same facts `sprag ls` prints.
    let mut lines = vec![text_line(&format!("{name}  ·  {windows}w"), 13, fg)];
    let subtitle = subtitle(cwd, branch, ports);
    if !subtitle.is_empty() {
        lines.push(text_line(
            &subtitle,
            11,
            theme.resolve(ColorRole::OnSurfaceMuted),
        ));
    }
    // The two lines stacked vertically inside the SWITCH body.
    let content = Scene::Container(
        ContainerNode::new(lines).with_layout(
            LayoutStyle::new()
                .flex(FlexDirection::Column)
                .with_align_items(AlignItems::Start)
                .with_justify(JustifyContent::Center),
        ),
    );
    // The SWITCH body: tagged for row `i`'s switch button, flex-grown to fill the rail minus the
    // "×", so a click anywhere but the "×" switches to this session.
    let body = Scene::Container(
        ContainerNode::new(vec![content])
            .with_tag(row_tag(i))
            .with_layout(
                LayoutStyle::new()
                    .flex(FlexDirection::Row)
                    .with_align_items(AlignItems::Center)
                    .with_justify(JustifyContent::Start)
                    .with_padding(Rect::new(12, 0, 0, 0))
                    .with_flex_grow(1.0),
            ),
    );
    // The "×" KILL target on the right edge: tagged for row `i`'s kill button, a fixed band centred
    // on the glyph. Muted so it reads as a secondary affordance, not competing with the highlight.
    let kill = Scene::Container(
        ContainerNode::new(vec![text_line(
            "×",
            15,
            theme.resolve(ColorRole::OnSurfaceMuted),
        )])
        .with_tag(kill_tag(i))
        .with_layout(
            LayoutStyle::new()
                .flex(FlexDirection::Row)
                .with_align_items(AlignItems::Center)
                .with_justify(JustifyContent::Center)
                .with_size(Size::auto().with_width(SizeValue::Px(KILL_WIDTH))),
        ),
    );
    // The outer row carries the highlight fill + fixed height; the two children stretch to fill it
    // (Stretch) so the whole band is hit-testable — the body switch left, the "×" kill right.
    Scene::Container(
        ContainerNode::new(vec![body, kill])
            .with_style(BoxStyle::filled(fill))
            .with_layout(
                LayoutStyle::new()
                    .flex(FlexDirection::Row)
                    .with_align_items(AlignItems::Stretch)
                    .with_justify(JustifyContent::Start)
                    .with_size(Size::auto().with_height(SizeValue::Px(ROW_HEIGHT))),
            ),
    )
}

/// The "+" new-session row at the bottom of the rail, tagged so its click routes to its
/// [`ButtonExternal`].
fn new_session_node(theme: &Theme) -> Scene {
    let label = text_line(
        "+  new session",
        13,
        theme.resolve(ColorRole::OnSurfaceMuted),
    );
    clickable(NEW_SESSION_TAG.to_owned(), label, Color::TRANSPARENT)
}

/// A single left-aligned text line at `px` logical size in `fg` — a row's title or subtitle.
fn text_line(label: &str, px: u32, fg: Color) -> Scene {
    Scene::Text(TextNode::styled(
        label.to_owned(),
        Rect::default(),
        TextStyle::new().with_size_px(px).with_fg(fg),
    ))
}

/// The muted second line of a session row: the cwd's BASENAME, the git BRANCH, and the listening
/// PORTS, joined with a middle dot — where the session is working and what it is serving, at a
/// glance. The basename (not the full path) keeps it inside the narrow rail; the full path is a
/// `sprag ls` away. Any segment that is unknown/empty is dropped (no stray separators); empty when
/// all three are, so the caller omits the line rather than drawing a blank one.
fn subtitle(cwd: Option<&str>, branch: Option<&str>, ports: &[u16]) -> String {
    let mut segments: Vec<String> = Vec::new();
    if let Some(dir) = cwd.and_then(basename) {
        segments.push(dir.to_owned());
    }
    if let Some(branch) = branch {
        segments.push(branch.to_owned());
    }
    if !ports.is_empty() {
        segments.push(ports_label(ports));
    }
    segments.join(" · ")
}

/// The listening ports as a compact `:3000 :8080` badge — space-separated, each colon-prefixed the
/// way cmux shows a served port. Empty for no ports (the [`subtitle`] then drops the segment).
fn ports_label(ports: &[u16]) -> String {
    ports
        .iter()
        .map(|port| format!(":{port}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The last non-empty path component of `path`, for display — `/home/coin/sprag` -> `sprag`.
/// `None` for a path with no named component (e.g. `/`).
fn basename(path: &str) -> Option<&str> {
    path.rsplit('/').find(|component| !component.is_empty())
}

/// A tagged, clickable cell wrapping `content` over `fill`, hit-tested by `tag` (the pinion input
/// router drives the [`ButtonExternal`] registered at that tag on a press — mouse hit-testing is by
/// tag + rect, independent of keyboard focus). Now used only for the "+" new-session action; a row's
/// two hit-targets (switch body + "×" kill) are built inline by [`row_node`], which needs the
/// flex-grow split `clickable`'s single container does not express.
///
/// NOT `with_focusable`: the rail is mouse-first for v1 (like the window tab strip and the context
/// menu, which also defer keyboard nav), so a click still routes but the cell does not enter the pane
/// Tab-order. Keyboard / a11y for the rail is a tracked follow-up; the `sprag` CLI covers keyboard
/// session switching in the meantime.
fn clickable(tag: String, content: Scene, fill: Color) -> Scene {
    Scene::Container(
        ContainerNode::new(vec![content])
            .with_tag(tag)
            .with_style(BoxStyle::filled(fill))
            .with_layout(
                LayoutStyle::new()
                    .flex(FlexDirection::Row)
                    .with_align_items(AlignItems::Center)
                    .with_justify(JustifyContent::Start)
                    .with_padding(Rect::new(12, 0, 12, 0))
                    .with_size(Size::auto().with_height(SizeValue::Px(ROW_HEIGHT))),
            ),
    )
}

/// Route a drained intent: if it is one of the session rail's button "click"s (a row or the "+"),
/// run the corresponding session action against `slots` and report handled. Any other intent is
/// left for the caller's own reducer arms.
pub(crate) fn handle_session_intent(intent: &Intent, slots: &SlotView) -> bool {
    let Some((who, event)) = intent.tag_str().rsplit_once('.') else {
        return false;
    };
    if event != CLICK_EVENT {
        return false;
    }
    if who == NEW_SESSION_TAG {
        // Create a fresh session and switch to it (the wire client does both; the in-process
        // debug host no-ops). The returned name is not needed here — the mirror refresh paints it.
        let _ = slots.new_session();
        return true;
    }
    if let Some(idx) = row_index(who) {
        // Resolve the clicked row's index into the CURRENT session list and switch by NAME. The
        // index is positional (from paint time); re-reading the live list at click time means a
        // list that changed since paint switches to a neighbour or no-ops (`.get(idx)` -> `None`)
        // rather than acting on a stale name — never a panic, never a dead name. `switch_session`
        // itself no-ops a switch to the already-attached session. Benign and self-healing.
        if let Some(session) = slots.sessions().get(idx) {
            slots.switch_session(&session.name);
        }
        return true;
    }
    if let Some(idx) = kill_index(who) {
        // A row's "×": resolve its index into the CURRENT session list and KILL by NAME — the same
        // positional-index-resolved-live discipline as the switch arm (a list that moved since paint
        // kills a neighbour or no-ops, never a panic or a stale name). Killing THIS client's own
        // attached session detaches it; killing another drops that row from the rail
        // ([`SlotView::kill_session`] -> [`WireHost::kill_session`](crate::wire)).
        //
        // TRACKED BOUND (destructive asymmetry vs the switch arm): if the session list mutates OUT OF
        // BAND (another client / the `sprag` CLI) between paint and this click, the index resolves to
        // a NEIGHBOUR — for switch that is benign (correctable), but for KILL it destroys the wrong
        // session, or flips a "kill another" into a self-detach when the neighbour is the attached
        // one. The window needs a concurrent registry mutation and is narrow; the durable fix is a
        // confirmation affordance (tracked follow-up), not a wider index protocol.
        if let Some(session) = slots.sessions().get(idx) {
            slots.kill_session(&session.name);
        }
        return true;
    }
    false
}

/// The row index a `{ROW_TAG_PREFIX}{i}` (switch-body) button tag names, or `None` for any other.
fn row_index(who: &str) -> Option<usize> {
    who.strip_prefix(ROW_TAG_PREFIX)?.parse().ok()
}

/// The row index a `{KILL_TAG_PREFIX}{i}` ("×") button tag names, or `None` for any other. Disjoint
/// from [`row_index`] (`stab` vs `skill` prefixes never both match), so a click resolves to exactly
/// one of switch / kill.
fn kill_index(who: &str) -> Option<usize> {
    who.strip_prefix(KILL_TAG_PREFIX)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pinion_core::GridBuffer;
    use pinion_core::external::IntrospectValue;
    use sprag_host::{HostClient, PaneScrollFacts};
    use sprag_input::Modifiers;
    use sprag_terminal::{LayoutSnapshot, LayoutWire, PaneId, SessionInfo, WindowInfo};
    use std::borrow::Cow;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// A [`HostClient`] that serves a fixed session list and RECORDS the session actions the
    /// reducer invokes — so [`handle_session_intent`]'s dispatch (a row body → `switch_session`, a
    /// row "×" → `kill_session`, each of that session's NAME; "+" → `new_session`) is unit-tested
    /// without a daemon. The in-process `Host` cannot stand in here: it no-ops
    /// `switch_session`/`new_session`/`kill_session` (a debug hatch renders only the default
    /// session), so a recording fake is the only way to observe the routing. The record is behind
    /// `Rc<RefCell<_>>` so the test still reads it after the host is boxed into the `SlotView` (the
    /// slotview `FakeHost` shares its ids the same way). Every other method is an inert default; the
    /// reducer touches only `sessions`/`switch_session`/`kill_session`/`new_session`.
    struct RecordingHost {
        names: Vec<String>,
        switched: Rc<RefCell<Vec<String>>>,
        created: Rc<RefCell<usize>>,
        killed: Rc<RefCell<Vec<String>>>,
    }

    impl HostClient for RecordingHost {
        fn sessions(&self) -> Vec<SessionInfo> {
            self.names
                .iter()
                .map(|name| SessionInfo {
                    name: name.clone(),
                    windows: 1,
                    default: false,
                    cwd: None,
                    branch: None,
                    ports: Vec::new(),
                })
                .collect()
        }
        fn switch_session(&self, name: &str) {
            self.switched.borrow_mut().push(name.to_owned());
        }
        fn new_session(&self) -> String {
            *self.created.borrow_mut() += 1;
            "new".to_owned()
        }
        fn kill_session(&self, name: &str) {
            self.killed.borrow_mut().push(name.to_owned());
        }
        fn current_session(&self) -> String {
            String::new()
        }
        fn pane_ids(&self) -> Vec<PaneId> {
            Vec::new()
        }
        fn pane_cells(&self, _id: PaneId, _off: usize) -> GridBuffer {
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
        fn pane_title(&self, _id: PaneId) -> Option<String> {
            None
        }
        fn layout(&self) -> LayoutSnapshot {
            LayoutSnapshot::default()
        }
        fn set_layout(&self, _tree: LayoutWire, _expected: u64) -> LayoutSnapshot {
            LayoutSnapshot::default()
        }
        fn set_floating(&self, _id: PaneId, _floating: bool) -> LayoutSnapshot {
            LayoutSnapshot::default()
        }
        fn windows(&self) -> Vec<WindowInfo> {
            Vec::new()
        }
        fn select_window(&self, _name: &str) {}
        fn new_window(&self) -> String {
            String::new()
        }
        fn kill_window(&self, _name: &str) {}
    }

    /// The scoped intent tag the shell delivers for a button click at `tag`.
    fn click(tag: &str) -> Intent {
        Intent {
            tag: Cow::Owned(format!("{tag}.{CLICK_EVENT}")),
            payload: IntrospectValue::Null,
        }
    }

    /// A row BODY click routes to `switch_session`, a row "×" to `kill_session`, each of the session
    /// at THAT ROW INDEX by name; the "+" routes to `new_session`; a non-rail intent is left
    /// unhandled. REVERT-PROOF: swap the reducer's `switch_session`/`kill_session`/`new_session`
    /// calls (or mis-index a row) and these assertions change — the routing is not vacuous, and the
    /// switch/kill split is proven distinct (row 1's body switches, row 1's "×" kills, same index).
    #[test]
    fn a_row_body_switches_and_its_x_kills_that_sessions_name_and_plus_creates_one() {
        let switched: Rc<RefCell<Vec<String>>> = Rc::default();
        let created: Rc<RefCell<usize>> = Rc::default();
        let killed: Rc<RefCell<Vec<String>>> = Rc::default();
        let host = RecordingHost {
            names: vec!["0".to_owned(), "work".to_owned(), "work2".to_owned()],
            switched: Rc::clone(&switched),
            created: Rc::clone(&created),
            killed: Rc::clone(&killed),
        };
        let slots = crate::slotview::SlotView::new(Box::new(host));

        // A row BODY click resolves its INDEX into the live list and switches by that session's NAME.
        assert!(handle_session_intent(&click(&row_tag(1)), &slots));
        assert!(handle_session_intent(&click(&row_tag(2)), &slots));
        // A row "×" click resolves the SAME way but KILLS — proving the two per-row targets are
        // routed distinctly (row 1's body switched to 'work'; row 1's "×" kills 'work').
        assert!(handle_session_intent(&click(&kill_tag(1)), &slots));
        assert!(handle_session_intent(&click(&kill_tag(0)), &slots));
        // The "+" creates a session (and the wire client switches to it).
        assert!(handle_session_intent(&click(NEW_SESSION_TAG), &slots));
        // A non-rail intent is NOT consumed (left for the caller's other reducer arms).
        assert!(!handle_session_intent(&click("sprag_gui.pane.0"), &slots));

        assert_eq!(
            *switched.borrow(),
            vec!["work".to_owned(), "work2".to_owned()],
            "row 1 -> 'work', row 2 -> 'work2' (index resolved to the session NAME)",
        );
        assert_eq!(
            *killed.borrow(),
            vec!["work".to_owned(), "0".to_owned()],
            "row 1's × -> 'work', row 0's × -> '0' (killed by NAME, distinct from switch)",
        );
        assert_eq!(*created.borrow(), 1, "the + created one session");
    }

    #[test]
    fn row_and_kill_tags_round_trip_through_their_index_parsers() {
        for i in [0, 3, MAX_SESSION_TABS - 1] {
            // The scoped intent tag a switch-body click arrives as: `{row_tag}.click`.
            let switch = format!("{}.{CLICK_EVENT}", row_tag(i));
            let (who, event) = switch.rsplit_once('.').expect("a scoped tag");
            assert_eq!(event, CLICK_EVENT);
            assert_eq!(
                row_index(who),
                Some(i),
                "the switch-body row index round-trips"
            );
            // ...and a "×" click as: `{kill_tag}.click`.
            let kill = format!("{}.{CLICK_EVENT}", kill_tag(i));
            let (who, event) = kill.rsplit_once('.').expect("a scoped tag");
            assert_eq!(event, CLICK_EVENT);
            assert_eq!(kill_index(who), Some(i), "the kill row index round-trips");
        }
    }

    #[test]
    fn the_switch_kill_and_new_tags_are_never_confused() {
        // The "+" tag must parse as neither a switch nor a kill, or a click on it would act on a
        // session instead of creating one.
        assert_eq!(row_index(NEW_SESSION_TAG), None);
        assert_eq!(kill_index(NEW_SESSION_TAG), None);
        // A switch-body tag is a row and NOT a kill; a "×" tag is a kill and NOT a row — the
        // `stab`/`skill` prefixes are disjoint, so every click resolves to exactly one action.
        assert_eq!(row_index(&row_tag(2)), Some(2));
        assert_eq!(kill_index(&row_tag(2)), None);
        assert_eq!(kill_index(&kill_tag(2)), Some(2));
        assert_eq!(row_index(&kill_tag(2)), None);
    }

    #[test]
    fn one_switch_and_one_kill_button_are_registered_per_row_plus_the_new_action() {
        // The rail routes at most MAX_SESSION_TABS rows — each a switch AND a kill button — plus
        // "+", so 2·MAX + 1 externals.
        assert_eq!(create_session_externals().len(), 2 * MAX_SESSION_TABS + 1);
    }

    /// Every `TextNode` content in `scene`'s subtree, space-joined — the visible glyphs under a node,
    /// so a test can assert WHICH text a tagged sub-node carries.
    fn subtree_text(scene: &Scene) -> String {
        match scene {
            Scene::Text(text) => text.content.clone(),
            Scene::Container(container) => container
                .children
                .iter()
                .map(subtree_text)
                .collect::<Vec<_>>()
                .join(" "),
            _ => String::new(),
        }
    }

    /// The first node in `scene`'s subtree whose intent tag is `tag`.
    fn find_tagged<'a>(scene: &'a Scene, tag: &str) -> Option<&'a Scene> {
        if scene.tag() == Some(tag) {
            return Some(scene);
        }
        match scene {
            Scene::Container(container) => container
                .children
                .iter()
                .find_map(|child| find_tagged(child, tag)),
            _ => None,
        }
    }

    /// The SAFETY-CRITICAL placement the synthetic-intent reducer tests cannot see: the SWITCH tag
    /// sits on the body (which shows the session identity) and the KILL tag on the "×". Swap the two
    /// `.with_tag(...)` in [`row_node`] — so an ordinary body click KILLS instead of switching — and
    /// this fails (the row-tag subtree would then carry the "×", the kill-tag subtree the name). The
    /// reducer tests stay green under that swap; only this paint-structure check catches it.
    #[test]
    fn a_rows_switch_body_carries_the_identity_and_its_x_carries_the_kill_glyph() {
        let theme = Theme::default();
        let scene = row_node(
            3,
            "work",
            2,
            false,
            Some("/home/coin/sprag"),
            Some("main"),
            &[3000],
            &theme,
        );

        let body = find_tagged(&scene, &row_tag(3)).expect("the switch body is tagged for row 3");
        let kill = find_tagged(&scene, &kill_tag(3)).expect("the × is tagged for row 3's kill");

        // The SWITCH body carries the session identity (name + subtitle), NEVER the kill glyph.
        let body_text = subtree_text(body);
        assert!(
            body_text.contains("work"),
            "the switch body shows the session name: {body_text:?}",
        );
        assert!(
            !body_text.contains('×'),
            "the × is not under the switch body: {body_text:?}",
        );
        // The KILL "×" carries the glyph, NEVER the session identity.
        let kill_text = subtree_text(kill);
        assert!(
            kill_text.contains('×'),
            "the × glyph is under the kill target: {kill_text:?}",
        );
        assert!(
            !kill_text.contains("work"),
            "the session name is not under the kill target: {kill_text:?}",
        );
    }

    #[test]
    fn the_subtitle_joins_the_cwd_basename_branch_and_ports() {
        // All three present: "basename · branch · :ports" (the classic prompt shape + what it serves).
        assert_eq!(
            subtitle(Some("/home/coin/sprag"), Some("main"), &[3000, 8080]),
            "sprag · main · :3000 :8080"
        );
        // cwd + branch, no ports: the pre-Slice-3 shape (no trailing separator).
        assert_eq!(
            subtitle(Some("/home/coin/sprag"), Some("main"), &[]),
            "sprag · main"
        );
        // Only one segment present: just that one, no stray separator.
        assert_eq!(subtitle(Some("/home/coin/sprag"), None, &[]), "sprag");
        assert_eq!(subtitle(None, Some("main"), &[]), "main");
        assert_eq!(subtitle(None, None, &[3000]), ":3000");
        // None at all: empty, so the caller omits the second line entirely.
        assert_eq!(subtitle(None, None, &[]), "");
        // basename takes the last NON-EMPTY component (a trailing slash is ignored); `/` has none.
        assert_eq!(subtitle(Some("/var/log/"), None, &[]), "log");
        assert_eq!(basename("/"), None);
    }

    #[test]
    fn ports_label_is_a_compact_colon_prefixed_badge() {
        assert_eq!(ports_label(&[3000]), ":3000");
        assert_eq!(ports_label(&[3000, 8080]), ":3000 :8080");
        assert_eq!(ports_label(&[]), "");
    }
}
