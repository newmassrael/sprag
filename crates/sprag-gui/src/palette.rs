//! The COMMAND PALETTE: type a few letters, run any command by name (`Ctrl+Shift+P`).
//!
//! The commands themselves are not here — they live in [`crate::command`], which owns what each one
//! is called and what it does. This module is the SURFACE: a modal panel with a query field, a
//! ranked list of the commands that still match, and the keyboard to walk it. The split is the same
//! one the find bar keeps from the search engine ("this module is the display half"), and for the
//! same reason: a palette is a way to REACH commands, never a second opinion about what they are.
//!
//! ## Composed from delivered pinion substrate, not a new widget
//!
//! pinion ships `hello-command-palette` as a reference consumer of exactly this shape, and it is a
//! reference rather than a reusable widget — the example owns its own External and its own matcher,
//! because what a palette RUNS is the application's. So this module follows that shape with sprag's
//! own policy, over four pieces pinion already delivers:
//!
//! * [`ModalState`] — the open flag and the focus trap, moved TOGETHER. A palette is genuinely
//!   modal (the panel covers the panes and Tab must not wander behind it), and pinion's own note on
//!   this holder is that flipping the flag without moving the trap is the bug it exists to prevent.
//! * [`TextFieldExternal`] for the query, exactly as the find bar uses it — so the palette inherits
//!   a real caret, selection, `Home`/`End`, and IME composition rather than a hand-rolled string
//!   that a terminal user would immediately out-type.
//! * the composite-send wire: a row painted `{PALETTE_TAG}#{i}` routes a click to
//!   [`PaletteExternal`], which selects that row and ARMS it — palette rows are run-on-click, but
//!   the run itself happens in the reducer (see that type's docs for why it cannot happen there).
//! * a modal introspection face answering `open`, so "is the palette up?" is a first-class QUESTION
//!   over RPC — paired with an `open` VERB on the palette's own External, so opening it is a
//!   first-class REQUEST. Without that verb the surface was reachable only by its chord, and a
//!   chord is precisely what a headless smoke cannot press ([`PaletteExternal`]'s `open`).
//!
//! ## What is frozen when the palette opens, and why
//!
//! Two things are CAPTURED at open time and read from that capture afterwards:
//!
//! * **the target pane.** Opening the palette moves focus into its query field, so by the time a row
//!   is activated the focused pane is the palette, not the pane the user was looking at. This is the
//!   same trap the context menu documents ("a menu-item click BLURS the pane focus"), and the same
//!   fix: capture at open, act on the capture.
//! * **the command list.** [`crate::command::catalog`] reads live window and session lists, which a
//!   second client or an agent can change while the palette is up. Freezing means the rows the user
//!   read and the row an `Enter` runs come from ONE list, so an activation can never run the
//!   neighbour that a mid-open change shifted into that position. The QUERY still filters live — it
//!   is the frozen list that is filtered, so typing stays responsive without re-reading the host.
//!
//! ## Freshness, stated honestly
//!
//! Because the list is frozen, a window created while the palette is open does not appear in it
//! until the palette is re-opened. That is the deliberate trade named above (a stable list beats a
//! live one for a surface whose whole job is "run the thing I just read"), and it is cheap to undo:
//! `Escape` then `Ctrl+Shift+P` rebuilds it.

use std::rc::Rc;

use pinion_a11y::{
    AccessNode, AccessValue, AriaRole, AutoComplete, ListOption, listbox_option_nodes,
};
use pinion_core::composite_tag::{send_activation_index, send_activation_key};
use pinion_core::external::{
    Backend, BackendFallback, BackendSupport, External, ExternalIntrospect, InterveneError,
    IntrospectSchema, IntrospectValue, InvokeError, ReadRefusal, RepaintOwner, SchemaArg,
    SchemaField, ThreadOwnership,
};
use pinion_core::intent::Intent;
use pinion_core::reactive::{Owner, Signal};
use pinion_core::scene::{ContainerNode, Rect, TextNode};
use pinion_core::style::{
    AlignItems, BoxStyle, FlexDirection, JustifyContent, LayoutStyle, Size, TextStyle,
};
use pinion_core::theme::{ColorRole, Theme};
use pinion_core::widget_core::ExtraExternal;
use pinion_core::widgets::caret_blink::use_caret_blink;
use pinion_core::widgets::listbox_item::ListboxItemState;
use pinion_core::widgets::modal::{ModalState, modal_introspection_extra, use_modal};
use pinion_core::widgets::text_edit::use_text_edit_state;
use pinion_core::widgets::text_field::{TextFieldExternal, TextFieldState};
use pinion_core::{Modifiers, Scene};
use pinion_widget_paint::scrim::{M3_SCRIM_ALPHA, scrim_backdrop, scrim_fill};
use pinion_widget_paint::text_field as tf_paint;

use crate::command::{self, Command};
use crate::terminal::{focused_pane, use_terminal};

/// The query field's tag: its External registration key, its `use_text_edit_state` /
/// `use_caret_blink` key, its paint tag and its focus tag — one string for one surface, like the
/// find field's.
pub(crate) const PALETTE_FIELD_TAG: &str = "sprag_palette_query";

/// The palette's own External tag — the selection cursor and the `execute` verb. Row `i` paints as
/// the composite `{PALETTE_TAG}#{i}`, so a click on a row routes back to this one handle.
pub(crate) const PALETTE_TAG: &str = "sprag_palette";

/// The modal's introspection tag, answering `open` — registered whether or not the palette is up, so
/// "is the palette open?" is a question with an address rather than an inference from the tree.
const PALETTE_MODAL_TAG: &str = "sprag_palette_modal";

/// The backdrop container's tag — a COMPOSITE of [`PALETTE_TAG`], so the router's `#`-split delivers
/// a click on the backdrop to the palette's own External as the send key [`SCRIM_KEY`]. That is what
/// makes a click beside the panel DISMISS the palette (the light-dismiss every editor's palette has)
/// rather than merely being swallowed — the same composite-barrier shape the context menu uses for
/// its own click-outside.
///
/// Written out in full rather than `format!`-ed because the paint helper takes a `&'static str`; the
/// unit test `the_scrim_tag_is_a_composite_of_the_palette_tag` pins the two together (de-linked
/// deliberately — a `#[cfg(test)]` item is not in the doc build's scope) so a rename of one cannot
/// silently orphan the other.
const PALETTE_SCRIM_TAG: &str = "sprag_palette#scrim";

/// The send key the backdrop's composite tag arrives under.
const SCRIM_KEY: &str = "scrim";

/// The panel container's tag (the queryable anchor for the palette's own box).
const PALETTE_PANEL_TAG: &str = "sprag_palette_panel";

/// `use_modal` key for the palette's open flag + focus trap.
const PALETTE_MODAL_KEY: &str = "sprag_gui.palette.modal";
/// `Owner::cache` key for the command list FROZEN when the palette opened.
const PALETTE_CATALOG_KEY: &str = "sprag_gui.palette.catalog";
/// `Owner::cache` key for the pane CAPTURED when the palette opened.
const PALETTE_TARGET_KEY: &str = "sprag_gui.palette.target";
/// `Owner::cache` key for the cursor's index within the VISIBLE (filtered) rows.
const PALETTE_CURSOR_KEY: &str = "sprag_gui.palette.cursor";
/// `Owner::cache` key for the report about a project whose config could not be used.
const PALETTE_CONFIG_ERRORS_KEY: &str = "sprag_gui.palette.config_errors";

/// The query field's ACCESSIBLE NAME. Not a placeholder: `view_field` takes this only for
/// `with_aria_label` and paints no hint text, which is why the visible affordance is the prompt
/// glyph in [`view_palette`]'s input row instead.
const PALETTE_PLACEHOLDER: &str = "Run a command";

/// The open flag + focus trap, as one holder (see the module docs).
fn use_palette_modal() -> Rc<ModalState> {
    use_modal(PALETTE_MODAL_KEY)
}

/// The command list frozen at open time — the SSOT both the painted rows and an activation resolve
/// against, so they cannot disagree.
fn use_frozen_catalog() -> Signal<Rc<Vec<Command>>> {
    Owner::current()
        .expect("use_frozen_catalog() requires an active Owner scope")
        .cache(PALETTE_CATALOG_KEY, || Signal::new(Rc::new(Vec::new())))
        .as_ref()
        .clone()
}

/// Why a config contributed no commands, captured with the catalog it could not contribute to —
/// one report per broken file. Empty when every config that exists is fine.
fn use_config_errors() -> Signal<Rc<Vec<String>>> {
    Owner::current()
        .expect("use_config_errors() requires an active Owner scope")
        .cache(PALETTE_CONFIG_ERRORS_KEY, || {
            Signal::new(Rc::new(Vec::new()))
        })
        .as_ref()
        .clone()
}

/// The pane the palette's pane-commands act on, captured when it opened.
fn use_target_pane() -> Signal<Option<usize>> {
    Owner::current()
        .expect("use_target_pane() requires an active Owner scope")
        .cache(PALETTE_TARGET_KEY, || Signal::new(None))
        .as_ref()
        .clone()
}

/// The cursor's index within the VISIBLE rows (not the frozen catalog) — what `Enter` runs.
fn use_cursor() -> Signal<usize> {
    Owner::current()
        .expect("use_cursor() requires an active Owner scope")
        .cache(PALETTE_CURSOR_KEY, || Signal::new(0))
        .as_ref()
        .clone()
}

/// The text typed into the query field, read from the field's own state (the SSOT — the palette
/// keeps no second copy of it, exactly as the find bar reads its needle).
pub(crate) fn query() -> String {
    use_text_edit_state(PALETTE_FIELD_TAG).text()
}

/// Whether the palette is up. Subscribes the caller, so opening / closing repaints.
pub(crate) fn is_open() -> bool {
    use_palette_modal().is_open()
}

// ─── Matching ────────────────────────────────────────────────────────────────────────────────────

