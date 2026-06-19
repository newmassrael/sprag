//! `sprag-gui` — the read-only **GPU windowed terminal viewer** (R24, R25, R26).
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
//! ## Winsize (§3): the window drives the size, resize reflows the PTY (R26)
//!
//! The window opens at [`WINDOW_W`] x [`WINDOW_H`]; the grid fills it, so its
//! rect IS the viewport and `(cols, rows)` derive from `rect / cell`
//! ([`grid_dims`], the single derivation SSOT). The PTY is spawned at those
//! boot dims, and a resize Effect ([`install_reflow`]) keeps them live: it
//! subscribes to pinion's R1006 viewport-size
//! [`Signal`](pinion_core::reactive::Signal) and, on every OS window resize,
//! re-derives `(cols, rows)` from the new viewport and reflows the pane
//! (`TIOCSWINSZ`). The reflow is a real side-effect (an ioctl on a live fd), so
//! it lives in an [`Effect`] — gated out of `dry_run` / snapshot paint by the
//! R1006 seam — never the pure `view`.
//!
//! Input (keyboard -> PTY) is a later additive round reusing the existing
//! `sprag-input` encoder + `SpragPaneExternal`; this viewer is display-only
//! (a [`StubExternal`], no focusable tags).

use pinion_a11y::{AccessNode, WidgetA11y};
use pinion_core::external::{External, StubExternal};
use pinion_core::reactive::{Effect, Owner};
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
/// `(cols, rows)` derive from `WINDOW / measured-cell` ([`grid_dims`]; §3 — the
/// window drives the winsize, the inverse of the headless host). The window
/// stays resizable; live resize -> PTY reflow keeps the dims current via the
/// [`install_reflow`] Effect.
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
/// `Owner::cache` key for the resize -> reflow [`Effect`] (kept alive across
/// frames by the cache — a dropped [`Effect`] handle stops firing).
const REFLOW_KEY: &str = "sprag_gui.reflow";

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

/// Derive the terminal `(cols, rows)` that fill `viewport` (logical px) at
/// `metric`'s cell size, floored at `1x1`. Reuses pinion's winsize SSOT —
/// [`CellMetric::cols_for`] / [`CellMetric::rows_for`], the R1.4 "whole cells
/// spanning this width, floor the partial, saturate at `u16::MAX`" authority a
/// terminal host reports via `TIOCSWINSZ` — and adds only the PTY's own `>= 1`
/// floor (a PTY cannot be `0`-sized): a zero-area viewport (the `(0, 0)`
/// "unknown" boot value, or a window narrower than one cell) still yields a
/// valid `1x1` PTY. For any viewport at least one cell wide the count equals the
/// painted grid's own `cols_for`/`rows_for`, so the PTY size and what is drawn
/// agree (§3, PR-6 R6.3). The single derivation site for both the boot spawn and
/// the resize Effect, so the two can never compute `(cols, rows)` differently.
fn grid_dims(viewport: (u32, u32), metric: CellMetric) -> (u16, u16) {
    let (width, height) = viewport;
    (metric.cols_for(width).max(1), metric.rows_for(height).max(1))
}

