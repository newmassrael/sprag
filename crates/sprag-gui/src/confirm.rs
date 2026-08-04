//! The CONFIRMATION: the one place this client asks before it destroys something.
//!
//! A command that cannot be undone is not run when it is activated. It is ARMED — held, with the
//! sentence describing it, until the user answers a modal prompt. The decision about WHICH commands
//! work that way is not here: it is [`Command::confirmation`], beside what each command does, so the
//! catalog stays the one definition of a named command in every respect ([`crate::command`] carries
//! that argument in full). This module owns only the asking.
//!
//! ## One authority, not one confirmation per surface
//!
//! [`run_or_arm`] is the entry EVERY surface uses to activate a command — the palette's `Enter`, a
//! context-menu row, the window strip's "×". None of them calls [`Command::run`] directly any more,
//! and that is the point: a surface added next year inherits the guard by using the same door, rather
//! than by remembering a rule. The alternative, a confirm step built into the palette, would have
//! left the strip's "×" killing a window on one unguarded click — which is exactly what it did, and
//! why this is a module rather than a palette mode.
//!
//! The session rail keeps its OWN confirmation ([`crate::stabs`]) and should: its prompt replaces the
//! rail's footer, ANCHORED under the row that was clicked, which is strictly better than a centred
//! modal for a gesture that started on a specific row. The split is the wording split's twin — the
//! POLICY (what is destructive, what the sentence says) is shared, the SURFACE is not, because an
//! anchored question beats a modal one wherever a surface can anchor it. The palette and the strip
//! cannot: a palette row is read out of context and the strip is a 30px band with nowhere to put a
//! prompt, so for both of them the modal IS the anchored option.
//!
//! ## Why `Enter` does not confirm
//!
//! The safe choice is the DEFAULT one ([`Choice::default`]), and `Enter` activates whatever is
//! chosen — so `Enter` on a fresh prompt cancels. That is not timidity, it is the hazard this whole
//! module exists for: the palette ARMS on `Enter`, so a confirm-on-`Enter` prompt would turn one
//! keystroke too many into a dead session, which is the precise sentence that kept these commands out
//! of the catalog in the first place. Reaching the destructive button takes a deliberate move toward
//! it (`Tab`, `→`), and the layout follows the keyboard: the safe button is left, danger is rightward.
//!
//! The session rail has no such hazard and so needs no such rule — there, `Delete` arms and `Enter`
//! confirms, two different keys, no double-tap path.
//!
//! ## Bounds, stated
//!
//! * **The sentence is CAPTURED at arm time**, not re-derived when it is answered: the user agrees to
//!   what they read. A window that became the session's last one while the prompt was up is therefore
//!   described by the prompt as it was — and killing it still ends the session, which is the outcome
//!   the reconcile below cannot make prettier. What IS handled is the target VANISHING
//!   ([`reconcile`]).
//! * **Name reuse is not closed**, because a name is the only window / session identity on the wire.
//!   [`Command::target_still_exists`] states that bound; it is the session rail's, unchanged.
//! * **A screen reader hears the prompt** ([`confirm_access_nodes`]) as a modal dialog whose NAME
//!   carries the question AND its consequence — see that function for why the consequence is not an
//!   `aria-describedby`. What it does NOT get is a live-region announcement at the moment of arming:
//!   the dialog is announced when focus enters it, which is how the focus trap already behaves, so
//!   there is nothing to add until a surface arms a prompt WITHOUT moving focus.

use std::rc::Rc;

use pinion_a11y::{AccessNode, AriaRole};
use pinion_core::composite_tag::send_activation_key;
use pinion_core::external::{
    Backend, BackendFallback, BackendSupport, External, ExternalIntrospect, InterveneError,
    IntrospectSchema, IntrospectValue, InvokeError, RepaintOwner, SchemaField, ThreadOwnership,
};
use pinion_core::intent::Intent;
use pinion_core::reactive::{Owner, Signal};
use pinion_core::scene::{ContainerNode, Rect, TextNode};
use pinion_core::style::{
    AlignItems, BoxStyle, FlexDirection, JustifyContent, LayoutStyle, Size, TextStyle,
};
use pinion_core::theme::{ColorRole, Theme};
use pinion_core::widget_core::ExtraExternal;
use pinion_core::widgets::modal::{ModalState, modal_introspection_extra, use_modal};
use pinion_core::{Color, Scene};
use pinion_widget_paint::scrim::{M3_SCRIM_ALPHA, scrim_backdrop, scrim_fill};

use sprag_host::keymap::BoundAction;
use sprag_host::prompt::Ask;

use crate::command::{Command, Confirmation};
use crate::slotview::SlotView;
use crate::terminal::use_terminal;

/// This surface's External tag — the prompt's text, the choice, and the two verbs. The buttons and
/// the backdrop paint as COMPOSITES of it (`{CONFIRM_TAG}#{key}`), so every click on the prompt
/// routes back to this one handle, exactly as the palette's rows do.
pub(crate) const CONFIRM_TAG: &str = "sprag_confirm";

/// The panel's tag: the prompt's own box, and the modal's single focus-trap member — so a key
/// arriving while the prompt is up belongs to the prompt.
pub(crate) const CONFIRM_PANEL_TAG: &str = "sprag_confirm_panel";

/// The modal's introspection tag, answering `open` — registered whether or not a prompt is up, so
/// "is something awaiting confirmation?" is a question with an address.
const CONFIRM_MODAL_TAG: &str = "sprag_confirm_modal";

/// The destructive button's composite tag. Named for its ROLE, not its word: the word is
/// [`Confirmation::verb`], data on the command, and a tag that said `#kill` would be a second,
/// silently-diverging opinion about what the button does.
const CONFIRM_ACCEPT_TAG: &str = "sprag_confirm#accept";
/// The safe button's composite tag.
const CONFIRM_DISMISS_TAG: &str = "sprag_confirm#dismiss";
/// The backdrop's composite tag — a click beside the panel dismisses, which for a destructive prompt
/// is the safe direction (light-dismiss can only ever cancel).
const CONFIRM_SCRIM_TAG: &str = "sprag_confirm#scrim";

/// The send key [`CONFIRM_ACCEPT_TAG`] arrives under.
const ACCEPT_KEY: &str = "accept";
/// The send key [`CONFIRM_DISMISS_TAG`] arrives under.
const DISMISS_KEY: &str = "dismiss";
/// The send key [`CONFIRM_SCRIM_TAG`] arrives under.
const SCRIM_KEY: &str = "scrim";

/// The event emitted to PERFORM the armed command. Arrives at the reducer scoped as
/// `{CONFIRM_TAG}.{ACCEPT_EVENT}` (pinion prefixes the emitting external's own tag).
const ACCEPT_EVENT: &str = "accept";
/// The event emitted to discard it — a distinct address rather than a flag on [`ACCEPT_EVENT`], so
/// neither end can confuse "do it" with "do nothing".
const DISMISS_EVENT: &str = "dismiss";

