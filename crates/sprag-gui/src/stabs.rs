//! The session SIDEBAR (cmux "workspaces" / tmux sessions): a fixed-width VERTICAL rail down the
//! left of the window, one row per session of the daemon, the attached one highlighted — click a
//! row's BODY to SWITCH this client to that session IN PLACE (tmux `switch-client`), or its "×" to
//! ARM a kill of that session — confirmed by a `kill '<name>'?` prompt that replaces the "+" footer
//! (tmux `kill-session`; killing the ATTACHED session detaches this client) — plus a "+" at the
//! bottom (tmux `new-session`).
//!
//! The kill is DELIBERATELY TWO-STEP: the "×" only captures the session NAME
//! ([`use_pending_kill`]) and the confirm acts on that captured name, so a REORDERING of the list
//! between paint and confirm cannot redirect the kill onto a different row — it kills what the prompt
//! NAMES, not a re-resolved row index. A captured session that VANISHES from the live list (killed
//! out of band — another client, the `sprag` CLI, or its own last pane exiting — while the prompt is
//! up) is AUTO-DISARMED by the pre-view reconcile ([`reconcile_pending_kill`]), so the confirmation
//! strip cannot linger on a session that no longer exists. One inherent residual remains, narrow and
//! pre-existing to the sidebar's name-addressing (there is no stable session id on the wire): NAME
//! REUSE — if the captured session is killed and a NEW one takes its name before the confirm, the
//! name still reads as live (so the reconcile does NOT disarm it) and the confirm hits the new bearer
//! of that name (the prompt cannot tell them apart).
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

use pinion_core::reactive::{Owner, Signal};
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
/// The per-row KILL tag prefix; row `i`'s "×" (a click ARMS a kill of that session, awaiting
/// confirmation) is tagged `{KILL_TAG_PREFIX}{i}`. Distinct from [`ROW_TAG_PREFIX`] (`stab` vs
/// `skill`) so the reducer routes a body click to a SWITCH and an "×" click to a KILL-arm, never
/// confusing the two.
const KILL_TAG_PREFIX: &str = "sprag_gui.skill.";
/// The "Kill" confirm button tag (shown only while a kill is pending) — CONFIRMS the armed kill.
const CONFIRM_KILL_TAG: &str = "sprag_gui.skillok";
/// The "Cancel" button tag (shown only while a kill is pending) — DISARMS the pending kill.
const CANCEL_KILL_TAG: &str = "sprag_gui.skillno";
/// [`Owner::cache`] key for the [`use_pending_kill`] capture.
const PENDING_KILL_KEY: &str = "sprag_gui.stab.pending_kill";
/// The event a [`ButtonExternal`] emits on activation — pinion scopes it as `{tag}.click`.
const CLICK_EVENT: &str = "click";

/// The session NAME a kill is PENDING confirmation on — captured at "×"-click time (client-local,
/// [`Owner::cache`], the [`ctxmenu`](crate::ctxmenu) `use_target_pane` pattern), `None` when no kill
/// is pending. The confirmation strip DISPLAYS this name and the confirm ACTS on it — never a
/// re-resolved row index — so a session list that moved out of band between the "×" click and the
/// confirmation cannot kill the wrong session (the destructive stale-index bound the two-step flow
/// closes). Reading it in the paint subscribes the sidebar so setting it repaints the strip.
fn use_pending_kill() -> Signal<Option<String>> {
    Owner::current()
        .expect("use_pending_kill() requires an active Owner scope")
        .cache(PENDING_KILL_KEY, || Signal::new(None))
        .as_ref()
        .clone()
}

