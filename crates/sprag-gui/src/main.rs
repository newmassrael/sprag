//! `sprag-gui` — the read-only **GPU windowed terminal viewer** (R24).
//!
//! A window that paints one terminal pane's **live** screen. It is the human
//! observation path; the north star (an AI reading/driving the terminal as
//! *data*) is the headless `sprag-host` RPC path, which needs none of this.
//! This binding is therefore a faithful pixel projection of the *same* scene
//! the AI reads — it introduces no second terminal-state authority.
//!
//! ## How it stays on the substrate (no hacks)
//!
//! The hard part of a live terminal viewer is that a child process writes its
//! PTY from a **separate OS thread**, so the static `view` must read changing,
//! cross-thread (`Send`) data and the window must repaint on change without
//! owning the event loop. pinion's R999 `RepaintSink` seam
//! ([`use_repaint_sink`]) is exactly that edge — this binding is its terminal
//! consumer, mirroring pinion's `examples/hello-live-data` reference:
//!
//! - [`use_terminal`] self-creates the [`Workspace`] + initial pane once in an
//!   `Owner::cache` hook (the `use_storage` pattern — nothing flows through
//!   `main`), spawning the PTY in [`WidgetCore::create_extra_externals`] at
//!   boot, never in the pure `view`.
//! - The pane is spawned via [`Workspace::spawn_with_dirty`] (the sprag R23
//!   hook) with an `on_dirty` callback that calls
//!   [`RepaintSink::request_repaint`](pinion_core::RepaintSink::request_repaint):
//!   each PTY batch wakes the shell, the next frame's `view` re-reads the
//!   screen. Event-driven, no animation-loop polling. `State` stays `()`
//!   (`Copy`) — the producer-authoritative screen is read directly, R969.
//! - [`grid_scene`] reuses the **single** projection
//!   ([`sprag_host::text_grid_node`] → `sprag_grid::project`) and only attaches
//!   a [`LayoutStyle`] so the shell's layout pass derives the cell `(cols,
//!   rows)` from the resolved rect (the §3 GUI winsize SSOT). No projection is
//!   duplicated here.
//!
//! Input (keyboard → PTY) is a later additive round that reuses the existing
//! `sprag-input` encoder + `SpragPaneExternal`; this viewer is display-only
//! (a [`StubExternal`], no focusable tags).

use pinion_a11y::{AccessNode, WidgetA11y};
use pinion_core::external::{External, StubExternal};
use pinion_core::reactive::Owner;
use pinion_core::scene::ContainerNode;
use pinion_core::style::{BoxStyle, LayoutStyle, Size};
use pinion_core::theme::{use_theme, ColorRole, Theme};
use pinion_core::use_repaint_sink;
use pinion_core::widget_core::ExtraExternal;
use pinion_core::{CellMetric, Frame, Scene, WidgetCore};
use pinion_shell::{vello_renderer_impl, SizeStrategy, WidgetView};
use pinion_text::LayoutCache;
use sprag_terminal::{CommandBuilder, Workspace};
use sprag_vt::Screen;
use std::rc::Rc;

include!(concat!(env!("OUT_DIR"), "/app.rs"));
vello_renderer_impl!(SpragGuiRenderer, SpragGuiRendererError);

/// The initial pane is an 80 x 24 cell grid (the classic terminal size). The
/// cell metric is **measured** from the resolved monospace face at
/// [`FONT_SIZE_PX`] via pinion's R1002 `measure_monospace_cell`, so `cell_w`
/// equals the painted glyph advance *by construction* (tight, true-monospace
/// spacing — no guessed aspect) and `cell_h` contains the natural line box (no
/// descender clip, R1001). The window is sized to the measured grid's exact
/// pixel extent (the headless host's inverse: here the window drives the
/// winsize, §3 GUI ownership).
const COLS: u16 = 80;
const ROWS: u16 = 24;
/// Default glyph size (logical px) — readable; the font-size SSOT the cell is
/// measured from. `SPRAG_GUI_FONT=<px>` overrides it live (larger ⇒ bigger).
const FONT_SIZE_PX: u32 = 20;

/// The glyph size: `SPRAG_GUI_FONT=<px>` overrides the readable default
/// ([`FONT_SIZE_PX`]) without a rebuild; same env-config pattern as
/// `SPRAG_GUI_CMD`. A malformed or zero value falls back to the default.
fn font_size_px() -> u32 {
    std::env::var("SPRAG_GUI_FONT")
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .filter(|&px| px > 0)
        .unwrap_or(FONT_SIZE_PX)
}

/// Measure the monospace cell for `font_size_px` (pinion R1002). Creates a
/// throwaway [`LayoutCache`] (it shapes, needing a `FontContext`), so this runs
/// ONCE at boot — never in the per-frame `view`. Falls back to the 8 x 16
/// baseline only if the probe degenerates (never for a real monospace face).
fn measure_cell(font_size_px: u32) -> CellMetric {
    let mut cache = LayoutCache::new();
    cache
        .measure_monospace_cell(font_size_px)
        .unwrap_or(CellMetric::DEFAULT)
}

