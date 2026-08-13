//! Find-in-scrollback: the search bar, its navigation, and the match highlight.
//!
//! The SEARCH itself is not here — it runs where the cells are
//! ([`sprag_vt::Screen::find`], reached through the host's `find.<needle>` query, see
//! [`crate::slotview::SlotView::pane_find`]). This module is the display half: a text field to type
//! the needle into, the jump between matches, and the colours laid over the grid.
//!
//! ## Two search LANGUAGES, one field
//!
//! The field's text is read either literally or as a regular expression
//! ([`use_find_regex`], the `Regex` toggle / `Alt+R`), and that choice picks between two DISTINCT
//! queries — `find.<needle>` and `regex.<pattern>` — never one query with a mode argument. The same
//! characters mean different things in the two languages, which is why the wire keeps them at separate
//! addresses and why the mode must be VISIBLE here: nothing else on screen would say which language
//! the user is typing. A pattern the engine refuses is reported as a refusal ([`use_find_error`]),
//! never as an empty result, for the same reason.
//!
//! ## What is client state and what is not
//!
//! The needle, the open flag, the current match index and the last answer are all THIS client's —
//! two GUIs attached to one session search independently, exactly as two browser tabs do, and
//! nothing about a search reaches the session. That is also why the search is a READ on the wire: it
//! changes nothing, so a keystroke here wakes no other client.
//!
//! ## Freshness, honestly stated
//!
//! Matches are re-queried when the NEEDLE changes (and when the bar opens), not per frame — a search
//! over a full scrollback on every paint would be a socket round trip at frame rate. So a match list
//! describes the pane as it was at the last keystroke; output printed after that is not in it until
//! the user edits the needle or re-opens the bar. That is the same contract a browser's find bar
//! has on a live-updating page, and it is what keeps the paint path free of IO.

use std::rc::Rc;

use pinion_core::WidgetStateName;
use pinion_core::external::IntrospectValue;
use pinion_core::reactive::{Owner, Signal};
use pinion_core::theme::Theme;
use pinion_core::widget_core::ExtraExternal;
use pinion_core::widgets::caret_blink::use_caret_blink;
use pinion_core::widgets::checkbox::{CheckboxExternal, CheckboxState};
use pinion_core::widgets::text_edit::use_text_edit_state;
use pinion_core::widgets::text_field::{TextFieldExternal, TextFieldState};
use pinion_core::{Modifiers, Scene, TermColor};
use pinion_widget_paint::checkbox as pw_checkbox;
use pinion_widget_paint::text_field as tf_paint;
use sprag_grid::MatchSpan;
use sprag_host::PaneMatch;

use crate::terminal::{pane_tag, use_terminal};

/// The find field's tag: the External's registration key, the `use_text_edit_state` /
/// `use_caret_blink` key, the paint tag, and the focus tag — one string for one surface.
pub(crate) const FIND_FIELD_TAG: &str = "sprag_find";

/// The regex-mode checkbox's tag: its External registration key, its paint tag, and the tag its
/// `checked` intent arrives under — one string for one surface, like [`FIND_FIELD_TAG`].
pub(crate) const FIND_REGEX_TAG: &str = "sprag_find_regex";

/// `Owner::cache` key for the searched pane (`None` = the bar is closed).
const FIND_PANE_KEY: &str = "sprag_gui.find.pane";
/// `Owner::cache` key for the last answered match list.
const FIND_MATCHES_KEY: &str = "sprag_gui.find.matches";
/// `Owner::cache` key for the index of the CURRENT match within that list.
const FIND_INDEX_KEY: &str = "sprag_gui.find.index";
/// `Owner::cache` key for whether the needle is read as a REGULAR EXPRESSION.
const FIND_REGEX_KEY: &str = "sprag_gui.find.regex";
/// `Owner::cache` key for the engine's message about a pattern it refused (`None` = it searched).
const FIND_ERROR_KEY: &str = "sprag_gui.find.error";

/// The field's ACCESSIBLE NAME. Not a visible placeholder, despite the value reading like one:
/// [`tf_paint::view_field`] takes this argument only for `with_aria_label` and paints no hint text of
/// its own, so an empty bar shows an empty field. (Corrected after the command palette hit the same
/// assumption and its screenshot showed a blank input.)
const FIND_PLACEHOLDER: &str = "Find";

/// The match highlight colours, as ANSI palette indices so they resolve through the pane's OWN live
/// palette (an `OSC 4` re-theme moves them too) rather than pinning RGB the terminal never chose.
///
/// Cyan-on-black for every match and magenta-on-black for the current one is tmux's
/// `copy-mode-match-style` / `copy-mode-current-match-style` pairing — the convention a terminal user
/// already reads, and two colours rather than one because "which match am I on" is the question
/// next/prev exists to answer.
const MATCH_FG: TermColor = TermColor::Indexed(0);
const MATCH_BG: TermColor = TermColor::Indexed(6);
const CURRENT_MATCH_BG: TermColor = TermColor::Indexed(5);

/// The pane the bar is searching, or `None` when it is closed. Read by the paint (subscribing it),
/// so opening / closing repaints.
pub(crate) fn use_find_pane() -> Signal<Option<usize>> {
    Owner::current()
        .expect("use_find_pane() requires an active Owner scope")
        .cache(FIND_PANE_KEY, || Signal::new(None))
        .as_ref()
        .clone()
}

/// The matches the last query answered, in reading order.
pub(crate) fn use_find_matches() -> Signal<Rc<Vec<PaneMatch>>> {
    Owner::current()
        .expect("use_find_matches() requires an active Owner scope")
        .cache(FIND_MATCHES_KEY, || Signal::new(Rc::new(Vec::new())))
        .as_ref()
        .clone()
}

