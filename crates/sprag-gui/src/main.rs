//! `sprag-gui` — the **interactive GPU windowed terminal** (R24-R29).
//!
//! A window that paints one terminal pane's **live** screen (and its scrollback
//! history) and types into it.
//! It is the human observation/interaction path; the north star (an AI
//! reading/driving the terminal as *data*) is the headless `sprag-host` RPC
//! path, which needs none of this. This binding is a faithful pixel projection
//! of the *same* cell data the AI reads — it reuses the single projection
//! through the host's [`sprag_host::pane_view_scene`] seam rather than
//! re-deriving it — and routes keystrokes through the *same*
//! [`SpragPaneExternal`] `invoke("key", ...)` wire the AI drives (§2 #2).
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
//! ## Input (R27): keystroke -> PTY through the one input wire
//!
//! The root model External is the boot pane's [`SpragPaneExternal`] (built in
//! [`WidgetCore::create_external`] over the pane's `SessionHandle`). The
//! terminal is the single focusable tag ([`ROOT_TAG`], focused at boot), so
//! [`WidgetCore::apply_key`] routes a focused keystroke + W3C modifiers to that
//! External's `invoke("key", {key, ctrl, alt, shift, super})` — the *same*
//! `scene/invoke` channel the RPC client uses, where the sprag-owned encoder
//! ([`sprag_input`](https://docs.rs/sprag-input)) turns the key into PTY bytes
//! (R2.6). Returning `true` swallows Escape/Tab from the shell's quit/traverse
//! defaults so a full-screen TUI (vim) receives them.
//!
//! ## Scrollback (R29): scroll the history view
//!
//! `Shift+PageUp` / `Shift+PageDown` scroll a view over the pane's history (a
//! [`Signal`]-backed offset in lines from the live bottom; `apply_key` writes
//! it, `view` reads it, so a scroll re-renders). The view reuses the host's one
//! projection seam at its scrolled entry ([`sprag_host::pane_view_scene_scrolled`]),
//! so history and the live screen share one authority. The scroll keys do NOT
//! reach the PTY; any other key snaps back to the live bottom (you type at the
//! prompt). Scrolled history is **text-only** (the R16 scrollback model keeps
//! text, not cells) — it renders in default colors; the live screen is exact.
//! `offset == 0` follows the bottom with no drift; scrolling *during* active
//! output may shift (the offset is relative to the live bottom) — a v1 limit.

use pinion_a11y::{AccessNode, AccessValue, AriaRole, WidgetA11y};
use pinion_core::external::{External, IntrospectValue};
use pinion_core::reactive::{Effect, Owner, Signal};
use pinion_core::scene::ContainerNode;
use pinion_core::style::{BoxStyle, LayoutStyle, Size, SizeValue};
use pinion_core::theme::{use_theme, ColorRole, Theme};
use pinion_core::use_repaint_sink;
use pinion_core::widget_core::ExtraExternal;
use pinion_core::{CellMetric, Frame, Modifiers, Scene, WidgetCore};
use pinion_shell::{vello_renderer_impl, SizeStrategy, WidgetView};
use sprag_host::SpragPaneExternal;
use sprag_terminal::{CommandBuilder, SessionHandle, Workspace};
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
/// Paint-root + input-engine ([`SpragPaneExternal`]) anchor tag, and the single
/// focus tab stop (`V::tag()` on the root container).
const ROOT_TAG: &str = "sprag_gui";
/// `Owner::cache` key for the live terminal (created once at boot).
const SESSION_KEY: &str = "sprag_gui.terminal";
/// `Owner::cache` key for the resize -> reflow [`Effect`] (kept alive across
/// frames by the cache — a dropped [`Effect`] handle stops firing).
const REFLOW_KEY: &str = "sprag_gui.reflow";
/// `Owner::cache` key for the scrollback view offset (lines scrolled up from
/// the live bottom; `0` = live).
const SCROLL_KEY: &str = "sprag_gui.scroll";

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

