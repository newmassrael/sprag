//! Mouse-drag text selection + clipboard copy / paste (R139).
//!
//! A press on a pane's grid anchors a selection; a drag sweeps it (a live inverted
//! band, [`sprag_grid::overlay_selection`]); the swept text is published to the X11
//! PRIMARY selection continuously (select-to-copy) and to the CLIPBOARD on
//! `Ctrl+Shift+C`. A middle-click pastes PRIMARY, `Ctrl+Shift+V` pastes CLIPBOARD —
//! both write the text to the focused pane's PTY through the same
//! [`HostClient::send_text`](sprag_host::HostClient::send_text) seam an IME commit and
//! an AI peer use.
//!
//! All of this rides pinion primitives (NO pinion change): the pointer coordinates
//! arrive through [`WidgetView::position_caret_for_point`](pinion_shell::WidgetView) /
//! `select_drag_to_point` (the shell's text-selection seam, retargeted from bytes to
//! grid cells), the clipboard through
//! [`use_app_clipboard`](pinion_platform_clipboard::use_app_clipboard) (arboard), and
//! the highlight by inverting the selected cells in the per-frame `GridBuffer`
//! projection — the same display-only overlay path as the IME preedit.
//!
//! Single-selection model (like a real terminal): ONE active selection across all
//! panes, held in an `Owner::cache` [`Signal`]. The selection is anchored to the pane's
//! VISIBLE grid (not its scrollback content), so scrolling a pane after selecting does
//! not follow the text — a v1 limit, consistent with the visible-grid coordinate.

use crate::terminal::{pane_index_of, pane_tag, use_terminal};
use pinion_core::reactive::{Owner, Signal};
use pinion_core::{Clipboard, ClipboardSelection, GridBuffer, Scene};
use serde::{Deserialize, Serialize};
use std::rc::Rc;

/// `Owner::cache` key for the single active [`PaneSelection`].
const SELECTION_KEY: &str = "sprag_gui.selection";

/// `Owner::cache` key for the process clipboard handle
/// ([`use_app_clipboard`](pinion_platform_clipboard::use_app_clipboard)).
const CLIPBOARD_KEY: &str = "sprag_gui.clipboard";

/// A `(col, row)` cell in a pane's VISIBLE grid.
type Cell = (u16, u16);

/// The ONE active text selection: which pane it is in, and its `anchor` (the pinned
/// end the press set) + `focus` (the dragged end) cells. An empty selection
/// (`anchor == focus`) is a bare click — it highlights and copies nothing but clears
/// any prior selection.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PaneSelection {
    pub(crate) pane: usize,
    anchor: Cell,
    focus: Cell,
}

impl PaneSelection {
    fn is_empty(self) -> bool {
        self.anchor == self.focus
    }

    /// The `(start, end)` cells in reading (row-major) order, both inclusive.
    fn normalized(self) -> (Cell, Cell) {
        let key = |c: Cell| (c.1, c.0); // order by (row, col)
        if key(self.anchor) <= key(self.focus) {
            (self.anchor, self.focus)
        } else {
            (self.focus, self.anchor)
        }
    }
}

/// The single active selection Signal (`None` = nothing selected). `view` reads it
/// each frame (subscribing the paint), so a `set` repaints the band via the R705.1
/// reactive-dirty bridge — the same mechanism the preedit overlay uses.
pub(crate) fn use_selection() -> Signal<Option<PaneSelection>> {
    Owner::current()
        .expect("use_selection() requires an active Owner scope")
        .cache(SELECTION_KEY, || Signal::new(None))
        .as_ref()
        .clone()
}

/// The process clipboard (real OS clipboard via arboard; an in-memory fallback when
/// arboard cannot init). `Owner::cache`-keyed so one handle is shared per session — the ONE
/// clipboard handle, shared with the OSC 52 integration ([`crate::clipboard_osc`]) so a program
/// setting the clipboard via OSC 52 and a `Ctrl+Shift+C` copy reach the SAME buffer.
pub(crate) fn clipboard() -> Rc<dyn Clipboard> {
    pinion_platform_clipboard::use_app_clipboard(CLIPBOARD_KEY)
}

