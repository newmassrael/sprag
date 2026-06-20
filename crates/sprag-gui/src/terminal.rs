//! The booted terminal model, its config (font + command), and the winsize
//! derivation — everything about *creating and holding* the live pane, spawned
//! at boot off the pure `view`. See the crate-root module docs for the seams.

use crate::{WINDOW_H, WINDOW_W};
use pinion_core::CellMetric;
use pinion_core::reactive::Owner;
use pinion_core::use_repaint_sink;
use sprag_terminal::{CommandBuilder, Pane, SessionHandle, Workspace};
use std::rc::Rc;

/// Default glyph size (logical px) — the font-size SSOT the cell is measured
/// from. `SPRAG_GUI_FONT=<px>` overrides it live (larger ⇒ bigger).
const FONT_SIZE_PX: u32 = 20;

/// `Owner::cache` key for the live terminal (created once at boot).
const SESSION_KEY: &str = "sprag_gui.terminal";

/// The maximum number of tiled panes. The per-pane focus tags must be
/// `&'static str` (the [`WidgetCore::focusable_tags`](pinion_core::WidgetCore::focusable_tags)
/// / [`WidgetCore::tag`](pinion_core::WidgetCore::tag) contract), so they come
/// from this fixed table rather than being minted at runtime. A windowed
/// terminal tiling more than a handful of panes is not useful (each pane shrinks
/// toward unreadable), so a small cap is the honest bound, not a limitation to
/// design around. Dynamic pane creation (a deferred round) keeps this cap; an
/// unbounded count would need a pinion runtime-focusable-refresh seam.
pub(crate) const MAX_PANES: usize = 8;

/// The per-pane identity tags (`sprag_gui.pane.<i>`), one per tile up to
/// [`MAX_PANES`]. The SINGLE source of pane tags, shared by the model-scene input
/// Externals ([`create_external`](crate::TerminalViewer) /
/// [`create_extra_externals`](crate::TerminalViewer)), the
/// [`focusable_tags`](pinion_core::WidgetCore::focusable_tags) enumeration +
/// focus, the paint-scene pane Containers ([`sprag_host::pane_view_scene`] — the
/// R1012 [`use_pane_viewport_size`](pinion_core::use_pane_viewport_size) rect
/// target + focus ring + click anchor), and the per-pane reflow Effects. One
/// string per pane across all of those, so input / focus / measure / paint can
/// never address different panes.
pub(crate) const PANE_TAGS: [&str; MAX_PANES] = [
    "sprag_gui.pane.0",
    "sprag_gui.pane.1",
    "sprag_gui.pane.2",
    "sprag_gui.pane.3",
    "sprag_gui.pane.4",
    "sprag_gui.pane.5",
    "sprag_gui.pane.6",
    "sprag_gui.pane.7",
];

/// The identity tag of the pane at tile `index` (`index < `[`MAX_PANES`]).
pub(crate) fn pane_tag(index: usize) -> &'static str {
    PANE_TAGS[index]
}

/// The tile index of the pane whose identity tag is `tag`, or `None` if `tag` is
/// not a pane tag (a non-pane / absent focus). The inverse of [`pane_tag`], so
/// input routing maps the focused tag back to its pane.
pub(crate) fn pane_index_of(tag: &str) -> Option<usize> {
    PANE_TAGS.iter().position(|&t| t == tag)
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
/// overrides the default of [`PANE_COUNT_DEFAULT`]. Env-read (no `Owner` scope),
/// so it is the count both the boot spawn ([`use_terminal`]) and the static
/// [`focusable_tags`](pinion_core::WidgetCore::focusable_tags) enumeration share —
/// they agree because they read the one source.
pub(crate) fn pane_count() -> usize {
    parse_pane_count(
        std::env::var("SPRAG_GUI_PANES").ok().as_deref(),
        PANE_COUNT_DEFAULT,
    )
}

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
pub(crate) struct TerminalView {
    pub(crate) workspace: Workspace,
    pub(crate) metric: CellMetric,
    pub(crate) font_size_px: u32,
}

impl TerminalView {
    /// The number of tiled panes (== [`pane_count`] at boot; the panes are
    /// spawned once in [`pane_tag`] order and not closed this round).
    pub(crate) fn pane_count(&self) -> usize {
        self.workspace.panes().len()
    }

    /// The pane at tile `index` (`index < `[`Self::pane_count`]). This is the
    /// **one** place the "which pane?" question is answered: `view` /
    /// `access_node` / `scroll_view` / input routing / the reflow Effect all
    /// resolve a pane through here (replacing the single-pane `boot_pane`), so
    /// pane selection lives in one site. The boot panes are spawned in
    /// [`pane_tag`] order and never closed this round, so `index` (sourced from a
    /// [`pane_tag`] / focus tag) is a hard in-range invariant, not an `Option`.
    pub(crate) fn pane(&self, index: usize) -> &Pane {
        self.workspace
            .panes()
            .get(index)
            .expect("pane index in range (boot panes spawned 0..pane_count, never closed)")
    }

    /// Pane `index`'s cloneable I/O handle (its input engine + reflow seam).
    pub(crate) fn pane_handle(&self, index: usize) -> SessionHandle {
        self.pane(index).handle()
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
pub(crate) fn grid_dims(viewport: (u32, u32), metric: CellMetric) -> (u16, u16) {
    let (width, height) = viewport;
    (
        metric.cols_for(width).max(1),
        metric.rows_for(height).max(1),
    )
}

/// The inverse of [`grid_dims`]: the pixel size that exactly fits `(cols, rows)`
/// cells at `metric`. The one `cols -> px` site (a fixed-size undock window's
/// intrinsic size, [`dock`](crate::dock)), kept beside the `px -> cols`
/// derivation so the cell<->pixel round-trip lives in one module.
pub(crate) fn cell_px(metric: CellMetric, cols: u16, rows: u16) -> (u32, u32) {
    (
        u32::from(cols) * metric.cell_w(),
        u32::from(rows) * metric.cell_h(),
    )
}

/// Self-create (once) the live terminal: measure the resolved monospace cell
/// via the R1003 seam, derive `(cols, rows)` from the window + cell (§3), and
/// spawn the [`pane_count`] tiled panes, each wired to the shell's
/// [`RepaintSink`](pinion_core::RepaintSink) (the R23 `on_dirty` -> R999 seam) so
/// any pane's output wakes the window. Spawns the PTYs on first call — therefore
/// invoked from `create_extra_externals` (boot), never the pure `view`.
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
        let mut workspace = Workspace::new((cols, rows));
        for _ in 0..pane_count() {
            let sink = sink.clone();
            let (command, label) = pane_command();
            workspace
                .spawn_with_dirty(
                    command,
                    label,
                    cols,
                    rows,
                    Some(Box::new(move || sink.request_repaint())),
                )
                .expect("spawn a sprag-gui pane");
        }
        TerminalView {
            workspace,
            metric,
            font_size_px,
        }
    })
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
