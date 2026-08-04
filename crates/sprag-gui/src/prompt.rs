//! The NAME PROMPT: the one place this client asks a user to type something at it.
//!
//! [`crate::confirm`] is its sibling — that one asks whether to destroy something and takes a
//! yes/no; this takes a LINE, because the verbs behind it (`rename-window`, `rename-session`,
//! `rename-pane`) need a string a keystroke cannot carry. Both are armed from
//! [`sprag_host::prompt`], which decides WHICH actions ask and what an answer does; what lives here
//! is the surface.
//!
//! ## Why a field of pinion's, and not the shared editor
//!
//! [`sprag_host::prompt::Line`] is a cursor-aware line editor, and `sprag-tui` uses it because a
//! terminal client has no text widget. This client has one — a [`TextFieldExternal`] with a caret,
//! a selection, a clipboard and an IME behind it — and using the shared buffer here would take
//! those away from the user to buy a uniformity nobody experiences. The split is the one
//! [`crate::command`]'s catalog already draws: what must not differ between surfaces belongs to the
//! command, what must differ belongs to the surface. What must not differ is upstream of both — the
//! question, the grammar, and the commit.
//!
//! ## What the field is NOT allowed to decide
//!
//! It holds text. It does not trim it, does not validate it, and does not name the thing being
//! renamed. `Enter` hands what was typed to [`Subject::check`] and then to [`Subject::commit`], and
//! the daemon's answer is what closes the prompt — so this client cannot come to a different
//! opinion about a name than the daemon that stores it. The rival trims client-side in two places
//! and stores whatever is left with no grammar at all (herdr `9a4ce5e1`).
//!
//! ## The refusal STAYS, and so does the text
//!
//! A refused name leaves the prompt open with what the user typed still in it, and the reason
//! painted under the field. Closing on a refusal would make the user retype a name to find out that
//! it is still wrong, and the rival's own empty-name path — `if !new_name.is_empty()` — closes the
//! dialog and silently does nothing at all.

use pinion_a11y::{AccessNode, AccessValue, AriaRole};
use pinion_core::reactive::{Owner, Signal};
use pinion_core::scene::{ContainerNode, Rect, TextNode};
use pinion_core::style::{
    AlignItems, BoxStyle, FlexDirection, JustifyContent, LayoutStyle, Size, TextStyle,
};
use pinion_core::theme::{ColorRole, Theme};
use pinion_core::widget_core::ExtraExternal;
use pinion_core::widgets::caret_blink::use_caret_blink;
use pinion_core::widgets::modal::{ModalState, modal_introspection_extra, use_modal};
use pinion_core::widgets::text_edit::use_text_edit_state;
use pinion_core::widgets::text_field::{TextFieldExternal, TextFieldState};
use pinion_core::{Modifiers, Scene};
use pinion_widget_paint::scrim::{M3_SCRIM_ALPHA, scrim_backdrop, scrim_fill};
use pinion_widget_paint::text_field as tf_paint;
use sprag_host::prompt::Subject;

use crate::terminal::use_terminal;

/// The field's tag — the prompt's only Tab stop, and the modal's single focus-trap member, so a key
/// arriving while the prompt is up belongs to the prompt.
pub(crate) const PROMPT_FIELD_TAG: &str = "sprag_prompt_field";
/// The panel's tag: the box an accessible dialog resolves its bounds from.
const PROMPT_PANEL_TAG: &str = "sprag_prompt_panel";
/// The backdrop's tag. A click beside the panel dismisses — for a prompt that changes a name, light
/// dismiss can only ever cancel, which is the safe direction [`crate::confirm`] states too.
const PROMPT_SCRIM_TAG: &str = "sprag_prompt#scrim";
/// The modal's introspection tag, answering `open`, so "is this client asking for a name?" is a
/// question with an address rather than something a test has to infer from pixels.
const PROMPT_MODAL_TAG: &str = "sprag_prompt_modal";

/// `use_modal` key for the open flag + focus trap.
const PROMPT_MODAL_KEY: &str = "sprag_gui.prompt.modal";
/// `Owner::cache` key for the ARMED subject.
const PROMPT_ARMED_KEY: &str = "sprag_gui.prompt.armed";
/// `Owner::cache` key for the standing refusal.
const PROMPT_REFUSAL_KEY: &str = "sprag_gui.prompt.refusal";

/// The panel's width in px, and the field's height — the palette's numbers, so the two modals of
/// this client are the same object at two sizes rather than two designs.
const PANEL_W: u32 = 520;
/// The field's height in px.
const FIELD_H: u32 = 40;
/// The panel's inner padding in px.
const PANEL_PADDING: u32 = 12;
/// The gap between the question, the field and the refusal, in px.
const ROW_GAP: u32 = 8;
/// The panel's corner radius in px.
const PANEL_RADIUS: u32 = 12;
/// The question's font size in px.
const QUESTION_FONT_PX: u32 = 14;

