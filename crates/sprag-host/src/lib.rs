//! sprag-host — the headless host layer.
//!
//! Assembles the producer's terminal panes into a pinion scene and exposes
//! them as scene-as-data. The scene is a workspace `Scene::Container` of N
//! pane children plus one control child:
//!
//! * each **pane** is a `Scene::Container` of the R1.7 data/engine split — a
//!   `Scene::TextGrid` (the cell grid an AI reads via `scene/snapshot` /
//!   `scene/query`) and a `Scene::External` ([`SpragPaneExternal`]) whose
//!   `scene/invoke` action channel injects input (key→PTY-byte encoding is
//!   sprag-owned — PINION-REQUIREMENTS R2.6); and
//! * the **workspace control** `Scene::External` ([`WorkspaceExternal`]),
//!   whose `scene/invoke` actions spawn / close / resize panes (the Round 7
//!   headless multiplex control core).
//!
//! This is the layer that owns the pinion-rpc dependency (the RPC dispatch
//! and the `scene/snapshot` wire), keeping the producer ([`sprag_terminal`])
//! and projection ([`sprag_grid`]) crates free of its heavy transitive deps.
//!
//! Pipeline (DESIGN.md §5): the producer's [`Workspace`](sprag_terminal::Workspace)
//! holds the panes (those of the current window of the session a request is SCOPED to,
//! resolved out of the [`SessionRegistry`] — see [`SessionScope`]);
//! [`workspace_scene`] assembles the tree (handing engines their `PanePtyHandle`, and
//! refreshing each grid from its live screen only when the request can READ one —
//! [`PaneCells`]); [`snapshot`] reads one
//! grid back as the [`TextGridSnapshot`] an AI consumer sees; [`dispatch_frames`]
//! runs the single-owner JSON-RPC dispatch loop (R104).
//!
//! ## Why this is GPU-free (DESIGN.md §5 host-viability risk)
//!
//! `pinion_shell::run` opens a winit/Vello window; the platform's reason to
//! exist, though, is that AI reads the screen as *data*, not pixels
//! (DESIGN.md §1). `pinion-rpc` carries no GPU dependency, so this whole
//! path runs headlessly. Only the (deferred, paint-opaque) human rendering
//! would need a GPU.
//!
//! ## Winsize ownership (DESIGN.md §3)
//!
//! §3 makes the layout-resolved pixel `rect` the winsize SSOT for a *GUI*
//! host: layout fills the rect, pinion derives `(cols, rows)`, the producer
//! adopts them. A headless host has no layout pass, so that derivation does
//! not apply: the producer/consumer specifies the size, the emulator and PTY
//! are sized to it, and the authoritative `(cols, rows)` live in the
//! GridBuffer — reported by [`TextGridSnapshot::buffer_cols`] /
//! [`TextGridSnapshot::buffer_rows`]. [`scene`] therefore leaves the node's
//! GUI `rect` unset; its rect-derived `cols` / `rows` stay `0` here by
//! design (not faked to mirror the buffer). A future windowed host fills the
//! rect via `pinion_runtime::compute_layout`.

pub mod agent;
pub mod attach;
pub mod attention;
pub mod chooser;
pub mod config;
/// Asking systemd for the cgroup subtree a pane's share is enforced in (R336). Linux-only: it is
/// the platform that has cgroups, and the crate that has the bus client.
#[cfg(target_os = "linux")]
pub mod delegation;
pub mod drive;
pub mod durability;
pub mod events;
mod external;
pub mod history;
pub mod hooks;
pub mod host;
pub mod job;
pub mod keyhelp;
pub mod keymap;
/// **Debt 64c's measurement**: the ai-loop path driven against a LIVE agent CLI, through this
/// crate's own detector. Opt-in and test-only — it costs an agent's real turns to run.
#[cfg(test)]
mod live_agent;
pub mod notify;
pub mod options;
pub mod outward;
pub mod pane;
pub mod pane_address;
pub mod plugins;
pub mod project;
pub mod prompt;
// The pane surface this daemon SERVES, read from outside its process — the seam a run driver needs
// in order to live somewhere else (register item 544).
pub mod remote_access;
pub mod report;
pub mod rpc;
pub mod runs;
pub mod scope;
// A plain `//` (not `///`): an outer doc comment on a `mod` merges with the module's own `//!`
// docs and resolves THEIR intra-doc links in this scope, which manufactures phantom errors.
pub mod shellword;
pub mod ssh;
pub mod status;
pub mod sweep;
// Every verb the product has, named ONCE — the table the CLI dispatches through, the help is built
// from, and the keyboard's own list of forms is derived from. Written because those three were three
// hand-written lists with nothing holding any two together (R323).
pub mod vocabulary;
// Dropped-file delivery (paste a local path / `scp` it to a remote workspace first). Private: the
// policy is reached ONLY through the `drop_file` action + `HostClient::drop_file`, so there is one
// way in and no second policy site. A plain comment, not a doc comment: an outer doc here would be
// merged with the module's own `//!` docs and resolve THEIR intra-doc links in this scope instead.
mod upload;
// What the host hands a client on a WAKE, as ONE closed set taken through one door — written
// because a per-wake duty kept only in each front's own loop is a duty one front forgets, which is
// what `detach-on-destroy`'s four switch policies did on `sprag-tui` for as long as they existed
// (R326).
pub mod wake;
pub mod window;
pub mod wire;
pub mod workspace;
pub use agent::{AgentClock, AgentFacts, AgentRegistry};
pub use attach::{
    AttachOutcome, AttachmentRegistry, Audience, ClientId, ClientInfo, ClientSize, Delivery,
    SizeOutcome,
};
pub use config::{CONFIG_FILE, ConfigError, UserConfig};
pub use durability::{
    gui_window_path, load_gui_window, load_runs, load_snapshot, reconnect_command,
    restore_allowlist, restore_command, runs_path, save_gui_window_if_changed, save_if_changed,
    save_runs_if_changed, save_snapshot, snapshot_path,
};
pub use events::{Batch, Event, EventLog, Record};
pub use history::{
    DEFAULT_HISTORY_IMAGE_BYTES, NO_HISTORY_CEILING, SavedHistory, history_dir, history_file_pane,
    history_image_bytes, history_limit, history_limits, load_pane_history, purge_histories,
    save_histories_if_changed,
};
pub use hooks::{HookError, Outcome, Plan, Target, plan_install, plan_uninstall, report_for};
pub use host::{
    Host, HostClient, NESTED_AGENT_MARKERS, PANE_ENV_VAR, PaneAgent, PaneClipboardQuery,
    PaneClipboardWrite, PaneFind, PaneFrame, PaneMatch, PaneNotification, PaneScrollFacts,
    pane_args_source, pane_env_source, pane_identity_source,
};
pub use job::JobWatch;
pub use keymap::{BoundAction, KeyError, KeySpec, Keymap};
pub use notify::{
    ChannelRegistry, OutputChannel, OutputQuery, OutputSignal, PaneWaitChannel, RevisionChannel,
};
pub use options::{OPTIONS, OptionError, OptionKind, OptionSpec, Options};
pub use pane::{CellFrame, SpragPaneExternal, focus, mouse, paste, send_key, send_text};
pub use plugins::PluginsExternal;
pub use project::{PROJECT_FILE, Project, ProjectAction, ProjectError};
pub use rpc::{
    BirthPin, FrameIngress, HostState, IngressEvent, SUPPORTED_METHODS, bump_on_dirty,
    dispatch_channel, dispatch_frames, handle_parsed, handle_request, pane_attention_hook,
    pane_exit_hook, spawn_reaper, stdin_frames,
};
pub use runs::{RunId, RunRegistry, RunState};
pub use scope::{ScopeError, SessionScope};
pub use ssh::{PortForward, SshTarget, SshTargetError};
pub use sweep::{SweepReport, sweep_once};
pub use window::{WindowSize, arbitrate};
pub use wire::{mux_action_path, pane_container_tag, pane_input_path, plugins_path};
pub use workspace::WorkspaceExternal;

