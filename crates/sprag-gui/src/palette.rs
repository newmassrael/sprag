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
//! * a query-only introspection face, so `open` is a first-class question over RPC.
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

use pinion_core::composite_tag::{send_activation_index, send_activation_key};
use pinion_core::external::{
    Backend, BackendFallback, BackendSupport, External, ExternalIntrospect, InterveneError,
    IntrospectSchema, IntrospectValue, InvokeError, RepaintOwner, SchemaArg, SchemaField,
    ThreadOwnership,
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
use pinion_core::widgets::modal::{ModalState, modal_introspection_extra, use_modal};
use pinion_core::widgets::text_edit::use_text_edit_state;
use pinion_core::widgets::text_field::{TextFieldExternal, TextFieldState};
use pinion_core::{Modifiers, Scene};
use pinion_widget_paint::scrim::{M3_SCRIM_ALPHA, scrim_backdrop, scrim_fill};
use pinion_widget_paint::text_field as tf_paint;

use crate::command::{self, Command};
use crate::terminal::use_terminal;

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
const PALETTE_PROJECT_ERROR_KEY: &str = "sprag_gui.palette.project_error";

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

/// Why the target pane's project contributed no commands, captured with the catalog it could not
/// contribute to. `None` when there is no project or its config is fine.
fn use_project_error() -> Signal<Option<Rc<str>>> {
    Owner::current()
        .expect("use_project_error() requires an active Owner scope")
        .cache(PALETTE_PROJECT_ERROR_KEY, || Signal::new(None))
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
    use_project_error().set(catalog.project_error.map(Rc::from));
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
    use_project_error().set(None);
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
                    SchemaField::new("select", "int"),
                    SchemaField::new("execute", "json"),
                    SchemaField::new("send", "string"),
                ]
            },
        )
    }

    fn query(&self, path: &str) -> Option<IntrospectValue> {
        let rows = self.row_titles();
        match path {
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

    fn intervene(&mut self, path: &str, value: IntrospectValue) -> Result<(), InterveneError> {
        match path {
            "cursor" => match value {
                IntrospectValue::Int(n) => {
                    let i = usize::try_from(n).map_err(|_| InterveneError::OutOfRange)?;
                    if self.select(i) {
                        Ok(())
                    } else {
                        Err(InterveneError::OutOfRange)
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
        match path {
            "select" => match args {
                IntrospectValue::Int(n) => {
                    let i = usize::try_from(n).map_err(|_| InvokeError::TypeMismatch)?;
                    if self.select(i) {
                        Ok(IntrospectValue::Int(n))
                    } else {
                        Err(InvokeError::Rejected)
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
    if let Some(report) = use_project_error().get() {
        children.push(Scene::Text(TextNode::styled(
            format!("{report}"),
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
    children.extend(
        rows.iter()
            .take(MAX_VISIBLE_ROWS)
            .enumerate()
            .filter_map(|(row, &index)| {
                catalog
                    .get(index)
                    .map(|command| view_row(row, command, row == cursor, theme))
            }),
    );

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
        let host = Host::new((40, 6));
        host.spawn(cat(), "cat".to_owned(), 40, 6, None, None)
            .unwrap();
        seed_terminal(host);
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

            let count = match external.query("row_count") {
                Some(IntrospectValue::Int(n)) => usize::try_from(n).unwrap(),
                other => panic!("row_count reads as an int, got {other:?}"),
            };
            assert_eq!(
                count,
                visible_rows().len(),
                "the External reports the same rows the paint walks"
            );
            assert!(matches!(
                external.query("row.0"),
                Some(IntrospectValue::Text(_))
            ));
            assert!(matches!(
                external.query("cursor_command"),
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
                .position(|&i| frozen[i] == Command::KillWindow(victim.clone()))
                .expect("the palette offers a kill for the new window");
            use_cursor().set(at);

            let activated = run_cursor_row().expect("the row activates");
            assert_eq!(activated, Command::KillWindow(victim.clone()));
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