/// Holds the resize -> PTY reflow [`Effect`] so the `Owner::cache` keeps it
/// alive across frames (a dropped [`Effect`] handle drops its subscription and
/// stops firing). It carries no data — only the live subscription.
struct ReflowMarker {
    _effect: Effect,
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
        // §3: the window viewport drives the winsize; derive the boot (cols,
        // rows) through the same SSOT the resize Effect uses (grid_dims), so the
        // spawn size and a later reflow can never diverge.
        let (cols, rows) = grid_dims((WINDOW_W, WINDOW_H), metric);
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

/// Install (once) the resize -> PTY reflow [`Effect`]: it subscribes to the
/// pinion R1006 viewport-size [`Signal`](pinion_core::reactive::Signal) and, on
/// every OS window resize, re-derives `(cols, rows)` from the new viewport and
/// the measured cell ([`grid_dims`]) and reflows the boot pane (`TIOCSWINSZ`
/// via [`Workspace::resize`]). It skips the `(0, 0)` "viewport unknown" boot
/// value (so it never reflows to `1x1` before the window is sized) and any
/// no-change frame (so a same-`(cols, rows)` resize issues no ioctl).
///
/// The viewport `Signal` and the terminal are BOTH `Owner::cache`-backed, so
/// they are resolved *before* this cache factory (the nested-factory guard):
/// the Effect closure captures the resolved handles and only calls
/// [`Signal::get`](pinion_core::reactive::Signal::get) (tracked) in its run —
/// never a `use_*` hook, which would re-enter `Owner::cache` and panic. Reading
/// the captured `Signal` directly (not `use_viewport_size`) also means the
/// Effect never resolves `Owner::current`, so the synchronous `set`-driven
/// re-run is robust regardless of scope (the shell still `set`s inside its
/// `root_owner`, per the R1006 contract). Registered from
/// [`WidgetCore::create_extra_externals`] so it is live before the first paint
/// and off the pure `view`.
fn install_reflow() -> Rc<ReflowMarker> {
    let owner = Owner::current().expect("install_reflow() requires an active Owner scope");
    // Resolve both cache-backed deps BEFORE the factory (nested-factory guard).
    let terminal = use_terminal();
    let viewport = owner.viewport_size_signal();
    let owner_for_effect = owner.clone();
    owner.cache(REFLOW_KEY, move || {
        let terminal = Rc::clone(&terminal);
        let viewport = viewport.clone();
        let effect = Effect::new(&owner_for_effect, move || {
            let size = viewport.get(); // tracked read: re-fires on every resize
            if size == (0, 0) {
                return; // "viewport unknown" (boot, before resume) — no reflow
            }
            let target = grid_dims(size, terminal.metric);
            // The boot pane is the only pane; reflow it only when the derived
            // size actually changed, so an unchanged frame issues no ioctl.
            let pane_to_reflow = terminal
                .workspace
                .panes()
                .first()
                .filter(|pane| pane.session().dimensions() != target)
                .map(|pane| pane.id());
            if let Some(id) = pane_to_reflow {
                let _ = terminal.workspace.resize(id, target.0, target.1);
            }
        });
        ReflowMarker { _effect: effect }
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

    /// Boot the live terminal (spawn the PTY + wire `on_dirty` -> repaint) AND
    /// install the resize -> reflow [`Effect`], before the first paint and off
    /// the pure `view`. [`install_reflow`] resolves [`use_terminal`], so it both
    /// boots the pane and wires the reflow in one call.
    fn create_extra_externals() -> Vec<ExtraExternal> {
        let _reflow = install_reflow();
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

    #[test]
    fn grid_dims_floors_at_one_by_one() {
        let metric = CellMetric::DEFAULT;
        let (cw, ch) = (metric.cell_w(), metric.cell_h());
        // Exact multiples divide cleanly.
        assert_eq!(grid_dims((cw * 10, ch * 4), metric), (10, 4));
        // A zero-area viewport (the (0,0) "unknown" value, or a minimized
        // window) still yields a valid 1x1 PTY rather than a 0-dimension one.
        assert_eq!(grid_dims((0, 0), metric), (1, 1));
        // A sub-cell viewport floors to 1x1 too.
        assert_eq!(grid_dims((cw - 1, ch - 1), metric), (1, 1));
    }

    /// End-to-end reflow: install the real reflow [`Effect`] over a real PTY,
    /// drive pinion's viewport [`Signal`](pinion_core::reactive::Signal), and
    /// assert the pane reflows. Exercises the whole R1006 consumer chain
    /// headlessly — the (0,0) boot skip, the resize on change, the emulator-SSOT
    /// dimensions, and the equality-skip no-op — without a window.
    #[test]
    fn reflow_resizes_the_boot_pane_when_the_viewport_changes() {
        use pinion_core::reactive::Signal;
        // No shell seeded: use_repaint_sink -> NullRepaintSink and
        // measured_monospace_cell -> None -> CellMetric::DEFAULT (what
        // use_terminal measures here). Seed a real viewport Signal so the reflow
        // Effect install_reflow registers subscribes to it.
        let owner = Owner::new();
        let metric = CellMetric::DEFAULT;
        let viewport = Signal::new((0_u32, 0_u32));
        owner.provide_viewport_size_signal(viewport.clone());
        // Boot the pane + install the reflow Effect. The eager run sees (0,0)
        // and skips, so the boot dims (window / cell) stand. Spawns a real
        // long-lived shell PTY; dropping `owner` at end of test reaps it.
        let _reflow = owner.run(install_reflow);
        let terminal = owner.run(use_terminal);
        let dims = || {
            terminal
                .workspace
                .panes()
                .first()
                .expect("boot pane present")
                .session()
                .dimensions()
        };
        let boot = dims();
        assert_eq!(
            boot,
            grid_dims((WINDOW_W, WINDOW_H), metric),
            "boot dims derive from the window (the (0,0) eager run skipped)",
        );
        // Publish a new viewport inside the owner scope (mirrors the shell's
        // R1006 set inside root_owner): the synchronous Effect re-run reflows.
        owner.run(|| viewport.set((400, 200)));
        let after = dims();
        assert_eq!(after, grid_dims((400, 200), metric), "pane reflowed to the new viewport");
        assert_ne!(after, boot, "the reflow actually changed the dims");
        // A repaint at the same viewport is inert (Signal equality-skip).
        owner.run(|| viewport.set((400, 200)));
        assert_eq!(dims(), after, "a same-size frame is a no-op");
    }
}