/// The fixed window size in px — the measured grid's exact pixel extent.
fn window_px(metric: CellMetric) -> (u32, u32) {
    (
        u32::from(COLS) * metric.cell_w(),
        u32::from(ROWS) * metric.cell_h(),
    )
}

/// Shared [`ThemeProvider`] cache key (the surface fill behind the grid).
const THEME_TAG: &str = "app";
/// Paint-root + [`StubExternal`] anchor tag (`V::tag()` on the root container).
const ROOT_TAG: &str = "sprag_gui";
/// `Owner::cache` key for the live workspace (created once at boot).
const SESSION_KEY: &str = "sprag_gui.session";

/// Self-create (once) the live terminal [`Workspace`] + initial pane, wiring
/// the pane's PTY-reader `on_dirty` to the shell's [`RepaintSink`] so child
/// output repaints the window (the sprag R23 hook → pinion R999 seam). Spawns
/// the PTY on first call — therefore invoked from `create_extra_externals`
/// (boot), never the pure `view`.
///
/// [`use_repaint_sink`] is resolved *before* the `Owner::cache` factory so the
/// factory never re-enters `Owner::cache` (the nested-factory guard).
fn use_terminal() -> Rc<TerminalView> {
    let owner = Owner::current().expect("use_terminal() requires an active Owner scope");
    let sink = use_repaint_sink();
    owner.cache(SESSION_KEY, move || {
        // Measure the font-derived cell ONCE here at boot (it shapes; never in
        // the per-frame view), then spawn the pane wired to the repaint sink.
        let font_size_px = font_size_px();
        let metric = measure_cell(font_size_px);
        let (command, label) = pane_command();
        let mut workspace = Workspace::new((COLS, ROWS));
        workspace
            .spawn_with_dirty(
                command,
                label,
                COLS,
                ROWS,
                Some(Box::new(move || sink.request_repaint())),
            )
            .expect("spawn the initial sprag-gui pane");
        TerminalView {
            workspace,
            metric,
            font_size_px,
        }
    })
}

/// The booted terminal: the live [`Workspace`] plus the font-measured cell
/// metric and glyph size (measured once at boot in [`use_terminal`], read each
/// frame by `view` — never re-measured).
struct TerminalView {
    workspace: Workspace,
    metric: CellMetric,
    font_size_px: u32,
}

/// The program the initial pane runs, plus its introspection label.
///
/// `SPRAG_GUI_CMD` (whitespace-split: program + args) overrides — parity with
/// the headless `sprag-term -- <program> [args]`; otherwise the user's `$SHELL`
/// (then `/bin/sh`). Read from the environment, not threaded through `main`,
/// matching pinion's `Owner::cache` config-from-env pattern.
fn pane_command() -> (CommandBuilder, String) {
    let spec = std::env::var("SPRAG_GUI_CMD").unwrap_or_default();
    let mut parts = spec.split_whitespace();
    let program = match parts.next() {
        Some(p) => p.to_owned(),
        None => std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned()),
    };
    let label = program.clone();
    let mut command = CommandBuilder::new(&program);
    for arg in parts {
        command.arg(arg);
    }
    command.env("TERM", "xterm-256color");
    (command, label)
}

/// Project a pane's live [`Screen`] into a laid-out `Scene::TextGrid`.
///
/// Reuses the single projection ([`sprag_host::text_grid_node`]) and only adds
/// a [`LayoutStyle`]: an absolute position + the window size, so the layout
/// pass resolves a deterministic rect and derives the cell `(cols, rows)` from
/// it (the §3 GUI winsize SSOT). No cell projection is duplicated here.
fn grid_scene(screen: &Screen, metric: CellMetric, font_size_px: u32) -> Scene {
    let (w, h) = window_px(metric);
    Scene::TextGrid(
        sprag_host::text_grid_node(screen, metric)
            // Paint at exactly the measured size so the rendered advance equals
            // `cell_w` by construction (R1002), not the R1001 cell-fit re-derive.
            .with_font_size_px(font_size_px)
            .with_layout(
                LayoutStyle::new()
                    .with_absolute_position(0, 0)
                    .with_size(Size::px(w, h)),
            ),
    )
}

/// Wrap the pane grid in the surface-filled paint root (tagged [`ROOT_TAG`]).
/// Pure: the live read happens in `view`; this is the static composition that
/// the unit test exercises without spawning a PTY.
fn compose(grid: Scene, theme: &Theme, metric: CellMetric) -> Scene {
    let (w, h) = window_px(metric);
    Scene::Container(
        ContainerNode::new(vec![grid])
            .with_tag(ROOT_TAG)
            .with_style(BoxStyle::filled(theme.resolve(ColorRole::Surface)))
            .with_layout(LayoutStyle::new().with_size(Size::px(w, h))),
    )
}

/// view-fn (§6.3): pure sync `() -> Scene`. Reads the producer-authoritative
/// screen of the (single) pane each frame and paints it; the producer thread
/// (the PTY reader) lives in `create_extra_externals`, not here.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn view(_state: (), _frame: &Frame) -> Scene {
    let theme = use_theme(THEME_TAG).theme_animated();
    let tv = use_terminal();
    let grid = tv.workspace.panes().first().map_or_else(
        || Scene::Container(ContainerNode::new(Vec::new())),
        |pane| {
            pane.session()
                .with_screen(|screen| grid_scene(screen, tv.metric, tv.font_size_px))
        },
    );
    compose(grid, &theme, tv.metric)
}

