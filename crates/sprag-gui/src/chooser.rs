//! The CHOOSER, as a modal this client can put on the screen — `prefix s` (R315).
//!
//! The fourth surface this client raises over its panes, beside [`crate::prompt`],
//! [`crate::confirm`] and [`crate::keyhelp`], and armed the same way: a shared module in
//! `sprag-host` decides what it MEANS and the code here decides only how it looks. Here the shared
//! module is [`sprag_host::chooser`], which builds the rows out of the daemon's tree, decides which
//! row a keystroke picks, and carries a pick out. **Nothing in this file knows what a session is.**
//!
//! ## Why it is the key table's shape and not the palette's
//!
//! The palette is the nearer relative on paper — both are a filtered list a person types into — and
//! this is built on [`crate::keyhelp`]'s layout instead, for one reason: the palette's rows are
//! this client's own commands, which it holds statically, and these come off the wire and change
//! while they are on the screen. What that costs is a REFRESH, which the palette has never needed;
//! what it buys is that a chooser open while somebody else makes a session shows the new one.
//!
//! ## The row a person is ON
//!
//! Marked, not just listed. The whole reason a chooser beats a name prompt is orientation: a user
//! who cannot name their other session usually cannot name THIS one either, so a list that did not
//! say where they were standing would answer half the question.
//!
//! ## The keyboard while it is up
//!
//! Every key is consumed — the rule the three siblings share and its reason: the panes are behind a
//! scrim, and a keystroke leaking to a shell the user cannot see is worse than one that is dropped.
//! Which key does what is [`Pick::typed`]'s, shared with `sprag-tui`, so the two clients cannot come
//! to disagree about what `ArrowDown` means.

use pinion_a11y::{AccessNode, AccessValue, AriaRole};
use pinion_core::reactive::{Owner, Signal};
use pinion_core::scene::{ContainerNode, Rect, TextNode};
use pinion_core::style::{
    AlignItems, BoxStyle, FlexDirection, JustifyContent, LayoutStyle, Size, TextStyle,
};
use pinion_core::theme::{ColorRole, Theme};
use pinion_core::widget_core::ExtraExternal;
use pinion_core::widgets::modal::{ModalState, modal_introspection_extra, use_modal};
use pinion_core::{Modifiers, Scene};
use pinion_widget_paint::scrim::{M3_SCRIM_ALPHA, scrim_backdrop, scrim_fill};
use sprag_host::chooser::{Pick, Row};
use sprag_host::prompt::Typed;

use crate::terminal::use_terminal;

/// The panel's tag: the box an accessible dialog resolves its bounds from, and the modal's single
/// focus-trap member — there is nothing inside to Tab to, so the panel itself is the stop.
pub(crate) const CHOOSER_PANEL_TAG: &str = "sprag_chooser_panel";
/// The backdrop's tag — the topmost hit target everywhere except over the panel, so a click beside
/// the panel is SWALLOWED and reaches nothing behind it. It does not dismiss; that is pinion's
/// contract ([`crate::keyhelp`] states it in full).
const CHOOSER_SCRIM_TAG: &str = "sprag_chooser#scrim";
/// The modal's introspection tag, answering `open` — so "is this client choosing?" has an address a
/// test can read rather than a shape it has to infer from pixels.
pub(crate) const CHOOSER_MODAL_TAG: &str = "sprag_chooser_modal";

/// `use_modal` key for the open flag + focus trap.
const CHOOSER_MODAL_KEY: &str = "sprag_gui.chooser.modal";
/// `Owner::cache` key for the open chooser.
const CHOOSER_OPEN_KEY: &str = "sprag_gui.chooser.open";

/// The panel's width in px — the key table's, because both hold rows of text a person reads across.
const PANEL_W: u32 = 720;
/// One row's height in px.
const ROW_H: u32 = 18;
/// The header's height in px.
const HEADER_H: u32 = 22;
/// The panel's inner padding in px.
const PANEL_PADDING: u32 = 12;
/// The panel's corner radius in px — the other three modals', so they are one object at four sizes.
const PANEL_RADIUS: u32 = 12;
/// The row font size in px.
const ROW_FONT_PX: u32 = 13;
/// The header font size in px.
const HEADER_FONT_PX: u32 = 14;
/// How much of the window's height the panel may take.
const PANEL_MAX_FRACTION: u32 = 4;
/// How far one level of the tree is indented, in px. A LAYOUT decision, which is exactly why
/// [`Row::depth`] is a number and not a rendered prefix.
const INDENT_PX: u32 = 18;
/// The marker column's width in px — where the "you are here" dot goes.
const MARKER_COL_PX: u32 = 14;

