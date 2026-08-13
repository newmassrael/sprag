//! Per-pane OSC-8 hyperlink hover + click (pinion R1405 seam; sprag R-71.1/.2/.3).
//!
//! The data model (R1403/PINION-PR69) already flows: the host projects each pane's
//! cells into a `GridBuffer` whose `TermCell::hyperlink` indexes an interning table
//! ([`sprag_grid`]), and [`sprag_grid::overlay_hyperlink_hover`] can reverse-video a
//! hovered link's whole id-group. This module adds the INTERACTION pinion R1405
//! opened for a widget: a companion **hover-oracle** [`External`] per pane that
//!
//! * opts into plain-hover `pointer_move` ([`External::wants_hover_move`], R1405) so
//!   the pointer's position over the grid reaches it,
//! * resolves that position to a cell and, if the cell is a link, records the hovered
//!   link (lighting its id-group in the view + showing the hand cursor, R-71.1/.2),
//! * captures the press ONLY while over a link
//!   ([`External::wants_pointer_capture`] dynamic) so a click on a link activates it
//!   (the router's `invoke("send", "PointerDown")`, R-71.3) while a press on plain
//!   text falls through to text selection / focus (`position_caret_for_point`), and
//! * hands the activated URI to [`reconcile_pane_hyperlinks`], which opens it with the
//!   platform handler ([`open_uri`], scheme-gated).
//!
//! ## Why co-tagged at `pane_tag(i)` (not painted)
//!
//! pinion's hover router resolves a hover over the grid's composite paint tag
//! `{pane}#grid` through its PRIMARY half — `pane_tag(i)` — and forwards
//! `pointer_move` to the [`External`] registered at that tag
//! (`forward_pointer_move` -> `find_external_by_tag(state_scene, primary)`). So the
//! oracle registers at [`pane_tag`]`(i)`, exactly as pinion's `hello-hyperlink`
//! example co-tags its oracle with the grid. The oracle PAINTS NOTHING, so the pane
//! `Container` keeps owning `pane_tag(i)` in the paint scene — focus, text selection
//! (`selection::cell_at`'s `rect_for_tag_absolute`), and the scrollbar are untouched;
//! only the state-scene `find_external_by_tag` (an ExternalNode lookup) picks the
//! oracle up.
//!
//! ## Shared `Rc<HoverState>` (the scrollbar pattern, not `intervene`)
//!
//! The oracle wraps a per-pane [`Rc<HoverState>`] cached like the scrollbar's
//! `Rc<ScrollState>` ([`crate::scrollbar::use_pane_scroll`]). Feeding the current
//! link map and draining a click are plain writes on that shared handle — NOT an
//! `intervene` on the live external, which pinion R689 (preserve-by-tag) would fight
//! since the reconciled `ExtraExternal` is freshly constructed each frame. The shared
//! state outlives the instance swap because both the old and new oracle resolve the
//! SAME cached `Rc`.
//!
//! tmux flattens OSC 8 to plain text — no hover, no open. sprag lights the whole
//! id-group across wraps, shows a hand cursor, opens scheme-gated on click, AND the
//! link is agent-readable as data (`read_pane_links`, prior round).
//!
//! ## The pane's ONE pointer authority (mouse tracking, DECSET 1000/1002/1003/1006)
//!
//! Because only ONE [`External`] may register at `pane_tag(i)`, this oracle is ALSO the
//! pane's mouse-report layer (xterm mouse tracking). When the pane's child has a tracking
//! mode active (the host's per-frame `mouse` bit, fed via [`reconcile_pane_hyperlinks`]),
//! [`External::wants_raw_pointer_buttons`] turns on and pinion routes EVERY left / middle /
//! right press and release — with the modifiers held at each edge (PINION-PR72's raw stream,
//! consumed since R1418) — to the oracle's `raw_pointer_button`, SUPPRESSING the pane's GUI
//! defaults (no context menu, no PRIMARY paste, no legacy `PointerDown` / `PointerUp` wire).
//! Each edge is recorded as a semantic [`MouseInput`] at the last hovered cell, and
//! [`take_pane_mouse_reports`] hands it to the reconcile to forward to the host (which gates +
//! encodes the X10 / SGR report at the PTY boundary — coordinate conversion is the ONLY job
//! here). With no mode active the pane keeps native link / selection / paste / context-menu
//! behaviour. Under button-event (1002) / any-event (1003) tracking a `pointer_move` also
//! forwards a DRAG (with the held button, `primary_held`) or bare MOTION report, cell-granular;
//! the R1418 implicit grab keeps the drag position flowing even off the pane's rect. Wheel is
//! reported separately via [`apply_wheel`](crate::TerminalViewer) (Stage 2).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::process::Command;
use std::rc::Rc;
use std::sync::Arc;

use pinion_core::composite_tag::split_send_payload;
use pinion_core::external::{
    Backend, BackendFallback, BackendSupport, External, ExternalIntrospect, InterveneError,
    IntrospectSchema, IntrospectValue, InvokeError, ReadRefusal, RepaintOwner, SchemaField,
    ThreadOwnership,
};
use pinion_core::reactive::{Owner, Signal, use_repaint_sink};
use pinion_core::term_grid::HyperlinkId;
use pinion_core::widget_core::ExtraExternal;
use pinion_core::{CellMetric, GridBuffer};
use pinion_core::{
    NullRepaintSink, PointerButton, PointerButtons, PointerEdge, RawPointerButton, RepaintSink,
};
use sprag_input::{Modifiers, MouseButton, MouseEventKind, MouseInput};
use sprag_vt::MouseProtocol;

use crate::input::to_input_mods;
use crate::terminal::{cell_index, pane_cache_key, pane_tag, rect_cells};

/// The URI schemes a click is allowed to open — the safety gate so a hostile child
/// cannot emit an OSC-8 link to a dangerous scheme that runs on click. tmux has no
/// OSC-8 open at all; a scheme allowlist is the tmux-superior-yet-safe middle.
const ALLOWED_SCHEMES: [&str; 5] = ["http", "https", "mailto", "file", "ftp"];

/// A pane's visible link cells: `(col, row)` -> the cell's link id (in the pane's
/// current buffer table) and the URI a click opens.
type LinkMap = HashMap<(u16, u16), (HyperlinkId, Rc<str>)>;

/// A repaint sink resolved in-scope at [`HoverState`] construction ([`use_pane_hover`], inside the
/// binding Owner) and stored so `HoverState::record_report` can schedule a drain frame from the
/// oracle's event handlers, which the framework dispatches OUTSIDE the Owner scope (where
/// [`use_repaint_sink`] would panic). Debug-opaque ([`RepaintSink`] is not `Debug`).
struct RepaintHandle(Arc<dyn RepaintSink>);

impl std::fmt::Debug for RepaintHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RepaintHandle")
    }
}