/// The index of the CURRENT match within [`use_find_matches`] (meaningless when that is empty).
pub(crate) fn use_find_index() -> Signal<usize> {
    Owner::current()
        .expect("use_find_index() requires an active Owner scope")
        .cache(FIND_INDEX_KEY, || Signal::new(0))
        .as_ref()
        .clone()
}

/// Whether the field's text is read as a REGULAR EXPRESSION rather than literal characters.
///
/// This client's state, like the needle — and it is a MODE only at this surface, never on the wire:
/// [`refresh`] picks between two distinct queries from it, so the address always says which language
/// it carries. It has to be visible in the bar for the same reason the two addresses exist — the same
/// characters mean different things in the two languages, so the user must be able to see which one
/// they are typing.
pub(crate) fn use_find_regex() -> Signal<bool> {
    Owner::current()
        .expect("use_find_regex() requires an active Owner scope")
        .cache(FIND_REGEX_KEY, || Signal::new(false))
        .as_ref()
        .clone()
}

/// The regex engine's explanation of a pattern it REFUSED, or `None` when the last search ran.
///
/// Kept as its own signal rather than folded into an empty match list, which is the same distinction
/// the wire refuses to collapse: "your pattern is wrong" and "nothing matched" are different answers,
/// and a bar that showed "no matches" for a half-typed `(foo` would be lying about a search it never
/// performed.
pub(crate) fn use_find_error() -> Signal<Option<Rc<str>>> {
    Owner::current()
        .expect("use_find_error() requires an active Owner scope")
        .cache(FIND_ERROR_KEY, || Signal::new(None))
        .as_ref()
        .clone()
}

/// The find field's live text — the needle.
pub(crate) fn needle() -> String {
    use_text_edit_state(FIND_FIELD_TAG).text()
}

/// The find field as an extra External, registered every reconcile at [`FIND_FIELD_TAG`] (pinion
/// R689 preserves its live state by tag, like the context menu's).
///
/// The text state and caret blink are the TAG-KEYED hooks, so the External, the painter
/// ([`tf_paint::view_field`], which resolves them itself) and [`needle`] all reach the same
/// instances — the wiring pinion's own editable-combobox binding uses.
pub(crate) fn create_find_external() -> ExtraExternal {
    ExtraExternal::new(
        FIND_FIELD_TAG.to_owned(),
        Box::new(
            TextFieldExternal::new()
                .attach_state(use_text_edit_state(FIND_FIELD_TAG))
                .attach_blink(use_caret_blink(FIND_FIELD_TAG)),
        ),
    )
}

/// The regex toggle as an extra External, registered every reconcile at [`FIND_REGEX_TAG`].
///
/// A real pinion [`CheckboxExternal`] rather than a hand-painted glyph: it comes focusable, hoverable,
/// keyboard-activatable and AT-named, it publishes itself in `scene/snapshot`, and — the property this
/// project needs most — it is drivable BY INTENT from its tag, so the toggle is testable headlessly
/// instead of only by synthesising a click at a pixel.
pub(crate) fn create_regex_external() -> ExtraExternal {
    ExtraExternal::new(FIND_REGEX_TAG.to_owned(), Box::new(CheckboxExternal::new()))
}

/// Handle the regex checkbox's `checked` intent — the click / Space path. Returns whether it was ours.
///
/// The checkbox's statechart owns its own interaction state; the CHECKED value it reports is mirrored
/// into [`use_find_regex`], which is this client's SSOT for the search language. Flipping it re-queries
/// immediately: the same characters now address a different query, so the matches on screen describe a
/// search that is no longer the one being asked.
pub(crate) fn handle_regex_intent(intent: &pinion_core::Intent) -> bool {
    let Some((who, _event)) = intent.tag_str().rsplit_once('.') else {
        return false;
    };
    if who != FIND_REGEX_TAG {
        return false;
    }
    if let IntrospectValue::Bool(on) = &intent.payload {
        use_find_regex().set(*on);
    } else {
        // No boolean payload (a checkbox intent that is not the `checked` one): treat the activation
        // as a plain toggle rather than dropping it, so the surface can never latch.
        use_find_regex().set(!use_find_regex().get());
    }
    refresh();
    true
}

/// The bar's interaction posture for the paint — the `Copy` snapshot the shell caches into the
/// binding's `State`, read out of the model scene like the context menu's own.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) struct FindFieldState {
    /// The field's SCXML interaction state (idle / hover / focused / editing).
    pub(crate) field: TextFieldState,
    /// The caret's byte offset within the needle.
    pub(crate) caret: u32,
    /// The regex checkbox's own SCXML interaction state (idle / hover / pressed / focused), so it
    /// paints its hover and focus like every other pinion control. Its CHECKED value is not here —
    /// that lives in [`use_find_regex`], this client's state, which the checkbox's intent writes.
    pub(crate) regex_box: CheckboxState,
}

/// Project the bar's posture out of the model scene (the `read_state` seam). Defaults when the
/// Externals are absent (before the first reconcile registers them).
pub(crate) fn read_field_state(scene: &Scene) -> FindFieldState {
    let (field, caret) = tf_paint::read_text_field_state(scene, FIND_FIELD_TAG);
    FindFieldState {
        field,
        caret,
        regex_box: read_checkbox_state(scene, FIND_REGEX_TAG),
    }
}

