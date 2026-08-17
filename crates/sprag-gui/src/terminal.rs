//! The booted terminal model, its config (font + command), and the winsize
//! derivation — everything about *creating and holding* the live pane, spawned
//! at boot off the pure `view`. See the crate-root module docs for the seams.

use crate::slotview::SlotView;
use crate::{WINDOW_H, WINDOW_W};
use pinion_core::CellMetric;
use pinion_core::reactive::Owner;
use pinion_core::{use_quit_sink, use_repaint_sink};
use sprag_client::WireHost;
use sprag_host::{Host, HostClient};
use sprag_terminal::CommandBuilder;
use std::rc::Rc;
use std::sync::Arc;

/// Default glyph size (logical px) — the font-size SSOT the cell is measured from. The user's
/// `gui-font` option overrides it ([`font_size_px`]).
///
/// Spelled here as well as in [`OPTIONS`](sprag_host::options::OPTIONS) because a `const` table
/// cannot compute a default this crate owns, and this crate must have one to fall back on when the
/// registry cannot answer. Nothing in the type system holds the two together; the drift guard
/// `the_registry_default_is_this_crates_own` does.
const FONT_SIZE_PX: u32 = 20;

/// `Owner::cache` key for the live terminal (created once at boot).
const SESSION_KEY: &str = "sprag_gui.terminal";

/// The maximum number of tiled panes. The per-pane tags must be `&'static str`
/// (the [`WidgetCore::tag`](pinion_core::WidgetCore::tag) + input-External tag
/// contract), so they come from this fixed table rather than being minted at
/// runtime. A windowed terminal tiling more than a handful of panes is not useful
/// (each pane shrinks toward unreadable), so a small cap is the honest bound, not
/// a limitation to design around. Dynamic pane creation (a deferred round) keeps
/// this cap. (The Tab-order refresh that dynamic panes need is no longer a gap:
/// pinion §5.39 (R1020) derives the focusable set per frame from the paint scene
/// — [`Scene::collect_focusable_tags`](pinion_core::Scene::collect_focusable_tags)
/// — so a pane appearing / disappearing joins / leaves the Tab order on its own.)
pub(crate) const MAX_PANES: usize = 8;

/// The complete `&'static str` identity of one tiled pane, grouped so its three
/// per-pane tags CANNOT drift apart — each row is one pane:
///
/// * [`pane`](Self::pane) (`sprag_gui.pane.<i>`) — the model-scene input External
///   tag ([`create_external`](crate::TerminalViewer) /
///   [`create_extra_externals`](crate::TerminalViewer)), the scene-derived focus
///   tag (the pane Container is `with_focusable`, so the shell's per-frame
///   [`Scene::collect_focusable_tags`](pinion_core::Scene::collect_focusable_tags)
///   picks it up — pinion §5.39 (R1020)), the paint-scene pane Container
///   ([`sprag_host::pane_view_scene_from_cells`] — the R1012
///   [`use_pane_viewport_size`](pinion_core::use_pane_viewport_size) rect target +
///   focus ring + click anchor), and the per-pane reflow Effect tag.
/// * `scrollbar` (`sprag_gui.scrollbar.<i>`) — the scrollbar track paint tag + its
///   drag `ScrollBarExternal` registration tag ([`crate::scrollbar`]).
/// * `scroll_key` (`sprag_gui.scroll.<i>`) — the `Owner::cache` key of the pane's
///   row-unit `ScrollState` ([`use_pane_scroll`](crate::scrollbar::use_pane_scroll)).
struct PaneSlot {
    pane: &'static str,
    scrollbar: &'static str,
    scroll_key: &'static str,
}