/// Per-pane client-local hover state, shared between the oracle [`External`] (writes
/// `hovered` / `activated` from pointer events) and the view + reconcile (feed
/// `links` / `geometry`, read `hovered` for the overlay, drain `activated` to open).
/// The scrollbar's `Rc<ScrollState>` shape: cached per pane so every site resolves
/// the one instance, and feeding is a plain write rather than an `intervene`.
#[derive(Debug)]
pub(crate) struct HoverState {
    /// Grid geometry the oracle maps a `[0,1]` hover fraction against: the pane's OWN grid
    /// (the session's `(cols, rows)`, from the live buffer) and, separately, the extent of the
    /// WIDGET the fraction is a fraction of ([`rect_cells`], fractional cells).
    ///
    /// Two numbers, because they are two facts and they are not equal: the daemon divides the
    /// arbitrated window in CELLS while this client's dock divides its surface in PIXELS, so a
    /// pane's widget routinely spans a cell more than the pane holds. Mapping the fraction with
    /// `cols` alone stretched the pane's columns across the widget's span and put the pointer up
    /// to a whole column left of the glyph under it — see [`cell_index`].
    cols: Cell<u16>,
    rows: Cell<u16>,
    /// The widget's extent in fractional cells — the scale the `[0,1]` fraction is taken over.
    /// `(0.0, 0.0)` until a layout has been measured, which [`HoverState::cell_at`] reads as
    /// "no geometry yet" and answers with the origin cell.
    rect_cells: Cell<(f32, f32)>,
    /// `(col, row)` -> the cell's link (its id in the pane's CURRENT buffer table,
    /// and the URI a click opens). Fed each frame from the live projection.
    links: RefCell<LinkMap>,
    /// The link the pointer is over (its `HyperlinkId` in the current buffer), or
    /// `None`. A reactive `Signal` so the view repaints the hover highlight when it
    /// changes (the same reactive path selection / scroll use).
    hovered: Signal<Option<HyperlinkId>>,
    /// A URI a click activated, awaiting [`reconcile_pane_hyperlinks`] to open it.
    activated: RefCell<Option<String>>,
    /// The pane's live mouse-tracking protocol LEVEL (fed each frame from the host's `mouse` token).
    /// Gates [`HyperlinkOracle::wants_raw_pointer_buttons`] (any active level makes the oracle own
    /// the raw L/M/R stream for REPORTING, not link / selection / paste / context-menu) AND, from
    /// the level, whether a `pointer_move` forwards a DRAG ([`MouseProtocol::reports_drag`]) or bare
    /// MOTION ([`MouseProtocol::reports_motion`]). Plain [`Cell`] — read on the pointer edges, not
    /// painted, so it needs no reactive `Signal`.
    mouse_protocol: Cell<MouseProtocol>,
    /// The button to report on a following DRAG — the PRIMARY button held after the last raw edge
    /// (left over middle over right, [`RawPointerButton::buttons`]'s primary), or `None` when no
    /// button is held. Set from every [`HyperlinkOracle::raw_pointer_button`] edge (PINION-PR72's
    /// raw multi-button stream), so a held move under 1002/1003 drags with the ACTUAL button
    /// (left / middle / right), not just left. `None` at `pointer_move` time means a bare MOTION.
    held: Cell<Option<MouseButton>>,
    /// The 0-based cell the pointer last resolved to (set on every `pointer_move`, link or not) —
    /// the coordinate a captured press/release report addresses, and the cell a drag/motion report
    /// dedupes against (only a CELL change reports, xterm's granularity — never per-pixel). Distinct
    /// from [`Self::hovered`], which is `None` off a link; a mouse report needs the cell over plain text.
    last_cell: Cell<(u16, u16)>,
    /// Semantic mouse reports the oracle captured (press / release), awaiting
    /// [`take_pane_mouse_reports`] to forward them to the host. A queue (not a single slot) so a
    /// press and its release both survive to the same drain frame.
    pending_mouse: RefCell<Vec<MouseInput>>,
    /// Fractional wheel-line remainder carried between wheel events, so a fine touchpad pan (many
    /// sub-line `Pixels` deltas) accumulates to whole notches instead of rounding to zero each
    /// event — the mouse-report twin of the scrollbar's `wheel_accum`, but counted in whole
    /// notches (one report per line) rather than scrollback rows. Read + written only by
    /// [`wheel_reports`], on the wheel-handler thread, so a plain [`Cell`] suffices (not painted,
    /// no reactive `Signal`).
    wheel_accum: Cell<f32>,
    /// The shell's repaint sink, resolved once in-scope (see [`RepaintHandle`]). `record_report`
    /// calls it so a queued report always gets a drain frame, even when the pointer event repainted
    /// nothing on its own.
    repaint: RepaintHandle,
}

impl Default for HoverState {
    fn default() -> Self {
        Self {
            cols: Cell::new(0),
            rows: Cell::new(0),
            rect_cells: Cell::new((0.0, 0.0)),
            links: RefCell::new(HashMap::new()),
            hovered: Signal::new(None),
            activated: RefCell::new(None),
            mouse_protocol: Cell::new(MouseProtocol::None),
            held: Cell::new(None),
            last_cell: Cell::new((0, 0)),
            pending_mouse: RefCell::new(Vec::new()),
            wheel_accum: Cell::new(0.0),
            // Null until `HoverState::new` overrides it with the in-scope sink; a bare `default()`
            // (never used to build a live oracle) simply no-ops its repaint requests.
            repaint: RepaintHandle(Arc::new(NullRepaintSink)),
        }
    }
}

impl HoverState {
    /// Build a [`HoverState`] carrying the current scope's [`RepaintSink`] — called by
    /// [`use_pane_hover`] inside the binding Owner, the ONE place the sink resolves.
    fn new(repaint: Arc<dyn RepaintSink>) -> Self {
        Self {
            repaint: RepaintHandle(repaint),
            ..Self::default()
        }
    }
}

impl HoverState {
    /// The cell a `[0,1]x[0,1]` pane-rect hover fraction lands on.
    ///
    /// The fraction is scaled by the WIDGET's extent ([`rect_cells`]) to recover the offset in
    /// cells, and only then floored and clamped to the PANE's grid ([`cell_index`]). Scaling by
    /// the grid's own count instead — which this did until the two were found to differ — spreads
    /// `cols` columns evenly across a widget that spans more than `cols` of them, so every column
    /// past the first drifts left of the glyph the user is pointing at.
    fn cell_at(&self, x_rel: f32, y_rel: f32) -> (u16, u16) {
        let (span_x, span_y) = self.rect_cells.get();
        (
            cell_index(x_rel * span_x, self.cols.get()),
            cell_index(y_rel * span_y, self.rows.get()),
        )
    }

    /// The URI interned for `id` this frame (any cell carrying it), if still present.
    fn uri_of(&self, id: HyperlinkId) -> Option<String> {
        self.links
            .borrow()
            .values()
            .find(|(cell_id, _)| *cell_id == id)
            .map(|(_, uri)| uri.to_string())
    }

    /// Queue a report of `button` / `kind` with the modifiers `mods` held at that edge, at the last
    /// resolved cell. The reconcile drains it via [`take_pane_mouse_reports`] and forwards it to the
    /// host, which gates + encodes it. A press/release edge carries its real modifiers (PINION-PR72's
    /// raw stream delivers them on BOTH edges); a drag / bare-motion report has no keyboard edge to
    /// read live, so those pass [`Modifiers::default`]. [`MouseButton::None`] marks a bare motion.
    fn record_report(&self, button: MouseButton, kind: MouseEventKind, mods: Modifiers) {
        let (col, row) = self.last_cell.get();
        self.pending_mouse.borrow_mut().push(MouseInput {
            button,
            kind,
            col,
            row,
            mods,
        });
        // Force a frame so `reconcile_frame` DRAINS this report even when the pointer event itself
        // repaints nothing: a bare motion over plain text leaves `hovered` unchanged, so its
        // `Signal::set` schedules no paint, and the queued report would otherwise wait for an
        // unrelated repaint. Uses the sink resolved in-scope at construction (calling
        // `use_repaint_sink()` HERE panics — a pointer event dispatches outside the Owner scope).
        // Mirrors the PTY `on_dirty` -> `request_repaint` seam; idempotent, and a Null sink no-ops.
        self.repaint.0.request_repaint();
    }
}

/// Pane `i`'s shared hover state (Owner::cache-backed, the scrollbar `ScrollState`
/// pattern) — resolved by the oracle, the view, and the reconcile to the ONE slot.
pub(crate) fn use_pane_hover(i: usize) -> Rc<HoverState> {
    let owner = Owner::current().expect("use_pane_hover() requires an active Owner scope");
    // PRE-RESOLVE the repaint sink here (in the Owner scope, the ONE place `use_repaint_sink` is
    // valid) — NOT inside the cache factory below, where a nested slot resolution is forbidden
    // ([[owner-cache-no-nested-factory]]). The oracle's later event handlers run outside the Owner
    // scope and reuse the stored sink. Cheap (an `Arc` clone) even when the state is already cached.
    let sink = use_repaint_sink();
    owner
        .cache(pane_cache_key("hyperlink_hover", i), || {
            Rc::new(HoverState::new(sink))
        })
        .as_ref()
        .clone()
}

/// The link the pointer is over pane `i`, for the view's hover overlay (R-71.2) and
/// hand-cursor hint (R-71.1). A tracked read: reading the `hovered` `Signal`
/// subscribes the paint, so a hover move repaints.
pub(crate) fn hovered_link(i: usize) -> Option<HyperlinkId> {
    use_pane_hover(i).hovered.get()
}

/// Whether pane `i`'s pointer oracle is CAPTURING presses for mouse reporting (its child has a
/// tracking level active) — the client-local mirror of the host level the reconcile last fed. Read
/// by `position_caret_for_point` to suppress text selection on a press the report path owns. A
/// plain [`Cell`] read (not a tracked `Signal`), so it does not subscribe the caller's scope.
pub(crate) fn pane_mouse_capturing(i: usize) -> bool {
    use_pane_hover(i).mouse_protocol.get().is_active()
}

