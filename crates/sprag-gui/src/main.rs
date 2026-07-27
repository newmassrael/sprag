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
//! holds only the GUI's own interaction externals (scroll / split / dock); sprag
//! declares NO primary ([`WidgetCore::primary_surface`] is `None`, R127 / PINION-PR51),
//! so every pane scrollbar is a symmetric extra and no slot is load-bearing (R122).
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
//! [`view_dock_surface_chrome`](pinion_widget_paint::dock::view_dock_surface_chrome), which wraps
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
//! This retires the former flat row/grid model (and the `SPRAG_GUI_LAYOUT` env). Floating a
//! pane asks the HOST to take it out of the tiling, so its leaf collapses and the siblings reclaim the
//! space. [`split`] / [`dock`] own the two orthogonal authorities -- the host's arrangement
//! (projected) and this client's OS windows -- and the docked set is DERIVED from the
//! projected tree. The tree is also restructured by a reorganize gesture
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

// `sprag-gui` is a binary crate: `cargo doc` always builds it with private items, so
// the crate-root architecture map above deliberately links to the bin's own internal
// modules/items (`terminal`, `split`, `dock`, `route_key`, ...) — a navigable map is
// the point. The `private_intra_doc_links` lint guards LIBRARIES, whose public-API
// docs publish WITHOUT private items and would ship broken links; a bin has no such
// published surface, so the lint is a structural false positive here. Allowed at the
// crate root rather than mangling 40+ correct links into plain code spans.
#![allow(rustdoc::private_intra_doc_links)]

mod a11y;
mod attention;
mod clipboard_osc;
mod command;
mod confirm;
mod ctxmenu;
mod diag;
mod dock;
mod find;
mod focus_report;
mod hyperlink;
mod input;
mod palette;
mod reflow;
mod rpc;
mod scrollbar;
mod selection;
mod slotview;
mod split;
mod stabs;
mod terminal;
mod view;
mod wire;
mod wtabs;

use pinion_a11y::AccessNode;
use pinion_core::command::Command;
use pinion_core::event::{LINE_HEIGHT_PX, WheelDelta};
use pinion_core::external::{DropPoint, External, IntrospectValue};
use pinion_core::intent::Intent;
use pinion_core::reactive::{Owner, Signal};
use pinion_core::widget_core::{ExtraExternal, PrimarySurface};
use pinion_core::{CompositionEvent, Frame, Modifiers, Scene, WidgetCore};
use pinion_shell::{
    ShellConfig, SizeStrategy, WidgetView, WindowChromeStyle, WindowPolicy, WindowSpec,
    vello_renderer_impl,
};
use pinion_widget_paint::dock::{
    DockPanelExternal, DockReorganizeExternal, DropResolution, TEAR_OFF_EVENT,
    TEAR_OFF_FOLLOW_EVENT, TEAR_OFF_REDOCK_AT_EVENT, TEAR_OFF_REDOCK_EVENT, WINDOW_MOVE_EVENT,
    dock_drop_preview_overlay, dock_outer_preview_overlay, dock_redock_preview_tint, resolve_drop,
};
use pinion_widget_paint::splitter::SplitterExternal;
use std::rc::Rc;

use crate::input::{route_composition, route_key};
use crate::reflow::install_reflow;
use crate::terminal::{focused_pane, pane_index_of, pane_scrollbar_tag, pane_tag, use_terminal};

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

/// Reset ALL of display slot `slot`'s per-slot reactive state when the slot FREES (Round 2b
/// live delta) — the ONE owner of "what freeing a slot entails", so a future per-slot
/// `Owner::cache` addition has a single, compiler-reachable place to be reset (the R124 fix
/// for the scattered-reset smell that let the scrollbar interaction signal be missed in
/// R122). The per-slot state registry, in full:
///
/// * scroll offset + bound, wheel sub-row accumulator, scrollbar interaction (hover/drag) —
///   all reset by [`scrollbar::reset_pane_scroll`] (the scrollbar module owns its three);
/// * the IME preedit overlay — reset by [`input::reset_pane_preedit`];
/// * the mouse text selection — cleared by [`selection::reset_pane_selection`] iff the
///   single active selection is in this slot (R139).
/// * the OSC 52 clipboard apply/answer acks — reset by [`clipboard_osc::reset_pane_clip_acks`] so a
///   reused slot re-primes against its new pane (no stale write replay / query answer).
///
/// NOT reset: the per-slot reflow [`Effect`](pinion_core::reactive::Effect) is slot-keyed and
/// deliberately KEPT — it re-resolves the slot's live pane each fire, so it correctly reflows
/// whatever pane later reuses the slot. The dock leaf + floating window are the topology
/// authority, evicted separately by [`dock::evict_pane`] (not per-slot reactive state). Runs
/// in the root owner (the pre-view `reconcile_frame` hook resolves the per-slot cache slots).
fn reset_freed_slot(slot: usize) {
    scrollbar::reset_pane_scroll(slot);
    input::reset_pane_preedit(slot);
    selection::reset_pane_selection(slot);
    attention::reset_pane_ack(slot);
    clipboard_osc::reset_pane_clip_acks(slot);
    hyperlink::reset_pane_hyperlinks(slot);
    view::reset_pane_images(slot);
}

struct TerminalViewer;

impl WidgetCore for TerminalViewer {
    /// The context-menu open/active projection (R140) — sprag's first stateful surface,
    /// so `State` grew from `()`. Read from the `ContextMenuExternal` each frame by
    /// [`Self::read_state`] and rendered by the pure `view` ([`ctxmenu::overlay`]).
    type State = view::ViewState;
    type Event = ();

    /// sprag has **NO primary interactive surface** (R127, consuming PINION-PR51): every
    /// routable surface is a per-pane / per-split DYNAMIC EXTRA
    /// ([`Self::create_extra_externals`] — scrollbars, splitters, dock-panel handles;
    /// `external_set_is_dynamic() == true`), so there is no single canonical primary to name.
    /// Declaring the absence explicitly means the substrate composes the state scene from the
    /// extras ALONE — no phantom index-0 node — and the bare `/external` RPC shorthand rejects
    /// with `NoExternalAtPath` (pinion R1307) instead of silently resolving an arbitrary pane.
    ///
    /// This RETIRES the R122 inert `PRIMARY_SENTINEL_TAG` workaround: the primary used to be
    /// homed on a tag nothing paints, purely to satisfy pinion's then-MANDATORY primary
    /// contract (homing it on a pane slot instead would have made that slot load-bearing — a
    /// close of slot 0 would dangle it). pinion R1306 made the primary optional, so the
    /// workaround is gone and EVERY pane scrollbar — slot 0 included — is a symmetric extra.
    ///
    /// Contract obligations of a `None` binding (pinion R1306), all satisfied here:
    /// [`Self::keybinding`] stays empty (not overridden — a keybinding event would reach the
    /// no-op `send_to_primary` and be dropped; sprag drives its extras by tag), and
    /// [`Self::apply_key`] never calls `forward_key_to_external` (which would re-derive
    /// `Self::tag()`). Topology B (R115c): keyboard / IME are client SENDs to the host
    /// ([`Self::apply_key`] -> `HostClient::send_key` / `send_text`), an AI peer drives the
    /// HOST's own pane input surface directly, and pane focus is paint-derived (R1020
    /// `collect_focusable_tags`) — never `tag()`-derived. So the model scene holds only the
    /// GUI's own interaction externals (scroll / split / dock).
    fn primary_surface() -> Option<PrimarySurface> {
        None
    }

    /// Never reached — sprag declares no primary ([`Self::primary_surface`] is `None`, so the
    /// default that would call this factory is overridden away).
    fn create_external() -> Box<dyn External> {
        unreachable!("sprag-gui has no primary surface — see primary_surface()")
    }