impl TerminalView {
    /// The boot pane's cloneable I/O handle. The pane is spawned at boot and
    /// never closed, so `first()` is always present — the input engine
    /// ([`SpragPaneExternal`]) and the reflow share this single pane.
    fn pane_handle(&self) -> SessionHandle {
        self.workspace
            .panes()
            .first()
            .expect("the boot pane is always present (spawned at boot, never closed)")
            .handle()
    }
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

/// The scrollback view offset (lines scrolled up from the live bottom), an
/// `Owner::cache`-backed [`Signal`] so `apply_key` writes it and `view` reads it
/// reactively (a `set` re-renders the view). `0` = live (follow the bottom).
fn use_scroll_offset() -> Signal<usize> {
    let owner = Owner::current().expect("use_scroll_offset() requires an active Owner scope");
    owner.cache(SCROLL_KEY, || Signal::new(0_usize)).as_ref().clone()
}

/// The scrollback offset after a `PageUp` / `PageDown` of `page` rows from
/// `current`, clamped to `[0, scrollback_len]`. Pure, so it is unit-testable;
/// `PageUp` walks into history (clamped at the depth), `PageDown` back toward
/// the live bottom (saturating at `0`).
fn next_scroll_offset(key: &str, current: usize, page: usize, scrollback_len: usize) -> usize {
    match key {
        "PageUp" => (current + page).min(scrollback_len),
        "PageDown" => current.saturating_sub(page),
        _ => current,
    }
}

/// Adjust the scrollback offset for a `Shift+PageUp` / `Shift+PageDown`, clamped
/// to the pane's retained scrollback depth. A page is the viewport height less
/// one row (one row of overlap for continuity). Reads the live pane for the
/// depth + row count; called from `apply_key` (outside any cache factory).
fn scroll_view(key: &str) {
    let terminal = use_terminal();
    let offset = use_scroll_offset();
    let Some(pane) = terminal.workspace.panes().first() else {
        return;
    };
    let (scrollback_len, rows) =
        pane.session().with_screen(|screen| (screen.scrollback_len(), screen.rows()));
    let page = usize::from(rows).saturating_sub(1).max(1);
    offset.set(next_scroll_offset(key, offset.get(), page, scrollback_len));
}

/// view-fn (§6.3): pure sync `() -> Scene`. Reads the producer-authoritative
/// screen of the (single) pane each frame and paints it (live, or a scrollback
/// window when [`use_scroll_offset`] is non-zero) via the host's projection
/// seam; the producer thread (the PTY reader) lives in `create_extra_externals`,
/// not here.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn view(_state: (), _frame: &Frame) -> Scene {
    let theme = use_theme(THEME_TAG).theme_animated();
    let tv = use_terminal();
    let offset = use_scroll_offset().get();
    // The boot pane is always present (spawned at boot, never closed). On child
    // EOF the pane stays and `view` paints its frozen final screen — the
    // deliberate read-only behavior (the program exited; its last output shows).
    let pane = tv
        .workspace
        .panes()
        .first()
        .expect("the boot pane is always present (spawned at boot, never closed)");
    // `offset == 0` is the live screen; a positive offset windows into history
    // (text-only, the R16 scrollback model) — one projection seam for both.
    let grid = pane.session().with_screen(|screen| {
        sprag_host::pane_view_scene_scrolled(screen, tv.metric, tv.font_size_px, offset)
    });
    compose(grid, &theme)
}

struct TerminalViewer;

impl WidgetCore for TerminalViewer {
    type State = ();
    type Event = ();

    /// The root model is the boot pane's input engine ([`SpragPaneExternal`],
    /// tagged [`ROOT_TAG`]) over the pane's live [`SessionHandle`]. The shell
    /// runs this inside the root Owner scope, so [`use_terminal`] resolves the
    /// (already-booted) pane here. [`Self::apply_key`] routes keystrokes to this
    /// External's `invoke("key", ...)` — the **same** wire the headless RPC path
    /// drives (one input substrate, §2 #2; key->PTY-byte encoding is sprag's,
    /// R2.6).
    fn create_external() -> Box<dyn External> {
        let terminal = use_terminal();
        Box::new(SpragPaneExternal::new(terminal.pane_handle()))
    }