/// One checkbox External's SCXML interaction state, off the standard introspect `"state"` slot —
/// pinion's own settings-panel binding reads its checkboxes exactly this way (there is no
/// `read_checkbox_state` sibling to `read_text_field_state` in `pinion-widget-paint`).
///
/// `Idle` when the External is not wired yet, which is the create-then-paint window every External
/// has on its first frame.
fn read_checkbox_state(scene: &Scene, tag: &str) -> CheckboxState {
    scene
        .find_external_with_tag(tag)
        .and_then(|node| node.handle.introspect())
        // ⚠ A REFUSAL AND AN ABSENCE COLLAPSE HERE, and that is this reader's own rule rather than
        // a loss from pinion R1674's widening: the fallback below is `Idle` for the create-then-
        // paint window, so *"the External is not wired yet"* and *"it refused the read"* are the
        // same instruction to a painter — draw the resting state. A caller that had to tell them
        // apart would be reading this signal for something it does not answer.
        .and_then(|intro| intro.query("state").ok())
        .and_then(|value| match value {
            IntrospectValue::Text(name) => Some(CheckboxState::from_name_or_default(&name)),
            _ => None,
        })
        .unwrap_or(CheckboxState::Idle)
}

/// Open the bar on pane `pane` and focus its field (`Ctrl+Shift+F`).
///
/// The previous needle is KEPT — reopening to search for the same thing again is the common case,
/// and the field selects nothing, so typing replaces nothing by surprise. Re-queries immediately, so
/// a re-open searches the pane's CURRENT content rather than showing the stale answer from last time.
pub(crate) fn open(pane: usize) {
    use_find_pane().set(Some(pane));
    refresh();
    pinion_core::focus_request::request(FIND_FIELD_TAG);
}

/// Close the bar and return focus to the pane it was searching. The needle survives for the next
/// open; the matches do not (they would paint a highlight for a bar that is gone).
pub(crate) fn close() {
    let pane = use_find_pane().get();
    use_find_pane().set(None);
    use_find_matches().set(Rc::new(Vec::new()));
    use_find_index().set(0);
    if let Some(pane) = pane {
        pinion_core::focus_request::request(pane_tag(pane));
    }
}

/// Whether `tag` is the find field — the router's gate, mirroring `stabs::is_sidebar_focus`.
pub(crate) fn is_find_focus(tag: &str) -> bool {
    tag == FIND_FIELD_TAG
}

/// Route a key while the find field holds focus. Returns whether it was consumed.
///
/// `Escape` closes, `Enter` / `Shift+Enter` step to the next / previous match, and everything else
/// is delegated to the field's own edit dispatch through pinion's
/// [`forward_key_to_field`](pinion_core::forward_key_to_field) SSOT — which drives the External (so
/// its statechart, caret and blink stay authoritative) rather than poking the text state behind its
/// back. A key the field recognizes re-queries: the needle changed, so the old matches describe a
/// different search.
pub(crate) fn handle_key(scene: &mut Scene, key: &str, modifiers: Modifiers) -> bool {
    // `Alt+R` toggles the search LANGUAGE without leaving the field — VS Code's / IntelliJ's binding
    // for the same toggle, so it needs no explanation. Checked before the field's own dispatch, since
    // the field would otherwise type an `r`.
    if modifiers.alt && key.eq_ignore_ascii_case("r") {
        use_find_regex().set(!use_find_regex().get());
        refresh();
        return true;
    }
    match key {
        "Escape" => {
            close();
            true
        }
        "Enter" => {
            step(if modifiers.shift { -1 } else { 1 });
            true
        }
        _ => {
            let before = needle();
            let handled = pinion_core::forward_key_to_field(scene, FIND_FIELD_TAG, key, modifiers);
            if handled && needle() != before {
                refresh();
            }
            handled
        }
    }
}

/// Re-run the search for the current needle on the searched pane and park the current match on the
/// first one at or below the view — the "start from where I am looking" rule a find bar wants, so
/// the first `Enter` moves forward from the visible region rather than jumping to the top of history.
pub(crate) fn refresh() {
    let Some(pane) = use_find_pane().get() else {
        return;
    };
    // Which LANGUAGE the needle is in decides which QUERY is sent — the one choice, made here, exactly
    // as the CLI makes it once before its sweep. Neither call takes a mode.
    let slots = &use_terminal().slots;
    let found = if use_find_regex().get() {
        slots.pane_find_regex(pane, &needle())
    } else {
        slots.pane_find(pane, &needle())
    };
    if found.truncated {
        tracing::debug!(
            target: "sprag_gui::find",
            pane,
            matches = found.matches.len(),
            "the search hit its cap; later matches were not scanned",
        );
    }
    // A refused pattern is NOT an empty result: keep the engine's message and drop the stale matches,
    // so the bar says why it did not search rather than claiming it found nothing.
    use_find_error().set(found.error.as_deref().map(Rc::from));
    let top = crate::scrollbar::use_pane_scroll(pane).offset_y();
    let index = first_at_or_after(&found.matches, top);
    use_find_matches().set(Rc::new(found.matches));
    use_find_index().set(index);
    scroll_to_current(pane);
}

/// Move the current match by `delta` (wrapping) and scroll it into view — `Enter` / `Shift+Enter`.
fn step(delta: isize) {
    let Some(pane) = use_find_pane().get() else {
        return;
    };
    let matches = use_find_matches().get();
    let Some(next) = wrapped_index(use_find_index().get(), delta, matches.len()) else {
        return;
    };
    use_find_index().set(next);
    scroll_to_current(pane);
}

/// Scroll `pane` so the current match is visible, if it is not already.
fn scroll_to_current(pane: usize) {
    let matches = use_find_matches().get();
    let Some(hit) = matches.get(use_find_index().get()) else {
        return;
    };
    let scroll = crate::scrollbar::use_pane_scroll(pane);
    let rows = use_terminal().slots.pane_scroll_facts(pane).visible_rows;
    if let Some(target) = scroll_target(hit.line, scroll.offset_y(), rows, scroll.max().1) {
        scroll.scroll_to(0, target);
    }
}