/// Case-insensitive subsequence score for `query` against `candidate`: `Some(score)` when every
/// non-whitespace character of the query appears in order, `None` otherwise. Higher is better.
///
/// The scoring is the palette convention every editor uses and pinion's own reference consumer
/// implements: a matched character scores `1`, one matched immediately after the previous adds a
/// consecutive-run bonus, and one at a word start adds a word-boundary bonus — so `nw` ranks
/// "New window" (two word starts) above an incidental mid-word hit. An empty query matches
/// everything at `0`, which is what makes the unfiltered palette list the catalog in its own order.
///
/// Whitespace is dropped from the query rather than matched, so "new win" finds "New window"; a
/// palette user types words with spaces and means "these letters, in this order".
///
/// Pure, so the ranking is deterministic and testable with no surface at all.
fn fuzzy_score(query: &str, candidate: &str) -> Option<i32> {
    let needle: Vec<char> = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    if needle.is_empty() {
        return Some(0);
    }
    let candidate: Vec<char> = candidate.chars().collect();
    let mut matched = 0usize;
    let mut score = 0i32;
    let mut previous: Option<usize> = None;
    for (at, character) in candidate.iter().enumerate() {
        if matched >= needle.len() {
            break;
        }
        if !character.to_lowercase().eq(needle[matched].to_lowercase()) {
            continue;
        }
        score += 1;
        if previous.is_some() && previous == at.checked_sub(1) {
            score += CONSECUTIVE_BONUS;
        }
        if at == 0 || matches!(candidate.get(at - 1), Some(' ' | '-' | '_' | '/')) {
            score += WORD_START_BONUS;
        }
        previous = Some(at);
        matched += 1;
    }
    (matched == needle.len()).then_some(score)
}

/// Bonus for a character matched immediately after the previous match (a run beats scattered hits).
const CONSECUTIVE_BONUS: i32 = 5;
/// Bonus for a character matched at a word start — worth more than a run, because typing initials
/// ("nw") is how a palette is actually used.
const WORD_START_BONUS: i32 = 10;

/// The indices into `catalog` that `query` matches, best first.
///
/// Ties keep catalog order (a stable sort), so an empty query lists every command exactly as
/// [`command::catalog`] ordered it, and a query that ranks two commands equally does not shuffle
/// them between frames.
fn visible(catalog: &[Command], query: &str) -> Vec<usize> {
    let mut scored: Vec<(usize, i32)> = catalog
        .iter()
        .enumerate()
        .filter_map(|(i, command)| fuzzy_score(query, &command.title()).map(|score| (i, score)))
        .collect();
    // Descending by score, and `sort_by_key` (not `sort_unstable_by_key`) because the STABILITY is
    // the point: equal scores must keep catalog order.
    scored.sort_by_key(|&(_, score)| core::cmp::Reverse(score));
    scored.into_iter().map(|(i, _)| i).collect()
}

/// The rows the palette is showing right now: the frozen catalog filtered by the live query.
fn visible_rows() -> Vec<usize> {
    visible(&use_frozen_catalog().get(), &query())
}

// ─── Open / close ────────────────────────────────────────────────────────────────────────────────

/// Open the palette (`Ctrl+Shift+P`), capturing the pane its pane-commands will act on and freezing
/// the command list (see the module docs for why both are captures).
///
/// The query is CLEARED, unlike the find bar's needle: a search is usually repeated, whereas a
/// palette is a fresh question each time, and a stale query would hide the command list behind the
/// last thing that was run.
pub(crate) fn open(target: Option<usize>) {
    let slots = &use_terminal().slots;
    use_target_pane().set(target);
    let catalog = command::catalog(target, slots);
    // The KEYMAP's report joins the catalog's, and is collected here rather than inside `catalog`
    // because `catalog` asks the HOST: a keybinding is what one client does with one keyboard, so
    // this client reads that half of `config.toml` itself and the host never looks at it. Which also
    // means the keymap's error is invisible to the collector beside it — `global_commands`
    // deserializes the very same file and a bad `action = "…"` string parses fine there.
    let mut errors = catalog.config_errors;
    errors.extend(crate::keys::use_client_keys().report());
    use_config_errors().set(Rc::new(errors));
    use_frozen_catalog().set(Rc::new(catalog.commands));
    use_cursor().set(0);
    use_text_edit_state(PALETTE_FIELD_TAG).set_text(String::new());
    // The field is the modal's only Tab stop: the rows are reached with the arrows (the palette
    // keyboard model every editor uses), so Tab has nowhere else to go and the trap has one member.
    use_palette_modal().open(vec![PALETTE_FIELD_TAG.to_owned()]);
}

/// Close the palette without running anything (`Escape`, or a command having just run).
///
/// Closing pops the focus trap, which restores focus to whatever the palette took it from — so the
/// pane the user was in gets it back with no explicit request here.
pub(crate) fn close() {
    use_palette_modal().close();
    use_frozen_catalog().set(Rc::new(Vec::new()));
    use_config_errors().set(Rc::new(Vec::new()));
    use_cursor().set(0);
}

/// ACTIVATE the row the cursor is on, then close. Returns the command that was activated, for the
/// caller's report.
///
/// "Activated", not "ran": a destructive row is HELD for a confirmation instead of performed, and the
/// choice is not made here. [`confirm::run_or_arm`](crate::confirm::run_or_arm) is the one door every
/// surface uses, and which side of it a command falls on is the command's own answer — see
/// [`crate::command`] on why that is not a palette policy. This is also what makes `Enter` on a
/// `Kill session` row safe enough to offer at all.
///
/// CLOSES FIRST, then activates: a command may itself move focus (`Find in scrollback` focuses the
/// find bar's field, a destructive one opens the prompt and traps focus in it), and the trap's
/// restore-on-close must not land after that. Activating into a closed palette is safe — the command
/// reads the pane that was captured, not the live focus.
///
/// That close-then-open pair is a modal HANDOFF — two stack edits from one user action, hence one
/// dispatch frame — and it is expressible only because the shell keeps every request a frame writes.
/// `modal_scope_request`'s mailbox was a single last-write-wins slot until pinion R1456, so the
/// `Close` this `close()` posts was OVERWRITTEN by the confirmation's `Open` and never popped: the
/// focus stack kept a scope for a palette that was no longer painted, and answering the prompt
/// popped only the prompt's, so after ONE confirmed row the client could never focus a pane again
/// and every pane-scoped row silently stopped being offered. The mailbox is an ordered queue now
/// (pop, then push), and the order written here — which never changed, because it was always the
/// intended one — is what it applies. `sprag-smoke`'s
/// `check_a_confirmed_row_leaves_the_focus_stack_clean` is what holds that, from outside the
/// process: a binding can read `focus_state::focused()` and nothing of the stack beneath it, so
/// sprag could not have detected the leak from in here even had it wanted to.
fn run_cursor_row() -> Option<Command> {
    let catalog = use_frozen_catalog().get();
    let rows = visible_rows();
    let command = catalog.get(*rows.get(cursor_in(rows.len()))?)?.clone();
    let target = use_target_pane().get();
    close();
    crate::confirm::run_or_arm(command.clone(), target, &use_terminal().slots);
    Some(command)
}

/// The cursor clamped into a row count — a query change can shrink the list under a stale cursor.
fn cursor_in(rows: usize) -> usize {
    if rows == 0 {
        0
    } else {
        use_cursor().get().min(rows - 1)
    }
}

// ─── Keyboard ────────────────────────────────────────────────────────────────────────────────────

/// Whether `tag` is the palette's query field — the router's gate, mirroring the find bar's.
pub(crate) fn is_palette_focus(tag: &str) -> bool {
    tag == PALETTE_FIELD_TAG
}

/// Route a key while the palette's field holds focus. Returns whether it was consumed.
///
/// `Escape` closes, `Enter` runs the cursor's row, the arrows and `Home` / `End` move the cursor,
/// and everything else is delegated to the field's own edit dispatch through pinion's
/// `forward_key_to_field` — which drives the External (keeping its statechart, caret and blink
/// authoritative) rather than writing the text behind its back. A key the field accepted resets the
/// cursor to the top row, which is the standard palette behaviour: the best match for what is now
/// typed is pre-selected, so `Enter` runs it without an arrow key.
///
/// Every key is consumed while the palette is up, including ones it does nothing with: the panes are
/// behind a modal scrim, and a keystroke leaking through to a shell that the user cannot see would
/// be worse than one that is dropped.
pub(crate) fn handle_key(scene: &mut Scene, key: &str, modifiers: Modifiers) -> bool {
    match key {
        "Escape" => {
            close();
            return true;
        }
        "Enter" => {
            run_cursor_row();
            return true;
        }
        "ArrowDown" | "ArrowUp" | "Home" | "End" => {
            move_cursor(key);
            return true;
        }
        _ => {}
    }
    let before = query();
    let handled = pinion_core::forward_key_to_field(scene, PALETTE_FIELD_TAG, key, modifiers);
    if handled && query() != before {
        use_cursor().set(0);
    }
    // Consumed either way — see the note above on not leaking a keystroke past the scrim.
    true
}

/// Move the cursor for a nav key, clamping at both ends.
///
/// Clamping rather than wrapping: a palette list is ranked, so its ends MEAN something (the best
/// match is at the top), and `ArrowUp` at the top silently jumping to the worst match is a
/// surprise. The find bar wraps because its matches are a ring in the scrollback, not a ranking.
fn move_cursor(key: &str) {
    let rows = visible_rows().len();
    if rows == 0 {
        return;
    }
    let last = rows - 1;
    let current = cursor_in(rows);
    let next = match key {
        "ArrowDown" => (current + 1).min(last),
        "ArrowUp" => current.saturating_sub(1),
        "Home" => 0,
        "End" => last,
        _ => current,
    };
    use_cursor().set(next);
}

// ─── The External (the RPC / AI surface) ─────────────────────────────────────────────────────────

