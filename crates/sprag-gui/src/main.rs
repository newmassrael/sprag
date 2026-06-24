//! `sprag-gui` — the **interactive GPU windowed terminal** (R24-R38).
//!
//! A window that tiles N terminal panes, paints each one's **live** screen (and
//! its scrollback history), and types into the focused one. It is the human
//! observation/interaction path; the north star (an AI reading/driving the
//! terminal as *data*) is the headless `sprag-host` RPC path, which needs none of
//! this. This binding is a faithful pixel projection of the *same* cell data the
//! AI reads — it reuses the host's per-pane projection seam
//! ([`sprag_host::pane_view_scene`]) and arranges the panes itself (the
//! interactive [`split`] layout — reactive ratios the headless host has no use
//! for) — and routes keystrokes through the *same* [`SpragPaneExternal`]
//! `invoke("key", ...)` wire the AI drives (§2 #2).
//!
//! ## Multi-pane (R36): N tiled panes, one focused
//!
//! [`pane_count`](terminal::pane_count) panes (default 2, `SPRAG_GUI_PANES=<n>`,
//! capped at [`MAX_PANES`](terminal::MAX_PANES)) are spawned at boot and tiled
//! left-to-right. Each pane has a single identity [`pane_tag`]
//! that is its model-scene input External tag (input routing), its focus tag, its
//! paint-scene Container tag (the pinion R1012
//! [`use_pane_viewport_size`](pinion_core::use_pane_viewport_size) rect target +
//! framework focus ring + click-focus anchor), and its per-pane reflow Effect tag
//! — one string so input / focus / measure / paint can never address different
//! panes. Keyboard focusability is **scene-derived** (pinion R1020 §5.39): the
//! pane's paint Container is marked `with_focusable(true)` in
//! [`sprag_host::pane_view_scene`] and the shell collects the Tab order each frame
//! via [`Scene::collect_focusable_tags`](pinion_core::Scene::collect_focusable_tags)
//! — there is no binding-side `focusable_tags()` list. The model scene is `Container([pane0, ...panesN])`
//! ([`WidgetCore::create_external`] is pane 0; [`WidgetCore::create_extra_externals`]
//! the rest), so [`WidgetCore::apply_key`] reaches the focused pane by
//! `find_external_with_tag_mut(focused)`. `Ctrl+PageUp/Down` cycles focus; the
//! framework draws the focus ring around the active pane. Per-pane GUI state
//! (scroll offset, IME preedit) is keyed by tile index ([`input`]).
//!
//! ## Dock / undock (R37): a pane in its own OS window
//!
//! `Ctrl+Shift+Enter` toggles the focused pane between **docked** (tiled in the
//! main window) and **undocked** (painted alone in its own OS window), via
//! pinion's multi-window seam: [`WidgetView::windows_signal`] returns the
//! [`dock`] topology `Signal<Vec<WindowSpec>>` (the floating SSOT — a pane floats
//! iff its `pane-{i}` window exists), and [`WidgetView::view_for_window`]
//! dispatches per window (main tiles the docked panes; `pane-{i}` paints that
//! pane). **Input is unchanged**: the model scene + focus are global, so a
//! keystroke reaches the focused pane's PTY regardless of which window paints it
//! — dock/undock changes only *where* a pane is painted, never its tag /
//! External / focusability (so it does NOT need runtime-focusable changes).
//! The undock window opens sized to the pane's intrinsic `(cols, rows) × cell` and
//! is **freely resizable — grow AND shrink — with the pane reflowing to its own
//! window size in both axes on OS resize**. Two pinion seams: R1021 publishes the
//! per-pane viewport rect for EVERY painted window (so the floated pane's existing
//! reflow Effect fires on the secondary window's rect), and R1059
//! `SizeStrategy::OpenResizable { min: None }` decouples the open size from the
//! OS-resize floor (so the window shrinks below its open size — `Fixed` blocked
//! that). See `dock` docs. Per-window a11y partitions nodes
//! ([`a11y::access_nodes_for_window`]).
//!
//! ## Dock split-tree layout (R60): drag to resize, collapse on undock
//!
//! The docked panes are arranged by a pinion [`DockTopology`](pinion_widget_paint::dock::DockTopology)
//! — an identity-keyed binary split-tree ([`split`]) — lowered to pixels by
//! [`view_dock_surface`](pinion_widget_paint::dock::view_dock_surface), which wraps
//! each pane in a [`view_dock_panel`](pinion_widget_paint::dock::view_dock_panel)
//! (a 28px header strip — the drag / tear-off handle — above the pane) and nests a
//! `view_splitter` per Split. Each Split's ratio is an `Owner::cache`-shared
//! `Signal<f32>` keyed on the Split's STABLE id ([`split::use_split_ratio`]): the
//! view reads it, and a [`SplitterExternal`] registered at that id
//! ([`create_extra_externals`](TerminalViewer)) writes it on a pointer drag (the
//! shell's pointer router delivers the drag — no `WidgetCore` pointer method). A
//! drag re-weights the flex layout -> the pane rects change -> the R1012 reflow
//! Effects resize the PTYs (automatic; the `reflow` seam was built for it).
//!
//! This retires the former flat row/grid model (and the `SPRAG_GUI_LAYOUT` env): the
//! topology holds only the DOCKED panes, so undocking a pane removes its leaf and the
//! rest reclaim its space ([`split::float_pane`]; docking back re-inserts it,
//! [`split::dock_pane`]). A dock-back mints a fresh Split id, so the splitter set is a
//! runtime-mutable projection of the topology — [`create_extra_externals`](TerminalViewer)
//! walks the LIVE topology and [`external_set_is_dynamic`](TerminalViewer::external_set_is_dynamic)
//! opts into pinion R689's per-frame reconcile so the new divider registers a routable
//! `SplitterExternal` and becomes drag-resizable. The interactive layout lives
//! GUI-side (not in the host) because it needs reactive ratios + registered Externals.
//!
//! ## Module map (R32, R36, R37, R60)
//!
//! The binding is split by concern so each axis grows in one place:
//!
//! - [`terminal`] — the booted [`TerminalView`](terminal::TerminalView) model
//!   (N panes), the [`pane_tag`] / [`pane_count`](terminal::pane_count)
//!   identity SSOT, font/command config, and the [`grid_dims`](terminal::grid_dims)
//!   winsize SSOT; [`use_terminal`] self-creates it.
//! - [`reflow`] — the per-pane resize -> PTY reflow [`Effect`](pinion_core::reactive::Effect)s
//!   ([`install_reflow`]).
//! - [`input`] — focused-pane keystroke / IME commit -> PTY routing
//!   ([`route_key`] / [`route_composition`]), the focus-cycle + dock-toggle
//!   chords, and the per-pane scrollback-view offset / preedit.
//! - [`dock`] — which OS window paints each pane: the topology
//!   [`Signal`] (floating SSOT) +
//!   [`toggle_pane_floating`](dock::toggle_pane_floating).
//! - [`split`] — the dock split-tree model: the held [`DockTopology`](pinion_widget_paint::dock::DockTopology)
//!   Signal ([`use_dock_topology`](split::use_dock_topology)), its collapse-on-undock
//!   mutation ([`float_pane`](split::float_pane) / [`dock_pane`](split::dock_pane)),
//!   and the per-Split ratio Signals ([`use_split_ratio`](split::use_split_ratio)).
//! - [`a11y`] — the per-pane (per-window) accessible-node projection (human-AT).
//! - [`view`] — the per-window paint ([`view::view_for_window`]: main tiling /
//!   single undocked pane) + the surface-filled paint root.
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
//! [`Signal`] (`use_pane_viewport_size(pane_tag)`)
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
//! [`SpragPaneExternal`] tagged its [`pane_tag`] (built in
//! [`WidgetCore::create_external`] / [`WidgetCore::create_extra_externals`] over each
//! pane's `SessionHandle`). Each pane is a focusable tab stop (its paint Container
//! is `with_focusable(true)`, R1020 scene-derived focus — above), so
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
//! history (a per-pane [`Signal`]-backed offset in
//! lines from the live bottom; [`route_key`] writes it, `view` reads it, so a
//! scroll re-renders that pane). The view reuses the host's per-pane projection
//! seam at its scrolled entry, so history and the live screen share one
//! authority. The scroll keys do NOT reach the PTY; any other key snaps that pane
//! back to the live bottom (you type at the prompt). Scrolled history retains its
//! **styled cells** (fg/bg/attrs preserved; R58 — scrollback stores cells, not
//! text), so it renders in its original colors, identical to the live screen.
//! `offset == 0` follows the bottom with no drift;
//! scrolling *during* active output may shift (the offset is relative to the live
//! bottom) — a v1 limit.