struct TerminalViewer;

impl WidgetCore for TerminalViewer {
    type State = ();
    type Event = ();

    /// Display-only: the only anchor is the no-op [`StubExternal`] at
    /// [`ROOT_TAG`]. No input routing into the grid yet (a later additive
    /// round reuses `sprag-input` / `SpragPaneExternal`).
    fn create_external() -> Box<dyn External> {
        Box::new(StubExternal::new())
    }

    /// Boot the live terminal (spawn the PTY + wire `on_dirty` → repaint)
    /// before the first paint, off the pure `view`.
    fn create_extra_externals() -> Vec<ExtraExternal> {
        let _ = use_terminal();
        Vec::new()
    }

    fn tag() -> &'static str {
        ROOT_TAG
    }

    fn read_state(_scene: &Scene) {}

    fn view(state: (), frame: &Frame) -> Scene {
        view(state, frame)
    }

    fn event_name(_event: ()) -> &'static str {
        "__internal__"
    }

    fn focusable_tags() -> Vec<&'static str> {
        Vec::new()
    }

    fn title() -> &'static str {
        "sprag terminal viewer (R24 read-only windowed host)"
    }
}

impl WidgetA11y for TerminalViewer {
    /// No a11y nodes yet — the cell data model is read via the AI-first
    /// `scene/snapshot` path (the headless host). A per-cell screen-reader
    /// tree is a later slice; returning empty is the honest state.
    fn access_node(_state: &(), _focused: Option<&str>) -> Vec<AccessNode> {
        Vec::new()
    }
}

impl WidgetView for TerminalViewer {
    type Renderer = SpragGuiRenderer;

    fn initial_size_strategy() -> SizeStrategy {
        // Measured at boot (not the view) — consistent with use_terminal's
        // measurement (same font size + monospace resolution ⇒ same metric).
        let (width, height) = window_px(measure_cell(font_size_px()));
        SizeStrategy::Fixed { width, height }
    }
}

fn main() {
    pinion_shell::run::<TerminalViewer>();
}

#[cfg(test)]
mod tests {
    use super::*;
    use sprag_vt::{Emulator, VtPort};

    /// The projection is reused (not duplicated): a screen advanced with known
    /// bytes yields a `Scene::TextGrid` tagged with sprag-host's `GRID_TAG`,
    /// carrying cells, with a layout the shell can resolve to a rect.
    #[test]
    fn grid_scene_reuses_the_projection_with_layout_and_font_size() {
        let mut em = Emulator::new(COLS, ROWS);
        em.advance(b"hello");
        match grid_scene(em.screen(), CellMetric::DEFAULT, 16) {
            Scene::TextGrid(node) => {
                assert_eq!(node.tag.as_deref(), Some(sprag_host::GRID_TAG));
                assert!(!node.cells().is_empty(), "carries the projected cells");
                // A layout is attached so the GUI winsize derives from the rect
                // (the headless projection leaves the default, position-less
                // LayoutStyle).
                assert!(
                    node.layout.absolute_position.is_some(),
                    "GUI attaches an absolute layout (headless leaves it unset)",
                );
                // The measured font size rides on the node so paint renders at
                // exactly it (advance == cell_w by construction, R1002).
                assert_eq!(node.font_size_px(), Some(16));
            }
            other => panic!("expected a TextGrid, got {other:?}"),
        }
    }

    /// The composed root is a valid paint root: it carries `ROOT_TAG` and
    /// contains the projected grid. Exercised without spawning a PTY (the live
    /// read is in `view`; `compose` + `grid_scene` are the pure parts).
    #[test]
    fn view_root_carries_paint_root_tag_and_grid() {
        let owner = Owner::new();
        let scene = owner.run(|| {
            let theme = use_theme(THEME_TAG).theme_animated();
            let mut em = Emulator::new(COLS, ROWS);
            em.advance(b"hi");
            compose(
                grid_scene(em.screen(), CellMetric::DEFAULT, 16),
                &theme,
                CellMetric::DEFAULT,
            )
        });
        assert!(scene.contains_tag(ROOT_TAG), "root paint-root tag present");
        assert!(scene.contains_tag(sprag_host::GRID_TAG), "the grid is mounted");
    }

    /// The window is exactly the grid's pixel extent (cols/rows x the cell
    /// size) — shown with the 8x16 baseline so the test needs no font
    /// measurement; the live path uses the measured metric instead.
    #[test]
    fn window_matches_cell_geometry() {
        assert_eq!(
            window_px(CellMetric::DEFAULT),
            (u32::from(COLS) * 8, u32::from(ROWS) * 16),
        );
    }

    /// The readable default glyph size, absent a `SPRAG_GUI_FONT` override.
    #[test]
    fn default_font_is_the_readable_size() {
        assert_eq!(FONT_SIZE_PX, 20);
    }
}