/// The per-pane **`&'static str` External-tag identity** SSOT — one [`PaneSlot`]
/// per tile up to [`MAX_PANES`], replacing the former parallel `pane` / `scrollbar`
/// / `scroll_key` arrays (three separately hand-typed tables that could index-drift;
/// grouped into one row each here, they cannot). `&'static str` literals because
/// the pinion External-tag / [`use_scroll_state`](pinion_core::widgets::scroll::use_scroll_state)
/// contract demands it.
///
/// SCOPE: this table is the `&'static` tags that flow into pinion APIs. Per-pane
/// `Owner::cache` keys that need NOT be `&'static` (preedit, reflow, wheel-accum)
/// are minted by [`pane_cache_key`] from the same pane index — a derived, single-
/// format axis that cannot drift the way the old parallel literal tables could
/// (the index is the call argument, not a hand-typed column). Per-DIVIDER identity
/// is yet another axis (the dock-tree Split ids, [`crate::split`], keyed by each
/// Split's stable id rather than the tile index).
#[rustfmt::skip]
const PANE_SLOTS: [PaneSlot; MAX_PANES] = [
    PaneSlot { pane: "sprag_gui.pane.0", scrollbar: "sprag_gui.scrollbar.0", scroll_key: "sprag_gui.scroll.0" },
    PaneSlot { pane: "sprag_gui.pane.1", scrollbar: "sprag_gui.scrollbar.1", scroll_key: "sprag_gui.scroll.1" },
    PaneSlot { pane: "sprag_gui.pane.2", scrollbar: "sprag_gui.scrollbar.2", scroll_key: "sprag_gui.scroll.2" },
    PaneSlot { pane: "sprag_gui.pane.3", scrollbar: "sprag_gui.scrollbar.3", scroll_key: "sprag_gui.scroll.3" },
    PaneSlot { pane: "sprag_gui.pane.4", scrollbar: "sprag_gui.scrollbar.4", scroll_key: "sprag_gui.scroll.4" },
    PaneSlot { pane: "sprag_gui.pane.5", scrollbar: "sprag_gui.scrollbar.5", scroll_key: "sprag_gui.scroll.5" },
    PaneSlot { pane: "sprag_gui.pane.6", scrollbar: "sprag_gui.scrollbar.6", scroll_key: "sprag_gui.scroll.6" },
    PaneSlot { pane: "sprag_gui.pane.7", scrollbar: "sprag_gui.scrollbar.7", scroll_key: "sprag_gui.scroll.7" },
];

/// The identity tag of the pane at tile `index` (`index < `[`MAX_PANES`]).
pub(crate) fn pane_tag(index: usize) -> &'static str {
    PANE_SLOTS[index].pane
}

/// Pane `index`'s scrollbar track + drag-External tag (`index < `[`MAX_PANES`]).
pub(crate) fn pane_scrollbar_tag(index: usize) -> &'static str {
    PANE_SLOTS[index].scrollbar
}

/// Pane `index`'s row-unit `ScrollState` `Owner::cache` key (`index < `[`MAX_PANES`]).
pub(crate) fn pane_scroll_key(index: usize) -> &'static str {
    PANE_SLOTS[index].scroll_key
}

/// The tile index of the pane whose identity tag is `tag`, or `None` if `tag` is
/// not a pane tag (a non-pane / absent focus). The inverse of [`pane_tag`], so
/// input routing maps the focused tag back to its pane.
pub(crate) fn pane_index_of(tag: &str) -> Option<usize> {
    PANE_SLOTS.iter().position(|s| s.pane == tag)
}

/// The tile index of the FOCUSED pane, or `None` when focus is off any pane — the app's one answer
/// to "which pane is the user acting on".
///
/// `focus_state::focused()` is the focus manager's own tag (the same SSOT `apply_key` routes by,
/// reflecting a click, Tab, or a focus request alike) and AUTO-SUBSCRIBES the reactive scope it is
/// read in, so a caller inside a reconcile / view re-runs on a focus change.
///
/// This is the target every POSITIONLESS interaction resolves to: the context menu's actions (a menu
/// click has since blurred the pane, so the target is snapshotted at open time) and an OS file drop,
/// which winit delivers with neither a position nor a window (see `TerminalViewer::on_file_drop`).
pub(crate) fn focused_pane() -> Option<usize> {
    pinion_core::focus_state::focused()
        .as_deref()
        .and_then(pane_index_of)
}

/// Pane `index`'s `Owner::cache` key in `namespace` — `sprag_gui.<namespace>.<index>`
/// — for the per-pane view-state slots that need NOT be `&'static` (preedit, reflow,
/// wheel-accum). The ONE site that mints a per-pane cache key, so the index suffix
/// is derived in one place: the index is the argument (not a hand-typed column), so
/// these cannot index-drift the way the former parallel `&'static` tag arrays could.
/// The `&'static` External tags live in [`PaneSlot`] instead (pinion API contract).
pub(crate) fn pane_cache_key(namespace: &str, index: usize) -> String {
    format!("sprag_gui.{namespace}.{index}")
}