mod a11y;
mod diag;
mod dock;
mod input;
mod reflow;
mod scrollbar;
mod split;
mod terminal;
mod view;

use pinion_a11y::AccessNode;
use pinion_core::event::{LINE_HEIGHT_PX, WheelDelta};
use pinion_core::external::External;
use pinion_core::reactive::{Owner, Signal};
use pinion_core::widget_core::ExtraExternal;
use pinion_core::{CompositionEvent, Frame, Modifiers, Scene, WidgetCore};
use pinion_shell::{SizeStrategy, WidgetView, WindowSpec, vello_renderer_impl};
use pinion_widget_paint::splitter::SplitterExternal;
use sprag_host::SpragPaneExternal;
use std::rc::Rc;

use crate::input::{route_composition, route_key};
use crate::reflow::install_reflow;
use crate::terminal::{pane_scrollbar_tag, pane_tag, use_terminal};

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
/// behind the tiled panes; see the [`view`] module). NOT a pane or a focus stop — the
/// per-pane identity + focus tags are [`pane_tag`] (`PaneSlot`), and the primary
/// input External is tagged `pane_tag(0)` via [`TerminalViewer::tag`].
const ROOT_TAG: &str = "sprag_gui";

/// `Owner::cache` key for the boot-focus seed marker ([`BootFocusSeed`]).
const BOOT_FOCUS_KEY: &str = "sprag_gui.boot_focus_seed";