/// The open flag plus the focus trap, moved together.
fn use_prompt_modal() -> std::rc::Rc<ModalState> {
    use_modal(PROMPT_MODAL_KEY)
}

/// The subject awaiting a name, or `None` when nothing is being asked.
fn use_armed() -> Signal<Option<Subject>> {
    Owner::current()
        .expect("use_armed() requires an active Owner scope")
        .cache(PROMPT_ARMED_KEY, || Signal::new(None))
        .as_ref()
        .clone()
}

/// The last refusal, painted under the field until the next edit.
fn use_refusal() -> Signal<Option<String>> {
    Owner::current()
        .expect("use_refusal() requires an active Owner scope")
        .cache(PROMPT_REFUSAL_KEY, || Signal::new(None))
        .as_ref()
        .clone()
}

/// Whether a name is being asked for.
#[must_use]
pub(crate) fn is_open() -> bool {
    use_prompt_modal().is_open()
}

/// Ask for `subject`'s new name, with `seed` already in the field and the caret at its END.
///
/// The name is AMENDED, not replaced: it is what the thing is already called, so the common edit
/// adds to it or fixes a character. A user starting over has `Ctrl+A` and the mouse, which is what a
/// real field buys over the terminal client's `C-u` — and it is the opposite of the rival, whose
/// `name_input_replace_on_type` destroys the seed on the first keystroke (herdr `9a4ce5e1`).
///
/// **`seed` and not `set_text`, and the pixel smoke is what found the difference.** `set_text`
/// alone clamps the caret to its PREVIOUS offset — zero on a first edit — so the first character
/// typed landed in FRONT of the name and the field held `z0` where `sprag-tui` held `0z`. pinion has
/// the pair as one call (`TextEditState::seed`, R878) for exactly this reason; a client that wrote
/// its own two lines was the ninth site to get it wrong.
pub(crate) fn open(subject: Subject, seed: &str) {
    use_armed().set(Some(subject));
    use_refusal().set(None);
    use_text_edit_state(PROMPT_FIELD_TAG).seed(seed.to_owned());
    use_prompt_modal().open(vec![PROMPT_FIELD_TAG.to_owned()]);
}

/// Drop the ask. The field's text is left alone — the next [`open`] sets it, and clearing here
/// would paint an empty field for the frame between the two.
pub(crate) fn close() {
    use_armed().set(None);
    use_refusal().set(None);
    use_prompt_modal().close();
}

/// Route a key while the prompt is up. Returns whether it was consumed — which is ALWAYS, for the
/// palette's own reason: the panes are behind a modal scrim, and a keystroke leaking to a shell the
/// user cannot see is worse than one that is dropped.
///
/// `Escape` cancels. `Enter` answers: the grammar first (the daemon's own function, so the sentence
/// names the rule), then the daemon, whose refusal has one cause left. Everything else is the
/// field's, delivered through pinion's `forward_key_to_field` so the External stays the authority on
/// its own caret and selection rather than having text written behind its back.
pub(crate) fn handle_key(scene: &mut Scene, key: &str, modifiers: Modifiers) -> bool {
    match key {
        "Escape" => {
            close();
            return true;
        }
        "Enter" => {
            answer();
            return true;
        }
        _ => {}
    }
    let before = text();
    let handled = pinion_core::forward_key_to_field(scene, PROMPT_FIELD_TAG, key, modifiers);
    if handled && text() != before {
        // The standing refusal was about text that no longer exists.
        use_refusal().set(None);
    }
    true
}

/// What is in the field.
fn text() -> String {
    use_text_edit_state(PROMPT_FIELD_TAG).text()
}

/// Answer the prompt with what has been typed, closing it only if the daemon took the name.
fn answer() {
    let Some(subject) = use_armed().get() else {
        return;
    };
    let typed = text();
    let outcome = subject
        .check(&typed)
        .and_then(|()| subject.commit(use_terminal().slots.host(), &typed));
    match outcome {
        Ok(_recorded) => close(),
        Err(why) => use_refusal().set(Some(why)),
    }
}

/// The prompt's Externals, registered every reconcile at their constant tags — the field's holds the
/// text, so it is registered whether or not the prompt is open (the palette's rule, and its reason:
/// an unpainted External costs nothing and re-registering by tag preserves live state).
pub(crate) fn create_prompt_externals() -> Vec<ExtraExternal> {
    vec![
        ExtraExternal::new(
            PROMPT_FIELD_TAG.to_owned(),
            Box::new(
                TextFieldExternal::new()
                    .attach_state(use_text_edit_state(PROMPT_FIELD_TAG))
                    .attach_blink(use_caret_blink(PROMPT_FIELD_TAG)),
            ),
        ),
        modal_introspection_extra(PROMPT_MODAL_TAG, use_prompt_modal()),
    ]
}

