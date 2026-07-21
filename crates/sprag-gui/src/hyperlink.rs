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

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::process::Command;
use std::rc::Rc;

use pinion_core::GridBuffer;
use pinion_core::external::{
    Backend, BackendFallback, BackendSupport, External, ExternalIntrospect, InterveneError,
    IntrospectSchema, IntrospectValue, InvokeError, RepaintOwner, SchemaField, ThreadOwnership,
};
use pinion_core::reactive::{Owner, Signal};
use pinion_core::term_grid::HyperlinkId;
use pinion_core::widget_core::ExtraExternal;

use crate::terminal::{pane_cache_key, pane_tag};

/// The URI schemes a click is allowed to open — the safety gate so a hostile child
/// cannot emit an OSC-8 link to a dangerous scheme that runs on click. tmux has no
/// OSC-8 open at all; a scheme allowlist is the tmux-superior-yet-safe middle.
const ALLOWED_SCHEMES: [&str; 5] = ["http", "https", "mailto", "file", "ftp"];

/// A pane's visible link cells: `(col, row)` -> the cell's link id (in the pane's
/// current buffer table) and the URI a click opens.
type LinkMap = HashMap<(u16, u16), (HyperlinkId, Rc<str>)>;

/// Per-pane client-local hover state, shared between the oracle [`External`] (writes
/// `hovered` / `activated` from pointer events) and the view + reconcile (feed
/// `links` / `geometry`, read `hovered` for the overlay, drain `activated` to open).
/// The scrollbar's `Rc<ScrollState>` shape: cached per pane so every site resolves
/// the one instance, and feeding is a plain write rather than an `intervene`.
#[derive(Debug)]
pub(crate) struct HoverState {
    /// Grid geometry the oracle maps a `[0,1]` hover fraction against.
    cols: Cell<u16>,
    rows: Cell<u16>,
    /// `(col, row)` -> the cell's link (its id in the pane's CURRENT buffer table,
    /// and the URI a click opens). Fed each frame from the live projection.
    links: RefCell<LinkMap>,
    /// The link the pointer is over (its `HyperlinkId` in the current buffer), or
    /// `None`. A reactive `Signal` so the view repaints the hover highlight when it
    /// changes (the same reactive path selection / scroll use).
    hovered: Signal<Option<HyperlinkId>>,
    /// A URI a click activated, awaiting [`reconcile_pane_hyperlinks`] to open it.
    activated: RefCell<Option<String>>,
}

impl Default for HoverState {
    fn default() -> Self {
        Self {
            cols: Cell::new(0),
            rows: Cell::new(0),
            links: RefCell::new(HashMap::new()),
            hovered: Signal::new(None),
            activated: RefCell::new(None),
        }
    }
}

impl HoverState {
    /// The cell a `[0,1]x[0,1]` pane-rect hover fraction lands on.
    fn cell_at(&self, x_rel: f32, y_rel: f32) -> (u16, u16) {
        (
            frac_to_index(x_rel, self.cols.get()),
            frac_to_index(y_rel, self.rows.get()),
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
}

/// Map a `[0,1]` rect fraction to a 0-based cell index in `[0, count-1]` (floor,
/// clamped one short of the extent — the same rounding as
/// `CellMetric::frac_to_px` + `px_to_cell`; an empty grid -> `0`).
fn frac_to_index(frac: f32, count: u16) -> u16 {
    if count == 0 {
        return 0;
    }
    let scaled = frac * f32::from(count);
    if scaled < 0.0 {
        0
    } else if scaled >= f32::from(count) {
        count - 1
    } else {
        // 0 <= scaled < count <= u16::MAX, so the floor fits a u16.
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "scaled is clamped to [0, count) and count <= u16::MAX"
        )]
        {
            scaled.floor() as u16
        }
    }
}