/// `use_modal` key for the prompt's open flag + focus trap.
const CONFIRM_MODAL_KEY: &str = "sprag_gui.confirm.modal";
/// `Owner::cache` key for the ARMED command awaiting an answer.
const CONFIRM_ARMED_KEY: &str = "sprag_gui.confirm.armed";
/// `Owner::cache` key for which button the keyboard is on.
const CONFIRM_CHOICE_KEY: &str = "sprag_gui.confirm.choice";

/// A command held for an answer, with everything needed to perform it and everything needed to
/// describe it — captured together, so the sentence and the act cannot drift apart.
#[derive(Clone, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
struct Armed {
    /// What will run if the answer is yes.
    guarded: Guarded,
    /// The pane the activating surface had captured, threaded through untouched.
    target: Option<usize>,
    /// The words to show, captured when the command was armed (see the module docs).
    confirmation: Confirmation,
}

/// What a yes will actually do — the two vocabularies this ONE surface guards.
///
/// A catalog [`Command`] is what a palette row, a menu row or the strip's "×" activates: this
/// client's own named things. A BOUND ACTION is what a user's `confirm-before` binding names, and
/// that is a different vocabulary with a different author — theirs. Both are destructive questions
/// with two answers, so both belong on this surface; neither can be expressed in the other's terms,
/// which is why this is a sum rather than a translation.
#[derive(Clone, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
enum Guarded {
    /// A command from [`crate::command`]'s catalog.
    Command(Command),
    /// A [`BoundAction`], held as its CANONICAL SPELLING.
    ///
    /// The spelling and not the value, because this record lives in a reactive `Signal` and so must
    /// serialize, while a `BoundAction` carries three types from two crates that deliberately have
    /// no serde. The spelling is not a workaround: round-tripping through
    /// [`Display`](std::fmt::Display) and `parse` is that type's stated contract — it is what
    /// `sprag list-keys` prints and what a user types back — and
    /// `actions_parse_from_the_shells_spelling_and_round_trip` is the test that holds it. A spelling
    /// that failed to parse would mean that contract had broken, so it is REPORTED rather than
    /// unwrapped, and the guarded act simply does not happen.
    Bound(String),
}

/// Which button the keyboard is on.
///
/// [`Choice::Dismiss`] is [`Default`] BY DESIGN, and the derive is load-bearing rather than tidy:
/// every path that arms a prompt or clears one resets to this, so the destructive button is never
/// what an `Enter` finds. (Serde-derived because it is held in a reactive `Signal`, whose value type
/// carries pinion's serialization bound.)
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
enum Choice {
    /// Answer no. The default — see the module docs on why `Enter` must not confirm.
    #[default]
    Dismiss,
    /// Answer yes, and perform the armed command.
    Accept,
}

impl Choice {
    /// The wire / RPC spelling, shared with the composite send keys so a caller reading `choice` and
    /// a caller clicking a button name the same two things.
    fn key(self) -> &'static str {
        match self {
            Self::Dismiss => DISMISS_KEY,
            Self::Accept => ACCEPT_KEY,
        }
    }
}

/// The open flag + focus trap, as one holder — pinion's note on this type is that flipping the flag
/// without moving the trap is the bug it exists to prevent.
fn use_confirm_modal() -> Rc<ModalState> {
    use_modal(CONFIRM_MODAL_KEY)
}

/// The command awaiting an answer, `None` when nothing is.
fn use_armed() -> Signal<Option<Armed>> {
    Owner::current()
        .expect("use_armed() requires an active Owner scope")
        .cache(CONFIRM_ARMED_KEY, || Signal::new(None))
        .as_ref()
        .clone()
}

/// Which button the keyboard is on.
fn use_choice() -> Signal<Choice> {
    Owner::current()
        .expect("use_choice() requires an active Owner scope")
        .cache(CONFIRM_CHOICE_KEY, || Signal::new(Choice::default()))
        .as_ref()
        .clone()
}

/// Whether a prompt is up. Subscribes the caller, so arming / answering repaints.
pub(crate) fn is_open() -> bool {
    use_confirm_modal().is_open()
}

// ─── The guarded entry ───────────────────────────────────────────────────────────────────────────

/// Activate `command` against `target`: run it, or ask first if it is destructive.
///
/// THE door. Every surface comes through here instead of calling [`Command::run`], so "does this need
/// asking?" is answered once, from the command itself, rather than once per surface — see the module
/// docs. A caller does not learn which branch was taken and does not need to: both mean "the user's
/// activation has been dealt with".
pub(crate) fn run_or_arm(command: Command, target: Option<usize>, slots: &SlotView) {
    match command.confirmation(target, slots) {
        Some(confirmation) => arm(Guarded::Command(command), target, confirmation),
        None => command.run(target, slots),
    }
}

/// Hold a `confirm-before` binding's action for an answer, showing the question
/// [`sprag_host::prompt`] built for it (R306).
///
/// The keymap's own door onto this surface, and it does NOT go through
/// [`run_or_arm`]: a bound action is not a catalog command, and the decision to ask was already
/// taken — by the user, when they wrote `confirm-before` in their config. This client's job is to
/// ask, in its own idiom, exactly what the shared ask says.
pub(crate) fn arm_bound(action: &BoundAction, active: usize, ask: &Ask) {
    let Ask::Confirm {
        question,
        consequence,
        verb,
        ..
    } = ask
    else {
        return;
    };
    arm(
        Guarded::Bound(action.to_string()),
        Some(active),
        Confirmation {
            prompt: question.clone(),
            consequence: consequence.clone(),
            verb: (*verb).to_owned(),
        },
    );
}

/// Hold `command` for an answer, showing `confirmation`.
///
/// Resets the choice to the safe one on every arm — a prompt must never open on the button a previous
/// prompt was left on.
fn arm(guarded: Guarded, target: Option<usize>, confirmation: Confirmation) {
    use_armed().set(Some(Armed {
        guarded,
        target,
        confirmation,
    }));
    use_choice().set(Choice::default());
    // The panel is the trap's single member: the two buttons are reached with the arrows / Tab (the
    // dialog keyboard model), so the trap has one Tab stop and every key belongs to the prompt.
    use_confirm_modal().open(vec![CONFIRM_PANEL_TAG.to_owned()]);
}

/// Answer YES: perform the armed command and clear the prompt. Returns what ran, for a caller's
/// report; `None` when nothing was armed.
///
/// CLEARS FIRST, then runs, for the reason the palette's own activation does: a command may move
/// focus, and the focus trap's restore-on-close must not land after that.
fn accept(slots: &SlotView) -> Option<Command> {
    let armed = use_armed().get()?;
    dismiss();
    match armed.guarded {
        Guarded::Command(command) => {
            command.run(armed.target, slots);
            Some(command)
        }
        // A bound action is carried out through the SAME `perform` a bare binding reaches, so a
        // guarded verb and an unguarded one cannot come to behave differently. `None` because there
        // is no catalog command to report: the caller's report is about the catalog.
        Guarded::Bound(spelling) => {
            match (BoundAction::parse(&spelling), armed.target) {
                (Ok(action), Some(active)) => crate::input::perform(action, active),
                // The spelling round-trip is `BoundAction`'s own contract (see [`Guarded::Bound`]),
                // so this is a broken invariant rather than a user mistake — reported, and nothing
                // destructive happens.
                (Err(error), _) => {
                    tracing::error!(target: "sprag_gui::confirm", %spelling, %error, "a guarded action did not parse back");
                }
                (Ok(_), None) => {
                    tracing::error!(target: "sprag_gui::confirm", %spelling, "a guarded action was armed with no pane");
                }
            }
            None
        }
    }
}