use std::borrow::Cow;
use std::fmt;
use std::sync::{Arc, Mutex};

use pinion_core::scene::{ContainerNode, ExternalNode, TextGridNode};
use pinion_core::style::{LayoutStyle, Size, SizeValue};
use pinion_core::{CellMetric, GridBuffer, Scene};
use sprag_terminal::{PaneId, PanePty, SessionRegistry};
use sprag_vt::{Palette, Screen};

use crate::external::lock;

/// What a DAEMON shares into a scene assembly, and an in-process host does not have.
///
/// Every OPTIONAL field is `None` for a non-daemon (the sampler is not one of them — see its own
/// note), and it is worth naming who those actually are, because the
/// older comments around this file say "the GUI's in-process host" and no such host exists: nothing
/// outside this crate builds a [`HostState`]. The real non-daemon owners are `sprag-latency` — which
/// measures the pane list and must not have a detector it did not ask for landing in the instrument —
/// and the two test harnesses.
///
/// They travel together because the mistake they invite is the same one: a surface wired with some of
/// them and not others answers plausibly and behaves differently from the daemon it is standing in
/// for. [`none`](Self::none) is the honest way to say "not a daemon" once.
#[derive(Clone, Default)]
pub struct DaemonShared {
    /// The self-cleaning daemon's pane-`on_exit` death-signal hook ([`spawn_reaper`]), wired into
    /// every pane a scene surface spawns so its death feeds the reaper.
    pub on_pane_exit: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Per-client session attachment, read when the `sessions` slot is served to fill each
    /// `SessionInfo::attached`. `None` leaves every count at 0 — an honest "no wire clients here".
    pub attachments: Option<Arc<Mutex<AttachmentRegistry>>>,
    /// The daemon's attention ROUTER ([`attention::AttentionRouter`]), asked for a fresh hook by every
    /// pane a scene surface spawns, so a child asking for a person reaches the people looking at the
    /// session that holds it. `None` off a daemon: an in-process host has no wire clients to address,
    /// which is the same reason [`Self::attachments`] is optional — and the two are the pair that must
    /// be present together, because a router with nowhere to deliver would run a thread to reach
    /// nobody.
    ///
    /// The ROUTER and not one shared closure, and the difference is WHERE A LOCK IS TAKEN: a hook is
    /// minted per BIRTH with its own channel sender, so the PTY reader thread that runs it takes none.
    /// See [`attention::AttentionRouter::signal`].
    pub attention: Option<Arc<attention::AttentionRouter>>,

    /// The agent-state memory (H3), read by the pane list. `None` leaves the `agent` key absent,
    /// which D8 already defines as "no agent here", so a host without a detector serves the pre-H3
    /// wire shape rather than a wrong answer.
    ///
    /// Whoever sets this must also drive the settle waker — see [`AgentClock`] and `sprag-term`.
    pub agents: Option<Arc<AgentClock>>,
    /// The host's [samplers](Samplers), read when a sampled slot is served.
    ///
    /// NOT an `Option`, unlike its neighbours, and the difference is the point: those three are
    /// facts only a daemon HAS (a reaper, wire clients, a detector), so a host without one states
    /// `None` and the wire answers honestly that there is nothing to report. A sampler is not like
    /// that — every host can answer where its sessions are working and what its panes are running —
    /// so making it optional would be inventing a mode in which the question has no answer. What a
    /// caller passes here decides only WHO SHARES the samples: the set taken from
    /// [`Host::samplers`] is the whole host's, and a fresh one is nobody else's.
    pub samplers: Samplers,
    /// ⚠⚠⚠⚠⚠ **THE RUNS THIS DAEMON IS DRIVING**, read when the pane list is served so a pane can
    /// answer *is anybody driving me* — register items 595 and 602.
    ///
    /// # Why the PANE list needs the RUN directory
    ///
    /// A daemon restart re-runs an allowlisted agent's argv, so a `claude` pane comes back holding
    /// its old conversation **with nothing driving it** ([`durability::restore_command`]). On the
    /// screen that is indistinguishable from a loop that is working — both are a `claude` prompt —
    /// and it was measured three times in one day, ending with `sprag runs` reporting **no running
    /// run** while three `claude` processes sat there.
    ///
    /// Register item 540 gave the RUN side of the answer (`driving`, a pane id a program can read).
    /// This field is what lets the PANE side use it, and it is here rather than passed separately
    /// because [`DaemonShared`] is exactly the bundle of *things only a daemon has* that this
    /// surface is already built from — adding a parameter would have changed six call sites to say
    /// the same thing.
    ///
    /// ⚠⚠ **THE JOIN HAS ONE HOME, AND THAT IS THE WHOLE REASON THIS IS NOT DONE IN THE MOUTHS.**
    /// `sprag panes` and the agent-facing `list_panes` could each fetch `runs` and match ids, and
    /// then there would be two readers deriving one fact — the defect this crate has paid for
    /// often enough to name. The host answers it once.
    ///
    /// ⚠ [`None`] off a daemon, like its optional neighbours: an in-process host with no run
    /// directory leaves the key absent, which is *nothing here drives panes* rather than a wrong
    /// answer.
    pub runs: Option<Arc<Mutex<runs::RunRegistry>>>,
    /// **HOW THIS IMAGE STARTS A RUN'S DRIVER IN A PROCESS OF ITS OWN**, for a given session — or
    /// [`None`] for a host that cannot be one. Register item 544.
    ///
    /// # ⚠⚠⚠⚠⚠ Why the ABILITY is injected and not decided in here, measured the hard way
    ///
    /// [`driver_spawn`] starts [`std::env::current_exe`] with `--drive`, which is correct exactly
    /// when the running image is `sprag-term` — *"the driver is this daemon's image by
    /// construction"*. This crate is linked into images that are NOT: the windowed client, and
    /// every test binary that builds an in-process host. While
    /// [`options::RUN_DRIVER_PROCESS`] defaulted to `off` that was invisible; the day it flipped,
    /// **eighteen tests failed** because a host was spawning its own test harness with `--drive`
    /// and waiting for it to drive a run.
    ///
    /// ⚠⚠ So *can this image be a driver* is a fact only the image knows, and it says so by handing
    /// one of these in ([`rpc::HostState::driving_out_of_process`]). The OPTION is still what
    /// decides whether to use the ability — a daemon told `off` provides none — but an image that
    /// cannot drive can no longer be asked to, whatever any config file says.
    ///
    /// ⚠ A FACTORY over the session rather than a spawner, because a driver is told which session
    /// to scope its waits to (`-t`) and the scope is per request, while this is built once at boot.
    pub spawn_driver: Option<DriverSpawns>,
}