/// The default tiled pane count when `SPRAG_GUI_PANES` is unset.
const PANE_COUNT_DEFAULT: usize = 2;

/// Parse a `SPRAG_GUI_PANES` spec into a pane count, clamped to
/// `[1, `[`MAX_PANES`]`]`. Absent / malformed / zero falls back to `default`.
/// Pure (no env) so it is unit-testable.
fn parse_pane_count(spec: Option<&str>, default: usize) -> usize {
    spec.and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|&n| n >= 1)
        .unwrap_or(default)
        .min(MAX_PANES)
}

/// The tiled pane count: `SPRAG_GUI_PANES=<n>` (clamped to `[1, `[`MAX_PANES`]`]`)
/// overrides the default of [`PANE_COUNT_DEFAULT`]. Env-read (no `Owner` scope), so
/// the boot spawn ([`use_terminal`]) and the boot divider-orientation registration
/// ([`create_extra_externals`](crate::TerminalViewer)) read the one source and
/// agree on the count. (Keyboard focus is no longer counted from here: pinion R1020
/// derives the Tab order per frame from the painted panes, not a binding-side
/// list — see [`pane_tag`].)
pub(crate) fn pane_count() -> usize {
    parse_pane_count(
        std::env::var("SPRAG_GUI_PANES").ok().as_deref(),
        PANE_COUNT_DEFAULT,
    )
}

/// The glyph size: the user's `gui-font` option, else [`FONT_SIZE_PX`].
///
/// # Why this reads the file with no holder
///
/// [`crate::keys`] holds the ONE live view of `config.toml` and re-reads it where a keystroke's
/// meaning is decided. This is not a second one: it is a single read at the moment the value is USED,
/// which for a glyph size is the birth of a window. A held copy would add a staleness verdict for a
/// value that is adopted once — the shape `sprag-client`'s destroy policy also avoids.
///
/// # Why the option does not take effect on a RUNNING window
///
/// The glyph size decides the measured cell, which decides every pane's grid, which is the size its
/// PTY was told. Changing it live is the RESIZE path plus a wake that says "the config moved" — and
/// this client has no such wake by design (no thread, no timer, no watcher; a keystroke is what wakes
/// the keymap re-read, and a font that changed on the next keypress would be worse than one that
/// changed at the next window). So the file is the authority and a window adopts it at birth. That is
/// honest in a way the env var could not be: `sprag show-options` prints what the next window will
/// use, where an env var could not be printed at all.
///
/// A config that cannot be READ falls back to the registry's defaults and says nothing here, because
/// the same file's problem is already reported through [`crate::keys::ClientKeys::report`] — twice
/// would be two lines about one typo.
fn font_size_px() -> u32 {
    sprag_host::config::options()
        .unwrap_or_default()
        .number(sprag_host::options::GUI_FONT)
        .unwrap_or(FONT_SIZE_PX)
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
    // Policy: the GUI's command spec is `SPRAG_GUI_CMD` ([`command_spec`]). The
    // assembly (TERM, args, label) and the `$SHELL` fallback are the shared SSOT.
    match command_spec() {
        Some((program, args)) => sprag_terminal::command_from_parts(program, args),
        // No explicit argv: the user's `default-command`, then `$SHELL`. `SPRAG_GUI_CMD` stays ahead
        // of it because an argv a launcher passed is an explicit choice, and `default-command` is by
        // definition what runs when there was none.
        None => sprag_host::config::default_pane_command(),
    }
}

/// The GUI's command spec from `SPRAG_GUI_CMD` (whitespace-split: program + args; no
/// shell quoting), or `None` when unset/empty. The ONE env-read + split site, shared
/// by the in-process boot ([`pane_command`] -> `CommandBuilder`) and the wire boot
/// ([`pane_argv`] -> JSON argv), so the two consumers cannot parse the spec differently.
fn command_spec() -> Option<(String, Vec<String>)> {
    let spec = std::env::var("SPRAG_GUI_CMD").unwrap_or_default();
    split_command(&spec)
}