/// Answer NO: clear the prompt, having run nothing. Also the auto-disarm and the light-dismiss.
pub(crate) fn dismiss() {
    use_confirm_modal().close();
    use_armed().set(None);
    use_choice().set(Choice::default());
}

/// Perform whichever button the keyboard is on.
fn activate_choice(slots: &SlotView) {
    match use_choice().get() {
        Choice::Accept => {
            accept(slots);
        }
        Choice::Dismiss => dismiss(),
    }
}

/// AUTO-DISARM when the armed command's target has VANISHED — a window or session killed out of band
/// (another client, the `sprag` CLI, its own last pane exiting) while the prompt was up.
///
/// Runs from `reconcile_frame`, pinion's pre-view binding-reconcile hook, for the reason the session
/// rail's equivalent does: the window / session lists live in the host mirror with no `Signal` for an
/// `Effect` to subscribe, so membership is reconciled there, BEFORE the pure view runs, and the view
/// only ever reads an already-consistent capture. The common nothing-armed case reads one signal and
/// returns without touching the host.
///
/// Without this a prompt could linger over something already gone, and its answer would be a benign
/// host no-op — the confirmation equivalent of a dialog for a file that has been deleted.
pub(crate) fn reconcile(slots: &SlotView) {
    if let Some(Armed {
        guarded: Guarded::Command(command),
        target,
        ..
    }) = use_armed().get()
        && !command.target_still_exists(target, slots)
    {
        dismiss();
    }
    // A BOUND action is deliberately not reconciled: it names no target (a keystroke acts where the
    // user is, which is `BoundAction`'s rule at every arm), so there is nothing that can vanish
    // while the prompt is up. The window `kill-window` will kill is whichever one is current when
    // the answer comes, which is the same resolution the rename verbs use and for the same reason.
}

// ─── Keyboard ────────────────────────────────────────────────────────────────────────────────────

/// Route a key while a prompt is up. Returns whether it was consumed.
///
/// Gated on [`is_open`] rather than on a focused tag, unlike the palette's and the find bar's
/// equivalents: those can share the screen with a pane and so must only claim keys while they hold
/// focus, whereas this one is asking whether to destroy something and must claim them all. The router
/// therefore consults this BEFORE it has even resolved what holds focus — an absent focus is exactly
/// the case where a leaked keystroke would reach a shell nobody can see.
///
/// `Escape` answers no, `Enter` / `Space` activate the CHOSEN button (which starts on no — see the
/// module docs), and `Tab` / the arrows move between the two. Every other key is swallowed: this is
/// the innermost modal in the client, and a keystroke leaking to a shell the user cannot see, while
/// they are being asked about destroying something, is the worst version of that bug.
pub(crate) fn handle_key(key: &str) -> bool {
    if !is_open() {
        return false;
    }
    let terminal = use_terminal();
    match key {
        "Escape" => dismiss(),
        "Enter" | "Space" => activate_choice(&terminal.slots),
        "Tab" | "ArrowRight" | "ArrowDown" => use_choice().set(Choice::Accept),
        // Backwards moves AWAY from the destructive button, so it lands on the safe one — there are
        // only two, and clamping toward safety beats wrapping into danger.
        "ArrowLeft" | "ArrowUp" => use_choice().set(Choice::Dismiss),
        _ => {}
    }
    true
}

// ─── The External (the RPC / AI surface) ─────────────────────────────────────────────────────────

/// The prompt's text and its two answers, as an External — so a confirmation is readable and
/// answerable by INTENT rather than by synthesising a click at a pixel, which is what makes the
/// destructive path testable at all.
///
/// Shaped exactly like [`PaletteExternal`](crate::palette): it CAPTURES its `Signal`s at construction
/// (an External's `query` / `invoke` run outside the root `Owner` scope, where a `use_…()` lookup
/// panics) and it EMITS an intent rather than performing anything, because performing reaches
/// `Owner`-scoped state that only the reducer has. Both constraints are pinion's, and both were
/// learned by watching the palette crash rather than by reading them.
struct ConfirmExternal {
    /// The armed command (the same `Signal` [`use_armed`] returns) — read for its captured words.
    armed: Signal<Option<Armed>>,
    /// Which button the keyboard is on (the same `Signal` [`use_choice`] returns).
    choice: Signal<Choice>,
    /// Intents awaiting the shell's drain (the field the emitter contract requires).
    pending_intents: Vec<Intent>,
}

impl ConfirmExternal {
    /// Ask the reducer to perform the armed command.
    fn arm_accept(&mut self) {
        self.pending_intents
            .push(Intent::new_static(ACCEPT_EVENT, IntrospectValue::Null));
    }

    /// Ask the reducer to discard it.
    fn arm_dismiss(&mut self) {
        self.pending_intents
            .push(Intent::new_static(DISMISS_EVENT, IntrospectValue::Null));
    }
}

/// Perform or discard on this surface's own intent, in the reducer's `Owner` scope. Returns whether
/// the intent was this surface's.
pub(crate) fn handle_confirm_intent(intent: &Intent) -> bool {
    let Some((who, event)) = intent.tag_str().rsplit_once('.') else {
        return false;
    };
    if who != CONFIRM_TAG {
        return false;
    }
    match event {
        ACCEPT_EVENT => {
            let terminal = use_terminal();
            accept(&terminal.slots);
            true
        }
        DISMISS_EVENT => {
            dismiss();
            true
        }
        _ => false,
    }
}

impl core::fmt::Debug for ConfirmExternal {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ConfirmExternal")
            .field("armed", &self.armed.get().is_some())
            .finish_non_exhaustive()
    }
}

// NOT `query_proxy_external_impl!`: that macro is for an External that emits NOTHING, and would leave
// `drain_intents` at its no-op default — silently swallowing every answer. This one emits, so it
// declares the descriptors by hand plus the drain, as the palette's does.
impl External for ConfirmExternal {
    fn backends(&self) -> BackendSupport {
        BackendSupport::new(&[Backend::Gui, Backend::Rpc], BackendFallback::Skip)
    }
    fn repaint_ownership(&self) -> RepaintOwner {
        RepaintOwner::Framework
    }
    fn thread_ownership(&self) -> ThreadOwnership {
        ThreadOwnership::UiThreadSync
    }
    fn introspect(&self) -> Option<&dyn ExternalIntrospect> {
        Some(self)
    }
    fn introspect_mut(&mut self) -> Option<&mut dyn ExternalIntrospect> {
        Some(self)
    }
    fn drain_intents(&mut self, sink: &mut dyn FnMut(Intent)) {
        for intent in self.pending_intents.drain(..) {
            sink(intent);
        }
    }
    /// Whether an answer is waiting to be performed.
    ///
    /// NOT optional, and not an optimisation. pinion's runtime harvests intents through
    /// `drain_one`, which returns EARLY unless this says yes — so an emitting External that keeps the
    /// trait default (`false`) queues answers the reducer is never handed, and the surface silently
    /// does nothing while every one of its own unit tests passes. That is the live defect this front's
    /// pixel smoke found, here and in the palette; `intent_query_external_impl!` exists precisely to
    /// implement this pair for you, and cannot be used here only because it declares an RPC-ONLY
    /// backend while both these surfaces are also clicked in the GUI.
    fn is_dirty(&self) -> bool {
        !self.pending_intents.is_empty()
    }
}