/// The host's SAMPLED facts, one sampler each, shared by every arm that serves them.
///
/// A set rather than a field per fact, because they are the same KIND of thing and are always
/// injected together: every one of them holds a reading between reads, and every one of them must
/// be the HOST's rather than minted per request — a sampler built where a scene is assembled holds
/// nothing at the moment it is asked, so each request would take its own walk and the coalescing
/// this design exists for would be silently absent while every individual answer still looked right.
///
/// Cloning a `Samplers` shares the samplers (each is an `Arc`); it does not copy the held readings.
#[derive(Clone, Default)]
pub struct Samplers {
    /// Where each session is working, on what, and what it is serving (R282).
    pub activity: Arc<sprag_terminal::ActivitySampler>,
    /// What each pane is RUNNING — the job that owns its terminal (R290).
    pub processes: Arc<sprag_terminal::PaneProcessSampler>,
    /// What each pane is TAKING of the machine — cores held, waiting, memory, processes (R338).
    ///
    /// Shared like its neighbours, and for one reason more than they have: it is the only sampler
    /// here that keeps a BASELINE, so two of them would answer one pane two different rates at one
    /// instant — each right about its own history and useless for the comparison the number exists
    /// to serve.
    pub resources: Arc<sprag_terminal::PaneResourceSampler>,
}

impl DaemonShared {
    /// Not a daemon: no reaper hook, no attachment map, no detector — and a sampler of its own,
    /// shared with nothing, since there is no other arm here to share it with.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }
}

impl fmt::Debug for DaemonShared {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DaemonShared")
            .field("on_pane_exit", &self.on_pane_exit.is_some())
            .field("attachments", &self.attachments.is_some())
            .field("agents", &self.agents.is_some())
            .finish()
    }
}

// Re-export the snapshot shapes a consumer reads, so downstream code need
// not depend on pinion-rpc's module layout directly.
pub use pinion_rpc::snapshot::{GridRowSnapshot, GridStyleRun, TextGridSnapshot};

/// The intent tag on the workspace's root `Scene::Container`.
pub const WORKSPACE_TAG: &str = "sprag_workspace";

/// The tag on the workspace-control `External` (pane management). The
/// `scene/invoke` path addresses it as `/sprag_mux/external/<action>`.
pub const MUX_TAG: &str = "sprag_mux";

/// The tag on the plugin-host `External`. The `scene/invoke` path addresses it
/// as `/sprag_plugins/external/run`.
pub const PLUGINS_TAG: &str = "sprag_plugins";

/// The tag on each pane's `TextGrid` child (the cell-data projection).
pub const GRID_TAG: &str = "sprag_grid";

/// The tag on each pane's input `External` child. The `scene/invoke` input
/// path addresses pane `<id>` as `/pane_<id>/sprag_input/external/key` (R2.6).
pub const INPUT_TAG: &str = "sprag_input";

/// The tagged `TextGrid` node carrying a pre-projected cell buffer — the
/// node-shape SSOT (metric + cells), shared by the headless data path
/// ([`text_grid_node`], tagged [`GRID_TAG`]) and the GUI view path
/// ([`view_text_grid`], tagged a per-pane `{pane}#grid` composite so a click
/// resolves to the focusable pane). The `tag` is the caller's because it differs by
/// path; the projection itself is [`sprag_grid::project`] /
/// [`sprag_grid::project_scrolled`]; this assembles the node around whatever cells
/// the caller projected.
///
/// # Every sprag grid states that sprag sized it (PINION-PR80, pinion R1542)
///
/// A `TextGridNode`'s `(cols, rows)` are derived from its layout-resolved `rect` unless the node
/// DECLARES them. Deriving is right when the layout is what sizes the producer. In sprag it never
/// is: the daemon tiles a session's arbitrated window in CELLS
/// ([`sprag_terminal::tile`]) and that division is both the PTY's
/// `TIOCSWINSZ` and this buffer's size, while a display client lays the panes out in PIXELS. One
/// boundary quantised twice leaves some pane a cell of slack, so no rect can carry both meanings —
/// measured, in the steady state, as a pane painting 38 columns over a 37-column buffer.
///
/// So the declaration is UNCONDITIONAL rather than per-path: it is not a fact about this call
/// site, it is a fact about sprag. A client reading `winsize_source` off any sprag grid finds
/// `"producer"`, and `cols == buffer_cols` is the steady state — which is what pinion's own
/// interpretation rule says a snapshot means, and what sprag's grids could not say before.
///
/// The one caller holding a placeholder buffer ([`PaneCells::Omitted`], a `0 x 0` grid no
/// supported method can reach — see `rpc::pane_cells_for`) declares `0 x 0` with it, so the two
/// modes still differ in cell content alone.
fn grid_node(
    tag: impl Into<Cow<'static, str>>,
    metric: CellMetric,
    cells: GridBuffer,
) -> TextGridNode {
    let (cols, rows) = (cells.cols(), cells.rows());
    TextGridNode::new(metric)
        .with_tag(tag)
        .with_cells(cells)
        .with_winsize(cols, rows)
}

/// Project a [`Screen`] into the bare `TextGrid` node carrying the cell data
/// (the headless data path: [`scene`] / [`pane_container`]). The one call site
/// of [`sprag_grid::project`] for the data path, so the cell projection has one
/// authority. The node carries no layout: the headless path leaves the GUI
/// `rect` unset, and since PINION-PR80 that costs a reader nothing — the node
/// DECLARES its winsize ([`grid_node`]), so `cols`/`rows` answer the daemon's
/// size rather than deriving `0` from the unset rect, and they agree with
/// [`TextGridSnapshot::buffer_cols`] / `buffer_rows`. The GUI seam adds layout +
/// font size via [`view_text_grid`].
pub(crate) fn text_grid_node(
    screen: &Screen,
    palette: &Palette,
    metric: CellMetric,
) -> TextGridNode {
    grid_node(GRID_TAG, metric, sprag_grid::project(screen, palette))
}

/// The per-pane cell DATA a display client reads to paint a pane — the host side
/// of the topology-B wire contract. Projects the pane's live `screen` scrolled by
/// `offset_lines` rows of history into the paint-authoritative [`GridBuffer`],
/// which is serde-able since PINION-PR49 (R106), so it crosses the wire
/// byte-for-byte — proven by `wire_round_trip_preserves_the_pane_cell_buffer`.
///
/// This is the PROJECTION half of what used to be an all-in-one pane builder. The
/// ASSEMBLY half is [`pane_view_scene_from_cells`], and the IME `preedit` overlay
/// ([`sprag_grid::overlay_preedit`]) is a CLIENT-local step (an uncommitted
/// composition that never reaches the PTY). Topology B splits the three along the
/// wire boundary: the host PROJECTS (here), the client OVERLAYS its preedit and
/// ASSEMBLES the node. The in-process GUI calls this directly on its own pane pool;
/// the end-state wire client reads the serialized `GridBuffer` off the host over
/// RPC and skips this. The `with_screen` lock is released when this returns (the
/// owned buffer needs none), so the client's overlay + assembly run lock-free —
/// unlike the old builder, which held the lock across the whole assembly.
///
/// `offset_lines == 0` is the live view; `n` windows `n` rows up into scrollback
/// ([`sprag_grid::project_scrolled`], which self-clamps to the retained depth and
/// drops the cursor while scrolled — so a client `overlay_preedit` self-gates off
/// a history view with no extra check).
#[must_use]
pub fn pane_cells(pty: &PanePty, offset_lines: usize) -> GridBuffer {
    pty.with_screen_palette(|screen, palette| {
        sprag_grid::project_scrolled(screen, offset_lines, palette)
    })
}