/// The index of the first match at or after view-top line `top`, or `0` when none is (wrapping to
/// the oldest — a search whose every match is above the view starts at its first).
fn first_at_or_after(matches: &[PaneMatch], top: i32) -> usize {
    let top = usize::try_from(top.max(0)).unwrap_or(0);
    matches.iter().position(|m| m.line >= top).unwrap_or(0)
}

/// `current + delta` over `len` matches, wrapping at both ends; `None` when there is nothing to step
/// through. Pure — the wrap is the kind of arithmetic that is wrong at exactly one end.
fn wrapped_index(current: usize, delta: isize, len: usize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let len_i = isize::try_from(len).unwrap_or(isize::MAX);
    let next = (isize::try_from(current).unwrap_or(0) + delta).rem_euclid(len_i);
    usize::try_from(next).ok()
}

/// The scroll `offset_y` that brings logical `line` into a `rows`-tall view whose top is `top`, or
/// `None` when it is already visible (scrolling then would move the text under the user for nothing).
///
/// An off-screen match is CENTRED rather than parked at the top edge: a match on the first row with
/// no context above reads as "the top of the pane", not as a found line. Clamped to `[0, max]`, which
/// is what makes centring safe near either end of the history.
fn scroll_target(line: usize, top: i32, rows: u16, max: i32) -> Option<i32> {
    let line = i32::try_from(line).unwrap_or(i32::MAX);
    let rows = i32::from(rows).max(1);
    if line >= top && line < top.saturating_add(rows) {
        return None;
    }
    let centred = line.saturating_sub(rows / 2).clamp(0, max.max(0));
    (centred != top).then_some(centred)
}

/// The match spans to highlight on pane `pane`'s VISIBLE grid, plus the current match's own span.
///
/// The mapping is the whole reason the host answers on the retained-row axis: the view's top row
/// IS scroll `offset_y` (the axis `prompt_positions` and this share), so a match starting on row
/// `R` paints on grid row `R - offset_y` — and rows outside `0..rows` are simply not painted,
/// which is how a scrolled-away match costs nothing.
///
/// A match that crossed a soft wrap paints as SEVERAL spans, one per row it covers: `hit.row` and
/// `hit.cols` for its head, then `hit.wrapped`, each of those starting at column 0 of the next row
/// down. Painting only the head would leave a person looking at half a highlighted word — and the
/// spans are clipped independently, so a match half-scrolled off the top still highlights the half
/// that is on screen.
pub(crate) fn visible_spans(pane: usize, top: i32, rows: u16) -> (Vec<MatchSpan>, Vec<MatchSpan>) {
    if use_find_pane().get() != Some(pane) {
        return (Vec::new(), Vec::new());
    }
    let matches = use_find_matches().get();
    let current = use_find_index().get();
    let mut all = Vec::new();
    let mut on_current = Vec::new();
    for (index, hit) in matches.iter().enumerate() {
        let target = if index == current {
            &mut on_current
        } else {
            &mut all
        };
        for (offset, (col, cols)) in std::iter::once((hit.col, hit.cols))
            .chain(hit.wrapped.iter().map(|cols| (0, *cols)))
            .enumerate()
        {
            if let Some(row) = row_of(hit.row.saturating_add(offset), top, rows) {
                target.push((row, col, cols));
            }
        }
    }
    (all, on_current)
}

/// The visible grid row that retained row `retained` paints on for a view whose top is `top`, or
/// `None` when it is off-screen.
fn row_of(retained: usize, top: i32, rows: u16) -> Option<u16> {
    let line = i64::try_from(retained).unwrap_or(i64::MAX);
    let row = line - i64::from(top.max(0));
    (row >= 0 && row < i64::from(rows)).then(|| u16::try_from(row).unwrap_or(u16::MAX))
}

/// Lay the match highlights over `cells` — every match in the match colour, the current one in the
/// current-match colour, so "which one is next" is visible without moving the view.
#[must_use]
pub(crate) fn overlay_matches(
    cells: pinion_core::GridBuffer,
    pane: usize,
    top: i32,
    rows: u16,
) -> pinion_core::GridBuffer {
    let (others, current) = visible_spans(pane, top, rows);
    let cells = sprag_grid::overlay_matches(cells, &others, MATCH_FG, MATCH_BG);
    sprag_grid::overlay_matches(cells, &current, MATCH_FG, CURRENT_MATCH_BG)
}