/// AUTO-DISARM the pending kill when the captured session has VANISHED from the live session list —
/// killed out of band (another client, the `sprag` CLI, or its own last pane exiting) while the
/// `kill '<name>'?` strip was up. Without this the strip would linger on a session that no longer
/// exists (its confirm then a benign host no-op), the stale-strip residual the two-step flow
/// otherwise leaves open.
///
/// Runs from [`reconcile_frame`](crate::TerminalViewer) — pinion R1047's pre-view binding-reconcile
/// hook, the SANCTIONED non-view-fn place to WRITE a `Signal` from off-thread-producer facts, exactly
/// like the scrollback-depth reconcile ([`scrollbar::reconcile_scroll`](crate::scrollbar::reconcile_scroll)):
/// the session list lives in the host mirror with no `Signal` for an `Effect` to subscribe (the wire
/// poll thread updates it then repaints), so membership is reconciled here every frame, BEFORE the
/// pure view runs — the view then reads an already-consistent capture and never has to write. The
/// [`set`](Signal::set) EQUALITY-SKIPS (`None` over `None` is inert), so once disarmed this is a
/// no-op and there is no repaint loop; the common no-kill-pending case reads the signal and returns
/// without touching the host.
///
/// Membership is BY NAME (the only session identity on the wire). Only a genuine VANISH disarms: a
/// still-present name is left armed, INCLUDING the inherent name-reuse case (the captured session
/// killed and a NEW one taking its name before this runs) — that residual is unchanged; this closes
/// only the stale-strip-on-a-gone-session one.
pub(crate) fn reconcile_pending_kill(slots: &SlotView) {
    let pending = use_pending_kill();
    if let Some(name) = pending.get() {
        let still_live = slots.sessions().iter().any(|session| session.name == name);
        if !still_live {
            pending.set(None);
        }
    }
}

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
/// switches and only the "×" arms a kill (confirmed via the footer prompt).
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
/// (its "×"), plus the "+" new-session action and the kill CONFIRM / CANCEL buttons — all at FIXED
/// tags (preserved across the dynamic-external reconcile by tag, like the window tab strip and the
/// context menu). The confirm / cancel buttons are registered ALWAYS (fixed tags) but only PAINTED
/// while a kill is pending, exactly as the context-menu external is always registered and painted
/// only when open. See the module docs for why they are per-row buttons.
pub(crate) fn create_session_externals() -> Vec<ExtraExternal> {
    let mut externals = Vec::with_capacity(2 * MAX_SESSION_TABS + 3);
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
    for tag in [NEW_SESSION_TAG, CONFIRM_KILL_TAG, CANCEL_KILL_TAG] {
        externals.push(ExtraExternal::new(
            tag.to_owned(),
            Box::new(ButtonExternal::new()),
        ));
    }
    externals
}

/// The session sidebar: a Column of one row per live session (the attached one highlighted) closed
/// by either the "+" new-session button OR — while a kill is pending — the kill CONFIRMATION strip.
/// Reads the session list + current session off the [`SlotView`] mirror and the pending-kill capture
/// off [`use_pending_kill`] (subscribing the paint, so arming / clearing it repaints) — no socket
/// call on the paint path. Mounted ONLY on the main window (via [`view::compose`](crate::view)).
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
    // The footer: the "Kill '<name>'?" confirmation while a kill is pending (it displays the
    // CAPTURED name — the confirm acts on it, not a row index), else the "+" new-session button.
    match use_pending_kill().get() {
        Some(name) => children.push(confirm_kill_node(&name, theme)),
        None => children.push(new_session_node(theme)),
    }
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
/// fixed-width "×" is tagged so a click ARMS a kill of it — captured by NAME and confirmed via the
/// footer's `kill '<name>'?` prompt (see [`handle_session_intent`]), never an immediate kill.
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

/// The kill CONFIRMATION strip shown at the foot of the rail while a kill is pending (in place of
/// the "+"): a `kill '<name>'?` prompt over a "Kill" confirm button and a "Cancel" button, on an
/// error-tinted fill so the destructive state reads as destructive. `name` is the CAPTURED session
/// name (see [`use_pending_kill`]); the confirm button acts on THAT name, so this strip is what makes
/// the kill immune to a session-list move since the "×" was clicked — it kills what the user READ,
/// not whatever now sits at the clicked row index.
fn confirm_kill_node(name: &str, theme: &Theme) -> Scene {
    let prompt = text_line(
        &format!("kill '{name}'?"),
        12,
        theme.resolve(ColorRole::OnErrorContainer),
    );
    let actions = Scene::Container(
        ContainerNode::new(vec![
            action_pill(CONFIRM_KILL_TAG, "Kill", theme.resolve(ColorRole::Error)),
            action_pill(
                CANCEL_KILL_TAG,
                "Cancel",
                theme.resolve(ColorRole::OnErrorContainer),
            ),
        ])
        .with_layout(
            LayoutStyle::new()
                .flex(FlexDirection::Row)
                .with_align_items(AlignItems::Center)
                .with_justify(JustifyContent::Start),
        ),
    );
    Scene::Container(
        ContainerNode::new(vec![prompt, actions])
            .with_style(BoxStyle::filled(theme.resolve(ColorRole::ErrorContainer)))
            .with_layout(
                LayoutStyle::new()
                    .flex(FlexDirection::Column)
                    .with_align_items(AlignItems::Start)
                    .with_justify(JustifyContent::Center)
                    .with_padding(Rect::new(12, 6, 8, 6)),
            ),
    )
}

