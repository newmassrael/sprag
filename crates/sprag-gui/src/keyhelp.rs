//! The KEY TABLE, as a modal this client can put on the screen — `prefix ?`.
//!
//! [`crate::prompt`] and [`crate::confirm`] are its siblings: three surfaces this client raises over
//! its panes, each armed from a shared module in `sprag-host` that decides what the surface MEANS
//! while the code here decides only how it looks. Here the shared module is
//! [`sprag_host::keyhelp`], which builds the rows out of the keymap in force and answers what each
//! key does to an open view; nothing in this file knows the name of an action or of a scroll key.
//!
//! ## Why this client needed it at all
//!
//! Before R308 the GUI showed a keyboard chord in exactly one place — the palette's hint column —
//! and those were five hardcoded strings, none of them a keymap binding. A user who wanted to know
//! what `prefix C-Left` did had to leave the window and run `sprag list-keys` in a shell. The rival
//! has had a help modal since long before this (`src/ui/keybind_help.rs`, herdr `9a4ce5e1`), which
//! is the honest half; what it does not have is a view that cannot drift from the table, and its
//! own already has (four bindings missing — see [`sprag_host::keyhelp`]).
//!
//! ## Why a modal and not a pane
//!
//! Every other answer this client gives about itself is a modal (the palette, the two prompts), and
//! this is the same kind of thing: a question about the CLIENT, asked while the panes wait. A pane
//! would put the answer inside the arrangement it is describing, and would make "what do the keys
//! do" a thing that resizes somebody's shell.
//!
//! ## The keyboard while it is up
//!
//! Every key is consumed — scroll, or leave, or nothing — which is [`crate::prompt`]'s rule and its
//! reason: the panes are behind a scrim, and a keystroke leaking to a shell the user cannot see is
//! worse than one that is dropped. The decision about WHICH key does which is not made here; it is
//! [`KeyHelp::pressed`], shared with `sprag-tui` so the two clients cannot come to disagree about
//! what `PageDown` means.

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
use sprag_host::keyhelp::{KeyHelp, Pressed, Row, Scroll};

/// The panel's tag: the box an accessible dialog resolves its bounds from, and the modal's single
/// focus-trap member — there is nothing inside to Tab to, so the panel itself is the stop.
pub(crate) const KEYHELP_PANEL_TAG: &str = "sprag_keyhelp_panel";
/// The backdrop's tag — the topmost hit target everywhere except over the panel, so a click beside
/// the panel is SWALLOWED and reaches nothing behind it.
///
/// It does not dismiss, and that is pinion's contract rather than a choice made here: `scrim_backdrop`
/// says light dismiss happens only where a binding attaches an `External` to this tag, and neither
/// this surface nor [`crate::prompt`] does. Written down because the first version of this comment
/// claimed the opposite, having been copied from the prompt's — whose own comment is wrong in the
/// same way and is corrected in place. *A mechanism claim in a durable comment is a claim.*
const KEYHELP_SCRIM_TAG: &str = "sprag_keyhelp#scrim";
/// The modal's introspection tag, answering `open` — so "is this client showing its keys?" has an
/// address a test can read rather than a shape it has to infer from pixels.
pub(crate) const KEYHELP_MODAL_TAG: &str = "sprag_keyhelp_modal";

/// `use_modal` key for the open flag + focus trap.
const KEYHELP_MODAL_KEY: &str = "sprag_gui.keyhelp.modal";
/// `Owner::cache` key for the view being shown.
const KEYHELP_SHOWN_KEY: &str = "sprag_gui.keyhelp.shown";

/// The panel's width in px. Wider than the prompt's and the palette's on purpose: this one holds two
/// columns of text a user reads across, where those hold one field.
const PANEL_W: u32 = 720;
/// One row's height in px.
const ROW_H: u32 = 18;
/// The header's height in px.
const HEADER_H: u32 = 22;
/// The panel's inner padding in px.
const PANEL_PADDING: u32 = 12;
/// The panel's corner radius in px — the prompt's, so the three modals are one object at three
/// sizes rather than three designs.
const PANEL_RADIUS: u32 = 12;
/// The row font size in px.
const ROW_FONT_PX: u32 = 13;
/// The header font size in px.
const HEADER_FONT_PX: u32 = 14;
/// How much of the window's height the panel may take.
const PANEL_MAX_FRACTION: u32 = 4;
/// Roughly how wide one character of the row font is, in px — what the shared chord column's width
/// in CHARACTERS is turned into for this surface.
const CHAR_PX: u32 = 8;
/// The narrowest and widest the chord column may be, in px, so a rebound prefix cannot squeeze the
/// actions off the panel and a short table cannot leave them floating in the middle of it.
const CHORD_COL_MIN_PX: u32 = 72;
/// The widest the chord column may be, in px.
const CHORD_COL_MAX_PX: u32 = 240;

/// The table being shown and where the reader is in it.
///
/// A photograph, for [`KeyHelp`]'s own reason: the rows are built when `?` is pressed and are not
/// rebuilt while the panel is up, so a config edit cannot scroll the view under the reader.
#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct Shown {
    /// The table as it was when the view opened.
    help: KeyHelp,
    /// Where the reader has scrolled to.
    scroll: Scroll,
}