/// The selection span `(start, end)` (inclusive, reading order) to HIGHLIGHT in pane
/// `i`, or `None` when the active selection is not for pane `i` or is empty. Read by
/// [`view`](crate::view) each frame.
pub(crate) fn span_for(i: usize) -> Option<(Cell, Cell)> {
    let sel = use_selection().get()?;
    (sel.pane == i && !sel.is_empty()).then(|| sel.normalized())
}

/// Clear the active selection (a typed key interacting with a PTY, or a freed slot).
pub(crate) fn clear() {
    let sig = use_selection();
    if sig.get().is_some() {
        sig.set(None);
    }
}

/// Clear the active selection iff it is in the (now freed) slot `pane` — the Round 2b
/// per-slot reset hook, so a slot reused by a later pane shows no inherited band.
pub(crate) fn reset_pane_selection(pane: usize) {
    let sig = use_selection();
    if sig.get().is_some_and(|s| s.pane == pane) {
        sig.set(None);
    }
}

/// A pointer press ([`WidgetView::position_caret_for_point`](pinion_shell::WidgetView)):
/// if it landed on a pane grid, (re)anchor the selection there and return an arm token
/// so the shell drives [`drag`] on the ensuing move. `extend` (Shift held) keeps the
/// existing anchor and moves only the focus (Shift-click extends). Returns `None` for a
/// press off any pane grid (a dock header / splitter / scrollbar / the surface gap), so
/// those gestures are untouched.
pub(crate) fn press(
    scene: &Scene,
    hit_tag: Option<&str>,
    x: f32,
    y: f32,
    extend: bool,
) -> Option<usize> {
    let pane = pane_of_hit(hit_tag)?;
    let cell = cell_at(scene, pane, x, y)?;
    let sig = use_selection();
    let selection = match (extend, sig.get()) {
        (true, Some(prev)) if prev.pane == pane => PaneSelection {
            pane,
            anchor: prev.anchor,
            focus: cell,
        },
        _ => PaneSelection {
            pane,
            anchor: cell,
            focus: cell,
        },
    };
    sig.set(Some(selection));
    if extend {
        publish_primary(selection);
    }
    Some((usize::from(cell.1) << 16) | usize::from(cell.0))
}

/// A drag move ([`WidgetView::select_drag_to_point`](pinion_shell::WidgetView)): extend
/// the active selection's focus to the cell under the cursor and, on a cell change,
/// publish the live selection to PRIMARY (X11 select-to-copy). Returns whether the
/// selection changed (the shell repaints the band).
pub(crate) fn drag(scene: &Scene, x: f32, y: f32) -> bool {
    let sig = use_selection();
    let Some(mut selection) = sig.get() else {
        return false;
    };
    let Some(cell) = cell_at(scene, selection.pane, x, y) else {
        return false;
    };
    if cell == selection.focus {
        return false;
    }
    selection.focus = cell;
    sig.set(Some(selection));
    publish_primary(selection);
    true
}

/// Copy the active selection to the CLIPBOARD selection (`Ctrl+Shift+C`). Returns
/// whether a non-empty selection was copied.
pub(crate) fn copy_selection() -> bool {
    let Some(selection) = use_selection().get() else {
        return false;
    };
    let text = selection_text(selection);
    if text.is_empty() {
        return false;
    }
    clipboard().copy(text);
    true
}

