//! `sprag-gui` — the read-only **GPU windowed terminal viewer** (R24, R25).
//!
//! A window that paints one terminal pane's **live** screen. It is the human
//! observation path; the north star (an AI reading/driving the terminal as
//! *data*) is the headless `sprag-host` RPC path, which needs none of this.
//! This binding is a faithful pixel projection of the *same* cell data the AI
//! reads — it reuses the single projection through the host's
//! [`sprag_host::pane_view_scene`] seam rather than re-deriving it.
//!
//! ## How it stays on the substrate (no hacks)
//!
//! A child process writes its PTY from a **separate OS thread**, so the static
//! `view` must read changing, cross-thread (`Send`) data and the window must
//! repaint on change without owning the event loop. The seams (all pinion):
//!
//! - [`use_terminal`] self-creates the [`Workspace`] + initial pane once in an
//!   `Owner::cache` hook (the `use_storage` pattern — nothing flows through
//!   `main`), spawned in [`WidgetCore::create_extra_externals`] at boot.
//! - The pane is spawned via [`Workspace::spawn_with_dirty`] (the sprag R23
//!   hook) with an `on_dirty` that calls
//!   [`RepaintSink::request_repaint`](pinion_core::RepaintSink::request_repaint)
//!   (pinion R999) — each PTY batch wakes the shell; the next frame's `view`
//!   re-reads the screen. Event-driven, no polling. `State` stays `()` (`Copy`).
//! - The cell is **measured once** at boot from the resolved monospace via
//!   pinion R1003 [`measured_monospace_cell`](pinion_core::measured_monospace_cell)
//!   (the shell seeds the provider before factories) — no private `FontContext`,
//!   no double measurement. `pane_view_scene` pins that size on the node so the
//!   painted advance equals `cell_w` (R1002), and fills the viewport so the
//!   cell `(cols, rows)` derive from the resolved rect (the §3 GUI winsize SSOT).
//!
//! ## Winsize (§3): the window drives the size
//!
//! The window opens at [`WINDOW_W`] x [`WINDOW_H`]; the grid fills it, so its
//! rect IS the viewport and `(cols, rows)` derive from `rect / cell`. The PTY
//! is spawned at those derived dims. Live resize -> PTY reflow (re-deriving
//! cols/rows from the resized rect and calling `Workspace::resize`) is the
//! deferred additive round; the §3 derivation is already live here.
//!
//! Input (keyboard -> PTY) is a later additive round reusing the existing
//! `sprag-input` encoder + `SpragPaneExternal`; this viewer is display-only
//! (a [`StubExternal`], no focusable tags).

use pinion_a11y::{AccessNode, WidgetA11y};
use pinion_core::external::{External, StubExternal};
use pinion_core::reactive::Owner;
use pinion_core::scene::ContainerNode;
use pinion_core::style::{BoxStyle, LayoutStyle, Size, SizeValue};
use pinion_core::theme::{use_theme, ColorRole, Theme};
use pinion_core::use_repaint_sink;
use pinion_core::widget_core::ExtraExternal;
use pinion_core::{CellMetric, Frame, Scene, WidgetCore};
use pinion_shell::{vello_renderer_impl, SizeStrategy, WidgetView};
use sprag_terminal::{CommandBuilder, Workspace};
use std::rc::Rc;

include!(concat!(env!("OUT_DIR"), "/app.rs"));
vello_renderer_impl!(SpragGuiRenderer, SpragGuiRendererError);

/// The window's initial logical-pixel size. The grid fills it and the terminal
/// `(cols, rows)` derive from `WINDOW / measured-cell` (§3 — the window drives
/// the winsize, the inverse of the headless host). The window stays resizable;
/// resize -> PTY reflow is the deferred additive round.
const WINDOW_W: u32 = 960;
const WINDOW_H: u32 = 600;
/// Default glyph size (logical px) — the font-size SSOT the cell is measured
/// from. `SPRAG_GUI_FONT=<px>` overrides it live (larger ⇒ bigger).
const FONT_SIZE_PX: u32 = 20;