/// The open flag plus the focus trap, moved together.
fn use_keyhelp_modal() -> std::rc::Rc<ModalState> {
    use_modal(KEYHELP_MODAL_KEY)
}

/// The view being shown, or `None` while the panel is closed.
fn use_shown() -> Signal<Option<Shown>> {
    Owner::current()
        .expect("use_shown() requires an active Owner scope")
        .cache(KEYHELP_SHOWN_KEY, || Signal::new(None))
        .as_ref()
        .clone()
}

/// Whether the key table is on the screen.
#[must_use]
pub(crate) fn is_open() -> bool {
    use_keyhelp_modal().is_open()
}

/// Show `help`, from the top.
///
/// It takes the RENDERED view rather than a keymap, because the one thing that may hand it over is
/// [`ClientKeys`](crate::keys::ClientKeys) — that is where the file is re-read, and a borrow of the
/// live table could not outlive the borrow anyway.
pub(crate) fn show(help: KeyHelp) {
    use_shown().set(Some(Shown {
        help,
        scroll: Scroll::default(),
    }));
    use_keyhelp_modal().open(vec![KEYHELP_PANEL_TAG.to_owned()]);
}

/// Put the panes back.
pub(crate) fn close() {
    use_shown().set(None);
    use_keyhelp_modal().close();
}

/// Route a key while the table is up. Returns whether it was consumed — which is ALWAYS, for this
/// module's stated reason.
///
/// What each key MEANS is [`KeyHelp::pressed`]'s, shared with `sprag-tui`. What this adds is the
/// only thing that is this surface's: how many rows are on screen, which the shared arithmetic
/// needs and cannot know.
pub(crate) fn handle_key(key: &str, modifiers: Modifiers, window: (u32, u32)) -> bool {
    let shown = use_shown();
    let Some(current) = shown.get() else {
        // Open with nothing to show cannot happen — `open` sets both — but a key arriving here would
        // otherwise fall through to a pane behind the scrim, so it is swallowed rather than trusted.
        return true;
    };
    match current.help.pressed(
        current.scroll,
        key,
        crate::input::to_input_mods(modifiers),
        viewport(window.1),
    ) {
        Pressed::Open(scroll) => shown.set(Some(Shown { scroll, ..current })),
        Pressed::Closed => close(),
    }
    true
}

/// How many rows of the table are on screen in a window `window_h` px tall.
///
/// The painter's own arithmetic, named once so the key handler and the paint cannot disagree about
/// the size of a page — `sprag-tui` names the same thing `help_viewport` for the same reason.
fn viewport(window_h: u32) -> usize {
    let panel = (window_h / PANEL_MAX_FRACTION * (PANEL_MAX_FRACTION - 1))
        .saturating_sub(HEADER_H + PANEL_PADDING * 2);
    usize::try_from((panel / ROW_H).max(1)).unwrap_or(1)
}

/// The table's Externals — the modal's introspection tag alone, since there is nothing to type into.
///
/// It is registered so that "is the key table up, and is it the thing holding the keyboard?" is a
/// question with an ADDRESS. [`crate::prompt`] has only this too, and the debt register carries the
/// consequence for its line editor; a read-only view has nothing further to publish.
pub(crate) fn create_keyhelp_externals() -> Vec<ExtraExternal> {
    vec![modal_introspection_extra(
        KEYHELP_MODAL_TAG,
        use_keyhelp_modal(),
    )]
}

/// The table's accessible tree, or nothing while it is closed.
///
/// A modal [`AriaRole::Dialog`] whose VALUE is the visible rows as text. The rows are not published
/// as a list of nodes: they are not selectable, not activatable and not focusable, and advertising a
/// listbox for them would be exactly the affordance [`crate::a11y`]'s rule forbids inventing. What a
/// screen reader gets is what a sighted reader gets — the lines that are on the screen.
#[must_use]
pub(crate) fn keyhelp_access_nodes(focused: Option<&str>, window: (u32, u32)) -> Vec<AccessNode> {
    let Some(shown) = use_shown().get() else {
        return Vec::new();
    };
    if !is_open() {
        return Vec::new();
    }
    let lines: Vec<String> = visible(&shown, window)
        .map(|row| row.to_string())
        .filter(|line| !line.is_empty())
        .collect();
    vec![
        AccessNode::new(KEYHELP_PANEL_TAG, AriaRole::Dialog)
            .with_name("what the keys do")
            .with_modal()
            .with_value(AccessValue::Text(lines.join("\n")))
            .with_focused(focused == Some(KEYHELP_PANEL_TAG)),
    ]
}

/// The rows on screen, in order.
fn visible(shown: &Shown, window: (u32, u32)) -> impl Iterator<Item = &Row> {
    let rows = viewport(window.1);
    shown
        .help
        .rows()
        .skip(shown.scroll.offset(shown.help.len(), rows))
        .take(rows)
}