/// The palette's cursor and its `execute` verb, as an External — so the surface is drivable and
/// readable by intent rather than only by synthesising clicks at pixels.
///
/// ## It CAPTURES its state, and does not RUN anything
///
/// Two constraints shape this type, and both were learned by watching it crash:
///
/// * **the handles are captured at construction, never resolved per call.** An External's `query` /
///   `invoke` are reached from the RPC dispatch, which is NOT inside the root `Owner` scope, so a
///   `use_…()` lookup there panics. [`create_palette_externals`] runs in that scope, so the
///   `Signal`s are resolved once, there, and held. (This is exactly the shape pinion's own reference
///   palette uses — it holds its `Rc<Signal<_>>`s as fields for the same reason.)
/// * **an activation EMITS an intent instead of running the command.** Running one reaches
///   [`crate::find`], [`crate::selection`], [`crate::dock`] — all of which resolve their own
///   `Owner`-scoped state, so none of them can be called from here either. The reducer, which
///   pinion drains intents into, DOES run in that scope; so a row activation pushes an intent and
///   [`handle_palette_intent`] runs the command there. The context menu's rows already work exactly
///   this way, which is why that is the shape to follow rather than an invention.
///
/// It holds no query and no command list of its own: the query is the field's text and the list is
/// the frozen catalog, so there is exactly one copy of each.
struct PaletteExternal {
    /// The command list frozen at open time (the same `Signal` [`use_frozen_catalog`] returns).
    catalog: Signal<Rc<Vec<Command>>>,
    /// The cursor within the visible rows (the same `Signal` [`use_cursor`] returns).
    cursor: Signal<usize>,
    /// The query field's own text state — read, never written, so the field stays its SSOT.
    text: Rc<pinion_core::widgets::text_edit::TextEditState>,
    /// Intents awaiting the shell's drain (the field the emitter contract requires).
    pending_intents: Vec<Intent>,
}

impl PaletteExternal {
    /// The indices into the frozen catalog that the live query matches — the same pure derivation
    /// [`visible_rows`] performs for the paint, over the same two pieces of state, so an RPC read
    /// and the screen cannot disagree.
    fn rows(&self) -> Vec<usize> {
        visible(&self.catalog.get(), &self.text.text())
    }

    /// The titles of those rows, in painted order.
    fn row_titles(&self) -> Vec<String> {
        let catalog = self.catalog.get();
        self.rows()
            .iter()
            .filter_map(|&i| catalog.get(i).map(Command::title))
            .collect()
    }

    /// This External's own view of the clamped cursor (it cannot call the module's, which resolves
    /// the `Signal` through the `Owner`).
    fn cursor_now(&self) -> usize {
        let rows = self.rows().len();
        if rows == 0 {
            0
        } else {
            self.cursor.get().min(rows - 1)
        }
    }

    /// Move the cursor to visible row `i`; `false` when out of range.
    fn select(&self, i: usize) -> bool {
        if i < self.rows().len() {
            self.cursor.set(i);
            true
        } else {
            false
        }
    }

    /// Ask the reducer to OPEN the palette.
    ///
    /// Armed rather than performed for the same reason a row activation is: [`open`] reads the live
    /// host through `use_terminal()` and writes the field's text state, all `Owner`-scoped, and this
    /// runs on the RPC dispatch outside that scope. It also carries no target — see
    /// [`open_on_request`], which is where the request's preconditions and its target live.
    fn arm_open(&mut self) {
        self.pending_intents
            .push(Intent::new_static(OPEN_EVENT, IntrospectValue::Null));
    }

    /// Ask the reducer to run the cursor's row (see the type docs for why it is not run here).
    /// Returns the title armed, so an RPC caller learns WHAT it just asked for.
    fn arm_run(&mut self) -> Option<String> {
        let title = self.row_titles().get(self.cursor_now()).cloned()?;
        // The event name only: pinion prefixes the emitting external's own tag, so this arrives at
        // the reducer as `{PALETTE_TAG}.{RUN_EVENT}`.
        self.pending_intents
            .push(Intent::new_static(RUN_EVENT, IntrospectValue::Null));
        Some(title)
    }

    /// Ask the reducer to dismiss the palette without running anything.
    fn arm_dismiss(&mut self) {
        self.pending_intents
            .push(Intent::new_static(DISMISS_EVENT, IntrospectValue::Null));
    }
}

/// The event this External emits to have the palette OPENED. A distinct address from the two below
/// for the same reason they are distinct from each other: the reducer must never have to infer which
/// of three transitions a payload meant.
const OPEN_EVENT: &str = "open";

/// The event this External emits to have the cursor's row RUN. Arrives at the reducer scoped as
/// `{PALETTE_TAG}.{RUN_EVENT}` (pinion prefixes an external's own tag).
const RUN_EVENT: &str = "run";

/// The event emitted to DISMISS without running — a distinct address rather than a payload flag on
/// [`RUN_EVENT`], so neither the emitter nor the reducer can confuse "run this" with "run nothing".
const DISMISS_EVENT: &str = "dismiss";

/// Run or dismiss on the palette's own intent, in the reducer's `Owner` scope. Returns whether the
/// intent was the palette's.
///
/// This is the other half of the External above: the activation happened there (a click, or an RPC
/// `execute`), and the EFFECT happens here, where the command's own state is reachable.
pub(crate) fn handle_palette_intent(intent: &Intent) -> bool {
    let Some((who, event)) = intent.tag_str().rsplit_once('.') else {
        return false;
    };
    if who != PALETTE_TAG {
        return false;
    }
    match event {
        OPEN_EVENT => {
            open_on_request();
            true
        }
        RUN_EVENT => {
            run_cursor_row();
            true
        }
        DISMISS_EVENT => {
            close();
            true
        }
        _ => false,
    }
}

/// Open the palette on an EXTERNAL's request — the one door such a request goes through, holding the
/// preconditions the keyboard gets for free from its router.
///
/// Both refusals are transitions the chord CANNOT produce, so honouring them is what keeps the RPC
/// face equal to the surface rather than a wider one:
///
/// * **already open.** [`crate::input`] routes a `Ctrl+Shift+P` typed into an open palette to the
///   query field, deliberately, so a second open never happens. Re-opening here would silently clear
///   a half-typed query — and the documented way to refresh the frozen list is still `Escape` then
///   the chord, which is two explicit steps rather than one that looks harmless.
/// * **the confirmation prompt is up.** `route_key` gates on [`crate::confirm::is_open`] before
///   anything else, so no key reaches the chord while the client is asking whether to destroy
///   something. [`crate::a11y`] states the consequence as an invariant its tree relies on — the two
///   modals are never up together — and an RPC caller must not be the one to break it.
///
/// The target is [`focused_pane`] rather than an argument: it is the app's one answer to which pane a
/// positionless interaction acts on (the context menu and an OS file drop resolve it the same way),
/// and it is exactly what the chord's `pane_index_of(focused)` yields. Letting a caller name a pane
/// would be a second authority over that question, and the two would eventually disagree.
fn open_on_request() {
    if is_open() {
        tracing::debug!(target: "sprag_gui::palette", "refusing an open request: already open");
        return;
    }
    if crate::confirm::is_open() {
        tracing::debug!(
            target: "sprag_gui::palette",
            "refusing an open request: the confirmation prompt is up"
        );
        return;
    }
    open(focused_pane());
}

impl core::fmt::Debug for PaletteExternal {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PaletteExternal")
            .field("rows", &self.rows().len())
            .finish_non_exhaustive()
    }
}

// NOT `query_proxy_external_impl!`: that macro is the SSOT for a query-proxy External that emits
// NOTHING, and it would leave `drain_intents` at its no-op default — silently swallowing every row
// activation. This External emits, so it declares the same five descriptors by hand (as pinion's own
// emitting externals do) plus the drain.
impl External for PaletteExternal {
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
    /// Whether a row activation is waiting to be run.
    ///
    /// This was MISSING, and its absence was a live defect: pinion's runtime harvests through
    /// `drain_one`, which skips [`drain_intents`](Self::drain_intents) entirely unless this says yes,
    /// and the trait default is `false`. So every activation that goes THROUGH this External — a click
    /// on a row, an RPC `execute` — armed an intent the reducer was never handed, and the palette
    /// silently did nothing. The keyboard path was unaffected (`Enter` runs in
    /// [`handle_key`] without touching the External), which is why the surface looked fine and why ten
    /// green unit tests said so: they call `drain_intents` directly, the one caller that does not ask.
    /// Found by the confirmation front's live pixel smoke; see the note on [`crate::confirm`]'s own.
    fn is_dirty(&self) -> bool {
        !self.pending_intents.is_empty()
    }
}

impl PaletteExternal {
    /// The reading itself — see [`query`](Self::query).
    fn read(&self, path: &str) -> Option<IntrospectValue> {
        let rows = self.row_titles();
        match path {
            sprag_host::wire::ACTION_GRAMMAR_SLOT => Some(IntrospectValue::Json(
                sprag_host::wire::ActionGrammar::answer(crate::wire_claim::grammar::palette()),
            )),
            "row_count" => Some(IntrospectValue::Int(i64::try_from(rows.len()).ok()?)),
            "cursor" => Some(IntrospectValue::Int(i64::try_from(self.cursor_now()).ok()?)),
            // Present-but-empty: the path always resolves, and an empty list reports Null rather
            // than an unknown-path error (a palette with no matches is a state, not a mistake).
            "cursor_command" => Some(match rows.get(self.cursor_now()) {
                Some(title) => IntrospectValue::Text(title.clone()),
                None => IntrospectValue::Null,
            }),
            _ => {
                let i: usize = path.strip_prefix("row.")?.parse().ok()?;
                rows.get(i)
                    .map(|title| IntrospectValue::Text(title.clone()))
            }
        }
    }
}

impl ExternalIntrospect for PaletteExternal {
    fn schema(&self) -> IntrospectSchema {
        IntrospectSchema::new(
            const {
                &[
                    SchemaField::new("row_count", "int"),
                    SchemaField::parametric(
                        "row.<i>",
                        "string",
                        const { &[SchemaArg::index("i", "row_count")] },
                    ),
                    SchemaField::new("cursor", "int"),
                    SchemaField::new("cursor_command", "string"),
                    // ⚠ VERBS, DECLARED AS SUCH (R352). All four were `SchemaField::new`,
                    // which puts a path on the READ channel — so this surface told a client it
                    // could `scene/query` four names that answer nothing, and `open` additionally
                    // claimed to be a `bool` somebody could read. pinion refuses an undeclared
                    // invoke from R1637 and reads the channel to do it, so the mis-declaration is
                    // not only a false claim on the read surface: it is what the next pin bump
                    // would refuse.
                    SchemaField::action("open", "json"),
                    SchemaField::action("select", "int"),
                    SchemaField::action("execute", "json"),
                    SchemaField::action("send", "string"),
                    // HOW TO CALL THE FOUR ABOVE — this surface's own grammar, answered by the
                    // surface that serves them (`sprag_host::wire::ACTION_GRAMMAR_SLOT`). Every
                    // argument here was folklore until R354, and two of these verbs take nothing at
                    // all, which is a fact a client could not have guessed from a name.
                    SchemaField::new(sprag_host::wire::ACTION_GRAMMAR_SLOT, "object"),
                ]
            },
        )
    }

