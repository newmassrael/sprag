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
//!
//! ## Keyboard + a11y (R179)
//!
//! The rail is a WAI-ARIA `tablist` of session `tab`s laid OVER the mouse rows above (no per-row
//! external rewrite): the session-list sub-container ([`SESSION_TABLIST_TAG`]) is the list's SINGLE
//! keyboard Tab stop, and a client-local roving CURSOR ([`use_session_cursor`]) — the
//! `aria-activedescendant` — moves within it. [`handle_sidebar_key`] owns the model: `↑`/`↓` /
//! `Home` / `End` rove the cursor, `Enter` / `Space` SWITCH to it, `Delete` ARMs a kill, and while a
//! kill is pending `Enter` CONFIRMs / `Escape` CANCELs. Every activation reuses
//! [`handle_session_intent`] by synthesising the same `{tag}.click` intent a mouse press produces,
//! so pointer, keyboard, and AT (a screen-reader Click lowers to `apply_key("Enter")`) share ONE
//! routing SSOT. The "+" footer is a separate focusable `button` Tab stop.
//! [`route_key`](crate::input::route_key) dispatches here — before its pane focus gate — for a tag
//! [`is_sidebar_focus`] recognises; the a11y tree is [`session_sidebar_access_nodes`]. (Pre-R179 the
//! rail was mouse-first — its keyboard analog was the `sprag` CLI + the `Ctrl+Shift` session chords.)

use pinion_a11y::{AccessNode, AriaRole};
use pinion_core::external::IntrospectValue;
use pinion_core::reactive::{Owner, Signal};
use pinion_core::scene::{ContainerNode, Rect, TextNode};
use pinion_core::style::{
    AlignItems, Border, BoxStyle, FlexDirection, JustifyContent, LayoutStyle, Size, SizeValue,
    TextStyle,
};
use pinion_core::theme::{ColorRole, Theme};
use pinion_core::widget_core::ExtraExternal;
use pinion_core::widgets::button::ButtonExternal;
use pinion_core::{Color, Intent, Scene};
use sprag_terminal::SessionActivity;
use std::borrow::Cow;

use crate::slotview::SlotView;

/// The sidebar container tag (the Column of the session TABLIST + the "+" / confirm footer).
const SESSION_RAIL_TAG: &str = "sprag_gui.srail";
/// The session TABLIST tag — the Column holding the session rows, marked `with_focusable(true)` so
/// the WHOLE list is a SINGLE keyboard Tab stop (the WAI-ARIA `tablist` roving-tabindex model:
/// `↑`/`↓` move a cursor between sessions, `Enter` switches, `Delete` arms a kill — see
/// [`handle_sidebar_key`]). Distinct from [`SESSION_RAIL_TAG`] (the whole rail, which also holds the
/// separately-focusable "+" footer) so Tab lands on the list ONCE, not on every row.
const SESSION_TABLIST_TAG: &str = "sprag_gui.stablist";
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
/// [`Owner::cache`] key for the [`use_session_cursor`] keyboard cursor.
const SESSION_CURSOR_KEY: &str = "sprag_gui.stab.cursor";
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

/// The session NAME the keyboard cursor rests on within the [`SESSION_TABLIST_TAG`] tablist —
/// client-local ([`Owner::cache`], the [`use_pending_kill`] pattern), `None` before any keyboard
/// navigation. `↑`/`↓`/`Home`/`End` write it; the paint reads it (subscribing, so a move repaints
/// the cursor ring) — but ONLY the resolved index is authoritative ([`resolve_cursor_index`]): a
/// name that no longer exists falls back to the attached session, so a cursor whose session was
/// killed out of band needs no reconcile pass. Distinct from [`use_pending_kill`]: the cursor is
/// where the NEXT action lands; the pending kill is an already-armed one awaiting confirmation.
fn use_session_cursor() -> Signal<Option<String>> {
    Owner::current()
        .expect("use_session_cursor() requires an active Owner scope")
        .cache(SESSION_CURSOR_KEY, || Signal::new(None))
        .as_ref()
        .clone()
}

/// The row index the keyboard cursor resolves to: the remembered [`use_session_cursor`] name when it
/// is still a LIVE session, else the ATTACHED session, else the first row. `None` only when there
/// are no sessions. Falling back to the attached session (rather than reconciling the signal) means
/// a cursor whose session was killed out of band snaps to a live row on the very next read, with no
/// separate reconcile hook. Pure / unit-testable.
fn resolve_cursor_index(names: &[String], cursor: Option<&str>, attached: &str) -> Option<usize> {
    if names.is_empty() {
        return None;
    }
    if let Some(cursor) = cursor
        && let Some(idx) = names.iter().position(|name| name == cursor)
    {
        return Some(idx);
    }
    names.iter().position(|name| name == attached).or(Some(0))
}