impl ExternalIntrospect for ConfirmExternal {
    fn schema(&self) -> IntrospectSchema {
        IntrospectSchema::new(
            const {
                &[
                    SchemaField::new("prompt", "string"),
                    SchemaField::new("consequence", "string"),
                    SchemaField::new("verb", "string"),
                    SchemaField::new("choice", "string"),
                    SchemaField::new("accept", "json"),
                    SchemaField::new("dismiss", "json"),
                    SchemaField::new("send", "string"),
                ]
            },
        )
    }

    fn query(&self, path: &str) -> Option<IntrospectValue> {
        let armed = self.armed.get();
        match path {
            // Present-but-empty: the paths always resolve, and nothing armed reports Null rather
            // than an unknown-path error (no prompt is a state, not a mistake).
            "prompt" => Some(text_or_null(
                armed.map(|armed| armed.confirmation.prompt.clone()),
            )),
            "consequence" => Some(text_or_null(
                armed.and_then(|armed| armed.confirmation.consequence.clone()),
            )),
            "verb" => Some(text_or_null(
                armed.map(|armed| armed.confirmation.verb.clone()),
            )),
            "choice" => Some(IntrospectValue::Text(self.choice.get().key().to_owned())),
            _ => None,
        }
    }

    fn intervene(&mut self, path: &str, value: IntrospectValue) -> Result<(), InterveneError> {
        match path {
            // The choice MOVES — the keyboard's own two moves, reachable over RPC — so a driver can
            // put the prompt into the state a screenshot should show without answering it. Writable
            // where the sentence is not: moving the cursor is not agreeing to anything.
            "choice" => match value {
                IntrospectValue::Text(ref key) => match key.as_str() {
                    ACCEPT_KEY => {
                        self.choice.set(Choice::Accept);
                        Ok(())
                    }
                    DISMISS_KEY => {
                        self.choice.set(Choice::Dismiss);
                        Ok(())
                    }
                    _ => Err(InterveneError::OutOfRange),
                },
                _ => Err(InterveneError::TypeMismatch),
            },
            // The captured sentence is the user's to read, never a caller's to rewrite: a prompt whose
            // words could be replaced from outside would be a confirmation of nothing.
            "prompt" | "consequence" | "verb" => Err(InterveneError::ReadOnly),
            _ => Err(InterveneError::UnknownPath),
        }
    }

    fn invoke(
        &mut self,
        path: &str,
        args: IntrospectValue,
    ) -> Result<IntrospectValue, InvokeError> {
        match path {
            // The two answers, each its own verb: an RPC caller (or the headless smoke) says which
            // one it means rather than moving a cursor and pressing a key.
            "accept" => {
                self.arm_accept();
                Ok(IntrospectValue::Bool(true))
            }
            "dismiss" => {
                self.arm_dismiss();
                Ok(IntrospectValue::Bool(true))
            }
            // The composite-send wire: a click on either button, or on the backdrop beside the panel.
            "send" => match args {
                IntrospectValue::Text(ref payload) => {
                    match send_activation_key(payload) {
                        Some(ACCEPT_KEY) => self.arm_accept(),
                        // The backdrop shares this handle: a click outside the panel answers NO,
                        // which is the only direction a light-dismiss may ever mean.
                        Some(DISMISS_KEY | SCRIM_KEY) => self.arm_dismiss(),
                        _ => {}
                    }
                    Ok(IntrospectValue::Bool(true))
                }
                _ => Err(InvokeError::TypeMismatch),
            },
            _ => Err(InvokeError::UnknownPath),
        }
    }
}

/// `Some(text)` as an introspected string, `None` as Null.
fn text_or_null(text: Option<String>) -> IntrospectValue {
    match text {
        Some(text) => IntrospectValue::Text(text),
        None => IntrospectValue::Null,
    }
}

/// This surface's Externals, registered every reconcile at their constant tags (pinion preserves a
/// surviving external's live state by tag). Registered whether or not a prompt is up — an unpainted
/// External costs nothing, and `open` stays answerable either way.
pub(crate) fn create_confirm_externals() -> Vec<ExtraExternal> {
    vec![
        ExtraExternal::new(
            CONFIRM_TAG.to_owned(),
            Box::new(ConfirmExternal {
                armed: use_armed(),
                choice: use_choice(),
                pending_intents: Vec::new(),
            }),
        ),
        modal_introspection_extra(CONFIRM_MODAL_TAG, use_confirm_modal()),
    ]
}

// ─── Accessibility ───────────────────────────────────────────────────────────────────────────────

/// The prompt's accessible tree, or nothing at all while nothing is armed.
///
/// A MODAL [`AriaRole::Dialog`] holding its two answers — `[dialog, button, button]`, the flat
/// `[parent, ...children]` list the session rail's builder returns, bounds left `None` for the shell to
/// resolve from each painted tag.
///
/// ## The consequence goes in the NAME, not a description
///
/// pinion offers [`describedby_region`](pinion_a11y::describedby_region) for an auxiliary description,
/// and it is the wrong tool here. `aria-describedby` is announced at the AT's discretion — verbosity
/// settings, mode, and how the user arrived all decide whether a description is read — and the
/// consequence line is the one sentence that CHANGES the answer ("this ends the session", "this client
/// detaches"). A destructive prompt may not leave that to a setting, so both sentences are the dialog's
/// accessible NAME, which is announced when the dialog takes focus. The visual split (question in the
/// surface ink, consequence in the error role) is preserved for a sighted reader; what an AT gets is one
/// sentence with nothing optional in it.
///
/// A second reason to avoid the description path: it would need the consequence line to be a TAGGED
/// painted node so the reference resolves, and a dangling `aria-describedby` — the state this prompt is
/// in whenever there is no consequence — is an AT defect rather than a style choice (that helper's own
/// docs say so). Folding it into the name has no absent case to get wrong.
///
/// The chosen button is marked focused: the panel owns real keyboard focus and the choice roves within
/// it, so the choice is this dialog's active descendant, expressed exactly as the rail expresses its
/// cursor row.
pub(crate) fn confirm_access_nodes(focused: Option<&str>) -> Vec<AccessNode> {
    let Some(armed) = use_armed().get() else {
        return Vec::new();
    };
    let choice = use_choice().get();
    let panel_has_focus = focused == Some(CONFIRM_PANEL_TAG);
    let words = &armed.confirmation;
    let name = match words.consequence.as_deref() {
        Some(consequence) => format!("{} {consequence}", words.prompt),
        None => words.prompt.clone(),
    };
    vec![
        AccessNode::new(CONFIRM_PANEL_TAG, AriaRole::Dialog)
            .with_name(name)
            .with_modal()
            .with_child(CONFIRM_DISMISS_TAG)
            .with_child(CONFIRM_ACCEPT_TAG),
        // Safe answer FIRST, as it is painted and as the keyboard reaches it — an AT walking the
        // children in order meets the way out before the way through.
        AccessNode::new(CONFIRM_DISMISS_TAG, AriaRole::Button)
            .with_name(DISMISS_LABEL)
            .with_focused(panel_has_focus && choice == Choice::Dismiss),
        AccessNode::new(CONFIRM_ACCEPT_TAG, AriaRole::Button)
            .with_name(words.verb.clone())
            .with_focused(panel_has_focus && choice == Choice::Accept),
    ]
}