/// Assemble ONE pane's `Scene::Container` from an already-projected cell buffer
/// — the **Screen-free** pane-node SSOT the display client assembles a pane node
/// from (the in-process GUI today; the wire client at the transport step).
///
/// The node is a tagged, **focus-stop** Container wrapping the pane's grid. The
/// Container's only own layout is the pinion R1020 `focusable` flag
/// ([`LayoutStyle::with_focusable`](pinion_core::style::LayoutStyle::with_focusable)),
/// declaring the pane a scene-derived keyboard Tab stop (pinion §5.39). Its SIZE/FLEX
/// come from the GUI's arrangement (`view_splitter` drag ratio for a tiled pane;
/// `Percent(100)` for a lone / undocked pane); those mutators edit the size/flex
/// fields in place, preserving the `focusable` flag set here. The inner grid
/// stays NON-focusable, so each pane is exactly one Tab stop.
///
/// The grid child carries the COMPOSITE `{tag}#grid` sub-tag: a pointer press
/// lands on the grid (the deepest tagged node under the cursor) and pinion's
/// click-to-focus resolves a `primary#sub` tag back to `primary` (via
/// `resolve_focusable` / `composite_tag::split_subindex`) — the focusable pane.
/// A plain shared tag (e.g. the headless [`GRID_TAG`]) resolves to nothing, so
/// the click would NOT move focus (the live bug: clicking a pane kept typing in
/// the previously-focused one) and is ambiguous across panes.
///
/// **This is the seam the topology-B display client assembles the pane node from.**
/// The in-process GUI
/// reaches it by projecting its PTY's screen via [`pane_cells`] and overlaying
/// its IME preedit ([`sprag_grid::overlay_preedit`]) first; the end-state wire
/// client feeds cells it reconstructed off the host's served data model — both
/// build the byte-identical pane node without this assembly ever touching a live
/// `Screen`. (The host's own RPC data path uses a different container —
/// `pane_container`, input `External` embedded — see [`workspace_scene`].)
#[must_use]
pub fn pane_view_scene_from_cells(
    tag: impl Into<Cow<'static, str>>,
    cells: GridBuffer,
    metric: CellMetric,
    font_size_px: u32,
) -> Scene {
    let tag = tag.into();
    let grid_tag = format!("{tag}#grid");
    Scene::Container(
        ContainerNode::new(vec![view_text_grid(cells, grid_tag, metric, font_size_px)])
            .with_tag(tag)
            .with_layout(LayoutStyle::new().with_focusable(true)),
    )
}

/// Assemble the windowed-host `Scene::TextGrid` from a pre-projected cell buffer
/// — the GUI presentation (R1002 font-size pin + fill layout) shared by every pane,
/// so the node shape lives in one place. `tag` is the per-pane `{pane}#grid`
/// composite ([`pane_view_scene_from_cells`]). It fills its pane Container (both axes
/// `Percent(100)`), so the grid spans whatever sub-rect the flex split resolved for
/// that pane.
fn view_text_grid(
    cells: GridBuffer,
    tag: impl Into<Cow<'static, str>>,
    metric: CellMetric,
    font_size_px: u32,
) -> Scene {
    Scene::TextGrid(
        grid_node(tag, metric, cells)
            .with_font_size_px(font_size_px)
            .with_layout(
                LayoutStyle::new().with_size(
                    Size::auto()
                        .with_width(SizeValue::Percent(100))
                        .with_height(SizeValue::Percent(100)),
                ),
            ),
    )
}

/// Assemble a [`Screen`] into a bare `Scene::TextGrid` (the cell-data view),
/// using the 8×16 baseline cell metric ([`CellMetric::DEFAULT`]).
///
/// This is the pure data projection — a function of the screen alone, with
/// no live pty — used by [`snapshot`] and data-only consumers. The RPC
/// server assembles the full pane tree via [`workspace_scene`]; the windowed
/// host builds per-pane scenes via [`pane_view_scene_from_cells`] (from cells it
/// reads through [`pane_cells`]) and arranges them GUI-side.
#[must_use]
pub fn scene(screen: &Screen, palette: &Palette) -> Scene {
    scene_with_metric(screen, palette, CellMetric::DEFAULT)
}

/// [`scene`] with an explicit cell metric.
#[must_use]
pub fn scene_with_metric(screen: &Screen, palette: &Palette, metric: CellMetric) -> Scene {
    Scene::TextGrid(text_grid_node(screen, palette, metric))
}

/// Whether an assembled scene's panes carry their projected cells.
///
/// Projecting a pane's grid is the most expensive thing an assembly does — one whole-screen
/// walk per pane, metered by [`sprag_grid::work`] — and [`workspace_scene`] used to pay it for
/// every request whatever the request was going to read. It is worth paying only when the
/// dispatched method can actually reach a `TextGrid` node; the decision itself is
/// `rpc::pane_cells_for`, which derives it from the METHOD (see there for why the method, and
/// not the path, is the honest discriminator).
///
/// [`Omitted`](Self::Omitted) keeps the tree's exact SHAPE — the pane still carries its
/// `TextGrid` child, tagged as always, holding an empty `0 x 0` buffer. Nothing about
/// resolution changes, so the only observable difference between the two modes is cell
/// content, which is what makes the equivalence guard in `rpc`'s tests a sharp instrument
/// rather than a shape comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneCells {
    /// Project every pane's live screen into its grid — what a reader of the cells needs.
    Projected,
    /// Leave every pane's grid empty. Reads no screen and takes no screen lock.
    Omitted,
}

/// Build one pane's `Scene::Container` (tagged `pane_<id>`) — the R1.7
/// data/engine split: a `TextGrid` (projected from the pane's live screen, or empty per
/// `cells`), and a [`SpragPaneExternal`] input engine holding the pane's
/// [`PanePtyHandle`](sprag_terminal::PanePtyHandle) so `scene/invoke` reaches
/// that pane's PTY.
///
/// The input engine is built from the pane's HANDLE alone, so an
/// [`Omitted`](PaneCells::Omitted) assembly touches the screen not at all — it neither
/// projects nor takes the screen lock, which is the whole cost of a pane child.
fn pane_container(id: PaneId, pty: &PanePty, cells: PaneCells) -> Scene {
    let grid = match cells {
        PaneCells::Projected => pty.with_screen_palette(|screen, palette| {
            text_grid_node(screen, palette, CellMetric::DEFAULT)
        }),
        PaneCells::Omitted => grid_node(GRID_TAG, CellMetric::DEFAULT, GridBuffer::new(0, 0)),
    };
    let input = Scene::External(
        ExternalNode::new(Box::new(pane::SpragPaneExternal::new(pty.handle()))).with_tag(INPUT_TAG),
    );
    Scene::Container(
        ContainerNode::new(vec![Scene::TextGrid(grid), input])
            .with_tag(wire::pane_container_tag(id.0)),
    )
}