/// Whether `tag` is one of the session rail's KEYBOARD-focusable tags — the session tablist (the
/// list's single Tab stop) or one of its footer buttons ("+", or the transient kill confirm /
/// cancel). [`route_key`](crate::input::route_key) consults this BEFORE the pane focus gate so a
/// key delivered while the rail owns focus routes to [`handle_sidebar_key`] instead of the PTY.
/// The confirm / cancel tags are included for the AT activation path (a screen-reader Click lowers
/// to `apply_key(tag, "Enter")` with that tag focused) even though they are not Tab stops — the
/// keyboard reaches them via the tablist's `Enter` / `Escape` while a kill is pending.
pub(crate) fn is_sidebar_focus(tag: &str) -> bool {
    matches!(
        tag,
        SESSION_TABLIST_TAG | NEW_SESSION_TAG | CONFIRM_KILL_TAG | CANCEL_KILL_TAG
    )
}

/// The scoped `{tag}.click` intent a mouse press on `tag` produces — synthesised so a keyboard /
/// AT activation runs the SAME [`handle_session_intent`] routing (switch / arm-kill / confirm /
/// cancel / new) a click does, rather than duplicating that decision. ONE activation SSOT for
/// pointer, keyboard, and AT.
fn synth_click(tag: &str) -> Intent {
    Intent {
        tag: Cow::Owned(format!("{tag}.{CLICK_EVENT}")),
        payload: IntrospectValue::Null,
    }
}

/// The session rail's KEYBOARD model, dispatched from [`route_key`](crate::input::route_key) when
/// the rail owns focus (`focused` is one of [`is_sidebar_focus`]'s tags). Returns whether the key
/// was consumed.
///
/// The tablist ([`SESSION_TABLIST_TAG`]) is a SINGLE Tab stop with a roving cursor
/// ([`use_session_cursor`]) — the WAI-ARIA `tablist` model:
/// - `↑`/`↓` (and `Home`/`End`) move the cursor between sessions (continuous — a held arrow keeps
///   roving); the paint repaints the cursor ring.
/// - `Enter`/`Space` SWITCH this client to the cursor session; `Delete`/`Backspace` ARM a kill of
///   it (both DISCRETE — one action per press, so a held key does not re-fire). Both reuse
///   [`handle_session_intent`] via [`synth_click`], so keyboard and mouse share one routing SSOT.
/// - While a kill is PENDING the tablist is modal-ish: `Enter`/`Space` CONFIRM, `Escape` CANCELs,
///   and every other key is swallowed so navigation cannot run under the confirmation prompt.
///
/// The footer buttons ("+" / kill confirm / cancel) activate on `Enter`/`Space` — the plain-button
/// path that also serves the AT `Click → apply_key("Enter")` activation for the confirm / cancel
/// (which are not Tab stops but are AT-reachable).
pub(crate) fn handle_sidebar_key(focused: &str, key: &str, repeat: bool, slots: &SlotView) -> bool {
    // The footer buttons ("+", or the transient confirm / cancel): a plain button activates on
    // Enter / Space. This also serves the AT activation path (Click lowers to apply_key("Enter")
    // with the button's tag focused), so it must run for a tag that is not a keyboard Tab stop.
    if matches!(
        focused,
        NEW_SESSION_TAG | CONFIRM_KILL_TAG | CANCEL_KILL_TAG
    ) {
        if !repeat && matches!(key, "Enter" | "Space") {
            return handle_session_intent(&synth_click(focused), slots);
        }
        return false;
    }
    if focused != SESSION_TABLIST_TAG {
        return false;
    }
    // While a kill is PENDING the confirmation prompt owns the tablist: Enter confirms, Escape
    // cancels, every other key is swallowed so a stray arrow / Delete cannot run under the prompt.
    if use_pending_kill().get().is_some() {
        if !repeat {
            match key {
                "Enter" | "Space" => {
                    handle_session_intent(&synth_click(CONFIRM_KILL_TAG), slots);
                }
                "Escape" => {
                    handle_session_intent(&synth_click(CANCEL_KILL_TAG), slots);
                }
                _ => {}
            }
        }
        return true;
    }
    let names: Vec<String> = slots.sessions().into_iter().map(|info| info.name).collect();
    let attached = slots.current_session();
    let Some(idx) = resolve_cursor_index(&names, use_session_cursor().get().as_deref(), &attached)
    else {
        // No sessions to navigate — swallow so the key does not leak past the PTY-less rail.
        return true;
    };
    // Navigation is CONTINUOUS (a held arrow keeps roving), so it runs on auto-repeat too.
    let moved = match key {
        "ArrowDown" => crate::input::session_neighbour(&names, &names[idx], true),
        "ArrowUp" => crate::input::session_neighbour(&names, &names[idx], false),
        "Home" => names.first().cloned(),
        "End" => names.last().cloned(),
        _ => None,
    };
    if let Some(target) = moved {
        use_session_cursor().set(Some(target));
        return true;
    }
    // Activation is DISCRETE (one per press): drop the OS auto-repeat re-send so a held Enter does
    // not re-switch and a held Delete does not re-arm.
    if repeat {
        return matches!(key, "Enter" | "Space" | "Delete" | "Backspace");
    }
    match key {
        "Enter" | "Space" => {
            handle_session_intent(&synth_click(&row_tag(idx)), slots);
            true
        }
        "Delete" | "Backspace" => {
            handle_session_intent(&synth_click(&kill_tag(idx)), slots);
            true
        }
        _ => false,
    }
}