/// Select pane `pane`'s ENTIRE visible grid (context-menu "Select all") and publish it
/// to PRIMARY. Reads the pane's current visible cells for the `(cols, rows)` extent.
pub(crate) fn select_all(pane: usize) {
    let tv = use_terminal();
    let scroll = crate::scrollbar::use_pane_scroll(pane);
    let facts = tv.slots.pane_scroll_facts(pane);
    let offset_lines =
        crate::scrollbar::offset_lines_from_top(scroll.offset_y(), facts.scrollback_len);
    let cells = tv.slots.pane_cells(pane, offset_lines);
    let (cols, rows) = (cells.cols(), cells.rows());
    if cols == 0 || rows == 0 {
        return;
    }
    let selection = PaneSelection {
        pane,
        anchor: (0, 0),
        focus: (cols - 1, rows - 1),
    };
    use_selection().set(Some(selection));
    publish_primary(selection);
}

/// Paste the CLIPBOARD selection into pane `pane` (`Ctrl+Shift+V`).
pub(crate) fn paste_clipboard(pane: usize) -> bool {
    paste_into(pane, ClipboardSelection::Clipboard)
}

/// Paste the PRIMARY selection into pane `pane` (middle-click).
pub(crate) fn paste_primary(pane: usize) -> bool {
    paste_into(pane, ClipboardSelection::Primary)
}

/// Read `selection`'s clipboard and write it to pane `pane`'s PTY (the same
/// text->PTY seam an IME commit uses). No-op on an empty / absent clipboard.
fn paste_into(pane: usize, selection: ClipboardSelection) -> bool {
    let Some(text) = clipboard().paste_from(selection) else {
        return false;
    };
    if text.is_empty() {
        return false;
    }
    let _ = use_terminal().slots.send_text(pane, &text);
    true
}

/// Publish a non-empty selection's text to the PRIMARY selection (best-effort;
/// PRIMARY has no effect on macOS / Windows, where `copy_to` no-ops it).
fn publish_primary(selection: PaneSelection) {
    let text = selection_text(selection);
    if !text.is_empty() {
        clipboard().copy_to(ClipboardSelection::Primary, text);
    }
}

/// The selected text: the pane's CURRENT visible cells over the normalized span, rows
/// joined with `\n`, each row right-trimmed of the terminal's blank padding.
fn selection_text(selection: PaneSelection) -> String {
    if selection.is_empty() {
        return String::new();
    }
    let (start, end) = selection.normalized();
    let tv = use_terminal();
    let scroll = crate::scrollbar::use_pane_scroll(selection.pane);
    let facts = tv.slots.pane_scroll_facts(selection.pane);
    let offset_lines =
        crate::scrollbar::offset_lines_from_top(scroll.offset_y(), facts.scrollback_len);
    let cells = tv.slots.pane_cells(selection.pane, offset_lines);
    extract_text(&cells, start, end)
}

/// Extract the text of the span `[start, end]` (inclusive, reading order) from `cells`.
/// Rows join with `\n`; each row's run is right-trimmed of spaces (a terminal pads
/// short lines with blanks). Wide-cluster trailer cells carry an empty cluster, so the
/// head contributes the whole glyph and the trailer nothing.
fn extract_text(cells: &GridBuffer, start: Cell, end: Cell) -> String {
    let cols = cells.cols();
    let rows = cells.rows();
    if cols == 0 || rows == 0 {
        return String::new();
    }
    let (start_col, start_row) = start;
    let (end_col, end_row) = end;
    let last_row = end_row.min(rows - 1);
    let mut out = String::new();
    let mut row = start_row;
    while row <= last_row {
        let first = if row == start_row { start_col } else { 0 };
        let last_incl = if row == end_row { end_col } else { cols - 1 }.min(cols - 1);
        let mut line = String::new();
        let mut col = first;
        while col <= last_incl {
            if let Some(cell) = cells.cell(col, row) {
                line.push_str(&cell.cluster);
            }
            col += 1;
        }
        out.push_str(line.trim_end_matches(' '));
        if row != last_row {
            out.push('\n');
        }
        row += 1;
    }
    out
}

/// The pane a router hit-target `hit_tag` addresses, or `None` if it is not a pane
/// grid. The grid node is tagged `{pane_tag}#grid`, so strip any `#…` suffix before
/// resolving; a non-pane target (dock header / splitter / scrollbar) resolves to `None`.
fn pane_of_hit(hit_tag: Option<&str>) -> Option<usize> {
    let tag = hit_tag?;
    let base = tag.split('#').next().unwrap_or(tag);
    pane_index_of(base)
}