/// What the open chooser holds: the [`Pick`] itself and the daemon's standing refusal.
///
/// The refusal is HERE rather than inside [`Pick`] for the reason [`crate::prompt`] keeps its own
/// there too: it is a sentence to paint, and how long a sentence stays on a screen is a property of
/// the screen.
#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct Choosing {
    /// The rows, the query and the picked row.
    pick: Pick,
    /// The daemon's refusal — the picked row was gone — standing until the next keystroke.
    refusal: Option<String>,
}

/// The open flag plus the focus trap, moved together.
fn use_chooser_modal() -> std::rc::Rc<ModalState> {
    use_modal(CHOOSER_MODAL_KEY)
}

/// The chooser being shown, or `None` while the panel is closed.
fn use_choosing() -> Signal<Option<Choosing>> {
    Owner::current()
        .expect("use_choosing() requires an active Owner scope")
        .cache(CHOOSER_OPEN_KEY, || Signal::new(None))
        .as_ref()
        .clone()
}

/// Whether the chooser is on the screen.
#[must_use]
pub(crate) fn is_open() -> bool {
    use_chooser_modal().is_open()
}

/// Show `pick`.
///
/// It takes the built chooser rather than a tree, because WHICH rows exist and where the cursor
/// starts are [`Pick::new`]'s decisions and are shared with the other frontend — this surface is
/// handed the answer.
pub(crate) fn show(pick: Pick) {
    use_choosing().set(Some(Choosing {
        pick,
        refusal: None,
    }));
    use_chooser_modal().open(vec![CHOOSER_PANEL_TAG.to_owned()]);
}

/// Put the panes back.
pub(crate) fn close() {
    use_choosing().set(None);
    use_chooser_modal().close();
}

/// Re-read the tree under an open chooser, keeping the cursor on the row the person is looking at.
///
/// Called from the per-frame reconcile, which is what makes the list LIVE: a session another client
/// creates appears, and one that ends goes. The cursor is [`Pick::refresh`]'s to move, and it moves
/// only when its own row is the one that went — the identity claim this whole surface rests on.
///
/// A no-op while the panel is closed, so the frame path pays one signal read for it.
pub(crate) fn refresh() {
    let choosing = use_choosing();
    let Some(mut current) = choosing.get() else {
        return;
    };
    let terminal = use_terminal();
    let host = terminal.slots.host();
    let before = current.pick.clone();
    current.pick.refresh(&host.tree(), &host.current_session());
    // Written back only on a CHANGE, because a `Signal::set` wakes every observer of it and this
    // runs every frame — the same equality skip the window title's own setter states.
    if current.pick != before {
        choosing.set(Some(current));
    }
}

/// Route a key while the chooser is up. Returns whether it was consumed — which is ALWAYS, for this
/// module's stated reason.
///
/// What each key MEANS is [`Pick::typed`]'s and what an answer DOES is [`Pick::commit`]'s, both
/// shared with `sprag-tui`. What this adds is the only part that is the surface's: what happens to
/// the panel afterwards.
pub(crate) fn handle_key(key: &str, modifiers: Modifiers) -> bool {
    let choosing = use_choosing();
    let Some(mut current) = choosing.get() else {
        // Open with nothing to show cannot happen — `show` sets both — but a key arriving here
        // would otherwise fall through to a pane behind the scrim, so it is swallowed.
        return true;
    };
    // ⚠ THE PASTE CHORD, handled HERE because this gate runs BEFORE `route_key`'s clipboard arm —
    // so without it `Ctrl+Shift+V` reached `Line::typed`, matched no chord and was swallowed, and
    // this client's chooser was the only one of the two that could not be pasted into. Found by the
    // debt sweep: `sprag-tui` gets it from the terminal's own paste EVENT, which this client has no
    // counterpart for, and the asymmetry is invisible from either side alone.
    //
    // The pane's own paste is NOT reachable from here and must not be: the panes are behind a
    // scrim, and `route_key`'s arm would send the text to a shell the user cannot see.
    //
    // It asks `clipboard_chord` rather than re-spelling `ctrl && shift && !alt && "v"`, and that is
    // the audit correcting ITSELF: the first version of this block hand-wrote the rule, which is a
    // SECOND copy of the very thing it was added to reach — the shape this sweep is looking for.
    let typed = if matches!(
        crate::input::clipboard_chord(key, modifiers),
        Some(crate::input::ClipboardChord::Paste)
    ) {
        match crate::selection::clipboard().paste_from(pinion_core::ClipboardSelection::Clipboard) {
            Some(text) => current.pick.pasted(&text),
            None => Typed::Ignored,
        }
    } else {
        current
            .pick
            .typed(key, crate::input::to_input_mods(modifiers))
    };
    match typed {
        Typed::Ignored => {}
        Typed::Edited => {
            // The standing refusal was about a row the cursor has left.
            current.refusal = None;
            choosing.set(Some(current));
        }
        Typed::Cancel => close(),
        // The chooser STAYS OPEN on a refusal, exactly as the name prompt does and for its stated
        // reason: a person whose row went while they were reading has lost that row and nothing
        // else, and closing the list would make them press the key again to see what is left.
        Typed::Commit => match current.pick.commit(use_terminal().slots.host()) {
            Ok(()) => close(),
            Err(why) => {
                current.refusal = Some(why);
                choosing.set(Some(current));
            }
        },
    }
    true
}