    /// ⚠⚠ **THE IDENTITY MIGRATION** — see the same note on this crate's sibling surfaces.
    /// pinion R1674 turned a read's absence into a REFUSAL, and its dispatch maps `UnknownPath`
    /// onto the fault a `None` produced before, so this preserves the wire exactly. The three
    /// richer arms are a per-path decision and are registered as owed rather than guessed.
    fn query(&self, path: &str) -> Result<IntrospectValue, ReadRefusal> {
        self.read(path).ok_or(ReadRefusal::UnknownPath)
    }

    fn intervene(&mut self, path: &str, value: IntrospectValue) -> Result<(), InterveneError> {
        match path {
            "cursor" => match value {
                IntrospectValue::Int(n) => {
                    let rows = self.rows().len();
                    let out_of_range =
                        || InterveneError::out_of_range(format!("row {n} is outside 0..{rows}"));
                    let i = usize::try_from(n).map_err(|_| out_of_range())?;
                    if self.select(i) {
                        Ok(())
                    } else {
                        Err(out_of_range())
                    }
                }
                _ => Err(InterveneError::TypeMismatch),
            },
            "row_count" | "cursor_command" => Err(InterveneError::ReadOnly),
            _ => Err(InterveneError::UnknownPath),
        }
    }

    fn invoke(
        &mut self,
        path: &str,
        args: IntrospectValue,
    ) -> Result<IntrospectValue, InvokeError> {
        // A verb this surface does not PUBLISH is a verb it does not run — see
        // [`crate::wire_claim::declares_verb`] for the defect that makes this a guard rather than
        // a test.
        if !crate::wire_claim::declares_verb(&self.schema(), path) {
            return Err(InvokeError::UnknownPath);
        }
        match path {
            // ARMS an open (the reducer performs it, and may refuse it — see [`open_on_request`]),
            // answering only that the request was taken. Whether the palette is now UP is a separate
            // question with its own address: the modal face at `PALETTE_MODAL_TAG` answers it. That
            // split is deliberate — a return value here could only ever describe the arm, and a
            // caller that believed it described the effect would be reading a two-step as one.
            //
            // Takes no arguments, so any are ignored: the target pane is the app's, not the
            // caller's, and there is nothing else to say.
            "open" => {
                self.arm_open();
                Ok(IntrospectValue::Bool(true))
            }
            "select" => match args {
                IntrospectValue::Int(n) => {
                    let i = usize::try_from(n).map_err(|_| InvokeError::TypeMismatch)?;
                    if self.select(i) {
                        Ok(IntrospectValue::Int(n))
                    } else {
                        Err(InvokeError::rejected(format!(
                            "row {n} is outside 0..{}",
                            self.rows().len()
                        )))
                    }
                }
                _ => Err(InvokeError::TypeMismatch),
            },
            // ARMS the run (the reducer performs it on the intent drain — see the type docs), and
            // answers with the title it armed so a caller knows what it asked for. Null when there
            // is no row to run.
            "execute" => Ok(match self.arm_run() {
                Some(title) => IntrospectValue::Text(title),
                None => IntrospectValue::Null,
            }),
            // The composite-send wire: a click on row `i` selects it AND activates it. A palette row
            // is act-on-click (pinion's reference consumer's rule) — there is no second confirming
            // click in any editor's palette. What that one click cannot do is destroy anything: a
            // destructive row activates into the confirmation prompt, never into the act
            // ([`crate::confirm`]), which is what let those commands into the catalog at all.
            "send" => match args {
                IntrospectValue::Text(ref payload) => {
                    // The backdrop shares this handle through its composite tag: a click OUTSIDE the
                    // panel dismisses without running anything. Checked before the row index, since
                    // its key is not a number and would otherwise fall through as a no-op.
                    if send_activation_key(payload) == Some(SCRIM_KEY) {
                        self.arm_dismiss();
                    } else if let Some(i) = send_activation_index(payload)
                        && self.select(i)
                    {
                        self.arm_run();
                    }
                    Ok(IntrospectValue::Bool(true))
                }
                _ => Err(InvokeError::TypeMismatch),
            },
            _ => Err(InvokeError::UnknownPath),
        }
    }
}

/// The palette's Externals, registered every reconcile at their constant tags (pinion preserves a
/// surviving external's live state by tag, so the query text and caret survive re-registration).
///
/// Registered whether or not the palette is open, like the find bar's: the field's External is what
/// HOLDS the text, and an unpainted External costs nothing.
pub(crate) fn create_palette_externals() -> Vec<ExtraExternal> {
    vec![
        ExtraExternal::new(
            PALETTE_FIELD_TAG.to_owned(),
            Box::new(
                TextFieldExternal::new()
                    .attach_state(use_text_edit_state(PALETTE_FIELD_TAG))
                    .attach_blink(use_caret_blink(PALETTE_FIELD_TAG)),
            ),
        ),
        ExtraExternal::new(
            PALETTE_TAG.to_owned(),
            Box::new(PaletteExternal {
                catalog: use_frozen_catalog(),
                cursor: use_cursor(),
                text: use_text_edit_state(PALETTE_FIELD_TAG),
                pending_intents: Vec::new(),
            }),
        ),
        modal_introspection_extra(PALETTE_MODAL_TAG, use_palette_modal()),
    ]
}

// ─── Accessibility ───────────────────────────────────────────────────────────────────────────────

/// The palette's accessible tree, or nothing at all while it is closed.
///
/// Shape: a MODAL [`AriaRole::Dialog`] on the panel, holding the query field and the `listbox` of
/// matching commands — `[dialog, combobox, listbox, option…]`, the flat `[parent, ...children]` list
/// the session rail's own builder returns, with bounds left `None` for the shell to resolve from each
/// tag's painted rect ([`crate::a11y`]'s discipline).
///
/// ## Why the field is an editable COMBOBOX
///
/// [`crate::a11y`] sets the test a role has to pass: never advertise an affordance the widget does not
/// implement (it is why a pane is a `Group` and not a textbox). Run the editable-combobox contract
/// against this surface and every clause holds — the field is a real [`TextFieldExternal`] with a
/// caret, selection and IME; typing filters a popup list; the arrows move an ACTIVE option while focus
/// stays in the field; `Enter` acts on that option. The one clause that does not hold is inline
/// completion, which is exactly the difference between [`AutoComplete::List`] (declared) and
/// `AutoComplete::Both` (not). So the role is earned rather than aspirational.
///
/// `aria-activedescendant` is expressed the way the rail expresses it: the CURSOR row is the option
/// marked focused, and only while the field actually owns focus — an active descendant of a surface
/// that does not have focus would be a lie about where the next keystroke goes.
///
/// ## The bound worth stating
///
/// The options are the PAINTED rows, so `aria-setsize` is the painted count
/// ([`MAX_VISIBLE_ROWS`]) rather than the number of matches: with thirty matches an AT hears "1 of
/// 10", which is what is on screen. Announcing a set the surface does not render — rows with no tag,
/// no bounds and no way to be reached but by typing — would be the worse of the two inaccuracies.
pub(crate) fn palette_access_nodes(focused: Option<&str>) -> Vec<AccessNode> {
    if !is_open() {
        return Vec::new();
    }
    let catalog = use_frozen_catalog().get();
    let rows = visible_rows();
    let cursor = cursor_in(rows.len());
    let field_has_focus = focused == Some(PALETTE_FIELD_TAG);

    // One option per PAINTED row, labelled with what the row says AND the chord it teaches, so an AT
    // user hears the keyboard shortcut the sighted user reads in the right-hand column.
    let labels: Vec<String> = rows
        .iter()
        .take(MAX_VISIBLE_ROWS)
        .filter_map(|&index| catalog.get(index))
        .map(|command| match command.hint() {
            Some(hint) => format!("{}, {hint}", command.title()),
            None => command.title(),
        })
        .collect();
    let options: Vec<ListOption<'_>> = labels
        .iter()
        .enumerate()
        .map(|(row, label)| ListOption {
            tag: row_tag(row),
            label: Some(label.as_str()),
            // Idle, honestly: sprag paints no per-row hover or pressed posture for a palette row, so
            // any other value would describe a state this surface does not have.
            state: ListboxItemState::Idle,
            selected: row == cursor,
            focused: field_has_focus && row == cursor,
        })
        .collect();

    let mut nodes = vec![
        AccessNode::new(PALETTE_PANEL_TAG, AriaRole::Dialog)
            .with_name(PALETTE_PLACEHOLDER)
            .with_modal()
            .with_child(PALETTE_FIELD_TAG)
            .with_child(PALETTE_ROWS_TAG),
        AccessNode::new(PALETTE_FIELD_TAG, AriaRole::EditableComboBox)
            .with_name(PALETTE_PLACEHOLDER)
            .with_value(AccessValue::Text(query()))
            .with_auto_complete(AutoComplete::List)
            .with_expanded(!options.is_empty())
            .with_controls(PALETTE_ROWS_TAG)
            .with_focused(field_has_focus),
    ];
    nodes.extend(listbox_option_nodes(
        PALETTE_ROWS_TAG,
        ROWS_LIST_NAME,
        false,
        &options,
    ));
    nodes
}

/// Visible row `row`'s tag — the composite the row PAINTS under, so the accessible node and the
/// clickable node are one identity. Leaked as a `&'static str` because [`ListOption`] borrows its tag
/// and the rows are a bounded set ([`MAX_VISIBLE_ROWS`]) of stable strings; the alternative is
/// threading an owned `Vec<String>` through the builder purely to satisfy a lifetime.
fn row_tag(row: usize) -> &'static str {
    const TAGS: [&str; MAX_VISIBLE_ROWS] = [
        "sprag_palette#0",
        "sprag_palette#1",
        "sprag_palette#2",
        "sprag_palette#3",
        "sprag_palette#4",
        "sprag_palette#5",
        "sprag_palette#6",
        "sprag_palette#7",
        "sprag_palette#8",
        "sprag_palette#9",
    ];
    TAGS[row]
}

/// The `listbox`'s accessible name — what the list IS, distinct from the dialog's name (what the
/// surface is FOR), so an AT walking in does not hear the same phrase twice.
const ROWS_LIST_NAME: &str = "Matching commands";