/// Assemble the window of the session `scope` names as a `Scene::Container` of its panes
/// plus the pane-management [`WorkspaceExternal`] (Round 7 multiplex control core).
///
/// Each pane child is refreshed from its PTY's current screen; the
/// engines and the control surface hold shared handles (a `PanePtyHandle`
/// per pane, an `Arc<Mutex<SessionRegistry>>` for mux control and an
/// `Arc<Mutex<Workspace>>` for the plugin host), so the per-request
/// scene stays a throwaway projection (R969) while input and pane lifecycle
/// reach live state. The workspace lock is released before returning so a
/// dispatched `scene/invoke` (spawn/close/resize) can re-acquire it without
/// deadlock.
///
/// ## The scope is the whole assembly, not a filter on it
///
/// Everything here is built from [`scope`](SessionScope)'s pool, so a request sees exactly
/// the one session it named: its panes are the only `pane_<id>` nodes in the tree, and a
/// pane belonging to another session is not addressable — `scene/invoke` on it answers
/// unknown-path, because the node genuinely is not there. Scoping is therefore structural
/// rather than a check each surface has to remember to make.
///
/// The control external is handed the scope too, and that is load-bearing rather than
/// tidy: it is the one child that reaches PAST the pool to the registry (sessions, windows
/// and layout are mux concerns), so without the scope it would assemble under `work` and
/// write to the default session — pinion's R889 "wrong target for writes", exactly. The
/// plugin host and the pane children need no such care: they are built from the resolved
/// pool and cannot address anything else. That the tightest surface needs the most
/// threading, and the narrow ones none, is the Interface Segregation split paying off.
///
/// `revision` is the shared scene-version token ([`HostState`]'s): the control
/// surface wires each pane it SPAWNS with a `bump_on_dirty(&revision)` hook (so a
/// mux-spawned pane's output wakes parked `scene/waitFor`, exactly as the boot
/// pane's does) and bumps it directly on a spawn / close (so a pane-set change
/// wakes a waiter before the new pane's first output). Without it a client that
/// long-polls change-notification would never learn about mux-spawned panes.
///
/// **v1 bound:** that token is ONE for the whole registry, so a change in any session wakes
/// every attached client, which then re-reads its own scene and finds it unchanged. That is
/// waste, not error — a shared revision can only over-report (`park_if_current` answers a
/// stale baseline and parks a current one; nothing consults it as a write precondition), so
/// no session's request is ever refused or mis-answered because another was busy.
///
/// ## The daemon's own state travels as ONE value
///
/// [`DaemonShared`] carries the three things a DAEMON has and an in-process host does not. They are
/// grouped rather than passed positionally because they share one property that is easy to get wrong
/// separately: each is `None` off a daemon, and a caller that supplies some and not others gets a
/// half-wired surface rather than an error. One value makes "this host is a daemon" a single
/// statement at the call site.
///
/// ## `cells` is the assembly's whole cost
///
/// Everything else here is `Arc` clones and handles; the panes' [`PaneCells`] are the one
/// term that scales with the screen. A caller that cannot read a `TextGrid` passes
/// [`Omitted`](PaneCells::Omitted) and the assembly stops being proportional to the pane set
/// at all — see `rpc::pane_cells_for` for which callers those are and why the answer is
/// decidable from the method alone.
/// **A CALL THAT ANNOUNCES SOMETHING ABOUT ONE RUN**, as the plugin surface takes it.
///
/// A named type rather than the shape spelled at each site, for the reason clippy states as
/// `type_complexity` and this workspace states as *a signature nobody can read is a signature
/// nobody checks*: there are four of these and they are all the same call.
pub type RunAnnounce = Arc<dyn Fn(crate::runs::RunId) + Send + Sync>;

/// **HOW AN IMAGE THAT CAN BE A DRIVER STARTS ONE, FOR A SESSION** — see
/// [`DaemonShared::spawn_driver`], which holds the whole argument for why this is injected.
pub type DriverSpawns = Arc<dyn Fn(&str) -> plugins::DriverSpawn + Send + Sync>;

/// **WHERE A RUN'S END AND A PERSON'S ORDER TO IT ARE ANNOUNCED**, for watchers of `session` —
/// `(on_end, on_ordered)`.
///
/// # ⚠⚠⚠⚠⚠ Why this is a function and not two closures at the one call site it used to have
///
/// It has two call sites now. [`workspace_scene`] mints these per request, and a daemon BOOT mints
/// them again for a run it is putting back on a driver (register item 543's sixth brick) — and a
/// resumed run whose end nobody announced would be a row that stops moving with every watcher of it
/// still parked. Two spellings of *where a run's news goes* is the shape this workspace has paid for
/// at every surface it duplicated, drifting first in whichever event one of them forgot.
///
/// ⚠⚠ **THE SAME OPAQUE-`Fn` DISCIPLINE THE PLUGIN SURFACE'S OTHER HOOKS FOLLOW**, and for a sharper
/// reason here: a journal is per SESSION and a run is not in one (the registry is the daemon's, the
/// pane pool is a window's). What crosses the boundary is a call with a run id in it; which channel
/// that reaches is this side's business.
///
/// ⚠ The name is captured rather than borrowed: the caller is a throwaway projection and the worker
/// that will announce outlives it.
///
/// ⚠⚠⚠ A run's ORDERS were readable and unannounceable — register item 648. The row publishes
/// `stood_down` and `cancelled_by`, so a driver outside this daemon (item 544) could ASK, and had
/// nothing to be woken by, which means asking on a clock. Items 629/630/631/640 spent four rounds
/// taking exactly that off the PANE axis; the second of these is what stops the run axis being where
/// it comes back.
#[must_use]
pub fn run_announcers(
    channels: &Arc<ChannelRegistry>,
    session: &str,
) -> (RunAnnounce, RunAnnounce) {
    let on_end = {
        let channels = Arc::clone(channels);
        let session = session.to_owned();
        Arc::new(move |id: crate::runs::RunId| {
            channels.announce(&session, vec![events::Event::RunFinished(id.0)]);
        }) as RunAnnounce
    };
    let on_ordered = {
        let channels = Arc::clone(channels);
        let session = session.to_owned();
        Arc::new(move |id: crate::runs::RunId| {
            channels.announce(&session, vec![events::Event::RunOrdered(id.0)]);
        }) as RunAnnounce
    };
    (on_end, on_ordered)
}

