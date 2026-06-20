//! `sprag-gui` — the **interactive GPU windowed terminal** (R24-R36).
//!
//! A window that tiles N terminal panes, paints each one's **live** screen (and
//! its scrollback history), and types into the focused one. It is the human
//! observation/interaction path; the north star (an AI reading/driving the
//! terminal as *data*) is the headless `sprag-host` RPC path, which needs none of
//! this. This binding is a faithful pixel projection of the *same* cell data the
//! AI reads — it reuses the host's per-pane projection + tiling seams
//! ([`sprag_host::pane_view_scene`] / [`sprag_host::workspace_view_scene`]) rather
//! than re-deriving them — and routes keystrokes through the *same*
//! [`SpragPaneExternal`] `invoke("key", ...)` wire the AI drives (§2 #2).
//!
//! ## Multi-pane (R36): N tiled panes, one focused
//!
//! [`pane_count`](terminal::pane_count) panes (default 2, `SPRAG_GUI_PANES=<n>`,
//! capped at [`MAX_PANES`](terminal::MAX_PANES)) are spawned at boot and tiled
//! left-to-right. Each pane has a single identity [`pane_tag`](terminal::pane_tag)
//! that is its model-scene input External tag (input routing), its
//! [`focusable_tags`](WidgetCore::focusable_tags) / focus tag, its paint-scene
//! Container tag (the pinion R1012
//! [`use_pane_viewport_size`](pinion_core::use_pane_viewport_size) rect target +
//! framework focus ring + click-focus anchor), and its per-pane reflow Effect tag
//! — one string so input / focus / measure / paint can never address different
//! panes. The model scene is `Container([pane0, ...panesN])`
//! ([`WidgetCore::create_external`] is pane 0; [`WidgetCore::create_extra_externals`]
//! the rest), so [`WidgetCore::apply_key`] reaches the focused pane by
//! `find_external_with_tag_mut(focused)`. `Ctrl+PageUp/Down` cycles focus; the
//! framework draws the focus ring around the active pane. Per-pane GUI state
//! (scroll offset, IME preedit) is keyed by tile index ([`input`]).
//!
//! ## Module map (R32, R36)
//!
//! The binding is split by concern so each axis grows in one place:
//!
//! - [`terminal`] — the booted [`TerminalView`](terminal::TerminalView) model
//!   (N panes), the [`pane_tag`](terminal::pane_tag) / [`pane_count`](terminal::pane_count)
//!   identity SSOT, font/command config, and the [`grid_dims`](terminal::grid_dims)
//!   winsize SSOT; [`use_terminal`] self-creates it.
//! - [`reflow`] — the per-pane resize -> PTY reflow [`Effect`](pinion_core::reactive::Effect)s
//!   ([`install_reflow`]).
//! - [`input`] — focused-pane keystroke / IME commit -> PTY routing
//!   ([`route_key`] / [`route_composition`]), the focus-cycle chord, and the
//!   per-pane scrollback-view offset / preedit.
//! - [`a11y`] — the per-pane accessible-node projection (the human-AT path).
//! - [`view`] — the pure view-fn ([`view::view`]) tiling the panes + the
//!   surface-filled paint root.
//!
//! ## How it stays on the substrate (no hacks)
//!
//! A child process writes its PTY from a **separate OS thread**, so the static
//! `view` must read changing, cross-thread (`Send`) data and the window must
//! repaint on change without owning the event loop. The seams (all pinion):
//!
//! - [`use_terminal`] self-creates the
//!   [`Workspace`](sprag_terminal::Workspace) + the N panes once in an
//!   `Owner::cache` hook (the `use_storage` pattern — nothing flows through
//!   `main`), spawned in [`WidgetCore::create_extra_externals`] at boot.
//! - Each pane is spawned via [`Workspace::spawn_with_dirty`](sprag_terminal::Workspace::spawn_with_dirty)
//!   (the sprag R23 hook) with an `on_dirty` that calls
//!   [`RepaintSink::request_repaint`](pinion_core::RepaintSink::request_repaint)
//!   (pinion R999) — any pane's PTY batch wakes the shell; the next frame's `view`
//!   re-reads the screens. Event-driven, no polling. `State` stays `()` (`Copy`).
//! - The cell is **measured once** at boot from the resolved monospace via
//!   pinion R1003 [`measured_monospace_cell`](pinion_core::measured_monospace_cell)
//!   (the shell seeds the provider before factories) — no private `FontContext`,
//!   no double measurement. `pane_view_scene` pins that size on each node so the
//!   painted advance equals `cell_w` (R1002).
//!
//! ## Winsize (§3): the window drives the size, resize reflows each PTY (R26, R36)
//!
//! The window opens at [`WINDOW_W`] x [`WINDOW_H`]; the tiling fills it, so each
//! pane's resolved sub-rect IS its viewport and its `(cols, rows)` derive from
//! `sub-rect / cell` ([`grid_dims`](terminal::grid_dims), the single derivation
//! SSOT). Each pane's PTY boots at the full-window dims (the honest pre-layout
//! value) and a **per-pane** reflow Effect ([`install_reflow`]) keeps it live: it
//! subscribes to pinion's R1012 per-pane viewport-size
//! [`Signal`](pinion_core::reactive::Signal) (`use_pane_viewport_size(pane_tag)`)
//! and, whenever that pane's measured rect changes (an OS resize re-divides the
//! tiles), re-derives `(cols, rows)` and reflows that pane (`TIOCSWINSZ`). The
//! reflow is a real side-effect (an ioctl on a live fd), so it lives in an
//! [`Effect`](pinion_core::reactive::Effect) — gated out of `dry_run` / snapshot
//! paint by the seam — never the pure `view`. No window-side split math: the
//! authoritative sub-rect comes from pinion's post-layout measure, not a
//! consumer-replicated flex calc (the SSOT trap).
//!
//! ## Input (R27, R36): keystroke -> the focused pane's PTY
//!
//! The model scene is `Container([pane0, ...panesN])`, each pane an
//! [`SpragPaneExternal`] tagged its [`pane_tag`](terminal::pane_tag) (built in
//! [`WidgetCore::create_external`] / [`WidgetCore::create_extra_externals`] over each
//! pane's `SessionHandle`). Each pane is a focusable tab stop, so
//! [`WidgetCore::apply_key`] routes a focused keystroke + W3C modifiers to the
//! **focused** pane's External (`find_external_with_tag_mut(focused)`) via
//! `invoke("key", {key, ctrl, alt, shift, super})` — the *same* `scene/invoke`
//! channel the RPC client uses, where the sprag-owned encoder
//! ([`sprag_input`](https://docs.rs/sprag-input)) turns the key into PTY bytes
//! (R2.6). Returning `true` swallows Escape/Tab from the shell's quit/traverse
//! defaults so a full-screen TUI (vim) receives them. `Ctrl+PageUp/Down` is
//! reserved to cycle focus between tiles (a pinion `focus_request`, not the PTY).
//!
//! IME-composed input (R31, R34) — Hangul, CJK — arrives not as keystrokes but
//! as [`WidgetCore::apply_composition`] events targeting the focused pane. The
//! in-progress preedit is mirrored into that pane's
//! [`use_preedit`](input::use_preedit) overlay Signal and drawn underlined at its
//! cursor by `view` (R34 — see [`route_composition`] and `sprag_grid::overlay_preedit`
//! for why a terminal renders the preedit itself); on
//! [`CompositionEvent::Commit`] the overlay clears and the finished text is
//! written *literally* (no key-encoding) through the focused pane's sibling
//! `invoke("text", …)` wire.
//!
//! ## Scrollback (R29, R36): scroll a pane's history view
//!
//! `Shift+PageUp` / `Shift+PageDown` scroll a view over the **focused pane's**
//! history (a per-pane [`Signal`](pinion_core::reactive::Signal)-backed offset in
//! lines from the live bottom; [`route_key`] writes it, `view` reads it, so a
//! scroll re-renders that pane). The view reuses the host's per-pane projection
//! seam at its scrolled entry, so history and the live screen share one
//! authority. The scroll keys do NOT reach the PTY; any other key snaps that pane
//! back to the live bottom (you type at the prompt). Scrolled history is
//! **text-only** (the R16 scrollback model keeps text, not cells) — it renders in
//! default colors; the live screen is exact. `offset == 0` follows the bottom
//! with no drift; scrolling *during* active output may shift (the offset is
//! relative to the live bottom) — a v1 limit.