/// The prompt's accessible tree, or nothing while it is closed.
///
/// A modal [`AriaRole::Dialog`] whose NAME is the question, holding one [`AriaRole::TextInput`] —
/// a plain text input and not the palette's editable combobox, because there is no list to expand
/// and [`crate::a11y`]'s rule is never to advertise an affordance the widget does not have.
///
/// A standing refusal is folded into the field's NAME, which is what [`crate::confirm`] does with
/// its consequence and for that function's stated reason: the alternative is a description an AT may
/// or may not read, and a user who cannot see the red line under the field would otherwise be told
/// only that their `Enter` did nothing.
#[must_use]
pub(crate) fn prompt_access_nodes(focused: Option<&str>) -> Vec<AccessNode> {
    let Some(subject) = use_armed().get() else {
        return Vec::new();
    };
    if !is_open() {
        return Vec::new();
    }
    let name = match use_refusal().get() {
        Some(why) => format!("{}. {why}", subject.question()),
        None => subject.question().to_owned(),
    };
    let field = AccessNode::new(PROMPT_FIELD_TAG, AriaRole::TextInput)
        .with_name(name)
        .with_value(AccessValue::Text(text()))
        .with_focused(focused == Some(PROMPT_FIELD_TAG));
    vec![
        AccessNode::new(PROMPT_PANEL_TAG, AriaRole::Dialog)
            .with_name(subject.question())
            .with_modal()
            .with_child(PROMPT_FIELD_TAG),
        field,
    ]
}

/// Project the prompt field's posture out of the model scene, exactly as the palette's is.
#[must_use]
pub(crate) fn read_field_state(scene: &Scene) -> (TextFieldState, u32) {
    tf_paint::read_text_field_state(scene, PROMPT_FIELD_TAG)
}

/// The prompt's paint: a scrim over the window centring a panel of the question, the field, and the
/// refusal if one is standing — or nothing at all when it is closed.
///
/// Centred vertically, where the palette hangs from the top: a palette's list grows downward from a
/// fixed field and must not move, and this panel has nothing that grows.
#[must_use]
pub(crate) fn view_prompt(
    state: (TextFieldState, u32),
    theme: &Theme,
    window: (u32, u32),
) -> Option<Scene> {
    let subject = use_armed().get()?;
    if !is_open() {
        return None;
    }
    let style = tf_paint::TextFieldStyle {
        field_w: PANEL_W - PANEL_PADDING * 2,
        field_h: FIELD_H,
        ..tf_paint::TextFieldStyle::m3_filled()
    };
    let mut children = vec![
        Scene::Text(TextNode::styled(
            subject.question().to_owned(),
            Rect::default(),
            TextStyle::new()
                .with_size_px(QUESTION_FONT_PX)
                .with_fg(theme.resolve(ColorRole::OnSurface)),
        )),
        tf_paint::view_field(
            PROMPT_FIELD_TAG,
            state.0,
            state.1,
            theme,
            &style,
            subject.question(),
        ),
    ];
    if let Some(why) = use_refusal().get() {
        // In the error role, under the field it is about — a refusal that read like the question
        // would be indistinguishable from instructions.
        children.push(Scene::Text(TextNode::styled(
            why,
            Rect::default(),
            TextStyle::new()
                .with_size_px(QUESTION_FONT_PX)
                .with_fg(theme.resolve(ColorRole::Error)),
        )));
    }
    let rows = u32::try_from(children.len()).unwrap_or(2);
    let panel = Scene::Container(
        ContainerNode::new(children)
            .with_tag(PROMPT_PANEL_TAG)
            .with_style(
                BoxStyle::filled(theme.resolve(ColorRole::SurfaceContainerHigh))
                    .with_corner_radius(PANEL_RADIUS),
            )
            .with_layout(
                LayoutStyle::new()
                    .flex(FlexDirection::Column)
                    .with_gap(ROW_GAP)
                    .with_padding(Rect::new(
                        PANEL_PADDING,
                        PANEL_PADDING,
                        PANEL_PADDING,
                        PANEL_PADDING,
                    ))
                    .with_size(Size::px(
                        PANEL_W,
                        PANEL_PADDING * 2
                            + FIELD_H
                            + (rows - 1) * ROW_GAP
                            + (rows - 1) * QUESTION_FONT_PX * 2,
                    )),
            ),
    );
    Some(scrim_backdrop(
        PROMPT_SCRIM_TAG,
        scrim_fill(M3_SCRIM_ALPHA),
        window,
        FlexDirection::Column,
        AlignItems::Center,
        JustifyContent::Center,
        panel,
    ))
}