/// Feed pane `i`'s oracle its current link map + geometry from the live `buffer`, and
/// DRAIN any click-activated URI to open it. Runs per-frame from
/// [`reconcile_frame`](crate::TerminalViewer) (the sanctioned off-thread-fact -> UI
/// seam), NOT the pure view. The link map is rebuilt from the buffer's
/// `TermCell::hyperlink` + interning table so a click resolves the exact URI the
/// hovered cell shows. A hovered id that no longer resolves (the buffer changed under
/// a paused pointer) is cleared so a stale highlight cannot linger.
///
/// `rect_px` is the pane widget's laid-out extent (pinion R1012's per-pane viewport). It is
/// fed here — beside the grid, from the same frame — because the two are the two halves of one
/// mapping and a stale half is a mis-aimed pointer: the fraction pinion delivers is a fraction
/// of the WIDGET, and the cell it names belongs to the PANE. Unmeasured is `(0, 0)`, which
/// [`HoverState::cell_at`] answers with the origin cell.
pub(crate) fn reconcile_pane_hyperlinks(
    i: usize,
    buffer: &GridBuffer,
    mouse_protocol: MouseProtocol,
    rect_px: (u32, u32),
    metric: CellMetric,
) {
    let state = use_pane_hover(i);
    let cols = buffer.cols();
    let rows = buffer.rows();
    state.cols.set(cols);
    state.rows.set(rows);
    state.rect_cells.set(rect_cells(rect_px, metric));
    // Feed the live tracking level so `wants_raw_pointer_buttons` gates the next press + `pointer_move`
    // decides drag / motion forwarding correctly.
    state.mouse_protocol.set(mouse_protocol);
    {
        let mut links = state.links.borrow_mut();
        links.clear();
        for row in 0..rows {
            for col in 0..cols {
                if let Some(id) = buffer.cell(col, row).and_then(|c| c.hyperlink)
                    && let Some(link) = buffer.hyperlink(id)
                {
                    links.insert((col, row), (id, Rc::from(link.uri.as_str())));
                }
            }
        }
    }
    // Clear a stale hover whose link left the buffer (equality-skips if already None).
    if state
        .hovered
        .get()
        .is_some_and(|id| buffer.hyperlink(id).is_none())
    {
        state.hovered.set(None);
    }
    // Open at most one click's URI, then clear it (single-open-per-click).
    let activated = state.activated.borrow_mut().take();
    if let Some(uri) = activated {
        open_uri(&uri);
    }
}

/// Drain pane `i`'s captured mouse reports (empty when the child is not tracking or no press
/// landed). The reconcile ([`reconcile_frame`](crate::TerminalViewer)) forwards each to the host
/// via [`SlotView::mouse`](crate::slotview::SlotView::mouse) — the send lives THERE (not here) so
/// this module needs no host handle, mirroring how the URI open drains through
/// [`reconcile_pane_hyperlinks`]. Draining leaves the queue empty so a report sends exactly once.
pub(crate) fn take_pane_mouse_reports(i: usize) -> Vec<MouseInput> {
    std::mem::take(&mut *use_pane_hover(i).pending_mouse.borrow_mut())
}

/// Reset pane slot `i`'s hyperlink hover state when the slot FREES (the ONE reset
/// owner, [`reset_freed_slot`](crate::reset_freed_slot)), so a reused slot inherits no
/// stale hover / links / pending open.
pub(crate) fn reset_pane_hyperlinks(i: usize) {
    let state = use_pane_hover(i);
    state.hovered.set(None);
    state.links.borrow_mut().clear();
    *state.activated.borrow_mut() = None;
    state.cols.set(0);
    state.rows.set(0);
    state.mouse_protocol.set(MouseProtocol::None);
    state.held.set(None);
    state.last_cell.set((0, 0));
    state.pending_mouse.borrow_mut().clear();
    state.wheel_accum.set(0.0);
}

/// A safety bound on the wheel reports emitted from a single event — a defensive clamp on a
/// pathological driver / pixel delta (never a real physical scroll), the same child-buffer-clamp
/// discipline as the clipboard / image byte caps. Well above any genuine fling.
const WHEEL_REPORT_CAP: i32 = 64;

/// Fold `lines` of wheel delta into whole notches, carrying the sub-notch remainder in `accum`.
/// Returns `(new_accum, notches)` where `notches` is signed (positive = scroll-DOWN / toward the
/// live bottom = xterm wheel-down, negative = wheel-up), truncated toward zero so a reversal
/// cancels rather than double-counting. Pure — the storage (`HoverState::wheel_accum`) and the
/// report construction live in [`wheel_reports`]; this is the unit-testable core.
fn accumulate_wheel_notches(accum: f32, lines: f32) -> (f32, i32) {
    let total = accum + lines;
    let notches = total.trunc();
    #[allow(
        clippy::cast_possible_truncation,
        reason = "notches is a small whole scroll count clamped below by WHEEL_REPORT_CAP at the caller"
    )]
    let count = notches as i32;
    (total - notches, count)
}

/// Build the wheel-button reports for a `lines` delta over tracking pane `i`, addressed to cell
/// `(col, row)` with `mods` held (the wheel handler HAS the real modifiers, unlike the press-edge
/// capture channel). Accumulates the sub-notch remainder in the pane's [`HoverState::wheel_accum`]
/// so a fine touchpad pan still reports, and emits one report per whole notch (xterm's model: a
/// wheel step is a press-only pseudo-button 64/65, no release). The direction follows the sign
/// ([`accumulate_wheel_notches`]); the per-event count is clamped to [`WHEEL_REPORT_CAP`]. The
/// caller ([`apply_wheel`](crate::TerminalViewer)) forwards each via
/// [`SlotView::mouse`](crate::slotview::SlotView::mouse), which gates + encodes at the PTY boundary.
pub(crate) fn wheel_reports(
    i: usize,
    col: u16,
    row: u16,
    lines: f32,
    mods: Modifiers,
) -> Vec<MouseInput> {
    let state = use_pane_hover(i);
    let (remainder, notches) = accumulate_wheel_notches(state.wheel_accum.get(), lines);
    state.wheel_accum.set(remainder);
    if notches == 0 {
        return Vec::new();
    }
    let button = if notches > 0 {
        MouseButton::WheelDown
    } else {
        MouseButton::WheelUp
    };
    let count = notches.unsigned_abs().min(WHEEL_REPORT_CAP.unsigned_abs()) as usize;
    (0..count)
        .map(|_| MouseInput {
            button,
            kind: MouseEventKind::Press,
            col,
            row,
            mods,
        })
        .collect()
}

/// Register pane `i`'s hover-oracle [`External`] at [`pane_tag`]`(i)` (the primary
/// half of the grid's `#grid` composite hit-tag), wired in
/// [`create_extra_externals`](crate::TerminalViewer). Pointer-only + unpainted: it
/// contributes no paint node, so the pane `Container` keeps the tag for focus /
/// selection / rect lookups; only the hover router's `find_external_by_tag` picks it.
pub(crate) fn pane_hyperlink_external(i: usize) -> ExtraExternal {
    ExtraExternal::new(
        pane_tag(i).to_owned(),
        Box::new(HyperlinkOracle {
            state: use_pane_hover(i),
        }),
    )
}

/// The per-pane hover-oracle: a thin [`External`] over the shared [`HoverState`].
#[derive(Debug)]
struct HyperlinkOracle {
    state: Rc<HoverState>,
}

impl HyperlinkOracle {
    /// Record the hovered link's URI for the reconcile to open (the click landed).
    fn activate(&self) {
        if let Some(uri) = self
            .state
            .hovered
            .get()
            .and_then(|id| self.state.uri_of(id))
        {
            *self.state.activated.borrow_mut() = Some(uri);
        }
    }

    /// A PointerDown on the legacy `send` wire: activate a hovered link. Only reached when the pane
    /// is NOT tracking — while a mouse mode is active the oracle owns the raw multi-button stream
    /// ([`External::wants_raw_pointer_buttons`]) and pinion SUPPRESSES this legacy wire, routing
    /// every L/M/R press/release through [`HyperlinkOracle::raw_pointer_button`] as a mouse report.
    /// The `is_active` guard keeps "a tracking press reports, it does not activate a link" even if
    /// the wire were ever reached mid-tracking.
    fn on_pointer_down(&self) {
        if !self.state.mouse_protocol.get().is_active() {
            self.activate();
        }
    }
}

/// The event name a `send` payload carries. A NATIVE grid pointer send is composite
/// (`"grid:PointerDown"` — the `{pane}#grid` sub-index the router splits on); the RPC / test path
/// sends the bare event name. [`split_send_payload`] returns `None` for the colon-free bare form,
/// so falling back to the whole string decodes both without the caller knowing which arrived. This
/// is why an earlier exact `== "PointerDown"` match missed native clicks.
fn send_event_name(payload: &str) -> &str {
    split_send_payload(payload).map_or(payload, |split| split.event)
}

/// The button a DRAG should report given the set still held after a raw edge: the PRIMARY of the
/// held set, left over middle over right (xterm reports a drag with one held button, so a chord
/// picks a deterministic primary). `None` when nothing is held — the disarm that ends a drag.
fn primary_held(buttons: PointerButtons) -> Option<MouseButton> {
    if buttons.contains(PointerButton::Left) {
        Some(MouseButton::Left)
    } else if buttons.contains(PointerButton::Middle) {
        Some(MouseButton::Middle)
    } else if buttons.contains(PointerButton::Right) {
        Some(MouseButton::Right)
    } else {
        None
    }
}