/// The rail's accessible tree (main window only): a WAI-ARIA `tablist` of session `tab`s — each
/// carrying `aria-selected` (the attached session), the `posinset` / `setsize` "tab N of M" axes,
/// and — while the tablist owns focus — `aria-activedescendant` on the cursor row
/// ([`AccessNode::with_focused`]) — followed by the footer's `button`s ("+", or the transient kill
/// confirm / cancel). Mirrors the `radiogroup` builder's `[parent, ...children]` flat-list shape
/// ([`pinion_a11y::radiogroup_radio_nodes`]); the bounds are left `None` for the shell to resolve
/// from each tag's painted rect (the [`access_nodes_for_window`](crate::a11y) discipline). Empty
/// when there are no sessions (no rail is painted then either).
pub(crate) fn session_sidebar_access_nodes(
    slots: &SlotView,
    focused: Option<&str>,
) -> Vec<AccessNode> {
    let sessions = slots.sessions();
    if sessions.is_empty() {
        return Vec::new();
    }
    let attached = slots.current_session();
    let count = sessions.len().min(MAX_SESSION_TABS);
    // The cursor's activedescendant only matters while the tablist actually owns focus.
    let cursor_idx = if focused == Some(SESSION_TABLIST_TAG) {
        let names: Vec<String> = sessions.iter().map(|info| info.name.clone()).collect();
        resolve_cursor_index(&names, use_session_cursor().get().as_deref(), &attached)
    } else {
        None
    };
    // The sampled half, joined by NAME below — the same read the paint does, for the same reason
    // (an announced name and a painted row must state the same facts).
    let activity = slots.session_activity();
    let mut nodes: Vec<AccessNode> = Vec::with_capacity(count + 2);
    let mut tablist = AccessNode::new(SESSION_TABLIST_TAG, AriaRole::TabList).with_name("Sessions");
    for i in 0..count {
        tablist = tablist.with_child(row_tag(i));
    }
    nodes.push(tablist);
    for (i, session) in sessions.iter().enumerate().take(MAX_SESSION_TABS) {
        nodes.push(
            AccessNode::new(row_tag(i), AriaRole::Tab)
                .with_name(sidebar_access_name(
                    session,
                    activity.iter().find(|row| row.name == session.name),
                ))
                .with_selected(session.name == attached)
                .with_set_position(i, count)
                .with_focused(cursor_idx == Some(i)),
        );
    }
    // The footer: the "+" new-session button when idle, else the confirmation strip's actions.
    match use_pending_kill().get() {
        None => {
            nodes.push(AccessNode::new(NEW_SESSION_TAG, AriaRole::Button).with_name("New session"))
        }
        Some(name) => {
            nodes.push(
                AccessNode::new(CONFIRM_KILL_TAG, AriaRole::Button)
                    .with_name(format!("Confirm kill session {name}")),
            );
            nodes.push(AccessNode::new(CANCEL_KILL_TAG, AriaRole::Button).with_name("Cancel kill"));
        }
    }
    nodes
}