    /// Boot the live terminal (spawn the N pane PTYs + wire each `on_dirty` ->
    /// repaint) AND install the per-pane resize -> reflow
    /// [`Effect`](pinion_core::reactive::Effect)s (pinion R1012), before the first
    /// paint and off the pure `view`. [`install_reflow`] resolves [`use_terminal`]
    /// (booting the panes) and wires every pane's reflow in one call. Then focus
    /// pane 0 so keystrokes reach a pane without a click — the shell drains this
    /// focus request before the first paint. Returns the GUI's own interaction
    /// externals (a scrollbar per occupied slot + the dock splitters/panel handles);
    /// sprag declares no primary ([`Self::primary_surface`] is `None`, R127), so every
    /// scrollbar is a symmetric extra (R122); topology B serves no per-pane INPUT external.
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
        // which IS the painted `SplitterStyle` tag the view's `view_dock_surface_chrome`
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
        // EVERY occupied slot's scrollbar is a symmetric extra (R122): sprag declares no
        // primary ([`Self::primary_surface`] is `None`, R127), so no slot — 0 included — is
        // load-bearing, and a Round 2b close of any slot just drops its extra here.
        externals.extend(
            occupied
                .iter()
                .copied()
                .map(scrollbar::pane_scrollbar_external),
        );
        // One OSC-8 hover-oracle per pane (pinion R1405, R-71.1/.2/.3): registered at
        // `pane_tag(i)` (the primary half of the grid's `#grid` composite hit-tag) so
        // the hover router forwards a grid hover to it, without painting (the pane
        // Container keeps the tag for focus / selection / rect lookups). Pointer-only,
        // never focusable — like the scrollbars/splitters above.
        externals.extend(
            occupied
                .iter()
                .copied()
                .map(hyperlink::pane_hyperlink_external),
        );
        // The find bar's text field (find-in-scrollback). Registered EVERY reconcile at its constant
        // tag, like the context menu's External — pinion R689 preserves a surviving external's live
        // state by tag, so the needle and caret survive the per-frame re-registration. It is
        // registered whether or not the bar is open: the External is what holds the text the bar
        // shows when it re-opens, and an unpainted External costs nothing.
        externals.push(find::create_find_external());
        // ...and its regex-mode toggle, on the same terms: one constant tag, registered every
        // reconcile, holding the checkbox statechart whose `checked` intent the reducer routes.
        externals.push(find::create_regex_external());
        // The command palette's query field, its row-selection / `execute` handle, and its modal
        // `open` query — the same every-reconcile-at-a-constant-tag terms as the find field above,
        // and registered while it is CLOSED for the same reason: the field's External is what holds
        // the query text, and an unpainted External costs nothing.
        externals.extend(palette::create_palette_externals());
        // ...and the destructive-command PROMPT the palette (and the window strip) arms instead of
        // acting: its captured sentence, its two answers, and its own modal `open` query. Registered
        // on the same terms, and readable over RPC while nothing is armed so "is this client asking
        // about anything?" has an address rather than being inferred from the tree.
        externals.extend(confirm::create_confirm_externals());
        // Drag-to-dock / tear-off (pinion R1081/R1084/R1094 §5.51, P2/PR-31): one R742
        // `DockPanelExternal` per pane, registered at the panel ROOT tag
        // (`split::panel_id(i)` = the `view_dock_panel` root the `view_dock_surface_chrome`
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
                        // that cannot float (the last docked one, tmux semantics) starts NO
                        // drag — no misleading chip/preview. The factory re-runs per float/dock
                        // (R70) and computes the live flag (diag `externals_rebuilt` shows
                        // [false,true] once a pane floats). LIVE since PINION-PR42: sprag's tags
                        // are constant, so `reconcile_externals` still takes its steady-state
                        // early-return, but that path now re-projects a preserved panel's
                        // declarative policy (`External::reconcile_from`) instead of dropping
                        // the rebuilt external — so this value reaches `begin_drag` after boot.
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
        // (R153) The CANONICAL REORGANIZE SURFACE — the dock as DATA, driven and observed
        // with NO pixels. It shares the same `DockReorganizer` and `drop_preview` Signal the
        // panel externals above hold, so it is a view onto the live gesture, never a second
        // authority.
        //
        // This is pinion's §2 #2 "RPC-as-primary-path" contract, which sprag had been failing
        // for its dock alone: a splitter drag and a pane's key/cells are all addressable as
        // data, but a dock reorganize was reachable ONLY by synthesising pixel coordinates
        // (`scene/drag`) — which meant no agent (and no test) could drive or watch a dock
        // gesture by intent. It cost real time: hunting drop-zone pixels landed on the window
        // RESIZE border instead of the dock band, and a near-miss tore a pane off rather than
        // docking it. `reorganize`/`drop` now take a `{source, target, zone}`; `drop_preview`
        // reports the in-flight `{source, target, zone}`; `last_outcome` reports what
        // committed; `topology` serves the tree. Same surface the editor example registers.
        externals.push(ExtraExternal::new(
            split::DOCK_REORGANIZE_TAG,
            Box::new(
                DockReorganizeExternal::from_reorganizer(Rc::clone(&reorganizer))
                    .with_drop_preview(Rc::clone(&drop_preview)),
            ),
        ));
        // R140: the right-click context menu, one `ContextMenuExternal` at a constant
        // tag (pinion R689 preserves its live open state across this per-frame rebuild,
        // like the splitters). Its item clicks / barrier dismiss route through the shell
        // pointer router; `apply_secondary_click` opens it, `update` runs its commands.
        externals.push(ctxmenu::create_menu_external());
        // The window tab strip's buttons: one `ButtonExternal` per possible tab plus "+" / "×",
        // all at fixed tags (pinion R689 preserves them across this per-frame rebuild, like the
        // context menu and splitters). Their clicks route through `update` to `SlotView` window
        // actions (select / new / kill window).
        externals.extend(wtabs::create_window_externals());
        // The session sidebar's buttons: one `ButtonExternal` per possible row plus "+", all at
        // fixed tags (pinion R689 preserves them across this per-frame rebuild). Their clicks route
        // through `update` to `SlotView` session actions (switch / new session).
        externals.extend(stabs::create_session_externals());
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
        // The scene is threaded through for ONE route: a find-bar edit, which pinion delivers
        // through the field External. Every other route is a client SEND to the host (route_key ->
        // Host::send_key), not a mutation of the GUI's own scene (topology B).
        route_key(_scene, focused, key, modifiers, false)
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
        route_key(_scene, focused, key, modifiers, repeat)
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

    /// Middle-click pastes the X11 PRIMARY selection into the FOCUSED pane (R139) —
    /// the desktop convention. This hook is focus-gated and coordinate-less (a click
    /// focuses the pane under it first, so "focused" is the pane clicked), so the
    /// PRIMARY text is written to that pane's PTY via the same
    /// [`HostClient::send_text`](sprag_host::HostClient::send_text) seam an IME commit
    /// uses. A no-op off a pane / with an empty PRIMARY.
    fn apply_middle_click(
        _scene: &mut Scene,
        focused: Option<&str>,
        _modifiers: Modifiers,
    ) -> bool {
        match focused.and_then(pane_index_of) {
            Some(i) => selection::paste_primary(i),
            None => false,
        }
    }

    /// Right-click opens the pane context menu (R140) at the press point — the
    /// position-bearing secondary-click hook. Forwards to [`ctxmenu::open_at`], which
    /// finds the menu external in the model scene and anchors the popup. The universal
    /// AI peer `scene/click {button:"right"}` reaches this same override.
    fn apply_secondary_click(scene: &mut Scene, x: f32, y: f32) -> bool {
        ctxmenu::open_at(scene, x, y)
    }

    /// Pre-view reconcile (pinion R1047 / PR-20): the sanctioned non-view-fn place to
    /// reconcile OFF-THREAD-PRODUCER state into the reactive graph, BEFORE the pure `view`
    /// fn runs — pinion runs it every real paint (including a bare wire-poll
    /// `request_repaint`) in the binding root `Owner`, so [`use_terminal`] / the per-slot
    /// hooks resolve. Two off-thread facts are reconciled here (both live in the host with
    /// no `Signal` for an `Effect` to subscribe):
    ///
    /// 1. **The pane SET (Round 2b live delta).** [`SlotView::reconcile`](crate::slotview::SlotView::reconcile)
    ///    mirrors the host's current [`PaneId`](sprag_terminal::PaneId) set onto display
    ///    slots and returns the freed + added slots. A FREED slot (its pane closed
    ///    host-side — an operator/AI close in attach mode) evicts its dock leaf / floating
    ///    window ([`dock::evict_pane`]) and resets its per-slot GUI state
    ///    ([`scrollbar::reset_pane_scroll`] / [`input::reset_pane_preedit`]) so a slot
    ///    reused by a later pane starts clean. An ADDED slot needs no hook: a new pane is in
    ///    the HOST's arrangement, which [`split::sync_layout`] projects on the same frame.
    ///    Done FIRST so the layout sync, the scroll reconcile below,
    ///    `view`, and the post-view `create_extra_externals` (the dynamic external set —
    ///    scrollbars / dock panels / splitters) all project against the reconciled set on
    ///    this SAME frame. The wire poll thread only mutates the plain cache + repaints; it
    ///    never touches a `Signal`/`Owner::cache` (those are UI-thread-only), so the map
    ///    reconcile is here, not on that thread.
    /// 2. **Each pane's scrollback depth.** Grow each pane's row-unit `ScrollState` bound to
    ///    its live `scrollback_len` and tail-follow — the terminal grid is an
    ///    `offset_lines`-projecting `Scene::TextGrid` (no `Scene::Scroll` clip for the
    ///    layout reducer) and the depth lives in the off-thread PTY producer, so neither
    ///    runtime path can reconcile it.
    ///
    /// Caveat: pinion gates this to the primary window's paint, so a FLOATED pane whose
    /// undock window alone repaints can lag one reconcile until the primary repaints —
    /// harmless in practice (live PTY output / a set change flips the root dirty bit via
    /// the R118 rail, so the primary repaints too).
    fn reconcile_frame() {
        let terminal = use_terminal();
        // (0) (R176) FIRST: resolve a session lost OUT OF BAND under the `detach-on-destroy` policy.
        // When another client / the `sprag` CLI kills THIS client's attached session, the wire poll
        // thread (which cannot switch — a UI-thread op) flags it + repaints; here, on the UI thread,
        // we switch-to-next or detach per the policy. Runs BEFORE the slot reconcile below so a switch
        // (which swaps the pane cache to the new session) is mapped onto slots THIS frame, not one
        // frame late over the dead session's now-absent panes. A no-op for the in-process host and
        // whenever no session was lost.
        terminal.slots.reconcile_lost_session();
        // (1) Mirror the host's live pane set onto slots, then fold the delta. A slot that
        // FREED drops its floating window and resets its per-slot reactive state; the dock
        // LEAF is not touched here, because the host's arrangement is the one authority on
        // which panes are tiled and (2) projects it.
        let delta = terminal.slots.reconcile();
        for slot in delta.freed {
            dock::evict_pane(slot); // the floating window (pixels — this client's)
            reset_freed_slot(slot); // the per-slot reactive state (the ONE reset owner)
        }
        // (2) Keep the dock surface and the host's arrangement in step: project a change the
        // host made (a spawn, a close, another client's gesture) and write back one this
        // client's user just settled. Runs BEFORE the view, so the first paint already has
        // the host's arrangement and no frame is painted from a stale tree.
        split::sync_layout(&terminal.slots);
        // (2b) Keep a pane on the keyboard. The window / session ops re-seed the focus ring
        // themselves (inside their own dispatch, so nothing is lost); this is the BACKSTOP for the
        // pane-set changes that reach this hook with no dispatch to live in — the poll thread's
        // lost-session switch above — plus, until the pin carrying PINION-PR78 lands, the palette
        // path, whose modal pop overrides the op's request. See
        // [`SlotView::reseed_pane_focus_if_idle`] for why it costs one input event and the op-site
        // seam does not.
        terminal.slots.reseed_pane_focus_if_idle();
        // (2) Grow each occupied pane's scroll bound to its live scrollback depth, then
        // feed its OSC-8 hover-oracle the current link map (R-71, pinion R1405) and open
        // any click-activated URI. The link map comes from the SAME projection the view
        // renders (`pane_cells` at the reconciled scroll offset), so a click resolves the
        // exact URI the hovered cell shows, and a scrolled-back link is hoverable too.
        for i in terminal.slots.occupied_slots() {
            let facts = terminal.slots.pane_scroll_facts(i);
            let scroll = scrollbar::use_pane_scroll(i);
            scrollbar::reconcile_scroll(&scroll, facts.scrollback_len);
            let offset = scrollbar::offset_lines_from_top(scroll.offset_y(), facts.scrollback_len);
            let buffer = terminal.slots.pane_cells(i, offset);
            // Feed the oracle its link map AND the child's live mouse-tracking bit, then DRAIN any
            // press/release the oracle captured while tracking and forward it to the host (which
            // gates + encodes the X10 / SGR report at the PTY boundary). The send lives here — where
            // `terminal.slots` is in scope — not in the oracle, mirroring the URI-open drain.
            let mouse_protocol = terminal.slots.pane_mouse_protocol(i);
            hyperlink::reconcile_pane_hyperlinks(i, &buffer, mouse_protocol);
            for event in hyperlink::take_pane_mouse_reports(i) {
                let _ = terminal.slots.mouse(i, event);
            }
            // Fetch + register the pane's new/changed inline-image RGBA on demand (R1404 Stage 5),
            // before the pure view references the `memory://` keys.
            view::reconcile_pane_images(&terminal.slots, i);
        }
        // (3) (R130) Track each floating window's OS title to its pane's live display
        // title (a child retitle renames its taskbar entry). Diffs internally, so it
        // only writes the windows signal when a title actually changed.
        dock::sync_floating_titles();
        // (4) (R132, PINION-PR53) Track the MAIN window's title to the FOCUSED pane's
        // display title (the tmux rule). `focus_state::focused()` is the focus manager's
        // own tag (same SSOT as `apply_key`'s `focused` arg, reflecting click / Tab /
        // request alike) and auto-subscribes this root-scope reconcile, so a focus change
        // re-runs it and repaints. A non-pane / absent focus maps to `None` -> the app name.
        let focused_pane = focused_pane();
        // (4b) ACK the focused pane's attention notification (R-PR67 follow-on): viewing a pane
        // clears its "wants attention" marker. Runs BEFORE `sync_main_title` so the focused pane's
        // OS title never flashes the marker the instant a notification lands on the pane in view.
        attention::ack_focused(&terminal.slots, focused_pane);
        dock::sync_main_title(focused_pane);
        // (4c) DEC 1004 focus reporting (mouse-tracking Stage 5): tell each pane's child when it
        // gains / loses focus (`ESC [ I` / `ESC [ O`), gated host-side on the child's 1004 mode.
        // The within-app `focused_pane` (title SSOT) is INTERSECTED with OS-window focus (pinion
        // R1419–R1421 / PINION-PR73): `window_focus_state::os_focused_window()` names the OS window
        // the WM has activated (auto-subscribing this root-scope reconcile, so an OS focus / blur
        // re-runs it and repaints). The child's focus is real only while the OS window CONTAINING
        // its pane holds focus, so alt-tabbing the whole app away emits `ESC [ O` and returning
        // `ESC [ I`. Compared by window IDENTITY (main tiling window vs the pane's `pane-{i}`
        // tear-off) so a floating pane reports its OWN window's focus, not a shared app-wide bool.
        let os_focused = pinion_core::window_focus_state::os_focused_window();
        let effective_focus =
            focus_report::os_gated_focus(focused_pane, os_focused.as_deref(), |i| {
                if dock::is_pane_floating(i) {
                    dock::pane_window_id(i)
                } else {
                    dock::MAIN_WINDOW_ID.to_owned()
                }
            });
        focus_report::reconcile_focus(&terminal.slots, effective_focus);
        // (5) (R175) Auto-disarm a pending session kill whose captured session has VANISHED from the
        // live list (killed out of band while the `kill '<name>'?` strip was up), so the confirmation
        // strip cannot linger on a session that no longer exists. Like (2)/(3) above, this reconciles
        // an off-thread-producer fact (the session mirror the wire poll updates) into a UI-thread
        // `Signal` BEFORE the pure view runs.
        stabs::reconcile_pending_kill(&terminal.slots);
        // (5b) The same auto-disarm for the client-wide destructive prompt: a window or session killed
        // out of band while its `Kill '<name>'?` modal was up takes the modal with it, so a
        // confirmation cannot outlive the thing it is asking about. Same hook, same reason as (5).
        confirm::reconcile(&terminal.slots);
        // (6) OSC 52: apply any new clipboard writes to this client's system clipboard and answer
        // any new read queries (subject to the SPRAG_OSC52 policy). Like (2)/(5), this reconciles
        // an off-thread-producer fact (the child's OSC 52 output) on the UI thread each frame —
        // here into a real side effect (the system clipboard / a PTY reply), not just a signal.
        clipboard_osc::reconcile_clipboard(&terminal.slots);
    }