    /// Boot the live terminal (spawn the PTY + wire `on_dirty` -> repaint) AND
    /// install the resize -> reflow [`Effect`], before the first paint and off
    /// the pure `view`. [`install_reflow`] resolves [`use_terminal`], so it both
    /// boots the pane and wires the reflow in one call. Then focus the terminal
    /// (the single tab stop) so keystrokes reach the pane without a click — the
    /// shell drains this focus request before the first paint.
    fn create_extra_externals() -> Vec<ExtraExternal> {
        let _reflow = install_reflow();
        pinion_core::focus_request::request(ROOT_TAG);
        Vec::new()
    }

    /// Route a focused keystroke to the boot pane's PTY. The roving-tabindex
    /// gate (`focused == Some(ROOT_TAG)`) keeps keys scoped to the terminal, and
    /// the key + W3C modifiers go to the root [`SpragPaneExternal`]'s
    /// `invoke("key", {key, ctrl, alt, shift, super})` — the same `scene/invoke`
    /// wire the RPC client uses (§2 #2), where the sprag-owned encoder turns the
    /// key into PTY bytes (R2.6). An unencodable key (a bare modifier press, an
    /// `Fn`-style key the encoder does not map) returns `Err` -> `false`, so it
    /// falls through to the shell default rather than injecting nothing silently.
    /// The terminal keys that matter all encode: returning `true` for them
    /// swallows the key from the shell's Escape-quits / Tab-traverses defaults,
    /// so Escape and Tab reach a full-screen TUI (vim) instead of the window.
    fn apply_key(scene: &mut Scene, focused: Option<&str>, key: &str, modifiers: Modifiers) -> bool {
        if focused != Some(ROOT_TAG) {
            return false;
        }
        // Scrollback: Shift+PageUp / Shift+PageDown scroll the history view and do
        // NOT reach the PTY (a terminal app sees an unmodified PageUp). Every
        // other key is a live interaction, so it first snaps the view back to the
        // bottom — you type at the prompt, which is at the live bottom.
        if modifiers.shift && matches!(key, "PageUp" | "PageDown") {
            scroll_view(key);
            return true;
        }
        use_scroll_offset().set(0);
        let Scene::External(node) = scene else {
            return false;
        };
        let Some(intro) = node.handle.introspect_mut() else {
            return false;
        };
        let args = serde_json::json!({
            "key": key,
            "ctrl": modifiers.ctrl,
            "alt": modifiers.alt,
            "shift": modifiers.shift,
            // pinion's `meta` (Cmd/Super/Win) maps to the encoder's "super".
            "super": modifiers.meta,
        });
        intro.invoke("key", IntrospectValue::Json(args)).is_ok()
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

    /// The terminal is the single tab stop — focusing [`ROOT_TAG`] gates
    /// [`Self::apply_key`] (and lets a click re-focus the pane).
    /// [`Self::create_extra_externals`] requests this focus at boot so typing
    /// works without a click.
    fn focusable_tags() -> Vec<&'static str> {
        vec![ROOT_TAG]
    }

    fn title() -> &'static str {
        "sprag terminal viewer (R26 windowed host, R27 keyboard input)"
    }
}