/// Pane `i`'s shared hover state (Owner::cache-backed, the scrollbar `ScrollState`
/// pattern) — resolved by the oracle, the view, and the reconcile to the ONE slot.
pub(crate) fn use_pane_hover(i: usize) -> Rc<HoverState> {
    Owner::current()
        .expect("use_pane_hover() requires an active Owner scope")
        .cache(pane_cache_key("hyperlink_hover", i), || {
            Rc::new(HoverState::default())
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

/// Feed pane `i`'s oracle its current link map + geometry from the live `buffer`, and
/// DRAIN any click-activated URI to open it. Runs per-frame from
/// [`reconcile_frame`](crate::TerminalViewer) (the sanctioned off-thread-fact -> UI
/// seam), NOT the pure view. The link map is rebuilt from the buffer's
/// `TermCell::hyperlink` + interning table so a click resolves the exact URI the
/// hovered cell shows. A hovered id that no longer resolves (the buffer changed under
/// a paused pointer) is cleared so a stale highlight cannot linger.
pub(crate) fn reconcile_pane_hyperlinks(i: usize, buffer: &GridBuffer) {
    let state = use_pane_hover(i);
    let cols = buffer.cols();
    let rows = buffer.rows();
    state.cols.set(cols);
    state.rows.set(rows);
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

    /// Capture the press ONLY while over a link, so a click on a link activates it
    /// but a press on plain text falls through to text selection / focus. Dynamic
    /// from `hovered` (set by the last `pointer_move`).
    fn wants_pointer_capture(&self) -> bool {
        self.state.hovered.get().is_some()
    }

    /// Each hover move delivers a `[0,1]` pane-rect fraction: reconstruct the cell and
    /// set the hovered link (or `None` off a link).
    fn pointer_move(&mut self, x_rel: f32, y_rel: f32) {
        let (col, row) = self.state.cell_at(x_rel, y_rel);
        let hovered = self
            .state
            .links
            .borrow()
            .get(&(col, row))
            .map(|(id, _)| *id);
        self.state.hovered.set(hovered);
    }

    fn introspect(&self) -> Option<&dyn ExternalIntrospect> {
        Some(self)
    }

    fn introspect_mut(&mut self) -> Option<&mut dyn ExternalIntrospect> {
        Some(self)
    }
}

impl ExternalIntrospect for HyperlinkOracle {
    fn schema(&self) -> IntrospectSchema {
        IntrospectSchema::new(
            const {
                &[
                    SchemaField::new("hover_index", "int"),
                    SchemaField::new("activated_uri", "string"),
                    SchemaField::new("send", "string"),
                ]
            },
        )
    }

    fn query(&self, path: &str) -> Option<IntrospectValue> {
        match path {
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
        match path {
            // The router press/release channel (R1401): a PointerDown over a link
            // activates it (the click). Other sends are ignored.
            "send" => {
                if matches!(args, IntrospectValue::Text(ref name) if name == "PointerDown") {
                    self.activate();
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

    #[test]
    fn frac_to_index_floors_and_clamps_one_short() {
        assert_eq!(frac_to_index(0.0, 80), 0);
        assert_eq!(frac_to_index(0.5, 80), 40);
        assert_eq!(
            frac_to_index(1.0, 80),
            79,
            "clamped one short of the extent"
        );
        assert_eq!(frac_to_index(-0.5, 80), 0, "negative clamps to 0");
        assert_eq!(frac_to_index(2.0, 80), 79, "past the end clamps to last");
        assert_eq!(frac_to_index(0.5, 0), 0, "empty grid -> 0");
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
            let buffer = sprag_grid::project(sprag_vt::VtPort::screen(&screen));
            reconcile_pane_hyperlinks(0, &buffer);

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
            let buffer = sprag_grid::project(sprag_vt::VtPort::screen(&screen));
            reconcile_pane_hyperlinks(1, &buffer);
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
                ExternalIntrospect::query(&oracle, "activated_uri"),
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
            reconcile_pane_hyperlinks(2, &empty); // drains -> recorder (no spawn) + clears
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
            reconcile_pane_hyperlinks(2, &empty);
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
}