#[must_use]
pub fn workspace_scene(
    scope: &SessionScope,
    registry: &Arc<Mutex<SessionRegistry>>,
    runs: &Arc<Mutex<RunRegistry>>,
    channels: &Arc<ChannelRegistry>,
    daemon: DaemonShared,
    cells: PaneCells,
) -> Scene {
    // The two signals the PLUGIN surface needs, taken before the rest moves into the mux surface.
    // Both are registry-FREE opaque `Fn`s (see `spawn_reaper` and `attention`), so a plugin-spawned
    // pane feeds the reaper and can ask for a person exactly like a mux one, without the plugin
    // layer ever learning what either hook does — the ISP boundary intact.
    // ⚠ Cloned WHOLE rather than field by field: the plugin surface needs four of these now
    // (the three hooks and whether this image can start a driver), and the mux surface below
    // consumes the value — see `plugin_host`, which takes them off it.
    let plugin_side = daemon.clone();
    // The scoped session's pool, resolved when the scope was (never re-derived here — one
    // question, one answer). The registry lock is not held, so taking the workspace lock
    // below cannot nest inside it.
    let workspace = scope.workspace();
    let mut children: Vec<Scene> = {
        let guard = lock(workspace);
        guard
            .panes()
            .iter()
            .map(|pane| pane_container(pane.id(), pane.pty(), cells))
            .collect()
    };
    // The mux control plane speaks the REGISTRY (sessions / windows / layout are mux
    // concerns), so it carries the scope that says WHICH session it may act on...
    children.push(Scene::External(
        ExternalNode::new(Box::new(workspace::WorkspaceExternal::new(
            Arc::clone(registry),
            scope.clone(),
            Arc::clone(channels),
            daemon,
        )))
        .with_tag(MUX_TAG),
    ));
    // ...while the plugin host speaks only the pane POOL: a plugin has no business knowing
    // about the session tree (Interface Segregation). It DOES carry the same `on_pane_exit`
    // death-signal, but that is a registry-FREE opaque `Fn` (see [`crate::spawn_reaper`]) — a
    // plugin-spawned pane feeds the reaper exactly like a mux one without the plugin layer ever
    // learning what the hook does, so no pane category can leave a lingering daemon and the ISP
    // boundary is intact.
    children.push(Scene::External(
        ExternalNode::new(Box::new(plugin_host(
            Arc::clone(workspace),
            runs,
            scope.session(),
            channels,
            &plugin_side,
        )))
        .with_tag(PLUGINS_TAG),
    ));
    Scene::Container(ContainerNode::new(children).with_tag(WORKSPACE_TAG))
}

/// **THE PLUGIN SURFACE THIS DAEMON SERVES OVER ONE PANE POOL** — the ONE spelling of it.
///
/// # ⚠⚠⚠⚠⚠ Why it is a function, and what its second caller is
///
/// It had one caller — [`workspace_scene`], per request — right up until a daemon BOOT needed one
/// too, to put the runs it inherited back on drivers (register item 543's sixth brick). Assembling
/// it a second time over there would have been a second answer to *what plugin surface does this
/// daemon serve*, and the key that answer turns on is [`options::RUN_DRIVER_PROCESS`] — so the
/// second copy would have been free to resume a run on a thread inside a daemon whose whole
/// configuration says drivers live in processes of their own. That is the invisible divergence that
/// option's own doc promises cannot happen, and one function is how.
///
/// ⚠⚠ **`session` IS FOR THE ANNOUNCEMENTS AND THE DRIVER, NEVER FOR THE PLUGIN.** The surface
/// itself speaks only the pool: a plugin has no business knowing about the session tree (Interface
/// Segregation). What the name is needed for is where a run's news is published
/// ([`run_announcers`]) and which session a driver process must scope its waits to — both of them
/// this side's business, carried in as opaque `Fn`s and a `-t` flag.
///
/// ⚠ The three hooks are taken as arguments rather than a `DaemonShared`, because the boot holds
/// them individually and an in-process host holds none of them — `None` is a host with no wire
/// clients to address, which is what each of those fields already documents.
#[must_use]
pub fn plugin_host(
    workspace: Arc<Mutex<sprag_terminal::Workspace>>,
    runs: &Arc<Mutex<RunRegistry>>,
    session: &str,
    channels: &Arc<ChannelRegistry>,
    daemon: &DaemonShared,
) -> plugins::PluginsExternal {
    let (on_end, on_ordered) = run_announcers(channels, session);
    let host = plugins::PluginsExternal::new(
        workspace,
        Arc::clone(runs),
        daemon.on_pane_exit.clone(),
        daemon.attention.clone(),
        daemon.agents.clone(),
        Some(on_end),
        Some(on_ordered),
    );
    // ⚠⚠⚠⚠⚠ AND WHERE A RUN'S DRIVER IS PUT IN A PROCESS OF ITS OWN — register items 544 / 643 /
    // 650, minted here on the announcements' exact terms and for their reason: starting a driver
    // needs to know WHICH BINARY THIS IS, WHICH ENDPOINT it serves and WHICH SESSION a client of it
    // must scope to, and the session tree is precisely what the plugin surface is free of.
    //
    // ⚠⚠ **AND SINCE 2026-08-25 A DAEMON NOBODY CONFIGURED TAKES THIS BRANCH.** The default was
    // flipped to `on` on 2026-08-24 and flipped BACK the same day — the workspace sweep answered
    // with eighteen failures, then nine, then two, and those two were register items 664 and 665.
    // Both are paid, and the word moved on a sweep with nothing left in it. Two gates hold the two
    // different sentences: `cli`'s
    // `a_daemon_told_to_drive_runs_in_processes_of_their_own_does_and_one_told_not_to_does_not` is
    // the OPTION's contract in both directions and cannot see a default at all, and
    // `a_daemon_nobody_configured_drives_its_runs_in_processes_of_their_own` is the one that reads
    // the shipped word.
    //
    // ⚠⚠⚠⚠⚠ **AND THE ABILITY IS THE CALLER'S TO GRANT, NOT THIS FUNCTION'S TO ASSUME** — see
    // [`DaemonShared::spawn_driver`]. The option says whether to USE a driver process; whether this
    // image can BE one is a fact only the image knows, and a host that hands none drives on a
    // thread whatever any config file says.
    match &daemon.spawn_driver {
        Some(spawn) => host.driving_out_of_process(spawn(session)),
        None => host,
    }
}

/// **HOW THIS DAEMON STARTS A RUN'S DRIVER**, scoped to `session` — register items 544 / 643 / 650.
///
/// # ⚠⚠⚠ The three facts it closes over, and why each has to be closed over here
///
/// * **Which binary.** [`std::env::current_exe`], so *the driver is this daemon's image* is true by
///   construction rather than by a version check — `crate::drive`'s module doc argues that at
///   length. A named artefact would be a second thing to ship and a second version to skew.
/// * **Which endpoint.** Passed in the child's environment under the same name every other client
///   of this daemon reads ([`sprag_rpc::HOST_SOCKET`]'s override), so the two can never disagree
///   about which host they mean.
/// * **Which session.** `-t`, because a park is served from ONE session's revision channel and the
///   daemon refuses a wait for a pane that session does not hold. Reading and typing are addressed
///   by global pane id and would not need it; the WAIT does.
///
/// # ⚠⚠ The request goes on stdin, and the pipe is closed
///
/// A brief carries prose somebody wrote, and quoting a paragraph through a command line is a class
/// of bug this avoids rather than handles. ⚠ The write TOLERATES a broken pipe: a driver that
/// refused before reading is one whose exit status is the answer, and treating the pipe as fatal
/// would report `Broken pipe` instead — register item 471, which `sprag-gate`'s own ratchet holds
/// every test in this tree to.
pub fn driver_spawn(session: &str) -> plugins::DriverSpawn {
    let session = session.to_owned();
    Arc::new(
        move |id: crate::runs::RunId, request: &serde_json::Map<String, serde_json::Value>| {
            let mut child = std::process::Command::new(std::env::current_exe()?)
                .arg(drive::DRIVE_FLAG)
                .arg(id.0.to_string())
                .arg("-t")
                .arg(&session)
                // ⚠ THE ENDPOINT'S OWN ENV NAME, taken from the endpoint rather than spelled here:
                // `HOST_SOCKET` carries both the path policy and the variable that overrides it, so a
                // driver reads the socket through the same door every other client of this daemon does.
                .env(
                    sprag_rpc::HOST_SOCKET.path_env,
                    sprag_rpc::socket_path(sprag_rpc::HOST_SOCKET),
                )
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()?;
            {
                use std::io::Write as _;
                let mut stdin = child.stdin.take().ok_or_else(|| {
                    std::io::Error::other(
                        "a driver is spawned with a piped stdin, and this one has none",
                    )
                })?;
                let brief = serde_json::Value::Object(request.clone()).to_string();
                match stdin.write_all(brief.as_bytes()) {
                    Ok(()) => {}
                    // ⚠ THE WHOLE POINT — see this function's own note. The child refused before
                    // reading, and its exit status is what the collector came for.
                    Err(why) if why.kind() == std::io::ErrorKind::BrokenPipe => {}
                    Err(why) => return Err(why),
                }
            }
            Ok(child)
        },
    )
}