// ─── Paint ───────────────────────────────────────────────────────────────────────────────────────

/// The query field's interaction posture, read out of the model scene (the `read_state` seam) —
/// the same projection the find bar's field needs, for the same reason: only the model scene knows
/// an External's SCXML state, so it has to cross into the pure paint.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) struct PaletteFieldState {
    /// The field's interaction state (idle / hover / focused / editing).
    pub(crate) field: TextFieldState,
    /// The caret's byte offset within the query.
    pub(crate) caret: u32,
}

/// Project the palette field's posture out of the model scene. Defaults before the first reconcile
/// has registered the External.
pub(crate) fn read_field_state(scene: &Scene) -> PaletteFieldState {
    let (field, caret) = tf_paint::read_text_field_state(scene, PALETTE_FIELD_TAG);
    PaletteFieldState { field, caret }
}

/// The palette's paint: a scrim over the whole window centring a panel of the query field above the
/// matching rows — or nothing at all when it is closed.
///
/// Anchored near the TOP of the window rather than centred vertically, which is where every editor
/// puts a palette: the list grows downward from a fixed field, so the row under the cursor does not
/// move as the query narrows the list.
pub(crate) fn view_palette(
    state: PaletteFieldState,
    theme: &Theme,
    window: (u32, u32),
) -> Option<Scene> {
    if !is_open() {
        return None;
    }
    let catalog = use_frozen_catalog().get();
    let rows = visible_rows();
    let cursor = cursor_in(rows.len());

    let style = tf_paint::TextFieldStyle {
        field_w: PANEL_W - PANEL_PADDING * 2 - PROMPT_W,
        field_h: FIELD_H,
        ..tf_paint::TextFieldStyle::m3_filled()
    };
    let field = tf_paint::view_field(
        PALETTE_FIELD_TAG,
        state.field,
        state.caret,
        theme,
        &style,
        PALETTE_PLACEHOLDER,
    );
    // The field is wrapped in an explicitly-filled row behind a prompt glyph, because
    // `view_field` paints NO placeholder — its last argument is the accessible NAME only, so an
    // empty field is an empty rectangle the same colour as the panel. Without this the palette
    // opens showing a blank strip where its input is, which the live smoke's screenshot caught.
    let input = Scene::Container(
        ContainerNode::new(vec![
            Scene::Text(TextNode::styled(
                PROMPT_GLYPH,
                Rect::default(),
                TextStyle::new()
                    .with_size_px(ROW_FONT_PX)
                    .with_fg(theme.resolve(ColorRole::Accent)),
            )),
            field,
        ])
        .with_tag(PALETTE_INPUT_TAG)
        .with_style(
            BoxStyle::filled(theme.resolve(ColorRole::SurfaceContainerHighest))
                .with_corner_radius(ROW_RADIUS),
        )
        .with_layout(
            LayoutStyle::new()
                .flex(FlexDirection::Row)
                .with_align_items(AlignItems::Center)
                .with_padding(Rect::new(ROW_PADDING, 0, 0, 0))
                .with_size(Size::px(PANEL_W - PANEL_PADDING * 2, FIELD_H)),
        ),
    );

    let mut children: Vec<Scene> = Vec::with_capacity(rows.len() + 3);
    children.push(input);
    // A project whose config is broken gets a line of its own, above the rows: it is a REPORT, not a
    // command, so it never becomes a row that looks runnable. Painted in the error role so it does
    // not read as one more thing to pick.
    for report in use_config_errors().get().iter() {
        children.push(Scene::Text(TextNode::styled(
            report.clone(),
            Rect::default(),
            TextStyle::new()
                .with_size_px(CHORD_FONT_PX)
                .with_fg(theme.resolve(ColorRole::Error)),
        )));
    }
    if rows.is_empty() {
        // An honest empty state. "No matching command" describes what happened; a blank panel would
        // read as a palette that had broken.
        children.push(Scene::Text(TextNode::styled(
            NO_MATCH_LABEL,
            Rect::default(),
            TextStyle::new()
                .with_size_px(ROW_FONT_PX)
                .with_fg(theme.resolve(ColorRole::OnSurfaceMuted)),
        )));
    }
    // The rows live in a container of their OWN, tagged, rather than as further children of the
    // panel. What it adds is a painted BOX for the `listbox` node of [`palette_access_nodes`] to
    // resolve its bounds from, the same way the session rail's `tablist` hangs off its own painted
    // sub-container: an a11y container with no painted peer has no bounds for the shell to resolve,
    // which is why this is a paint change and not purely a tree one. Omitted entirely when nothing
    // matches, so an empty palette grows no stray box (and no stray gap).
    //
    // It carries no style and no padding, and the CONTENT HEIGHT is unchanged: the panel used to hold
    // N rows and N gaps, and now holds one gap plus a box of N rows and N-1 gaps. That equality is
    // pinned by `the_rows_container_fits_inside_the_panel_at_every_paintable_count` as ARITHMETIC
    // between the two sizing functions; the painted geometry it predicts is what the live smoke reads
    // back, reaching this panel through the External's `open` verb (the chord it used to need is the
    // one thing a headless run cannot press).
    let row_nodes: Vec<Scene> = rows
        .iter()
        .take(MAX_VISIBLE_ROWS)
        .enumerate()
        .filter_map(|(row, &index)| {
            catalog
                .get(index)
                .map(|command| view_row(row, command, row == cursor, theme))
        })
        .collect();
    if !row_nodes.is_empty() {
        children.push(Scene::Container(
            ContainerNode::new(row_nodes)
                .with_tag(PALETTE_ROWS_TAG)
                .with_layout(
                    LayoutStyle::new()
                        .flex(FlexDirection::Column)
                        .with_gap(ROW_GAP)
                        .with_size(Size::px(
                            PANEL_W - PANEL_PADDING * 2,
                            painted_rows_height(rows.len()),
                        )),
                ),
        ));
    }

    let panel = Scene::Container(
        ContainerNode::new(children)
            .with_tag(PALETTE_PANEL_TAG)
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
                    .with_size(Size::px(PANEL_W, panel_height(rows.len()))),
            ),
    );

    Some(scrim_backdrop(
        PALETTE_SCRIM_TAG,
        scrim_fill(M3_SCRIM_ALPHA),
        window,
        FlexDirection::Column,
        AlignItems::Center,
        // Start, not Center: the panel hangs from the top of the window (see the fn docs).
        JustifyContent::Start,
        panel,
    ))
}

/// One command row: its title, and the chord that runs it without the palette.
///
/// Tagged the composite `{PALETTE_TAG}#{row}` so a click routes to [`PaletteExternal`]'s `send`.
/// The index in the tag is the VISIBLE row, which is what `send` selects — paint and activation
/// therefore index the same list, the invariant the frozen catalog exists to guarantee.
fn view_row(row: usize, command: &Command, at_cursor: bool, theme: &Theme) -> Scene {
    let ink = theme.resolve(if at_cursor {
        ColorRole::OnSurface
    } else {
        ColorRole::OnSurfaceMuted
    });
    let title = Scene::Text(TextNode::styled(
        command.title(),
        Rect::default(),
        TextStyle::new().with_size_px(ROW_FONT_PX).with_fg(ink),
    ));
    let mut cells = vec![title];
    if let Some(hint) = command.hint() {
        cells.push(Scene::Text(TextNode::styled(
            hint,
            Rect::default(),
            TextStyle::new()
                .with_size_px(CHORD_FONT_PX)
                .with_fg(theme.resolve(ColorRole::OnSurfaceMuted)),
        )));
    }
    // The cursor row is filled rather than outlined: the palette's list is dense, and a fill reads
    // at a glance where a 2px border on one of a dozen rows does not.
    let fill = if at_cursor {
        BoxStyle::filled(theme.resolve(ColorRole::SurfaceContainerHighest))
            .with_corner_radius(ROW_RADIUS)
    } else {
        BoxStyle::default()
    };
    Scene::Container(
        ContainerNode::new(cells)
            .with_tag(format!("{PALETTE_TAG}#{row}"))
            .with_style(fill)
            .with_layout(
                LayoutStyle::new()
                    .flex(FlexDirection::Row)
                    .with_align_items(AlignItems::Center)
                    .with_justify(JustifyContent::SpaceBetween)
                    .with_padding(Rect::new(ROW_PADDING, 0, ROW_PADDING, 0))
                    .with_size(Size::px(PANEL_W - PANEL_PADDING * 2, ROW_H)),
            ),
    )
}

/// The panel's height for a given row count: the field plus the rows it will actually paint. Sized
/// to the CONTENT so the panel shrinks as a query narrows the list, rather than leaving a tall empty
/// box under three matches.
fn panel_height(rows: usize) -> u32 {
    let painted = u32::try_from(rows.clamp(1, MAX_VISIBLE_ROWS)).unwrap_or(1);
    PANEL_PADDING * 2 + FIELD_H + ROW_GAP + painted * (ROW_H + ROW_GAP)
}

/// The height of the rows' own container — the painted rows and the gaps BETWEEN them (one fewer
/// than the rows), the wrapper contributing no padding of its own. Derived from the same two
/// constants [`panel_height`] uses, so the box the rows sit in cannot drift from the panel sized to
/// hold it.
fn painted_rows_height(rows: usize) -> u32 {
    let painted = u32::try_from(rows.clamp(1, MAX_VISIBLE_ROWS)).unwrap_or(1);
    painted * ROW_H + painted.saturating_sub(1) * ROW_GAP
}

/// The panel's width in logical pixels — wide enough for the longest command title plus its chord.
const PANEL_W: u32 = 460;
/// The panel's inner padding on every edge (M3 dialog-ish).
const PANEL_PADDING: u32 = 12;
/// The panel's corner radius.
const PANEL_RADIUS: u32 = 12;
/// The query field's height (M3 filled single-line, as the find bar's).
const FIELD_H: u32 = 40;
/// One row's height.
const ROW_H: u32 = 28;
/// The gap between the field and the rows, and between rows.
const ROW_GAP: u32 = 4;
/// A row's inner padding.
const ROW_PADDING: u32 = 8;
/// The cursor row's fill corner radius (shared with the input row's).
const ROW_RADIUS: u32 = 6;
/// The width reserved for the prompt glyph left of the query field.
const PROMPT_W: u32 = 20;
/// The prompt glyph — the shell convention for "a command goes here", so the input reads as an
/// input even while it is empty (`view_field` paints no placeholder of its own).
const PROMPT_GLYPH: &str = "›";
/// The input row's tag (the queryable anchor for the field's visible box).
const PALETTE_INPUT_TAG: &str = "sprag_palette_input";
/// The rows' own container tag — the painted box the `listbox` accessible node resolves its bounds
/// from (see the note in [`view_palette`] where it is built).
const PALETTE_ROWS_TAG: &str = "sprag_palette_rows";
/// A row title's font size.
const ROW_FONT_PX: u32 = 14;
/// The right-hand hint's font size (a chord, or a project command's line) — smaller than the title,
/// because it says what the row does rather than naming it.
const CHORD_FONT_PX: u32 = 12;