/// Shared [`ThemeProvider`] cache key (the surface fill behind the grid).
const THEME_TAG: &str = "app";
/// Paint-root + [`StubExternal`] anchor tag (`V::tag()` on the root container).
const ROOT_TAG: &str = "sprag_gui";
/// `Owner::cache` key for the live terminal (created once at boot).
const SESSION_KEY: &str = "sprag_gui.terminal";

/// Parse a `SPRAG_GUI_FONT` spec into a glyph px size. Absent / malformed /
/// zero falls back to `default`. Pure (no env) so it is unit-testable.
fn parse_font_size(spec: Option<&str>, default: u32) -> u32 {
    spec.and_then(|s| s.trim().parse::<u32>().ok())
        .filter(|&px| px > 0)
        .unwrap_or(default)
}

/// The glyph size: `SPRAG_GUI_FONT=<px>` overrides [`FONT_SIZE_PX`] live.
fn font_size_px() -> u32 {
    parse_font_size(
        std::env::var("SPRAG_GUI_FONT").ok().as_deref(),
        FONT_SIZE_PX,
    )
}

/// Split a `SPRAG_GUI_CMD` spec into `(program, args)` on whitespace (no shell
/// quoting). Empty / whitespace-only yields `None`. Pure so it is testable.
fn split_command(spec: &str) -> Option<(String, Vec<String>)> {
    let mut parts = spec.split_whitespace().map(str::to_owned);
    let program = parts.next()?;
    Some((program, parts.collect()))
}

/// The program the initial pane runs, plus its introspection label.
///
/// `SPRAG_GUI_CMD` (whitespace-split: program + args; no shell quoting) — parity
/// with the headless `sprag-term -- <program> [args]`; otherwise the user's
/// `$SHELL` (then `/bin/sh`). Read from the environment, not threaded through
/// `main`, matching pinion's `Owner::cache` config-from-env pattern.
fn pane_command() -> (CommandBuilder, String) {
    let spec = std::env::var("SPRAG_GUI_CMD").unwrap_or_default();
    let (program, args) = split_command(&spec).unwrap_or_else(|| {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned());
        (shell, Vec::new())
    });
    let label = program.clone();
    let mut command = CommandBuilder::new(&program);
    for arg in &args {
        command.arg(arg);
    }
    command.env("TERM", "xterm-256color");
    (command, label)
}

/// The booted terminal: the live [`Workspace`] plus the once-measured cell
/// metric and the glyph size it was measured at (read each frame by `view`,
/// never re-measured).
struct TerminalView {
    workspace: Workspace,
    metric: CellMetric,
    font_size_px: u32,
}