/// The scene a request whose SESSION SCOPE COULD NOT BE RESOLVED is offered before it is refused:
/// the mux control surface with no scope, no panes and no plugin host.
///
/// ## Why a reader with no session gets a scene at all
///
/// [`SessionScope`] gates every method, and a client scoped to a session that was then destroyed is
/// refused whatever it asks — which is the DETACH signal a display client's poll thread runs on,
/// and stays it. What R326 measured is that the refusal also covered the reads that need no session
/// to answer, and that this is not academic: `detach-on-destroy`'s switch policies pick where to
/// land by reading the SESSION LIST, and they read it at the one moment their own scope has just
/// died. `no-detached` — tmux's *"switch, but never onto a session somebody else is in"* — was
/// therefore deciding on a mirror nothing bounds the staleness of, and was measured joining a
/// session another client was sitting in.
///
/// ## The safety is structural, not a per-slot judgement
///
/// This scene's only child is a [`RegistryExternal`](workspace::RegistryExternal), which holds no
/// [`SessionScope`] — so no read served here can be about a session, and none can be about the
/// WRONG one. There is nothing to classify and nothing to keep in sync: an address whose answer
/// would need a scope is simply not answerable from this scene, and `rpc`'s door turns that into
/// the scope refusal the reader had coming. A scene built from the DEFAULT session instead would
/// have been one bug away from serving a client panes it never named, which is precisely the
/// failure [`scope`] exists to prevent.
///
/// No panes and no plugin host, for the same reason stated positively: both are built from a
/// resolved pool, and there is no pool here to build them from.
#[must_use]
pub fn registry_scene(registry: &Arc<Mutex<SessionRegistry>>, daemon: DaemonShared) -> Scene {
    Scene::Container(
        ContainerNode::new(vec![Scene::External(
            ExternalNode::new(Box::new(workspace::RegistryExternal::new(
                Arc::clone(registry),
                daemon,
            )))
            .with_tag(MUX_TAG),
        )])
        .with_tag(WORKSPACE_TAG),
    )
}