/// The most rows painted at once. The palette does not scroll (a fuzzy query is the way to reach a
/// command past the cut, and it is faster than scrolling), so this is an honest bound rather than a
/// viewport: rows past it are reachable by typing one more letter, exactly as the tab strip's
/// overflow is reachable through the CLI.
const MAX_VISIBLE_ROWS: usize = 10;

/// What an empty result set says.
const NO_MATCH_LABEL: &str = "No matching command";

#[cfg(test)]
mod tests {
    use sprag_host::Host;

    /// The `KillWindow` command a person clicking the row for the window NAMED `name` would get —
    /// resolved through the live list, so a test carries the identity the surface carries (R330).
    fn kill_of(slots: &crate::slotview::SlotView, name: &str) -> crate::command::Command {
        let window = slots
            .windows()
            .into_iter()
            .find(|row| row.name == name)
            .expect("the window is in the live list");
        crate::command::Command::KillWindow {
            window: window
                .id
                .expect("the in-process host publishes an identity"),
            label: window.name,
        }
    }
    use sprag_terminal::CommandBuilder;

    use super::*;
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

    /// ⚠⚠ **THIS SURFACE'S DECLARATION SAYS WHAT ITS PATHS ARE.** All four of its verbs were
    /// declared on the READ channel until R352 — and `open` additionally claimed to be a readable
    /// `bool`, which is the shape a client would have believed. See [`crate::wire_claim`].
    #[test]
    fn a_declared_path_is_what_it_claims() {
        Owner::new().run(|| {
            let mut surface = external();
            crate::wire_claim::a_declared_path_is_what_it_claims(&mut surface);
        });
    }

    /// ⚠⚠ **THE PUBLISHED GRAMMAR OF THIS SURFACE, HELD TO THE SURFACE** — the claims in
    /// [`sprag_conformance`], driven through this external's real `invoke`.
    ///
    /// The claims live in one crate for every surface that publishes one (six of them
    /// now: three in the daemon's scene and three in this window's). What stays here is the fixture
    /// and the COUNTS — a number per claim, so a table whose declarations quietly went missing fails
    /// on a count rather than passing by driving nothing.
    #[test]
    fn the_published_grammar_is_this_surfaces_own() {
        Owner::new().run(|| {
            let mut surface = external();
            let table = crate::wire_claim::grammar::palette();

            // ⚠ ZERO PUBLISHED WORDS, ASSERTED RATHER THAN SKIPPED. Not one of this window's eight
            // verbs takes a closed vocabulary — they take an event name, a row index, or nothing —
            // so this claim drives nothing today and the number is what says so. An argument that
            // gains a `one_of` moves it, and the claim starts holding it.
            assert_eq!(
                sprag_conformance::every_published_word_is_accepted(table, &mut |action, args| {
                    surface.invoke(action, args)
                })
                .count_or_panic(),
                0,
                "this surface publishes no closed vocabulary",
            );

            assert_eq!(
                sprag_conformance::a_constrained_argument_publishes_what_it_admits(
                    table,
                    &mut |action, args| surface.invoke(action, args)
                )
                .count_or_panic(),
                1,
                "one open string argument: the composite event payload `send` takes",
            );

            assert_eq!(
                sprag_conformance::an_optional_argument_may_be_declined_as_null(
                    table,
                    &mut |action, args| surface.invoke(action, args)
                )
                .count_or_panic(),
                0,
                "⚠⚠ ZERO IS THE MEASUREMENT, AND IT IS A TRIPWIRE. This surface declares no \
                 OPTIONAL argument at all, so the `null`-is-declined class cannot arise on it — \
                 which is a fact about the grammar and not a pass. Asserted as a COUNT because \
                 the first form of this gate checked only that there were no FINDINGS, and a \
                 walker with nothing to drive has none: it reported this surface clean while \
                 measuring nothing. The day a form here grows its first optional argument this \
                 number moves, and whoever moves it has to answer the question",
            );

            assert_eq!(
                sprag_conformance::a_declared_argument_is_one_the_daemon_reads(
                    table,
                    &mut |action, args| surface.invoke(action, args)
                )
                .count_or_panic(),
                2,
                "one probe per declared argument: a row index and an event payload, each \
                 the whole `args` value of its own scalar form",
            );

            assert_eq!(
                sprag_conformance::a_nullary_form_is_a_verb_that_needs_nothing(
                    table,
                    &mut |action, args| surface.invoke(action, args)
                )
                .count_or_panic(),
                4,
                "two calls each for `open` and `execute`: the bare call, and one carrying \
                 an argument no declaration mentions",
            );

            // ⚠⚠⚠ AND THE SHAPES, against the protocol number — the pin the daemon has and cannot
            // point at this window. See `wire_claim::grammar::shapes_are_pinned_to_the_protocol`.
            let served = surface
                .query(sprag_host::wire::ACTION_GRAMMAR_SLOT)
                .expect("the palette serves its grammar");
            let pinion_core::external::IntrospectValue::Json(served) = served else {
                panic!("the grammar slot answers JSON: {served:?}");
            };
            crate::wire_claim::grammar::shapes_are_pinned_to_the_protocol(
                &served,
                "the command palette",
                // R371: re-stamped for a bump this surface's own shapes had no part in — an
                // argument added to the plugin host's three looping run forms. ⚠ THE THIRD OF
                // THREE HAND-EDITS FOR ONE BUMP, which is the cost the register predicted when
                // this pin became three call sites: the daemon DERIVES its surfaces from the
                // served scene and cannot be left out of a re-stamp; these hang in a window and
                // are pinned by whoever remembers all three.
                // ⚠⚠⚠ R372: AND AGAIN, THE VERY NEXT BUMP — three hand-edits for a widened ANSWER
                // vocabulary (`taken_over`) that no GUI surface publishes, serves or reads. Twice
                // in consecutive rounds is no longer a prediction: the price is being paid every
                // time the wire moves for ANY reason, which is the argument for the driver that
                // builds all three externals in one test rather than three copies of this number.
                31,
                &[
                    // ⚠ MEASURED, not guessed: it is `select` that carries the row, and `execute`
                    // acts on the palette's own armed request — which is exactly why five of these
                    // eight verbs are nullary and why a pin over the shapes is worth having.
                    "open[nullary]:",
                    "execute[nullary]:",
                    "select[scalar]:row:int",
                    "send[scalar]:event:string",
                ],
            );
        });
    }

    /// The External as [`create_palette_externals`] builds it — same captured handles, so a test
    /// drives the real thing rather than a look-alike.
    fn external() -> PaletteExternal {
        PaletteExternal {
            catalog: use_frozen_catalog(),
            cursor: use_cursor(),
            text: use_text_edit_state(PALETTE_FIELD_TAG),
            pending_intents: Vec::new(),
        }
    }

    /// Drain `external`'s emitted intents through the reducer hook, which is what the shell does —
    /// the step that actually RUNS an armed command. Returns how many the palette claimed.
    ///
    /// The tag is SCOPED here (`{PALETTE_TAG}.{event}`) because an external pushes only its event
    /// name and pinion prefixes the emitting external's tag on the way to the reducer. Skipping that
    /// is the live bug the context menu's docs record (matching a bare event never fires), so the
    /// test has to reproduce the prefixing rather than the raw push.
    ///
    /// The [`is_dirty`](External::is_dirty) gate is reproduced FIRST because the runtime applies it
    /// first (`drain_one` returns early on a clean external). Draining unconditionally is what let a
    /// missing `is_dirty` — a dead click path in the live app — pass every test in this module.
    fn drain_into_reducer(external: &mut PaletteExternal) -> usize {
        let mut intents = Vec::new();
        if external.is_dirty() {
            external.drain_intents(&mut |intent| intents.push(intent));
        }
        intents
            .iter()
            .map(|intent| Intent {
                tag: std::borrow::Cow::Owned(format!("{PALETTE_TAG}.{}", intent.tag_str())),
                payload: intent.payload.clone(),
            })
            .filter(handle_palette_intent)
            .count()
    }

    /// A one-pane in-process host, seeded so `use_terminal()` answers. Returns inside an `Owner`
    /// scope, so callers wrap the body in `Owner::new().run(..)`.
    fn seed_one_pane() {
        seed_panes(1);
    }

    /// The same, with `count` panes — so a test that pins WHICH pane was captured can focus one that
    /// is not slot 0, and a hard-coded target cannot pass it.
    fn seed_panes(count: usize) {
        let host = Host::new((40, 6));
        for _ in 0..count {
            host.spawn(
                cat(),
                "cat".to_owned(),
                40,
                6,
                sprag_terminal::PaneBirthHooks::default(),
            )
            .unwrap();
        }
        seed_terminal(host);
    }

    /// Publish focus on pane `i` through the focus manager's own carrier — the stand-in pinion's own
    /// `focus_state` tests use, and the one `crate::terminal::focused_pane` actually reads, so a test
    /// drives the REAL derivation rather than a seam beneath it.
    fn focus_pane(i: usize) {
        Owner::current()
            .expect("inside the owner scope")
            .focused_tag_signal()
            .set(Some(crate::terminal::pane_tag(i).to_owned()));
    }

    #[test]
    fn the_scrim_tag_is_a_composite_of_the_palette_tag() {
        // The backdrop reaches the palette's External only through the router's `#`-split, so the
        // two halves of its tag must stay in step. REVERT-PROOF: renaming either constant alone
        // fails here rather than silently producing a backdrop whose click goes nowhere.
        let (base, key) = PALETTE_SCRIM_TAG
            .split_once('#')
            .expect("the scrim tag is a composite");
        assert_eq!(base, PALETTE_TAG);
        assert_eq!(key, SCRIM_KEY);
    }