/// The bar's own paint: the field plus a match counter, or nothing when the bar is closed.
///
/// Absolutely positioned at the window's top-right — out of the way of a shell prompt (bottom-left)
/// and where every browser's find bar lives, so it needs no explanation.
pub(crate) fn view_bar(state: FindFieldState, theme: &Theme, window: (u32, u32)) -> Option<Scene> {
    use pinion_core::scene::{ContainerNode, Rect, TextNode};
    use pinion_core::style::{LayoutStyle, TextStyle};

    use_find_pane().get()?;
    let matches = use_find_matches().get();
    // The refusal outranks the count: a pattern the engine rejected was never searched, so reporting
    // "no matches" for it would describe a search that did not happen. The message is the engine's own
    // ("unclosed group"), which is what tells the user WHICH character to fix.
    let counter = if let Some(error) = use_find_error().get() {
        format!("bad pattern: {error}")
    } else if matches.is_empty() {
        if needle().is_empty() {
            String::new()
        } else {
            "no matches".to_owned()
        }
    } else {
        format!("{}/{}", use_find_index().get() + 1, matches.len())
    };
    let style = tf_paint::TextFieldStyle {
        field_w: FIND_FIELD_W,
        field_h: FIND_FIELD_H,
        ..tf_paint::TextFieldStyle::m3_filled()
    };
    let field = tf_paint::view_field(
        FIND_FIELD_TAG,
        state.field,
        state.caret,
        theme,
        &style,
        FIND_PLACEHOLDER,
    );
    let label = Scene::Text(TextNode::styled(
        counter,
        Rect::default(),
        TextStyle::new().with_size_px(13),
    ));
    // The mode has to be VISIBLE, not just bound to a key: with two search languages behind one field,
    // the same characters mean different things, and nothing else on screen says which is in force.
    // The label is a WORD rather than VS Code's `.*` glyph because it doubles as the AT accessible name
    // and as the label an RPC consumer reads.
    let regex_box = pw_checkbox::view_checkbox(
        FIND_REGEX_TAG,
        state.regex_box,
        use_find_regex().get(),
        theme,
        &pw_checkbox::CheckboxStyle::m3_filled(),
        REGEX_LABEL,
    );
    let x = window.0.saturating_sub(FIND_FIELD_W + FIND_BAR_MARGIN * 2);
    Some(Scene::Container(
        ContainerNode::new(vec![field, regex_box, label])
            .with_layout(LayoutStyle::new().with_absolute_position(x, FIND_BAR_MARGIN)),
    ))
}

