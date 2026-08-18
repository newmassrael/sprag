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
pub mod doctor;
/// Deriving a directory's current git branch — a session-sidebar display fact (crate-internal).
mod git;
pub mod layout;
pub mod pane_name;
pub mod pane_pty;
/// Discovering the TCP ports a session's process subtree is listening on — a session-sidebar
/// display fact (crate-internal).
mod ports;
pub mod processes;
/// A process's facts, read from whichever OS this is: its parent, its group, its terminal's
/// foreground group, its arguments and the environment it was exec'd with.
///
/// `/proc` on Linux, `proc_pidinfo` / `proc_listpids` / `KERN_PROCARGS2` on macOS, and an honest
/// absence elsewhere. Every PARSE here is portable and compiled — and tested — everywhere; only the
/// calls that touch an OS are per-platform, which is the split that stopped a plain struct from
/// vanishing off Linux (R340) and the one that makes a reader's absence visible (R343).
///
/// Mostly crate-internal. `procfs::environ` is public because the question *"what did
/// whoever started this process hand it?"* has a consumer outside this crate — `sprag-mcp`'s
/// ancestor walk, which read `/proc` itself and therefore answered nothing off Linux.
pub mod procfs;
/// The OS pseudoterminal and the child on the far side of it — the platform boundary this crate
/// owns (R336). Unix-only; a Windows arm would be a sibling of its two entry points.
#[cfg(unix)]
pub mod pty;
pub mod registry;
pub mod remote;
pub mod resources;
pub mod sampled;
pub mod session_name;
pub mod share;
pub mod snapshot;
pub mod stop;
pub mod tiling;
pub mod window_name;
pub mod workspace;

pub use activity::{ActivityReading, ActivitySampler, SessionActivity};
pub use command::{command_from_parts, default_shell_command, shell_command_line};
pub use doctor::{
    Blind, Ccache, Check, Diagnosis, Evidence, Finding, Level, Load, Measurement, NoEvidence,
    PaneReading, PaneSite, Readings, Sibling, Sources, Subject, SubtreeReading, Verdict,
};
pub use layout::{
    DividerStep, LayoutError, LayoutNode, LayoutNodeWire, LayoutSnapshot, LayoutTree, LayoutWire,
    LeafHome, MAX_LAYOUT_DEPTH, PaneDir, PaneStep, SplitDir, SplitId, SplitSide,
};
pub use pane_name::{PaneName, PaneNameError};
pub use pane_pty::{
    Attention, CommandBuilder, ECHO_TRAIL_CAP, Hand, Hands, PaneExit, PaneHooks, PanePty,
    PanePtyError, PanePtyHandle, RawOutput, exit_phrase, foreground_pgid_of,
};
pub use processes::{
    ForegroundJob, JobProcess, PaneProcessReading, PaneProcessSampler, PaneProcesses,
    foreground_leader_of,
};
#[cfg(unix)]
pub use pty::{PaneEcho, PaneEndOfInput, PaneSignalKeys, SignalKey, Unraised};
pub use registry::{
    Ended, KillOutcome, Located, LocatedWindow, OrderStep, PaneKillOutcome, PaneMoveError,
    PlaceHow, Session, SessionError, SessionId, SessionInfo, SessionRegistry, TreePane,
    TreeSession, TreeWindow, Window, WindowBirth, WindowId, WindowInfo, WindowKillOutcome,
    WindowPlace, ZoomOutcome,
};
pub use remote::SshRemote;
pub use resources::{Cpu, PaneResourceReading, PaneResourceSampler, PaneResources, SETTLE, Taken};
pub use sampled::{Reading, Sampled};
pub use session_name::{SessionName, SessionNameError};
pub use share::{
    Ceiling, CgroupNode, Charge, Counted, Enforcement, Grant, Granted, Landing, LimitSource,
    Limits, PaneHomes, PaneLineage, Percent, Placement, PoolLineage, Pressure, Refusal, Share,
    ShareError, Tree, TreeError, Unenforceable, Unmeasured, Waiting, mount_point,
};
pub use snapshot::{
    MIN_READABLE_SNAPSHOT_VERSION, PaneHistory, PaneRestore, PaneSnapshot, RestorePlan,
    SNAPSHOT_VERSION, SessionSnapshot, Snapshot, SnapshotError, WindowSnapshot, pane_histories,
    snapshot,
};
/// Declaring an enum together with the array of every one of its variants, so the two cannot drift.
///
/// It LIVES in [`sprag_vt`] — the workspace's bottom crate, which every other sprag crate depends
/// on — because the emulator declares closed sets too ([`sprag_vt::port::Urgency`]) and a
/// vocabulary primitive one crate cannot reach is a primitive that gets hand-rolled there instead:
/// exactly the drifting `ALL` array this macro exists to remove. Re-exported here so the twenty-odd
/// `sprag_terminal::closed_set!` call sites above this crate keep one spelling.
pub use sprag_vt::closed_set;
/// Projecting a closed set through its own spelling into the array of wire words a declaration can
/// publish — the companion of the macro above, re-exported here for the same reason it is.
///
/// No intra-doc link to that macro: `closed_set` names a module AND a macro in `sprag_vt`, so a
/// bare reference is the ambiguity the doc gate refuses, and R344's rule is that a word naming two
/// things is a defect rather than a link to spell around.
pub use sprag_vt::wire_words;
pub use stop::{Reach, Stop, StoppedJob, Unstopped, stop_foreground_job};
pub use tiling::{Divider, PaneRect, Projection, Rect, Tiling, fit_window, tile, with_ratio};
pub use window_name::{WindowName, WindowNameError};
pub use workspace::{
    HistoryLimitSource, Pane, PaneArgsSource, PaneBirthHooks, PaneEnvSource, PaneId,
    PaneIdentitySource, PaneInfo, PaneRebirth, Workspace,
};
