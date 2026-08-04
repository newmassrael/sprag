//! sprag-terminal — the PTY producer.
//!
//! Owns the OS pseudoterminal and the [`sprag_vt::Emulator`] it feeds,
//! exposing the live terminal [`sprag_vt::Screen`] as queryable state. This
//! is the producer side of the walking-skeleton slice (DESIGN.md §5):
//!
//! ```text
//! PTY ─▶ termwiz parser ─▶ sprag emulator ─▶ queryable Screen
//! ```
//!
//! Scene assembly and the scene-as-data RPC server live one layer up, in
//! `sprag-host`, so this crate depends only on the emulator and the PTY
//! abstraction — never on pinion (DESIGN.md §3: the producer owns state; the
//! host projects it).

pub mod activity;
pub mod arrangement;
pub mod command;
/// Deriving a directory's current git branch — a session-sidebar display fact (crate-internal).
mod git;
pub mod layout;
pub mod pane_name;
pub mod pane_pty;
/// Discovering the TCP ports a session's process subtree is listening on — a session-sidebar
/// display fact (crate-internal).
mod ports;
pub mod processes;
/// The one parse of a process's `/proc/<pid>/stat` line, shared by every `/proc` reader in this
/// crate (crate-internal, Linux-only).
#[cfg(target_os = "linux")]
mod procfs;
pub mod registry;
pub mod remote;
pub mod sampled;
pub mod session_name;
pub mod snapshot;
pub mod tiling;
pub mod workspace;

pub use activity::{ActivityReading, ActivitySampler, SessionActivity};
pub use command::{command_from_parts, default_shell_command, shell_command_line};
pub use layout::{
    LayoutError, LayoutNode, LayoutNodeWire, LayoutSnapshot, LayoutTree, LayoutWire, LeafHome,
    MAX_LAYOUT_DEPTH, PaneDir, PaneStep, SplitDir, SplitId, SplitSide,
};
pub use pane_name::{PaneName, PaneNameError};
pub use pane_pty::{
    CommandBuilder, PaneExit, PanePty, PanePtyError, PanePtyHandle, RawOutput, foreground_pgid_of,
};
pub use processes::{
    ForegroundJob, JobProcess, PaneProcessReading, PaneProcessSampler, PaneProcesses,
};
pub use registry::{
    KillOutcome, PaneMoveError, Session, SessionError, SessionId, SessionInfo, SessionRegistry,
    Window, WindowId, WindowInfo, WindowKillOutcome, WindowStep, ZoomOutcome,
};
pub use remote::SshRemote;
pub use sampled::{Reading, Sampled};
pub use session_name::{SessionName, SessionNameError};
pub use snapshot::{
    MIN_READABLE_SNAPSHOT_VERSION, PaneHistory, PaneRestore, PaneSnapshot, RestorePlan,
    SNAPSHOT_VERSION, SessionSnapshot, Snapshot, SnapshotError, WindowSnapshot, pane_histories,
    snapshot,
};
pub use tiling::{Divider, PaneRect, Projection, Rect, Tiling, fit_window, tile, with_ratio};
pub use workspace::{
    HistoryLimitSource, Pane, PaneEnvSource, PaneId, PaneInfo, PaneRebirth, Workspace,
};