// ─── Paint ───────────────────────────────────────────────────────────────────────────────────────

/// The prompt: a scrim over the whole window centring a panel of the question, its consequence, and
/// the two answers — or nothing at all when nothing is armed.
///
/// Centred, unlike the palette's top-hung panel: there is nothing to type and no list to grow, and a
/// question about destroying something should be where the eye already is.
pub(crate) fn view_confirm(theme: &Theme, window: (u32, u32)) -> Option<Scene> {
    let armed = use_armed().get()?;
    let choice = use_choice().get();
    let words = &armed.confirmation;

    let mut children: Vec<Scene> = Vec::with_capacity(3);
    children.push(text_line(
        &words.prompt,
        PROMPT_FONT_PX,
        theme.resolve(ColorRole::OnSurface),
    ));
    // The consequence, when the name does not already imply it, in the ERROR role — this is the line
    // that changes an answer, so it must not read as a subtitle.
    if let Some(consequence) = words.consequence.as_deref() {
        children.push(text_line(
            consequence,
            CONSEQUENCE_FONT_PX,
            theme.resolve(ColorRole::Error),
        ));
    }
    // Safe LEFT, destructive RIGHT: the layout mirrors the keyboard, where the arrows move toward
    // danger and away from it (see the module docs).
    children.push(Scene::Container(
        ContainerNode::new(vec![
            answer_button(
                CONFIRM_DISMISS_TAG,
                DISMISS_LABEL,
                choice == Choice::Dismiss,
                theme.resolve(ColorRole::OnSurface),
                theme,
            ),
            answer_button(
                CONFIRM_ACCEPT_TAG,
                &words.verb,
                choice == Choice::Accept,
                theme.resolve(ColorRole::Error),
                theme,
            ),
        ])
        .with_layout(
            LayoutStyle::new()
                .flex(FlexDirection::Row)
                .with_align_items(AlignItems::Center)
                .with_justify(JustifyContent::End)
                .with_gap(BUTTON_GAP)
                .with_size(Size::px(PANEL_W - PANEL_PADDING * 2, BUTTON_H)),
        ),
    ));

    let panel = Scene::Container(
        ContainerNode::new(children)
            .with_tag(CONFIRM_PANEL_TAG)
            .with_style(
                BoxStyle::filled(theme.resolve(ColorRole::SurfaceContainerHigh))
                    .with_corner_radius(PANEL_RADIUS),
            )
            .with_layout(
                LayoutStyle::new()
                    .flex(FlexDirection::Column)
                    .with_align_items(AlignItems::Start)
                    .with_gap(ROW_GAP)
                    .with_padding(Rect::new(
                        PANEL_PADDING,
                        PANEL_PADDING,
                        PANEL_PADDING,
                        PANEL_PADDING,
                    ))
                    .with_size(Size::px(PANEL_W, panel_height(words)))
                    // The modal's single Tab stop, so a key delivered while the prompt is up routes
                    // to `handle_key` rather than to a pane behind the scrim.
                    .with_focusable(true),
            ),
    );

    Some(scrim_backdrop(
        CONFIRM_SCRIM_TAG,
        scrim_fill(M3_SCRIM_ALPHA),
        window,
        FlexDirection::Column,
        AlignItems::Center,
        JustifyContent::Center,
        panel,
    ))
}

/// One answer button: its word, filled when the keyboard is on it.
///
/// The chosen one is FILLED rather than outlined for the palette cursor's reason (a fill reads at a
/// glance), and this is the affordance carrying the whole default-to-safe design — so the fill is
/// what a test asserts on.
fn answer_button(tag: &str, label: &str, chosen: bool, ink: Color, theme: &Theme) -> Scene {
    let fill = if chosen {
        BoxStyle::filled(theme.resolve(ColorRole::SurfaceContainerHighest))
            .with_corner_radius(BUTTON_RADIUS)
    } else {
        BoxStyle::default()
    };
    Scene::Container(
        ContainerNode::new(vec![text_line(label, BUTTON_FONT_PX, ink)])
            .with_tag(tag.to_owned())
            .with_style(fill)
            .with_layout(
                LayoutStyle::new()
                    .flex(FlexDirection::Row)
                    .with_align_items(AlignItems::Center)
                    .with_justify(JustifyContent::Center)
                    .with_padding(Rect::new(BUTTON_PADDING, 0, BUTTON_PADDING, 0))
                    .with_size(
                        Size::auto().with_height(pinion_core::style::SizeValue::Px(BUTTON_H)),
                    ),
            ),
    )
}

/// A single text line at `px` logical size in `ink`.
fn text_line(label: &str, px: u32, ink: Color) -> Scene {
    Scene::Text(TextNode::styled(
        label.to_owned(),
        Rect::default(),
        TextStyle::new().with_size_px(px).with_fg(ink),
    ))
}

/// The panel's height for `words`: the question, the answers, and the consequence line only when
/// there is one — sized to the CONTENT so a prompt with nothing extra to say is not a tall box with a
/// gap in it.
fn panel_height(words: &Confirmation) -> u32 {
    let consequence = if words.consequence.is_some() {
        CONSEQUENCE_H + ROW_GAP
    } else {
        0
    };
    PANEL_PADDING * 2 + PROMPT_H + ROW_GAP + consequence + BUTTON_H
}

/// The panel's width in logical pixels — wide enough for the longest consequence sentence on one
/// line, and narrower than the palette's list so the two never read as the same surface.
const PANEL_W: u32 = 420;
/// The panel's inner padding on every edge.
const PANEL_PADDING: u32 = 16;
/// The panel's corner radius (the palette's, so the client has one dialog shape).
const PANEL_RADIUS: u32 = 12;
/// The question line's reserved height.
const PROMPT_H: u32 = 22;
/// The consequence line's reserved height.
const CONSEQUENCE_H: u32 = 18;
/// The gap between the panel's stacked lines.
const ROW_GAP: u32 = 8;
/// An answer button's height.
const BUTTON_H: u32 = 32;
/// The gap between the two answer buttons.
const BUTTON_GAP: u32 = 8;
/// An answer button's horizontal padding.
const BUTTON_PADDING: u32 = 16;
/// An answer button's fill corner radius.
const BUTTON_RADIUS: u32 = 6;
/// The question's font size — the largest text on the panel, because it is the sentence being agreed
/// to.
const PROMPT_FONT_PX: u32 = 15;
/// The consequence line's font size.
const CONSEQUENCE_FONT_PX: u32 = 12;
/// An answer button's font size.
const BUTTON_FONT_PX: u32 = 13;

