//! `sprag-gui` — the **interactive GPU windowed terminal** (R24-R32).
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
//! ## Module map (R32)
//!
//! The binding is split by concern so the multi-pane round grows each axis in
//! one place rather than in one 800-line file:
//!
//! - [`terminal`] — the booted [`TerminalView`](terminal::TerminalView) model,
//!   its font/command config, and the [`grid_dims`](terminal::grid_dims)
//!   winsize SSOT; [`use_terminal`] self-creates it.
//! - [`reflow`] — the resize -> PTY reflow [`Effect`](pinion_core::reactive::Effect)
//!   ([`install_reflow`]).
//! - [`input`] — focused keystroke / IME commit -> PTY routing
//!   ([`route_key`] / [`route_composition`])
//!   and the scrollback-view offset.
//! - [`a11y`] — the accessible-node projection (the human-AT path).
//! - [`view`] — the pure view-fn ([`view::view`]) + the surface-filled paint root.
//!
//! ## How it stays on the substrate (no hacks)
//!
//! A child process writes its PTY from a **separate OS thread**, so the static
//! `view` must read changing, cross-thread (`Send`) data and the window must
//! repaint on change without owning the event loop. The seams (all pinion):
//!
//! - [`use_terminal`] self-creates the
//!   [`Workspace`](sprag_terminal::Workspace) + initial pane once in an
//!   `Owner::cache` hook (the `use_storage` pattern — nothing flows through
//!   `main`), spawned in [`WidgetCore::create_extra_externals`] at boot.
//! - The pane is spawned via [`Workspace::spawn_with_dirty`](sprag_terminal::Workspace::spawn_with_dirty)
//!   (the sprag R23 hook) with an `on_dirty` that calls
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
//! ([`grid_dims`](terminal::grid_dims), the single derivation SSOT). The PTY is
//! spawned at those boot dims, and a resize Effect ([`install_reflow`])
//! keeps them live: it subscribes to pinion's R1006 viewport-size
//! [`Signal`](pinion_core::reactive::Signal) and, on every OS window resize,
//! re-derives `(cols, rows)` from the new viewport and reflows the pane
//! (`TIOCSWINSZ`). The reflow is a real side-effect (an ioctl on a live fd), so
//! it lives in an [`Effect`](pinion_core::reactive::Effect) — gated out of
//! `dry_run` / snapshot paint by the R1006 seam — never the pure `view`.
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
//! IME-composed input (R31) — Hangul, CJK — arrives not as keystrokes but as
//! [`WidgetCore::apply_composition`] events; the committed text is written
//! *literally* (no key-encoding) through the sibling `invoke("text", …)` wire.
//! The in-progress preedit is rendered by the platform IME itself (an inline
//! grid overlay is a later round).
//!
//! ## Scrollback (R29): scroll the history view
//!
//! `Shift+PageUp` / `Shift+PageDown` scroll a view over the pane's history (a
//! [`Signal`](pinion_core::reactive::Signal)-backed offset in lines from the
//! live bottom; [`route_key`] writes it, `view` reads it, so a
//! scroll re-renders). The view reuses the host's one projection seam at its
//! scrolled entry ([`sprag_host::pane_view_scene_scrolled`]), so history and the
//! live screen share one authority. The scroll keys do NOT reach the PTY; any
//! other key snaps back to the live bottom (you type at the prompt). Scrolled
//! history is **text-only** (the R16 scrollback model keeps text, not cells) —
//! it renders in default colors; the live screen is exact. `offset == 0` follows
//! the bottom with no drift; scrolling *during* active output may shift (the
//! offset is relative to the live bottom) — a v1 limit.

mod a11y;
mod input;
mod reflow;
mod terminal;
mod view;

use pinion_core::external::External;
use pinion_core::widget_core::ExtraExternal;
use pinion_core::{CompositionEvent, Frame, Modifiers, Scene, WidgetCore};
use pinion_shell::{vello_renderer_impl, SizeStrategy, WidgetView};
use sprag_host::SpragPaneExternal;

use crate::input::{route_composition, route_key};
use crate::reflow::install_reflow;
use crate::terminal::use_terminal;

include!(concat!(env!("OUT_DIR"), "/app.rs"));
vello_renderer_impl!(SpragGuiRenderer, SpragGuiRendererError);

/// The window's initial logical-pixel size. The grid fills it and the terminal
/// `(cols, rows)` derive from `WINDOW / measured-cell`
/// ([`grid_dims`](terminal::grid_dims); §3 — the window drives the winsize, the
/// inverse of the headless host). The window stays resizable; live resize -> PTY
/// reflow keeps the dims current via the [`install_reflow`]
/// Effect.
const WINDOW_W: u32 = 960;
const WINDOW_H: u32 = 600;

/// Paint-root + input-engine ([`SpragPaneExternal`]) anchor tag, and the single
/// focus tab stop (`V::tag()` on the root container).
const ROOT_TAG: &str = "sprag_gui";

struct TerminalViewer;

impl WidgetCore for TerminalViewer {
    type State = ();
    type Event = ();

    /// The root model is the boot pane's input engine ([`SpragPaneExternal`],
    /// tagged [`ROOT_TAG`]) over the pane's live `SessionHandle`. The shell runs
    /// this inside the root Owner scope, so [`use_terminal`]
    /// resolves the (already-booted) pane here. [`Self::apply_key`] routes
    /// keystrokes to this External's `invoke("key", ...)` — the **same** wire the
    /// headless RPC path drives (one input substrate, §2 #2; key->PTY-byte
    /// encoding is sprag's, R2.6).
    fn create_external() -> Box<dyn External> {
        let terminal = use_terminal();
        Box::new(SpragPaneExternal::new(terminal.pane_handle()))
    }

    /// Boot the live terminal (spawn the PTY + wire `on_dirty` -> repaint) AND
    /// install the resize -> reflow [`Effect`](pinion_core::reactive::Effect),
    /// before the first paint and off the pure `view`.
    /// [`install_reflow`] resolves
    /// [`use_terminal`], so it both boots the pane and
    /// wires the reflow in one call. Then focus the terminal (the single tab
    /// stop) so keystrokes reach the pane without a click — the shell drains this
    /// focus request before the first paint.
    fn create_extra_externals() -> Vec<ExtraExternal> {
        let _reflow = install_reflow();
        pinion_core::focus_request::request(ROOT_TAG);
        Vec::new()
    }

    /// Route a focused keystroke to the boot pane's PTY — delegates to
    /// [`route_key`] (the roving-tabindex focus gate + the
    /// `invoke("key", ...)` wire + the scrollback keys).
    fn apply_key(scene: &mut Scene, focused: Option<&str>, key: &str, modifiers: Modifiers) -> bool {
        route_key(scene, focused, key, modifiers)
    }

    /// Route committed IME text to the boot pane's PTY — delegates to
    /// [`route_composition`] (the focus gate + the
    /// literal `invoke("text", ...)` wire; preedit / cancel are no-ops).
    fn apply_composition(scene: &mut Scene, focused: Option<&str>, event: &CompositionEvent) -> bool {
        route_composition(scene, focused, event)
    }

    fn tag() -> &'static str {
        ROOT_TAG
    }

    fn read_state(_scene: &Scene) {}

    fn view(state: (), frame: &Frame) -> Scene {
        view::view(state, frame)
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
        "sprag terminal (interactive)"
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
