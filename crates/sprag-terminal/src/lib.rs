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

pub mod command;
/// Deriving a directory's current git branch — a session-sidebar display fact (crate-internal).
mod git;
pub mod layout;
pub mod pane_pty;
/// Discovering the TCP ports a session's process subtree is listening on — a session-sidebar
/// display fact (crate-internal).
mod ports;
pub mod registry;
pub mod remote;
pub mod snapshot;
pub mod tiling;
pub mod workspace;

pub use command::{command_from_parts, default_shell_command, shell_command_line};
pub use layout::{
    LayoutError, LayoutNode, LayoutNodeWire, LayoutSnapshot, LayoutTree, LayoutWire, LeafHome,
    MAX_LAYOUT_DEPTH, PaneDir, SplitDir, SplitId, SplitSide,
};
pub use pane_pty::{CommandBuilder, PaneExit, PanePty, PanePtyError, PanePtyHandle, RawOutput};
pub use registry::{
    KillOutcome, PaneMoveError, Session, SessionError, SessionInfo, SessionRegistry, Window,
    WindowInfo, WindowKillOutcome,
};
pub use remote::SshRemote;
pub use snapshot::{
    MIN_READABLE_SNAPSHOT_VERSION, PaneHistory, PaneRestore, PaneSnapshot, RestorePlan,
    SNAPSHOT_VERSION, SessionSnapshot, Snapshot, SnapshotError, WindowSnapshot, pane_histories,
    snapshot,
};
pub use tiling::{Divider, PaneRect, Rect, Tiling, fit_window, tile, with_ratio};
pub use workspace::{
    HistoryLimitSource, Pane, PaneEnvSource, PaneId, PaneInfo, PaneRebirth, Workspace,
};
