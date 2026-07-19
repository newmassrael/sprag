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
pub mod layout;
pub mod pane_pty;
pub mod registry;
pub mod snapshot;
pub mod workspace;

pub use command::{command_from_parts, default_shell_command};
pub use layout::{
    FloatHome, LayoutError, LayoutNode, LayoutNodeWire, LayoutSnapshot, LayoutTree, LayoutWire,
    SplitDir, SplitId, SplitSide,
};
pub use pane_pty::{CommandBuilder, PanePty, PanePtyError, PanePtyHandle, RawOutput};
pub use registry::{
    KillOutcome, Session, SessionError, SessionInfo, SessionRegistry, Window, WindowInfo,
    WindowKillOutcome,
};
pub use snapshot::{
    PaneRestore, PaneSnapshot, RestorePlan, SNAPSHOT_VERSION, SessionSnapshot, Snapshot,
    SnapshotError, WindowSnapshot, snapshot,
};
pub use workspace::{Pane, PaneId, PaneInfo, Workspace};