/// The safe answer's word. A plain "Cancel" against the command's own destructive verb, so the two
/// buttons can never both read as actions.
const DISMISS_LABEL: &str = "Cancel";

#[cfg(test)]
mod tests {
    use sprag_host::Host;
    use sprag_terminal::CommandBuilder;

    use pinion_a11y::AriaRole;

    use super::*;
    use crate::command::catalog;
    use crate::terminal::seed_terminal;

    /// A long-lived `cat` pane (holds its PTY open across the drive), the deterministic pane the
    /// other reducer tests seed.
    fn cat() -> CommandBuilder {
        let mut c = CommandBuilder::new("/bin/sh");
        c.arg("-c");
        c.arg("cat");
        c.env("TERM", "dumb");
        c
    }

    /// A one-pane in-process host, seeded so `use_terminal()` answers.
    fn seed_one_pane() {
        let host = Host::new((40, 6));
        host.spawn(cat(), "cat".to_owned(), 40, 6, None, None)
            .unwrap();
        seed_terminal(host);
    }

    /// The External as [`create_confirm_externals`] builds it — same captured handles, so a test
    /// drives the real thing rather than a look-alike.
    fn external() -> ConfirmExternal {
        ConfirmExternal {
            armed: use_armed(),
            choice: use_choice(),
            pending_intents: Vec::new(),
        }
    }

    /// Drain `external`'s emitted intents through the reducer hook, which is what the shell does —
    /// the step that actually performs an accepted command. Returns how many this surface claimed.
    ///
    /// The tag is SCOPED here (`{CONFIRM_TAG}.{event}`) because an external pushes only its event
    /// name and pinion prefixes the emitting external's tag on the way to the reducer.
    ///
    /// The [`is_dirty`](External::is_dirty) gate is reproduced FIRST because the runtime applies it
    /// first (`drain_one` returns early on a clean external), and a test that drains unconditionally
    /// is exactly how a missing `is_dirty` stayed invisible until a live smoke caught it. Draining
    /// through the gate means these tests fail if it ever goes away again.
    fn drain_into_reducer(external: &mut ConfirmExternal) -> usize {
        let mut intents = Vec::new();
        if external.is_dirty() {
            external.drain_intents(&mut |intent| intents.push(intent));
        }
        intents
            .iter()
            .map(|intent| Intent {
                tag: std::borrow::Cow::Owned(format!("{CONFIRM_TAG}.{}", intent.tag_str())),
                payload: intent.payload.clone(),
            })
            .filter(handle_confirm_intent)
            .count()
    }

    /// Every painted string in `scene`, in tree order.
    fn walk_text(scene: &Scene) -> Vec<String> {
        let mut out = Vec::new();
        collect(scene, &mut out, &mut Vec::new());
        out
    }

    /// Every tag in `scene`.
    fn walk_tags(scene: &Scene) -> Vec<String> {
        let mut tags = Vec::new();
        collect(scene, &mut Vec::new(), &mut tags);
        tags
    }

    fn collect(scene: &Scene, text: &mut Vec<String>, tags: &mut Vec<String>) {
        match scene {
            Scene::Text(node) => text.push(node.content.clone()),
            Scene::Container(node) => {
                if let Some(tag) = node.tag.as_ref() {
                    tags.push(tag.to_string());
                }
                for child in &node.children {
                    collect(child, text, tags);
                }
            }
            _ => {}
        }
    }

    /// Whether the button at `tag` is painted FILLED — the affordance that says which answer the
    /// keyboard is on.
    fn button_is_filled(scene: &Scene, tag: &str) -> bool {
        match scene {
            Scene::Container(node) => {
                if node.tag.as_deref() == Some(tag) {
                    return node.style != BoxStyle::default();
                }
                node.children
                    .iter()
                    .any(|child| button_is_filled(child, tag))
            }
            _ => false,
        }
    }

    #[test]
    fn the_composite_tags_are_all_composites_of_the_one_external_tag() {
        // Every click on this surface reaches its External only through the router's `#`-split, so
        // the halves must stay in step. REVERT-PROOF: renaming any constant alone fails here rather
        // than silently producing a button whose click goes nowhere.
        for (tag, expected) in [
            (CONFIRM_ACCEPT_TAG, ACCEPT_KEY),
            (CONFIRM_DISMISS_TAG, DISMISS_KEY),
            (CONFIRM_SCRIM_TAG, SCRIM_KEY),
        ] {
            let (base, key) = tag.split_once('#').expect("a composite tag");
            assert_eq!(base, CONFIRM_TAG);
            assert_eq!(key, expected);
        }
    }

    #[test]
    fn a_safe_command_runs_immediately_and_arms_nothing() {
        Owner::new().run(|| {
            seed_one_pane();
            let terminal = use_terminal();
            assert_eq!(terminal.slots.windows().len(), 1);

            run_or_arm(Command::NewWindow, Some(0), &terminal.slots);

            assert!(!is_open(), "a command that needs no asking is not held up");
            assert_eq!(
                use_terminal().slots.windows().len(),
                2,
                "it ran, through the same door a destructive one is stopped at"
            );
        });
    }

    /// THE guard: a destructive command activated through the shared door does NOT act. It is held,
    /// with the sentence describing it, until an answer arrives.
    ///
    /// REVERT-PROOF: make `run_or_arm` call `Command::run` unconditionally and the window is gone
    /// before anyone was asked — this fails on both the count and `is_open`.
    #[test]
    fn a_destructive_command_is_held_for_an_answer_instead_of_run() {
        Owner::new().run(|| {
            seed_one_pane();
            let terminal = use_terminal();
            let victim = terminal.slots.new_window();
            let before = terminal.slots.windows().len();
            assert!(before > 1, "two windows, so a kill is observable");

            run_or_arm(Command::KillWindow(victim.clone()), None, &terminal.slots);

            assert!(is_open(), "the prompt is up");
            assert_eq!(
                use_terminal().slots.windows().len(),
                before,
                "and NOTHING has been killed yet"
            );
            assert_eq!(
                use_choice().get(),
                Choice::Dismiss,
                "a fresh prompt opens on the SAFE answer, so a reflex Enter cancels"
            );
        });
    }

    /// ...and answering yes performs it, through the same `Command::run` every other surface reaches.
    ///
    /// REVERT-PROOF: drop the `accept` call from `activate_choice` and the window survives.
    #[test]
    fn answering_yes_performs_the_held_command() {
        Owner::new().run(|| {
            seed_one_pane();
            let terminal = use_terminal();
            let victim = terminal.slots.new_window();
            let before = terminal.slots.windows().len();

            run_or_arm(Command::KillWindow(victim.clone()), None, &terminal.slots);
            // Move to the destructive button deliberately, as a user must.
            assert!(handle_key("ArrowRight"));
            assert_eq!(use_choice().get(), Choice::Accept);
            assert!(handle_key("Enter"));

            assert!(!is_open(), "answering clears the prompt");
            let after = use_terminal().slots.windows();
            assert_eq!(after.len(), before - 1, "the window is gone");
            assert!(
                !after.iter().any(|window| window.name == victim),
                "and it is the one the prompt NAMED that went: {after:?}"
            );
        });
    }