/// Zero-size marker cached once to run the boot focus request a single time. The
/// dynamic external set ([`TerminalViewer::external_set_is_dynamic`]) makes the shell
/// re-run [`create_extra_externals`](TerminalViewer::create_extra_externals) every
/// reconcile, so the one-shot focus seed lives behind an `Owner::cache` factory that
/// fires exactly once (the R689 "boot-time seeding side effect must not re-fire" rule).
struct BootFocusSeed;

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
        // Boot-once focus seed. `external_set_is_dynamic` makes the shell re-run this
        // factory every reconcile (to register a dock-back-minted splitter), so
        // requesting focus unconditionally would re-pin it to pane 0 each frame and
        // defeat click / Ctrl+PageUp focus moves. The `Owner::cache` factory fires the
        // seed exactly once (the R689 "boot-time seeding must not re-fire" rule).
        Owner::current()
            .expect("create_extra_externals runs in the root Owner scope")
            .cache(BOOT_FOCUS_KEY, || {
                pinion_core::focus_request::request(pane_tag(0));
                BootFocusSeed
            });
        // Panes 1.. are the extra input Externals (pane 0 is the primary).
        let mut externals: Vec<ExtraExternal> = (1..terminal.pane_count())
            .map(|i| {
                ExtraExternal::new(
                    pane_tag(i),
                    Box::new(SpragPaneExternal::new(terminal.pane_handle(i))),
                )
            })
            .collect();
        // The draggable dividers: one `SplitterExternal` per Split in the LIVE dock
        // topology ([`split::use_dock_topology`]), keyed on the Split's stable id —
        // which IS the painted `SplitterStyle` tag the view's `view_dock_surface`
        // walker emits (one SSOT), with the topology's orientation + initial ratio.
        // Walking the LIVE topology (not a fixed boot list) is what lets a dock-back-
        // minted split register its External and become drag-resizable:
        // [`Self::external_set_is_dynamic`] opts this factory into the per-frame
        // `reconcile_externals`, which re-runs it on a topology change and registers
        // the new tag while preserving surviving splitters' drag state (pinion R689).
        // Pointer-only (never focusable — their handles carry no `with_focusable`, so
        // R1020's per-frame `collect_focusable_tags` never enumerates them). The ratio
        // `Signal` is shared with the view via `split::use_split_ratio`.
        if let Some(topology) = split::use_dock_topology().get() {
            topology.for_each_split(|id, orientation, ratio| {
                externals.push(ExtraExternal::new(
                    id.to_string(),
                    Box::new(
                        SplitterExternal::new(orientation)
                            .attach_ratio(split::use_split_ratio(id.to_string(), ratio)),
                    ),
                ));
            });
        }
        // One draggable scrollbar peer per pane (R49): a `ScrollBarExternal` over
        // the pane's row-unit `ScrollState`, tagged its `scrollbar.{i}` so the
        // shell's pointer router routes a press on the painted track to it. Like
        // the splitters, pointer-only (never focusable); the caller-owned
        // `ScrollState` authority (pinion R1032) needs no mirror.
        externals.extend((0..terminal.pane_count()).map(scrollbar::pane_scrollbar_external));
        externals
    }

    /// The splitter external set is a projection of the live dock topology
    /// ([`split::use_dock_topology`]): a dock-back gesture mints a fresh Split id whose
    /// `SplitterExternal` must register to make the new divider draggable. Opt into the
    /// per-frame `CoreShell::reconcile_externals`
    /// (pinion R689) so the new surface gets a routable target; a static binding leaves
    /// this at the `false` default. See [`Self::create_extra_externals`] (the boot-once
    /// focus seed is guarded for exactly this re-run).
    fn external_set_is_dynamic() -> bool {
        true
    }

    /// Route a focused keystroke to the focused pane's PTY — delegates to
    /// [`route_key`] (the roving-tabindex focus gate + the focused pane's
    /// `invoke("key", ...)` wire + the focus-cycle / scrollback / dock-toggle chords).
    ///
    /// This is the `repeat == false` entry — the RPC `scene/key` injection and any
    /// single-activation caller (a synthesised key is never an OS auto-repeat). The
    /// live shell drives the repeat-aware sibling [`Self::apply_key_repeat`] instead.
    fn apply_key(
        scene: &mut Scene,
        focused: Option<&str>,
        key: &str,
        modifiers: Modifiers,
    ) -> bool {
        diag::key_in(
            "apply_key",
            key,
            modifiers.ctrl,
            modifiers.shift,
            false,
            focused,
        );
        route_key(scene, focused, key, modifiers, false)
    }

    /// Repeat-aware key dispatch (pinion R1071 / PINION-PR27) — the variant the live
    /// shell drives, carrying the platform `KeyEvent.repeat` flag. Both this and
    /// [`Self::apply_key`] are thin delegates to [`route_key`], which OWNS the repeat
    /// policy: a held DISCRETE window chord (dock-toggle / focus-cycle) acts once per
    /// press, not on every OS auto-repeat — without this a held `Ctrl+Shift+Enter`
    /// dock-then-undocked in the multi-window state. Scrollback chords and PTY keys
    /// still repeat (continuous).
    fn apply_key_repeat(
        scene: &mut Scene,
        focused: Option<&str>,
        key: &str,
        modifiers: Modifiers,
        repeat: bool,
    ) -> bool {
        diag::key_in(
            "apply_key_repeat",
            key,
            modifiers.ctrl,
            modifiers.shift,
            repeat,
            focused,
        );
        route_key(scene, focused, key, modifiers, repeat)
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

    /// Pre-view reconcile (pinion R1047 / PR-20): grow each pane's row-unit
    /// `ScrollState` bound to its live scrollback depth and tail-follow, BEFORE the
    /// pure `view` fn runs. This is the sanctioned non-view-fn place for the
    /// reactive `Signal` write the bar/projection need current — the terminal grid
    /// is an `offset_lines`-projecting `Scene::TextGrid` (no `Scene::Scroll` clip for
    /// the layout reducer) and `scrollback_len` lives in an off-thread PTY producer
    /// (no `Signal` for an `Effect`), so neither runtime path can reconcile it. Runs
    /// in the binding root `Owner`, so [`use_terminal`] / `use_pane_scroll` resolve.
    /// Caveat: pinion gates this to the primary window's paint, so a FLOATED pane
    /// whose undock window alone repaints can lag one bound-grow until the primary
    /// repaints — harmless in practice (live PTY output flips the root dirty bit, so
    /// the primary repaints too).
    fn reconcile_frame() {
        let terminal = use_terminal();
        for i in 0..terminal.pane_count() {
            let scrollback_len = terminal
                .pane(i)
                .session()
                .with_screen(|screen| screen.scrollback_len());
            scrollbar::reconcile_scroll(&scrollbar::use_pane_scroll(i), scrollback_len);
        }
    }

    /// Mouse-wheel / touchpad two-finger scroll over a pane scrolls its scrollback
    /// (pinion R1045 / PR-18 — the GUI-side wheel seam). Hit-tests the cursor to the
    /// pane under it (grid OR scrollbar gutter), converts the `WheelDelta` to LINES
    /// (notched `Lines.dy`, or touchpad `Pixels.dy / LINE_HEIGHT_PX`), and scrolls
    /// the **GUI's own** `ScrollState` via [`scrollbar::wheel_scroll_pane`] — NOT the
    /// AI-facing pane engine (the R1.7 boundary). Runs in the binding root `Owner`.
    fn apply_wheel(
        scene: &Scene,
        cursor: (f64, f64),
        delta: WheelDelta,
        _modifiers: Modifiers,
    ) -> bool {
        let (cx, cy) = cursor;
        let hit = |tag: &str| {
            scene.rect_for_tag_absolute(tag).is_some_and(|r| {
                cx >= f64::from(r.x)
                    && cx < f64::from(r.x) + f64::from(r.w)
                    && cy >= f64::from(r.y)
                    && cy < f64::from(r.y) + f64::from(r.h)
            })
        };
        // A pane's grid and its scrollbar gutter are disjoint sibling rects
        // (wrap_pane_with_bar), so first-hit is unambiguous; and a pane tag is
        // painted in at most one window per frame (its dock window), so on an undock
        // window's single-pane scene the absent panes simply miss (rect -> None).
        let Some(i) = (0..use_terminal().pane_count())
            .find(|&i| hit(pane_tag(i)) || hit(pane_scrollbar_tag(i)))
        else {
            return false;
        };
        let lines = match delta {
            WheelDelta::Lines { dy, .. } => dy,
            WheelDelta::Pixels { dy, .. } => dy / LINE_HEIGHT_PX,
            _ => return false,
        };
        if lines == 0.0 {
            return false;
        }
        scrollbar::wheel_scroll_pane(i, lines);
        true
    }

    /// Pane 0's identity tag — the primary input External
    /// ([`Self::create_external`]) is tagged with it (the model-scene primary's
    /// tag is `V::tag()`), so it joins panes 1.. (the extras) under one uniform
    /// [`pane_tag`] addressing.
    fn tag() -> &'static str {
        pane_tag(0)
    }

    fn read_state(_scene: &Scene) {}

    /// The windowless / RPC-snapshot fallback paints the **main** window (the
    /// docked panes). The live multi-window paint goes through
    /// [`WidgetView::view_for_window`]; this keeps the no-window-context path
    /// (an RPC `scene/snapshot` without a window) showing what the human sees.
    fn view(state: (), frame: &Frame) -> Scene {
        view::view_for_window(dock::MAIN_WINDOW_ID, state, frame)
    }

    fn event_name(_event: ()) -> &'static str {
        "__internal__"
    }

    // Keyboard focus enumeration is no longer a binding-side method: pinion R1020
    // §5.39 removed `WidgetCore::focusable_tags()` and DERIVES the Tab order each
    // frame from the paint scene via `Scene::collect_focusable_tags` — the
    // `focusable`-marked, tagged nodes. Each pane declares itself a Tab stop where
    // it is painted: `sprag_host::pane_view_scene` marks the pane Container
    // `with_focusable(true)` (one Tab stop per pane; its inner grid is not focused).
    // Focusing a pane gates [`Self::apply_key`] to it, the framework draws its focus
    // ring, and a click on its rect re-focuses it; `Ctrl+PageUp/Down` cycles between
    // them ([`route_key`]). [`Self::create_extra_externals`] requests focus on pane 0
    // at boot so typing works without a click — the shell re-derives that request
    // against the first paint scene's collected tags (pane 0 is among them).

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

    /// Opt into runtime windows: the dock topology Signal (the floating SSOT,
    /// [`dock::use_windows_topology`]). The shell subscribes it and reconciles
    /// winit windows on each `set` — the `Ctrl+Shift+Enter` chord
    /// ([`dock::toggle_pane_floating`] via [`route_key`]) undocks/docks the
    /// focused pane.
    fn windows_signal() -> Option<Rc<Signal<Vec<WindowSpec>>>> {
        Some(dock::use_windows_topology())
    }

    /// Per-window paint: the main window tiles the DOCKED panes; an undock window
    /// (`pane-{i}`) paints that pane alone. Delegates to [`view::view_for_window`].
    fn view_for_window(window_id: &str, state: (), frame: &Frame) -> Scene {
        view::view_for_window(window_id, state, frame)
    }

    /// Per-window a11y: each window advertises only the panes IT paints (the main
    /// window the docked panes; an undock window its one pane), so a sibling
    /// window's AT tree carries no ghost pane nodes. Delegates to
    /// [`a11y::access_nodes_for_window`].
    fn access_node_for_window(
        window_id: &str,
        _state: &(),
        focused: Option<&str>,
    ) -> Vec<AccessNode> {
        a11y::access_nodes_for_window(window_id, focused)
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
        // Each docked pane now sits below a 28px dock-panel header (R60), so its
        // content height is the window minus that strip — derive the expected rows
        // through the same winsize SSOT against the reduced height.
        let header_px = pinion_widget_paint::dock::DockPanelStyle::m3_default("x").header_height_px;
        let content_rows = crate::terminal::grid_dims((WINDOW_W, WINDOW_H - header_px), metric).1;
        assert!(
            content_rows < full_rows,
            "the dock header subtracts at least one row"
        );

        assert_eq!(
            dims.len(),
            2,
            "two tiled pane grids are painted, got {dims:?}"
        );
        for (cols, rows) in &dims {
            // Horizontal split: each pane fills the window height BELOW its 28px dock
            // header (the header is the only vertical chrome a horizontal split adds)...
            assert_eq!(
                *rows, content_rows,
                "pane fills the window height below the dock header"
            );
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

    /// An UNDOCK window reflows its one pane to ITS OWN window size — the headless
    /// proof of resizable undock (pinion R1021 / PINION-PR10 per-window pane-viewport
    /// publish, consumed). Drive the `pane-0` undock window's real paint at two
    /// different sizes and assert pane 0's grid reflows to each: a larger window
    /// yields strictly more cells. The undock paint path carries NO winit
    /// `min_inner_size` floor (that limit only bounds the live user *drag*, see the
    /// [`dock`] SizeStrategy note), so the reflow MECHANISM is direction-agnostic —
    /// A and B exercise both grow and shrink. pinion's `pane_viewport_seam.rs` proves
    /// the per-window publish with a mock widget; this proves sprag's undock window
    /// tags / publishes / derives so the floated pane follows its own window. Reads
    /// the real measured cell metric from the booted terminal (the shell seeds the
    /// monospace provider), not `CellMetric::DEFAULT`.
    #[test]
    fn undock_window_reflows_its_pane_to_its_own_size() {
        let mut core = ShellCore::<TerminalViewer>::new();
        let metric = core.root_owner().run(|| use_terminal().metric);
        let win = dock::pane_window_id(0);

        // Boot through the main window once (as the app does) so the shell installs
        // the per-pane reflow Effects via `create_extra_externals` before the user
        // undocks a pane.
        let _ = core.compute_paint_scene(WINDOW_W, WINDOW_H);

        // A smaller and a clearly larger undock-window size (both axes differ).
        let (wa, ha) = (600u32, 400u32);
        let (wb, hb) = (900u32, 720u32);

        let dims_a = painted_grid_dims(&core.compute_paint_scene_for_window(&win, wa, ha));
        let dims_b = painted_grid_dims(&core.compute_paint_scene_for_window(&win, wb, hb));

        // The undock window paints exactly its one pane (no tiling, no siblings).
        assert_eq!(
            dims_a.len(),
            1,
            "undock window paints one pane, got {dims_a:?}"
        );
        assert_eq!(
            dims_b.len(),
            1,
            "undock window paints one pane, got {dims_b:?}"
        );

        // Rows fill the window height (the vertical scrollbar is a side gutter, so it
        // takes width, not height) — exact against the window-derived rows.
        let full_a = crate::terminal::grid_dims((wa, ha), metric);
        let full_b = crate::terminal::grid_dims((wb, hb), metric);
        assert_eq!(
            dims_a[0].1, full_a.1,
            "pane A fills the undock window height"
        );
        assert_eq!(
            dims_b[0].1, full_b.1,
            "pane B fills the undock window height"
        );

        // Cols track the window width minus the scrollbar gutter: strictly fewer than
        // the full-width derivation, never zero, and the larger window B reflows to
        // strictly MORE cols than A — the resize genuinely reflowed the pane.
        assert!(
            dims_a[0].0 > 0 && dims_a[0].0 < full_a.0,
            "A cols sane: {dims_a:?}"
        );
        assert!(
            dims_b[0].0 > 0 && dims_b[0].0 < full_b.0,
            "B cols sane: {dims_b:?}"
        );
        assert!(
            dims_b[0].0 > dims_a[0].0 && dims_b[0].1 > dims_a[0].1,
            "a larger undock window reflows the pane to more cells: A={dims_a:?} B={dims_b:?}",
        );
    }

    /// R1020 §5.39 scene-derived focus: the REAL viewer's paint scene marks every
    /// tiled pane Container `with_focusable(true)`, so the shell's per-frame
    /// [`Scene::collect_focusable_tags`](pinion_core::Scene::collect_focusable_tags)
    /// walk enumerates both pane tags as Tab stops — and ONLY those (the surface
    /// root and the splitter handle are not focus stops). This is the contract that
    /// replaced the removed binding-side `focusable_tags()`; a regression here would
    /// drop focus to `None` every frame (no focus ring, dead keyboard input), so it
    /// is pinned end-to-end through the live paint path.
    #[test]
    fn panes_are_scene_derived_focus_stops() {
        let mut core = ShellCore::<TerminalViewer>::new();
        let scene = core.compute_paint_scene(WINDOW_W, WINDOW_H);
        let focusable = scene.collect_focusable_tags();
        assert!(
            focusable.iter().any(|t| t == pane_tag(0)),
            "pane 0 is a scene-derived Tab stop, got {focusable:?}",
        );
        assert!(
            focusable.iter().any(|t| t == pane_tag(1)),
            "pane 1 is a scene-derived Tab stop, got {focusable:?}",
        );
        // Exactly the two panes — the surface root and the divider handle are not
        // focus stops (no `with_focusable`), so they never enter the Tab order.
        assert_eq!(
            focusable.len(),
            2,
            "only the two panes are focusable, got {focusable:?}",
        );
        assert!(
            !focusable.iter().any(|t| t == ROOT_TAG),
            "the surface root is not a Tab stop",
        );
    }

    /// R1020 §5.39 boot focus: [`create_extra_externals`](TerminalViewer::create_extra_externals)
    /// requests focus on pane 0 at boot; once the dispatch tail drains
    /// ([`ShellCore::finalize_frame`]), that request lands on `pane_tag(0)` — the
    /// drain re-derives the focusable set from the painted scene (where the pane is
    /// `with_focusable`), so the request resolves and the framework frames pane 0
    /// with its focus ring. Pre-migration (the removed `focusable_tags()` list) this
    /// was a binding-side seed; this proves the scene-derived path keeps typing
    /// working without a click.
    #[test]
    fn boot_focus_lands_on_pane_zero() {
        let mut core = ShellCore::<TerminalViewer>::new();
        // First paint seeds the scene-derived enumeration (both panes focusable).
        let scene = core.compute_paint_scene(WINDOW_W, WINDOW_H);
        // Drive the dispatch tail so the boot focus_request drains and resolves.
        core.finalize_frame(scene);
        assert_eq!(
            core.focus().focused(),
            Some(pane_tag(0)),
            "boot focus lands on pane 0 -> the framework frames it with the focus ring",
        );
    }

    /// Click-to-focus: a mouse press inside a pane focuses THAT pane, so typing
    /// then reaches it. The pane's grid is tagged `{pane_tag}#grid` (a pinion
    /// composite sub-tag), so a press on the grid — the deepest tagged node under
    /// the pointer — resolves through `resolve_focusable` (split on `#`) to the
    /// focusable pane tag. With a plain, non-composite grid tag the press resolved
    /// to nothing and focus never moved — the live bug where clicking the right
    /// pane kept typing in the left. Driven through the REAL shell pointer path
    /// (`cursor_moved` -> `mouse_pressed` -> `click_to_focus`).
    #[test]
    fn clicking_a_pane_focuses_it() {
        use pinion_runtime::PointerId;
        let mut core = ShellCore::<TerminalViewer>::new();
        // Boot-focus pane 0 (the dispatch tail drains the boot focus_request).
        let scene = core.compute_paint_scene(WINDOW_W, WINDOW_H);
        core.finalize_frame(scene);
        assert_eq!(core.focus().focused(), Some(pane_tag(0)), "boots on pane 0");

        // The router hit-tests against the last painted scene; take pane centers.
        let scene = core.compute_paint_scene(WINDOW_W, WINDOW_H);
        let center = |tag: &str| {
            let r = scene
                .rect_for_tag_absolute(tag)
                .unwrap_or_else(|| panic!("{tag} painted"));
            (
                f64::from(r.x) + f64::from(r.w) / 2.0,
                f64::from(r.y) + f64::from(r.h) / 2.0,
            )
        };
        let click = |core: &mut ShellCore<TerminalViewer>, (x, y): (f64, f64)| {
            core.cursor_moved_for_window(dock::MAIN_WINDOW_ID, PointerId::MOUSE, x, y);
            core.mouse_pressed_for_window(dock::MAIN_WINDOW_ID, PointerId::MOUSE);
        };

        // Click pane 1 -> focus moves to pane 1 (the bug: it stayed on pane 0).
        click(&mut core, center(pane_tag(1)));
        assert_eq!(
            core.focus().focused(),
            Some(pane_tag(1)),
            "clicking inside pane 1 focuses pane 1",
        );
        // Click back into pane 0 -> focus returns.
        click(&mut core, center(pane_tag(0)));
        assert_eq!(
            core.focus().focused(),
            Some(pane_tag(0)),
            "clicking inside pane 0 focuses pane 0",
        );
    }

    /// End-to-end dock/undock through the REAL `TerminalViewer` + the shell's
    /// per-window paint dispatch: undock pane 1 and assert the main window drops
    /// it while the `pane-1` undock window paints exactly it — the docked/floating
    /// partition as scene DATA. The secondary-window paint runs pure geometry
    /// (R1006/R1012 publishes are default-window-gated — the documented gap), so
    /// this asserts the partition, not a secondary reflow; the undock window is
    /// driven at the pane's own size.
    #[test]
    fn undock_partitions_panes_across_windows() {
        let mut core = ShellCore::<TerminalViewer>::new();
        // Boot: both panes docked -> the main window tiles both.
        let main0 = core.compute_paint_scene_for_window(dock::MAIN_WINDOW_ID, WINDOW_W, WINDOW_H);
        assert!(main0.contains_tag(pane_tag(0)), "main tiles pane 0");
        assert!(main0.contains_tag(pane_tag(1)), "main tiles pane 1");

        // Undock pane 1 (the chord runs in the shell root owner scope).
        core.root_owner().run(|| dock::toggle_pane_floating(1));

        // The main window now drops the floated pane 1, keeps the docked pane 0.
        let main1 = core.compute_paint_scene_for_window(dock::MAIN_WINDOW_ID, WINDOW_W, WINDOW_H);
        assert!(
            main1.contains_tag(pane_tag(0)),
            "main keeps the docked pane 0"
        );
        assert!(
            !main1.contains_tag(pane_tag(1)),
            "main drops the floated pane 1"
        );

        // The undock window paints exactly pane 1 (not pane 0).
        let undock = core.compute_paint_scene_for_window(&dock::pane_window_id(1), 400, 300);
        assert!(
            undock.contains_tag(pane_tag(1)),
            "the undock window paints pane 1"
        );
        assert!(
            !undock.contains_tag(pane_tag(0)),
            "the undock window does not paint pane 0"
        );

        // Dock back: the main window tiles pane 1 again.
        core.root_owner().run(|| dock::toggle_pane_floating(1));
        let main2 = core.compute_paint_scene_for_window(dock::MAIN_WINDOW_ID, WINDOW_W, WINDOW_H);
        assert!(main2.contains_tag(pane_tag(1)), "dock-back re-tiles pane 1");
    }

    /// Undock focus-FOLLOW (consumes pinion R1069 / PINION-PR26): undocking the focused
    /// pane keeps focus ON that pane, even though the main window repaints WITHOUT it a
    /// frame before its undock window paints. Pre-R1069 the main-window paint's
    /// `update_focusable_tags` dropped the floated `pane_tag` (absent from the primary's
    /// `collect_focusable_tags`) and the one-shot `focus_request` was already spent →
    /// focus fell to `None` (the user's "have to press it twice" report). R1069 derives
    /// the focusable union from the DECLARED `windows_signal` topology — the `pane-{i}`
    /// window is enumerable via its pure `view_for_window` the moment it is declared,
    /// before it paints — so the requested focus survives. sprag consumes it with NO
    /// code change: `route_key`'s ToggleDock already `focus_request`s the undocked pane;
    /// this pins that the seam now holds end-to-end through the real shell.
    #[test]
    fn undock_keeps_focus_on_the_torn_pane() {
        let mut core = ShellCore::<TerminalViewer>::new();
        let boot = core.compute_paint_scene(WINDOW_W, WINDOW_H);
        core.finalize_frame(boot);
        assert_eq!(
            core.focus().focused(),
            Some(pane_tag(0)),
            "boots focused on pane 0"
        );

        // Undock pane 0 exactly as the Ctrl+Shift+Enter chord does (route_key's
        // ToggleDock): float the pane + request focus on that same pane.
        core.root_owner().run(|| {
            dock::toggle_pane_floating(0);
            pinion_core::focus_request::request(pane_tag(0));
        });

        // Repaint the MAIN window (now without pane 0) + drain the dispatch tail. The
        // declared pane-0 window keeps pane 0 in the focusable union (R1069), so focus
        // follows the torn pane instead of dropping to None.
        let main = core.compute_paint_scene_for_window(dock::MAIN_WINDOW_ID, WINDOW_W, WINDOW_H);
        core.finalize_frame(main);
        assert_eq!(
            core.focus().focused(),
            Some(pane_tag(0)),
            "focus follows the undocked pane (PR-26); not dropped during the window race",
        );
    }

    /// In the 2-window state, ONE physical press double-delivered to both windows
    /// toggles ONCE — pinion R1073's press-owner snapshot (PINION-PR27 / R27.4) gates
    /// the re-delivery to the window the press did NOT begin on (here OS focus stays
    /// on the press owner; the harder close-moves-focus case is the next test). The
    /// keyup (`note_key_state(key, false)`) ends the physical press and clears the
    /// owner — the lifecycle pinion keys the gate on. End-to-end through the real
    /// shell via the `key_press_for_window` / `note_os_focus` seam (R27.2), no GUI.
    /// (The discrete-chord auto-repeat drop is covered by
    /// `input::tests::dock_chord_auto_repeat_is_dropped_scroll_repeats`.)
    #[test]
    fn multiwindow_dock_chord_toggles_once_per_press() {
        let mut core = ShellCore::<TerminalViewer>::new();
        let scene = core.compute_paint_scene(WINDOW_W, WINDOW_H);
        core.finalize_frame(scene);
        let win = dock::pane_window_id(0);
        let count = |core: &mut ShellCore<TerminalViewer>| {
            core.root_owner()
                .run(|| dock::use_windows_topology().get().len())
        };
        // Hold Ctrl+Shift so each "Enter" is the dock chord.
        core.set_modifiers(Modifiers {
            ctrl: true,
            shift: true,
            ..Modifiers::default()
        });

        // Float pane 0 (setup — done DIRECTLY, not via a key press, so no press owner
        // is pinned; the 2nd-action presses below start from a clean rising edge).
        core.root_owner().run(|| dock::toggle_pane_floating(0));
        assert_eq!(count(&mut core), 2, "pane 0 floated");

        // The undock window holds OS focus. ONE physical press double-delivered to both
        // windows (NO keyup between): the press begins on pane-0, so the re-delivery to
        // main is gated by the owner snapshot -> exactly one toggle (dock), no bounce.
        core.note_os_focus(&win, true);
        let to_owner = core.key_press_for_window(&win, "Enter", false);
        let to_other = core.key_press_for_window(dock::MAIN_WINDOW_ID, "Enter", false);
        assert!(
            to_owner,
            "the press's owner window (pane-0) dispatches -> docks pane 0"
        );
        assert!(
            !to_other,
            "the re-delivery to main (not the press owner) is gated"
        );
        assert_eq!(count(&mut core), 1, "exactly one toggle: docked, no bounce");
        core.note_key_state("Enter", false); // keyup ends the physical press
    }

    /// The close-during-dispatch bounce — the live "docks-then-undocks from the 2nd
    /// action" the user hit — FIXED by pinion R1073 (PINION-PR27 / R27.4)'s press-owner
    /// snapshot. The live `[DIAG]` trace proved ONE physical press in the 2-window state
    /// produced TWO chord dispatches (dock then undock, 32ms apart, REPEAT=false): the
    /// press began on the floated pane's window, the FIRST dispatch docked it and CLOSED
    /// that window, OS focus moved to main, and R1071's live-focus gate then ADMITTED the
    /// re-delivery to main (now focused) -> undock = bounce. R1073 pins the owner at the
    /// press's rising edge, so every later delivery is gated against the window the press
    /// BEGAN on (pane-0), not the focus its own dock side-effect moved -> no bounce.
    ///
    /// This is the test my R63 guard SHOULD have been: the difference is driving the
    /// focus move as a CONSEQUENCE of the dock closing the window (the `note_os_focus`
    /// pair below), not hand-pinning it. The keyups (`note_key_state(key, false)`) bound
    /// each physical press — the two deliveries of the 2nd press carry NO keyup between
    /// them (same press), which is exactly what makes them share one owner snapshot.
    #[test]
    fn multiwindow_dock_does_not_bounce_when_close_moves_focus() {
        let mut core = ShellCore::<TerminalViewer>::new();
        let scene = core.compute_paint_scene(WINDOW_W, WINDOW_H);
        core.finalize_frame(scene);
        let win = dock::pane_window_id(0);
        let count = |core: &mut ShellCore<TerminalViewer>| {
            core.root_owner()
                .run(|| dock::use_windows_topology().get().len())
        };
        core.set_modifiers(Modifiers {
            ctrl: true,
            shift: true,
            ..Modifiers::default()
        });

        // Float pane 0 (setup — directly, so no press owner is pinned before the press
        // under test).
        core.root_owner().run(|| dock::toggle_pane_floating(0));
        assert_eq!(count(&mut core), 2, "pane 0 floated");

        // The undock window grabs OS focus.
        core.note_os_focus(&win, true);

        // 2nd action — ONE physical press double-delivered (NO keyup between -> one owner):
        // delivery A -> pane-0 (the press owner, OS-focused): docks pane 0 -> CLOSES it.
        core.key_press_for_window(&win, "Enter", false);
        // Closing pane-0 moves OS focus to main (winit Focused(pane-0,false)+Focused(main,true)).
        core.note_os_focus(&win, false);
        core.note_os_focus(dock::MAIN_WINDOW_ID, true);
        // delivery B -> main (NOW OS-focused, but NOT the press owner): R1073 gates it on
        // the owner snapshot (pane-0), so it does NOT dispatch. (Pre-R1073 it passed
        // R1071's live-focus gate and undocked = the bounce.)
        let to_other = core.key_press_for_window(dock::MAIN_WINDOW_ID, "Enter", false);
        core.note_key_state("Enter", false);

        assert!(
            !to_other,
            "the re-delivery to main is gated by the press-owner snapshot (R1073 R27.4)"
        );
        assert_eq!(
            count(&mut core),
            1,
            "one physical press = one net toggle (docked); no bounce"
        );
    }

    /// End-to-end divider drag through the REAL viewer: setting the boot split's ratio
    /// Signal (the exact write a pointer drag performs via `SplitterExternal`)
    /// re-weights the two panes — the left pane reflows wider, tracking ~0.7. Proves
    /// the read side (`view_dock_surface` -> `view_splitter` -> layout -> R1012 reflow)
    /// sprag wires; the pointer->Signal write side is pinion's `SplitterExternal`
    /// (its own tests) + the live drag smoke.
    #[test]
    fn dragging_a_divider_reweights_the_panes() {
        let mut core = ShellCore::<TerminalViewer>::new();
        // Even split at boot — the two panes are within a cell of each other.
        let even = painted_grid_dims(&core.compute_paint_scene(WINDOW_W, WINDOW_H));
        assert_eq!(even.len(), 2, "two panes, got {even:?}");
        assert!(
            even[0].0.abs_diff(even[1].0) <= 1,
            "boots even, got {even:?}"
        );

        // Drag divider 0 to a 0.7 left-share (the same Signal a pointer drag sets).
        // The boot split between pane 0 and pane 1 carries `boot_split_id(0)`.
        core.root_owner()
            .run(|| split::use_split_ratio(split::boot_split_id(0), 0.5).set(0.7));
        let weighted = painted_grid_dims(&core.compute_paint_scene(WINDOW_W, WINDOW_H));
        assert_eq!(weighted.len(), 2);
        let (left, right) = (weighted[0].0, weighted[1].0);
        assert!(
            left > right,
            "left pane reflowed wider after ratio 0.7, got {weighted:?}"
        );
        let left_frac = f32::from(left) / f32::from(left + right);
        assert!(
            (left_frac - 0.7).abs() < 0.06,
            "the split tracks ~0.7 (got {left_frac:.2}, {weighted:?})",
        );
    }

    /// End-to-end drag wiring (R49): the real viewer paints a scrollbar track
    /// tagged its `scrollbar.{i}` for each pane — the hit target the shell's pointer
    /// router routes a press to, for the `ScrollBarExternal` registered at the same
    /// tag ([`create_extra_externals`] -> [`scrollbar::pane_scrollbar_external`]).
    /// This is the integration claim sprag owns (paint a tagged track + register a
    /// peer at that tag); the pointer->`scroll_to` write is pinion's
    /// `ScrollBarExternal` (its own tests), and the `offset_y`->thumb read is the
    /// [`scrollbar`] unit tests. Mirrors how the splitter trusts pinion for its
    /// write side.
    #[test]
    fn each_pane_paints_a_tagged_scrollbar_track() {
        let mut core = ShellCore::<TerminalViewer>::new();
        let scene = core.compute_paint_scene(WINDOW_W, WINDOW_H);
        assert!(
            scene.contains_tag(pane_scrollbar_tag(0)),
            "pane 0 paints a tagged scrollbar track (the drag hit target)",
        );
        assert!(
            scene.contains_tag(pane_scrollbar_tag(1)),
            "pane 1 paints a tagged scrollbar track",
        );
    }

    /// Undocking every pane leaves the main window empty — the dock topology collapses
    /// to `None` and `view_main` paints a childless surface Container (no panic, no
    /// pane painted).
    #[test]
    fn undock_all_panes_yields_an_empty_main_without_panic() {
        let mut core = ShellCore::<TerminalViewer>::new();
        let n = core.root_owner().run(|| use_terminal().pane_count());
        core.root_owner().run(|| {
            for i in 0..n {
                dock::toggle_pane_floating(i);
            }
        });
        let main = core.compute_paint_scene_for_window(dock::MAIN_WINDOW_ID, WINDOW_W, WINDOW_H);
        for i in 0..n {
            assert!(
                !main.contains_tag(pane_tag(i)),
                "pane {i} is floated, not in main"
            );
        }
    }
}