/// The booted terminal MODEL the GUI holds each frame: the [`SlotView`] it reaches the
/// panes through (by display slot), plus the client's own rendering config.
///
/// [`SlotView`] wraps a `Box<dyn HostClient>` chosen at boot (topology B): by default a
/// [`WireHost`] — a pure wire client of a `sprag-term` host PROCESS (the GUI owns no
/// `Workspace` / PTYs) — or, under `SPRAG_GUI_HOST=inprocess`, an in-process [`Host`]
/// (the debug / test escape hatch). Both are pure IDENTITY clients (they speak
/// `PaneId`); the `SlotView` is the ONE place display slots map onto those ids, so the
/// slot concept lives entirely in the GUI. Every pane call site reaches the panes ONLY
/// through this slot view, so the frontend code is identical across the two clients. The
/// rendering config — the once-measured cell metric and the glyph size it was measured at
/// — is read each frame by `view`, never re-measured; deliberately client-side, not host
/// state.
pub(crate) struct TerminalView {
    pub(crate) slots: SlotView,
    pub(crate) metric: CellMetric,
    pub(crate) font_size_px: u32,
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
pub(crate) fn grid_dims(viewport: (u32, u32), metric: CellMetric) -> (u16, u16) {
    let (width, height) = viewport;
    (
        metric.cols_for(width).max(1),
        metric.rows_for(height).max(1),
    )
}

/// The inverse of [`grid_dims`]: the pixel size that exactly fits `(cols, rows)`
/// cells at `metric`. The one `cols -> px` site (an undock window's intrinsic
/// open size, [`dock`](crate::dock)), kept beside the `px -> cols` derivation so
/// the cell<->pixel round-trip lives in one module.
pub(crate) fn cell_px(metric: CellMetric, cols: u16, rows: u16) -> (u32, u32) {
    (
        u32::from(cols) * metric.cell_w(),
        u32::from(rows) * metric.cell_h(),
    )
}

/// A pane widget's laid-out extent measured in CELLS — **fractional on purpose**.
///
/// A pane's rect is not a whole number of cells, and pinion's `TextGrid` fills the sub-cell
/// remainder with the terminal's own background (pinion R1028) rather than leaving it to the
/// surface behind — so the remainder is invisible but real. Anything mapping a `[0, 1]`
/// fraction OF THAT RECT back to a cell has to scale by this, not by a cell COUNT: see
/// [`cell_index`] for what the count-scaled form gets wrong.
#[allow(
    clippy::cast_precision_loss,
    reason = "a logical-pixel extent is far below f32's exact-integer range"
)]
pub(crate) fn rect_cells(rect_px: (u32, u32), metric: CellMetric) -> (f32, f32) {
    let axis = |extent: u32, cell: u32| {
        if cell == 0 {
            0.0
        } else {
            extent as f32 / cell as f32
        }
    };
    (
        axis(rect_px.0, metric.cell_w()),
        axis(rect_px.1, metric.cell_h()),
    )
}

/// The 0-based cell an offset of `offset_cells` CELLS into a pane lands on, in a grid of
/// `count` cells — the ONE pointer -> cell rule, and the reason it takes an offset already in
/// cells rather than a fraction and a count.
///
/// Two halves, and BOTH are load-bearing:
///
/// * the offset is measured in CELLS (a pixel offset divided by the cell, or a rect fraction
///   scaled by [`rect_cells`]) — never a fraction scaled by `count`. The count-scaled form
///   stretches `count` cells across a rect that spans MORE than `count` of them, which lands
///   the pointer up to a whole column left of where the user clicked;
/// * `count` is the pane's OWN grid — what the session gave it — never the cell span of the
///   widget holding it. The two are different numbers: the daemon divides the arbitrated
///   window in CELLS ([`sprag_terminal::tiling`]) while this client's dock divides its surface
///   in PIXELS, and the two roundings can differ by a cell. A converter clamped to the widget
///   can name a column the pane does not have, which a tracking child then receives as a mouse
///   report outside its own width.
///
/// Clamped rather than refused, because a drag past an edge lands on the edge cell — the
/// gesture is still inside the pane. An empty grid -> `0`.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is floored then clamped to `count - 1`, a u16 by construction"
)]
pub(crate) fn cell_index(offset_cells: f32, count: u16) -> u16 {
    let last = count.saturating_sub(1);
    if last == 0 || !offset_cells.is_finite() || offset_cells <= 0.0 {
        return 0;
    }
    offset_cells.floor().min(f32::from(last)) as u16
}

