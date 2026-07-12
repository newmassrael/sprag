//! `sprag-gui` — the **interactive GPU windowed terminal** (R24-R38).
//!
//! A window that tiles N terminal panes, paints each one's **live** screen (and
//! its scrollback history), and types into the focused one. It is the human
//! observation/interaction path; the north star (an AI reading/driving the
//! terminal as *data*) is the headless `sprag-host` RPC path, which needs none of
//! this. This binding is a faithful pixel projection of the *same* cell data the
//! AI reads — it is a CLIENT of the host ([`HostClient`](sprag_host::HostClient)):
//! by default a wire client of a `sprag-term` host PROCESS ([`wire::WireHost`], the
//! topology-B default), or an in-process [`Host`](sprag_host::Host) under
//! `SPRAG_GUI_HOST=inprocess`. It reads each pane's cell DATA through that client,
//! assembles the node itself ([`sprag_host::pane_view_scene_from_cells`], the
//! Screen-free wire seam), arranges the panes (the interactive [`split`] layout —
//! reactive ratios the headless host has no use for), and routes keystrokes as
//! client SENDs to the host
//! ([`HostClient::send_key`](sprag_host::HostClient::send_key) — the same key->PTY
//! encoder the AI's `scene/invoke` drives, §2 #2).
//!
//! ## Multi-pane (R36): N tiled panes, one focused
//!
//! [`pane_count`](terminal::pane_count) panes (default 2, `SPRAG_GUI_PANES=<n>`,
//! capped at [`MAX_PANES`](terminal::MAX_PANES)) are spawned at boot and tiled
//! left-to-right. Each pane has a single identity [`pane_tag`]
//! that is its focus tag, its
//! paint-scene Container tag (the pinion R1012
//! [`use_pane_viewport_size`](pinion_core::use_pane_viewport_size) rect target +
//! framework focus ring + click-focus anchor), and its per-pane reflow Effect tag
//! — one string so input / focus / measure / paint can never address different
//! panes. Keyboard focusability is **scene-derived** (pinion R1020 §5.39): the
//! pane's paint Container is marked `with_focusable(true)` in
//! [`sprag_host::pane_view_scene_from_cells`] and the shell collects the Tab order each frame
//! via [`Scene::collect_focusable_tags`](pinion_core::Scene::collect_focusable_tags)
//! — there is no binding-side `focusable_tags()` list. Topology B (R115c): the GUI
//! serves NO per-pane INPUT external — [`WidgetCore::apply_key`] maps the focused
//! `pane_tag` to its tile index and SENDS to the host
//! ([`HostClient::send_key`](sprag_host::HostClient::send_key)); the model scene
//! holds only the GUI's own interaction externals (scroll / split / dock), the
//! primary ([`WidgetCore::create_external`]) being pane 0's scrollbar.
//! `Ctrl+PageUp/Down` cycles focus; the
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
//! ## Dock split-tree layout (R60, R72): drag to resize; float keeps the leaf (placeholder)
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
//! This retires the former flat row/grid model (and the `SPRAG_GUI_LAYOUT` env). The dock
//! model is SELECTABLE (R77, `SPRAG_GUI_DOCK_MODE`): in the DEFAULT `collapse` mode floating
//! a pane REMOVES its leaf so siblings reclaim the space (tmux fill); in the opt-in
//! `placeholder` mode the leaf STAYS and the view paints a placeholder holding its slot.
//! Either way [`split`] / [`dock`] own the orthogonal authorities and the docked set is
//! DERIVED (see [`dock::DockMode`]). The tree is also restructured by a reorganize gesture
//! (drag-to-dock + zone-redock), which mints a fresh Split id,
//! so the splitter set is a runtime-mutable projection of the topology —
//! [`create_extra_externals`](TerminalViewer)
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
//!   Signal ([`use_dock_topology`](split::use_dock_topology)) of ALL panes' leaves (static
//!   under float/dock; the docked set is DERIVED via [`docked_pane_indices`](split::docked_pane_indices)),
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
//!   no double measurement. `pane_view_scene_from_cells` pins that size on each
//!   node so the painted advance equals `cell_w` (R1002).
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
//! Topology B (R115c): keystrokes do NOT flow through a GUI-owned pane External —
//! they are client SENDs to the host. Each pane is a focusable tab stop (its paint
//! Container is `with_focusable(true)`, R1020 scene-derived focus — above), so
//! [`WidgetCore::apply_key`] takes the **focused** `pane_tag`, maps it to its tile
//! index, and calls [`HostClient::send_key`](sprag_host::HostClient::send_key)`(i,
//! key, mods)`. The host encodes the W3C key + modifiers to PTY bytes with the
//! sprag-owned encoder ([`sprag_input`](https://docs.rs/sprag-input)) (R2.6) — the
//! *same* SSOT the RPC `scene/invoke` path uses, so the human keyboard and an AI
//! peer encode identically. Returning `true` swallows Escape/Tab from the shell's
//! quit/traverse defaults so a full-screen TUI (vim) receives them.
//! `Ctrl+PageUp/Down` is reserved to cycle focus between tiles (a pinion
//! `focus_request`, not the PTY).
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
mod rpc;
mod scrollbar;
mod slotview;
mod split;
mod terminal;
mod view;
mod wire;

use pinion_a11y::AccessNode;
use pinion_core::command::Command;
use pinion_core::event::{LINE_HEIGHT_PX, WheelDelta};
use pinion_core::external::{DropPoint, External, IntrospectValue};
use pinion_core::intent::Intent;
use pinion_core::reactive::{Owner, Signal};
use pinion_core::widget_core::ExtraExternal;
use pinion_core::{CompositionEvent, Frame, Modifiers, Scene, WidgetCore};
use pinion_shell::{
    ShellConfig, SizeStrategy, WidgetView, WindowChromeStyle, WindowPolicy, WindowSpec,
    vello_renderer_impl,
};
use pinion_widget_paint::dock::{
    DockPanelExternal, DropResolution, TEAR_OFF_EVENT, TEAR_OFF_FOLLOW_EVENT,
    TEAR_OFF_REDOCK_AT_EVENT, TEAR_OFF_REDOCK_EVENT, WINDOW_MOVE_EVENT, dock_drop_preview_overlay,
    dock_outer_preview_overlay, dock_redock_preview_tint, resolve_drop,
};
use pinion_widget_paint::splitter::SplitterExternal;
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