/// Snapshot a [`Screen`] as scene-as-data: the [`TextGridSnapshot`] an AI
/// consumer reads over `scene/snapshot`, produced headlessly (no GPU, no
/// shell event loop).
///
/// # Panics
///
/// Panics only on an internal invariant break: the scene root is a
/// `Scene::TextGrid` by construction, and the empty path is always a
/// supported `scene/snapshot` query, so neither failure can occur for a
/// scene built by [`scene`].
#[must_use]
pub fn snapshot(screen: &Screen, palette: &Palette) -> TextGridSnapshot {
    let scene = scene(screen, palette);
    match pinion_rpc::snapshot::snapshot(&scene, "")
        .expect("the empty path is a supported scene/snapshot query")
    {
        pinion_rpc::snapshot::SnapshotNode::TextGrid(grid) => grid,
        other => unreachable!("scene root is a TextGrid by construction, got {other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sprag_vt::{Emulator, VtPort};

    fn snapshot_of(bytes: &[u8], cols: u16, rows: u16) -> TextGridSnapshot {
        let mut em = Emulator::new(cols, rows);
        em.advance(bytes);
        snapshot(em.screen(), em.palette())
    }

    #[test]
    fn snapshot_round_trips_content_and_buffer_dims() {
        let snap = snapshot_of(b"hi", 20, 3);
        // The authoritative terminal size is the producer's GridBuffer.
        assert_eq!((snap.buffer_cols, snap.buffer_rows), (20, 3));
        // PINION-PR80 restated this. It used to read `(0, 0)` — the derivation off an
        // unset rect — with the note "not faked to mirror the buffer", because mirroring
        // was the only way to make the numbers agree and a mirror says nothing about WHO
        // decided them. A declaration does: `winsize_source` carries the authority, so the
        // agreement below is sprag stating that its daemon sized this grid, not a rect that
        // happens to match.
        assert_eq!((snap.cols, snap.rows), (20, 3));
        assert_eq!(snap.winsize_source, "producer");
        assert_eq!(snap.grid_rows.len(), 3);
        assert!(snap.grid_rows[0].text.starts_with("hi"));
        assert_eq!((snap.cursor.col, snap.cursor.row), (2, 0));
        assert_eq!(snap.screen, "main");
    }

    #[test]
    fn snapshot_carries_alt_screen() {
        let snap = snapshot_of(b"\x1b[?1049hX", 10, 2);
        assert_eq!(snap.screen, "alternate");
        assert!(snap.grid_rows[0].text.starts_with("X"));
    }

    #[test]
    fn snapshot_marks_wide_cluster_runs() {
        let snap = snapshot_of("\u{4e16}".as_bytes(), 6, 1);
        let runs = &snap.grid_rows[0].runs;
        assert_eq!(runs[0].width, "wide");
        assert_eq!(runs[1].width, "trailer");
    }

    #[test]
    fn snapshot_reports_per_row_damage_generation() {
        let snap = snapshot_of(b"a", 4, 2);
        assert!(snap.grid_rows[0].generation > 0);
        assert_eq!(snap.grid_rows[1].generation, 0);
    }

    /// Extract the lone projected `TextGrid` from a [`pane_view_scene_from_cells`]
    /// pane Container (the pane wraps the grid in a tagged flex-share Container).
    fn pane_grid(scene: &Scene) -> &TextGridNode {
        match scene {
            Scene::Container(c) => match c.children.first() {
                Some(Scene::TextGrid(node)) => node,
                other => unreachable!("a pane Container holds one TextGrid, got {other:?}"),
            },
            other => unreachable!("pane_view_scene_from_cells returns a Container, got {other:?}"),
        }
    }

    /// The topology-B seam: `pane_view_scene_from_cells` assembles the identical
    /// focusable pane node from a hand-built cell buffer — no live `Screen`. This
    /// is the exact call a wire display client makes with cells it reconstructed
    /// off the host's data model, so a regression in the shared node shape (the
    /// focus-stop Container / composite `{tag}#grid` click anchor / R1002 font pin)
    /// fails HERE, in the crate that owns the shape, not silently in the GUI.
    #[test]
    fn pane_view_scene_from_cells_assembles_the_pane_node_without_a_screen() {
        let cells = GridBuffer::new(4, 1);
        let scene = super::pane_view_scene_from_cells("pane.wire", cells, CellMetric::DEFAULT, 18);
        match &scene {
            Scene::Container(c) => {
                assert_eq!(c.tag.as_deref(), Some("pane.wire"));
                assert_eq!(c.layout, LayoutStyle::new().with_focusable(true));
            }
            other => unreachable!("pane_view_scene_from_cells returns a Container, got {other:?}"),
        }
        // The one scene-derived Tab stop is the pane; its grid child is not.
        assert_eq!(scene.collect_focusable_tags(), vec!["pane.wire".to_owned()]);
        let node = pane_grid(&scene);
        assert_eq!(node.tag.as_deref(), Some("pane.wire#grid"));
        // The composite splits back to the focusable pane tag — the exact
        // resolution pinion's click-to-focus performs on a pointer press.
        assert_eq!(
            pinion_core::composite_tag::split_subindex("pane.wire#grid").0,
            "pane.wire",
            "the grid tag resolves to the focusable pane (click-to-focus)",
        );
        assert_eq!(node.cells().cols(), 4);
        assert_eq!(node.font_size_px(), Some(18));
        assert_eq!(node.layout.size.width, SizeValue::Percent(100));
        assert_eq!(node.layout.size.height, SizeValue::Percent(100));
    }

    /// The topology-B wire fidelity guarantee (PINION-PR49 / R106): the
    /// paint-authoritative [`GridBuffer`] survives a JSON round-trip byte-for-byte,
    /// so a wire display client reconstructs the EXACT buffer the host projected —
    /// every colour (indexed / truecolor / default), attribute, wide-cluster
    /// head+trailer split, cursor, screen kind, and per-row damage generation. This
    /// is the load-bearing assumption of the whole GUI-as-client-of-host arc: if
    /// serde dropped a field, the human (GUI) path would render differently from the
    /// host's served data and the "read-data-not-pixels" invariant would break
    /// SILENTLY in the GUI. It fails HERE — in the crate that owns the wire seam —
    /// instead. Proven on REAL projected production data (not a synthetic buffer),
    /// so it also guards `sprag_grid::project` against emitting a non-round-trippable
    /// cell.
    #[test]
    fn wire_round_trip_preserves_the_pane_cell_buffer() {
        use sprag_vt::{Emulator, VtPort};
        // A rich MAIN screen written at the top-left: bold indexed-red, truecolor,
        // reverse, a wide (CJK) cluster (head+trailer). Writing row 0 also stamps it
        // with a nonzero damage generation. Everything a real terminal frame carries.
        let mut em = Emulator::new(12, 2);
        em.advance(b"\x1b[1;31mred\x1b[0m "); // bold indexed-red "red"
        em.advance(b"\x1b[38;2;10;20;30mR\x1b[0m "); // truecolor "R"
        em.advance(b"\x1b[7mV\x1b[0m"); // reverse "V"
        em.advance("世".as_bytes()); // a wide cluster (head + trailer)
        let buf = sprag_grid::project_scrolled(em.screen(), 0, em.palette());

        // The wire: serialize -> deserialize the paint-authoritative buffer.
        let json = serde_json::to_string(&buf).expect("GridBuffer serializes (PR-49)");
        let back: GridBuffer = serde_json::from_str(&json).expect("GridBuffer round-trips (PR-49)");

        // The whole buffer is byte-identical — the strongest fidelity claim
        // (GridBuffer: Eq covers cells + cursor + screen + row generations).
        assert_eq!(
            buf, back,
            "serde round-trip is lossless for the paint buffer"
        );

        // Position-independent spot-checks so a dropped field type names itself (not
        // just "Eq failed"): search the round-tripped buffer for a cell of each kind.
        let find = |pred: &dyn Fn(&pinion_core::TermCell) -> bool| {
            (0..back.cols())
                .flat_map(|c| (0..back.rows()).map(move |r| (c, r)))
                .find_map(|(c, r)| back.cell(c, r).filter(|x| pred(x)))
        };
        // bold + the indexed "red": the projection resolves the palette, so ANSI index 1
        // reaches the wire as its concrete xterm RGB (0xcd0000), not a bare `Indexed(1)`.
        let red = find(&|x| {
            x.fg == pinion_core::TermColor::Rgb(pinion_core::style::Color::rgb(0xcd, 0x00, 0x00))
        })
        .expect("the resolved indexed-red fg survived");
        assert!(
            red.attrs.bold,
            "bold attr survived alongside the resolved indexed fg"
        );
        // truecolor fg (the "R" at 10,20,30) — distinct from the resolved red above.
        assert!(
            find(&|x| x.fg
                == pinion_core::TermColor::Rgb(pinion_core::style::Color::rgb(10, 20, 30)))
            .is_some(),
            "a truecolor (rgb) fg survived",
        );
        // reverse attr (the "V").
        assert!(
            find(&|x| x.attrs.reverse).is_some(),
            "the reverse attr survived",
        );
        // the wide cluster: head carries the cluster + Wide, its trailer is Trailer.
        let (wide_col, wide_row) = (0..back.cols())
            .flat_map(|c| (0..back.rows()).map(move |r| (c, r)))
            .find(|&(c, r)| {
                back.cell(c, r)
                    .is_some_and(|x| x.width == pinion_core::CellWidth::Wide)
            })
            .expect("the wide cluster survived the round-trip");
        assert_eq!(back.cell(wide_col, wide_row).unwrap().cluster, "世");
        assert_eq!(
            back.cell(wide_col + 1, wide_row).unwrap().width,
            pinion_core::CellWidth::Trailer,
        );
        // cursor (position / shape / visibility) and a nonzero damage generation.
        assert_eq!(back.cursor(), buf.cursor(), "cursor survived");
        assert!(back.cursor().visible, "the live cursor is visible");
        assert!(
            back.row_generation(0).is_some_and(|g| g > 0),
            "per-row damage generation survived",
        );

        // And the CLIENT node assembled from the round-tripped buffer carries the
        // round-tripped cells — a wire client paints byte-identically to the host's
        // projection (the buffers are Eq, so the deterministic assembly matches).
        let node_wire =
            super::pane_view_scene_from_cells("sprag_gui.pane.0", back, CellMetric::DEFAULT, 18);
        let node_direct =
            super::pane_view_scene_from_cells("sprag_gui.pane.0", buf, CellMetric::DEFAULT, 18);
        assert_eq!(
            pane_grid(&node_wire).cells(),
            pane_grid(&node_direct).cells(),
            "the client node built off the wire matches the direct projection",
        );

        // The screen-kind field round-trips as Alternate too (not just the Main
        // default above): a fullscreen app's alt screen must reach the client.
        let mut alt = Emulator::new(4, 2);
        alt.advance(b"\x1b[?1049hA");
        let altbuf = sprag_grid::project(alt.screen(), alt.palette());
        assert_eq!(altbuf.screen(), pinion_core::ScreenKind::Alternate);
        let altback: GridBuffer =
            serde_json::from_str(&serde_json::to_string(&altbuf).unwrap()).unwrap();
        assert_eq!(altback, altbuf, "alt-screen buffer round-trips");
        assert_eq!(
            altback.screen(),
            pinion_core::ScreenKind::Alternate,
            "the Alternate screen kind survived the wire",
        );
    }
}