/// The table's paint: a scrim over the window centring a panel of the header and the visible rows —
/// or nothing at all when it is closed.
///
/// Centred like the prompt rather than hung from the top like the palette, and for the palette's own
/// stated reason inverted: a palette's list grows under a fixed field as the user types, so it must
/// not move; this panel's height is decided once when it opens and never changes while it is up.
#[must_use]
pub(crate) fn view_keyhelp(theme: &Theme, window: (u32, u32)) -> Option<Scene> {
    let shown = use_shown().get()?;
    if !is_open() {
        return None;
    }
    let rows = viewport(window.1);
    let mut children = vec![Scene::Text(TextNode::styled(
        header(&shown, rows),
        Rect::default(),
        TextStyle::new()
            .with_size_px(HEADER_FONT_PX)
            .with_fg(theme.resolve(ColorRole::OnSurface)),
    ))];
    // The chord column is as wide as the WIDEST chord in the table, converted from the shared
    // module's characters into this surface's pixels. Measuring per screenful instead would make the
    // column jump as the reader scrolled past the arrows.
    let chord_col = (u32::try_from(shown.help.chord_width()).unwrap_or(8) * CHAR_PX)
        .clamp(CHORD_COL_MIN_PX, CHORD_COL_MAX_PX);
    for row in visible(&shown, window) {
        children.push(view_row(row, chord_col, theme));
    }
    let painted = u32::try_from(children.len()).unwrap_or(1);
    let panel = Scene::Container(
        ContainerNode::new(children)
            .with_tag(KEYHELP_PANEL_TAG)
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
    Some(scrim_backdrop(
        KEYHELP_SCRIM_TAG,
        scrim_fill(M3_SCRIM_ALPHA),
        window,
        FlexDirection::Column,
        AlignItems::Center,
        JustifyContent::Center,
        panel,
    ))
}

/// What the header says: how to leave, and whether there is more.
///
/// The same two facts `sprag-tui`'s header carries, in this client's own words — a reader who cannot
/// scroll must not be told to, and a reader who can must not have to guess.
fn header(shown: &Shown, rows: usize) -> String {
    let mut text = "what the keys do — Esc to close".to_owned();
    if shown.scroll.more_below(shown.help.len(), rows) {
        text.push_str(", PgDn for more");
    } else if shown.scroll.more_above(shown.help.len(), rows) {
        text.push_str(" (end)");
    }
    text
}

/// One row, laid out as this surface lays rows out.
///
/// A binding is TWO COLUMNS here where the terminal client pads one string, and that is the whole of
/// what a pixel surface buys: a proportional font cannot be aligned with spaces, so the chord gets a
/// box of its own and the action starts at the same x on every row whatever is in it.
fn view_row(row: &Row, chord_col: u32, theme: &Theme) -> Scene {
    let text = |content: String, role: ColorRole, size: u32| {
        Scene::Text(TextNode::styled(
            content,
            Rect::default(),
            TextStyle::new()
                .with_size_px(size)
                .with_fg(theme.resolve(role)),
        ))
    };
    match row {
        // A heading is the surface's own voice, so it wears the accent the palette's group rows do.
        Row::Heading(heading) => text(heading.clone(), ColorRole::Accent, ROW_FONT_PX),
        // A blank still occupies its row: the panel's height was decided from the row COUNT, and a
        // skipped separator would shorten the list by one and leave a gap at the bottom.
        Row::Blank => text(String::new(), ColorRole::OnSurface, ROW_FONT_PX),
        Row::Bind {
            chord,
            action,
            repeat,
        } => Scene::Container(
            ContainerNode::new(vec![
                Scene::Container(
                    ContainerNode::new(vec![text(
                        chord.clone(),
                        ColorRole::OnSurface,
                        ROW_FONT_PX,
                    )])
                    .with_layout(LayoutStyle::new().with_size(Size::px(chord_col, ROW_H))),
                ),
                // tmux's `-r` marker, in the muted role: it qualifies the binding rather than
                // naming it, and a reader scanning for a verb must not stop on it.
                text(
                    if *repeat {
                        format!("{} ", KeyHelp::REPEAT)
                    } else {
                        String::new()
                    },
                    ColorRole::OnSurfaceMuted,
                    ROW_FONT_PX,
                ),
                text(action.clone(), ColorRole::OnSurface, ROW_FONT_PX),
            ])
            .with_layout(
                LayoutStyle::new()
                    .flex(FlexDirection::Row)
                    .with_size(Size::px(PANEL_W - PANEL_PADDING * 2, ROW_H)),
            ),
        ),
        // A form nothing reaches is MUTED rather than marked with a glyph: the whole section is a
        // list of things a user could bind, and the ones they already have are the exception worth
        // the eye. The word itself is the shared module's, so both clients say the same thing.
        Row::Vocabulary { form, bound } => {
            if *bound {
                text(form.clone(), ColorRole::OnSurface, ROW_FONT_PX)
            } else {
                text(
                    format!("{form}  ({})", KeyHelp::UNBOUND),
                    ColorRole::OnSurfaceMuted,
                    ROW_FONT_PX,
                )
            }
        }
    }
}