/// One compact tagged action button for the kill-confirmation strip (its "Kill" / "Cancel"),
/// content-sized so two fit on the strip's second line — unlike [`clickable`], which forces the full
/// row height for the rail's full-width buttons. The strip itself is CONTENT-height (a prompt line
/// over this action row, no fixed height) so it never clips two stacked lines the way a fixed
/// [`ROW_HEIGHT`] band would.
fn action_pill(tag: &str, label: &str, fg: Color) -> Scene {
    Scene::Container(
        ContainerNode::new(vec![text_line(label, 13, fg)])
            .with_tag(tag.to_owned())
            .with_layout(
                LayoutStyle::new()
                    .flex(FlexDirection::Row)
                    .with_align_items(AlignItems::Center)
                    .with_justify(JustifyContent::Center)
                    .with_padding(Rect::new(0, 2, 14, 2)),
            ),
    )
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

/// Route a drained intent: if it is one of the session rail's button "click"s (a row body, a row
/// "×", the "+", or the kill confirm / cancel), run the corresponding session action against `slots`
/// and report handled. Any other intent is left for the caller's own reducer arms.
///
/// The KILL is TWO-STEP: an "×" ARMS a kill (captures the session's NAME into [`use_pending_kill`]);
/// only the "Kill" confirm button actually kills, acting on the CAPTURED name. That is what closes
/// the destructive stale-index bound — a session list that moves between the "×" and the confirm can
/// never redirect the kill onto a different session, because the confirm never re-reads the index.
/// A switch / new-session / cancel supersedes a pending kill (clears it first).
pub(crate) fn handle_session_intent(intent: &Intent, slots: &SlotView) -> bool {
    let Some((who, event)) = intent.tag_str().rsplit_once('.') else {
        return false;
    };
    if event != CLICK_EVENT {
        return false;
    }
    if who == CONFIRM_KILL_TAG {
        // CONFIRM: kill the CAPTURED name (never a re-resolved index), then disarm. Killing THIS
        // client's own attached session detaches it; killing another drops that row from the rail
        // ([`SlotView::kill_session`] -> [`WireHost::kill_session`](crate::wire)). A session killed
        // out of band while the prompt was up is normally AUTO-DISARMED before the next frame
        // ([`reconcile_pending_kill`]) so the strip is already gone; a confirm click landing in the
        // SAME frame as the vanish (before the reconcile) still targets the gone name, which
        // `kill_session` treats as a benign host-side no-op.
        let pending = use_pending_kill();
        if let Some(name) = pending.get() {
            slots.kill_session(&name);
        }
        pending.set(None);
        return true;
    }
    if who == CANCEL_KILL_TAG {
        // CANCEL: disarm without killing anything.
        use_pending_kill().set(None);
        return true;
    }
    if who == NEW_SESSION_TAG {
        // Create a fresh session and switch to it (the wire client does both; the in-process
        // debug host no-ops). A different action supersedes a pending kill. The returned name is
        // not needed here — the mirror refresh paints it.
        use_pending_kill().set(None);
        let _ = slots.new_session();
        return true;
    }
    if let Some(idx) = row_index(who) {
        // Resolve the clicked row's index into the CURRENT session list and switch by NAME. The
        // index is positional (from paint time); re-reading the live list at click time means a
        // list that changed since paint switches to a neighbour or no-ops (`.get(idx)` -> `None`)
        // rather than acting on a stale name — never a panic, never a dead name. `switch_session`
        // itself no-ops a switch to the already-attached session. Benign and self-healing. A switch
        // supersedes a pending kill.
        use_pending_kill().set(None);
        if let Some(session) = slots.sessions().get(idx) {
            slots.switch_session(&session.name);
        }
        return true;
    }
    if let Some(idx) = kill_index(who) {
        // A row's "×": ARM a kill — CAPTURE the session's NAME now (resolving the paint-time index
        // into the live list once, here) and await confirmation; do NOT kill yet. Capturing the NAME
        // rather than re-resolving the index at confirm time is what closes the destructive
        // stale-index bound: whatever the user then reads in the "kill '<name>'?" prompt is exactly
        // what the confirm kills, immune to any list move in between. A stale index resolving to a
        // neighbour arms THAT neighbour's name (the user sees it in the prompt and can Cancel), never
        // silently kills the wrong one.
        if let Some(session) = slots.sessions().get(idx) {
            use_pending_kill().set(Some(session.name.clone()));
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

    /// A [`HostClient`] that serves a MUTABLE session list and RECORDS the session actions the
    /// reducer invokes — so [`handle_session_intent`]'s dispatch (a row body → `switch_session`, a
    /// row "×" → ARM, the confirm → `kill_session`, "+" → `new_session`, each of the right session's
    /// NAME) is unit-tested without a daemon. The in-process `Host` cannot stand in here: it no-ops
    /// `switch_session`/`new_session`/`kill_session` (a debug hatch renders only the default
    /// session), so a recording fake is the only way to observe the routing. `names` is behind an
    /// `Rc<RefCell<_>>` so a test can MUTATE the list between arming a kill and confirming it (the
    /// stale-index scenario); the record vecs likewise, so the test still reads them after the host is
    /// boxed into the `SlotView`. Every other method is an inert default; the reducer touches only
    /// `sessions`/`switch_session`/`kill_session`/`new_session`.
    struct RecordingHost {
        names: Rc<RefCell<Vec<String>>>,
        switched: Rc<RefCell<Vec<String>>>,
        created: Rc<RefCell<usize>>,
        killed: Rc<RefCell<Vec<String>>>,
    }

    impl HostClient for RecordingHost {
        fn sessions(&self) -> Vec<SessionInfo> {
            self.names
                .borrow()
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

    /// A `SlotView` over a fresh `RecordingHost` seeded with `names`, plus handles onto everything the
    /// reducer records (switched / created / killed) and the mutable name list. The reducer reads /
    /// writes the pending-kill `Signal`, so callers run the reducer inside an `Owner` scope.
    #[allow(clippy::type_complexity)]
    fn recording_slots(
        names: &[&str],
    ) -> (
        crate::slotview::SlotView,
        Rc<RefCell<Vec<String>>>, // switched
        Rc<RefCell<usize>>,       // created
        Rc<RefCell<Vec<String>>>, // killed
        Rc<RefCell<Vec<String>>>, // the live name list (mutate to move the list mid-flow)
    ) {
        let switched: Rc<RefCell<Vec<String>>> = Rc::default();
        let created: Rc<RefCell<usize>> = Rc::default();
        let killed: Rc<RefCell<Vec<String>>> = Rc::default();
        let list: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(
            names.iter().map(|n| (*n).to_owned()).collect(),
        ));
        let host = RecordingHost {
            names: Rc::clone(&list),
            switched: Rc::clone(&switched),
            created: Rc::clone(&created),
            killed: Rc::clone(&killed),
        };
        let slots = crate::slotview::SlotView::new(Box::new(host));
        (slots, switched, created, killed, list)
    }

    /// A row BODY click switches; a row "×" ARMS a kill (captures the NAME, kills NOTHING yet); the
    /// "Kill" confirm then kills the captured name; "Cancel" disarms; "+" creates. Each acts on the
    /// right session's NAME. REVERT-PROOF: an "×" that killed immediately (skipping the confirm)
    /// would make `killed` non-empty before the confirm click; routing the confirm to the wrong sink,
    /// or mis-indexing an arm, changes these exact-match assertions.
    #[test]
    fn the_x_arms_a_kill_that_only_the_confirm_commits() {
        let owner = Owner::new();
        owner.run(|| {
            let (slots, switched, created, killed, _list) =
                recording_slots(&["0", "work", "work2"]);

            // A BODY click switches by NAME.
            assert!(handle_session_intent(&click(&row_tag(1)), &slots));
            assert_eq!(*switched.borrow(), vec!["work".to_owned()]);

            // A row "×" ARMS a kill: it captures the NAME but kills nothing yet.
            assert!(handle_session_intent(&click(&kill_tag(1)), &slots));
            assert_eq!(
                use_pending_kill().get(),
                Some("work".to_owned()),
                "the × captured the session name, awaiting confirmation",
            );
            assert!(killed.borrow().is_empty(), "nothing killed until confirm");

            // The confirm kills the CAPTURED name, then disarms.
            assert!(handle_session_intent(&click(CONFIRM_KILL_TAG), &slots));
            assert_eq!(*killed.borrow(), vec!["work".to_owned()]);
            assert_eq!(use_pending_kill().get(), None, "confirm disarmed");

            // Arm another, then CANCEL: nothing more is killed and the pending clears.
            assert!(handle_session_intent(&click(&kill_tag(0)), &slots));
            assert_eq!(use_pending_kill().get(), Some("0".to_owned()));
            assert!(handle_session_intent(&click(CANCEL_KILL_TAG), &slots));
            assert_eq!(use_pending_kill().get(), None, "cancel disarmed");
            assert_eq!(
                *killed.borrow(),
                vec!["work".to_owned()],
                "cancel killed nothing"
            );

            // The "+" creates a session; a non-rail intent is left for other reducer arms.
            assert!(handle_session_intent(&click(NEW_SESSION_TAG), &slots));
            assert_eq!(*created.borrow(), 1);
            assert!(!handle_session_intent(&click("sprag_gui.pane.0"), &slots));
        });
    }

    /// THE BOUND THIS FEATURE CLOSES: if the session list MOVES between the "×" (arm) and the
    /// confirm, the confirm still kills the session the user READ in the prompt — the CAPTURED name —
    /// never whatever now sits at the clicked row index. REVERT-PROOF: make the confirm re-resolve the
    /// index instead of using the captured name and this kills "work" (the new occupant of index 0),
    /// failing the assertion.
    #[test]
    fn a_list_move_between_arm_and_confirm_cannot_redirect_the_kill() {
        let owner = Owner::new();
        owner.run(|| {
            let (slots, _switched, _created, killed, list) =
                recording_slots(&["0", "work", "work2"]);

            // Arm a kill of row 0 — the session named "0" — capturing that NAME.
            assert!(handle_session_intent(&click(&kill_tag(0)), &slots));
            assert_eq!(use_pending_kill().get(), Some("0".to_owned()));

            // The list moves OUT OF BAND (another client killed "0"): index 0 is now "work".
            *list.borrow_mut() = vec!["work".to_owned(), "work2".to_owned()];

            // Confirm: it kills the CAPTURED "0", NOT "work" (the new index-0 session).
            assert!(handle_session_intent(&click(CONFIRM_KILL_TAG), &slots));
            assert_eq!(
                *killed.borrow(),
                vec!["0".to_owned()],
                "the confirm killed the captured name, immune to the list move",
            );
        });
    }

    /// While a kill is pending, the footer paints the confirmation strip (the captured name + the
    /// CONFIRM / CANCEL buttons) in place of the "+"; with nothing pending it paints the "+".
    /// REVERT-PROOF: a footer that ignored the pending signal would keep the "+" tag and never carry
    /// the name, failing both halves.
    #[test]
    fn the_footer_shows_the_kill_confirmation_only_while_a_kill_is_pending() {
        let owner = Owner::new();
        owner.run(|| {
            let (slots, ..) = recording_slots(&["0", "work"]);
            let theme = Theme::default();

            // Nothing pending: the footer is the "+" new-session button, no confirm strip.
            let idle = view_session_sidebar(&slots, &theme);
            assert!(
                find_tagged(&idle, NEW_SESSION_TAG).is_some(),
                "the + is shown"
            );
            assert!(
                find_tagged(&idle, CONFIRM_KILL_TAG).is_none(),
                "no confirm strip while idle",
            );

            // Arm a kill of a SENTINEL name that is NOT any session row ("0"/"work"), so the ONLY
            // place it can appear in the painted tree is the confirmation PROMPT — arming a real
            // session name ("work") would also match its own row and make the display assertion
            // vacuous (it would pass even if the prompt omitted the name).
            let pending = "sentinel-kill-target";
            use_pending_kill().set(Some(pending.to_owned()));
            let armed = view_session_sidebar(&slots, &theme);
            let confirm =
                find_tagged(&armed, CONFIRM_KILL_TAG).expect("the Kill confirm button is shown");
            assert_eq!(subtree_text(confirm), "Kill");
            assert!(
                find_tagged(&armed, CANCEL_KILL_TAG).is_some(),
                "the Cancel button is shown",
            );
            assert!(
                find_tagged(&armed, NEW_SESSION_TAG).is_none(),
                "the + is replaced while a kill is pending",
            );
            // The PROMPT displays the captured name — provable because the sentinel appears in NO
            // row, so a `confirm_kill_node` that dropped the name would fail this.
            assert!(
                subtree_text(&armed).contains(pending),
                "the confirmation prompt names the captured session",
            );
        });
    }

    /// Arming a kill then SWITCHING (or creating) supersedes it — the pending clears, so the
    /// confirmation strip cannot linger on a session the user has navigated away from. REVERT-PROOF:
    /// drop the `use_pending_kill().set(None)` from the switch / new-session arms and the pending
    /// stays `Some`, failing these assertions.
    #[test]
    fn a_switch_or_new_session_supersedes_a_pending_kill() {
        let owner = Owner::new();
        owner.run(|| {
            let (slots, _switched, _created, killed, _list) =
                recording_slots(&["0", "work", "work2"]);

            // Arm, then SWITCH by clicking another row: the pending clears, nothing is killed.
            assert!(handle_session_intent(&click(&kill_tag(0)), &slots));
            assert_eq!(use_pending_kill().get(), Some("0".to_owned()));
            assert!(handle_session_intent(&click(&row_tag(2)), &slots));
            assert_eq!(
                use_pending_kill().get(),
                None,
                "a switch superseded the pending kill",
            );

            // Arm again, then NEW-session: the pending clears again.
            assert!(handle_session_intent(&click(&kill_tag(1)), &slots));
            assert_eq!(use_pending_kill().get(), Some("work".to_owned()));
            assert!(handle_session_intent(&click(NEW_SESSION_TAG), &slots));
            assert_eq!(
                use_pending_kill().get(),
                None,
                "a new-session superseded the pending kill",
            );
            assert!(killed.borrow().is_empty(), "superseding never kills");
        });
    }

    /// AUTO-DISARM: a pending kill whose captured session VANISHES from the live list (killed out of
    /// band while the `kill '<name>'?` strip was up) is cleared by the pre-view reconcile, so the
    /// strip cannot linger on a session that no longer exists — while a capture that is STILL LIVE
    /// stays armed. REVERT-PROOF in BOTH directions: a reconcile that never disarmed would leave the
    /// vanished capture pending (the second half fails); one that disarmed unconditionally would drop
    /// the still-live capture (the first half fails).
    #[test]
    fn a_vanished_captured_session_auto_disarms_the_pending_kill() {
        let owner = Owner::new();
        owner.run(|| {
            let (slots, _switched, _created, killed, list) =
                recording_slots(&["0", "work", "work2"]);

            // Arm a kill of "work"; it is captured by NAME.
            assert!(handle_session_intent(&click(&kill_tag(1)), &slots));
            assert_eq!(use_pending_kill().get(), Some("work".to_owned()));

            // STILL LIVE: the reconcile leaves an armed-and-present capture untouched.
            reconcile_pending_kill(&slots);
            assert_eq!(
                use_pending_kill().get(),
                Some("work".to_owned()),
                "a still-live captured session stays armed",
            );

            // "work" is killed OUT OF BAND (another client / the CLI): it leaves the live list.
            *list.borrow_mut() = vec!["0".to_owned(), "work2".to_owned()];

            // VANISHED: the reconcile auto-disarms the now-stale capture, killing nothing.
            reconcile_pending_kill(&slots);
            assert_eq!(
                use_pending_kill().get(),
                None,
                "the vanished captured session auto-disarmed",
            );
            assert!(killed.borrow().is_empty(), "auto-disarm never kills");
        });
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
        // The footer action tags (kill confirm / cancel) parse as NEITHER a switch nor a kill index
        // — they are matched by exact `==` arms before the prefix arms, and `skillok`/`skillno` also
        // fail the `sprag_gui.skill.` prefix (no trailing dot), so a click on them never arms/kills a
        // row.
        for tag in [CONFIRM_KILL_TAG, CANCEL_KILL_TAG] {
            assert_eq!(row_index(tag), None);
            assert_eq!(kill_index(tag), None);
        }
    }

    #[test]
    fn per_row_switch_and_kill_buttons_plus_the_new_confirm_and_cancel_actions_are_registered() {
        // The rail routes at most MAX_SESSION_TABS rows — each a switch AND a kill button — plus the
        // three footer actions ("+", kill-confirm, kill-cancel), so 2·MAX + 3 externals.
        assert_eq!(create_session_externals().len(), 2 * MAX_SESSION_TABS + 3);
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