impl External for HyperlinkOracle {
    fn backends(&self) -> BackendSupport {
        BackendSupport::new(&[Backend::Gui, Backend::Rpc], BackendFallback::Skip)
    }

    fn repaint_ownership(&self) -> RepaintOwner {
        RepaintOwner::Framework
    }

    fn thread_ownership(&self) -> ThreadOwnership {
        ThreadOwnership::UiThreadSync
    }

    /// Opt into plain-hover position (R1405) — the whole point of the affordance.
    fn wants_hover_move(&self) -> bool {
        true
    }

    /// Capture the press only while over a link, to activate it. A TRACKING pane does not capture
    /// here: it owns the raw multi-button stream ([`Self::wants_raw_pointer_buttons`]), whose
    /// PINION-PR72 R1418 implicit grab forwards the drag position and whose L/M/R press/release
    /// edges arrive through [`HyperlinkOracle::raw_pointer_button`] — capture is purely the
    /// link-activation affordance. A press over neither a link nor a tracking pane falls through to
    /// text selection / focus. Dynamic from `hovered` (set by the last `pointer_move`).
    fn wants_pointer_capture(&self) -> bool {
        self.state.hovered.get().is_some()
    }

    /// Own the pane's raw multi-button pointer stream whenever the child is TRACKING the mouse
    /// (PINION-PR72). Returning `true` makes pinion deliver EVERY left / middle / right press and
    /// release to [`HyperlinkOracle::raw_pointer_button`] with the modifiers held at each edge, and
    /// SUPPRESS the GUI defaults for this pane — no context menu on right, no PRIMARY paste on
    /// middle, no legacy `PointerDown` / `PointerUp` send wire — so a tracking TUI (vim right-drag,
    /// middle paste, a context-menu app) owns the buttons. Polled per edge, so it tracks the live
    /// mode: off a tracking mode the pane keeps the native GUI button semantics.
    fn wants_raw_pointer_buttons(&self) -> bool {
        self.state.mouse_protocol.get().is_active()
    }

    /// Each move delivers a `[0,1]` pane-rect fraction: reconstruct the cell and, when it CHANGES,
    /// forward a DRAG (the `held` button under button/any-event tracking) or bare MOTION (no
    /// button, any-event tracking) report at the new cell — cell-granular, never per-pixel (xterm's
    /// rule). Always records the last pointer cell (for a press/release report, link or not) and
    /// updates the hovered link (or `None` off a link, for the hover highlight).
    fn pointer_move(&mut self, x_rel: f32, y_rel: f32) {
        let cell = self.state.cell_at(x_rel, y_rel);
        if cell != self.state.last_cell.get() {
            let proto = self.state.mouse_protocol.get();
            self.state.last_cell.set(cell);
            if let Some(button) = self.state.held.get() {
                if proto.reports_drag() {
                    self.state
                        .record_report(button, MouseEventKind::Drag, Modifiers::default());
                }
            } else if proto.reports_motion() {
                self.state.record_report(
                    MouseButton::None,
                    MouseEventKind::Motion,
                    Modifiers::default(),
                );
            }
        }
        let hovered = self.state.links.borrow().get(&cell).map(|(id, _)| *id);
        self.state.hovered.set(hovered);
    }

    /// A raw pointer-button edge from the PINION-PR72 multi-button stream (opted into by
    /// [`Self::wants_raw_pointer_buttons`] while the child is tracking): report the left / middle /
    /// right press or release at the last resolved cell, with the modifiers held at THIS edge — the
    /// press edge now carries them too (the gap the legacy send wire had). Then track the PRIMARY
    /// held button (left over middle over right) from the event's held set, so a following
    /// `pointer_move` drags with the actual button and a full release (`buttons` empty) disarms it.
    fn raw_pointer_button(&mut self, event: RawPointerButton) {
        let button = match event.button {
            PointerButton::Left => MouseButton::Left,
            PointerButton::Middle => MouseButton::Middle,
            PointerButton::Right => MouseButton::Right,
        };
        let kind = match event.edge {
            PointerEdge::Down => MouseEventKind::Press,
            PointerEdge::Up => MouseEventKind::Release,
        };
        self.state
            .record_report(button, kind, to_input_mods(event.modifiers));
        self.state.held.set(primary_held(event.buttons));
    }

    fn introspect(&self) -> Option<&dyn ExternalIntrospect> {
        Some(self)
    }

    fn introspect_mut(&mut self) -> Option<&mut dyn ExternalIntrospect> {
        Some(self)
    }
}

impl HyperlinkOracle {
    /// The reading itself — see [`query`](Self::query).
    fn read(&self, path: &str) -> Option<IntrospectValue> {
        match path {
            sprag_host::wire::ACTION_GRAMMAR_SLOT => Some(IntrospectValue::Json(
                sprag_host::wire::ActionGrammar::answer(crate::wire_claim::grammar::hyperlink()),
            )),
            "hover_index" => Some(self.state.hovered.get().map_or(IntrospectValue::Null, |h| {
                IntrospectValue::Int(i64::from(h.0))
            })),
            "activated_uri" => Some(
                self.state
                    .activated
                    .borrow()
                    .clone()
                    .map_or(IntrospectValue::Null, IntrospectValue::Text),
            ),
            _ => None,
        }
    }
}

impl ExternalIntrospect for HyperlinkOracle {
    fn schema(&self) -> IntrospectSchema {
        IntrospectSchema::new(
            const {
                &[
                    SchemaField::new("hover_index", "int"),
                    SchemaField::new("activated_uri", "string"),
                    // ⚠ VERBS, DECLARED AS SUCH — and `activate` was DISPATCHED AND DECLARED
                    // NOWHERE, the same defect `report_agent` carried on the mux surface: the
                    // oracle answered it and `$schema` never mentioned it, so no client could
                    // discover the one verb this surface exists to offer.
                    SchemaField::action("send", "string"),
                    SchemaField::action("activate", "json"),
                    // HOW TO CALL THE TWO ABOVE — see the palette's note. `activate` takes nothing,
                    // which is the whole reason a client has to be told: the link it opens is the one
                    // `hover_index` names.
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
            // AI-first / no-pixel hover: set the hovered link index (Null clears).
            "hover_index" => match value {
                IntrospectValue::Null => {
                    self.state.hovered.set(None);
                    Ok(())
                }
                IntrospectValue::Int(i) => {
                    let id = u32::try_from(i).map_err(|_| InterveneError::TypeMismatch)?;
                    self.state.hovered.set(Some(HyperlinkId(id)));
                    Ok(())
                }
                _ => Err(InterveneError::TypeMismatch),
            },
            "activated_uri" | "hover_index_ro" => Err(InterveneError::ReadOnly),
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
            // The legacy router press channel (R1401). A native grid press arrives COMPOSITE
            // (`"grid:PointerDown"`, the `{pane}#grid` sub-index), the RPC / test path sends the
            // bare event name — `send_event_name` decodes both. This wire only fires when the pane
            // is NOT tracking (a tracking pane owns the raw multi-button stream, which suppresses
            // it): a PointerDown over a link activates it (the click). The release edge carries no
            // non-tracking semantic (a link activates on the press), so PointerUp is ignored here.
            // ⚠⚠ **THIS ARM ANSWERED `Ok` FOR ANY `args` AT ALL**, including an int, so the payload it
            // declares was one it did not read — while the palette's and the confirmation's `send`,
            // which take the same composite event name, both refuse a non-string as malformed. R330's
            // odd-one-out rule, found by `a_declared_argument_is_one_the_daemon_reads` the first time
            // it ran on this surface (R354). The narrowing is invisible to every well-formed caller:
            // the router sends the event NAME, which is a string.
            "send" => {
                let IntrospectValue::Text(payload) = &args else {
                    return Err(InvokeError::TypeMismatch);
                };
                if send_event_name(payload) == "PointerDown" {
                    self.on_pointer_down();
                }
                Ok(IntrospectValue::Null)
            }
            // AI-first / no-pixel click: activate the currently hovered link.
            "activate" => {
                self.activate();
                Ok(IntrospectValue::Null)
            }
            _ => Err(InvokeError::UnknownPath),
        }
    }
}

/// Open `uri` with the platform's default handler, best-effort and detached
/// (R-71.3): `xdg-open` (Linux/BSD), `open` (macOS), `cmd /c start` (Windows). Gated
/// to [`ALLOWED_SCHEMES`] so a hostile child cannot open a dangerous-scheme link.
/// Never blocks or panics; a spawn failure (no handler installed) logs a warning.
pub(crate) fn open_uri(uri: &str) {
    if !scheme_allowed(uri) {
        tracing::warn!(target: "sprag_gui::hyperlink", uri, "refusing a disallowed-scheme link");
        return;
    }
    open_allowed(uri);
}

/// Hand an ALREADY scheme-checked `uri` to the platform handler — the opener seam.
/// Split from [`open_uri`] so a unit test can intercept it: the `#[cfg(test)]` twin
/// below records the URI instead of spawning, letting the reconcile's
/// open-exactly-once contract be verified WITHOUT an OS side effect. Spawning
/// `xdg-open` under test would pop the desktop's `mailto`/`http` handler (e.g. a mail
/// client window) on every run — the DI seam keeps the effect out of the test path.
#[cfg(not(test))]
fn open_allowed(uri: &str) {
    use std::process::Stdio;

    match open_command(uri)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(_) => tracing::debug!(target: "sprag_gui::hyperlink", uri, "opened link"),
        Err(error) => {
            tracing::warn!(target: "sprag_gui::hyperlink", uri, %error, "failed to open link");
        }
    }
}

/// Whether `uri`'s scheme is in [`ALLOWED_SCHEMES`] (case-insensitive). A URI with no
/// `scheme:` prefix is rejected — the opener only handles absolute, schemed targets.
fn scheme_allowed(uri: &str) -> bool {
    match uri.split_once(':') {
        Some((scheme, _)) => ALLOWED_SCHEMES
            .iter()
            .any(|allowed| scheme.eq_ignore_ascii_case(allowed)),
        None => false,
    }
}

/// The platform command that opens `uri`. Passes the URI as an ARGUMENT (never via a
/// shell), so a crafted URI cannot inject a command.
fn open_command(uri: &str) -> Command {
    #[cfg(target_os = "macos")]
    {
        let mut cmd = Command::new("open");
        cmd.arg(uri);
        cmd
    }
    #[cfg(target_os = "windows")]
    {
        let mut cmd = Command::new("cmd");
        // `start` treats a first quoted arg as the window title, so pass an empty one.
        cmd.args(["/c", "start", "", uri]);
        cmd
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let mut cmd = Command::new("xdg-open");
        cmd.arg(uri);
        cmd
    }
}

// Test opener seam: under `#[cfg(test)]`, `open_allowed` records the URIs it is handed
// here instead of spawning the platform handler, so the reconcile's open-exactly-once
// contract is asserted with NO OS side effect. `None` = no recorder installed; an open
// in that state PANICS (fail-fast) so no test can silently spawn `xdg-open`.
// Thread-local, so parallel tests never contaminate each other.
#[cfg(test)]
thread_local! {
    static OPENER: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
}

/// The `#[cfg(test)]` twin of [`open_allowed`]: record the URI for assertion rather
/// than spawn. Panics if no [`RecordedOpener`] is installed — a test reaching a real
/// open path without intercepting it is a bug to surface, not a silent OS launch.
#[cfg(test)]
fn open_allowed(uri: &str) {
    OPENER.with(|slot| match slot.borrow_mut().as_mut() {
        Some(recorded) => recorded.push(uri.to_owned()),
        None => panic!(
            "open_uri({uri:?}) reached the opener with no RecordedOpener installed — \
             a test would have spawned the platform handler; install RecordedOpener::install()"
        ),
    });
}

/// RAII installer for the test opener recorder: `install()` swaps the thread-local
/// opener to a fresh recording buffer, `opened()` reads what the reconcile handed it,
/// and drop uninstalls so a later test on the same (pooled) thread starts clean.
#[cfg(test)]
struct RecordedOpener;

#[cfg(test)]
impl RecordedOpener {
    fn install() -> Self {
        OPENER.with(|slot| *slot.borrow_mut() = Some(Vec::new()));
        Self
    }