    /// The hazard this module exists for: `Enter` on a fresh prompt must CANCEL, because the palette
    /// arms on `Enter` and a double-tap would otherwise destroy something.
    ///
    /// REVERT-PROOF: default `Choice` to `Accept` (or make `Enter` confirm outright) and this fails
    /// with the window killed by a keystroke nobody aimed.
    #[test]
    fn a_reflex_enter_on_a_fresh_prompt_cancels_rather_than_confirming() {
        Owner::new().run(|| {
            seed_one_pane();
            let terminal = use_terminal();
            let victim = terminal.slots.new_window();
            let before = terminal.slots.windows().len();

            run_or_arm(Command::KillWindow(victim), None, &terminal.slots);
            assert!(handle_key("Enter"), "the prompt consumes the key");

            assert!(!is_open(), "the prompt is answered and gone");
            assert_eq!(
                use_terminal().slots.windows().len(),
                before,
                "and the answer was NO — nothing was killed"
            );
        });
    }

    #[test]
    fn escape_and_the_backdrop_both_answer_no() {
        Owner::new().run(|| {
            seed_one_pane();
            let terminal = use_terminal();
            let victim = terminal.slots.new_window();
            let before = terminal.slots.windows().len();

            run_or_arm(Command::KillWindow(victim.clone()), None, &terminal.slots);
            assert!(handle_key("Escape"));
            assert!(!is_open());
            assert_eq!(use_terminal().slots.windows().len(), before);

            // ...and a click beside the panel, which reaches the same External through the scrim's
            // composite tag.
            run_or_arm(Command::KillWindow(victim), None, &terminal.slots);
            let mut external = external();
            external
                .invoke(
                    "send",
                    IntrospectValue::Text(format!("{SCRIM_KEY}:PointerUp")),
                )
                .expect("the backdrop's send is accepted");
            assert_eq!(drain_into_reducer(&mut external), 1);
            assert!(!is_open(), "a light-dismiss answers no");
            assert_eq!(
                use_terminal().slots.windows().len(),
                before,
                "a light-dismiss can only ever cancel"
            );
        });
    }

    /// A click on the destructive button arms an intent; the REDUCER performs it. Asserting the
    /// two-step is the point — the External cannot reach the command's own state.
    ///
    /// REVERT-PROOF: neuter the `accept` arm of `handle_confirm_intent` and the window survives the
    /// drain.
    #[test]
    fn a_click_on_the_destructive_button_is_performed_by_the_reducer() {
        Owner::new().run(|| {
            seed_one_pane();
            let terminal = use_terminal();
            let victim = terminal.slots.new_window();
            let before = terminal.slots.windows().len();

            run_or_arm(Command::KillWindow(victim), None, &terminal.slots);
            let mut external = external();
            external
                .invoke(
                    "send",
                    IntrospectValue::Text(format!("{ACCEPT_KEY}:PointerUp")),
                )
                .expect("the button's send is accepted");
            assert_eq!(
                use_terminal().slots.windows().len(),
                before,
                "the External itself performs nothing"
            );

            assert_eq!(drain_into_reducer(&mut external), 1);
            assert_eq!(
                use_terminal().slots.windows().len(),
                before - 1,
                "the reducer performed the answer"
            );
        });
    }

    /// The prompt is readable and answerable over RPC — the drive-by-intent surface a headless smoke
    /// needs, since a modal that can only be driven by synthesised pixels is untestable by
    /// construction.
    #[test]
    fn the_external_reports_the_captured_sentence_and_the_default_choice() {
        Owner::new().run(|| {
            seed_one_pane();
            let terminal = use_terminal();
            let victim = terminal.slots.new_window();
            run_or_arm(Command::KillWindow(victim.clone()), None, &terminal.slots);

            let mut external = external();
            let prompt = match external.query("prompt") {
                Some(IntrospectValue::Text(text)) => text,
                other => panic!("the prompt reads as text, got {other:?}"),
            };
            assert!(
                prompt.contains(&victim),
                "the question names what will be destroyed: {prompt}"
            );
            assert_eq!(
                external.query("verb"),
                Some(IntrospectValue::Text("Kill".to_owned())),
                "the button's word comes from the command, not from this surface"
            );
            assert_eq!(
                external.query("choice"),
                Some(IntrospectValue::Text(DISMISS_KEY.to_owned())),
                "and the default answer is the safe one"
            );

            external
                .invoke("dismiss", IntrospectValue::Null)
                .expect("dismiss is invokable by name");
            assert_eq!(drain_into_reducer(&mut external), 1);
            assert!(!is_open());
        });
    }

    /// With nothing armed the paths still RESOLVE, reporting Null — an empty prompt is a state, not
    /// an unknown-path error.
    #[test]
    fn the_external_reports_null_rather_than_an_error_with_nothing_armed() {
        Owner::new().run(|| {
            seed_one_pane();
            let external = external();
            for path in ["prompt", "consequence", "verb"] {
                assert_eq!(
                    external.query(path),
                    Some(IntrospectValue::Null),
                    "{path} resolves to Null with nothing armed"
                );
            }
        });
    }

    /// A prompt whose target VANISHED out of band auto-disarms on the next frame, rather than
    /// lingering over something already gone.
    ///
    /// REVERT-PROOF: drop the `reconcile` body and the prompt is still up after the target is killed.
    #[test]
    fn a_prompt_auto_disarms_when_its_target_vanishes() {
        Owner::new().run(|| {
            seed_one_pane();
            let terminal = use_terminal();
            let victim = terminal.slots.new_window();
            run_or_arm(Command::KillWindow(victim.clone()), None, &terminal.slots);
            assert!(is_open());

            // Killed out of band — another client, the CLI, or its own last pane exiting.
            terminal.slots.kill_window(&victim);
            reconcile(&terminal.slots);

            assert!(
                !is_open(),
                "the prompt cannot outlive the thing it was asking about"
            );
        });
    }

    /// The panel PAINTS the captured sentence and its two answers, and the SAFE one carries the
    /// chosen fill.
    ///
    /// REVERT-PROOF: swap the two `chosen` arguments in `view_confirm` and the fill lands on the
    /// destructive button — this fails.
    #[test]
    fn the_panel_paints_the_question_with_the_safe_answer_preselected() {
        Owner::new().run(|| {
            seed_one_pane();
            let terminal = use_terminal();
            let victim = terminal.slots.new_window();
            run_or_arm(Command::KillWindow(victim.clone()), None, &terminal.slots);

            let panel = view_confirm(&Theme::dark(), (960, 600)).expect("a prompt paints");
            let text = walk_text(&panel);
            assert!(
                text.iter().any(|line| line.contains(&victim)),
                "the question is painted: {text:?}"
            );
            assert!(
                text.iter().any(|line| line == "Kill"),
                "the command's own verb is on the destructive button: {text:?}"
            );
            assert!(
                text.iter().any(|line| line == DISMISS_LABEL),
                "and the safe answer is offered: {text:?}"
            );

            let tags = walk_tags(&panel);
            for expected in [
                CONFIRM_SCRIM_TAG,
                CONFIRM_PANEL_TAG,
                CONFIRM_ACCEPT_TAG,
                CONFIRM_DISMISS_TAG,
            ] {
                assert!(tags.iter().any(|tag| tag == expected), "{expected} paints");
            }

            assert!(
                button_is_filled(&panel, CONFIRM_DISMISS_TAG),
                "the SAFE button is the preselected one"
            );
            assert!(
                !button_is_filled(&panel, CONFIRM_ACCEPT_TAG),
                "the destructive one is not"
            );
        });
    }