/// Self-create (once) the live terminal: measure the resolved monospace cell
/// via the R1003 seam, derive `(cols, rows)` from the window + cell (§3), and
/// spawn the initial pane wired to the shell's [`RepaintSink`] (the R23
/// `on_dirty` -> R999 seam). Spawns the PTY on first call — therefore invoked
/// from `create_extra_externals` (boot), never the pure `view`.
///
/// [`use_repaint_sink`] is resolved *before* the `Owner::cache` factory so the
/// factory never re-enters `Owner::cache` (the nested-factory guard).
fn use_terminal() -> Rc<TerminalView> {
    let owner = Owner::current().expect("use_terminal() requires an active Owner scope");
    // Pre-resolve the cache-backed deps BEFORE the factory (the nested-factory
    // guard): use_repaint_sink AND measured_monospace_cell both read
    // `Owner::cache` (the repaint-sink / monospace-metrics provider slots), so
    // resolving them inside the factory would re-enter and panic.
    let sink = use_repaint_sink();
    let font_size_px = font_size_px();
    // R1003 view-time seam: the shell seeded the monospace-metrics provider
    // before the factories run, so this is the font the shell will paint.
    let metric = pinion_core::measured_monospace_cell(font_size_px).unwrap_or(CellMetric::DEFAULT);
    owner.cache(SESSION_KEY, move || {
        // §3: the window viewport drives the winsize; derive (cols, rows) and
        // let the producer adopt them (cell axes are non-zero by construction).
        let cols = u16::try_from((WINDOW_W / metric.cell_w()).max(1)).unwrap_or(u16::MAX);
        let rows = u16::try_from((WINDOW_H / metric.cell_h()).max(1)).unwrap_or(u16::MAX);
        let (command, label) = pane_command();
        let mut workspace = Workspace::new((cols, rows));
        workspace
            .spawn_with_dirty(
                command,
                label,
                cols,
                rows,
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

/// Wrap the pane grid in the surface-filled paint root (tagged [`ROOT_TAG`])
/// that fills the window, so the single pane grid fills it and its rect = the
/// viewport (§3). Pure composition; the unit test exercises it without a PTY.
fn compose(grid: Scene, theme: &Theme) -> Scene {
    Scene::Container(
        ContainerNode::new(vec![grid])
            .with_tag(ROOT_TAG)
            .with_style(BoxStyle::filled(theme.resolve(ColorRole::Surface)))
            .with_layout(LayoutStyle::new().with_size(fill())),
    )
}

/// A both-axes `Percent(100)` size — fill the parent slot.
fn fill() -> Size {
    Size::auto()
        .with_width(SizeValue::Percent(100))
        .with_height(SizeValue::Percent(100))
}

/// view-fn (§6.3): pure sync `() -> Scene`. Reads the producer-authoritative
/// screen of the (single) pane each frame and paints it via the host's
/// projection seam; the producer thread (the PTY reader) lives in
/// `create_extra_externals`, not here.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn view(_state: (), _frame: &Frame) -> Scene {
    let theme = use_theme(THEME_TAG).theme_animated();
    let tv = use_terminal();
    // The boot pane is always present (spawned at boot, never closed). On child
    // EOF the pane stays and `view` paints its frozen final screen — the
    // deliberate read-only behavior (the program exited; its last output shows).
    let pane = tv
        .workspace
        .panes()
        .first()
        .expect("the boot pane is always present (spawned at boot, never closed)");
    let grid = pane
        .session()
        .with_screen(|screen| sprag_host::pane_view_scene(screen, tv.metric, tv.font_size_px));
    compose(grid, &theme)
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

    /// Boot the live terminal (spawn the PTY + wire `on_dirty` -> repaint)
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
        SizeStrategy::Fixed {
            width: WINDOW_W,
            height: WINDOW_H,
        }
    }
}

fn main() {
    pinion_shell::run::<TerminalViewer>();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_font_size_clamps_and_falls_back() {
        assert_eq!(parse_font_size(None, FONT_SIZE_PX), FONT_SIZE_PX);
        assert_eq!(parse_font_size(Some("28"), FONT_SIZE_PX), 28);
        assert_eq!(parse_font_size(Some("  16 "), FONT_SIZE_PX), 16); // trims
        assert_eq!(parse_font_size(Some("0"), FONT_SIZE_PX), FONT_SIZE_PX); // zero rejected
        assert_eq!(parse_font_size(Some("huge"), FONT_SIZE_PX), FONT_SIZE_PX); // malformed
        assert_eq!(parse_font_size(Some(""), FONT_SIZE_PX), FONT_SIZE_PX);
    }

    #[test]
    fn split_command_parses_program_and_args() {
        assert_eq!(split_command(""), None);
        assert_eq!(split_command("   "), None);
        assert_eq!(split_command("vim"), Some(("vim".to_owned(), Vec::new())));
        assert_eq!(
            split_command("ls -la /usr/bin"),
            Some(("ls".to_owned(), vec!["-la".to_owned(), "/usr/bin".to_owned()])),
        );
    }

    #[test]
    fn compose_wraps_the_grid_in_a_filling_paint_root() {
        let owner = Owner::new();
        let scene = owner.run(|| {
            let theme = use_theme(THEME_TAG).theme_animated();
            // A stand-in grid (the real one is the host's pane_view_scene,
            // tested in sprag-host) — compose only owns the root wrapping.
            let grid = Scene::Container(ContainerNode::new(Vec::new()).with_tag("grid_stub"));
            compose(grid, &theme)
        });
        match scene {
            Scene::Container(ref root) => {
                assert_eq!(root.tag.as_deref(), Some(ROOT_TAG));
                assert_eq!(root.layout.size.width, SizeValue::Percent(100));
                assert_eq!(root.layout.size.height, SizeValue::Percent(100));
            }
            other => unreachable!("compose returns a Container, got {other:?}"),
        }
        assert!(scene.contains_tag("grid_stub"), "the grid is mounted");
    }
}