    /// Mouse-wheel / touchpad two-finger scroll over a pane. Hit-tests the cursor to the pane
    /// under it (grid OR scrollbar gutter) and converts the `WheelDelta` to LINES (notched
    /// `Lines.dy`, or touchpad `Pixels.dy / LINE_HEIGHT_PX`). Then it forks (mouse tracking
    /// Stage 2 — the wheel step of the VT mouse-reporting front):
    ///
    /// * **over a mouse-tracking child's GRID, no Shift** -> REPORT the wheel to the child as
    ///   an xterm pseudo-button 64 (up) / 65 (down) press ([`hyperlink::wheel_reports`] builds
    ///   them, [`SlotView::mouse`](crate::slotview::SlotView::mouse) gates + encodes at the PTY
    ///   boundary). Coordinate conversion ([`selection::cell_at`]) is the only GUI job. Holding
    ///   Shift bypasses tracking to reach this client's own scrollback (the xterm escape hatch,
    ///   mirroring the selection escape hatch in `position_caret_for_point`).
    /// * **otherwise** (gutter, no tracking, or Shift held) -> scroll the **GUI's own**
    ///   `ScrollState` via [`scrollbar::wheel_scroll_pane`] (pinion R1045 / PR-18) — NOT the
    ///   AI-facing pane engine (the R1.7 boundary).
    ///
    /// Runs in the binding root `Owner`.
    fn apply_wheel(
        scene: &Scene,
        cursor: (f64, f64),
        delta: WheelDelta,
        modifiers: Modifiers,
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
        // window's single-pane scene the absent panes simply miss (rect -> None). Keep WHICH
        // sibling was hit: a wheel-report only fires over the grid, never the client's scrollbar.
        let terminal = use_terminal();
        let Some((i, on_grid)) = terminal.slots.occupied_slots().into_iter().find_map(|i| {
            if hit(pane_tag(i)) {
                Some((i, true))
            } else if hit(pane_scrollbar_tag(i)) {
                Some((i, false))
            } else {
                None
            }
        }) else {
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
        if on_grid && !modifiers.shift && terminal.slots.pane_mouse_active(i) {
            #[allow(
                clippy::cast_possible_truncation,
                reason = "cursor pixels fit f32 for the pixel→cell converter"
            )]
            let (fx, fy) = (cx as f32, cy as f32);
            if let Some((col, row)) = selection::cell_at(scene, i, fx, fy) {
                let mods = input::to_input_mods(modifiers);
                for event in hyperlink::wheel_reports(i, col, row, lines, mods) {
                    let _ = terminal.slots.mouse(i, event);
                }
            }
            return true;
        }
        scrollbar::wheel_scroll_pane(i, lines);
        true
    }

    /// Never reached — sprag declares no primary ([`Self::primary_surface`] is `None`), so no
    /// substrate site routes through a primary tag and the R55.G.17 painted-primary-tag
    /// convention does not apply. Pane focus is paint-derived (R1020 `collect_focusable_tags`
    /// over the `with_focusable` pane Containers), so it never depended on this tag either.
    fn tag() -> &'static str {
        unreachable!("sprag-gui has no primary surface — see primary_surface()")
    }

    /// Project the context menu's open/active state out of the model scene (R140) —
    /// the `read_state` seam the shell caches into [`Self::State`] for the pure `view`.
    fn read_state(scene: &Scene) -> Self::State {
        view::ViewState {
            menu: ctxmenu::read_menu_state(scene),
            find: find::read_field_state(scene),
            palette: palette::read_field_state(scene),
        }
    }

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
    fn update(_state: view::ViewState, intent: &Intent) -> Vec<Command> {
        // R140: a context-menu item activation arrives as the menu's `"command"`
        // intent — run its Copy / Paste / Select-all action and stop (it is not a
        // dock-panel tag, so the panel routing below would drop it anyway).
        if ctxmenu::handle_command(intent) {
            return Vec::new();
        }
        // The find bar's regex toggle: a checkbox `checked` intent, not a pane tag, so it is routed
        // here beside the context menu rather than falling through to the panel routing.
        if find::handle_regex_intent(intent) {
            return Vec::new();
        }
        // The command palette: a row activation (a click, or an RPC `execute`) arrives as the
        // palette's own `run` / `dismiss` intent. The External deliberately does NOT run the command
        // itself — it cannot, because a command reaches `Owner`-scoped state that only a reducer /
        // view path has — so the effect happens HERE, exactly as the context menu's rows do.
        if palette::handle_palette_intent(intent) {
            return Vec::new();
        }
        // The destructive-command prompt: an answer (a click on either button, a light-dismiss, or an
        // RPC `accept` / `dismiss`) arrives as its own intent. Like the palette's, the External arms
        // and the effect happens HERE — performing an armed command reaches `Owner`-scoped state that
        // only a reducer path has.
        if confirm::handle_confirm_intent(intent) {
            return Vec::new();
        }
        // The window tab strip: a tab / "+" / "×" button click routes to a `SlotView` window
        // action (select / new / kill window). Handled first, like the context menu — these are
        // not pane tags, so the panel routing below would drop them.
        if wtabs::handle_window_intent(intent, &use_terminal().slots) {
            return Vec::new();
        }
        // The session sidebar: a row / "+" button click routes to a `SlotView` session action
        // (switch / new session). Same as the window strip — not a pane tag.
        if stabs::handle_session_intent(intent, &use_terminal().slots) {
            return Vec::new();
        }
        let Some((panel, event)) = intent.tag_str().rsplit_once('.') else {
            return Vec::new();
        };
        // A settled splitter drag (pinion PR-56): the tag here is a SPLIT tag, not a pane, so
        // this must precede the pane routing below (which would drop it). The final ratio is
        // already in the divider's signal (the live drag set it), so we write the arrangement
        // now — the explicit commit edge that lets the layout survive with no idle repaint.
        if event == split::RATIO_COMMITTED_EVENT {
            split::commit_layout(&use_terminal().slots);
            return Vec::new();
        }
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
            // Order (R149, reversed): REDOCK first — the host puts the pane back into the
            // tiling (at its captured FloatHome since R156) and we adopt that — THEN relocate
            // the now-present leaf to the resolved zone, which `sync_layout` writes back once
            // the gesture settles. So the pane lands where it was DROPPED: a gesture outranks
            // the memo, because the gesture is applied second and is what gets written.
            // The intermediate "tiled at its home" arrangement is never painted: both steps
            // run inside one `update` tick with no reconcile between, so the redock still
            // paints ONCE at the final slot. A future refactor splitting them across ticks
            // would expose that transient.
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
                            dock::redock_pane(i);
                            let _ = reorganizer.dock_panel_at_resolved_zone(
                                &split::panel_id(i),
                                &target,
                                zone,
                            );
                        }
                        DropResolution::OuterDock { edge } => {
                            dock::redock_pane(i);
                            let _ = reorganizer.dock_panel_outer(&split::panel_id(i), edge);
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
    fn view(state: Self::State, frame: &Frame) -> Scene {
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
        dock::MAIN_WINDOW_TITLE
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
    fn view_for_window(window_id: &str, state: Self::State, frame: &Frame) -> Scene {
        view::view_for_window(window_id, state, frame)
    }

    /// Pointer press → text-selection anchor (R139). The shell fires this on every
    /// press with the router-resolved `hit_tag` + window-local `(x, y)`; a press on a
    /// pane grid (`{pane_tag}#grid`) anchors a selection at that cell ([`selection::press`])
    /// and returns an arm token so the shell drives [`Self::select_drag_to_point`] on the
    /// drag; a press off any pane grid returns `None` (dock / splitter / scrollbar
    /// gestures are untouched). `extend` (Shift) keeps the anchor and moves the focus.
    fn position_caret_for_point(
        _state: &Self::State,
        scene: &Scene,
        _focused: Option<&str>,
        hit_tag: Option<&str>,
        x: f32,
        y: f32,
        extend: bool,
    ) -> Option<usize> {
        // When the pane's child is TRACKING the mouse, an unmodified press is a REPORT (captured by
        // the pane pointer oracle at `pane_tag(i)`), not a text-selection anchor — suppress
        // selection here so a click does not both report AND start a drag-select. Shift is the
        // escape hatch: a Shift-press still selects natively (the xterm "override mouse mode"
        // convention), so `extend` presses fall through.
        if !extend
            && let Some(i) = selection::pane_of_hit(hit_tag)
            && hyperlink::pane_mouse_capturing(i)
        {
            return None;
        }
        selection::press(scene, hit_tag, x, y, extend)
    }

    /// Pointer drag → extend the active selection (R139). Fired on each move while the
    /// button stays held after an arming [`Self::position_caret_for_point`]; sweeps the
    /// selection focus to the cell under the cursor and publishes PRIMARY on a change
    /// ([`selection::drag`]). The `anchor` token is echoed but unused — the pane +
    /// anchor live in the reactive selection state, keyed by pane, not the byte token.
    fn select_drag_to_point(
        _state: &Self::State,
        scene: &Scene,
        _focused: Option<&str>,
        _anchor: usize,
        x: f32,
        y: f32,
    ) -> bool {
        selection::drag(scene, x, y)
    }

    /// OS file drop (pinion R770 §5.15, R1437 §5.16): a file dragged from the desktop onto a sprag
    /// window is delivered to the pane that window addresses — pasted as a local path, or
    /// `scp`-uploaded first when that pane is a `sprag ssh` remote workspace and then pasted as its
    /// REMOTE path. The host owns that decision ([`sprag_host::HostClient::drop_file`]); this end
    /// only resolves WHICH pane, from `window_id` ([`dock::drop_target_pane`], which is where the
    /// two-branch rule and its `None` are justified).
    ///
    /// `window_id` arrived with PINION-PR76 (pinion R1437) and is what makes the drop ROUTABLE: the
    /// substrate always knew which window caught the file, and the hook now passes it through, so a
    /// drop on an unfocused tear-off window no longer has to fall back to keyboard focus. winit's
    /// event still carries no position, so within the main window's split tree focus remains the
    /// only available target — routing is per WINDOW, not per pane rect.
    ///
    /// winit delivers ONE call per file, so a multi-file drop arrives as several drops and each
    /// pasted path carries a trailing space — they accumulate into a usable argument list.
    ///
    /// Returns `false` (no redraw request): nothing reactive changed here. The pasted path appears
    /// as ordinary pane output — for an upload, only once the transfer completes — so the pane's own
    /// change notification repaints it, exactly as a keystroke's echo does.
    fn on_file_drop(window_id: &str, _state: &Self::State, path: &str) -> bool {
        let terminal = use_terminal();
        let delivered =
            dock::drop_target_pane(window_id).and_then(|pane| terminal.slots.drop_file(pane, path));
        tracing::debug!(target: "sprag_gui::input", window_id, path, ?delivered, "os file drop");
        // The drag is over, so the affordance goes — and its removal IS a reactive change, unlike the
        // delivery itself (which shows up as ordinary pane output). winit sends one drop per file, so
        // a multi-file drop clears an already-cleared affordance: `clear_drop_target` is idempotent
        // and reports no change, which is why the redraw request is its answer rather than `false`.
        dock::clear_drop_target()
    }

    /// A file is being DRAGGED over `window_id` (pinion R770 §5.15, R1437 §5.16): raise the drop
    /// affordance on the pane that would receive it ([`dock::hover_drop_target`]).
    ///
    /// Worth having precisely BECAUSE sprag cannot hit-test a drop: winit reports no position, so a
    /// drop on the main window goes to the FOCUSED pane. Without an affordance the user has to know
    /// that rule; with one they can see the answer before letting go — and can retarget by clicking a
    /// pane first. The `path` is ignored: WHERE the file will land is what a drop zone must show, and
    /// sprag's answer does not depend on which file it is.
    fn on_file_hover(window_id: &str, _state: &Self::State, _path: &str) -> bool {
        dock::hover_drop_target(window_id)
    }

    /// The drag left `window_id` without dropping: take the affordance down. Positionless and
    /// path-less — the OS reports neither on cancel, which is all this needs, since it clears whatever
    /// was raised rather than matching it against a target.
    fn on_file_hover_cancel(window_id: &str, _state: &Self::State) -> bool {
        let _ = window_id;
        dock::clear_drop_target()
    }

    /// Suppress pinion's framework focus ring (R142) — the content-surface opt-out a
    /// terminal pane takes (pinion's own `hello-grid-pointer` does the same). sprag draws
    /// its OWN focus indicator instead: a translucent dark scrim over every INACTIVE pane
    /// (`view::build_pane_scene`'s `dim_inactive`, reading `focus_state::focused()`), so
    /// the active pane reads brighter (the iTerm2 / kitty / tmux "dim inactive split"
    /// convention) and — unlike the shell's top-most box ring overlay — never paints over
    /// the context-menu popup. Focus semantics are untouched: panes stay `with_focusable`,
    /// so Tab / click-to-focus / [`Self::apply_key`] routing all work.
    fn focus_ring_style(_focused_tag: &str) -> Option<pinion_shell::FocusRingStyle> {
        None
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
        _state: &view::ViewState,
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
    use sprag_host::{Host, HostClient};
    use sprag_terminal::CommandBuilder;

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

    /// Every painted `Scene::Text` string, in tree order — lets a test assert on the
    /// header/label strings a surface actually PAINTS (R130 titles).
    fn painted_texts(scene: &Scene) -> Vec<String> {
        match scene {
            Scene::Text(node) => vec![node.content.clone()],
            Scene::Container(c) => c.children.iter().flat_map(painted_texts).collect(),
            Scene::Scroll(s) => painted_texts(&s.content),
            _ => Vec::new(),
        }
    }

    /// A pane that sets an `OSC 2` window title, then stays alive (`cat`) so the PTY
    /// does not close before the paint.
    fn titled_pane(title: &str) -> CommandBuilder {
        let mut c = CommandBuilder::new("/bin/sh");
        c.arg("-c");
        c.arg(format!("printf '\\033]2;{title}\\007'; exec cat"));
        c.env("TERM", "dumb");
        c
    }

    /// A pane whose child sets NO window title — for asserting the display-title FALLBACK.
    ///
    /// Deliberately distinct from [`titled_pane`]: a child that sets a title cannot prove
    /// what happens when none is set, it can only prove it TRANSIENTLY, before its `printf`
    /// lands. `the_main_window_title_tracks_the_focused_panes_display_title` used
    /// `titled_pane("plain")` for exactly that and so raced its own fixture — passing only
    /// while the child was slow, and failing on an idle machine where it was quick.
    fn untitled_pane() -> CommandBuilder {
        let mut c = CommandBuilder::new("/bin/sh");
        c.arg("-c");
        c.arg("exec cat");
        c.env("TERM", "dumb");
        c
    }

    /// Pane width for the file-drop fixtures, which assert `pane_full_text(..).contains(path)`
    /// after the pasted path echoes back through `cat`.
    ///
    /// Wide enough that the path cannot SOFT-WRAP: a wrap puts a newline inside the needle, so the
    /// `contains` fails even though the drop was delivered correctly. At the 40 columns these tests
    /// used to share, whether they passed depended on the length of `TMPDIR` plus the fixture's own
    /// directory name — a fixture rename away from a false failure. The width is not otherwise
    /// load-bearing here (routing is per WINDOW; no assertion reads a column).
    const DROP_PANE_COLS: u16 = 200;

    /// A drop on the MAIN window lands on the FOCUSED pane, and only on it.
    ///
    /// That is the main window's half of the routing rule, and it needs pinning because it is not
    /// derived from anything the drop itself carries: winit's `DroppedFile` has no position, so
    /// within the split tree [`TerminalViewer::on_file_drop`] resolves the target the same way the
    /// context menu does — [`crate::terminal::focused_pane`]. Focus is published the way the focus
    /// manager publishes it (writing the owner's focus mirror, the stand-in pinion's own
    /// `focus_state` tests use), so this drives the REAL read rather than a seam beneath it. The
    /// tear-off half is `a_drop_on_a_tear_off_window_lands_on_that_windows_pane`.
    ///
    /// REVERT-PROOF: routing to a fixed pane — or to every pane — fails the second assertion, which
    /// is why the drop targets pane 1 while pane 0 exists and is checked for silence.
    #[test]
    fn an_os_file_drop_lands_on_the_focused_pane() {
        use std::time::{Duration, Instant};

        let dir = std::env::temp_dir().join(format!("sprag-gui-drop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the drop dir");
        let dropped = std::fs::canonicalize({
            let path = dir.join("dropped.txt");
            std::fs::write(&path, b"payload").expect("write the dropped file");
            path
        })
        .expect("canonicalize the dropped file");

        let host = Host::new((DROP_PANE_COLS, 6));
        host.spawn(
            untitled_pane(),
            "sh".to_owned(),
            DROP_PANE_COLS,
            6,
            None,
            None,
        )
        .unwrap();
        host.spawn(
            untitled_pane(),
            "sh".to_owned(),
            DROP_PANE_COLS,
            6,
            None,
            None,
        )
        .unwrap();

        let owner = Owner::new();
        owner.run(|| {
            crate::terminal::seed_terminal(host);
            let terminal = use_terminal();
            assert_eq!(terminal.slots.occupied_slots(), vec![0, 1]);

            // Publish focus on pane 1 — the focus manager's own carrier.
            Owner::current()
                .expect("inside the owner scope")
                .focused_tag_signal()
                .set(Some(pane_tag(1).to_owned()));
            assert_eq!(crate::terminal::focused_pane(), Some(1));

            assert!(
                !<TerminalViewer as WidgetView>::on_file_drop(
                    dock::MAIN_WINDOW_ID,
                    &view::ViewState::default(),
                    dropped.to_str().unwrap(),
                ),
                "a drop changes no reactive state, so it requests no redraw",
            );

            // The panes are `cat`: the pasted path echoes back through the PTY it was written to.
            let path = dropped.to_str().unwrap().to_owned();
            let deadline = Instant::now() + Duration::from_secs(5);
            while !terminal.slots.pane_full_text(1).contains(&path) {
                assert!(
                    Instant::now() < deadline,
                    "the dropped path never reached the FOCUSED pane",
                );
                std::thread::sleep(Duration::from_millis(20));
            }
            assert!(
                !terminal.slots.pane_full_text(0).contains(&path),
                "the drop must reach ONLY the focused pane, never its unfocused sibling",
            );
        });

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The hover HOOKS drive the affordance, and a DROP takes it down — the wiring, driven through the
    /// same `WidgetView` entry points the shell calls.
    ///
    /// The drop's return value is the interesting one: it requests a redraw because the affordance
    /// disappearing IS a reactive change, whereas the delivery itself is not (a pasted path arrives as
    /// ordinary pane output, repainted by the pane's own change notification).
    ///
    /// REVERT-PROOF: leaving `on_file_drop` returning a bare `false` leaves the affordance raised over
    /// a drag that already ended, failing both the cleared assertion and the redraw one.
    #[test]
    fn a_hover_raises_the_drop_affordance_and_the_drop_takes_it_down() {
        let dir = std::env::temp_dir().join(format!("sprag-gui-hover-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the drop dir");
        let dropped = dir.join("dropped.txt");
        std::fs::write(&dropped, b"payload").expect("write the dropped file");

        let host = Host::new((DROP_PANE_COLS, 6));
        host.spawn(
            untitled_pane(),
            "sh".to_owned(),
            DROP_PANE_COLS,
            6,
            None,
            None,
        )
        .unwrap();

        let owner = Owner::new();
        owner.run(|| {
            crate::terminal::seed_terminal(host);
            Owner::current()
                .expect("inside the owner scope")
                .focused_tag_signal()
                .set(Some(pane_tag(0).to_owned()));
            let state = view::ViewState::default();

            assert!(
                <TerminalViewer as WidgetView>::on_file_hover(
                    dock::MAIN_WINDOW_ID,
                    &state,
                    dropped.to_str().unwrap(),
                ),
                "the hover requests a redraw — the affordance appeared",
            );
            assert_eq!(
                dock::use_drop_hover().get(),
                Some(0),
                "and it names the pane the file would land on",
            );

            assert!(
                <TerminalViewer as WidgetView>::on_file_drop(
                    dock::MAIN_WINDOW_ID,
                    &state,
                    dropped.to_str().unwrap(),
                ),
                "the drop requests a redraw: the affordance came down",
            );
            assert_eq!(
                dock::use_drop_hover().get(),
                None,
                "a drag that ended leaves no affordance behind",
            );

            // The cancel path takes it down too, for a drag that leaves without dropping.
            assert!(<TerminalViewer as WidgetView>::on_file_hover(
                dock::MAIN_WINDOW_ID,
                &state,
                dropped.to_str().unwrap(),
            ));
            assert!(<TerminalViewer as WidgetView>::on_file_hover_cancel(
                dock::MAIN_WINDOW_ID,
                &state,
            ));
            assert_eq!(dock::use_drop_hover().get(), None, "the cancel cleared it");
        });

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A drop on a TEAR-OFF window reaches that window's pane even though another pane holds
    /// keyboard focus — the payoff of PINION-PR76 (pinion R1437), which passes the dropped
    /// window's id into the hook.
    ///
    /// This is not reachable by the focus-only rule at all: X11 / Wayland DND does not focus a
    /// window before the drop, so a file dragged onto a floater arrived with focus still on the
    /// pane the user last typed in. The routing decision itself is unit-tested across all its
    /// branches ([`dock::drop_target_pane`]); what this pins is the WIRING — that `window_id`
    /// reaches the host write and the bytes land in the addressed pane's PTY.
    ///
    /// REVERT-PROOF: routing from focus alone writes into pane 1, so the first loop times out on
    /// pane 0 AND the silence assertion on pane 1 fires — the two panes cannot both be explained
    /// by a fixed target.
    #[test]
    fn a_drop_on_a_tear_off_window_lands_on_that_windows_pane() {
        use std::time::{Duration, Instant};

        let dir = std::env::temp_dir().join(format!("sprag-gui-drop-float-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the drop dir");
        let dropped = std::fs::canonicalize({
            let path = dir.join("dropped.txt");
            std::fs::write(&path, b"payload").expect("write the dropped file");
            path
        })
        .expect("canonicalize the dropped file");

        let host = Host::new((DROP_PANE_COLS, 6));
        host.spawn(
            untitled_pane(),
            "sh".to_owned(),
            DROP_PANE_COLS,
            6,
            None,
            None,
        )
        .unwrap();
        host.spawn(
            untitled_pane(),
            "sh".to_owned(),
            DROP_PANE_COLS,
            6,
            None,
            None,
        )
        .unwrap();

        let owner = Owner::new();
        owner.run(|| {
            crate::terminal::seed_terminal(host);
            let terminal = use_terminal();
            assert_eq!(terminal.slots.occupied_slots(), vec![0, 1]);

            // Focus pane 1, then tear pane 0 off into its own window: the drop target and the
            // focus are now DIFFERENT panes, which is the whole point of the fixture.
            Owner::current()
                .expect("inside the owner scope")
                .focused_tag_signal()
                .set(Some(pane_tag(1).to_owned()));
            crate::split::sync_layout(&terminal.slots);
            dock::toggle_pane_floating(0);
            assert_eq!(crate::terminal::focused_pane(), Some(1));

            assert!(
                !<TerminalViewer as WidgetView>::on_file_drop(
                    &dock::pane_window_id(0),
                    &view::ViewState::default(),
                    dropped.to_str().unwrap(),
                ),
                "a drop changes no reactive state, so it requests no redraw",
            );

            let path = dropped.to_str().unwrap().to_owned();
            let deadline = Instant::now() + Duration::from_secs(5);
            while !terminal.slots.pane_full_text(0).contains(&path) {
                assert!(
                    Instant::now() < deadline,
                    "the dropped path never reached the TORN-OFF pane the drop landed on",
                );
                std::thread::sleep(Duration::from_millis(20));
            }
            assert!(
                !terminal.slots.pane_full_text(1).contains(&path),
                "the drop must not reach the FOCUSED pane in the other window",
            );
        });

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// (R130, PINION-PR52-A) The DOCKED panel's header paints the child's live OSC title,
    /// not its `panel_id` — the surface R128 could not reach until pinion R1318 gave the
    /// dock walker a display-title provider. The pane's IDENTITY is unchanged: the leaf /
    /// scene tag is still `terminal-0`, so drag/dock/RPC keep addressing it by that.
    #[test]
    fn a_docked_panel_header_paints_the_childs_osc_title_not_its_panel_id() {
        use std::time::{Duration, Instant};
        let host = Host::new((40, 6));
        let id = host
            .spawn(
                titled_pane("vim README"),
                "sh".to_owned(),
                40,
                6,
                None,
                None,
            )
            .unwrap();

        // The reader thread applies the OSC asynchronously — wait for the host to see it.
        let start = Instant::now();
        while host.pane_title(id).as_deref() != Some("vim README") {
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "the child's OSC title never reached the host",
            );
            std::thread::sleep(Duration::from_millis(20));
        }

        let owner = Owner::new();
        let scene = owner.run(|| {
            crate::terminal::seed_terminal(host);
            split::sync_layout(&use_terminal().slots); // project the host arrangement
            view::view_for_window(
                dock::MAIN_WINDOW_ID,
                view::ViewState::default(),
                &Frame::new(),
            )
        });

        let texts = painted_texts(&scene);
        assert!(
            texts.iter().any(|t| t == "vim README"),
            "the docked header paints the child's OSC title; painted: {texts:?}",
        );
        assert!(
            !texts.iter().any(|t| t == "terminal-0"),
            "and no longer its panel_id; painted: {texts:?}",
        );
        // Identity is untouched: the leaf is still addressed as `terminal-0`.
        assert!(
            scene.contains_tag("terminal-0"),
            "the panel's scene tag (its address) stays the panel_id, not the title",
        );
    }

    /// (R130, PINION-PR52-B) A FLOATING window's OS title tracks its pane's live display
    /// title, so a child that retitles renames its taskbar / alt-tab entry (pinion R1319
    /// made `WindowSpec::title` live; pre-R1319 it was create-time only). The MAIN window
    /// is deliberately left alone — its title would want the FOCUSED pane's, which the
    /// binding cannot read in the paint path (PINION-PR53).
    #[test]
    fn a_floating_windows_os_title_tracks_its_panes_display_title() {
        use std::time::{Duration, Instant};
        let host = Host::new((40, 6));
        let titled = host
            .spawn(
                titled_pane("vim README"),
                "sh".to_owned(),
                40,
                6,
                None,
                None,
            )
            .unwrap();
        // A second pane so floating the first is not refused (the last docked pane cannot
        // float — `float_would_empty_the_dock`). Its title is irrelevant here.
        host.spawn(untitled_pane(), "sh".to_owned(), 40, 6, None, None)
            .unwrap();

        let start = Instant::now();
        while host.pane_title(titled).as_deref() != Some("vim README") {
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "the child's OSC title never reached the host",
            );
            std::thread::sleep(Duration::from_millis(20));
        }

        let owner = Owner::new();
        owner.run(|| {
            crate::terminal::seed_terminal(host);
            split::sync_layout(&use_terminal().slots);
            dock::toggle_pane_floating(0); // tear pane 0 off into its own window
            dock::sync_floating_titles();

            let windows = dock::use_windows_topology().get();
            let floater = windows
                .iter()
                .find(|w| w.id.as_ref() == dock::pane_window_id(0))
                .expect("pane 0 has a floating window");
            assert_eq!(
                floater.title, "sprag terminal — vim README",
                "the floater's OS title carries the child's live title (app prefix kept \
                 so a taskbar entry still names the app)",
            );

            let main = windows
                .iter()
                .find(|w| w.id.as_ref() == dock::MAIN_WINDOW_ID)
                .expect("the main window");
            assert_eq!(
                main.title, "sprag terminal (interactive)",
                "sync_floating_titles leaves the main window alone (it is focus-driven, \
                 handled by sync_main_title)",
            );
        });
    }

    /// (R132, PINION-PR53) The MAIN window's title tracks the FOCUSED pane's display title
    /// (the tmux rule), and falls back to the app name when focus is off a pane. This is
    /// the last title surface, unblocked by pinion R1327's readable focus. Driven at the
    /// `sync_main_title` seam (the `reconcile_frame` caller reads `focus_state::focused()`
    /// and maps the tag to this index; the focus manager itself needs the live shell).
    #[test]
    fn the_main_window_title_tracks_the_focused_panes_display_title() {
        use std::time::{Duration, Instant};
        let host = Host::new((40, 6));
        let titled = host
            .spawn(
                titled_pane("vim README"),
                "sh".to_owned(),
                40,
                6,
                None,
                None,
            )
            .unwrap();
        // A child that sets NO title, so the fallback assertion below is a FACT rather than
        // a race against the child's own `printf` (see `untitled_pane`).
        host.spawn(untitled_pane(), "sh".to_owned(), 40, 6, None, None)
            .unwrap();

        let start = Instant::now();
        while host.pane_title(titled).as_deref() != Some("vim README") {
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "the child's OSC title never reached the host",
            );
            std::thread::sleep(Duration::from_millis(20));
        }

        let owner = Owner::new();
        owner.run(|| {
            crate::terminal::seed_terminal(host);
            split::sync_layout(&use_terminal().slots);

            let main_title = || {
                dock::use_windows_topology()
                    .get()
                    .iter()
                    .find(|w| w.id.as_ref() == dock::MAIN_WINDOW_ID)
                    .expect("the main window")
                    .title
                    .clone()
            };

            // Focus pane 0 (the OSC-titled one): the main window names it.
            dock::sync_main_title(Some(0));
            assert_eq!(
                main_title(),
                "sprag terminal — vim README",
                "the focused pane's live title names the main window (tmux rule)",
            );

            // Focus pane 1 (no OSC title): its display title falls back to the panel_id.
            dock::sync_main_title(Some(1));
            assert_eq!(
                main_title(),
                "sprag terminal — terminal-1",
                "a focused pane with no OSC title falls back to its stable panel_id",
            );

            // Focus off any pane (None): back to the plain app name.
            dock::sync_main_title(None);
            assert_eq!(
                main_title(),
                dock::MAIN_WINDOW_TITLE,
                "no focused pane -> the app name, not a stranded pane title",
            );

            // (R133 review fix) A focused FLOATING pane does NOT name the main window: it
            // is painted in its own window (titled by sync_floating_titles), not the main
            // one. Focus is binding-wide and a torn-off pane keeps its slot occupied, so
            // without the `!is_pane_floating` guard the main window would leak pane 0's
            // title even though pane 0 is not in it.
            dock::toggle_pane_floating(0); // tear pane 0 off
            dock::sync_main_title(Some(0)); // pane 0 focused, but now floating
            assert_eq!(
                main_title(),
                dock::MAIN_WINDOW_TITLE,
                "a focused floating pane leaves the main window at the app name",
            );
        });
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
        // The window tab strip (tmux "windows") takes a fixed band above the panes too, so the
        // pane content height is the window minus ALL THREE: chrome + tab strip + dock header.
        let strip_px = crate::wtabs::STRIP_HEIGHT;
        let content_rows = crate::terminal::grid_dims(
            (WINDOW_W, WINDOW_H - chrome_px - strip_px - header_px),
            metric,
        )
        .1;
        assert!(
            content_rows < full_rows,
            "the chrome strip + tab strip + dock header subtract rows"
        );
        // The session sidebar (tmux sessions / cmux workspaces) takes a fixed band down the LEFT,
        // so the pane AREA width is the window minus the sidebar — derive the expected full-area
        // cols through the same winsize SSOT against the reduced width (the horizontal analogue of
        // `content_rows`). Each of the two horizontally-split panes then fills ~half of THAT.
        let sidebar_px = crate::stabs::SIDEBAR_WIDTH;
        let area_cols = crate::terminal::grid_dims((WINDOW_W - sidebar_px, WINDOW_H), metric).0;
        let area_half = area_cols / 2;
        let full_half = full_cols / 2;
        // Test SENSITIVITY precondition: the sidebar must be wide enough that half-the-reduced-area
        // is meaningfully below half-the-full-window, or the per-pane bound below could not tell a
        // sidebar-narrowed pane from a full-window one. (With SIDEBAR_WIDTH=180 the two halves are
        // ~10 cols apart, so the `area_half + 1` upper bound sits well under `full_half`.)
        assert!(
            area_half + 1 < full_half,
            "SIDEBAR_WIDTH must narrow a pane by several cols for this test to discriminate",
        );

        assert_eq!(
            dims.len(),
            2,
            "two tiled pane grids are painted, got {dims:?}"
        );
        for (cols, rows) in &dims {
            // Horizontal split: each pane fills the window height BELOW its 28px dock header...
            assert_eq!(
                *rows, content_rows,
                "pane fills the window height below the dock header"
            );
            // ...and its WIDTH is ~half the SIDEBAR-REDUCED area, NOT half the full window. This is
            // the revert-proof check: with the sidebar removed / zero-width / an overlay, each pane
            // would reflow to ~`full_half` (well above `area_half + 1`), which this REJECTS — so a
            // sidebar that failed to narrow the pane area fails the test.
            assert!(
                (area_half - 3..=area_half + 1).contains(cols),
                "pane reflowed to ~half the sidebar-reduced width ({cols}); \
                 half the FULL window would be ~{full_half}, which this rejects",
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
    /// walk enumerates both pane tags as Tab stops. Since R179 the session sidebar
    /// adds exactly TWO more: the session `tablist` (one Tab stop for the whole list,
    /// roving inside) and the "+" new-session button — but NOT the surface root, the
    /// divider handle, or the individual rows. This is the contract that replaced the
    /// removed binding-side `focusable_tags()`; a regression here would drop focus to
    /// `None` every frame (no focus ring, dead keyboard input), so it is pinned
    /// end-to-end through the live paint path.
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
        // The session rail contributes EXACTLY two Tab stops (R179): the session
        // tablist (the list's single stop — arrows rove within) and the "+" footer.
        // The individual rows / the "×" kills are NOT stops (arrows reach them, not
        // Tab), and neither confirm/cancel is painted at boot.
        let rail_stops = focusable
            .iter()
            .filter(|t| crate::stabs::is_sidebar_focus(t))
            .count();
        assert_eq!(
            rail_stops, 2,
            "the session tablist + the '+' are the rail's two Tab stops, got {focusable:?}",
        );
        // Exactly the two panes plus those two rail stops — the surface root and the
        // divider handle are not focus stops (no `with_focusable`).
        assert_eq!(
            focusable.len(),
            4,
            "two panes + the tablist + the '+', got {focusable:?}",
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

    /// End-to-end through the REAL pinion raw-pointer router (R1416 / PINION-PR72): a TRACKING pane's
    /// oracle owns the raw multi-button stream, so a RIGHT and a MIDDLE press+release route through
    /// the shell's `pointer_button_for_window` seam (the same one winit's `MouseInput` reaches) ->
    /// `deliver_raw_pointer_button` -> `dispatch_raw_button` (which polls the oracle's
    /// `wants_raw_pointer_buttons`) into `HyperlinkOracle::raw_pointer_button`, and the shell
    /// SUPPRESSES the GUI default — no context menu on right, no PRIMARY paste on middle. The unit
    /// tests drive the oracle method directly; this proves the pinion routing reaches it for a real
    /// button edge (the seam a pixel smoke exercises, without a GPU). A drained report per edge with
    /// the right button IS the proof the raw path consumed it: had the GUI arc run instead, the
    /// oracle would have recorded nothing.
    #[test]
    fn a_tracking_pane_reports_right_and_middle_raw_clicks_through_the_real_router() {
        use pinion_core::{PointerButton, PointerEdge};
        use pinion_runtime::PointerId;
        use sprag_input::{MouseButton, MouseEventKind};

        let mut core = ShellCore::<TerminalViewer>::new();
        let scene = core.compute_paint_scene(WINDOW_W, WINDOW_H);
        core.finalize_frame(scene);
        let scene = core.compute_paint_scene(WINDOW_W, WINDOW_H);
        let (cx, cy) = {
            let r = scene
                .rect_for_tag_absolute(pane_tag(0))
                .expect("pane 0 painted");
            (
                f64::from(r.x) + f64::from(r.w) / 2.0,
                f64::from(r.y) + f64::from(r.h) / 2.0,
            )
        };

        // A cloned owner handle so the owner-scoped `use_pane_hover` calls (which resolve the cached
        // `HoverState`) run inside `owner.run` WITHOUT holding a `&core` borrow that would block the
        // `&mut core` pointer drive between them.
        let owner = core.root_owner().clone();

        // Feed pane 0's oracle a live tracking level, exactly as the per-frame host `mouse` token
        // would when the child enables DECSET 1000. Resolves the SAME cached `HoverState` the scene's
        // oracle reads; done AFTER the last paint so no reconcile overwrites it before the edges.
        owner.run(|| {
            let mut em = sprag_vt::Emulator::new(8, 3);
            sprag_vt::VtPort::advance(&mut em, b"........");
            let buffer = sprag_grid::project(
                sprag_vt::VtPort::screen(&em),
                sprag_vt::VtPort::palette(&em),
            );
            crate::hyperlink::reconcile_pane_hyperlinks(0, &buffer, sprag_vt::MouseProtocol::Click);
        });

        // Hover the pane center so the raw router resolves the pane as the pointer target, then drive
        // a right and a middle press+release through the shell seam.
        core.cursor_moved_for_window(dock::MAIN_WINDOW_ID, PointerId::MOUSE, cx, cy);
        for button in [PointerButton::Right, PointerButton::Middle] {
            core.pointer_button_for_window(dock::MAIN_WINDOW_ID, button, PointerEdge::Down);
            core.pointer_button_for_window(dock::MAIN_WINDOW_ID, button, PointerEdge::Up);
        }

        let seq: Vec<_> = owner
            .run(|| crate::hyperlink::take_pane_mouse_reports(0))
            .iter()
            .map(|r| (r.button, r.kind))
            .collect();
        assert_eq!(
            seq,
            vec![
                (MouseButton::Right, MouseEventKind::Press),
                (MouseButton::Right, MouseEventKind::Release),
                (MouseButton::Middle, MouseEventKind::Press),
                (MouseButton::Middle, MouseEventKind::Release),
            ],
            "the real pinion router routes right/middle edges to the tracking oracle (GUI defaults suppressed)",
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
    /// the read side (`view_dock_surface_chrome` -> `view_splitter` -> layout -> R1012 reflow)
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
        // The boot split between pane 0 and pane 1 carries the host SplitId(0) tag.
        core.root_owner().run(|| {
            split::use_split_ratio(split::split_tag(sprag_terminal::SplitId(0)), 0.5).set(0.7)
        });
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

    /// sprag declares NO primary surface (R127, consuming PINION-PR51): every routable
    /// surface is a per-pane / per-split dynamic EXTRA, so the R122 inert sentinel is gone.
    /// FOUR claims, all regressions someone re-adding a primary would trip — the three
    /// obligations pinion R1306 places on a `None`-primary binding plus the composition:
    /// (1) the opt-out is declared; (2) the substrate never REACHES the `unreachable!`
    /// `create_external` / `tag` — booting a `ShellCore` + painting would panic if any site
    /// still routed through a primary; (3) the composed state scene resolves NO primary, so
    /// the bare `/external` RPC shorthand rejects (`NoExternalAtPath`, pinion R1307) instead
    /// of silently naming an arbitrary pane — while the pane extras still paint; (4)
    /// `keybinding` stays the empty default — pinion routes a keybinding event to the no-op
    /// `send_to_primary` on a no-primary binding (it would be silently dropped), and sprag
    /// drives every key through `apply_key` -> `route_key` -> the host instead, so an
    /// override here would be a latent input-loss bug.
    #[test]
    fn declares_no_primary_surface_and_composes_from_extras_alone() {
        assert!(
            <TerminalViewer as WidgetCore>::primary_surface().is_none(),
            "sprag opts out of the primary contract — all surfaces are dynamic extras",
        );
        assert!(
            <TerminalViewer as WidgetCore>::keybinding("Enter").is_none()
                && <TerminalViewer as WidgetCore>::keybinding("a").is_none(),
            "a no-primary binding must not bind keys — they would hit the no-op \
             send_to_primary and be dropped (R1306 obligation)",
        );

        let mut core = ShellCore::<TerminalViewer>::new();
        let scene = core.compute_paint_scene(WINDOW_W, WINDOW_H);

        assert!(
            core.scene().primary_external().is_none(),
            "the state scene is no-primary-head: a bare `/external` must reject, not \
             resolve an arbitrary pane extra",
        );
        assert!(
            scene.contains_tag(pane_scrollbar_tag(0)),
            "the pane extras still paint — the scene composes from the extras alone",
        );
    }

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

    /// A settled splitter drag (pinion PR-56 `ratio_committed`) writes the new ratio to the
    /// host in ONE reducer dispatch — no settle frame. The intent's tag is a SPLIT tag, not a
    /// pane, so this also guards that the reducer routes it BEFORE the pane-panel gate (which
    /// would drop it). This is the explicit commit edge the ratio path relies on now that the
    /// idle-repaint livelock fix (R152) removed the spurious frames the settle detector rode.
    #[test]
    fn ratio_committed_intent_writes_the_settled_ratio_to_the_host() {
        use sprag_terminal::{LayoutNodeWire, SplitId};
        use std::borrow::Cow;
        let mut core = ShellCore::<TerminalViewer>::new();
        let scene = core.compute_paint_scene(WINDOW_W, WINDOW_H);
        core.finalize_frame(scene); // boot layout adopted: `seen` + topology populated

        // The boot two-pane row has one divider, host SplitId 0 -> tag "sprag_gui.split.0".
        let tag = split::split_tag(SplitId(0));
        let host_ratio = |core: &ShellCore<TerminalViewer>| {
            core.root_owner()
                .run(|| match use_terminal().slots.layout().tree.root {
                    Some(LayoutNodeWire::Split { ratio, .. }) => Some(ratio),
                    _ => None,
                })
        };
        assert_eq!(host_ratio(&core), Some(0.5), "the boot divider is even");

        // Simulate the live drag: pinion's `pointer_move` sets this signal; set it directly,
        // then fire the drag-end intent the shell delivers on release.
        core.root_owner()
            .run(|| split::use_split_ratio(tag.clone(), 0.5).set(0.75));
        let intent = Intent {
            tag: Cow::Owned(format!("{tag}.{}", split::RATIO_COMMITTED_EVENT)),
            payload: IntrospectValue::Float(0.75),
        };
        core.dispatch_intent(&intent);

        assert_eq!(
            host_ratio(&core),
            Some(0.75),
            "the settled ratio reached the host in one dispatch, with no settle frame",
        );
    }

    /// The window tab strip end to end through the REAL shell: a "+" click creates + selects a
    /// window, a tab click selects one by its index into the live list, and a "×" click ASKS —
    /// arming the client's confirmation prompt, which a second intent answers before anything is
    /// destroyed. Each is routed by `update`, to a `SlotView` window action or to
    /// [`confirm`](crate::confirm), against the in-process host. Built with the SCOPED tags the shell
    /// actually delivers (`{tag}.{event}`), the R67/R68 lesson that a synthetic input which does not
    /// match the live wire masks the bug.
    ///
    /// The "×" leg is the whole confirmation chain in one test: strip click → armed prompt → answer →
    /// kill. REVERT-PROOF for the front: restore the direct `slots.kill_window` in
    /// [`wtabs::handle_window_intent`] and the "destroyed nothing" assertion fails; drop the
    /// `confirm::handle_confirm_intent` arm from `update` and the window is never killed at all.
    #[test]
    fn window_tab_clicks_route_to_the_host_select_new_and_close() {
        use std::borrow::Cow;
        let mut core = ShellCore::<TerminalViewer>::new();
        let scene = core.compute_paint_scene(WINDOW_W, WINDOW_H);
        core.finalize_frame(scene);

        // Read the in-process host's window list through the SlotView, in the root owner scope.
        let windows = |core: &ShellCore<TerminalViewer>| {
            core.root_owner().run(|| use_terminal().slots.windows())
        };
        let current = |core: &ShellCore<TerminalViewer>| {
            windows(core)
                .into_iter()
                .find(|w| w.current)
                .map(|w| w.name)
        };
        let click = |tag: &str| Intent {
            tag: Cow::Owned(format!("{tag}.click")),
            payload: IntrospectValue::Null,
        };

        // Boot: one window "0", current.
        assert_eq!(windows(&core).len(), 1, "one boot window");
        assert_eq!(current(&core).as_deref(), Some("0"));

        // "+" creates a window AND selects it (tmux new-window).
        core.dispatch_intent(&click("sprag_gui.wnew"));
        assert_eq!(windows(&core).len(), 2, "the + button created a window");
        let created = current(&core).expect("a current window");
        assert_ne!(created, "0", "new-window selected the new one");

        // A tab click on tab index 0 selects window "0" (tmux select-window) — every click fires,
        // so switching back to the pre-`+` tab is NOT silenced.
        core.dispatch_intent(&click("sprag_gui.wtab.0"));
        assert_eq!(
            current(&core).as_deref(),
            Some("0"),
            "the tab click selected window 0",
        );

        // "×" ASKS before it closes: the click arms the confirmation and destroys nothing on its own.
        core.dispatch_intent(&click("sprag_gui.wclose"));
        assert_eq!(
            windows(&core).len(),
            2,
            "the × button destroyed nothing on its own",
        );
        assert!(
            core.root_owner().run(confirm::is_open),
            "it armed the confirmation instead",
        );

        // ...and the ANSWER closes the CURRENT window ("0"); the other survives and becomes current.
        core.dispatch_intent(&Intent {
            tag: Cow::Owned(format!("{}.accept", confirm::CONFIRM_TAG)),
            payload: IntrospectValue::Null,
        });
        let after = windows(&core);
        assert_eq!(
            after.len(),
            1,
            "the confirmed kill closed the current window"
        );
        assert_eq!(after[0].name, created, "the other window survived");
        assert!(after[0].current, "and it is now current");
        assert!(
            !core.root_owner().run(confirm::is_open),
            "and the prompt went with the answer",
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
    /// the pane's membership restores. The leaf was removed on float (the host stopped tiling
    /// it) and the redock asks the host to tile it again, so membership is restored by
    /// PROJECTING the host's answer — not by a second builder here. Then a SECOND redock (the
    /// pane already docked) is a harmless no-op. Live scoped tag + `Text` payload, asserting
    /// the membership both authorities expose.
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

    /// Cross-window zone-honoring redock — the REDUCER ARM (R1100 / PINION-PR33), and the
    /// claim that let R149 delete the placeholder model.
    ///
    /// GIVEN the `terminal-0.tear_off_redock_at` intent (what pinion's R1148->R1163b
    /// `over_window` composition fires when a floater is dropped over another window's dock
    /// zone), the reducer must land the pane AT THE DROP ZONE — `[terminal-1, terminal-0]`,
    /// the reverse of boot home — not at the place it came from. Zone-honoring here was once
    /// placeholder mode's whole reason to exist; it now works with the leaf REMOVED on float,
    /// because the reducer redocks first (the host tiles the pane again) and then relocates
    /// the leaf, which `sync_layout` writes back. pinion R1173 also made
    /// `dock_panel_at_resolved_zone` total over an absent leaf.
    ///
    /// **R156 SHARPENED THIS TEST WITHOUT TOUCHING IT.** Before the float home anchor the
    /// redock step APPENDED, which for two panes already produced `[terminal-1, terminal-0]`
    /// — so the assertion was satisfiable with the relocate doing nothing at all. Now the
    /// redock puts the pane back at its home (`[terminal-0, terminal-1]`), so only the
    /// relocate can produce what this asserts. It stopped being a coincidence and became a
    /// test of the thing it names.
    ///
    /// SCOPE (honest, avoiding the R69/R83 overclaim): this does NOT prove the LIVE gesture
    /// "drag a settled floater onto main -> it redocks" end to end — whether the shell FIRES
    /// this intent on that gesture is pinion's `over_window` composition. This guard covers
    /// sprag's half, the consumption; the live firing is a separate check owed a real mouse.
    #[test]
    fn tear_off_redock_at_in_collapse_honors_the_zone_cleanly() {
        use std::borrow::Cow;
        let mut core = ShellCore::<TerminalViewer>::new();
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

    /// Paint ONE frame — the budget the real app has once it parks on `ControlFlow::Wait`.
    ///
    /// This models the single property R152 changed and no other test here captures. A signal
    /// mutation arms exactly ONE repaint, so a settled gesture gets exactly ONE frame to be
    /// written; every other test in this file paints unconditionally and therefore silently
    /// hands the code frames the real app stopped producing the moment the idle-repaint
    /// livelock was fixed. That gap is how a lost-layout regression shipped with 110 tests
    /// green (R152 -> R154).
    fn paint_one_frame(core: &mut ShellCore<TerminalViewer>) {
        let scene = core.compute_paint_scene(WINDOW_W, WINDOW_H);
        core.finalize_frame(scene);
    }

    /// The host's arrangement, as the client's mirror reports it.
    fn host_layout(core: &ShellCore<TerminalViewer>) -> sprag_terminal::LayoutWire {
        core.root_owner().run(|| use_terminal().slots.layout().tree)
    }

    /// A settled drag-to-dock re-arrangement reaches the HOST on the single frame its commit
    /// arms — no second frame required.
    ///
    /// The regression this pins (R152 -> R154) lost the user's layout: `sync_layout` waited for
    /// one STILL frame before writing, which only ever arrived because the idle-repaint livelock
    /// was manufacturing frames. R152 fixed the livelock, the shell began parking on
    /// `ControlFlow::Wait`, a commit armed exactly ONE repaint — the frame that merely RECORDED
    /// the settle candidate — and the write never happened. The re-arrangement was then lost on
    /// detach, defeating the whole point of the arc, and nondeterministically: any later
    /// unrelated repaint would flush the stale candidate, so a busy pane hid what an idle pane
    /// lost.
    ///
    /// So this drives the gesture through the SAME call pinion's drop path uses
    /// (`dock_panel_at_resolved_zone` -> `DockReorganizer::commit` -> the topology signal) and
    /// then paints exactly ONE frame — the budget a commit arms. Asserting the HOST's tree, not
    /// the client's, is the point: the client repainted correctly the entire time it was losing
    /// the user's work.
    #[test]
    fn a_settled_reorganize_reaches_the_host_on_the_one_frame_it_arms() {
        use pinion_widget_paint::dock::DockDropZone;
        let mut core = ShellCore::<TerminalViewer>::new();
        paint_one_frame(&mut core);

        let before = host_layout(&core);
        assert!(
            matches!(
                before.root,
                Some(sprag_terminal::LayoutNodeWire::Split {
                    dir: sprag_terminal::SplitDir::Horizontal,
                    ..
                })
            ),
            "the boot host arrangement is a horizontal row: {before:?}",
        );

        // The drop: dock pane 0 under pane 1. This is what pinion's `drag_release_at` calls.
        core.root_owner().run(|| {
            let reorg = split::use_dock_reorganizer();
            let _ = reorg.dock_panel_at_resolved_zone(
                &split::panel_id(0),
                &split::panel_id(1),
                DockDropZone::Bottom,
            );
        });

        paint_one_frame(&mut core);
        let after = host_layout(&core);
        assert_ne!(
            after, before,
            "the settled re-arrangement never reached the host on the one frame its commit \
             arms — it would be LOST on detach",
        );
        assert!(
            matches!(
                after.root,
                Some(sprag_terminal::LayoutNodeWire::Split {
                    dir: sprag_terminal::SplitDir::Vertical,
                    ..
                })
            ),
            "the host holds the dropped arrangement (a vertical stack): {after:?}",
        );
    }

    /// A divider drag must NOT write mid-gesture just because something else repainted.
    ///
    /// The retired settle detector inferred "settled" from "unchanged since the last frame", so
    /// a user pausing a drag while any pane printed (`top`, a log tail — ordinary in a terminal)
    /// got the intermediate ratio written: a blocking socket round trip on the UI thread, the
    /// exact thing the gate existed to prevent. Ratio is now owned solely by the
    /// `ratio_committed` commit edge, so no number of frames can write a drag that has not ended.
    #[test]
    fn frames_alone_never_write_an_unfinished_ratio_drag() {
        use sprag_terminal::SplitId;
        let mut core = ShellCore::<TerminalViewer>::new();
        paint_one_frame(&mut core);

        let before = host_layout(&core);
        // Mid-drag: pinion's `pointer_move` has moved the live ratio, but no release yet.
        core.root_owner()
            .run(|| split::use_split_ratio(split::split_tag(SplitId(0)), 0.5).set(0.8));

        // Paint generously — far more than a real pause over a printing pane would ever see.
        for _ in 0..4 {
            paint_one_frame(&mut core);
        }
        assert_eq!(
            host_layout(&core),
            before,
            "an unfinished ratio drag must not be written by frames alone",
        );
    }

    /// The canonical reorganize surface is REGISTERED, so the dock is addressable as data
    /// (`reorganize {source,target,zone}`, `drop_preview`, `last_outcome`, `topology`) instead
    /// of only by pixels — pinion's §2 #2 RPC-as-primary-path contract.
    ///
    /// Guarded because its ABSENCE is silent and expensive: without it the only way to drive a
    /// dock reorganize is synthesising drop-zone COORDINATES, which no test can do reliably —
    /// R153 burned a session proving that, landing on the window resize border and tearing a
    /// pane off on a near-miss. A surface that exists only in pixels is untestable by
    /// construction.
    ///
    /// It pins REGISTRATION, and R154's review was right that the name used to promise more
    /// ("...is_drivable..."): it exercises none of the four wire surfaces it names, and would
    /// stay green if the external were built from a FRESH `DockReorganizer` instead of the
    /// shared one — the "a view, not a fork" property `5af431e` headlined. Drivability itself
    /// is proven live over RPC (R153) and by
    /// [`a_settled_reorganize_reaches_the_host_on_the_one_frame_it_arms`], which drives the
    /// shared reorganizer and asserts the host moved.
    #[test]
    fn the_canonical_reorganize_surface_is_registered() {
        let core = ShellCore::<TerminalViewer>::new();
        core.root_owner().run(|| {
            let tags: Vec<String> = <TerminalViewer as WidgetCore>::create_extra_externals()
                .into_iter()
                .map(|e| e.tag.into_owned())
                .collect();
            assert!(
                tags.iter().any(|t| t == split::DOCK_REORGANIZE_TAG),
                "the canonical reorganize surface must be registered, else the dock is \
                 reachable only by synthesised pixels: {tags:?}",
            );
        });
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
            let render =
                || view::view_for_window(dock::MAIN_WINDOW_ID, view::ViewState::default(), &frame);

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
        // The hook resolves through `use_dock_reorganizer`, which classifies against the
        // LIVE topology — so project the host's arrangement first, as the pre-view reconcile
        // does each frame (an unprojected surface has no panels to drop onto).
        let core = ShellCore::<TerminalViewer>::new();
        core.root_owner().run(|| {
            split::sync_layout(&use_terminal().slots);
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
            split::sync_layout(&use_terminal().slots); // the pre-view projection
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