/// Map a window-local pixel `(x, y)` to a `(col, row)` cell of pane `pane`, clamped to
/// the pane's visible grid (a drag past an edge lands on the edge cell). Uses the pane
/// container's laid-out rect ([`Scene::rect_for_tag_absolute`]) and the measured cell
/// size — the same geometry `grid_dims` derives the PTY winsize from.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "grid coords are small non-negative counts that fit f32 then u16 after clamping"
)]
fn cell_at(scene: &Scene, pane: usize, x: f32, y: f32) -> Option<Cell> {
    let rect = scene.rect_for_tag_absolute(pane_tag(pane))?;
    let tv = use_terminal();
    let cw = tv.metric.cell_w() as f32;
    let ch = tv.metric.cell_h() as f32;
    let (rx, ry, rw, rh) = (rect.x as f32, rect.y as f32, rect.w as f32, rect.h as f32);
    if cw <= 0.0 || ch <= 0.0 || rw <= 0.0 || rh <= 0.0 {
        return None;
    }
    let cols = (rw / cw).floor().max(1.0);
    let rows = (rh / ch).floor().max(1.0);
    let col = ((x - rx) / cw).floor().clamp(0.0, cols - 1.0);
    let row = ((y - ry) / ch).floor().clamp(0.0, rows - 1.0);
    Some((col as u16, row as u16))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pinion_core::{TermCell, TermColor};

    fn grid(text_rows: &[&str], cols: u16) -> GridBuffer {
        let mut buffer = GridBuffer::new(cols, text_rows.len() as u16);
        for (r, line) in text_rows.iter().enumerate() {
            let cells: Vec<TermCell> = (0..cols)
                .map(|c| {
                    let ch = line.chars().nth(usize::from(c)).unwrap_or(' ');
                    TermCell::new(ch.to_string(), TermColor::Default, TermColor::Default)
                })
                .collect();
            buffer = buffer.with_row(r as u16, cells);
        }
        buffer
    }

    #[test]
    fn extract_text_same_row_is_the_inclusive_span() {
        let g = grid(&["hello world"], 11);
        assert_eq!(extract_text(&g, (0, 0), (4, 0)), "hello");
        assert_eq!(extract_text(&g, (6, 0), (10, 0)), "world");
    }

    #[test]
    fn extract_text_multi_row_joins_with_newlines_and_rtrims() {
        // 8 cols; short lines are blank-padded, so each row right-trims.
        let g = grid(&["ab      ", "cdef    ", "gh      "], 8);
        // From (1,0) to (1,2): first row "b" (tail), middle row full "cdef",
        // last row up to col 1 -> "gh".
        assert_eq!(extract_text(&g, (1, 0), (1, 2)), "b\ncdef\ngh");
    }

    #[test]
    fn selection_normalized_orders_by_row_then_col() {
        let sel = PaneSelection {
            pane: 0,
            anchor: (5, 2),
            focus: (1, 0),
        };
        assert_eq!(sel.normalized(), ((1, 0), (5, 2)));
        assert!(!sel.is_empty());
        let click = PaneSelection {
            pane: 0,
            anchor: (3, 1),
            focus: (3, 1),
        };
        assert!(click.is_empty());
    }

    #[test]
    fn pane_of_hit_strips_the_grid_suffix() {
        assert_eq!(pane_of_hit(Some("sprag_gui.pane.1#grid")), Some(1));
        assert_eq!(pane_of_hit(Some("sprag_gui.pane.0")), Some(0));
        assert_eq!(pane_of_hit(Some("sprag_gui.scrollbar.0")), None);
        assert_eq!(pane_of_hit(Some("terminal-2")), None);
        assert_eq!(pane_of_hit(None), None);
    }
}