/// Self-create (once) the live terminal: measure the resolved monospace cell
/// via the R1003 seam, derive `(cols, rows)` from the window + cell (§3), and boot
/// the [`HostClient`] the GUI reaches its [`pane_count`] panes through.
///
/// By default (topology B) that is a [`WireHost`] — a wire client of a `sprag-term`
/// DAEMON the GUI connect-or-spawns on the well-known socket (never a child it owns; it
/// outlives the GUI, the tmux detach); its poll thread wakes the window on host output.
/// Under `SPRAG_GUI_HOST=inprocess` it is an
/// in-process [`Host`] whose panes wire their `on_dirty` straight to the shell's
/// [`RepaintSink`](pinion_core::RepaintSink) (the R23 -> R999 seam). Either way this
/// boots on first call — therefore invoked from `create_extra_externals` (boot),
/// never the pure `view`.
///
/// [`use_repaint_sink`] is resolved *before* the `Owner::cache` factory so the
/// factory never re-enters `Owner::cache` (the nested-factory guard).
pub(crate) fn use_terminal() -> Rc<TerminalView> {
    let owner = Owner::current().expect("use_terminal() requires an active Owner scope");
    // Pre-resolve the cache-backed deps BEFORE the factory (the nested-factory
    // guard): use_repaint_sink AND measured_monospace_cell both read
    // `Owner::cache` (the repaint-sink / monospace-metrics provider slots), so
    // resolving them inside the factory would re-enter and panic.
    let sink = use_repaint_sink();
    // The shell's quit edge, resolved here for the same reason as the repaint sink
    // (a cache-backed provider read, so BEFORE the factory — the nested-factory
    // guard) and handed to the wire host's poll thread: a dead daemon ends the client.
    let quit = use_quit_sink();
    let font_size_px = font_size_px();
    // R1003 view-time seam: the shell seeded the monospace-metrics provider
    // before the factories run, so this is the font the shell will paint.
    let metric = pinion_core::measured_monospace_cell(font_size_px).unwrap_or(CellMetric::DEFAULT);
    owner.cache(SESSION_KEY, move || {
        // §3: the window viewport drives the boot winsize; derive the boot (cols,
        // rows) through the same SSOT a reflow uses (grid_dims). Each pane boots
        // at the FULL window size — the honest pre-layout value, since a pane's
        // sub-rect is only known post-layout — and its per-pane R1012 reflow
        // Effect shrinks it to its tile on the first paint. No split math here:
        // computing each pane's share window-side would duplicate pinion's flex
        // resolution (the SSOT trap the per-pane `use_pane_viewport_size` avoids).
        let (cols, rows) = grid_dims((WINDOW_W, WINDOW_H), metric);
        let host: Box<dyn HostClient> = if use_inprocess_host() {
            // Escape hatch (`SPRAG_GUI_HOST=inprocess`): the Workspace lives IN the
            // GUI process. Kept for tests / debugging; NOT the default. Each pane's
            // `on_dirty` repaints the window directly (the R23 -> R999 seam).
            // The pane-hook factory is installed BEFORE the boot spawns so a pane created later
            // through the client protocol (a palette `Split into a new pane`) repaints this window
            // exactly as a boot pane does — `HostClient::new_pane` takes no arguments, so this is
            // the only place the display concern can be stated.
            let host = {
                let sink = sink.clone();
                Host::new((cols, rows)).with_pane_hooks(move || {
                    let sink = sink.clone();
                    Some(Box::new(move || sink.request_repaint()))
                })
            };
            for _ in 0..pane_count() {
                let sink = sink.clone();
                let (command, label) = pane_command();
                host.spawn(
                    command,
                    label,
                    cols,
                    rows,
                    sprag_terminal::PaneBirthHooks {
                        on_dirty: Some(Box::new(move || sink.request_repaint())),
                        // The in-process host (a test / debug escape hatch) does not self-exit: it
                        // lives with the GUI process, so no daemon reaper — and no attention router
                        // either, for the same kind of reason: nothing ATTACHES to an in-process
                        // host over a wire, so there is no client a pane's message could be
                        // addressed to. The shipped GUI is a display client of the daemon, whose
                        // panes are born wired.
                        ..sprag_terminal::PaneBirthHooks::default()
                    },
                )
                .expect("spawn a sprag-gui pane");
            }
            Box::new(host)
        } else {
            // Default (topology B): a pure wire client of a `sprag-term` DAEMON the GUI
            // connect-or-spawns on the well-known socket (detached, never owned; it survives
            // this GUI). Its panes' output repaints the window through the poll thread's
            // `on_change`.
            let sink = sink.clone();
            Box::new(
                WireHost::spawn_or_attach(
                    // A WINDOW: detaching leaves nothing to draw, so its destroy default prefers a
                    // surviving session rather than tmux's `on`. Item 282.
                    sprag_client::Frontend::Window,
                    pane_argv(),
                    cols,
                    rows,
                    pane_count(),
                    Arc::new(move || sink.request_repaint()),
                    quit,
                )
                .expect("boot the sprag-term wire host"),
            )
        };
        TerminalView {
            slots: SlotView::new(host),
            metric,
            font_size_px,
        }
    })
}