mod a11y;
mod input;
mod reflow;
mod terminal;
mod view;

use pinion_core::external::External;
use pinion_core::widget_core::ExtraExternal;
use pinion_core::{CompositionEvent, Frame, Modifiers, Scene, WidgetCore};
use pinion_shell::{SizeStrategy, WidgetView, vello_renderer_impl};
use sprag_host::SpragPaneExternal;

use crate::input::{route_composition, route_key};
use crate::reflow::install_reflow;
use crate::terminal::{PANE_TAGS, pane_count, pane_tag, use_terminal};

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

/// The paint-root / window-background `Scene::Container` tag (the surface fill
/// behind the tiled panes; see [`view::view`]). NOT a pane or a focus stop — the
/// per-pane identity + focus tags are [`pane_tag`] (`PANE_TAGS`), and the primary
/// input External is tagged `pane_tag(0)` via [`TerminalViewer::tag`].
const ROOT_TAG: &str = "sprag_gui";

struct TerminalViewer;

impl WidgetCore for TerminalViewer {
    type State = ();
    type Event = ();

    /// The primary input External is **pane 0**'s [`SpragPaneExternal`] (tagged
    /// [`Self::tag`] == `pane_tag(0)`) over its live `SessionHandle`; panes 1.. are
    /// the [`Self::create_extra_externals`] extras, so the shell's model scene is
    /// `Container([pane0, ...panesN])`. The shell runs this inside the root Owner
    /// scope, so [`use_terminal`] resolves the (already-booted) panes here.
    /// [`Self::apply_key`] routes a keystroke to the FOCUSED pane's External
    /// (`find_external_with_tag_mut(pane_tag)` -> `invoke("key", ...)`) — the
    /// **same** wire the headless RPC path drives (one input substrate, §2 #2;
    /// key->PTY-byte encoding is sprag's, R2.6).
    fn create_external() -> Box<dyn External> {
        Box::new(SpragPaneExternal::new(use_terminal().pane_handle(0)))
    }