    #[test]
    fn the_matcher_ranks_word_starts_above_incidental_hits() {
        // "nw" is how a palette is actually driven: initials. It must land on the command whose
        // WORDS start with those letters, above one that merely contains them in order.
        let initials = fuzzy_score("nw", "New window").expect("initials match");
        let incidental = fuzzy_score("nw", "Unknown widget").expect("also a subsequence");
        assert!(
            initials > incidental,
            "word-start hits ({initials}) must outrank incidental ones ({incidental})"
        );
    }

    #[test]
    fn the_matcher_accepts_a_subsequence_and_refuses_a_non_match() {
        assert!(fuzzy_score("fis", "Find in scrollback").is_some());
        assert!(
            fuzzy_score("FIS", "Find in scrollback").is_some(),
            "matching is case-insensitive, since Shift upper-cases what is typed"
        );
        assert!(
            fuzzy_score("new win", "New window").is_some(),
            "whitespace in the query is dropped, not matched"
        );
        assert!(
            fuzzy_score("zzz", "New window").is_none(),
            "a query that is not a subsequence matches nothing"
        );
        assert_eq!(
            fuzzy_score("", "New window"),
            Some(0),
            "an empty query matches everything, so the palette opens on the whole catalog"
        );
    }

    #[test]
    fn filtering_keeps_catalog_order_for_equal_scores() {
        // Ties must be stable, or rows would shuffle between frames under an unchanged query.
        let catalog = vec![
            Command::NewWindow,
            Command::NewSession,
            Command::LastSession,
        ];
        let rows = visible(&catalog, "");
        assert_eq!(
            rows,
            vec![0, 1, 2],
            "an empty query lists the catalog as given"
        );
    }

    #[test]
    fn opening_freezes_the_catalog_and_captures_the_pane_and_closing_clears_both() {
        Owner::new().run(|| {
            seed_one_pane();
            assert!(!is_open(), "the palette starts closed");

            open(Some(0));
            assert!(is_open());
            assert_eq!(use_target_pane().get(), Some(0), "the pane is captured");
            let frozen = use_frozen_catalog().get().len();
            assert!(frozen > 0, "the catalog is frozen non-empty");
            assert!(
                use_frozen_catalog().get().contains(&Command::Find),
                "a captured pane means the pane commands are in the frozen list"
            );

            close();
            assert!(!is_open());
            assert!(
                use_frozen_catalog().get().is_empty(),
                "closing drops the frozen list rather than leaving it to be painted"
            );
        });
    }

    #[test]
    fn the_cursor_clamps_at_both_ends_of_the_visible_rows() {
        Owner::new().run(|| {
            seed_one_pane();
            open(Some(0));
            let rows = visible_rows().len();
            assert!(rows > 1, "the fixture offers several commands");

            move_cursor("ArrowUp");
            assert_eq!(
                use_cursor().get(),
                0,
                "ArrowUp at the top stays — it does not wrap to the worst match"
            );
            move_cursor("End");
            assert_eq!(use_cursor().get(), rows - 1);
            move_cursor("ArrowDown");
            assert_eq!(
                use_cursor().get(),
                rows - 1,
                "ArrowDown at the bottom stays"
            );
            move_cursor("Home");
            assert_eq!(use_cursor().get(), 0);
        });
    }

    #[test]
    fn the_external_reports_the_rows_it_paints_and_refuses_an_out_of_range_select() {
        Owner::new().run(|| {
            seed_one_pane();
            open(Some(0));
            let mut external = external();

            let count = match external.query("row_count").ok() {
                Some(IntrospectValue::Int(n)) => usize::try_from(n).unwrap(),
                other => panic!("row_count reads as an int, got {other:?}"),
            };
            assert_eq!(
                count,
                visible_rows().len(),
                "the External reports the same rows the paint walks"
            );
            assert!(matches!(
                external.query("row.0").ok(),
                Some(IntrospectValue::Text(_))
            ));
            assert!(matches!(
                external.query("cursor_command").ok(),
                Some(IntrospectValue::Text(_))
            ));

            assert!(
                external.invoke("select", IntrospectValue::Int(0)).is_ok(),
                "row 0 is selectable"
            );
            assert!(
                external
                    .invoke(
                        "select",
                        IntrospectValue::Int(i64::try_from(count).unwrap() + 1)
                    )
                    .is_err(),
                "a row past the end is refused rather than clamped silently"
            );
        });
    }

    /// The `open` verb: the whole two-step, from an RPC invoke to a palette that is UP over the pane
    /// the user was on — the path that makes this surface reachable without its chord, and therefore
    /// reachable by a headless smoke at all.
    ///
    /// The target is asserted to be pane 1 while pane 0 exists, which is what distinguishes the real
    /// [`focused_pane`] derivation from a hard-coded first slot.
    ///
    /// REVERT-PROOF: drop the `OPEN_EVENT` arm of [`handle_palette_intent`] and the palette never
    /// opens; have [`PaletteExternal::arm_open`] push nothing and the drain count falls to zero
    /// (`is_dirty` goes quiet, exactly as the dead click path did); open on `Some(0)` instead of the
    /// focused pane and the target assertion fails.
    #[test]
    fn the_open_verb_arms_a_request_the_reducer_performs() {
        Owner::new().run(|| {
            seed_panes(2);
            focus_pane(1);
            assert!(!is_open(), "the palette starts closed");

            let mut external = external();
            assert!(
                external.schema().field_for("open").is_some(),
                "the verb is DECLARED, so a caller can discover it rather than guess it"
            );
            assert_eq!(
                external.invoke("open", IntrospectValue::Null),
                Ok(IntrospectValue::Bool(true)),
                "the request is taken"
            );
            assert!(
                !is_open(),
                "the External itself opens nothing — it cannot reach the catalog's own state"
            );

            assert_eq!(
                drain_into_reducer(&mut external),
                1,
                "exactly one palette intent reached the reducer"
            );
            assert!(is_open(), "the reducer performed the armed open");
            assert_eq!(
                use_target_pane().get(),
                Some(1),
                "the FOCUSED pane is the capture, as the chord and the context menu resolve it"
            );
            assert_eq!(
                use_target_pane().get(),
                focused_pane(),
                "...which is one derivation, not a second authority"
            );
            assert!(
                use_frozen_catalog().get().contains(&Command::Find),
                "a captured pane means the pane commands are in the frozen list"
            );
        });
    }

    /// An open request over an ALREADY-OPEN palette is refused rather than honoured: re-opening
    /// rebuilds the frozen list and clears the query, and the chord cannot do that (the field
    /// swallows a `Ctrl+Shift+P` typed into an open palette). A half-typed query survives.
    ///
    /// REVERT-PROOF: drop the `is_open()` guard in [`open_on_request`] and BOTH the query and the
    /// captured target are wiped — the target back to `None`, since nothing here holds focus.
    #[test]
    fn an_open_request_refuses_to_reopen_over_a_typed_query() {
        Owner::new().run(|| {
            seed_one_pane();
            open(Some(0));
            use_text_edit_state(PALETTE_FIELD_TAG).set_text("new window".to_owned());

            let mut external = external();
            external
                .invoke("open", IntrospectValue::Null)
                .expect("the request is taken");
            assert_eq!(
                drain_into_reducer(&mut external),
                1,
                "the intent is the palette's whether or not it is honoured — claiming it is routing"
            );

            assert!(is_open(), "the palette is still up");
            assert_eq!(query(), "new window", "and the half-typed query survives");
            assert_eq!(
                use_target_pane().get(),
                Some(0),
                "as does the pane the open captured"
            );
        });
    }

    /// An open request while the destructive-command prompt is up is refused. `route_key` gates every
    /// key on that prompt before the chord is even considered, so this is the RPC face honouring the
    /// same rule — and [`crate::a11y`] states the two modals never being up together as an invariant
    /// its tree relies on.
    ///
    /// REVERT-PROOF: drop the [`crate::confirm::is_open`] guard in [`open_on_request`] and the
    /// palette opens over the prompt, stacking two modal dialogs.
    #[test]
    fn an_open_request_refuses_while_the_confirmation_prompt_is_up() {
        Owner::new().run(|| {
            seed_one_pane();
            let victim = use_terminal().slots.new_window();
            crate::confirm::run_or_arm(
                kill_of(&use_terminal().slots, &victim),
                Some(0),
                &use_terminal().slots,
            );
            assert!(crate::confirm::is_open(), "the prompt is up");

            let mut external = external();
            external
                .invoke("open", IntrospectValue::Null)
                .expect("the request is taken");
            assert_eq!(drain_into_reducer(&mut external), 1);

            assert!(
                !is_open(),
                "the palette stays down while the client is asking whether to destroy something"
            );
            assert!(
                crate::confirm::is_open(),
                "and the prompt it would have covered is untouched"
            );
            crate::confirm::dismiss();
        });
    }

    /// A row activation runs a REAL command end to end: open the palette, narrow the query to "New
    /// window", then drive the composite-send wire the way a click does — and the session gains a
    /// window. This is the whole chain (frozen catalog → filter → cursor → `Command::run` →
    /// `SlotView` → host) without a shell or an X server.
    ///
    /// REVERT-PROOF: neutering the `send` arm's `run_cursor_row()` leaves the window count at one;
    /// so does dropping the `NewWindow` arm of [`Command::run`].
    #[test]
    fn activating_a_row_runs_that_command_and_closes_the_palette() {
        Owner::new().run(|| {
            seed_one_pane();
            let slots = &use_terminal().slots;
            assert_eq!(slots.windows().len(), 1, "one window to begin with");

            open(Some(0));
            // Type the query, as the field's own edit dispatch would leave it.
            use_text_edit_state(PALETTE_FIELD_TAG).set_text("new window".to_owned());
            let titles = external().row_titles();
            assert_eq!(
                titles.first().map(String::as_str),
                Some("New window"),
                "the query ranks the command it names first, so row 0 is what a click runs"
            );

            let mut external = external();
            external
                .invoke("send", IntrospectValue::Text("0:PointerUp".to_owned()))
                .expect("the composite send is accepted");
            // The activation ARMS the run; the shell's intent drain is what performs it. Nothing
            // has happened yet, and asserting that is the point — it pins the two-step.
            assert_eq!(
                use_terminal().slots.windows().len(),
                1,
                "the External itself runs nothing (it cannot reach the command's own state)"
            );
            assert_eq!(
                drain_into_reducer(&mut external),
                1,
                "exactly one palette intent reached the reducer"
            );

            assert_eq!(
                use_terminal().slots.windows().len(),
                2,
                "the reducer ran the armed row and the window was created"
            );
            assert!(!is_open(), "running a command closes the palette");
        });
    }