/// Whether to boot the in-process [`Host`] instead of the default wire client.
///
/// `SPRAG_GUI_HOST=inprocess` (case-insensitive) forces it; any other explicit value
/// forces the wire client. When UNSET the default splits by build: production boots
/// the [`WireHost`] (topology B), but a **unit test** boots the in-process [`Host`]
/// — a test process cannot spawn a real `sprag-term` child, and the wire path is
/// covered by the live end-to-end drive, not the unit suite. The env override wins
/// in both builds, so a test can still opt into either explicitly.
fn use_inprocess_host() -> bool {
    match std::env::var("SPRAG_GUI_HOST") {
        Ok(value) => value.trim().eq_ignore_ascii_case("inprocess"),
        Err(_) => cfg!(test),
    }
}

/// The initial pane command as a wire argv (`[program, args…]`), or `None` for the
/// host's default `$SHELL`. Reads the same [`command_spec`] as [`pane_command`] (one
/// parser), passed to the host's `--`/mux spawn; `None` lets `sprag-term` apply the
/// shared `default_shell_command` SSOT, so the shell fallback is never re-encoded here.
fn pane_argv() -> Option<Vec<String>> {
    command_spec().map(|(program, args)| {
        let mut argv = Vec::with_capacity(args.len() + 1);
        argv.push(program);
        argv.extend(args);
        argv
    })
}