    /// Nothing armed paints nothing at all.
    #[test]
    fn nothing_is_painted_with_nothing_armed() {
        Owner::new().run(|| {
            seed_one_pane();
            assert!(view_confirm(&Theme::dark(), (960, 600)).is_none());
        });
    }

    /// The accessible tree: a MODAL dialog whose NAME carries the question AND the consequence, over
    /// two named buttons, with the chosen one as the active descendant.
    ///
    /// The consequence being in the NAME is the assertion that matters — an `aria-describedby` would
    /// leave the sentence that changes the answer to the AT's verbosity settings.
    ///
    /// REVERT-PROOF: fold the consequence out of the name and the first assertion fails; drop
    /// `with_modal` and the modal one does; swap the two `choice ==` comparisons and the focus ones do.
    #[test]
    fn the_accessible_tree_is_a_modal_dialog_naming_the_whole_consequence() {
        Owner::new().run(|| {
            seed_one_pane();
            assert!(
                confirm_access_nodes(Some(CONFIRM_PANEL_TAG)).is_empty(),
                "nothing armed advertises nothing at all"
            );

            let terminal = use_terminal();
            let windows = terminal.slots.windows();
            let only = windows[0].name.clone();
            // The one-window case, so there IS a consequence to fold in.
            run_or_arm(Command::KillWindow(only.clone()), None, &terminal.slots);
            let nodes = confirm_access_nodes(Some(CONFIRM_PANEL_TAG));
            assert_eq!(nodes.len(), 3, "dialog + two answers");

            let dialog = &nodes[0];
            assert_eq!(dialog.role, AriaRole::Dialog);
            assert!(dialog.modal, "a destructive prompt is a modal boundary");
            let name = dialog.name.as_deref().expect("the dialog is named");
            assert!(
                name.contains(&only) && name.contains("the session ends with it"),
                "the NAME carries the question and the consequence in one announcement: {name}"
            );

            let safe = &nodes[1];
            assert_eq!(safe.tag, CONFIRM_DISMISS_TAG);
            assert_eq!(safe.role, AriaRole::Button);
            assert_eq!(safe.name.as_deref(), Some(DISMISS_LABEL));
            assert!(
                safe.state.focused,
                "the SAFE answer is the active descendant of a fresh prompt"
            );
            let accept = &nodes[2];
            assert_eq!(accept.tag, CONFIRM_ACCEPT_TAG);
            assert_eq!(
                accept.name.as_deref(),
                Some("Kill"),
                "the command's own verb"
            );
            assert!(!accept.state.focused);

            // ...and moving the choice moves the active descendant with it.
            assert!(handle_key("ArrowRight"));
            let moved = confirm_access_nodes(Some(CONFIRM_PANEL_TAG));
            assert!(moved[2].state.focused && !moved[1].state.focused);
            dismiss();
        });
    }

    /// With no consequence the name is the question alone — no trailing fragment, and nothing invented
    /// to fill the gap.
    #[test]
    fn a_prompt_with_no_consequence_names_only_the_question() {
        Owner::new().run(|| {
            seed_one_pane();
            let terminal = use_terminal();
            let spare = terminal.slots.new_window();
            run_or_arm(Command::KillWindow(spare.clone()), None, &terminal.slots);

            let nodes = confirm_access_nodes(Some(CONFIRM_PANEL_TAG));
            assert_eq!(
                nodes[0].name.as_deref(),
                Some(format!("Kill window '{spare}'?").as_str()),
                "the question, and only the question"
            );
            dismiss();
        });
    }

    /// The active descendant is claimed only while the panel actually holds focus.
    ///
    /// REVERT-PROOF: drop the `panel_has_focus &&` guard and this fails.
    #[test]
    fn no_answer_is_active_while_the_panel_does_not_hold_focus() {
        Owner::new().run(|| {
            seed_one_pane();
            let terminal = use_terminal();
            let spare = terminal.slots.new_window();
            run_or_arm(Command::KillWindow(spare), None, &terminal.slots);

            let elsewhere = confirm_access_nodes(Some("sprag_gui.pane.0"));
            assert!(
                elsewhere.iter().all(|node| !node.state.focused),
                "no active descendant while focus is elsewhere"
            );
            dismiss();
        });
    }

    /// The prompt states the ESCALATION, not just the name: killing a session's last window ends the
    /// session, and that is the fact that changes an answer.
    ///
    /// REVERT-PROOF: drop the `consequence` arm of `Command::confirmation` and this fails.
    #[test]
    fn killing_the_last_window_says_that_it_ends_the_session() {
        Owner::new().run(|| {
            seed_one_pane();
            let terminal = use_terminal();
            let windows = terminal.slots.windows();
            assert_eq!(windows.len(), 1, "the fixture has one window");
            let only = windows[0].name.clone();

            run_or_arm(Command::KillWindow(only), None, &terminal.slots);
            let consequence = match external().query("consequence") {
                Some(IntrospectValue::Text(text)) => text,
                other => panic!("the consequence reads as text, got {other:?}"),
            };
            assert!(
                consequence.contains("session"),
                "the prompt names the escalation: {consequence}"
            );

            // ...and with a second window there is no escalation to state.
            dismiss();
            let spare = terminal.slots.new_window();
            run_or_arm(Command::KillWindow(spare), None, &terminal.slots);
            assert_eq!(
                external().query("consequence"),
                Some(IntrospectValue::Null),
                "a window that is not the last one carries no extra warning"
            );
        });
    }

    /// EVERY destructive command in the live catalog is held rather than run, not just the one the
    /// tests above happen to name. This is the structural version of the guard: a command added later
    /// that answers `confirmation()` is covered by this the day it appears.
    ///
    /// REVERT-PROOF: give `run_or_arm` an early `Command::run` for any variant and this fails.
    #[test]
    fn every_destructive_command_the_catalog_offers_is_held_for_an_answer() {
        Owner::new().run(|| {
            seed_one_pane();
            let terminal = use_terminal();
            terminal.slots.new_window();
            let offered = catalog(Some(0), &terminal.slots).commands;

            let destructive: Vec<Command> = offered
                .into_iter()
                .filter(|command| command.confirmation(Some(0), &terminal.slots).is_some())
                .collect();
            assert!(
                !destructive.is_empty(),
                "the catalog offers destructive commands at all"
            );

            for command in destructive {
                dismiss();
                run_or_arm(command.clone(), Some(0), &terminal.slots);
                assert!(
                    is_open(),
                    "{command:?} reached its effect without being asked about"
                );
                assert_eq!(
                    use_choice().get(),
                    Choice::Dismiss,
                    "{command:?} opened its prompt on the destructive answer"
                );
            }
            dismiss();
        });
    }
}