    /// Boot the live terminal (spawn the N pane PTYs + wire each `on_dirty` ->
    /// repaint) AND install the per-pane resize -> reflow
    /// [`Effect`](pinion_core::reactive::Effect)s (pinion R1012), before the first
    /// paint and off the pure `view`. [`install_reflow`] resolves [`use_terminal`]
    /// (booting the panes) and wires every pane's reflow in one call. Then focus
    /// pane 0 so keystrokes reach a pane without a click — the shell drains this
    /// focus request before the first paint. Returns panes 1.. as the extra input
    /// Externals (pane 0 is the primary), each tagged its [`pane_tag`].
    fn create_extra_externals() -> Vec<ExtraExternal> {
        let terminal = use_terminal();
        install_reflow();
        pinion_core::focus_request::request(pane_tag(0));
        (1..terminal.pane_count())
            .map(|i| {
                ExtraExternal::new(
                    pane_tag(i),
                    Box::new(SpragPaneExternal::new(terminal.pane_handle(i))),
                )
            })
            .collect()
    }

    /// Route a focused keystroke to the focused pane's PTY — delegates to
    /// [`route_key`] (the roving-tabindex focus gate + the focused pane's
    /// `invoke("key", ...)` wire + the focus-cycle / scrollback chords).
    fn apply_key(
        scene: &mut Scene,
        focused: Option<&str>,
        key: &str,
        modifiers: Modifiers,
    ) -> bool {
        route_key(scene, focused, key, modifiers)
    }

    /// Route committed IME text to the focused pane's PTY — delegates to
    /// [`route_composition`] (the focus gate + the focused pane's literal
    /// `invoke("text", ...)` wire + its preedit overlay).
    fn apply_composition(
        scene: &mut Scene,
        focused: Option<&str>,
        event: &CompositionEvent,
    ) -> bool {
        route_composition(scene, focused, event)
    }