    fn opened(&self) -> Vec<String> {
        OPENER.with(|slot| slot.borrow().clone().unwrap_or_default())
    }
}

#[cfg(test)]
impl Drop for RecordedOpener {
    fn drop(&mut self) {
        OPENER.with(|slot| *slot.borrow_mut() = None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pinion_core::reactive::Owner;

    /// Build a lone raw button edge — a single press or release of `button` with `mods` held. The
    /// held set is just that button on a press, empty on a release (a single-button click); the
    /// PINION-PR72 stream the oracle's `raw_pointer_button` consumes while a pane is tracking.
    fn raw_edge(
        button: PointerButton,
        edge: PointerEdge,
        mods: pinion_core::Modifiers,
    ) -> RawPointerButton {
        let buttons = match edge {
            PointerEdge::Down => PointerButtons::empty().with(button),
            PointerEdge::Up => PointerButtons::empty(),
        };
        RawPointerButton {
            button,
            edge,
            modifiers: mods,
            buttons,
            // A LONE edge, which is what this helper builds: pinion reports `1` for a first press
            // and for a release with no tracked press. `2` would make it a double-click — a
            // different gesture, and not one the tracking oracle reads (pinion R1422).
            click_count: 1,
        }
    }

    /// Feed pane `slot`'s oracle the way a frame does, with the widget laid out to EXACTLY the
    /// buffer — the case where the two halves of the mapping agree, so a test about links or
    /// mouse reports is not also a test about geometry. The divergent case (a widget wider than
    /// the pane, which is the ordinary case on a tiled surface) is
    /// `a_hover_lands_on_the_glyph_under_it_when_the_widget_is_wider_than_the_pane`.
    fn feed(slot: usize, buffer: &GridBuffer, proto: MouseProtocol) {
        let metric = CellMetric::DEFAULT;
        let rect = crate::terminal::cell_px(metric, buffer.cols(), buffer.rows());
        reconcile_pane_hyperlinks(slot, buffer, proto, rect, metric);
    }

    /// The pointer lands on the glyph it is over even when the pane's WIDGET spans more cells
    /// than the pane holds — the ordinary case on a tiled surface, because the daemon divides the
    /// arbitrated window in cells while this client's dock divides its surface in pixels.
    ///
    /// The measured shape: an 80-column, 24-row pane in a widget one cell wider and taller. The
    /// retired form scaled the hover fraction by the pane's own count, spreading 80 columns
    /// across 81 cells of pixels.
    ///
    /// **The probe points are chosen to DISCRIMINATE, and the first draft's were not.** The two
    /// forms differ by `frac * (span - count)`, which is below one cell everywhere and therefore
    /// crosses a cell boundary only for some fractions: at the widget's mid-point both answer 40,
    /// so a test written there passes on the very implementation it excludes (it did). Column 60
    /// and column 79 are past the crossing — `floor(0.7469 * 81) = 60` against
    /// `floor(0.7469 * 80) = 59` — and row 20 is the same on the other axis.
    #[test]
    fn a_hover_lands_on_the_glyph_under_it_when_the_widget_is_wider_than_the_pane() {
        Owner::new().run(|| {
            let metric = CellMetric::DEFAULT;
            let (cols, rows) = (80_u16, 24_u16);
            let mut screen = sprag_vt::Emulator::new(cols, rows);
            sprag_vt::VtPort::advance(&mut screen, b".");
            let buffer = sprag_grid::project(
                sprag_vt::VtPort::screen(&screen),
                sprag_vt::VtPort::palette(&screen),
            );
            // One cell of slack on each axis — what a pixel layout and a cell tiling routinely
            // disagree by, and what the smoke measured live as 38 painted over a 37-col buffer.
            let (span_x, span_y) = (f32::from(cols) + 1.0, f32::from(rows) + 1.0);
            let widget = crate::terminal::cell_px(metric, cols + 1, rows + 1);
            reconcile_pane_hyperlinks(6, &buffer, MouseProtocol::None, widget, metric);
            let state = use_pane_hover(6);

            // A pointer in the MIDDLE of the widget's cell `c` is the fraction `(c + 0.5) / span`.
            let at = |col: u16, row: u16| {
                (
                    (f32::from(col) + 0.5) / span_x,
                    (f32::from(row) + 0.5) / span_y,
                )
            };
            assert_eq!(state.cell_at(0.0, 0.0), (0, 0), "the origin cell");
            let (x, y) = at(60, 20);
            assert_eq!(
                state.cell_at(x, y),
                (60, 20),
                "the glyph under the pointer, not the one a stretched scale names",
            );
            let (x, y) = at(79, 23);
            assert_eq!(
                state.cell_at(x, y),
                (79, 23),
                "the pane's last cell is reachable — a stretched scale never gets there",
            );
            assert_eq!(
                state.cell_at(1.0, 1.0),
                (cols - 1, rows - 1),
                "the widget's far edge clamps to the PANE's last cell, not the widget's",
            );
        });
    }

    /// The one pointer -> cell rule: an offset already in cells, floored, clamped one short of
    /// the pane's own grid. (Was `frac_to_index`, whose signature took the count to SCALE by —
    /// the shape that made the stretched mapping expressible.)
    #[test]
    fn cell_index_floors_and_clamps_one_short() {
        use crate::terminal::cell_index;
        assert_eq!(cell_index(0.0, 80), 0);
        assert_eq!(cell_index(40.0, 80), 40);
        assert_eq!(cell_index(40.9, 80), 40, "a part-cell offset floors");
        assert_eq!(cell_index(79.0, 80), 79);
        assert_eq!(cell_index(80.0, 80), 79, "the extent clamps one short");
        assert_eq!(cell_index(-0.5, 80), 0, "negative clamps to 0");
        assert_eq!(cell_index(200.0, 80), 79, "past the end clamps to last");
        assert_eq!(cell_index(0.5, 0), 0, "empty grid -> 0");
        assert_eq!(cell_index(f32::NAN, 80), 0, "an unmeasured scale -> 0");
    }

    /// A widget's extent in cells is FRACTIONAL, and a zero cell size cannot divide.
    #[test]
    fn rect_cells_measures_the_widget_in_fractional_cells() {
        use crate::terminal::rect_cells;
        let metric = CellMetric::DEFAULT;
        let (cw, ch) = (metric.cell_w(), metric.cell_h());
        assert_eq!(rect_cells((cw * 80, ch * 24), metric), (80.0, 24.0));
        let (x, _) = rect_cells((cw * 80 + cw / 2, ch * 24), metric);
        assert!(
            (x - 80.5).abs() < f32::EPSILON,
            "a sub-cell remainder is kept, not floored away: {x}",
        );
        assert_eq!(rect_cells((0, 0), metric), (0.0, 0.0), "unmeasured");
    }

    /// A 6-char link on a 4-col screen (wraps) fed to the oracle: a hover over a link
    /// cell sets the hovered id + captures the press; a hover over plain text clears
    /// it + does not capture; a `send PointerDown` over the link records its URI.
    #[test]
    fn oracle_hovers_captures_and_activates_only_over_a_link() {
        Owner::new().run(|| {
            let screen = sprag_vt::Emulator::new(4, 3);
            let mut screen = screen;
            sprag_vt::VtPort::advance(
                &mut screen,
                b"\x1b]8;;https://ok\x1b\\ABCDEF\x1b]8;;\x1b\\gh",
            );
            let buffer = sprag_grid::project(
                sprag_vt::VtPort::screen(&screen),
                sprag_vt::VtPort::palette(&screen),
            );
            feed(0, &buffer, MouseProtocol::None);

            let mut oracle = HyperlinkOracle {
                state: use_pane_hover(0),
            };
            // Hover the first cell (col 0, row 0) — the link 'A'.
            oracle.pointer_move(0.01, 0.01);
            assert!(
                oracle.state.hovered.get().is_some(),
                "over a link -> hovered"
            );
            assert!(
                oracle.wants_pointer_capture(),
                "captures the press over a link"
            );
            // A click activates the hovered link's URI.
            oracle.activate();
            assert_eq!(
                oracle.state.activated.borrow().as_deref(),
                Some("https://ok")
            );
            // Hover a plain cell (row 2 is blank) — clears the hover, no capture.
            oracle.pointer_move(0.5, 0.9);
            assert!(
                oracle.state.hovered.get().is_none(),
                "off a link -> no hover"
            );
            assert!(
                !oracle.wants_pointer_capture(),
                "plain text press falls through to selection"
            );
        });
    }

    #[test]
    fn send_pointerdown_activates_via_introspection() {
        Owner::new().run(|| {
            let mut screen = sprag_vt::Emulator::new(20, 1);
            sprag_vt::VtPort::advance(&mut screen, b"\x1b]8;;https://ex\x1b\\link\x1b]8;;\x1b\\");
            let buffer = sprag_grid::project(
                sprag_vt::VtPort::screen(&screen),
                sprag_vt::VtPort::palette(&screen),
            );
            feed(1, &buffer, MouseProtocol::None);
            let mut oracle = HyperlinkOracle {
                state: use_pane_hover(1),
            };
            oracle.pointer_move(0.01, 0.5); // over 'l'
            // The router's real-click channel.
            let _ = ExternalIntrospect::invoke(
                &mut oracle,
                "send",
                IntrospectValue::Text("PointerDown".to_owned()),
            );
            assert_eq!(
                ExternalIntrospect::query(&oracle, "activated_uri").ok(),
                Some(IntrospectValue::Text("https://ex".to_owned()))
            );
        });
    }

    /// The reconcile drains a click-activated URI to the opener EXACTLY ONCE and clears
    /// it — asserted against the recording opener seam so the test never spawns the
    /// platform handler (`xdg-open mailto:…` would pop the desktop mail client). This
    /// is the contract the old spawn-through test could not check without an OS effect.
    #[test]
    fn reconcile_drains_opens_once_and_clears() {
        Owner::new().run(|| {
            let opener = RecordedOpener::install();
            let state = use_pane_hover(2);
            *state.activated.borrow_mut() = Some("mailto:x@example.com".to_owned());
            let empty = GridBuffer::new(1, 1);
            feed(2, &empty, MouseProtocol::None); // drains -> recorder (no spawn) + clears
            assert_eq!(
                opener.opened(),
                vec!["mailto:x@example.com".to_owned()],
                "the drained URI reached the opener exactly once"
            );
            assert!(
                state.activated.borrow().is_none(),
                "the activation was cleared after draining"
            );
            // A second reconcile with nothing activated opens nothing more.
            feed(2, &empty, MouseProtocol::None);
            assert_eq!(
                opener.opened().len(),
                1,
                "no re-open without a fresh activation"
            );
        });
    }

    #[test]
    fn scheme_gate_allows_web_and_rejects_dangerous_or_bare() {
        assert!(scheme_allowed("https://example.com"));
        assert!(scheme_allowed("HTTP://EXAMPLE.COM"), "case-insensitive");
        assert!(scheme_allowed("mailto:a@b.c"));
        assert!(scheme_allowed("file:///home/x"));
        assert!(!scheme_allowed("javascript:alert(1)"), "dangerous scheme");
        assert!(!scheme_allowed("data:text/html,x"), "not on the allowlist");
        assert!(!scheme_allowed("plain text no scheme"));
    }

    #[test]
    fn open_command_targets_the_platform_handler() {
        let cmd = open_command("https://example.com");
        let program = cmd.get_program().to_string_lossy().to_string();
        #[cfg(target_os = "macos")]
        assert_eq!(program, "open");
        #[cfg(target_os = "windows")]
        assert_eq!(program, "cmd");
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        assert_eq!(program, "xdg-open");
        // The URI is an argument (never a shell fragment) — no injection surface.
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert!(args.iter().any(|a| a == "https://example.com"));
    }

    // ----- Mouse tracking (the pane pointer authority) -----

    /// A native grid press arrives COMPOSITE (`"grid:PointerDown"` — the `{pane}#grid` sub-index);
    /// the RPC / test path sends the bare event name. Both must decode to the same event, or a
    /// native click is missed (the exact `== "PointerDown"` bug this replaced).
    #[test]
    fn send_event_name_decodes_composite_and_bare() {
        assert_eq!(send_event_name("grid:PointerDown"), "PointerDown");
        assert_eq!(send_event_name("grid:PointerUp"), "PointerUp");
        assert_eq!(
            send_event_name("PointerUp"),
            "PointerUp",
            "bare RPC/test form"
        );
    }

    /// While the child is tracking the mouse, the oracle OWNS the raw multi-button stream
    /// ([`wants_raw_pointer_buttons`]) and a left press/release becomes a LEFT report at the last
    /// hovered cell — the report path, not link / selection. Revert-proof: reverting
    /// `wants_raw_pointer_buttons` to `false` stops the router delivering the edges; dropping the
    /// `record_report` in `raw_pointer_button` empties the drained queue.
    #[test]
    fn tracking_captures_every_press_and_reports_left_press_then_release() {
        Owner::new().run(|| {
            // A plain screen with NO links — a report must not need a link under the cursor.
            let mut screen = sprag_vt::Emulator::new(8, 3);
            sprag_vt::VtPort::advance(&mut screen, b"hello");
            let buffer = sprag_grid::project(
                sprag_vt::VtPort::screen(&screen),
                sprag_vt::VtPort::palette(&screen),
            );
            feed(3, &buffer, MouseProtocol::Click); // the child is tracking
            let mut oracle = HyperlinkOracle {
                state: use_pane_hover(3),
            };
            oracle.pointer_move(0.3, 0.0); // a plain cell on row 0 (no link)
            assert!(
                oracle.wants_raw_pointer_buttons(),
                "a tracking pane owns the raw multi-button stream, link or not"
            );
            let cell = use_pane_hover(3).last_cell.get();
            let mods = pinion_core::Modifiers::default();
            oracle.raw_pointer_button(raw_edge(PointerButton::Left, PointerEdge::Down, mods));
            oracle.raw_pointer_button(raw_edge(PointerButton::Left, PointerEdge::Up, mods));
            let reports = take_pane_mouse_reports(3);
            assert_eq!(reports.len(), 2, "one press + one release queued");
            assert_eq!(
                (reports[0].button, reports[0].kind),
                (MouseButton::Left, MouseEventKind::Press),
            );
            assert_eq!(
                (reports[1].button, reports[1].kind),
                (MouseButton::Left, MouseEventKind::Release),
            );
            assert_eq!(
                (reports[0].col, reports[0].row),
                cell,
                "reported at the last hovered cell"
            );
            assert!(
                take_pane_mouse_reports(3).is_empty(),
                "the drain emptied the queue (send exactly once)"
            );
            assert!(
                use_pane_hover(3).activated.borrow().is_none(),
                "a tracking press reports, it does not activate a link"
            );
        });
    }

    /// With NO tracking, a press over a link still ACTIVATES it (via the composite decode — the
    /// native-click fix) and queues NO mouse report. Revert-proof: the old exact `== "PointerDown"`
    /// match would miss the composite payload, leaving `activated` `None`.
    #[test]
    fn not_tracking_a_press_activates_the_link_and_queues_no_report() {
        Owner::new().run(|| {
            let mut screen = sprag_vt::Emulator::new(20, 1);
            sprag_vt::VtPort::advance(&mut screen, b"\x1b]8;;https://ex\x1b\\link\x1b]8;;\x1b\\");
            let buffer = sprag_grid::project(
                sprag_vt::VtPort::screen(&screen),
                sprag_vt::VtPort::palette(&screen),
            );
            feed(4, &buffer, MouseProtocol::None); // NOT tracking
            let mut oracle = HyperlinkOracle {
                state: use_pane_hover(4),
            };
            oracle.pointer_move(0.01, 0.5); // over 'l' (a link cell)
            let _ = ExternalIntrospect::invoke(
                &mut oracle,
                "send",
                IntrospectValue::Text("grid:PointerDown".to_owned()),
            );
            assert_eq!(
                use_pane_hover(4).activated.borrow().as_deref(),
                Some("https://ex"),
                "a native composite press activates the link"
            );
            assert!(
                take_pane_mouse_reports(4).is_empty(),
                "not tracking => no mouse report"
            );
        });
    }

    /// The pure notch folder: a whole line is one notch; sub-line touchpad steps carry until they
    /// cross a whole line then fire once; a reversal cancels (no phantom up-then-down); a fling
    /// reports its whole magnitude.
    #[test]
    fn wheel_notches_accumulate_and_carry_the_fraction() {
        assert_eq!(accumulate_wheel_notches(0.0, 1.0), (0.0, 1));
        let (accum, n) = accumulate_wheel_notches(0.0, 0.4);
        assert_eq!(n, 0, "0.4 line: no notch yet");
        let (accum, n) = accumulate_wheel_notches(accum, 0.4);
        assert_eq!(n, 0, "0.8 line: still none");
        let (accum, n) = accumulate_wheel_notches(accum, 0.4);
        assert_eq!(n, 1, "1.2 line: one notch fires");
        assert!((accum - 0.2).abs() < 1e-5, "0.2 remainder carried");
        let (_, n) = accumulate_wheel_notches(0.6, -0.9);
        assert_eq!(n, 0, "-0.3 total: no notch, no phantom up-and-down");
        assert_eq!(accumulate_wheel_notches(0.0, -3.0), (0.0, -3));
    }

    /// `wheel_reports` maps the sign to xterm's pseudo-buttons (down = 65, up = 64), emits one
    /// press-only report per whole notch at the cell under the pointer, and carries the real
    /// modifiers the wheel handler holds. A sub-line pan yields nothing (accumulated per pane).
    #[test]
    fn wheel_reports_map_sign_to_pseudo_button_carry_cell_and_mods() {
        Owner::new().run(|| {
            let mods = Modifiers::default();
            // Scroll DOWN (+lines, toward the live bottom) -> wheel-down.
            let down = wheel_reports(0, 4, 2, 1.0, mods);
            assert_eq!(down.len(), 1);
            assert_eq!(down[0].button, MouseButton::WheelDown);
            assert_eq!(
                down[0].kind,
                MouseEventKind::Press,
                "a wheel step is press-only (no release)"
            );
            assert_eq!(
                (down[0].col, down[0].row),
                (4, 2),
                "addresses the cell under the pointer"
            );
            // Scroll UP (-lines) -> wheel-up, and a multi-line fling -> one report per notch.
            let up = wheel_reports(0, 0, 0, -3.0, mods);
            assert_eq!(up.len(), 3);
            assert!(up.iter().all(|e| e.button == MouseButton::WheelUp));
            // A sub-line pan on a fresh pane accumulates, reporting nothing yet.
            assert!(wheel_reports(1, 0, 0, 0.3, mods).is_empty());
            // Ctrl held at the wheel handler reaches the report (the wheel path HAS real mods,
            // unlike the press-edge capture channel).
            let ctrl = Modifiers {
                ctrl: true,
                ..Modifiers::default()
            };
            let reports = wheel_reports(2, 1, 1, 1.0, ctrl);
            assert_eq!(reports.len(), 1);
            assert_eq!(reports[0].mods, ctrl, "the held modifiers reach the report");
        });
    }

    /// Feed a plain 8x3 grid to a pane oracle at a given tracking level, returning the oracle over
    /// its shared state (helper for the drag / motion tests).
    #[cfg(test)]
    /// ⚠⚠ **THE AI-FIRST CLICK, DRIVEN** — `activate` had no test at all until R352, which is
    /// half of why nobody noticed it was undeclared.
    ///
    /// It is also what makes the declaration load-bearing rather than decorative: the dispatch is
    /// guarded on the schema ([`crate::wire_claim::declares_verb`]), so deleting this verb's line
    /// from the surface's own declaration makes this call answer `UnknownPath` and reddens here.
    /// **Measured, both ways** — the gate below cannot see an omission and this one can.
    #[test]
    fn the_declared_activate_verb_activates_the_hovered_link() {
        Owner::new().run(|| {
            let mut oracle = oracle_at(0, MouseProtocol::None);
            assert!(
                !matches!(
                    oracle.invoke("activate", IntrospectValue::Null),
                    Err(InvokeError::UnknownPath)
                ),
                "the verb this surface publishes is the verb it dispatches",
            );
        });
    }

    /// ⚠⚠ **THIS SURFACE'S DECLARATION SAYS WHAT ITS PATHS ARE**, and it was wrong in BOTH
    /// directions until R352: `send` sat on the read channel, and `activate` — the AI-first,
    /// no-pixel click this oracle exists to offer — was dispatched and declared NOWHERE, so the one
    /// verb a client would come here for could not be discovered. See [`crate::wire_claim`].
    #[test]
    fn a_declared_path_is_what_it_claims() {
        Owner::new().run(|| {
            let mut surface = oracle_at(0, MouseProtocol::None);
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
            let mut surface = oracle_at(0, MouseProtocol::None);
            let table = crate::wire_claim::grammar::hyperlink();

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
                1,
                "one probe: `send`'s payload, the whole `args` value of its scalar form",
            );

            assert_eq!(
                sprag_conformance::a_nullary_form_is_a_verb_that_needs_nothing(
                    table,
                    &mut |action, args| surface.invoke(action, args)
                )
                .count_or_panic(),
                2,
                "two calls for `activate`, whose subject is the link `hover_index` names",
            );
        });
    }

    fn oracle_at(slot: usize, proto: MouseProtocol) -> HyperlinkOracle {
        let mut screen = sprag_vt::Emulator::new(8, 3);
        sprag_vt::VtPort::advance(&mut screen, b"........");
        let buffer = sprag_grid::project(
            sprag_vt::VtPort::screen(&screen),
            sprag_vt::VtPort::palette(&screen),
        );
        feed(slot, &buffer, proto);
        HyperlinkOracle {
            state: use_pane_hover(slot),
        }
    }

    /// Under button-event tracking (1002) a captured LEFT press then a cell-changing move reports a
    /// DRAG (button Left) at the new cell; a bare move (no button) reports nothing; a move that does
    /// not change cell reports nothing. Revert-proof: dropping the `pointer_move` drag branch empties
    /// the drag; removing the cell-change guard would report on the no-op move.
    #[test]
    fn button_event_tracking_reports_left_drag_only_on_a_cell_change_while_held() {
        Owner::new().run(|| {
            let mut oracle = oracle_at(5, MouseProtocol::ButtonEvent);
            // A bare move (no button) under 1002 reports NOTHING (1002 = drag only, no bare motion).
            oracle.pointer_move(0.05, 0.05); // ~cell (0,0)
            assert!(
                take_pane_mouse_reports(5).is_empty(),
                "1002 reports no bare motion"
            );
            // A raw left press (tracking) then a drag to a new cell -> one Drag report there.
            oracle.raw_pointer_button(raw_edge(
                PointerButton::Left,
                PointerEdge::Down,
                pinion_core::Modifiers::default(),
            ));
            oracle.pointer_move(0.95, 0.05); // ~cell (7,0)
            let reports = take_pane_mouse_reports(5);
            let drags: Vec<_> = reports
                .iter()
                .filter(|r| r.kind == MouseEventKind::Drag)
                .collect();
            assert_eq!(drags.len(), 1, "one drag on the cell change");
            assert_eq!(drags[0].button, MouseButton::Left);
            assert_eq!((drags[0].col, drags[0].row), (7, 0), "drag at the new cell");
            // A move within the SAME cell reports nothing (cell-granular, not per-pixel).
            oracle.pointer_move(0.96, 0.06); // still ~cell (7,0)
            assert!(
                take_pane_mouse_reports(5)
                    .iter()
                    .all(|r| r.kind != MouseEventKind::Drag),
                "no drag without a cell change"
            );
        });
    }

    /// Under any-event tracking (1003) a bare move (no button) reports MOTION (button None); with a
    /// button held the same move is a DRAG, not bare motion. Revert-proof: dropping the motion
    /// branch empties the motion assertion.
    #[test]
    fn any_event_tracking_reports_bare_motion_then_drag_when_held() {
        Owner::new().run(|| {
            let mut oracle = oracle_at(6, MouseProtocol::AnyEvent);
            // A bare move to a new cell reports MOTION with no button.
            oracle.pointer_move(0.5, 0.05); // ~cell (4,0), changed from init (0,0)
            let motions: Vec<_> = take_pane_mouse_reports(6)
                .into_iter()
                .filter(|r| r.kind == MouseEventKind::Motion)
                .collect();
            assert_eq!(
                motions.len(),
                1,
                "one bare-motion report on the cell change"
            );
            assert_eq!(motions[0].button, MouseButton::None);
            // With a button held (raw left press), a move is a DRAG (Left), not bare motion.
            oracle.raw_pointer_button(raw_edge(
                PointerButton::Left,
                PointerEdge::Down,
                pinion_core::Modifiers::default(),
            ));
            oracle.pointer_move(0.95, 0.05); // ~cell (7,0)
            let held = take_pane_mouse_reports(6);
            assert!(
                held.iter()
                    .any(|r| r.kind == MouseEventKind::Drag && r.button == MouseButton::Left),
                "a held move is a drag"
            );
            assert!(
                !held.iter().any(|r| r.kind == MouseEventKind::Motion),
                "no bare motion while a button is held"
            );
        });
    }

    /// Under click tracking (1000) a move — held or not — reports NO drag / motion (only the
    /// press/release edges). Guards the level gate: 1000 must not leak the 1002/1003 behaviour.
    #[test]
    fn click_tracking_reports_no_drag_or_motion() {
        Owner::new().run(|| {
            let mut oracle = oracle_at(7, MouseProtocol::Click);
            oracle.pointer_move(0.1, 0.1);
            oracle.raw_pointer_button(raw_edge(
                PointerButton::Left,
                PointerEdge::Down,
                pinion_core::Modifiers::default(),
            ));
            oracle.pointer_move(0.9, 0.1); // a held move
            assert!(
                take_pane_mouse_reports(7)
                    .iter()
                    .all(|r| matches!(r.kind, MouseEventKind::Press | MouseEventKind::Release)),
                "1000 reports only press/release, never drag or motion"
            );
        });
    }

    /// PINION-PR72 S3: the raw stream reports the MIDDLE and RIGHT buttons on BOTH edges (not just
    /// left) — the arc pinion's legacy send wire never delivered (right had no Released arm, middle
    /// no press edge). Revert-proof: mapping every button to Left would fail the button asserts.
    #[test]
    fn right_and_middle_press_release_report_their_button_both_edges() {
        Owner::new().run(|| {
            let mut oracle = oracle_at(9, MouseProtocol::Click);
            oracle.pointer_move(0.3, 0.3);
            let mods = pinion_core::Modifiers::default();
            for button in [PointerButton::Right, PointerButton::Middle] {
                oracle.raw_pointer_button(raw_edge(button, PointerEdge::Down, mods));
                oracle.raw_pointer_button(raw_edge(button, PointerEdge::Up, mods));
            }
            let reports = take_pane_mouse_reports(9);
            let kinds: Vec<_> = reports.iter().map(|r| (r.button, r.kind)).collect();
            assert_eq!(
                kinds,
                vec![
                    (MouseButton::Right, MouseEventKind::Press),
                    (MouseButton::Right, MouseEventKind::Release),
                    (MouseButton::Middle, MouseEventKind::Press),
                    (MouseButton::Middle, MouseEventKind::Release),
                ],
                "each button reports its own identity on both the press and release edge"
            );
        });
    }

    /// PINION-PR72 S4 press-mods: the raw stream carries the modifiers held at EACH edge — the press
    /// now too (the legacy `PointerDown` wire dropped them). A Ctrl+Shift left click reports both on
    /// press and release. Revert-proof: hard-coding `Modifiers::default()` in `record_report` for the
    /// edge would drop them.
    #[test]
    fn a_raw_press_carries_the_modifiers_held_at_that_edge() {
        Owner::new().run(|| {
            let mut oracle = oracle_at(10, MouseProtocol::Click);
            oracle.pointer_move(0.3, 0.3);
            let ctrl_shift = pinion_core::Modifiers {
                ctrl: true,
                shift: true,
                ..pinion_core::Modifiers::default()
            };
            oracle.raw_pointer_button(raw_edge(PointerButton::Left, PointerEdge::Down, ctrl_shift));
            oracle.raw_pointer_button(raw_edge(PointerButton::Left, PointerEdge::Up, ctrl_shift));
            let reports = take_pane_mouse_reports(10);
            let want = Modifiers {
                ctrl: true,
                shift: true,
                ..Modifiers::default()
            };
            assert_eq!(reports.len(), 2);
            assert!(
                reports.iter().all(|r| r.mods == want),
                "both edges carry the Ctrl+Shift modifiers held at the click"
            );
        });
    }

    /// A held RIGHT button drags with the RIGHT button under 1002 — the generalized held-button path
    /// (`primary_held` off the raw held set), not the old left-only flag. Revert-proof: reverting
    /// `held` to a bool would drag as Left.
    #[test]
    fn a_right_button_drag_reports_the_right_button() {
        Owner::new().run(|| {
            let mut oracle = oracle_at(11, MouseProtocol::ButtonEvent);
            oracle.pointer_move(0.05, 0.05); // ~cell (0,0)
            let mods = pinion_core::Modifiers::default();
            oracle.raw_pointer_button(raw_edge(PointerButton::Right, PointerEdge::Down, mods));
            oracle.pointer_move(0.95, 0.05); // ~cell (7,0) — a drag
            let drags: Vec<_> = take_pane_mouse_reports(11)
                .into_iter()
                .filter(|r| r.kind == MouseEventKind::Drag)
                .collect();
            assert_eq!(drags.len(), 1, "one drag on the cell change");
            assert_eq!(
                drags[0].button,
                MouseButton::Right,
                "drags with the held button"
            );
            // Releasing disarms the drag (held set empties).
            oracle.raw_pointer_button(raw_edge(PointerButton::Right, PointerEdge::Up, mods));
            oracle.pointer_move(0.5, 0.05); // another cell change, no button held
            assert!(
                take_pane_mouse_reports(11)
                    .iter()
                    .all(|r| r.kind != MouseEventKind::Drag),
                "no drag after the release disarms the held button"
            );
        });
    }

    /// Queuing a report REQUESTS A REPAINT so `reconcile_frame` drains it even when the pointer
    /// event repaints nothing on its own (a bare motion over plain text leaves `hovered` unchanged).
    /// Asserted against a provided counting [`RepaintSink`] — revert-proof: dropping the
    /// `request_repaint()` call in `record_report` leaves the count at 0.
    #[test]
    fn a_queued_report_requests_a_repaint_so_the_drain_frame_runs() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        #[derive(Debug)]
        struct CountingSink(Arc<AtomicUsize>);
        impl pinion_core::RepaintSink for CountingSink {
            fn request_repaint(&self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }

        let owner = Owner::new();
        let count = Arc::new(AtomicUsize::new(0));
        let sink: Arc<dyn pinion_core::RepaintSink> = Arc::new(CountingSink(Arc::clone(&count)));
        pinion_core::REPAINT_SINK.provide(&owner, sink);
        owner.run(|| {
            let mut oracle = oracle_at(8, MouseProtocol::AnyEvent);
            // A bare motion to a new cell queues a report -> must have requested a repaint.
            oracle.pointer_move(0.5, 0.5);
        });
        assert!(
            count.load(Ordering::SeqCst) > 0,
            "queuing a mouse report must request a repaint so the drain frame runs"
        );
    }
}