    /// A DESTRUCTIVE row activates into the question, not into the act — the property that let
    /// `kill-window` / `kill-session` into the catalog at all.
    ///
    /// The cursor is placed on the row by identity rather than by typing a query, so the test pins the
    /// ACTIVATION and not the matcher's ranking.
    ///
    /// REVERT-PROOF: call `command.run(...)` directly from `run_cursor_row` (what it did before the
    /// confirmation front) and the window is gone with nobody asked — this fails on both counts.
    #[test]
    fn activating_a_destructive_row_asks_instead_of_acting() {
        Owner::new().run(|| {
            seed_one_pane();
            let victim = use_terminal().slots.new_window();
            let before = use_terminal().slots.windows().len();

            open(Some(0));
            let rows = visible_rows();
            let frozen = use_frozen_catalog().get();
            let at = rows
                .iter()
                .position(|&i| frozen[i] == kill_of(&use_terminal().slots, &victim))
                .expect("the palette offers a kill for the new window");
            use_cursor().set(at);

            let activated = run_cursor_row().expect("the row activates");
            assert_eq!(activated, kill_of(&use_terminal().slots, &victim));
            assert!(!is_open(), "the palette closes behind the activation");
            assert!(
                crate::confirm::is_open(),
                "and the confirmation is up in its place"
            );
            assert_eq!(
                use_terminal().slots.windows().len(),
                before,
                "NOTHING was killed by the activation itself"
            );
            crate::confirm::dismiss();
        });
    }

    #[test]
    fn a_click_on_the_backdrop_dismisses_without_running_anything() {
        Owner::new().run(|| {
            seed_one_pane();
            let before = use_terminal().slots.windows().len();
            open(Some(0));
            use_text_edit_state(PALETTE_FIELD_TAG).set_text("new window".to_owned());

            let mut external = external();
            external
                .invoke(
                    "send",
                    IntrospectValue::Text(format!("{SCRIM_KEY}:PointerUp")),
                )
                .expect("the backdrop's send is accepted");
            assert_eq!(
                drain_into_reducer(&mut external),
                1,
                "the backdrop's dismiss reaches the reducer too"
            );

            assert!(!is_open(), "a click beside the panel dismisses it");
            assert_eq!(
                use_terminal().slots.windows().len(),
                before,
                "and runs NOTHING — the cursor's row must not be activated by a dismiss"
            );
        });
    }

    /// What the panel actually PAINTS: the prompt glyph that makes the empty input visible, one row
    /// per command with its title, and the chord hints. The field paints no placeholder of its own
    /// (its `aria_label` is an AT name), so without the prompt the palette opens showing a blank
    /// strip — which is what the live screenshot caught. REVERT-PROOF: drop the prompt glyph from
    /// the input row and this fails.
    #[test]
    fn the_panel_paints_a_visible_prompt_and_a_row_per_command() {
        Owner::new().run(|| {
            seed_one_pane();
            open(Some(0));
            let panel = view_palette(
                PaletteFieldState::default(),
                &pinion_core::theme::Theme::dark(),
                (960, 600),
            )
            .expect("the palette paints while open");

            let text = walk_text(&panel);
            assert!(
                text.iter().any(|t| t == PROMPT_GLYPH),
                "the prompt glyph is painted, so the empty input is visible: {text:?}"
            );
            assert!(
                text.iter().any(|t| t == "Find in scrollback"),
                "a command's title is painted: {text:?}"
            );
            assert!(
                text.iter().any(|t| t == "Ctrl+Shift+F"),
                "and the chord that also runs it: {text:?}"
            );
            let tags = walk_tags(&panel);
            for expected in [PALETTE_SCRIM_TAG, PALETTE_PANEL_TAG, PALETTE_INPUT_TAG] {
                assert!(tags.iter().any(|t| t == expected), "{expected} is painted");
            }
            assert!(
                tags.iter().any(|t| t == &format!("{PALETTE_TAG}#0")),
                "row 0 paints under the composite tag its click routes on: {tags:?}"
            );
        });
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

    /// The rows' own container FITS inside the panel sized to hold it, at every row count the palette
    /// can paint — the property that makes wrapping the rows a structural change and not a visual one.
    ///
    /// Asserted as arithmetic between the two sizing functions rather than on painted rects, because a
    /// `Scene`'s rects are `Rect::default()` until the shell lays it out; the live geometry is checked
    /// by the pixel smoke instead. REVERT-PROOF: give the wrapper a padding, or drop the `- 1` from
    /// [`painted_rows_height`]'s gap count, and the fit fails at the top of the range.
    #[test]
    fn the_rows_container_fits_inside_the_panel_at_every_paintable_count() {
        for rows in 1..=MAX_VISIBLE_ROWS {
            let needed = PANEL_PADDING * 2 + FIELD_H + ROW_GAP + painted_rows_height(rows);
            assert!(
                needed <= panel_height(rows),
                "{rows} rows need {needed} but the panel is {}",
                panel_height(rows)
            );
        }
    }

    /// The hand-written row-tag table MUST equal the tag each row actually paints under. A rename of
    /// [`PALETTE_TAG`] would otherwise leave the accessible nodes pointing at tags nothing paints —
    /// nodes with no bounds, describing rows an AT could never reach.
    ///
    /// REVERT-PROOF: change one entry of the table (or `view_row`'s format) and this fails.
    #[test]
    fn every_accessible_row_tag_is_the_tag_that_row_paints_under() {
        for row in 0..MAX_VISIBLE_ROWS {
            assert_eq!(
                row_tag(row),
                format!("{PALETTE_TAG}#{row}"),
                "row {row}'s accessible tag must be its painted tag"
            );
        }
    }

    /// The accessible tree: a MODAL dialog holding the query combobox and a `listbox` of the matching
    /// commands, the cursor row carrying both `aria-selected` and the active-descendant focus, and
    /// every option's label teaching the chord the sighted user reads.
    ///
    /// REVERT-PROOF: drop `with_modal` and the modal assertion fails; mark the wrong row selected and
    /// the cursor assertions fail; drop the `hint` from the label and the chord assertion fails.
    #[test]
    fn the_accessible_tree_is_a_modal_dialog_over_a_listbox_of_commands() {
        Owner::new().run(|| {
            seed_one_pane();
            assert!(
                palette_access_nodes(Some(PALETTE_FIELD_TAG)).is_empty(),
                "a closed palette advertises nothing at all"
            );

            open(Some(0));
            use_text_edit_state(PALETTE_FIELD_TAG).set_text("find".to_owned());
            let nodes = palette_access_nodes(Some(PALETTE_FIELD_TAG));

            let dialog = &nodes[0];
            assert_eq!(dialog.tag, PALETTE_PANEL_TAG);
            assert_eq!(dialog.role, AriaRole::Dialog);
            assert!(dialog.modal, "a palette is a modal boundary for an AT too");
            assert_eq!(dialog.name.as_deref(), Some(PALETTE_PLACEHOLDER));

            let field = &nodes[1];
            assert_eq!(field.tag, PALETTE_FIELD_TAG);
            assert_eq!(field.role, AriaRole::EditableComboBox);
            assert!(field.state.focused, "the field owns the keyboard");
            match &field.value {
                Some(AccessValue::Text(text)) => assert_eq!(text, "find", "the live query"),
                other => panic!("the combobox value is its text, got {other:?}"),
            }

            let list = nodes
                .iter()
                .find(|node| node.role == AriaRole::Listbox)
                .expect("the matching commands are a listbox");
            assert_eq!(list.tag, PALETTE_ROWS_TAG);

            let options: Vec<&AccessNode> = nodes
                .iter()
                .filter(|node| node.role == AriaRole::ListBoxOption)
                .collect();
            assert_eq!(
                options.len(),
                visible_rows().len().min(MAX_VISIBLE_ROWS),
                "one option per PAINTED row"
            );
            assert!(
                options[0].selected == Some(true) && options[0].state.focused,
                "the cursor row is the selected option and the active descendant"
            );
            assert!(
                options
                    .iter()
                    .skip(1)
                    .all(|node| node.selected != Some(true)),
                "and it is the only selected one"
            );
            assert!(
                options
                    .iter()
                    .any(|node| node
                        .name
                        .as_deref()
                        .is_some_and(|name| name.contains("Find in scrollback")
                            && name.contains("Ctrl+Shift+F"))),
                "an option teaches the chord alongside the title: {:?}",
                options
                    .iter()
                    .map(|n| n.name.as_deref())
                    .collect::<Vec<_>>()
            );
        });
    }

    /// The active descendant is claimed ONLY while the field actually holds focus — an active option
    /// on an unfocused surface would misreport where the next keystroke goes.
    ///
    /// REVERT-PROOF: drop the `field_has_focus &&` guard and this fails.
    #[test]
    fn no_option_is_active_while_the_field_does_not_hold_focus() {
        Owner::new().run(|| {
            seed_one_pane();
            open(Some(0));
            let elsewhere = palette_access_nodes(Some("sprag_gui.stablist"));
            assert!(
                elsewhere
                    .iter()
                    .filter(|node| node.role == AriaRole::ListBoxOption)
                    .all(|node| !node.state.focused),
                "no active descendant while focus is on another surface"
            );
            assert!(
                elsewhere
                    .iter()
                    .any(|node| node.role == AriaRole::ListBoxOption && node.selected == Some(true)),
                "...though the cursor row is still the SELECTED one"
            );
        });
    }

    #[test]
    fn a_query_that_matches_nothing_paints_the_honest_empty_state() {
        Owner::new().run(|| {
            seed_one_pane();
            open(Some(0));
            use_text_edit_state(PALETTE_FIELD_TAG).set_text("zzzzzz".to_owned());
            assert!(visible_rows().is_empty());
            // Nothing to run, and `Enter` must be a no-op rather than a panic on an empty list.
            assert!(run_cursor_row().is_none());
            assert!(
                is_open(),
                "a fruitless Enter leaves the palette up to be retyped"
            );
        });
    }
}