/// Seed the terminal cache (test-only) with a caller-controlled [`Host`], so
/// [`use_terminal`] returns panes the test OWNS (e.g. deterministic `cat` panes)
/// instead of spawning `$SHELL`. Must be called inside an [`Owner`] scope BEFORE the
/// first `use_terminal()` — the [`Owner::cache`] slot at `SESSION_KEY` is then
/// populated, so `use_terminal` returns this instead of running its spawn factory.
/// This is the headless seam input-routing tests use to drive `apply_key` /
/// `apply_composition` end-to-end and assert the bytes reach the intended pane's PTY.
#[cfg(test)]
pub(crate) fn seed_terminal(host: Host) {
    let owner = Owner::current().expect("seed_terminal() requires an active Owner scope");
    owner.cache(SESSION_KEY, || TerminalView {
        slots: SlotView::new(Box::new(host)),
        metric: CellMetric::DEFAULT,
        font_size_px: FONT_SIZE_PX,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The drift guard [`FONT_SIZE_PX`] names: the registry's `gui-font` default and this crate's
    /// fallback are one number spelled twice, and only this holds them together.
    ///
    /// A disagreement would be invisible in every ordinary run — the option is always answered, so the
    /// fallback is only reached if the registry cannot answer — and would then boot a window at a size
    /// `sprag show-options` does not print. The trim / zero-rejection / malformed rules this test used
    /// to check are now `OptionKind::Number`'s, tested where the validation lives.
    #[test]
    fn the_registry_default_is_this_crates_own() {
        let spec = sprag_host::options::spec(sprag_host::options::GUI_FONT)
            .expect("gui-font is a registered option");
        assert_eq!(
            spec.default,
            FONT_SIZE_PX.to_string(),
            "the registry's gui-font default must be this crate's fallback",
        );
    }

    /// The option in force is what the glyph size follows, including the default when the user is
    /// silent — the mapping [`font_size_px`] performs, driven without touching the environment.
    #[test]
    fn the_glyph_size_follows_the_option() {
        let mut options = sprag_host::options::Options::default();
        assert_eq!(
            options.number(sprag_host::options::GUI_FONT),
            Some(FONT_SIZE_PX),
            "a silent user gets this crate's own size",
        );
        options
            .set(sprag_host::options::GUI_FONT, "31")
            .expect("31 is a size");
        assert_eq!(options.number(sprag_host::options::GUI_FONT), Some(31));
    }

    #[test]
    fn split_command_parses_program_and_args() {
        assert_eq!(split_command(""), None);
        assert_eq!(split_command("   "), None);
        assert_eq!(split_command("vim"), Some(("vim".to_owned(), Vec::new())));
        assert_eq!(
            split_command("ls -la /usr/bin"),
            Some((
                "ls".to_owned(),
                vec!["-la".to_owned(), "/usr/bin".to_owned()]
            )),
        );
    }

    #[test]
    fn parse_pane_count_clamps_and_falls_back() {
        assert_eq!(
            parse_pane_count(None, PANE_COUNT_DEFAULT),
            PANE_COUNT_DEFAULT
        );
        assert_eq!(parse_pane_count(Some("3"), PANE_COUNT_DEFAULT), 3);
        assert_eq!(parse_pane_count(Some(" 1 "), PANE_COUNT_DEFAULT), 1); // trims
        assert_eq!(
            parse_pane_count(Some("0"), PANE_COUNT_DEFAULT),
            PANE_COUNT_DEFAULT
        ); // zero rejected
        assert_eq!(
            parse_pane_count(Some("huge"), PANE_COUNT_DEFAULT),
            PANE_COUNT_DEFAULT
        ); // malformed
        assert_eq!(
            parse_pane_count(Some(""), PANE_COUNT_DEFAULT),
            PANE_COUNT_DEFAULT
        );
        // Clamped to MAX_PANES (no out-of-table pane tag can be requested).
        assert_eq!(parse_pane_count(Some("999"), PANE_COUNT_DEFAULT), MAX_PANES);
    }

    #[test]
    fn pane_tag_round_trips_through_pane_index_of() {
        for i in 0..MAX_PANES {
            assert_eq!(
                pane_index_of(pane_tag(i)),
                Some(i),
                "pane_tag/pane_index_of are inverses"
            );
        }
        assert_eq!(
            pane_index_of("sprag_gui"),
            None,
            "the root tag is not a pane"
        );
        assert_eq!(pane_index_of("nope"), None);
    }

    #[test]
    fn pane_slot_tags_match_their_namespace_and_index() {
        // The structural guard that replaced the old parallel hand-typed arrays:
        // each slot's tags must be EXACTLY `sprag_gui.<ns>.<i>`. Asserting the full
        // string (not just the `.<i>` suffix) also catches a prefix typo
        // (`scrollbor`) and a duplicated/misordered row, which a suffix-only check
        // would miss. The runtime cache-key axis (pane_cache_key) is checked too.
        for i in 0..MAX_PANES {
            assert_eq!(pane_tag(i), format!("sprag_gui.pane.{i}").as_str());
            assert_eq!(
                pane_scrollbar_tag(i),
                format!("sprag_gui.scrollbar.{i}").as_str()
            );
            assert_eq!(pane_scroll_key(i), format!("sprag_gui.scroll.{i}").as_str());
            assert_eq!(
                pane_cache_key("preedit", i),
                format!("sprag_gui.preedit.{i}")
            );
        }
        // The three &'static columns are distinct namespaces (no Owner::cache
        // collision between the scrollbar interaction signal and the ScrollState).
        assert_ne!(pane_tag(0), pane_scrollbar_tag(0));
        assert_ne!(pane_scrollbar_tag(0), pane_scroll_key(0));
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
}