/// The bar's field width in logical pixels — wide enough for a real search string, narrow enough to
/// leave the pane readable behind it.
const FIND_FIELD_W: u32 = 280;
/// The bar's field height in logical pixels (M3 filled single-line).
const FIND_FIELD_H: u32 = 40;
/// The bar's inset from the window's top-right corner.
const FIND_BAR_MARGIN: u32 = 12;
/// The regex toggle's label — the visible text AND its accessible name.
const REGEX_LABEL: &str = "Regex";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TerminalViewer;
    use crate::terminal::seed_terminal;
    use pinion_core::WidgetCore;
    use pinion_core::scene::ContainerNode;
    use sprag_host::Host;
    use sprag_terminal::{CommandBuilder, PanePtyHandle};
    use std::time::{Duration, Instant};

    /// A match on a line that occupies ONE row — the ordinary case. A wrapping match is built
    /// field by field, because `row` and `wrapped` are what such a test is about.
    fn hit(line: usize, col: u16, cols: u16) -> PaneMatch {
        PaneMatch {
            line,
            row: line,
            col,
            cols,
            wrapped: Vec::new(),
        }
    }

    /// A long-lived `cat` pane: it echoes what is written to it, so a test can put known text on
    /// the screen and then search for it.
    fn cat() -> CommandBuilder {
        let mut c = CommandBuilder::new("/bin/sh");
        c.arg("-c");
        c.arg("cat");
        c.env("TERM", "dumb");
        c
    }

    /// Poll `handle`'s row 0 until it contains `needle`, so the search runs against text that has
    /// actually reached the emulator (not a race with the echo).
    fn wait_for_row0(handle: &PanePtyHandle, needle: &str) {
        wait_for_row(handle, 0, needle);
    }

    /// [`wait_for_row0`] for any row — a line that WRAPS lands its tail on the row below, and
    /// waiting on row 0 would let the search run before the continuation had arrived.
    fn wait_for_row(handle: &PanePtyHandle, row: u16, needle: &str) {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if handle.with_screen(|s| s.row_text(row)).contains(needle) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!(
            "the pane never echoed {needle:?} onto row {row}; row {row} holds {:?}",
            handle.with_screen(|s| s.row_text(row))
        );
    }

    /// The whole find bar over a REAL pane: open -> search -> navigate -> close, driven through the
    /// same entry points the shell drives (`TerminalViewer::apply_key`).
    ///
    /// This is the slice's vertical: `SlotView::pane_find` -> `Host::pane_find` -> `Screen::find`,
    /// then the logical-line answer mapped back onto the visible grid as a highlight span. The pane
    /// echoes `err a err`, which has TWO matches on one row — so the span mapping, the match count
    /// and the next/prev step are all observable at once.
    #[test]
    fn the_find_bar_searches_navigates_and_closes_over_a_real_pane() {
        let host = Host::new((40, 6));
        let id = host
            .spawn(
                cat(),
                "cat".to_owned(),
                40,
                6,
                sprag_terminal::PaneBirthHooks::default(),
            )
            .unwrap();
        let handle = host.pane_handle(id).expect("pane handle");
        let owner = Owner::new();
        owner.run(|| {
            seed_terminal(host);
            let terminal = use_terminal();
            assert!(terminal.slots.send_text(0, "err a err"), "seed the pane");
            wait_for_row0(&handle, "err a err");

            // Type the needle the way `commit_selection` does upstream, then open the bar (which
            // queries immediately).
            use_text_edit_state(FIND_FIELD_TAG).seed("err".to_owned());
            open(0);

            let matches = use_find_matches().get();
            assert_eq!(
                matches.as_slice(),
                &[hit(0, 0, 3), hit(0, 6, 3)],
                "both occurrences on row 0, in cell columns",
            );
            let (others, current) = visible_spans(0, 0, 6);
            assert_eq!(
                current,
                vec![(0, 0, 3)],
                "the first match is the current one"
            );
            assert_eq!(
                others,
                vec![(0, 6, 3)],
                "the rest highlight in the other colour"
            );

            // Enter steps to the next match; the pane is unaffected (the key belongs to the bar).
            let mut scene = Scene::Container(ContainerNode::new(Vec::new()));
            assert!(TerminalViewer::apply_key(
                &mut scene,
                Some(FIND_FIELD_TAG),
                "Enter",
                Modifiers::default(),
            ));
            assert_eq!(
                use_find_index().get(),
                1,
                "Enter advances the current match"
            );
            assert_eq!(
                visible_spans(0, 0, 6).1,
                vec![(0, 6, 3)],
                "...and the current-match highlight moves with it",
            );

            // Escape closes: no pane, no matches, nothing left to highlight.
            assert!(TerminalViewer::apply_key(
                &mut scene,
                Some(FIND_FIELD_TAG),
                "Escape",
                Modifiers::default(),
            ));
            assert_eq!(use_find_pane().get(), None, "Escape closes the bar");
            assert!(use_find_matches().get().is_empty());
            assert_eq!(visible_spans(0, 0, 6), (Vec::new(), Vec::new()));
        });
    }

    /// A match that crosses the pane's right edge highlights on BOTH rows — R344's point at the
    /// surface a person actually looks at.
    ///
    /// The pane echoes 26 characters onto a 20-column screen, so the alphabet is one logical line
    /// over two rows. The needle `stuvw` straddles the margin: before R344 the search answered
    /// NOTHING for it (the two rows were searched separately and neither held it), and a bar that
    /// painted only `hit.col`/`hit.cols` would now show half a highlighted word instead.
    ///
    /// Driven end to end — real pane, real host query, real signal — because the claim is about
    /// what the person sees, and the mapping from the answer's rows to grid rows is the part that
    /// only this surface performs.
    #[test]
    fn a_match_that_wraps_highlights_every_row_it_covers() {
        let host = Host::new((20, 6));
        let id = host
            .spawn(
                cat(),
                "cat".to_owned(),
                20,
                6,
                sprag_terminal::PaneBirthHooks::default(),
            )
            .unwrap();
        let handle = host.pane_handle(id).expect("pane handle");
        let owner = Owner::new();
        owner.run(|| {
            seed_terminal(host);
            let terminal = use_terminal();
            assert!(
                terminal.slots.send_text(0, "abcdefghijklmnopqrstuvwxyz"),
                "seed the pane",
            );
            wait_for_row(&handle, 1, "uvwxyz");

            use_text_edit_state(FIND_FIELD_TAG).seed("stuvw".to_owned());
            open(0);

            assert_eq!(
                use_find_matches().get().as_slice(),
                &[PaneMatch {
                    line: 0,
                    row: 0,
                    col: 18,
                    cols: 2,
                    wrapped: vec![3],
                }],
                "one match: `st` at the end of row 0, `uvw` at the start of row 1",
            );
            let (others, current) = visible_spans(0, 0, 6);
            assert_eq!(
                current,
                vec![(0, 18, 2), (1, 0, 3)],
                "the current match paints on BOTH rows, the continuation from column 0",
            );
            assert!(others.is_empty(), "there is only the one match");
        });
    }

    /// A match that lies wholly on a CONTINUATION row highlights on that row — not on the row its
    /// line began on.
    ///
    /// The pair to [`a_match_that_wraps_highlights_every_row_it_covers`], and the case that one
    /// structurally cannot make: there the match starts where its line does, so a client that
    /// confused the two would paint correctly by accident. Here `line` is 0 and `row` is 1, and
    /// they must not be swapped — which is exactly the silent misrender that made R344 a wire
    /// PROTOCOL bump rather than an additive key, since an old client parses the new answer
    /// perfectly and paints a row too high.
    #[test]
    fn a_match_below_the_wrap_highlights_its_own_row_not_its_lines() {
        let host = Host::new((20, 6));
        let id = host
            .spawn(
                cat(),
                "cat".to_owned(),
                20,
                6,
                sprag_terminal::PaneBirthHooks::default(),
            )
            .unwrap();
        let handle = host.pane_handle(id).expect("pane handle");
        let owner = Owner::new();
        owner.run(|| {
            seed_terminal(host);
            let terminal = use_terminal();
            assert!(
                terminal.slots.send_text(0, "abcdefghijklmnopqrstuvwxyz"),
                "seed the pane",
            );
            wait_for_row(&handle, 1, "uvwxyz");

            use_text_edit_state(FIND_FIELD_TAG).seed("vwxyz".to_owned());
            open(0);

            assert_eq!(
                use_find_matches().get().as_slice(),
                &[PaneMatch {
                    line: 0,
                    row: 1,
                    col: 1,
                    cols: 5,
                    wrapped: Vec::new(),
                }],
                "the match is on row 1; the LINE it belongs to began on row 0",
            );
            assert_eq!(
                visible_spans(0, 0, 6).1,
                vec![(1, 1, 5)],
                "and it highlights on row 1 — the row it is on, not the row its line started on",
            );
        });
    }

    /// The two routing halves the bar depends on, both driven through the real router:
    /// `Ctrl+Shift+F` on a focused PANE opens the bar, and once the FIELD holds focus an ordinary
    /// key EDITS THE NEEDLE instead of reaching the pane.
    ///
    /// The scene carries the real field External, which is what makes the second half a proof
    /// rather than a coincidence: "the pane did not receive `x`" is true even with no routing at
    /// all (a non-pane focus falls through to `false`), so the assertion that discriminates is that
    /// the NEEDLE grew. REVERT-PROOF: drop either the find-focus gate or the chord arm in
    /// `route_key` and one of these fails — measured, not assumed.
    #[test]
    fn the_chord_opens_the_bar_and_a_key_edits_the_needle_not_the_pane() {
        let host = Host::new((40, 6));
        let id = host
            .spawn(
                cat(),
                "cat".to_owned(),
                40,
                6,
                sprag_terminal::PaneBirthHooks::default(),
            )
            .unwrap();
        let handle = host.pane_handle(id).expect("pane handle");
        let owner = Owner::new();
        owner.run(|| {
            seed_terminal(host);
            use_text_edit_state(FIND_FIELD_TAG).seed(String::new());
            // The model scene the shell would have built: the field External at its tag, which is
            // what pinion's `forward_key_to_field` dispatches an edit through.
            let mut scene = Scene::Container(ContainerNode::new(vec![Scene::External(
                pinion_core::scene::ExternalNode::new(Box::new(
                    TextFieldExternal::new().attach_state(use_text_edit_state(FIND_FIELD_TAG)),
                ))
                .with_tag(FIND_FIELD_TAG),
            )]));

            let chord = Modifiers {
                ctrl: true,
                shift: true,
                ..Modifiers::default()
            };
            assert!(TerminalViewer::apply_key(
                &mut scene,
                Some(pane_tag(0)),
                "F",
                chord,
            ));
            assert_eq!(use_find_pane().get(), Some(0), "the chord opens the bar");

            assert!(
                TerminalViewer::apply_key(
                    &mut scene,
                    Some(FIND_FIELD_TAG),
                    "x",
                    Modifiers::default(),
                ),
                "the field consumed the key",
            );
            assert_eq!(needle(), "x", "the key edited the NEEDLE");
            std::thread::sleep(Duration::from_millis(50));
            assert!(
                !handle.with_screen(|s| s.row_text(0)).contains('x'),
                "...and never reached the pane",
            );
            close();
        });
    }

    #[test]
    fn scroll_target_leaves_a_visible_match_alone() {
        // Top = 100, 24 rows: lines 100..124 are on screen and need no scroll. Scrolling a visible
        // match into the centre would yank the text the user is already reading.
        assert_eq!(scroll_target(100, 100, 24, 1000), None);
        assert_eq!(scroll_target(123, 100, 24, 1000), None);
    }

    #[test]
    fn scroll_target_centres_an_off_screen_match() {
        // Below the view: centred (line - rows/2), so the match lands mid-screen with context.
        assert_eq!(scroll_target(500, 100, 24, 1000), Some(488));
        // Above the view: same rule, one axis.
        assert_eq!(scroll_target(10, 100, 24, 1000), Some(0));
        // Clamped at the far end rather than scrolling past the tail.
        assert_eq!(scroll_target(999, 100, 24, 500), Some(500));
    }

    #[test]
    fn wrapped_index_wraps_at_both_ends() {
        assert_eq!(wrapped_index(0, 1, 3), Some(1));
        assert_eq!(wrapped_index(2, 1, 3), Some(0), "next wraps past the last");
        assert_eq!(
            wrapped_index(0, -1, 3),
            Some(2),
            "previous wraps past the first",
        );
        assert_eq!(wrapped_index(0, 1, 0), None, "nothing to step through");
    }

    #[test]
    fn first_at_or_after_starts_from_the_view() {
        let matches = [hit(5, 0, 3), hit(50, 0, 3), hit(500, 0, 3)];
        assert_eq!(first_at_or_after(&matches, 0), 0);
        assert_eq!(first_at_or_after(&matches, 40), 1, "the first at or below");
        assert_eq!(
            first_at_or_after(&matches, 900),
            0,
            "every match is above the view -> wrap to the oldest",
        );
    }

    #[test]
    fn row_of_maps_the_logical_line_onto_the_visible_grid() {
        // The view's top row IS `offset_y`, so the mapping is a subtraction — and off-screen
        // lines map to nothing rather than to a clamped row that would paint a false highlight.
        assert_eq!(row_of(100, 100, 24), Some(0));
        assert_eq!(row_of(123, 100, 24), Some(23));
        assert_eq!(row_of(124, 100, 24), None, "one past the last row");
        assert_eq!(row_of(99, 100, 24), None, "above the view");
    }

    /// The bar can search in the OTHER language: the same characters find different things once the
    /// regex mode is on, and both the checkbox intent and `Alt+R` flip it.
    ///
    /// The needle `e.r` is the discriminator — it occurs NOWHERE literally in `err a e r`, and as a
    /// pattern it matches both `err` (via `.` = `r`) and `e r`. So a mode that failed to reach the wire
    /// could not produce these two matches, and a mode stuck ON could not produce the literal answer.
    ///
    /// The toggle is driven BY INTENT (its tag) and by the key, never by a synthesised click at a
    /// pixel — the surface has a symbolic external precisely so it is testable.
    ///
    /// REVERT-PROOF: routing `refresh` to `pane_find` regardless of the mode leaves the literal
    /// assertion passing and finds nothing in regex mode.
    #[test]
    fn the_find_bar_searches_in_the_regex_language_when_toggled() {
        let host = Host::new((40, 6));
        let id = host
            .spawn(
                cat(),
                "cat".to_owned(),
                40,
                6,
                sprag_terminal::PaneBirthHooks::default(),
            )
            .unwrap();
        let handle = host.pane_handle(id).expect("pane handle");
        let owner = Owner::new();
        owner.run(|| {
            seed_terminal(host);
            assert!(use_terminal().slots.send_text(0, "err a e r"), "seed");
            wait_for_row0(&handle, "err a e r");

            use_text_edit_state(FIND_FIELD_TAG).seed("e.r".to_owned());
            open(0);
            assert!(!use_find_regex().get(), "literal is the default language");
            assert!(
                use_find_matches().get().is_empty(),
                "`e.r` occurs nowhere LITERALLY",
            );

            // The checkbox's own intent — the click path, addressed by tag.
            assert!(handle_regex_intent(&pinion_core::Intent::new_owned(
                format!("{FIND_REGEX_TAG}.checked"),
                IntrospectValue::Bool(true),
            )));
            assert!(use_find_regex().get(), "the intent set the mode");
            assert_eq!(
                use_find_matches().get().as_slice(),
                &[hit(0, 0, 3), hit(0, 6, 3)],
                "as a PATTERN `e.r` matches `err` and `e r`",
            );

            // Alt+R is the same toggle from the keyboard, and it re-queries.
            let mut scene = Scene::Container(ContainerNode::new(Vec::new()));
            assert!(TerminalViewer::apply_key(
                &mut scene,
                Some(FIND_FIELD_TAG),
                "r",
                Modifiers {
                    alt: true,
                    ..Modifiers::default()
                },
            ));
            assert!(!use_find_regex().get(), "Alt+R toggled it back");
            assert!(
                use_find_matches().get().is_empty(),
                "...and the literal search answers nothing again",
            );
            close();
        });
    }

    /// A pattern the engine REFUSES is reported as a refusal, never as "no matches" — the same
    /// distinction the wire keeps, now visible to a human.
    ///
    /// `(unclosed` is invalid; the bar must show the engine's own message (which names what to fix)
    /// and drop the stale matches from the previous, valid search. Fixing the pattern clears it.
    ///
    /// REVERT-PROOF: dropping `use_find_error` and letting the empty match list stand renders
    /// "no matches" — a claim about a search that never ran. Both the message assertion and the
    /// cleared-again assertion fail.
    #[test]
    fn an_invalid_pattern_reports_the_refusal_not_an_empty_result() {
        let host = Host::new((40, 6));
        let id = host
            .spawn(
                cat(),
                "cat".to_owned(),
                40,
                6,
                sprag_terminal::PaneBirthHooks::default(),
            )
            .unwrap();
        let handle = host.pane_handle(id).expect("pane handle");
        let owner = Owner::new();
        owner.run(|| {
            seed_terminal(host);
            assert!(use_terminal().slots.send_text(0, "hello"), "seed");
            wait_for_row0(&handle, "hello");

            use_find_regex().set(true);
            use_text_edit_state(FIND_FIELD_TAG).seed("hel".to_owned());
            open(0);
            assert!(use_find_error().get().is_none(), "a valid pattern searched");
            assert_eq!(use_find_matches().get().len(), 1);

            use_text_edit_state(FIND_FIELD_TAG).seed("(unclosed".to_owned());
            refresh();
            let error = use_find_error().get().expect("the refusal is carried");
            assert!(
                !error.is_empty(),
                "the engine's own message says what to fix: {error}",
            );
            assert!(
                use_find_matches().get().is_empty(),
                "the previous search's matches must not linger over a search that never ran",
            );

            // The bar SAYS so, rather than showing a count or "no matches".
            let bar = view_bar(
                FindFieldState::default(),
                &pinion_core::theme::Theme::dark(),
                (1200, 800),
            )
            .expect("the bar paints while open");
            let text = scene_text(&bar);
            assert!(
                text.iter().any(|t| t.starts_with("bad pattern:")),
                "the bar reports the refusal: {text:?}",
            );
            assert!(
                !text.iter().any(|t| t == "no matches"),
                "and never calls a refusal an empty result: {text:?}",
            );

            // A valid pattern again clears it.
            use_text_edit_state(FIND_FIELD_TAG).seed("hel".to_owned());
            refresh();
            assert!(use_find_error().get().is_none(), "the refusal cleared");
            close();
        });
    }

    /// Every `TextNode` string in `scene`, in DFS order — the bar's visible words, so an assertion can
    /// read what the user reads instead of a pixel.
    fn scene_text(scene: &Scene) -> Vec<String> {
        let mut out = Vec::new();
        collect_text(scene, &mut out);
        out
    }

    fn collect_text(scene: &Scene, out: &mut Vec<String>) {
        match scene {
            Scene::Text(node) => out.push(node.content.clone()),
            Scene::Container(node) => {
                for child in &node.children {
                    collect_text(child, out);
                }
            }
            _ => {}
        }
    }

    /// The regex toggle is a REAL widget in the painted bar, not a keyboard-only mode: it carries the
    /// tag its intent arrives under and the label that doubles as its accessible name.
    ///
    /// This is what makes the mode discoverable. With two search languages behind one field, a mode
    /// bound only to `Alt+R` would leave the user unable to see which language their characters are in.
    #[test]
    fn the_bar_paints_a_labelled_regex_toggle() {
        let owner = Owner::new();
        owner.run(|| {
            use_find_pane().set(Some(0));
            let bar = view_bar(
                FindFieldState::default(),
                &pinion_core::theme::Theme::dark(),
                (1200, 800),
            )
            .expect("the bar paints while open");
            assert!(
                scene_text(&bar).iter().any(|t| t == REGEX_LABEL),
                "the toggle's label is visible: {:?}",
                scene_text(&bar),
            );
            assert!(
                find_tag(&bar, FIND_REGEX_TAG),
                "and it paints under the tag its intent arrives on",
            );
        });
    }

    /// Whether any node in `scene` carries `tag` — the symbolic address, which is how the toggle is
    /// reached by intent and by an RPC consumer.
    fn find_tag(scene: &Scene, tag: &str) -> bool {
        match scene {
            Scene::Container(node) => {
                node.tag.as_deref() == Some(tag)
                    || node.children.iter().any(|child| find_tag(child, tag))
            }
            other => other.tag() == Some(tag),
        }
    }
}