impl WidgetA11y for TerminalViewer {
    /// Expose the live terminal as one accessible text region so a screen
    /// reader / AccessKit tree can read it (the AI path is `scene/snapshot`;
    /// this is the human-AT path). It runs in the root Owner scope, so
    /// [`use_terminal`] resolves the live pane here. The node tag is
    /// [`ROOT_TAG`], so AT focus and the keyboard focus gate ([`Self::apply_key`])
    /// share one identity.
    fn access_node(_state: &(), focused: Option<&str>) -> Vec<AccessNode> {
        let terminal = use_terminal();
        let Some(pane) = terminal.workspace.panes().first() else {
            return Vec::new();
        };
        // `full_text` is the pane's text SSOT — the same string the RPC
        // `full_text` query and the plugin capture read, so the AT and the AI
        // see one notion of the screen (scrollback + visible).
        let text = pane.session().with_screen(|screen| screen.full_text());
        vec![terminal_a11y_node(
            pane.command_label(),
            text,
            focused == Some(ROOT_TAG),
        )]
    }
}

/// Build the terminal's accessible node: a neutral focusable region
/// ([`AriaRole::Group`]) named with the pane's command, carrying the screen text
/// as its value, and the focus state from the focus manager. Pure (no Owner / no
/// live pane), so it is unit-testable; the bounds are left `None` for the shell
/// to resolve from [`ROOT_TAG`] after layout.
///
/// `Group` (not `TextInput`): pinion has no `terminal` / `log` role, and a
/// textbox role advertises caret + place-click + set-value edit affordances that
/// this widget does not implement (input is raw keystrokes funneled to the PTY
/// by `apply_key`, not textbox editing). A neutral region carrying a
/// read value is the honest shape — it does not promise an edit contract the
/// terminal cannot honor. The cell data the AI reads stays the `scene/snapshot`
/// path; this node is the human-AT label + text.
fn terminal_a11y_node(command_label: &str, text: String, focused: bool) -> AccessNode {
    AccessNode::new(ROOT_TAG, AriaRole::Group)
        .with_name(format!("Terminal: {command_label}"))
        .with_value(AccessValue::Text(text))
        .with_focused(focused)
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

    /// End-to-end keyboard input: build the model scene the shell assembles
    /// (`Scene::External(SpragPaneExternal, ROOT_TAG)`) over a live `cat` pane
    /// and drive `apply_key`. The focus gate is deterministic; the typed text
    /// echoes back through the cooked-mode PTY (bounded poll, the sprag-terminal
    /// test idiom).
    #[test]
    fn apply_key_routes_focused_keystrokes_to_the_pane() {
        use pinion_core::scene::ExternalNode;
        use std::thread::sleep;
        use std::time::{Duration, Instant};

        let mut ws = Workspace::new((40, 6));
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg("cat"); // echoes stdin; keeps the PTY open across the keys
        command.env("TERM", "dumb");
        let id = ws.spawn(command, "cat".to_owned(), 40, 6).unwrap();
        let handle = ws.pane(id).unwrap().handle();
        let mut scene = Scene::External(
            ExternalNode::new(Box::new(SpragPaneExternal::new(handle.clone()))).with_tag(ROOT_TAG),
        );

        // apply_key runs inside the shell's root Owner scope (it reads the
        // scrollback-offset Signal); mirror that here.
        let owner = Owner::new();
        owner.run(|| {
            // Focus gate: an unfocused / wrong-tag keystroke is a no-op.
            assert!(!TerminalViewer::apply_key(&mut scene, None, "a", Modifiers::default()));
            assert!(!TerminalViewer::apply_key(&mut scene, Some("other"), "a", Modifiers::default()));
            // Focused: each key is injected and the cooked-mode PTY echoes it back.
            for ch in ["h", "i"] {
                assert!(TerminalViewer::apply_key(&mut scene, Some(ROOT_TAG), ch, Modifiers::default()));
            }
        });
        let start = Instant::now();
        let mut row0 = String::new();
        while start.elapsed() < Duration::from_secs(5) {
            row0 = handle.with_screen(|screen| screen.row_text(0));
            if row0.contains("hi") {
                break;
            }
            sleep(Duration::from_millis(20));
        }
        assert!(row0.contains("hi"), "typed keys echo to the pane screen; row0 = {row0:?}");
    }

    #[test]
    fn terminal_a11y_node_exposes_role_name_value_and_focus() {
        let node = terminal_a11y_node("bash", "line one\nline two".to_owned(), true);
        assert_eq!(node.tag, ROOT_TAG);
        assert!(matches!(node.role, AriaRole::Group));
        assert_eq!(node.name.as_deref(), Some("Terminal: bash"));
        match &node.value {
            Some(AccessValue::Text(text)) => assert_eq!(text, "line one\nline two"),
            other => panic!("expected a Text value, got {other:?}"),
        }
        assert!(node.state.focused, "focused state flows through");
        // Unfocused: the AT focus follows the focus manager.
        assert!(!terminal_a11y_node("sh", String::new(), false).state.focused);
    }

    /// `access_node` reads the live pane through `use_terminal` (spawns a real
    /// shell PTY) and returns one focus-gated terminal node — the use_terminal
    /// -> screen -> node plumbing, headlessly.
    #[test]
    fn access_node_reads_the_live_pane() {
        let owner = Owner::new();
        let nodes = owner.run(|| TerminalViewer::access_node(&(), Some(ROOT_TAG)));
        assert_eq!(nodes.len(), 1, "one terminal node");
        let node = &nodes[0];
        assert_eq!(node.tag, ROOT_TAG);
        assert!(matches!(node.role, AriaRole::Group));
        assert!(node.state.focused, "ROOT_TAG focus -> focused node");
        assert!(matches!(node.value, Some(AccessValue::Text(_))), "value is the screen text");
        // Same cached pane, unfocused dispatch -> the focus flag clears.
        let unfocused = owner.run(|| TerminalViewer::access_node(&(), None));
        assert!(!unfocused[0].state.focused);
    }

    #[test]
    fn next_scroll_offset_clamps_and_saturates() {
        // PageUp accumulates up to the scrollback depth (clamped).
        assert_eq!(next_scroll_offset("PageUp", 0, 10, 25), 10);
        assert_eq!(next_scroll_offset("PageUp", 20, 10, 25), 25); // clamp at depth
        // PageDown walks back toward the live bottom (saturating at 0).
        assert_eq!(next_scroll_offset("PageDown", 25, 10, 25), 15);
        assert_eq!(next_scroll_offset("PageDown", 5, 10, 25), 0); // saturate
        // No scrollback -> stays live regardless of the key.
        assert_eq!(next_scroll_offset("PageUp", 0, 10, 0), 0);
    }

    #[test]
    fn scroll_offset_signal_defaults_live_and_round_trips() {
        let owner = Owner::new();
        assert_eq!(owner.run(|| use_scroll_offset().get()), 0, "boots at the live bottom");
        owner.run(|| use_scroll_offset().set(7));
        assert_eq!(owner.run(|| use_scroll_offset().get()), 7);
    }

    /// `apply_key` treats Shift+PageUp as a scroll (handled, not sent to the PTY)
    /// and snaps the view back to the live bottom on any other (typed) key.
    #[test]
    fn apply_key_scrolls_and_snaps_to_bottom() {
        use pinion_core::scene::ExternalNode;
        let owner = Owner::new();
        owner.run(|| {
            // The model scene over the live boot pane (same pane use_terminal
            // caches), so the PTY routing reaches the pane scroll_view reads.
            let handle = use_terminal().pane_handle();
            let mut scene = Scene::External(
                ExternalNode::new(Box::new(SpragPaneExternal::new(handle))).with_tag(ROOT_TAG),
            );
            // Shift+PageUp is consumed as a scroll (true = handled, not the PTY).
            let shift = Modifiers { shift: true, ..Modifiers::default() };
            assert!(TerminalViewer::apply_key(&mut scene, Some(ROOT_TAG), "PageUp", shift));
            // A scrolled-up view snaps to the live bottom when the user types.
            use_scroll_offset().set(5);
            assert!(TerminalViewer::apply_key(&mut scene, Some(ROOT_TAG), "a", Modifiers::default()));
            assert_eq!(use_scroll_offset().get(), 0, "typing snaps to the live bottom");
        });
    }
}