/// How many rows are on screen in a window `window_h` px tall.
///
/// The painter's own arithmetic, named once so the paint and the accessible tree cannot disagree
/// about what is visible — [`crate::keyhelp`] names the same thing for the same reason.
fn viewport(window_h: u32) -> usize {
    let panel = (window_h / PANEL_MAX_FRACTION * (PANEL_MAX_FRACTION - 1))
        .saturating_sub(HEADER_H + PANEL_PADDING * 2);
    usize::try_from((panel / ROW_H).max(1)).unwrap_or(1)
}

/// The chooser's Externals — the modal's introspection tag alone.
///
/// Registered so that "is this client choosing, and is it the thing holding the keyboard?" is a
/// question with an ADDRESS. The QUERY is not published as a field External: it is edited through
/// [`Pick::typed`] rather than through a pinion text field, which is the deliberate difference from
/// [`crate::prompt`] — see this module's head.
pub(crate) fn create_chooser_externals() -> Vec<ExtraExternal> {
    vec![modal_introspection_extra(
        CHOOSER_MODAL_TAG,
        use_chooser_modal(),
    )]
}

/// The chooser's accessible tree, or nothing while it is closed.
///
/// A modal [`AriaRole::Dialog`] whose VALUE is the visible rows as text, with the picked one named.
/// The rows are not published as a listbox of focusable nodes for [`crate::keyhelp`]'s stated
/// reason inverted — they ARE activatable here — but the affordance a screen reader would then be
/// promised is arrow-key navigation THROUGH the tree, which this surface does not have (the arrows
/// move one selection). Announcing the selection is the honest half; a listbox would be the
/// invented one.
#[must_use]
pub(crate) fn chooser_access_nodes(focused: Option<&str>, window: (u32, u32)) -> Vec<AccessNode> {
    let Some(choosing) = use_choosing().get() else {
        return Vec::new();
    };
    if !is_open() {
        return Vec::new();
    }
    let lines: Vec<String> = visible(&choosing, window)
        .map(|row| {
            format!(
                "{}{}{}",
                if row.target == choosing.pick.cursor() {
                    "> "
                } else {
                    "  "
                },
                row.label,
                if row.here { " (here)" } else { "" },
            )
        })
        .collect();
    vec![
        AccessNode::new(CHOOSER_PANEL_TAG, AriaRole::Dialog)
            .with_name("go to a session, window or pane")
            .with_modal()
            .with_value(AccessValue::Text(lines.join("\n")))
            .with_focused(focused == Some(CHOOSER_PANEL_TAG)),
    ]
}

/// The rows on screen, in order.
///
/// Scrolled to KEEP THE SELECTION VISIBLE rather than to a stored offset, which is `sprag-tui`'s
/// rule and the same argument: there is no second piece of scroll state to go stale when a query
/// moves the cursor twenty rows.
fn visible(choosing: &Choosing, window: (u32, u32)) -> impl Iterator<Item = &Row> {
    let rows = viewport(window.1);
    let visible = choosing.pick.visible();
    let at = choosing.pick.cursor_at().unwrap_or(0);
    let offset = at
        .saturating_sub(rows.saturating_sub(1))
        .max(visible.len().saturating_sub(rows).min(at));
    choosing
        .pick
        .visible()
        .into_iter()
        .skip(offset)
        .take(rows)
        .collect::<Vec<_>>()
        .into_iter()
}