/// The tile slot whose scrollbar is the shell's PRIMARY External (registered by
/// `create_external`, named by `tag`); every OTHER occupied slot's scrollbar is an extra
/// (`create_extra_externals`). One name for the coupling between those sites. Slot 0 is
/// the primary; when it is EMPTY (a zero-pane attach, or a boot hole) the primary is a
/// harmless dangling external — nothing paints it, since `view` / a11y render only
/// occupied slots. Re-homing the primary onto a non-pane sentinel tag (so no pane slot is
/// load-bearing) is future work.
const PRIMARY_SCROLLBAR_SLOT: usize = 0;

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

    /// The primary External is **pane 0**'s draggable scrollbar (tagged
    /// [`Self::tag`] == [`pane_scrollbar_tag`]`(0)`); panes 1.. scrollbars + the
    /// splitters + dock-panel handles are the [`Self::create_extra_externals`]
    /// extras.
    ///
    /// Topology B (R115c): the GUI serves NO pane-INPUT external on its own socket —
    /// keyboard / IME are client SENDs to the host ([`Self::apply_key`] ->
    /// `HostClient::send_key` / `send_text`), and an AI peer drives the HOST's own
    /// pane input surface directly. So the model scene holds only the GUI's own
    /// interaction externals (scroll / split / dock); the retired `SpragPaneExternal`
    /// + `pane_handle` (a live `SessionHandle` cannot cross the wire) are gone.
    fn create_external() -> Box<dyn External> {
        crate::scrollbar::pane_scrollbar_external(PRIMARY_SCROLLBAR_SLOT).handle
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
        // Topology B (R115c): NO per-pane INPUT externals in the GUI model scene —
        // input is a client SEND to the host (keyboard via `HostClient::send_key`,
        // an AI peer via the host's own pane surface). The externals assembled below
        // are the GUI's OWN interactions: splitters, scrollbars, dock-panel handles.
        let mut externals: Vec<ExtraExternal> = Vec::new();
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
        // The occupied slots, snapshot ONCE (one mirror lock + alloc) and reused by both
        // the scrollbar-extras and the dock-panel externals below.
        let occupied = terminal.slots.occupied_slots();
        // Slot 0's scrollbar is the PRIMARY external ([`Self::create_external`]); the
        // OTHER occupied slots are extras, so the drag-scrollbar set stays complete
        // without a duplicate registration at `pane_scrollbar_tag(0)`. (Slot 0 is always
        // occupied at boot; a Round 2b close of slot 0 would need the primary re-homed.)
        externals.extend(
            occupied
                .iter()
                .copied()
                .filter(|&i| i != PRIMARY_SCROLLBAR_SLOT)
                .map(scrollbar::pane_scrollbar_external),
        );
        // Drag-to-dock / tear-off (pinion R1081/R1084/R1094 §5.51, P2/PR-31): one R742
        // `DockPanelExternal` per pane, registered at the panel ROOT tag
        // (`split::panel_id(i)` = the `view_dock_panel` root the `view_dock_surface`
        // walker emits — NOT the `#header` composite; the R51.42 dispatch splits at `#`
        // and routes the header press to the root-tagged external). Each shares the ONE
        // reorganizer (`split::use_dock_reorganizer`, total over the `Option` topology
        // per PR-29.1) so a pointer drop reorganizes `split::use_dock_topology`, and the
        // ONE drop-preview (`split::use_drop_preview`) the dragged panel writes for the
        // target panel's zone highlight (read in `view::view_main`). A drop onto another
        // panel's zone docks (split / swap); an escape-drop (cursor left every panel)
        // emits the live `tear_off_follow` per move (→ `dock::float_pane_at`) and, when
        // the cursor returns over a dock zone before release, `tear_off_redock`
        // (→ `dock::redock_pane`); the no-cursor fallback fires the legacy `tear_off`
        // toggle (→ `dock::toggle_pane_floating`, the same the Ctrl+Shift+Enter key path
        // drives, P3 unified).
        //
        // Registered for ALL panes, NOT just the docked ones — the symmetric model
        // pinion's reference consumer (hello-dock-panels: "one DockPanelExternal per
        // panel ... docked-→floating and floating-→docked share the same external") uses,
        // and the fix for the R69 live gap. `external_set_is_dynamic` re-runs this factory
        // on every dock/undock; a tear-off is ONE continuous press captured by the source
        // pane's external, and the very first escaped move floats that pane (a topology
        // change → reconcile → this factory re-runs MID-DRAG). Filtering to the docked set
        // would DROP the in-progress drag's external on that reconcile, killing the gesture
        // after one frame (follow stops; redock never reached). Registering every pane
        // keeps the source external alive across the float (R689 preserve-by-tag preserves
        // its drag state), so the continuous gesture survives: escape → follow, return over
        // a dock zone → redock, all resolved by the captured main-window router. And a
        // SETTLED floating pane's external is now LIVE: its `pane-{i}` window paints a
        // `view_dock_panel` header at this same `panel_id(i)` tag (R74), so that window's
        // per-window `InputRouter` routes a press on the header to this external — the user
        // can re-grab a settled floating window and drag it back. pinion auto-resolves that
        // cross-window drop and fires `tear_off_redock_at` (the reducer's PR-33 arm relocates
        // the leaf to the dropped zone). Each window's router hit-tests its OWN painted
        // scene, so the one shared external (the model-scene set is a superset of any one
        // window's painted tags) services the docked header, the floating header, and the
        // in-flight drag without collision.
        let reorganizer = split::use_dock_reorganizer();
        let drop_preview = split::use_drop_preview();
        externals.extend(occupied.iter().copied().map(|i| {
            ExtraExternal::new(
                split::panel_id(i),
                Box::new(
                    DockPanelExternal::new(split::panel_id(i))
                        .with_reorganizer(Rc::clone(&reorganizer))
                        .with_drop_preview(Rc::clone(&drop_preview))
                        // (pinion R1172 / R91) LOCK the sole docked pane's tear-off at the
                        // SOURCE: `movable=false` makes `begin_drag` return `None`, so a pane
                        // that cannot float (the last docked one, tmux semantics) would start
                        // NO drag — no misleading chip/preview. The factory re-runs per
                        // float/dock (R70) and computes the live flag (diag `externals_rebuilt`
                        // shows [false,true] once a pane floats). BLOCKED LIVE on PINION-PR42:
                        // `reconcile_externals` early-returns on an unchanged tag set (sprag's
                        // tags are constant), DISCARDING the rebuilt external, so this dynamic
                        // `movable` is create-time-only and not applied after boot. Kept as the
                        // correct forward seam (sprag change 0 when pinion re-applies per-panel
                        // flags); until then the reducer/float gate keeps behavior correct while
                        // the chip/preview still (wrongly) shows.
                        .with_movable(dock::pane_is_movable(i))
                        // (pinion R1116 / PINION-PR38 ②) Declare this pane's own floating
                        // window id, so a header drag INSIDE that window is a borderless
                        // title-bar WINDOW MOVE (grab-offset delta → the `WINDOW_MOVE`
                        // reducer arm), not a dock tear-off. The id is the same
                        // `pane_window_id` SSOT the float/redock paths use, so a settled
                        // floating window can finally be re-dragged by its header (②).
                        .with_floating_window(dock::pane_window_id(i)),
                ),
            )
        }));
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
        _scene: &mut Scene,
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
        // The paint scene is not read: input is a client SEND to the host (route_key
        // -> Host::send_key), not a mutation of the GUI's own scene (topology B).
        route_key(focused, key, modifiers, false)
    }

    /// Repeat-aware key dispatch (pinion R1071 / PINION-PR27) — the variant the live
    /// shell drives, carrying the platform `KeyEvent.repeat` flag. Both this and
    /// [`Self::apply_key`] are thin delegates to [`route_key`], which OWNS the repeat
    /// policy: a held DISCRETE window chord (dock-toggle / focus-cycle) acts once per
    /// press, not on every OS auto-repeat — without this a held `Ctrl+Shift+Enter`
    /// dock-then-undocked in the multi-window state. Scrollback chords and PTY keys
    /// still repeat (continuous).
    fn apply_key_repeat(
        _scene: &mut Scene,
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
        route_key(focused, key, modifiers, repeat)
    }

    /// Route committed IME text to the focused pane's PTY — delegates to
    /// [`route_composition`] (the focus gate + the focused pane's literal
    /// `invoke("text", ...)` wire + its preedit overlay).
    fn apply_composition(
        _scene: &mut Scene,
        focused: Option<&str>,
        event: &CompositionEvent,
    ) -> bool {
        route_composition(focused, event)
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
        for i in terminal.slots.occupied_slots() {
            let scrollback_len = terminal.slots.pane_scroll_facts(i).scrollback_len;
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
        let Some(i) = use_terminal()
            .slots
            .occupied_slots()
            .into_iter()
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

    /// The primary External's tag ([`Self::create_external`] is pane 0's scrollbar):
    /// the model-scene primary is registered at `V::tag()`, so the shell routes a
    /// press on pane 0's painted scrollbar track to it. Pane focus is paint-derived
    /// (R1020 `collect_focusable_tags` over the `with_focusable` pane Containers), so
    /// it does NOT depend on this tag.
    fn tag() -> &'static str {
        pane_scrollbar_tag(PRIMARY_SCROLLBAR_SLOT)
    }

    fn read_state(_scene: &Scene) {}

    /// Reducer — pinion routes external-drained intents through `WidgetCore::update`
    /// (R51.168 §5.23). sprag consumes the dock-panel **tear-off** family a
    /// [`DockPanelExternal`] emits while a header is dragged, mapping the panel id
    /// back to its tile ([`split::pane_index_of_panel`]) and driving the [`dock`]
    /// window/topology SSOT — so the drag tear-off and the `Ctrl+Shift+Enter` key
    /// undock share one authority. Four intents (pinion R1094 / PINION-PR31, R1100 / PR-33):
    ///
    /// * [`TEAR_OFF_FOLLOW_EVENT`] (`Json{panel,x,y}`, the main-window-logical
    ///   cursor) — fired on every escaped drag move + the escape-release: ensure the
    ///   pane floats and track the cursor via [`dock::float_pane_at`] (non-toggling,
    ///   so the per-move re-emit only repositions). This is the live cursor-following
    ///   tear-off (browser-tab style).
    /// * [`TEAR_OFF_REDOCK_EVENT`] (`Text(panel_id)`) — fired on a SAME-window
    ///   escape-then-return and on a snap-back restore: de-float the pane via
    ///   [`dock::redock_pane`] (idempotent; it re-tiles at its home leaf, which never left).
    /// * [`TEAR_OFF_REDOCK_AT_EVENT`] (`Json{panel,window,target,x_rel,y_rel}`) —
    ///   CROSS-window: a settled floating window dragged back over the MAIN window's dock
    ///   zone (pinion R1163b composes `over_window` across windows so the drop resolves on
    ///   main). Classify the drop with `resolve_drop` (the SSOT that retired
    ///   `apply_zone_redock`, R1128) and apply the outcome — `Dock`/`OuterDock` relocate
    ///   the source leaf to the zone then de-float; `Float` (a dead-zone) leaves it
    ///   floating. This is the zone-honoring drag-back redock ("끌어서 dock").
    /// * [`TEAR_OFF_EVENT`] (`Text(panel_id)`) — the legacy release toggle pinion
    ///   fires ONLY when no drag cursor was forwarded (pre-R1093 / unit paths): the
    ///   discrete [`dock::toggle_pane_floating`], the same the key path drives.
    ///
    /// (Drag-to-DOCK — a drop onto a panel zone — needs no reducer: the shared
    /// [`DockReorganizer`](pinion_widget_paint::dock::DockReorganizer) mutates the
    /// topology directly.) Runs in the root Owner scope; emits no [`Command`]s.
    ///
    /// Each intent arrives **scoped** as `{external_tag}.{event}` — pinion prefixes an
    /// external's emitted intent with that external's tag before routing to the reducer
    /// (the editor's `viewport_btn.click` carries the same scoping). The
    /// [`DockPanelExternal`] is registered at tag `terminal-{i}`, so e.g. its bare
    /// [`TEAR_OFF_EVENT`] arrives as `terminal-{i}.tear_off`; matching the bare event
    /// would never fire (the live bug R68 corrected).
    ///
    /// The scoped tag is split ONCE on the last `.`: the prefix is the **trusted**
    /// routing key (pinion guarantees it is the emitting external's registered tag),
    /// and `{event}` selects the arm. Routing by the tag (not the payload's `panel`
    /// field) is the correct SSOT — and splitting once avoids the per-call `format!`
    /// allocation a candidate-rebuild-per-arm would cost on the `follow` hot path
    /// (the follow fires per drag-move, hundreds of times in one gesture).
    fn update(_state: (), intent: &Intent) -> Vec<Command> {
        let Some((panel, event)) = intent.tag_str().rsplit_once('.') else {
            return Vec::new();
        };
        let Some(i) = split::pane_index_of_panel(panel) else {
            return Vec::new();
        };
        match (event, &intent.payload) {
            // Live cursor-following tear-off (R1094, browser-tab style): the JSON
            // carries the main-window-logical cursor; the panel is the trusted tag.
            // Non-toggling float-or-reposition. Fires per escaped drag-move + release.
            (TEAR_OFF_FOLLOW_EVENT, IntrospectValue::Json(v)) => {
                if let (Some(x), Some(y)) = (
                    v.get("x").and_then(serde_json::Value::as_f64),
                    v.get("y").and_then(serde_json::Value::as_f64),
                ) {
                    // `source_window` (pinion R1107): the window the cursor was measured
                    // in — so the desktop conversion uses the RIGHT origin. Dragging a
                    // SETTLED floating window's header reports its own `pane-{i}` frame,
                    // not main; without threading this the reposition added the main
                    // origin and the window jumped/froze (the "undocked window won't drag"
                    // bug). `None` → main, as before.
                    let source_window = v.get("source_window").and_then(serde_json::Value::as_str);
                    dock::float_pane_at(i, source_window, (x, y));
                }
            }
            // Same-window redock / restore (R1094): a same-window escape-then-return or
            // a snap-back. Window-only de-float; the leaf never left, so the pane re-tiles
            // at its home slot. Idempotent.
            (TEAR_OFF_REDOCK_EVENT, IntrospectValue::Text(_)) => dock::redock_pane(i),
            // CROSS-WINDOW redock (R1100/R1163b/PR-33): a SETTLED floating window dragged
            // back over the MAIN window's dock zone — the user's "끌어서 dock". pinion now
            // composes `over_window` across windows (R1148→R1163b), auto-resolves the
            // cross-window drop, and fires this with the target window + drop zone (`target`
            // = the zone's paint tag, `x_rel`/`y_rel` normalised over it). We CLASSIFY the
            // drop through `resolve_drop` (the SSOT that retired `apply_zone_redock`) and
            // relocate the source leaf to the resolved zone (`Dock`/`OuterDock`) then drop
            // the float window; a dead-zone `Float` leaves the pane floating (R1163b
            // preview==result). Only the main window bears the dock topology; a non-main
            // window gate above keeps a non-slot target out.
            // Order matters: relocate-leaf-while-still-floating THEN drop-window, so the
            // redock paints ONCE at the final slot (reversing it would flash the pane at its
            // old slot first). The two `signal.set`s (topology, then windows) are distinct
            // reactive txns but paint-atomic here — they run inside one `update` tick with no
            // reconcile between; a future refactor splitting them across ticks would expose
            // the transient (leaf relocated, still floating).
            (TEAR_OFF_REDOCK_AT_EVENT, IntrospectValue::Json(v)) => {
                if v.get("window").and_then(serde_json::Value::as_str) == Some(dock::MAIN_WINDOW_ID)
                    && let (Some(target), Some(x_rel), Some(y_rel)) = (
                        v.get("target").and_then(serde_json::Value::as_str),
                        v.get("x_rel").and_then(serde_json::Value::as_f64),
                        v.get("y_rel").and_then(serde_json::Value::as_f64),
                    )
                {
                    // R86 (pinion R1128/R1163b): redock through the `resolve_drop` SSOT
                    // (the retired `apply_zone_redock` is gone). `resolve_drop` classifies
                    // the drop point — folding the `{panel}_placeholder` strip internally
                    // (no manual strip needed) — into a `DropResolution`, then we apply the
                    // matching reorganizer primitive and de-float. `tabbing()` is false
                    // (split.rs `with_tabbing(false)`) so a centre drop never tabifies. A
                    // dead-zone `Float` leaves the pane FLOATING (R1163b "preview==result":
                    // a blank preview means it won't dock), so `redock_pane` is no longer
                    // unconditional. Mirrors the editor's `redock_cross_window`.
                    let reorganizer = split::use_dock_reorganizer();
                    let point = DropPoint {
                        tag: target.to_string(),
                        x_rel: x_rel as f32,
                        y_rel: y_rel as f32,
                    };
                    let resolution = resolve_drop(
                        Some(&point),
                        &split::panel_id(i),
                        |t| reorganizer.is_panel(t),
                        reorganizer.tabbing(),
                    );
                    // Cheap `&'static str` variant label (no eager alloc when the diag is off).
                    crate::diag::redock_resolution(
                        i,
                        target,
                        x_rel,
                        y_rel,
                        match resolution {
                            DropResolution::Dock { .. } => "Dock",
                            DropResolution::OuterDock { .. } => "OuterDock",
                            DropResolution::SnapBack { .. } => "SnapBack",
                            DropResolution::Float => "Float",
                        },
                    );
                    match resolution {
                        DropResolution::Dock { target, zone } => {
                            let _ = reorganizer.dock_panel_at_resolved_zone(
                                &split::panel_id(i),
                                &target,
                                zone,
                            );
                            dock::redock_pane(i);
                        }
                        DropResolution::OuterDock { edge } => {
                            let _ = reorganizer.dock_panel_outer(&split::panel_id(i), edge);
                            dock::redock_pane(i);
                        }
                        DropResolution::SnapBack { .. } => dock::redock_pane(i),
                        DropResolution::Float => {}
                    }
                }
            }
            // Borderless title-bar WINDOW MOVE (R1116/R1118 / PINION-PR38 ②): dragging a
            // SETTLED floating window by its own header. pinion sends a grab-relative
            // delta (distinct from `tear_off_follow`'s absolute cursor — an honest wire:
            // a move carries a displacement). Relocate the window by the delta.
            (WINDOW_MOVE_EVENT, IntrospectValue::Json(v)) => {
                if let (Some(dx), Some(dy)) = (
                    v.get("dx").and_then(serde_json::Value::as_f64),
                    v.get("dy").and_then(serde_json::Value::as_f64),
                ) {
                    dock::move_floating_window(i, (dx, dy));
                }
            }
            // Legacy release toggle (cursor-less escape / unit paths) — the same
            // discrete dock/undock the `Ctrl+Shift+Enter` key path drives.
            (TEAR_OFF_EVENT, IntrospectValue::Text(_)) => dock::toggle_pane_floating(i),
            _ => {}
        }
        Vec::new()
    }

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
    // it is painted: `sprag_host::pane_view_scene_from_cells` marks the pane Container
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

    /// Per-window chrome + resize policy (pinion R1190 `WindowPolicy`, which folded
    /// the former `window_chrome` + `window_resizable` hooks into one value-type read
    /// once by the shell). EVERY window is borderless (`with_decorations(false)` — the
    /// main window in [`dock::use_windows_topology`], the floating pane windows in
    /// `dock::undock_window_spec`), so the OS draws no title bar; this hook is the sole
    /// chrome authority.
    ///
    /// - **MAIN**: `chrome = Some(default)` (min + max/restore + close strip, routed to
    ///   winit actions by the shell) + the 8-direction resize border (chrome present ⇒
    ///   `resizable` derives on).
    /// - **FLOATING pane** (`pane-{i}`): `chrome = None` + `resizable = Some(true)` (R95,
    ///   pinion R1171/R1186/R1187 — PR-43, controls-in-header). Its dock-panel HEADER is
    ///   its title bar — pane identity is the tab label and the header is already the
    ///   window-drag surface (R1116), so a chrome strip stacked over it duplicated both.
    ///   The header hosts the window controls instead ([`view::view_for_window`] passes
    ///   the shell's `WINDOW_CHROME_*_TAG`s to `view_window_controls`), so
    ///   `try_chrome_press` routes them exactly as chrome buttons; the floater draws ONE
    ///   strip. `resizable = Some(true)` decouples the resize border from chrome (R1186):
    ///   the chrome-less floater keeps a BELOW-TITLEBAR border (sides + bottom — the
    ///   header owns the top edge, so no north region can shadow its close button).
    /// - **Unknown id**: default policy (no chrome, no forced resize).
    ///
    /// A close press routes through
    /// [`window_close_requested`](Self::window_close_requested), the binding seam R1121
    /// deferred: the MAIN window returns `false` there (close = app-quit, the right
    /// primary-close), a FLOATING pane window returns `true` (it docks the pane back
    /// instead of quitting — a floater is a live pane, R86).
    fn window_policy(window_id: &str) -> WindowPolicy {
        if window_id == dock::MAIN_WINDOW_ID {
            WindowPolicy::new().with_chrome(WindowChromeStyle::default())
        } else if dock::pane_window_index(window_id).is_some() {
            WindowPolicy::new().with_resizable(true)
        } else {
            WindowPolicy::new()
        }
    }

    /// Window-close seam (pinion R1170): the shell calls this on a close request (the
    /// chrome close button or the OS close path). A FLOATING pane window's close docks the
    /// pane back into the main window and returns `true` (handled — the shell does NOT
    /// exit), so closing a torn-off terminal returns it home instead of killing the app.
    /// The MAIN window returns `false` → the default app-exit (closing the primary window
    /// quits sprag). This is the proper resolution of the R84/R85 "a floater's X must not
    /// quit the terminal" constraint, now that pinion delivers the seam.
    fn window_close_requested(window_id: &str) -> bool {
        if let Some(i) = dock::pane_window_index(window_id) {
            dock::redock_pane(i);
            true
        } else {
            false
        }
    }

    /// Cross-window dock drop-preview (pinion R1163b): while a floating pane is dragged
    /// over the main window's dock, the shell calls this so the binding paints a preview
    /// overlay showing WHERE the panel will land. We classify the drop through the SAME
    /// `resolve_drop` SSOT the release applies (the `TEAR_OFF_REDOCK_AT_EVENT` arm), so the
    /// preview == the result by construction: an inner `Dock` zone previews the half-split
    /// band, an `OuterDock` previews the full-span perimeter band, a `Float`/`SnapBack`
    /// (dead zone) previews nothing. This is the affordance the user saw in pinion's editor
    /// but not here — sprag had left the hook at its `None` default. Mirrors the editor's
    /// `dock_drop_preview`.
    fn dock_drop_preview(
        source_panel: &str,
        target_tag: &str,
        panel_rect: pinion_core::scene::Rect,
        x_rel: f32,
        y_rel: f32,
    ) -> Option<Scene> {
        let reorganizer = split::use_dock_reorganizer();
        let point = DropPoint {
            tag: target_tag.to_string(),
            x_rel,
            y_rel,
        };
        let tint = dock_redock_preview_tint(&pinion_core::theme::Theme::default());
        match resolve_drop(
            Some(&point),
            source_panel,
            |t| reorganizer.is_panel(t),
            reorganizer.tabbing(),
        ) {
            DropResolution::OuterDock { edge } => {
                dock_outer_preview_overlay(panel_rect, edge, tint)
            }
            DropResolution::Dock { zone, .. } => dock_drop_preview_overlay(panel_rect, zone, tint),
            DropResolution::Float | DropResolution::SnapBack { .. } => None,
        }
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
    // Production logging (memory: production-logging-tracing): install sprag's
    // leveled `tracing` subscriber FIRST — env-filtered by `SPRAG_LOG` (default
    // `warn`) — so it is authoritative over pinion's `run()` fallback
    // (`init_tracing`, idempotent first-wins `try_init`). All first-party
    // diagnostics (`diag.rs`) flow through it; dial verbosity per subsystem, e.g.
    // `SPRAG_LOG=sprag_gui::dock=trace`.
    diag::install();
    tracing::info!(target: "sprag_gui", panes = terminal::pane_count(), "sprag-gui starting");
    // PR-47: mount the always-on RPC socket through pinion's winit-free
    // `on_rpc_ingress` seam (rpc::mount). The socket and pinion's built-in
    // stdin reader share one ingress -> one dispatch core, so this is purely
    // additive: stdin/FIFO RPC still works, plus a fixed-path socket that is
    // always there regardless of how the process was launched. Transport
    // policy (path, boot state, SIGUSR1 runtime on/off) is sprag's; see
    // [`rpc`].
    pinion_shell::run_with_config::<TerminalViewer>(ShellConfig::new().on_rpc_ingress(rpc::mount));
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
        // The main window is now borderless with the app's client chrome (R85), so the
        // shell insets the painted content below a `height_px` chrome strip; below that, a
        // docked pane sits under a 28px dock-panel header (R60). The pane content height is
        // the window minus BOTH strips — derive the expected rows through the same winsize
        // SSOT against the twice-reduced height.
        let chrome_px = WindowChromeStyle::default().height_px;
        let header_px = pinion_widget_paint::dock::DockPanelStyle::m3_default("x").header_height_px;
        let content_rows =
            crate::terminal::grid_dims((WINDOW_W, WINDOW_H - chrome_px - header_px), metric).1;
        assert!(
            content_rows < full_rows,
            "the chrome strip + dock header subtract rows"
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
    /// The R72 floating window paints a `view_dock_panel` HEADER (the drag source a
    /// settled floating window is re-grabbed from, enabling cross-window redock) above a
    /// pane that reflows its WIDTH to the window. Float pane 0, then paint its `pane-0`
    /// window at two widths: the header composite tag is present, and cols track the
    /// window width (fewer than the full-width derivation — the scrollbar gutter — and
    /// the wider window B reflows to MORE cols). HEIGHT reflow is the separate
    /// `undock_window_reflows_its_height_below_boot_content` test (PINION-PR35 delivered R78).
    #[test]
    fn undock_window_paints_a_header_and_reflows_width() {
        let mut core = ShellCore::<TerminalViewer>::new();
        let metric = core.root_owner().run(|| use_terminal().metric);
        let win = dock::pane_window_id(0);

        // Boot, then FLOAT pane 0 (the real undock side effect — R64/R65: drive the same
        // state as live; main paints pane 0's placeholder, only `pane-0` publishes it).
        let _ = core.compute_paint_scene(WINDOW_W, WINDOW_H);
        core.root_owner().run(|| dock::toggle_pane_floating(0));

        // Two paints per size (measure → publish → reflow → repaint).
        let (wa, ha) = (600u32, 400u32);
        let (wb, hb) = (900u32, 720u32);
        let _ = core.compute_paint_scene_for_window(&win, wa, ha);
        let scene_a = core.compute_paint_scene_for_window(&win, wa, ha);
        let dims_a = painted_grid_dims(&scene_a);
        let _ = core.compute_paint_scene_for_window(&win, wb, hb);
        let dims_b = painted_grid_dims(&core.compute_paint_scene_for_window(&win, wb, hb));

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

        // The header composite tag — the drag source for cross-window redock (Stage C).
        assert!(
            scene_a.contains_tag(format!("{}#header", split::panel_id(0)).as_str()),
            "the floating window paints a draggable view_dock_panel header",
        );

        // Cols track the window width minus the scrollbar gutter; the wider window B
        // reflows to MORE cols — the width genuinely reflowed.
        let full_a = crate::terminal::grid_dims((wa, ha), metric);
        let full_b = crate::terminal::grid_dims((wb, hb), metric);
        assert!(
            dims_a[0].0 > 0 && dims_a[0].0 < full_a.0,
            "A cols sane: {dims_a:?}"
        );
        assert!(
            dims_b[0].0 > 0 && dims_b[0].0 < full_b.0,
            "B cols sane: {dims_b:?}"
        );
        assert!(
            dims_b[0].0 > dims_a[0].0,
            "a wider undock window reflows the pane to more cols: A={dims_a:?} B={dims_b:?}",
        );
    }

    /// A floating window's pane reflows its HEIGHT to a window SMALLER than the pane's
    /// boot content. This was blocked on PINION-PR35 (pinion's `view_dock_panel`
    /// `content_wrapper` was `flex_grow(1.0)` with no `min_size:0`, so the grid's boot-row
    /// min-content pinned the panel — rows stuck at boot dims); pinion R1109 gave the
    /// content wrapper the R1086 idiom (`flex_basis:0 + flex_grow:1 + min-height:0`), so
    /// sprag's `fill_definite_shrinkable` (content-side `min_size.height:0`) now composes
    /// with it and the pane shrinks to the panel's distributed height. (Un-ignored at the
    /// R78 bump; was the `..._is_blocked_on_pinion_pr35` repro.)
    #[test]
    fn undock_window_reflows_its_height_below_boot_content() {
        let mut core = ShellCore::<TerminalViewer>::new();
        let metric = core.root_owner().run(|| use_terminal().metric);
        let win = dock::pane_window_id(0);
        let _ = core.compute_paint_scene(WINDOW_W, WINDOW_H);
        core.root_owner().run(|| dock::toggle_pane_floating(0));

        let (wa, ha) = (600u32, 400u32);
        let (wb, hb) = (900u32, 720u32);
        let _ = core.compute_paint_scene_for_window(&win, wa, ha);
        let dims_a = painted_grid_dims(&core.compute_paint_scene_for_window(&win, wa, ha));
        let _ = core.compute_paint_scene_for_window(&win, wb, hb);
        let dims_b = painted_grid_dims(&core.compute_paint_scene_for_window(&win, wb, hb));

        let full_a = crate::terminal::grid_dims((wa, ha), metric);
        assert!(
            dims_a[0].1 > 0 && dims_a[0].1 < full_a.1,
            "A rows sane under the header: dims_a={dims_a:?} full_a={full_a:?}",
        );
        assert!(
            dims_b[0].1 > dims_a[0].1,
            "a taller undock window reflows the pane to more rows: A={dims_a:?} B={dims_b:?}",
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

    /// Undocking every pane in PLACEHOLDER mode: the topology is UNCHANGED (every leaf
    /// survives), so `view_main` paints a placeholder per leaf — no pane grid, no panic.
    /// (Collapse mode empties the tree to `None` and paints nothing — see
    /// `collapse_float_removes_the_leaf_and_dock_restores_index_order` in split.rs.)
    #[test]
    fn undock_all_panes_paints_placeholders_not_grids() {
        let mut core = ShellCore::<TerminalViewer>::new();
        core.root_owner()
            .run(|| dock::set_dock_mode(dock::DockMode::Placeholder));
        let n = core
            .root_owner()
            .run(|| use_terminal().slots.occupied_slots().len());
        core.root_owner().run(|| {
            for i in 0..n {
                dock::toggle_pane_floating(i);
            }
        });
        // The topology still holds every leaf (no collapse).
        assert_eq!(
            core.root_owner().run(|| split::use_dock_topology()
                .get()
                .map(|t| t.panel_ids().len())),
            Some(n),
            "every leaf survives the float (placeholder model)"
        );
        let main = core.compute_paint_scene_for_window(dock::MAIN_WINDOW_ID, WINDOW_W, WINDOW_H);
        for i in 0..n {
            assert!(
                !main.contains_tag(pane_tag(i)),
                "pane {i}'s grid is floated, not in main"
            );
            assert!(
                main.contains_tag(format!("{}_placeholder", split::panel_id(i)).as_str()),
                "pane {i}'s leaf paints a placeholder holding its slot"
            );
        }
    }

    /// Drag tear-off (P2 / pinion R1081 §5.51): the `tear_off` intent a
    /// [`DockPanelExternal`] emits on an escape-drop (header dragged out past every
    /// drop target), routed by the REAL shell through `WidgetCore::update`, floats the
    /// named pane — the SAME `dock::toggle_pane_floating` window/topology mutation the
    /// Ctrl+Shift+Enter key path drives (P2 drag tear-off + P3 key undock = one SSOT).
    /// Driven via `ShellCore::dispatch_intent` (the reducer arc), so it pins the WIRING
    /// (shell routes the external's intent to sprag's reducer), not just the reducer body.
    #[test]
    fn tear_off_intent_floats_the_named_pane() {
        use std::borrow::Cow;
        let mut core = ShellCore::<TerminalViewer>::new();
        let scene = core.compute_paint_scene(WINDOW_W, WINDOW_H);
        core.finalize_frame(scene);
        assert_eq!(
            core.root_owner()
                .run(|| dock::use_windows_topology().get().len()),
            1,
            "boots main-only (both panes docked)"
        );

        // The escape-drop intent as the REDUCER actually receives it: pinion scopes the
        // external's emitted TEAR_OFF_EVENT with the external's tag, so it arrives as
        // `terminal-1.tear_off` (NOT the bare event). Constructing the bare tag here is
        // exactly what let the first version of this guard pass while the live drag
        // failed (R67 lesson: a synthetic input that doesn't match the live wire masks
        // the bug). Build the scoped tag the shell delivers.
        let intent = Intent {
            tag: Cow::Owned(format!("{}.{}", split::panel_id(1), TEAR_OFF_EVENT)),
            payload: IntrospectValue::Text(split::panel_id(1)),
        };
        core.dispatch_intent(&intent);

        assert_eq!(
            core.root_owner()
                .run(|| dock::use_windows_topology().get().len()),
            2,
            "tear_off floated pane 1 into its own window"
        );
        assert_eq!(
            core.root_owner().run(split::docked_pane_indices),
            vec![0],
            "pane 1 floats — only pane 0 docked"
        );
        // Default (collapse) mode: floating pane 1 REMOVED its leaf, so the tree shrank
        // and pane 0 reclaimed the space.
        assert_eq!(
            core.root_owner().run(|| split::use_dock_topology()
                .get()
                .map(|t| t.panel_ids().len())),
            Some(1),
            "collapse mode: pane 1's leaf is removed (the sibling reclaims the slot)"
        );
    }

    /// Live cursor-following tear-off (R1094 / PINION-PR31): the `tear_off_follow`
    /// intent a [`DockPanelExternal`] emits on every escaped drag move, routed by the
    /// REAL shell through `WidgetCore::update`, floats the named pane AND positions
    /// its window under the cursor — then a second move REPOSITIONS the same window
    /// rather than flipping it back (non-toggling, the R1071-R1078 lesson on the sprag
    /// side). Built with the LIVE scoped tag + `Json{panel,x,y}` payload the shell
    /// actually delivers (the R67/R68 lesson: a synthetic input that doesn't match the
    /// live wire masks the bug), and asserts the rendered topology AND the declared
    /// window position, not just that an intent was consumed.
    #[test]
    fn tear_off_follow_intent_floats_then_repositions_the_pane() {
        use std::borrow::Cow;
        let mut core = ShellCore::<TerminalViewer>::new();
        let scene = core.compute_paint_scene(WINDOW_W, WINDOW_H);
        core.finalize_frame(scene);

        // The position read back from the topology for pane 1's floating window.
        let pane1_pos = |core: &ShellCore<TerminalViewer>| {
            core.root_owner().run(|| {
                dock::use_windows_topology()
                    .get()
                    .iter()
                    .find(|w| w.id == dock::pane_window_id(1))
                    .and_then(|w| w.position)
            })
        };
        let follow = |x: f64, y: f64| Intent {
            tag: Cow::Owned(format!("{}.{}", split::panel_id(1), TEAR_OFF_FOLLOW_EVENT)),
            payload: IntrospectValue::Json(serde_json::json!({
                "panel": split::panel_id(1), "x": x, "y": y,
            })),
        };

        // First escaped move: pane 1 floats and its window opens under the cursor.
        // The boot main window is WM-placed (position None), so the desktop
        // conversion falls back to the (0, 0) origin → the cursor IS the position.
        core.dispatch_intent(&follow(300.0, 200.0));
        assert_eq!(
            core.root_owner()
                .run(|| dock::use_windows_topology().get().len()),
            2,
            "the first escaped follow floated pane 1 into its own window"
        );
        assert_eq!(
            core.root_owner().run(split::docked_pane_indices),
            vec![0],
            "pane 1 leaves the docked set on float (default collapse mode: its leaf is removed; \
             the outcome [0] is mode-robust — placeholder would filter the surviving leaf)"
        );
        assert_eq!(
            pane1_pos(&core),
            Some((300, 200)),
            "the floating window opened at the desktop-converted cursor"
        );

        // A second move repositions the SAME window — non-toggling: it does not
        // dock the pane back and does not spawn a second window.
        core.dispatch_intent(&follow(350.0, 260.0));
        assert_eq!(
            core.root_owner()
                .run(|| dock::use_windows_topology().get().len()),
            2,
            "the follow re-emit repositions, never duplicates or flips back"
        );
        assert_eq!(
            pane1_pos(&core),
            Some((350, 260)),
            "the window tracked the cursor to its new position"
        );
    }

    /// R78 fix: dragging a SETTLED floating window's header repositions it by ITS OWN
    /// window origin (the `source_window` pinion R1107 threads), not main's. The bug
    /// ("undocked window won't drag") hardcoded main's origin, so a floater's local cursor
    /// added to the main origin gave a bogus desktop point and the window jumped/froze.
    /// Driven through the REAL reducer with the scoped tag plus the exact `source_window`
    /// Json the live wire carries (R67/R68: the synthetic input must match the live wire).
    #[test]
    fn dragging_a_settled_floating_window_uses_its_own_origin() {
        use std::borrow::Cow;
        let mut core = ShellCore::<TerminalViewer>::new();
        let scene = core.compute_paint_scene(WINDOW_W, WINDOW_H);
        core.finalize_frame(scene);

        // Give main a KNOWN origin (not WM-placed None) so the source-window choice is
        // observable in the resulting position.
        core.root_owner().run(|| {
            let sig = dock::use_windows_topology();
            let mut w = sig.get();
            if let Some(m) = w.iter_mut().find(|s| s.id.as_ref() == dock::MAIN_WINDOW_ID) {
                m.position = Some((1000, 500));
            }
            sig.set(w);
        });

        let pane1_win = dock::pane_window_id(1);
        let pane1_pos = |core: &ShellCore<TerminalViewer>| {
            core.root_owner().run(|| {
                dock::use_windows_topology()
                    .get()
                    .iter()
                    .find(|w| w.id == dock::pane_window_id(1))
                    .and_then(|w| w.position)
            })
        };
        let follow = |sw: &str, x: f64, y: f64| Intent {
            tag: Cow::Owned(format!("{}.{}", split::panel_id(1), TEAR_OFF_FOLLOW_EVENT)),
            payload: IntrospectValue::Json(serde_json::json!({
                "panel": split::panel_id(1), "source_window": sw, "x": x, "y": y,
            })),
        };

        // Float pane 1 with the cursor in MAIN's frame → main origin (1000,500) + (300,200).
        core.dispatch_intent(&follow(dock::MAIN_WINDOW_ID, 300.0, 200.0));
        assert_eq!(
            pane1_pos(&core),
            Some((1300, 700)),
            "floated at main-origin + main-frame cursor"
        );

        // Now drag the SETTLED floating window by its OWN header (source_window = pane-1).
        // Its origin is (1300,700); a local cursor (10,5) → (1310,705). The bug would add
        // main's origin (1000,500)+(10,5)=(1010,505), jumping the window back near main.
        core.dispatch_intent(&follow(&pane1_win, 10.0, 5.0));
        assert_eq!(
            pane1_pos(&core),
            Some((1310, 705)),
            "a settled floating window repositions by ITS OWN origin, not main's (R78)"
        );
    }

    /// R82 (PINION-PR38 ②): a borderless floating window's title-bar drag is a grab-
    /// relative WINDOW_MOVE — pinion sends a `{panel,dx,dy}` delta (distinct from
    /// tear_off_follow's absolute cursor) and the reducer moves the window by it. This is
    /// the seam that makes "drag a SETTLED floating window by its header" finally work.
    /// Driven through the real reducer with the scoped tag + exact delta Json.
    #[test]
    fn window_move_intent_relocates_the_floating_window_by_delta() {
        use std::borrow::Cow;
        let mut core = ShellCore::<TerminalViewer>::new();
        let scene = core.compute_paint_scene(WINDOW_W, WINDOW_H);
        core.finalize_frame(scene);

        let pane1_pos = |core: &ShellCore<TerminalViewer>| {
            core.root_owner().run(|| {
                dock::use_windows_topology()
                    .get()
                    .iter()
                    .find(|w| w.id == dock::pane_window_id(1))
                    .and_then(|w| w.position)
            })
        };

        // Float pane 1 at a known position via a follow (source_window=main, origin None→0).
        core.dispatch_intent(&Intent {
            tag: Cow::Owned(format!("{}.{}", split::panel_id(1), TEAR_OFF_FOLLOW_EVENT)),
            payload: IntrospectValue::Json(serde_json::json!({
                "panel": split::panel_id(1), "source_window": dock::MAIN_WINDOW_ID,
                "x": 400.0, "y": 300.0,
            })),
        });
        assert_eq!(pane1_pos(&core), Some((400, 300)), "floated at the cursor");

        // Title-bar drag: a WINDOW_MOVE delta (+50, -20) shifts the window by the delta.
        let window_move = |dx: f64, dy: f64| Intent {
            tag: Cow::Owned(format!("{}.{}", split::panel_id(1), WINDOW_MOVE_EVENT)),
            payload: IntrospectValue::Json(serde_json::json!({
                "panel": split::panel_id(1), "dx": dx, "dy": dy,
            })),
        };
        core.dispatch_intent(&window_move(50.0, -20.0));
        assert_eq!(
            pane1_pos(&core),
            Some((450, 280)),
            "the title-bar drag moved the window by its grab-relative delta"
        );
        // A second delta accumulates (grab-offset follow).
        core.dispatch_intent(&window_move(-100.0, 5.0));
        assert_eq!(
            pane1_pos(&core),
            Some((350, 285)),
            "a second delta accumulates on the window position"
        );
    }

    /// Live redock / restore (R1094 / PINION-PR31): after a live-follow floated pane
    /// 1, the `tear_off_redock` intent (emitted on a same-window escape-return or a
    /// snap-back) docks it back through `WidgetCore::update` — the window is dropped and
    /// the pane's membership restores. Runs in the DEFAULT collapse mode, where the leaf was
    /// REMOVED on float and `redock_pane`→`dock_pane` re-inserts it index-relative (in
    /// placeholder mode the leaf would never have left; the restored membership is the same
    /// either way). Then a SECOND redock (the pane already docked) is a harmless no-op. Live
    /// scoped tag + `Text` payload, asserting the membership both authorities expose.
    #[test]
    fn tear_off_redock_intent_docks_the_floated_pane() {
        use std::borrow::Cow;
        let mut core = ShellCore::<TerminalViewer>::new();
        let scene = core.compute_paint_scene(WINDOW_W, WINDOW_H);
        core.finalize_frame(scene);

        // Float pane 1 via a live-follow move (the gesture redock undoes).
        core.dispatch_intent(&Intent {
            tag: Cow::Owned(format!("{}.{}", split::panel_id(1), TEAR_OFF_FOLLOW_EVENT)),
            payload: IntrospectValue::Json(serde_json::json!({
                "panel": split::panel_id(1), "x": 120.0, "y": 90.0,
            })),
        });
        assert_eq!(
            core.root_owner()
                .run(|| dock::use_windows_topology().get().len()),
            2,
            "pane 1 floated (precondition)"
        );

        let redock = Intent {
            tag: Cow::Owned(format!("{}.{}", split::panel_id(1), TEAR_OFF_REDOCK_EVENT)),
            payload: IntrospectValue::Text(split::panel_id(1)),
        };
        core.dispatch_intent(&redock);
        assert_eq!(
            core.root_owner()
                .run(|| dock::use_windows_topology().get().len()),
            1,
            "redock dropped pane 1's window"
        );
        assert_eq!(
            core.root_owner().run(split::docked_pane_indices),
            vec![0, 1],
            "redock restored pane 1's membership (collapse: its leaf re-inserted index-relative)"
        );

        // Idempotent: a redock of an already-docked pane changes nothing.
        core.dispatch_intent(&redock);
        assert_eq!(
            core.root_owner()
                .run(|| dock::use_windows_topology().get().len()),
            1,
            "a redock of a docked pane is a no-op"
        );
        assert_eq!(
            core.root_owner().run(split::docked_pane_indices),
            vec![0, 1],
            "the docked set is unchanged"
        );
    }

    /// Cross-window zone-honoring redock — the REDUCER ARM (R1100 / PINION-PR33). This
    /// guards the half sprag owns: GIVEN the `terminal-0.tear_off_redock_at` intent (the
    /// one pinion's R1148→R1163b `over_window` composition is designed to fire when a
    /// floater is dropped over another window's dock zone), the reducer relocates the leaf
    /// via the `resolve_drop` SSOT and drops the float window. Float pane 0 (its leaf
    /// SURVIVES — placeholder mode, set below — which is what lets the reorganizer relocate
    /// it), dispatch the exact live-scoped intent + Json (over terminal-1's RIGHT edge), and
    /// assert the pane re-tiles AT the drop zone (`[terminal-1, terminal-0]`), the reverse of
    /// boot home. R67/R68: the synthetic input matches the live wire (scoped tag + exact
    /// payload) and the assertion is the real topology relocation, not "an intent ran".
    ///
    /// SCOPE (honest, avoiding the R69/R83 overclaim): this does NOT prove the LIVE gesture
    /// "drag a settled floater onto main → it redocks" end to end — whether the shell FIRES
    /// this intent with `window:"main"` on that gesture is pinion's over_window composition
    /// (delivered R1163b; observed firing cross-window in the R91 diag, but this exact
    /// floater→main direction is not yet clean-live-verified here). This guard covers the
    /// consumption; the live firing is a separate check owed a real mouse.
    #[test]
    fn tear_off_redock_at_relocates_the_leaf_to_the_dropped_zone() {
        use std::borrow::Cow;
        let mut core = ShellCore::<TerminalViewer>::new();
        // Placeholder mode (the surviving leaf is what resolve_drop relocates). Collapse
        // ALSO zone-honors now via R1173 — see the sibling
        // `tear_off_redock_at_in_collapse_honors_the_zone_cleanly`; this test isolates the
        // placeholder path where the leaf never left.
        core.root_owner()
            .run(|| dock::set_dock_mode(dock::DockMode::Placeholder));
        let scene = core.compute_paint_scene(WINDOW_W, WINDOW_H);
        core.finalize_frame(scene);

        // Boot order is [terminal-0, terminal-1].
        assert_eq!(
            core.root_owner().run(|| split::use_dock_topology()
                .get()
                .expect("boots docked")
                .panel_ids()
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<String>>()),
            vec![split::panel_id(0), split::panel_id(1)],
        );

        // Float pane 0 (window appears; its leaf STAYS in the tree, painting a placeholder).
        core.root_owner().run(|| dock::toggle_pane_floating(0));
        assert_eq!(
            core.root_owner()
                .run(|| dock::use_windows_topology().get().len()),
            2,
            "pane 0 floated (precondition)"
        );

        // The cross-window drop the shell resolves + fires: terminal-0 released over
        // terminal-1's RIGHT edge (x_rel high) in the MAIN window.
        core.dispatch_intent(&Intent {
            tag: Cow::Owned(format!(
                "{}.{}",
                split::panel_id(0),
                TEAR_OFF_REDOCK_AT_EVENT
            )),
            payload: IntrospectValue::Json(serde_json::json!({
                "panel": split::panel_id(0),
                "window": dock::MAIN_WINDOW_ID,
                "target": split::panel_id(1),
                "x_rel": 0.85,
                "y_rel": 0.5,
            })),
        });

        // The float window is gone (de-floated) ...
        assert_eq!(
            core.root_owner()
                .run(|| dock::use_windows_topology().get().len()),
            1,
            "the floating window dropped on redock"
        );
        // ... and pane 0 re-tiled AT THE ZONE — to the RIGHT of pane 1, the reverse of its
        // boot home order. In placeholder mode the surviving leaf is what resolve_drop
        // relocates (collapse reaches the same zone via R1173's total insert).
        assert_eq!(
            core.root_owner().run(|| split::use_dock_topology()
                .get()
                .expect("docked")
                .panel_ids()
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<String>>()),
            vec![split::panel_id(1), split::panel_id(0)],
            "redock-at-zone placed pane 0 to the right of pane 1, not its index home"
        );
        assert_eq!(
            core.root_owner().run(split::docked_pane_indices),
            vec![1, 0],
            "both panes docked again, in the dropped visual order"
        );
    }

    /// R92: the SAME cross-window `tear_off_redock_at`, but in the DEFAULT collapse mode —
    /// the path most users hit, and the one the placeholder guard above does NOT cover.
    /// Writing this guard corrected a stale belief: pinion R1173's `dock_panel_at_resolved_zone`
    /// is TOTAL over an absent source leaf (it materializes the collapse-removed leaf directly
    /// at the resolved zone), so collapse now ALSO honors the drop zone — landing pane 0 at
    /// the dropped position `[terminal-1, terminal-0]`, NOT its index home. (The PINION-PR34
    /// fix effectively delivered inside R1173; the earlier "collapse lands index-relative"
    /// docs were stale.) Pins: de-floats (one window), zone-honored order, a CLEAN tree (no
    /// duplicate leaf despite `redock_pane`'s collapse leaf-op running after the zone insert),
    /// no panic.
    #[test]
    fn tear_off_redock_at_in_collapse_honors_the_zone_cleanly() {
        use std::borrow::Cow;
        let mut core = ShellCore::<TerminalViewer>::new();
        core.root_owner()
            .run(|| dock::set_dock_mode(dock::DockMode::Collapse));
        let scene = core.compute_paint_scene(WINDOW_W, WINDOW_H);
        core.finalize_frame(scene);

        core.root_owner().run(|| dock::toggle_pane_floating(0));
        // The cross-window drop over terminal-1's RIGHT edge (same intent as the placeholder
        // guard). Here the source leaf was removed on float; R1173 re-materializes it at the zone.
        core.dispatch_intent(&Intent {
            tag: Cow::Owned(format!(
                "{}.{}",
                split::panel_id(0),
                TEAR_OFF_REDOCK_AT_EVENT
            )),
            payload: IntrospectValue::Json(serde_json::json!({
                "panel": split::panel_id(0),
                "window": dock::MAIN_WINDOW_ID,
                "target": split::panel_id(1),
                "x_rel": 0.85,
                "y_rel": 0.5,
            })),
        });

        assert_eq!(
            core.root_owner()
                .run(|| dock::use_windows_topology().get().len()),
            1,
            "collapse redock still drops the floating window"
        );
        let docked = core.root_owner().run(split::docked_pane_indices);
        assert_eq!(
            docked,
            vec![1, 0],
            "collapse ALSO honors the drop zone (R1173): pane 0 lands right of pane 1, not index-home"
        );
        assert_eq!(
            docked.len(),
            2,
            "clean tree — exactly two docked leaves, no duplicate"
        );
    }

    /// R70: `create_extra_externals` registers a `DockPanelExternal` for EVERY pane,
    /// docked or floating — NOT just the docked set. This is the sprag-side condition for
    /// the R69 live gap's fix: a tear-off is one continuous captured press, and the first
    /// escaped move floats the source pane (a topology change that re-runs this factory
    /// mid-drag via `external_set_is_dynamic`); had the factory filtered to the docked
    /// set, the reconcile would DROP the in-flight drag's external (follow dies after one
    /// frame, redock unreachable). pinion's R689 preserve-by-tag then keeps the surviving
    /// tag's handle (with its drag state) — that half lives in pinion and can't be
    /// synthesized headlessly (an RPC batch delivers every move before any reconcile —
    /// exactly what masked this in R69), so this guards the part sprag owns: the panel
    /// external SET equals all panes in BOTH the all-docked and one-floated states (a set
    /// equality, so a regression that drops ANY pane — not just pane 1 — is caught).
    #[test]
    fn dock_panel_external_registered_for_every_pane_across_a_float() {
        use std::collections::BTreeSet;
        let core = ShellCore::<TerminalViewer>::new();
        core.root_owner().run(|| {
            // The panel-id externals only (filter out the pane/scrollbar/splitter tags).
            let panel_externals = || {
                <TerminalViewer as WidgetCore>::create_extra_externals()
                    .into_iter()
                    .map(|e| e.tag.into_owned())
                    .filter(|t| split::pane_index_of_panel(t).is_some())
                    .collect::<BTreeSet<String>>()
            };
            let expected: BTreeSet<String> = use_terminal()
                .slots
                .occupied_slots()
                .into_iter()
                .map(split::panel_id)
                .collect();
            assert_eq!(
                panel_externals(),
                expected,
                "boot: one dock-panel external per pane"
            );
            // Float pane 1, then re-run the factory exactly as the reconcile does.
            dock::toggle_pane_floating(1);
            assert_eq!(
                panel_externals(),
                expected,
                "every pane KEEPS its dock-panel external across a float (none dropped)"
            );
        });
    }

    /// R95 chrome split (pinion R1171/R1186 — PR-43), read through the R1190
    /// `WindowPolicy` value-type: the MAIN window keeps the app's client chrome
    /// (min/max/close strip); a FLOATER is chrome-less — its dock HEADER is its title bar
    /// (controls-in-header, `view_window_controls` in the header-trailing slot) — yet
    /// stays resizable via the decoupled `resizable` field.
    /// The close ACTION differs via `window_close_requested` (R86): main → `false`
    /// (app-quit, the right primary-close); a floater → docks its pane back + `true`
    /// (handled, no exit).
    #[test]
    fn window_chrome_main_only_floater_header_owns_controls_close_docks_back() {
        // Main: client chrome with min + max + close.
        let c = <TerminalViewer as WidgetView>::window_policy(dock::MAIN_WINDOW_ID)
            .chrome
            .expect("the main window gets client chrome (borderless)");
        assert!(
            c.show_minimize && c.show_maximize && c.show_close,
            "main chrome has min + max + close"
        );
        // A floater: NO chrome strip (its dock header is the title bar), but the
        // resize border survives chrome removal via the R1186 decoupled gate.
        assert!(
            <TerminalViewer as WidgetView>::window_policy(&dock::pane_window_id(0))
                .chrome
                .is_none(),
            "a floating pane window is chrome-less (controls-in-header)"
        );
        assert_eq!(
            <TerminalViewer as WidgetView>::window_policy(&dock::pane_window_id(0)).resizable,
            Some(true),
            "the chrome-less floater stays resizable (decoupled from chrome)"
        );
        assert_eq!(
            <TerminalViewer as WidgetView>::window_policy(dock::MAIN_WINDOW_ID).resizable,
            None,
            "main derives resizability from its chrome (pre-R1186 behaviour)"
        );
        // A non-pane / unknown window id: no chrome.
        assert!(
            <TerminalViewer as WidgetView>::window_policy("nope")
                .chrome
                .is_none(),
            "an unknown window id gets no client chrome"
        );

        // The close ACTION: main quits (false), a floater docks its pane back (true).
        let mut core = ShellCore::<TerminalViewer>::new();
        let scene = core.compute_paint_scene(WINDOW_W, WINDOW_H);
        core.finalize_frame(scene);
        core.root_owner().run(|| {
            assert!(
                !<TerminalViewer as WidgetView>::window_close_requested(dock::MAIN_WINDOW_ID),
                "main close is unhandled here → the shell's app-exit default"
            );
            // Float pane 1, then its window's close should dock it back (handled = true).
            dock::toggle_pane_floating(1);
            assert!(
                dock::pane_window_index(&dock::pane_window_id(1)).is_some()
                    && super::dock::use_windows_topology()
                        .get()
                        .iter()
                        .any(|w| w.id == dock::pane_window_id(1)),
                "pane 1 is floating (its window exists)"
            );
            assert!(
                <TerminalViewer as WidgetView>::window_close_requested(&dock::pane_window_id(1)),
                "a floater's close is HANDLED (docks back, no app-exit)"
            );
            assert!(
                !super::dock::use_windows_topology()
                    .get()
                    .iter()
                    .any(|w| w.id == dock::pane_window_id(1)),
                "the close docked pane 1 back — its floating window is gone"
            );
        });
    }

    /// R95 controls-in-header, PAINT-level (guard the rendered layout, not the data
    /// path — R68 lesson): a floater's window paints its window CONTROLS inside its
    /// dock-panel header — the three shell routing tags (`WINDOW_CHROME_*_TAG`) are
    /// present in its VIEW scene (they come from `view_window_controls` in the
    /// header-trailing slot, not from a shell-injected chrome strip; the floater's
    /// `window_policy().chrome` is `None`, asserted above). `try_chrome_press` routes a press
    /// on these tags exactly as it would a chrome button's, so min / max / close work
    /// with the strip gone.
    #[test]
    fn floater_view_hosts_window_controls_in_its_dock_header() {
        use pinion_shell::{
            WINDOW_CHROME_CLOSE_TAG, WINDOW_CHROME_MAXIMIZE_TAG, WINDOW_CHROME_MINIMIZE_TAG,
        };
        let mut core = ShellCore::<TerminalViewer>::new();
        let boot = core.compute_paint_scene(WINDOW_W, WINDOW_H);
        core.finalize_frame(boot);
        core.root_owner().run(|| dock::toggle_pane_floating(1));
        let floater = core.compute_paint_scene_for_window(&dock::pane_window_id(1), 480, 360);
        assert!(
            floater.contains_tag(&format!("{}#header", split::panel_id(1))),
            "the floater paints its dock-panel header (its sole title bar)"
        );
        for tag in [
            WINDOW_CHROME_MINIMIZE_TAG,
            WINDOW_CHROME_MAXIMIZE_TAG,
            WINDOW_CHROME_CLOSE_TAG,
        ] {
            assert!(
                floater.contains_tag(tag),
                "the floater view carries the {tag} window control in its header"
            );
        }
    }

    /// Total node count of a `Scene` (recursively). The faithful way to assert a preview
    /// actually PAINTED (R68 lesson: guard the rendered layout, not the data path) when the
    /// overlay carries no distinguishing tag — an appended overlay grows the count.
    fn scene_node_count(scene: &Scene) -> usize {
        1 + match scene {
            Scene::Container(c) => c.children.iter().map(scene_node_count).sum(),
            _ => 0,
        }
    }

    /// R87 same-window OUTER drop-preview, HEADLESS (refuting "preview is only live-
    /// verifiable" — the render is a pure function of the preview state, testable without a
    /// drag gesture). Setting `use_drop_preview` to an `OUTER_DOCK_ZONE_TAG` target makes
    /// `view_main` append the full-span outer overlay (R87's view addition). pinion's
    /// same-window `dock_outer_zone_highlight` carries no distinguishing tag (unlike the
    /// cross-window hook overlay), so the append is asserted by the node-count GROWING —
    /// the overlay is a real appended node. (The per-panel zone highlight is the pre-
    /// existing R1080/R1082 mechanism; R87's cross-window hook is covered by the sibling
    /// `dock_drop_preview_hook_*` guard.) The mid-gesture DRAG that SETS this state is the
    /// RPC-vs-live part; the preview RENDER — what the user sees — is proven here.
    #[test]
    fn outer_drop_preview_state_appends_the_overlay_headlessly() {
        use pinion_widget_paint::dock::{DockDropPreview, DockDropZone};
        // Render view_for_window("main") in the SAME owner the preview signal is set in
        // (ShellCore::new booted the topology there — see reorganizer_swaps). The `Frame`
        // is unused by view_main, so a default is fine.
        let core = ShellCore::<TerminalViewer>::new();
        core.root_owner().run(|| {
            let frame = Frame::new();
            let render = || view::view_for_window(dock::MAIN_WINDOW_ID, (), &frame);

            // Baseline: no preview.
            split::use_drop_preview().set(None);
            let base = scene_node_count(&render());

            // Outer preview: a drag into the window's outer band (OUTER_DOCK_ZONE_TAG) →
            // view_main appends the full-span outer overlay, so the paint grows.
            split::use_drop_preview().set(Some(DockDropPreview {
                source: split::panel_id(1),
                target: pinion_core::external::OUTER_DOCK_ZONE_TAG.to_string(),
                zone: DockDropZone::Right,
            }));
            assert!(
                scene_node_count(&render()) > base,
                "an OUTER_DOCK_ZONE_TAG preview appends the full-span outer overlay to the paint \
                 (view_main R1167) — it grows the scene vs the no-preview baseline"
            );
        });
    }

    /// R87 cross-window preview hook, HEADLESS: `dock_drop_preview` is a pure function of
    /// the same `resolve_drop` SSOT the redock arm applies (preview == result). Covers ALL
    /// four `DropResolution` arms: `Dock` (edge over a panel) and `OuterDock` (the
    /// window-outer band) each preview an overlay `Some`; `SnapBack` (own slot) and `Float`
    /// (a dead / non-panel target) preview `None`. Verifies the cross-window affordance
    /// without a live cross-window drag.
    #[test]
    fn dock_drop_preview_hook_previews_for_dock_zones_none_for_dead_zones() {
        // The hook resolves through `use_dock_reorganizer` (needs a booted topology owner).
        let core = ShellCore::<TerminalViewer>::new();
        core.root_owner().run(|| {
            let rect = pinion_core::scene::Rect::new(0, 0, 800, 600);
            let preview = |target: &str, x: f32, y: f32| {
                <TerminalViewer as WidgetView>::dock_drop_preview(
                    &split::panel_id(1),
                    target,
                    rect,
                    x,
                    y,
                )
            };
            // Dock: terminal-1 over terminal-0's RIGHT edge (x_rel high) → an overlay.
            assert!(
                preview(&split::panel_id(0), 0.9, 0.5).is_some(),
                "an edge drop over another panel previews an overlay (Dock)"
            );
            // OuterDock: the reserved window-outer band tag → a full-span overlay.
            assert!(
                preview(pinion_core::external::OUTER_DOCK_ZONE_TAG, 0.02, 0.5).is_some(),
                "an outer-band drop previews the full-span overlay (OuterDock)"
            );
            // SnapBack: the source's OWN slot (source == target) → no overlay.
            assert!(
                preview(&split::panel_id(1), 0.5, 0.5).is_none(),
                "a drop on the panel's own slot (SnapBack) previews nothing"
            );
            // Float: a non-panel / dead target → no overlay.
            assert!(
                preview("nope", 0.5, 0.5).is_none(),
                "a drop on a non-panel dead target (Float) previews nothing"
            );
        });
    }

    /// Drag-to-dock reorganize (P2 / pinion R1081 + PR-29.1): the shared
    /// [`DockReorganizer`](pinion_widget_paint::dock::DockReorganizer) — wired to sprag's
    /// `Signal<Option<DockTopology>>` SSOT (PR-29.1 made it total over `Option`) — applies
    /// a reorganize gesture and mutates the dock-tree. A Center-zone drop (the Swap the
    /// panel external builds) reorders the two boot panes; the topology stays `Some`
    /// (reorganize preserves leaf count, never empties). Pins that sprag's
    /// `Option<DockTopology>` SSOT drives the coordinator DIRECTLY — no second signal, no
    /// mirror.
    #[test]
    fn reorganizer_swaps_docked_panes_on_sprags_option_topology() {
        use pinion_widget_paint::dock::DockReorganizeIntent;
        let core = ShellCore::<TerminalViewer>::new();
        core.root_owner().run(|| {
            assert_eq!(
                split::docked_pane_indices(),
                vec![0, 1],
                "boots [0, 1] docked"
            );
            split::use_dock_reorganizer()
                .apply_intent(&DockReorganizeIntent::Swap {
                    source: split::panel_id(0),
                    target: split::panel_id(1),
                })
                .expect("swap applies on the Some topology");
            assert_eq!(
                split::docked_pane_indices(),
                vec![1, 0],
                "the reorganizer reordered sprag's Option-topology dock-tree",
            );
        });
    }

    /// Verifies **PINION-PR30 is consumed** (delivered pinion R1086, which added
    /// `LayoutStyle.min_size` and set the `view_splitter` flex child's main-axis `min` to
    /// `0`; this was an `#[ignore]` reproduce-the-gap guard pre-bump, the R64 pattern): a
    /// drag-to-dock reorganize that produces a **vertical** (stacked) split keeps BOTH
    /// panels within the window. sprag's terminal grid carries a large intrinsic main-axis
    /// size (rows × cell); before R1086 pinion's `view_splitter` distributed children by
    /// `flex_basis:0` with `flex_grow:ratio` but could not zero the flex child's main-axis
    /// `min`, so taffy's `min:auto` (the content height) clamped the panel to its content
    /// size and the second panel overflowed below the window (the user's "second panel
    /// disappeared"). R1086's main-axis `min:0` lets the panel shrink to its ratio share.
    /// Boot is always horizontal, so this only exercises via a Top/Bottom drop. (Headless-
    /// faithful: drives the SAME shared reorganizer the live `DockPanelExternal::drag_release`
    /// does, then paints through the real shell.)
    #[test]
    fn vertical_reorganize_keeps_both_panels_onscreen() {
        use pinion_widget_paint::dock::{DockReorganizeIntent, DockSplitPosition};
        use pinion_widget_paint::splitter::SplitterOrientation;
        let mut core = ShellCore::<TerminalViewer>::new();
        let boot = core.compute_paint_scene(WINDOW_W, WINDOW_H);
        core.finalize_frame(boot);
        // Drag-dock pane 0 onto pane 1's BOTTOM edge → a vertical (stacked) split.
        core.root_owner().run(|| {
            split::use_dock_reorganizer()
                .apply_intent(&DockReorganizeIntent::SplitInsert {
                    source: split::panel_id(0),
                    target: split::panel_id(1),
                    orientation: SplitterOrientation::Vertical,
                    position: DockSplitPosition::Second,
                })
                .expect("vertical split-insert applies on the Some topology");
        });
        let scene = core.compute_paint_scene_for_window(dock::MAIN_WINDOW_ID, WINDOW_W, WINDOW_H);
        let r0 = scene
            .rect_for_tag_absolute(&split::panel_id(0))
            .expect("pane 0 panel painted");
        let r1 = scene
            .rect_for_tag_absolute(&split::panel_id(1))
            .expect("pane 1 panel painted");
        assert!(
            r0.y + r0.h <= WINDOW_H && r1.y + r1.h <= WINDOW_H,
            "both panels must fit the {WINDOW_H}px window after a vertical reorganize; \
             got terminal-0=(y={},h={}) terminal-1=(y={},h={}) — PR-30 (no min:0)",
            r0.y,
            r0.h,
            r1.y,
            r1.h,
        );
    }
}