    /// Pane 0's identity tag — the primary input External
    /// ([`Self::create_external`]) is tagged with it (the model-scene primary's
    /// tag is `V::tag()`), so it joins panes 1.. (the extras) under one uniform
    /// [`pane_tag`] addressing.
    fn tag() -> &'static str {
        pane_tag(0)
    }

    fn read_state(_scene: &Scene) {}

    fn view(state: (), frame: &Frame) -> Scene {
        view::view(state, frame)
    }

    fn event_name(_event: ()) -> &'static str {
        "__internal__"
    }

    /// Each tiled pane is a tab stop ([`pane_tag`], one per pane up to
    /// `pane_count()`) — focusing one gates [`Self::apply_key`] to that pane (and
    /// a click on its rect re-focuses it; the framework draws its focus ring).
    /// `Ctrl+PageUp/Down` cycles between them ([`route_key`]).
    /// [`Self::create_extra_externals`] focuses pane 0 at boot so typing works
    /// without a click.
    fn focusable_tags() -> Vec<&'static str> {
        PANE_TAGS[..pane_count()].to_vec()
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

#[cfg(test)]
mod tests {
    use super::*;
    use pinion_shell::ShellCore;

    /// Collect every painted `TextGrid`'s `(cols, rows)` by walking the scene
    /// tree (the pane grids sit inside the tiling Containers; the focus-ring
    /// overlay is a `Box`, not a grid, so it is ignored).
    fn painted_grid_dims(scene: &Scene) -> Vec<(u16, u16)> {
        match scene {
            Scene::TextGrid(node) => vec![(node.cells().cols(), node.cells().rows())],
            Scene::Container(c) => c.children.iter().flat_map(painted_grid_dims).collect(),
            Scene::Scroll(s) => painted_grid_dims(&s.content),
            _ => Vec::new(),
        }
    }

    /// End-to-end multi-pane reflow through the REAL `TerminalViewer` driven by
    /// the shell's live paint path: build the shell (boots 2 panes, installs the
    /// per-pane R1012 reflow Effects), drive one `compute_paint_scene`, and assert
    /// each pane's painted grid reflowed to ITS measured half-rect (not the full
    /// window) — same-frame (the publish dirty-bit re-pass). pinion's
    /// `pane_viewport_seam.rs` proves the seam with a mock widget; this proves
    /// sprag tags/registers/derives correctly so the real viewer tiles and
    /// reflows. The shell seeds the monospace font provider, so the cell is the
    /// real measured metric (read back from the booted terminal), not
    /// `CellMetric::DEFAULT`.
    #[test]
    fn tiles_two_panes_each_reflowed_to_its_half_rect() {
        // Default env: 2 panes. (No SPRAG_GUI_PANES set in the test process.)
        let mut core = ShellCore::<TerminalViewer>::new();
        let scene = core.compute_paint_scene(WINDOW_W, WINDOW_H);
        let dims = painted_grid_dims(&scene);
        // The actual measured cell the shell sized the panes with (its monospace
        // provider is seeded, so this is NOT CellMetric::DEFAULT) — read from the
        // booted terminal in the shell's own root owner, then derive the
        // full-window dims at that cell to compare against.
        let metric = core.root_owner().run(|| use_terminal().metric);
        let (full_cols, full_rows) = crate::terminal::grid_dims((WINDOW_W, WINDOW_H), metric);

        assert_eq!(
            dims.len(),
            2,
            "two tiled pane grids are painted, got {dims:?}"
        );
        for (cols, rows) in &dims {
            // Horizontal split: each pane keeps the full height...
            assert_eq!(*rows, full_rows, "a horizontal split preserves pane height");
            // ...but is reflowed to roughly half the width — strictly narrower
            // than a full-window pane (the per-pane R1012 reflow shrank it from
            // its full-window boot size), same-frame.
            assert!(
                *cols < full_cols && *cols >= full_cols / 2 - 3,
                "pane reflowed to ~half ({cols} cols vs full {full_cols})",
            );
        }
        // The even split makes the two panes within one cell of each other.
        assert!(
            dims[0].0.abs_diff(dims[1].0) <= 1,
            "the two panes split the window evenly, got {dims:?}",
        );
    }
}