/// The chooser's paint: a scrim over the window centring a panel of the query row and the rows it
/// narrows to — or nothing at all when it is closed.
#[must_use]
pub(crate) fn view_chooser(theme: &Theme, window: (u32, u32)) -> Option<Scene> {
    let choosing = use_choosing().get()?;
    if !is_open() {
        return None;
    }
    let rows = viewport(window.1);
    let mut children = vec![Scene::Text(TextNode::styled(
        header(&choosing),
        Rect::default(),
        TextStyle::new()
            .with_size_px(HEADER_FONT_PX)
            .with_fg(theme.resolve(if choosing.refusal.is_some() {
                ColorRole::Error
            } else {
                ColorRole::OnSurface
            })),
    ))];
    for row in visible(&choosing, window) {
        children.push(view_row(row, row.target == choosing.pick.cursor(), theme));
    }
    let painted = u32::try_from(children.len()).unwrap_or(1);
    let panel = Scene::Container(
        ContainerNode::new(children)
            .with_tag(CHOOSER_PANEL_TAG)
            .with_style(
                BoxStyle::filled(theme.resolve(ColorRole::SurfaceContainerHigh))
                    .with_corner_radius(PANEL_RADIUS),
            )
            .with_layout(
                LayoutStyle::new()
                    .flex(FlexDirection::Column)
                    .with_padding(Rect::new(
                        PANEL_PADDING,
                        PANEL_PADDING,
                        PANEL_PADDING,
                        PANEL_PADDING,
                    ))
                    .with_size(Size::px(
                        PANEL_W,
                        PANEL_PADDING * 2 + HEADER_H + painted.saturating_sub(1) * ROW_H,
                    )),
            ),
    );
    let _ = rows;
    Some(scrim_backdrop(
        CHOOSER_SCRIM_TAG,
        scrim_fill(M3_SCRIM_ALPHA),
        window,
        FlexDirection::Column,
        AlignItems::Center,
        JustifyContent::Center,
        panel,
    ))
}

/// What the header says: what has been typed, and either how to leave or why the last pick failed.
fn header(choosing: &Choosing) -> String {
    let query = choosing.pick.query().text();
    // The ERRAND rather than a literal "go to": since R328 a chooser can be opened to MOVE the
    // focused pane, and two errands painting the same rows under the same words is a person
    // answering a question nobody asked them. `Errand::asking` is the one spelling, shared with the
    // terminal front and with what `bind-key` would take.
    let asking = choosing.pick.errand().asking();
    match &choosing.refusal {
        Some(why) => format!("{asking}: {query}  {why}"),
        None => format!("{asking}: {query}   type to narrow, Enter to commit, Esc to close"),
    }
}

/// One row, laid out as this surface lays rows out.
///
/// THREE columns — the indent, the "here" marker and the text — where the terminal client pads one
/// string, and that is what a pixel surface buys: a proportional font cannot be aligned with spaces.
/// The DETAIL is muted, so a person scanning for a name does not stop on a pane count.
fn view_row(row: &Row, picked: bool, theme: &Theme) -> Scene {
    let text = |content: String, role: ColorRole| {
        Scene::Text(TextNode::styled(
            content,
            Rect::default(),
            TextStyle::new()
                .with_size_px(ROW_FONT_PX)
                .with_fg(theme.resolve(role)),
        ))
    };
    let label = if picked {
        ColorRole::OnSurface
    } else if row.here {
        ColorRole::Accent
    } else {
        ColorRole::OnSurface
    };
    let mut children = Vec::new();
    if row.depth > 0 {
        children.push(Scene::Container(
            ContainerNode::new(Vec::new()).with_layout(
                LayoutStyle::new().with_size(Size::px(INDENT_PX * u32::from(row.depth), ROW_H)),
            ),
        ));
    }
    children.push(Scene::Container(
        ContainerNode::new(vec![text(
            // A dot rather than a word: it marks the row the client is on, and the row's own text
            // is what a person is reading.
            if row.here {
                "•".to_owned()
            } else {
                String::new()
            },
            ColorRole::Accent,
        )])
        .with_layout(LayoutStyle::new().with_size(Size::px(MARKER_COL_PX, ROW_H))),
    ));
    children.push(text(row.label.clone(), label));
    children.push(text(format!("  {}", row.detail), ColorRole::OnSurfaceMuted));
    Scene::Container(
        ContainerNode::new(children)
            .with_style(if picked {
                // The SELECTION is a filled row, which is what a pointer-and-pixels surface has and
                // the terminal one does not. It is the palette's own selected-row treatment, so the
                // two lists this client shows are picked from the same way.
                BoxStyle::filled(theme.resolve(ColorRole::SurfaceContainerHighest))
            } else {
                BoxStyle::default()
            })
            .with_layout(
                LayoutStyle::new()
                    .flex(FlexDirection::Row)
                    .with_size(Size::px(PANEL_W - PANEL_PADDING * 2, ROW_H)),
            ),
    )
}