/// A session tab's spoken accessible name — the same facts the row PAINTS, phrased for a screen
/// reader: `"work, 2 windows, sprag, main"` (name, window count, cwd basename, git branch). The
/// listening ports are display-only glanceable state, omitted from the announced name.
///
/// `activity` is this session's row of the SAMPLE (R282), which is a separate answer from the
/// session list and may be absent for a session the sample has not seen — a session created since
/// the last one was taken. Absent reads exactly like a session with no cwd: the name states the
/// window count and stops. That is the honest degradation, and it is why this takes an `Option`
/// rather than defaulting the fields.
fn sidebar_access_name(
    session: &sprag_terminal::SessionInfo,
    activity: Option<&SessionActivity>,
) -> String {
    let mut name = format!(
        "{}, {} window{}",
        session.name,
        session.windows,
        if session.windows == 1 { "" } else { "s" }
    );
    if let Some(dir) = activity
        .and_then(|row| row.cwd.as_deref())
        .and_then(basename)
    {
        name.push_str(", ");
        name.push_str(dir);
    }
    if let Some(branch) = activity.and_then(|row| row.branch.as_deref()) {
        name.push_str(", ");
        name.push_str(branch);
    }
    name
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
    // The sampled half, read from the same mirror the list came from and JOINED BY NAME rather than
    // by position: the two are separate answers over separate requests, so a session created between
    // them would shift every row after it if this indexed. A name is a session's address.
    let activity = slots.session_activity();
    let current = slots.current_session();
    // The keyboard cursor's row — highlighted ONLY while the tablist actually owns focus (so the
    // ring appears on Tab-in and clears on Tab-out). Reading both signals subscribes the paint, so
    // a cursor move (`↑`/`↓`) or a focus change repaints; `None` (no ring) when the rail is not
    // focused. The resolved index self-heals a stale cursor to the attached row
    // ([`resolve_cursor_index`]).
    let names: Vec<String> = sessions.iter().map(|info| info.name.clone()).collect();
    let cursor_name = use_session_cursor().get();
    let tablist_focused =
        pinion_core::focus_state::focused().as_deref() == Some(SESSION_TABLIST_TAG);
    let cursor_idx = if tablist_focused {
        resolve_cursor_index(&names, cursor_name.as_deref(), &current)
    } else {
        None
    };
    let mut rows: Vec<Scene> = Vec::with_capacity(sessions.len());
    for (i, session) in sessions.iter().enumerate().take(MAX_SESSION_TABS) {
        rows.push(row_node(
            i,
            &session.name,
            session.windows,
            session.name == current,
            cursor_idx == Some(i),
            activity.iter().find(|row| row.name == session.name),
            session.attached,
            theme,
        ));
    }
    // The rows are wrapped in a SINGLE focusable container (the WAI-ARIA `tablist`): Tab lands on
    // the list ONCE and `↑`/`↓` rove WITHIN it ([`handle_sidebar_key`]), rather than making every
    // row its own Tab stop. Focusable only when non-empty, so an empty rail is never a focus trap.
    let tablist = Scene::Container(
        ContainerNode::new(rows)
            .with_tag(SESSION_TABLIST_TAG)
            .with_layout(
                LayoutStyle::new()
                    .flex(FlexDirection::Column)
                    .with_align_items(AlignItems::Stretch)
                    .with_justify(JustifyContent::Start)
                    .with_focusable(!sessions.is_empty()),
            ),
    );
    // The footer: the "Kill '<name>'?" confirmation while a kill is pending (it displays the
    // CAPTURED name — the confirm acts on it, not a row index), else the "+" new-session button.
    let footer = match use_pending_kill().get() {
        Some(name) => confirm_kill_node(&name, theme),
        None => new_session_node(theme),
    };
    Scene::Container(
        ContainerNode::new(vec![tablist, footer])
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
/// KILL target on the right edge — filled when it is the ATTACHED session, and outlined with an
/// accent border when it is the keyboard CURSOR row (`is_cursor`, set only while the tablist owns
/// focus — see [`handle_sidebar_key`]). Two hit-targets in one row: the flex-grown body is tagged
/// so a click SWITCHES this client to row `i`'s session; the fixed-width "×" is tagged so a click
/// ARMS a kill of it — captured by NAME and confirmed via the footer's `kill '<name>'?` prompt (see
/// [`handle_session_intent`]), never an immediate kill.
///
/// `activity` is this session's row of the host's SAMPLE — cwd, git branch, listening ports, all
/// host-derived ([`SessionActivity`]), so the client only displays
/// them and never reads a path, runs git, or scans `/proc` itself. `None` for a session the sample
/// has not seen yet, which paints the row without its subtitle rather than inventing one.
#[allow(clippy::too_many_arguments)]
fn row_node(
    i: usize,
    name: &str,
    windows: usize,
    attached: bool,
    is_cursor: bool,
    activity: Option<&SessionActivity>,
    viewers: usize,
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
    let subtitle = subtitle(
        activity.and_then(|row| row.cwd.as_deref()),
        activity.and_then(|row| row.branch.as_deref()),
        activity.map_or(&[][..], |row| row.ports.as_slice()),
        viewers,
    );
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
    // (Stretch) so the whole band is hit-testable — the body switch left, the "×" kill right. The
    // keyboard cursor row adds an accent OUTLINE (orthogonal to the attached FILL, so the cursor
    // can rest on a non-attached row while the attached one stays filled).
    let mut box_style = BoxStyle::filled(fill);
    if is_cursor {
        box_style = box_style.with_border(Border::new(theme.resolve(ColorRole::Accent), 2));
    }
    Scene::Container(
        ContainerNode::new(vec![body, kill])
            .with_style(box_style)
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
fn subtitle(cwd: Option<&str>, branch: Option<&str>, ports: &[u16], viewers: usize) -> String {
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
    // The attached-CLIENT count (R-PR67) — tmux `list-clients` / cmux "N viewing" — shown when at
    // least one client (possibly this one) is attached; absent at 0, so a session nobody watches
    // carries no segment. Raw count, self included, like `list-clients`.
    if viewers > 0 {
        segments.push(format!("{viewers} viewing"));
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

/// A tagged, focusable, clickable cell wrapping `content` over `fill`, hit-tested by `tag` (the
/// pinion input router drives the [`ButtonExternal`] registered at that tag on a press — mouse
/// hit-testing is by tag + rect, independent of keyboard focus). Now used only for the "+"
/// new-session action; a row's two hit-targets (switch body + "×" kill) are built inline by
/// [`row_node`], which needs the flex-grow split `clickable`'s single container does not express.
///
/// `with_focusable(true)` (R179): the "+" is a keyboard Tab stop of its own — a WAI-ARIA `button`
/// beside the session `tablist` — so `Tab` reaches it and `Enter` / `Space` creates a session
/// ([`handle_sidebar_key`]). (Pre-R179 the whole rail was mouse-first; the `sprag` CLI covered
/// keyboard session control in the meantime.)
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
                    .with_size(Size::auto().with_height(SizeValue::Px(ROW_HEIGHT)))
                    .with_focusable(true),
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
        /// No sample: these fixtures exercise the ROUTING over a session list, not the facts a row
        /// paints. An empty reading of age zero is the honest "nothing sampled here" (see
        /// `HostClient::session_activity`), and it keeps every subtitle out of the fixture's way.
        fn session_activity(&self) -> sprag_terminal::ActivityReading {
            sprag_terminal::ActivityReading {
                age: std::time::Duration::ZERO,
                value: Vec::new(),
            }
        }
        fn sessions(&self) -> Vec<SessionInfo> {
            self.names
                .borrow()
                .iter()
                .map(|name| SessionInfo {
                    name: name.clone(),
                    windows: 1,
                    panes: 1,
                    default: false,
                    attached: 0,
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
        fn pane_prompt_positions(&self, _id: PaneId) -> Vec<usize> {
            Vec::new()
        }
        fn pane_grid_size(&self, _id: PaneId) -> (u16, u16) {
            (1, 1)
        }
        fn resize(&self, _id: PaneId, _cols: u16, _rows: u16, _cell_px: (u16, u16)) {}
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
        fn select_window_toward(&self, _step: sprag_terminal::WindowStep) -> Option<String> {
            None
        }
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
            false,
            Some(&SessionActivity {
                name: "work".to_owned(),
                cwd: Some("/home/coin/sprag".to_owned()),
                branch: Some("main".to_owned()),
                ports: vec![3000],
            }),
            0,
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
            subtitle(Some("/home/coin/sprag"), Some("main"), &[3000, 8080], 0),
            "sprag · main · :3000 :8080"
        );
        // cwd + branch, no ports: the pre-Slice-3 shape (no trailing separator).
        assert_eq!(
            subtitle(Some("/home/coin/sprag"), Some("main"), &[], 0),
            "sprag · main"
        );
        // Only one segment present: just that one, no stray separator.
        assert_eq!(subtitle(Some("/home/coin/sprag"), None, &[], 0), "sprag");
        assert_eq!(subtitle(None, Some("main"), &[], 0), "main");
        assert_eq!(subtitle(None, None, &[3000], 0), ":3000");
        // None at all: empty, so the caller omits the second line entirely.
        assert_eq!(subtitle(None, None, &[], 0), "");
        // basename takes the last NON-EMPTY component (a trailing slash is ignored); `/` has none.
        assert_eq!(subtitle(Some("/var/log/"), None, &[], 0), "log");
        assert_eq!(basename("/"), None);
    }

    #[test]
    fn the_subtitle_appends_a_viewer_count_when_attached() {
        // Viewers > 0 add a trailing "N viewing" segment (R-PR67), joined like the others.
        assert_eq!(subtitle(None, None, &[], 1), "1 viewing");
        assert_eq!(subtitle(None, None, &[], 2), "2 viewing");
        assert_eq!(
            subtitle(Some("/home/coin/sprag"), Some("main"), &[3000], 3),
            "sprag · main · :3000 · 3 viewing"
        );
        // Zero viewers add nothing — a session nobody watches keeps its prior shape.
        assert_eq!(subtitle(Some("/home/coin/sprag"), None, &[], 0), "sprag");
    }

    #[test]
    fn ports_label_is_a_compact_colon_prefixed_badge() {
        assert_eq!(ports_label(&[3000]), ":3000");
        assert_eq!(ports_label(&[3000, 8080]), ":3000 :8080");
        assert_eq!(ports_label(&[]), "");
    }

    // ── R179 keyboard / a11y ──────────────────────────────────────────────────────────────────

    /// The keyboard cursor resolves to the remembered name when live, else the ATTACHED session,
    /// else the first row — `None` only when there are no sessions.
    #[test]
    fn resolve_cursor_index_prefers_the_cursor_then_the_attached_then_first() {
        let names: Vec<String> = ["a", "b", "c"].iter().map(|s| (*s).to_owned()).collect();
        // A live remembered cursor wins.
        assert_eq!(resolve_cursor_index(&names, Some("b"), "a"), Some(1));
        // A stale cursor (its session gone) falls back to the attached session.
        assert_eq!(resolve_cursor_index(&names, Some("gone"), "c"), Some(2));
        // No cursor yet -> the attached session.
        assert_eq!(resolve_cursor_index(&names, None, "a"), Some(0));
        // Neither cursor nor attached present -> the first row (never a dead index).
        assert_eq!(resolve_cursor_index(&names, None, "gone"), Some(0));
        // No sessions -> nothing to rest on.
        assert_eq!(resolve_cursor_index(&[], Some("a"), "a"), None);
    }

    /// Only the rail's KEYBOARD-focus tags (the tablist + the footer buttons) are sidebar focus —
    /// NOT a per-row switch/kill tag (the rows are reached by roving the tablist cursor, not Tab)
    /// nor a pane. REVERT-PROOF: adding a row tag here would make Tab land on every row.
    #[test]
    fn is_sidebar_focus_matches_the_rail_tags_only() {
        assert!(is_sidebar_focus(SESSION_TABLIST_TAG));
        assert!(is_sidebar_focus(NEW_SESSION_TAG));
        assert!(is_sidebar_focus(CONFIRM_KILL_TAG));
        assert!(is_sidebar_focus(CANCEL_KILL_TAG));
        assert!(
            !is_sidebar_focus(&row_tag(0)),
            "a row body is not its own Tab stop"
        );
        assert!(
            !is_sidebar_focus(&kill_tag(0)),
            "an × is not its own Tab stop"
        );
        assert!(
            !is_sidebar_focus("sprag_gui.pane.0"),
            "a pane is not a rail focus"
        );
    }

    /// `↑`/`↓`/`Home`/`End` on the focused tablist rove the keyboard cursor over the session list
    /// (wrapping), and `Enter` switches to the cursor session — reusing [`handle_session_intent`]
    /// (so keyboard and mouse share the one routing SSOT). REVERT-PROOF: a rove that ignored the key
    /// leaves the cursor put; an Enter that did not switch leaves `switched` empty.
    #[test]
    fn sidebar_arrows_rove_the_cursor_and_enter_switches() {
        Owner::new().run(|| {
            let (slots, switched, _created, _killed, _list) =
                recording_slots(&["0", "work", "play"]);
            // Cursor starts unset -> resolves to the first row ("0"). Down -> "work".
            assert!(handle_sidebar_key(
                SESSION_TABLIST_TAG,
                "ArrowDown",
                false,
                &slots
            ));
            assert_eq!(use_session_cursor().get().as_deref(), Some("work"));
            // Down -> "play", then Down WRAPS forward to "0".
            assert!(handle_sidebar_key(
                SESSION_TABLIST_TAG,
                "ArrowDown",
                false,
                &slots
            ));
            assert_eq!(use_session_cursor().get().as_deref(), Some("play"));
            assert!(handle_sidebar_key(
                SESSION_TABLIST_TAG,
                "ArrowDown",
                false,
                &slots
            ));
            assert_eq!(
                use_session_cursor().get().as_deref(),
                Some("0"),
                "wraps forward"
            );
            // Up WRAPS backward to "play".
            assert!(handle_sidebar_key(
                SESSION_TABLIST_TAG,
                "ArrowUp",
                false,
                &slots
            ));
            assert_eq!(
                use_session_cursor().get().as_deref(),
                Some("play"),
                "wraps backward"
            );
            // Home -> first, End -> last.
            assert!(handle_sidebar_key(
                SESSION_TABLIST_TAG,
                "Home",
                false,
                &slots
            ));
            assert_eq!(use_session_cursor().get().as_deref(), Some("0"));
            assert!(handle_sidebar_key(
                SESSION_TABLIST_TAG,
                "End",
                false,
                &slots
            ));
            assert_eq!(use_session_cursor().get().as_deref(), Some("play"));
            // Enter switches to the cursor session by NAME.
            assert!(handle_sidebar_key(
                SESSION_TABLIST_TAG,
                "Enter",
                false,
                &slots
            ));
            assert_eq!(*switched.borrow(), vec!["play".to_owned()]);
        });
    }

    /// `Delete` on the focused tablist ARMS a kill of the cursor session (nothing killed yet); while
    /// pending the tablist is modal — `Enter` CONFIRMS, `Escape` CANCELs, navigation is frozen.
    /// REVERT-PROOF: a Delete that killed immediately makes `killed` non-empty before the confirm;
    /// an Escape that killed, or an Enter that did not, flips the exact-match assertions.
    #[test]
    fn sidebar_delete_arms_and_enter_confirms_escape_cancels() {
        Owner::new().run(|| {
            let (slots, _switched, _created, killed, _list) =
                recording_slots(&["0", "work", "play"]);
            use_session_cursor().set(Some("work".to_owned()));
            // Delete ARMS a kill of the cursor session — but kills nothing yet.
            assert!(handle_sidebar_key(
                SESSION_TABLIST_TAG,
                "Delete",
                false,
                &slots
            ));
            assert_eq!(use_pending_kill().get().as_deref(), Some("work"));
            assert!(killed.borrow().is_empty(), "arming kills nothing");
            // Navigation is FROZEN under the prompt (the arrow is swallowed, the cursor stays put).
            assert!(handle_sidebar_key(
                SESSION_TABLIST_TAG,
                "ArrowDown",
                false,
                &slots
            ));
            assert_eq!(
                use_session_cursor().get().as_deref(),
                Some("work"),
                "nav frozen while pending"
            );
            // Enter CONFIRMS -> kills the captured name and disarms.
            assert!(handle_sidebar_key(
                SESSION_TABLIST_TAG,
                "Enter",
                false,
                &slots
            ));
            assert_eq!(*killed.borrow(), vec!["work".to_owned()]);
            assert_eq!(use_pending_kill().get(), None, "confirm disarmed");
            // Arm again, then Escape CANCELs (kills nothing more).
            use_session_cursor().set(Some("0".to_owned()));
            assert!(handle_sidebar_key(
                SESSION_TABLIST_TAG,
                "Delete",
                false,
                &slots
            ));
            assert_eq!(use_pending_kill().get().as_deref(), Some("0"));
            assert!(handle_sidebar_key(
                SESSION_TABLIST_TAG,
                "Escape",
                false,
                &slots
            ));
            assert_eq!(use_pending_kill().get(), None, "escape disarmed");
            assert_eq!(
                *killed.borrow(),
                vec!["work".to_owned()],
                "cancel killed nothing more"
            );
        });
    }

    /// The footer buttons activate on `Enter` — the "+" creates a session, and (for the AT
    /// activation path, which lowers a Click to `apply_key("Enter")` with the button's tag focused)
    /// the confirm / cancel commit / disarm a pending kill.
    #[test]
    fn sidebar_footer_buttons_activate_on_enter() {
        Owner::new().run(|| {
            let (slots, _switched, created, killed, _list) = recording_slots(&["0", "work"]);
            // "+" creates.
            assert!(handle_sidebar_key(NEW_SESSION_TAG, "Enter", false, &slots));
            assert_eq!(*created.borrow(), 1);
            // Confirm button kills the pending capture.
            use_pending_kill().set(Some("work".to_owned()));
            assert!(handle_sidebar_key(CONFIRM_KILL_TAG, "Enter", false, &slots));
            assert_eq!(*killed.borrow(), vec!["work".to_owned()]);
            // Cancel button disarms.
            use_pending_kill().set(Some("0".to_owned()));
            assert!(handle_sidebar_key(CANCEL_KILL_TAG, "Enter", false, &slots));
            assert_eq!(use_pending_kill().get(), None);
        });
    }

    /// Activation keys are DISCRETE (a held Enter/Delete does not re-fire) while navigation is
    /// CONTINUOUS (a held arrow keeps roving) — the discrete-chord contract the window / session
    /// chords already carry. REVERT-PROOF: dropping the repeat guard makes a held Enter switch.
    #[test]
    fn sidebar_activation_drops_auto_repeat_but_arrows_repeat() {
        Owner::new().run(|| {
            let (slots, switched, _created, _killed, _list) =
                recording_slots(&["0", "work", "play"]);
            use_session_cursor().set(Some("0".to_owned()));
            // A held (auto-repeat) Enter is consumed but does NOT re-switch.
            assert!(handle_sidebar_key(
                SESSION_TABLIST_TAG,
                "Enter",
                true,
                &slots
            ));
            assert!(switched.borrow().is_empty(), "a held Enter does not switch");
            // A held Delete does NOT arm.
            assert!(handle_sidebar_key(
                SESSION_TABLIST_TAG,
                "Delete",
                true,
                &slots
            ));
            assert!(
                use_pending_kill().get().is_none(),
                "a held Delete does not arm"
            );
            // A held arrow keeps roving (continuous).
            assert!(handle_sidebar_key(
                SESSION_TABLIST_TAG,
                "ArrowDown",
                true,
                &slots
            ));
            assert_eq!(
                use_session_cursor().get().as_deref(),
                Some("work"),
                "held arrow roves"
            );
            // The leading press (repeat = false) switches.
            assert!(handle_sidebar_key(
                SESSION_TABLIST_TAG,
                "Enter",
                false,
                &slots
            ));
            assert_eq!(*switched.borrow(), vec!["work".to_owned()]);
        });
    }

    /// The rail's a11y tree is a `tablist` of session `tab`s (posinset/setsize, the attached one
    /// `aria-selected`) followed by the "+" `button`; while the tablist owns focus the cursor tab is
    /// the `aria-activedescendant` ([`AccessNode::with_focused`]), and NONE is when it does not.
    /// REVERT-PROOF: a builder that dropped the cursor's focused flag, mis-set the roles, or omitted
    /// the activedescendant when unfocused flips these assertions.
    #[test]
    fn session_sidebar_access_nodes_expose_a_tablist_of_tabs() {
        Owner::new().run(|| {
            let (slots, ..) = recording_slots(&["0", "work", "play"]);
            // Tablist focused: [TabList, Tab, Tab, Tab, Button("+")].
            let nodes = session_sidebar_access_nodes(&slots, Some(SESSION_TABLIST_TAG));
            assert_eq!(nodes.len(), 5);
            assert_eq!(nodes[0].role, AriaRole::TabList);
            assert_eq!(
                nodes[0].children.len(),
                3,
                "the tablist references every session tab"
            );
            assert!(nodes[1..4].iter().all(|n| n.role == AriaRole::Tab));
            assert_eq!(nodes[1].position_in_set, Some(1), "tab N of M posinset");
            assert_eq!(nodes[1].size_of_set, Some(3));
            assert_eq!(
                nodes[1].selected,
                Some(false),
                "no tab is attached in this fixture"
            );
            assert_eq!(
                nodes[4].role,
                AriaRole::Button,
                "the footer is the '+' button"
            );
            assert_eq!(nodes[4].tag, NEW_SESSION_TAG);
            // Cursor unset -> resolves to the first tab, which is the activedescendant.
            assert!(
                nodes[1].state.focused,
                "the cursor tab is the activedescendant"
            );
            assert!(!nodes[2].state.focused);
            // Moving the cursor moves the activedescendant.
            use_session_cursor().set(Some("play".to_owned()));
            let nodes = session_sidebar_access_nodes(&slots, Some(SESSION_TABLIST_TAG));
            assert!(
                nodes[3].state.focused,
                "the activedescendant follows the cursor"
            );
            assert!(!nodes[1].state.focused);
            // With the tablist UNFOCUSED there is no activedescendant (nor a cursor ring in paint).
            let unfocused = session_sidebar_access_nodes(&slots, Some("sprag_gui.pane.0"));
            assert!(
                unfocused[1..4].iter().all(|n| !n.state.focused),
                "no activedescendant while unfocused",
            );
            // A pending kill swaps the footer "+" for the confirm / cancel buttons.
            use_pending_kill().set(Some("work".to_owned()));
            let pending = session_sidebar_access_nodes(&slots, Some(SESSION_TABLIST_TAG));
            assert!(
                pending
                    .iter()
                    .any(|n| n.tag == CONFIRM_KILL_TAG && n.role == AriaRole::Button)
            );
            assert!(pending.iter().any(|n| n.tag == CANCEL_KILL_TAG));
            assert!(
                pending.iter().all(|n| n.tag != NEW_SESSION_TAG),
                "no '+' while pending"
            );
        });
    }

    /// The keyboard-CURSOR row carries an accent OUTLINE; a non-cursor row does not — the paint
    /// witness the synthetic-key reducer tests cannot see. REVERT-PROOF: drop the `with_border` in
    /// [`row_node`] and the cursor half fails; add it unconditionally and the plain half fails.
    #[test]
    fn a_cursor_row_is_outlined_a_plain_row_is_not() {
        let theme = Theme::default();
        let is_outlined =
            |scene: &Scene| matches!(scene, Scene::Container(c) if c.style.border.is_some());
        let cursor = row_node(0, "work", 1, false, true, None, 0, &theme);
        let plain = row_node(0, "work", 1, false, false, None, 0, &theme);
        assert!(is_outlined(&cursor), "the cursor row is outlined");
        assert!(!is_outlined(&plain), "a non-cursor row is not outlined");
    }
}
