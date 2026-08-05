//! The durability snapshot — sprag's cmux-parity ring.
//!
//! tmux's daemon keeps a session's live processes alive across a client DETACH, but nothing
//! across a REBOOT: the daemon dies, every PTY with it. cmux's parity claim is the orthogonal
//! one — the layout, working directories and agent panels come back after the machine restarts,
//! with no daemon at all — because the *logical shape* is serialized to disk. A live PTY cannot
//! cross a reboot; a layout can. This module is that serialization.
//!
//! ## What it is — a PROJECTION, not a second authority
//!
//! A [`Snapshot`] is to a [`SessionRegistry`] what
//! [`LayoutWire`] is to a [`LayoutTree`](crate::LayoutTree): a serde DTO
//! captured FROM the one live authority and restored back INTO a fresh one. It never becomes a
//! parallel source of truth — [`snapshot`] reads the registry, `from_snapshot` rebuilds a
//! registry, and the file on disk is overwritten by the next save. The registry stays the SSOT.
//!
//! ## What survives, and what cannot
//!
//! The SHAPE survives: sessions (their order IS the default), each session's windows IN THE ORDER
//! THE USER ARRANGED THEM (R310 — the order `select-window -n` walks and `move-window` sets) and
//! which is current, each window's [`LayoutTree`](crate::LayoutTree) arrangement, its float set, and —
//! per pane — its id,
//! working directory, launch label and size. The global pane-id high-water mark rides too, so a
//! restore never reissues a retired id.
//!
//! A live PTY, its child process and a running agent's in-memory state do NOT — a reboot ends
//! them. On restore each pane re-spawns a fresh shell IN ITS RECORDED CWD (slice 1; re-running the
//! exact command is a later, allowlisted increment), which is the honest cmux analogue: the pane
//! and its directory come back, and an agent resumes its own state through its own tool. The
//! snapshot carries `command_label` for display and for that future increment.
//!
//! A pane's SCROLLBACK does survive, but not through this DTO — see [`pane_histories`]. It is
//! captured as replayable terminal bytes into one raw file per pane, because it is orders of
//! magnitude larger than the shape, changes on every scroll, and would not survive a JSON string
//! escape intact.
//!
//! ## Homes are not persisted (a documented bound)
//!
//! A window's [`LeafHome`](crate::layout::LeafHome) sidecar — where a floated pane docks back —
//! is a non-authoritative memo with a defined graceful fallback (dock back at the END). It is
//! deliberately left out of the snapshot in slice 1: the pane's FLOAT membership survives, so the
//! user's choice to float it does; only the exact dock-back slot degrades to an append after a
//! reboot, which is the same "a home is a memo, not a promise" fallback the live path already
//! honors.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use sprag_vt::HistoryLimits;

use crate::layout::LayoutWire;
use crate::registry::SessionRegistry;
use crate::remote::SshRemote;
use crate::workspace::{Pane, PaneId};

/// The on-disk snapshot format version this build WRITES. Bumped when the shape changes
/// incompatibly; a loader that reads a version it does not understand refuses
/// (`SnapshotError::Version`) and the daemon falls back to an EMPTY boot rather than crashing on
/// a format it cannot parse.
///
/// 2 since a window's stored `layout` became the flat arena
/// ([`MAX_LAYOUT_DEPTH`](crate::MAX_LAYOUT_DEPTH)). The bump is not about what THIS build can
/// read — it reads both shapes, see [`MIN_READABLE_SNAPSHOT_VERSION`] — but about what an OLDER
/// one must not silently mistake for its own: a build that knows only version 1 would parse the
/// header, fail on the layout, and `load_snapshot`'s `.ok()` would turn that into an empty boot
/// with no reason given. Stamping 2 turns that silence into the loud, specific refusal the
/// version field exists for.
pub const SNAPSHOT_VERSION: u32 = 2;

/// The oldest on-disk format this build still RESTORES — the other half of a migration.
///
/// A version bump that only moved the write stamp would cost every user their saved sessions
/// once, which is the opposite of what durability is for. So the loader accepts the whole range
/// `MIN_READABLE_SNAPSHOT_VERSION..=SNAPSHOT_VERSION`, and the one shape that actually differs —
/// the arrangement — is read in either spelling by [`LayoutWire`]'s own
/// deserialiser. The next save rewrites the file at [`SNAPSHOT_VERSION`], so a snapshot migrates
/// by being loaded.
pub const MIN_READABLE_SNAPSHOT_VERSION: u32 = 1;

/// The whole durable shape of a [`SessionRegistry`], serialized.
///
/// Produced by [`snapshot`] and consumed by
/// [`SessionRegistry::from_snapshot`](crate::SessionRegistry::from_snapshot). Versioned JSON:
/// the format is human-inspectable (a user can read their saved layout) and forward-migratable
/// (`version` gates the loader).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Snapshot {
    /// The format version — [`SNAPSHOT_VERSION`] at write time; checked on restore.
    pub version: u32,
    /// The global pane-id high-water mark (the next id the counter would mint). Stored rather
    /// than derived from the restored ids, so a retired id whose pane did NOT come back — a gap
    /// at the top of the range — is still never reissued (see
    /// [`Workspace::with_seeded_counter`](crate::Workspace)).
    pub next_id: u64,
    /// The default `(cols, rows)` a dimension-less spawn adopts, so a restored registry mints
    /// panes at the same default the pre-reboot one did.
    pub default_size: (u16, u16),
    /// Every session, in order — the order IS the default-session order a restore restores.
    pub sessions: Vec<SessionSnapshot>,
}

/// One session's durable shape: its name, its windows in order, and which window is current
/// (BY NAME, the addressing scheme the registry uses — an index would be fragile to a reorder).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SessionSnapshot {
    /// The session's name (its address).
    pub name: String,
    /// The name of the current window — the one an attached client views. Restored via
    /// [`Session::select_window`](crate::Session); must name one of `windows`.
    pub current_window: String,
    /// The session's windows, IN ORDER — which since R310 is the order the user arranged with
    /// `move-window`, not the order they were created in. It is a user's decision now, so it is a
    /// thing a restore has to bring back rather than a by-product of how the `Vec` was filled.
    pub windows: Vec<WindowSnapshot>,
}

/// One window's durable shape: its name, how its tiled panes are arranged, which panes are
/// floated out, and the per-pane facts a restore re-spawns from.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WindowSnapshot {
    /// The window's name (a tab label / address).
    pub name: String,
    /// How the TILED panes are arranged — the `LayoutWire` the live window would serve.
    pub layout: LayoutWire,
    /// The panes floated OUT of the tiling. Restored as float membership; WHERE each floating
    /// window sits on screen is the client's (pixels), lost across a reboot and re-placed by the
    /// window manager — exactly as a detach/reattach already treats floats.
    pub floating: Vec<PaneId>,
    /// Every pane in the window's pool, in spawn order — the membership authority. A pane in
    /// neither `layout` nor `floating` (spawned but not yet reconciled) still comes back and is
    /// appended by the first post-restore reconcile.
    pub panes: Vec<PaneSnapshot>,
    /// The `(cols, rows)` an operator PINNED for this window (tmux `resize-window`), or `None` if
    /// nobody has — the one size in this file that is a DECISION rather than a measurement.
    ///
    /// Every pane carries its own `cols`/`rows` above, and those are readings taken at snapshot
    /// time: they say how big the panes happened to be, and the first client to attach after a
    /// restore overwrites them. This says how big the window was declared to be, which is why it has
    /// to be here — a reboot is exactly when there is no client to ask, so a pinned window that did
    /// not persist would come back pinned to nothing and reflow on the next attach, losing a
    /// decision the panes' own sizes cannot reconstruct (they are a tiling of it, not it).
    ///
    /// `#[serde(default)]` keeps the addition additive, the rule `argv` and `remote` already follow:
    /// a snapshot written before this field loads as `None`, which is the un-pinned behaviour those
    /// daemons had.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_size: Option<(u16, u16)>,
    /// The pane the window was ON (tmux's active pane), or `None` for a window that held none.
    ///
    /// Restored on [`manual_size`](Self::manual_size)'s argument, unchanged: it is a DECISION
    /// rather than a measurement, and a reboot is exactly the moment there is no client to ask.
    /// Coming back to a workspace with the cursor on whichever pane happened to be first would
    /// discard a fact the panes' own state cannot reconstruct.
    ///
    /// `#[serde(default)]` on the same additive terms: an older snapshot loads as `None`, and the
    /// first reconcile after the restore selects the window's first pane — which is the behaviour
    /// the daemons that wrote those snapshots had.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<PaneId>,
    /// The pane that was filling the window on its own (tmux's zoom), or `None` if none was.
    ///
    /// Restored on [`active`](Self::active)'s argument, and it is a pane ID rather than a flag for
    /// a reason a reboot makes sharp: a stored boolean can only come back bound to whichever pane
    /// the restore happens to make active, while an id either finds its pane or does not. If it
    /// did not come back, the first post-restore reconcile ENDS the zoom instead of handing it to
    /// a stranger.
    ///
    /// `#[serde(default)]` on the same additive terms: an older snapshot loads unzoomed, which is
    /// what the daemons that wrote it could express.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zoomed: Option<PaneId>,
}

/// One pane's restore facts: enough to re-spawn a shell where the pane was and address it by its
/// old id. NOT the live PTY — that dies with the reboot.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PaneSnapshot {
    /// The pane's registry-global id — restored EXACTLY, because the layout tree, float set and
    /// homes all reference the pane by it (see
    /// [`spawn_restored`](crate::Workspace::spawn_restored)).
    pub id: PaneId,
    /// The child's working directory at snapshot time, where the restored shell re-spawns.
    /// `None` when it could not be read (the child had exited, or a non-Linux host) — the
    /// restored shell then falls back to the daemon's own cwd.
    pub cwd: Option<PathBuf>,
    /// What was LAUNCHED in the pane (its introspection label / program name) — a display name.
    pub command_label: String,
    /// The full argv the pane was launched with (`[program, args…]`) — what an EXACT-COMMAND
    /// restore re-runs for an allowlisted program (`Host::restore`), else it falls back to a shell.
    /// `#[serde(default)]` so a pre-argv (slice-1) snapshot still loads — an empty argv simply
    /// restores a shell, the slice-1 behaviour.
    ///
    /// The ENVIRONMENT is deliberately NOT persisted: a restored command inherits the DAEMON's env
    /// (where API keys live), so an env-borne secret never reaches disk. The argv, HOWEVER, IS on
    /// disk — a secret passed as a FLAG (`mysql -pSECRET`) lands here, which is why the snapshot
    /// file is written owner-only (0600, `save_snapshot`). Prefer env / config files over
    /// command-line secrets.
    #[serde(default)]
    pub argv: Vec<String>,
    /// The structured remote endpoint of a `sprag ssh` workspace pane, or `None` for a local pane.
    /// Present marks a SANCTIONED remote workspace: on restore the host RECONNECTS it (`ssh -t
    /// user@host`) instead of falling back to a shell, and the argv allowlist is bypassed because
    /// the endpoint is explicit intent, not an argv that merely mentions `ssh`. `#[serde(default)]`
    /// keeps the addition additive — a pre-Slice-5 snapshot loads with `None`, the old behaviour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<SshRemote>,
    /// The pane whose occupant asked for this one ([`Pane::opened_by`](crate::Pane::opened_by)), or
    /// `None` for a pane nobody claims.
    ///
    /// Durable because pane IDS are durable: the layout tree brings every pane back under its old
    /// id, so an opener id still names the same pane after a reboot. A provenance dropped here would
    /// come back as "nobody asked for this", turning every agent-opened pane into one its opener can
    /// no longer close — the agent surface gates that verb on exactly this fact.
    ///
    /// `#[serde(default)]` on the usual additive terms: a pre-R294 snapshot loads with `None`, which
    /// is what the daemons that wrote it could express.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opened_by: Option<PaneId>,
    /// The name a person gave this pane ([`Pane::name`](crate::Pane::name)), or `None` for a pane
    /// nobody named.
    ///
    /// Durable for a stronger reason than the provenance above: a name is an ADDRESS. A script, a
    /// keybinding or an agent's own notes that say `--pane build` keep working across a reboot
    /// only if the name comes back with the pane, and a name silently dropped by a restart would
    /// resolve to nothing exactly when the pane is still sitting there.
    ///
    /// Restoring it also restores the UNIQUENESS the live registry enforces, because the ids come
    /// back exactly and each name came from one pane — the restore replays a set that was already
    /// valid rather than re-deriving one.
    ///
    /// Deserialised through [`PaneName::parse`](crate::PaneName::parse) (see that type's `Deserialize`),
    /// so a file an older daemon wrote — or one somebody edited — cannot smuggle in a name the live
    /// surfaces refuse. A name this build rejects fails the whole snapshot load rather than loading
    /// a pane that is addressable by something no caller can express.
    ///
    /// `#[serde(default)]` on the usual additive terms: a pre-R295 snapshot loads with `None`,
    /// which is what the daemons that wrote it could express.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<crate::PaneName>,
    /// The pane's size, so the restored shell opens at the same dimensions.
    pub cols: u16,
    pub rows: u16,
}

/// Capture the registry's whole durable shape as a serializable [`Snapshot`].
///
/// Reads each pane's LIVE cwd and size, so it must lock each window's
/// [`Workspace`](crate::Workspace) — but NEVER while holding the registry lock. The host holds
/// the workspace lock then the registry lock everywhere (see
/// `reconciled_layout`), so taking them the other way round here would be the one nesting that
/// could deadlock. So this captures the structure and a HANDLE to each pool under a brief
/// registry lock, releases it, then reads the panes with each pool's own lock — the two locks
/// taken sequentially, never nested, matching the discipline the rest of the host keeps.
#[must_use]
pub fn snapshot(registry: &Arc<Mutex<SessionRegistry>>) -> Snapshot {
    /// A window captured under the registry lock, its pool held only as a handle to read later.
    struct WinSkel {
        name: String,
        layout: LayoutWire,
        floating: Vec<PaneId>,
        manual_size: Option<(u16, u16)>,
        active: Option<PaneId>,
        zoomed: Option<PaneId>,
        pool: Arc<Mutex<crate::workspace::Workspace>>,
    }
    struct SessSkel {
        name: String,
        current_window: String,
        windows: Vec<WinSkel>,
    }

    // Phase 1 — registry lock ONLY. No workspace lock is taken here; the pools are cloned out as
    // Arcs and read in phase 2 with the registry lock released.
    let sessions_skel: Vec<SessSkel> = {
        let reg = registry.lock().unwrap_or_else(PoisonError::into_inner);
        reg.sessions()
            .iter()
            .map(|s| SessSkel {
                name: s.name().to_owned(),
                current_window: s.current_window().name().to_owned(),
                windows: s
                    .windows()
                    .iter()
                    .map(|w| {
                        let mut floating: Vec<PaneId> = w.floating().iter().copied().collect();
                        floating.sort(); // a stable serialization order
                        WinSkel {
                            name: w.name().to_owned(),
                            layout: LayoutWire::from(w.layout()),
                            floating,
                            manual_size: w.manual_size(),
                            active: w.active_pane(),
                            zoomed: w.zoomed(),
                            pool: Arc::clone(w.workspace()),
                        }
                    })
                    .collect(),
            })
            .collect()
    };

    // The global counter + default size are shared across every pool, so read them from the
    // first one (the registry is never empty; both fall back to harmless defaults if it were).
    let (next_id, default_size) = sessions_skel
        .first()
        .and_then(|s| s.windows.first())
        .map(|w| {
            let pool = w.pool.lock().unwrap_or_else(PoisonError::into_inner);
            (pool.next_id_hint(), pool.default_size())
        })
        .unwrap_or((0, (80, 24)));

    // Phase 2 — registry lock released. Each pool read under its OWN lock, never nested.
    let sessions = sessions_skel
        .into_iter()
        .map(|s| SessionSnapshot {
            name: s.name,
            current_window: s.current_window,
            windows: s
                .windows
                .into_iter()
                .map(|w| {
                    let pool = w.pool.lock().unwrap_or_else(PoisonError::into_inner);
                    let panes = pool.panes().iter().map(pane_snapshot).collect();
                    WindowSnapshot {
                        name: w.name,
                        layout: w.layout,
                        floating: w.floating,
                        panes,
                        manual_size: w.manual_size,
                        active: w.active,
                        zoomed: w.zoomed,
                    }
                })
                .collect(),
        })
        .collect();

    Snapshot {
        version: SNAPSHOT_VERSION,
        next_id,
        default_size,
        sessions,
    }
}

/// Capture every live pane's retained output as REPLAYABLE terminal bytes, bounded to `limit`
/// logical lines each — the CONTENT half of the durability ring, paired with [`snapshot`]'s SHAPE
/// half. `limit == 0` captures nothing (history persistence disabled).
///
/// ## Why content is not part of [`Snapshot`]
///
/// A pane's history is orders of magnitude larger than its shape and changes on every scroll,
/// while the shape changes only when the user rearranges something. Folding it into the snapshot
/// DTO would make any shape change — a resize, a `cd` — rewrite every pane's history with it, and
/// would put a stream full of `ESC` bytes through a JSON string escape, tripling it and destroying
/// the "a user can read their saved layout" property the snapshot file has. So the shape stays one
/// small human-readable JSON file and the content becomes one raw, `cat`-able file per pane.
///
/// The two are written at different instants and are deliberately allowed to disagree: a pane born
/// between them has a shape and no history (it restores blank) or a history and no shape (an
/// orphan file the next save reaps). Both degrade to less history, never to a corrupt restore.
///
/// Takes the registry lock and each workspace lock SEQUENTIALLY, never nested — the same discipline
/// [`snapshot`] documents, and for the same deadlock reason.
/// One pane's entry in a history capture: its id, the [`PanePty::history_epoch`](crate::PanePty::history_epoch) the capture observed,
/// and its encoded bytes — or `None` when the epoch matched what the caller already had, so nothing
/// was encoded.
///
/// The epoch travels WITH the bytes rather than being read again by the caller: the two must describe
/// the same instant, and a second read after the lock was released could observe a later one and then
/// store it against older bytes — which is precisely the direction that serves stale history forever.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneHistory {
    /// The pane the entry belongs to.
    pub id: PaneId,
    /// The content epoch at capture time — what a caller stores to compare against next tick.
    pub epoch: u64,
    /// The encoded history, or `None` for "unchanged since `seen`; not encoded".
    pub bytes: Option<Vec<u8>>,
}

/// ## Why the capture takes the epochs it already knows
///
/// Encoding a full scrollback is the expensive part of a save, and on an idle pane it produces bytes
/// identical to last time — so the capture asks each pane's [`PanePty::history_epoch`](crate::PanePty::history_epoch) FIRST and skips
/// the encode outright when it matches `seen`. An idle daemon then costs one `u64` read per pane per
/// tick instead of re-encoding every pane's whole history to discover that nothing moved.
///
/// A skipped pane is still REPORTED (with `bytes: None`), because the caller uses the pane set for
/// more than writing: it reaps the files of panes that are gone, and a pane omitted for being
/// unchanged would read as departed and have its history deleted.
#[must_use]
pub fn pane_histories(
    registry: &Arc<Mutex<SessionRegistry>>,
    limits: HistoryLimits,
    seen: &HashMap<PaneId, u64>,
) -> Vec<PaneHistory> {
    if limits.lines == 0 {
        return Vec::new();
    }
    // Phase 1 — registry lock ONLY: clone out the pools as handles, then release it.
    let pools: Vec<Arc<Mutex<crate::workspace::Workspace>>> = {
        let reg = registry.lock().unwrap_or_else(PoisonError::into_inner);
        reg.sessions()
            .iter()
            .flat_map(|session| session.windows())
            .map(|window| Arc::clone(window.workspace()))
            .collect()
    };
    // Phase 2 — registry lock released; each pool read under its OWN lock.
    pools
        .iter()
        .flat_map(|pool| {
            let pool = pool.lock().unwrap_or_else(PoisonError::into_inner);
            pool.panes()
                .iter()
                .map(|pane| {
                    let id = pane.id();
                    let epoch = pane.pty().history_epoch();
                    // The epoch is read and compared under the SAME lock acquisition the encode would
                    // take, so nothing can mutate between "unchanged" and the decision to skip.
                    // `limits` is the OPERATOR's ceiling and is passed WHOLE. It is deliberately not
                    // narrowed to this pane's own `history_limit`, which looks like the tighter
                    // budget and is not one: `history_bytes` encodes the scrollback AND the visible
                    // screen, so a limit that bounds only retention would cut the live screen out
                    // of a full pane's saved history — and save nothing at all for a pane set to
                    // keep no scrollback. The encoder already saturates at what the screen holds,
                    // so an unset ceiling persists exactly that and needs no help.
                    let bytes =
                        (seen.get(&id) != Some(&epoch)).then(|| pane.pty().history_bytes(limits));
                    PaneHistory { id, epoch, bytes }
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Why restoring a [`Snapshot`] was refused. Every case is a reason the daemon falls back to an
/// EMPTY boot rather than a corrupt one — a bad snapshot must never brick the daemon.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SnapshotError {
    /// The snapshot's `version` is not one this build understands ([`SNAPSHOT_VERSION`]).
    Version { found: u32, expected: u32 },
    /// The shape is malformed: no sessions, a session with no windows, a `current_window` that
    /// names no window, or a duplicate session/window name. The message says which.
    Malformed(String),
    /// A window's stored arrangement is not a well-formed layout — the underlying
    /// [`LayoutError`](crate::LayoutError), rendered.
    Layout(String),
}

impl std::fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Version { found, expected } => {
                write!(
                    f,
                    "snapshot version {found} is not the supported {expected}"
                )
            }
            Self::Malformed(why) => write!(f, "malformed snapshot: {why}"),
            Self::Layout(why) => write!(f, "snapshot layout is not well-formed: {why}"),
        }
    }
}

impl std::error::Error for SnapshotError {}

/// The panes a restore must (re-)spawn, produced alongside the rebuilt registry by
/// [`SessionRegistry::from_snapshot`](crate::SessionRegistry::from_snapshot).
///
/// The registry is rebuilt PANE-FREE (its sessions, windows, layout trees and float sets are all
/// in place, but the pools are empty): a pane must be born at the HOST so it carries the daemon's
/// death-signal (the D4 seam — this pinion-free crate holds no such hook). Each entry names where
/// the host spawns the pane and the facts to spawn it with.
#[derive(Clone, Debug, PartialEq)]
pub struct RestorePlan {
    /// Every pane to re-spawn, in the order it was recorded.
    pub panes: Vec<PaneRestore>,
}

/// One pane the host must re-spawn on restore: which window it belongs to, its old id, and where
/// and how big to spawn its shell.
#[derive(Clone, Debug, PartialEq)]
pub struct PaneRestore {
    /// The session that owns the pane's window.
    pub session: String,
    /// The window the pane docks into.
    pub window: String,
    /// The id to spawn it under (the layout references it by this — see
    /// [`spawn_restored`](crate::Workspace::spawn_restored)).
    pub id: PaneId,
    /// Where to spawn; `None` falls back to the daemon's cwd.
    pub cwd: Option<PathBuf>,
    /// The full argv (`[program, args…]`) — re-run exactly for an allowlisted program, else a
    /// shell in the cwd. Empty (a pre-argv snapshot) restores a shell. The restored pane's display
    /// label is DERIVED from what actually re-ran (`restore_command`), so the recorded
    /// `command_label` is not carried into the plan.
    pub argv: Vec<String>,
    /// The structured remote endpoint (a `sprag ssh` workspace pane), or `None` for a local pane.
    /// `Some` tells the host to RECONNECT (`ssh -t user@host`, allowlist bypassed) rather than run
    /// the recorded argv through the exact-command gate, and to re-mark the restored pane remote.
    pub remote: Option<SshRemote>,
    /// The pane whose occupant asked for this one, or `None`. Carried through the plan for the same
    /// reason [`remote`](Self::remote) is: the host re-stamps it on the reborn pane, and a
    /// provenance the plan dropped could not be recovered from anywhere else.
    pub opened_by: Option<PaneId>,
    /// The name a person gave this pane, or `None`. Carried through the plan on
    /// [`opened_by`](Self::opened_by)'s terms — the host re-stamps it on the reborn pane, and a
    /// name the plan dropped could not be recovered from anywhere else.
    pub name: Option<crate::PaneName>,
    /// The size to open at.
    pub cols: u16,
    pub rows: u16,
}

/// The single-pane view a [`Pane`] exposes for a snapshot, so tests and the registry share one
/// reading of a live pane. (`Pane` itself stays free of snapshot types — this is the boundary.)
pub(crate) fn pane_snapshot(pane: &Pane) -> PaneSnapshot {
    let (cols, rows) = pane.pty().dimensions();
    PaneSnapshot {
        id: pane.id(),
        cwd: pane.pty().cwd(),
        command_label: pane.command_label().to_owned(),
        argv: pane.argv().to_vec(),
        remote: pane.remote().cloned(),
        opened_by: pane.opened_by(),
        name: pane.name().cloned(),
        cols,
        rows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SessionRegistry;

    // The live-pane helpers below (real PTYs, cwd via /proc) are used only by the Linux-gated
    // round-trip test; gate them too so a non-Linux build under `-D warnings` sees no dead code.
    #[cfg(target_os = "linux")]
    use crate::CommandBuilder;
    #[cfg(target_os = "linux")]
    use std::path::Path;

    /// A long-lived `cat` child in `dir`, so a spawned pane's PTY (and its cwd) stay open.
    #[cfg(target_os = "linux")]
    fn cmd_in(dir: &Path) -> CommandBuilder {
        let mut c = CommandBuilder::new("/bin/sh");
        c.arg("-c");
        c.arg("cat");
        c.cwd(dir);
        c.env("TERM", "dumb");
        c
    }

    #[cfg(target_os = "linux")]
    fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        m.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Reconcile the named window's arrangement against its live pane set, so the tree the
    /// snapshot captures reflects the panes (the live path reconciles on read).
    #[cfg(target_os = "linux")]
    fn reconcile(reg: &Arc<Mutex<SessionRegistry>>, session: &str, window: &str) {
        let pool = lock(reg).window_workspace(session, window).unwrap();
        let panes: Vec<PaneId> = lock(&pool).panes().iter().map(Pane::id).collect();
        lock(reg)
            .window_mut(session, window)
            .unwrap()
            .reconcile_layout(&panes);
    }

    /// The window ORDER survives a restart, which since R310 is a USER DECISION rather than the
    /// order a `Vec` happened to fill in.
    ///
    /// Its own test rather than a line in the whole-shape round trip below, because the
    /// discriminator is different: that one asserts every field came back, and a `Vec` restored in
    /// creation order would satisfy it as long as the SET matched. Here the arranged order is
    /// deliberately NOT the creation order, so a restore that rebuilt from births fails.
    ///
    /// REVERT-PROOF: sort `from_snapshot`'s window build by name and this fails while the whole
    /// shape round trip below stays green.
    #[test]
    fn the_window_order_a_user_arranged_survives_a_restart() {
        let mut reg = SessionRegistry::new((80, 24));
        for name in ["a", "b", "c"] {
            reg.new_window("0", Some(name)).expect("a window");
        }
        reg.move_window("0", "c", &crate::WindowPlace::First)
            .expect("c moves to the front");
        reg.select_window("0", "b").expect("sit on b");
        let arranged: Vec<String> = reg
            .session("0")
            .expect("the session")
            .windows()
            .iter()
            .map(|window| window.name().to_owned())
            .collect();
        assert_eq!(
            arranged,
            ["c", "0", "a", "b"],
            "the fixture really is arranged out of creation order, or this test says nothing",
        );

        let snapshot = snapshot(&Arc::new(Mutex::new(reg)));
        let (restored, _plan) =
            SessionRegistry::from_snapshot(snapshot).expect("a well-formed snapshot");
        let back: Vec<String> = restored
            .session("0")
            .expect("the session came back")
            .windows()
            .iter()
            .map(|window| window.name().to_owned())
            .collect();
        assert_eq!(back, arranged, "the order the user arranged came back");
        assert_eq!(
            restored.session("0").unwrap().current_window().name(),
            "b",
            "and the window they were ON, which is stored by NAME and not by index",
        );
    }

    /// THE load-bearing durability claim: a live registry's whole shape — two sessions, the
    /// windows, which is current, the tiled arrangement, a floated pane, a PINNED window size, and
    /// every pane's cwd — serializes to JSON and rebuilds a structurally identical registry plus a
    /// plan naming each pane to re-spawn under its OLD id. A dropped field would restore a DIFFERENT
    /// desktop.
    ///
    /// The pin is deliberately set on ONE of the two sessions, so a `manual_size` that came back by
    /// being written everywhere rather than restored per window fails on the other.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_registry_round_trips_through_a_snapshot() {
        let dir = std::env::temp_dir();
        // A DISTINCTIVE default size (not the 80x24 the panes spawn at), so restoring it is
        // verifiable independently of the per-pane sizes below.
        let reg = Arc::new(Mutex::new(SessionRegistry::new((77, 21))));

        // The default session "0" / window "0": three tiled panes (ids 0,1,2), then float the
        // middle so it leaves the tiling — the float set is session state that must survive.
        let default_pool = lock(&reg).workspace_of("0").unwrap();
        let ids: Vec<PaneId> = (0..3)
            .map(|_| {
                lock(&default_pool)
                    .spawn(cmd_in(&dir), "sh".to_owned(), 80, 24)
                    .unwrap()
            })
            .collect();
        reconcile(&reg, "0", "0");
        let panes: Vec<PaneId> = lock(&default_pool).panes().iter().map(Pane::id).collect();
        lock(&reg)
            .window_mut("0", "0")
            .unwrap()
            .set_floating(ids[1], true, &panes);
        reconcile(&reg, "0", "0");
        // An operator's PINNED window size — a decision, not a reading, and the one size in the file
        // no client can reconstruct. Distinct from the registry default (77x21) and from every pane's
        // own size below, so restoring it is verifiable on its own.
        lock(&reg)
            .window_mut("0", "0")
            .unwrap()
            .set_manual_size(Some((123, 45)));

        // A second session "work" with one pane of its own — a real independent attach unit.
        lock(&reg).new_session(Some("work")).unwrap();
        let work_pool = lock(&reg).workspace_of("work").unwrap();
        lock(&work_pool)
            .spawn(cmd_in(&dir), "claude".to_owned(), 100, 30)
            .unwrap();
        reconcile(&reg, "work", "0");

        let snap = snapshot(&reg);
        assert_eq!(snap.version, SNAPSHOT_VERSION);
        assert_eq!(snap.next_id, 4, "four panes minted across both sessions");

        // JSON is lossless — a dropped field here restores a different layout.
        let json = serde_json::to_string(&snap).expect("a snapshot serializes");
        let back: Snapshot = serde_json::from_str(&json).expect("and round-trips");
        assert_eq!(back, snap, "serde is lossless for the whole shape");

        let (restored, plan) = SessionRegistry::from_snapshot(back).expect("a valid snapshot");

        // The session structure came back: order (the default), names, current window.
        assert_eq!(restored.sessions().len(), 2);
        assert_eq!(restored.default_session().name(), "0");
        assert_eq!(
            restored.session("work").unwrap().current_window().name(),
            "0"
        );

        // The default window's tiling is back with the floated pane OUT of it (0 and 2 tiled),
        // and pane 1 in the float set.
        let win = restored.session("0").unwrap().current_window();
        assert_eq!(
            win.layout().panes(),
            vec![ids[0], ids[2]],
            "the tiling survived"
        );
        assert!(win.floating().contains(&ids[1]), "the float survived");
        assert_eq!(
            win.manual_size(),
            Some((123, 45)),
            "the pinned window size survived the reboot"
        );
        assert_eq!(
            restored
                .session("work")
                .unwrap()
                .current_window()
                .manual_size(),
            None,
            "a window nobody pinned came back un-pinned"
        );

        // The plan names every pane to re-spawn, under its old id and with its cwd.
        assert_eq!(plan.panes.len(), 4, "three in session 0, one in work");
        let work_pane = plan
            .panes
            .iter()
            .find(|p| p.session == "work")
            .expect("work's pane is in the plan");
        assert_eq!(
            work_pane.argv,
            vec!["/bin/sh", "-c", "cat"],
            "the full launch argv survives into the plan — what an exact-command restore re-runs",
        );
        assert_eq!((work_pane.cols, work_pane.rows), (100, 30));
        assert_eq!(
            work_pane.cwd.as_deref().and_then(|c| c.canonicalize().ok()),
            dir.canonicalize().ok(),
            "the restored pane re-spawns in the recorded directory",
        );

        // next_id preserved: a fresh spawn on the RESTORED registry mints above every old id,
        // never reissuing one — the invariant that must hold across a reboot too.
        let restored = Arc::new(Mutex::new(restored));
        let pool = lock(&restored).workspace_of("0").unwrap();
        let fresh = lock(&pool)
            .spawn(cmd_in(&dir), "sh".to_owned(), 80, 24)
            .unwrap();
        assert_eq!(
            fresh,
            PaneId(4),
            "the counter resumed above the restored ids"
        );
        // The default size rode along too — a dimension-less spawn adopts the pre-reboot default,
        // not a hardcoded fallback.
        assert_eq!(
            lock(&pool).default_size(),
            (77, 21),
            "the pre-reboot default size was restored",
        );
    }

    /// A remote workspace pane carries its structured endpoint into the snapshot — the projection
    /// (`pane_snapshot`) reads `pane.remote()`. Revert-proof: drop `remote: pane.remote().cloned()`
    /// in the projection and this is `None`, so a restore would never know to reconnect.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_remote_pane_carries_its_endpoint_into_the_snapshot() {
        use crate::SshRemote;
        let dir = std::env::temp_dir();
        let reg = Arc::new(Mutex::new(SessionRegistry::new((80, 24))));
        let pool = lock(&reg).workspace_of("0").unwrap();
        let endpoint = SshRemote {
            user: Some("me".to_owned()),
            host: "srv".to_owned(),
            port: Some(22),
        };
        let id = {
            let mut ws = lock(&pool);
            let id = ws.spawn(cmd_in(&dir), "ssh".to_owned(), 80, 24).unwrap();
            ws.set_pane_remote(id, endpoint.clone());
            id
        };
        let ws = lock(&pool);
        let snap = pane_snapshot(ws.pane(id).unwrap());
        assert_eq!(snap.remote, Some(endpoint));
    }

    /// A pane's PROVENANCE reaches the restore plan the host acts on, which is the claim that
    /// matters — a snapshot field the plan dropped would restore every agent-opened pane as one
    /// nobody asked for, and the agent surface's close gate reads exactly that fact.
    #[test]
    fn a_panes_opener_reaches_the_restore_plan() {
        let mut snap = snap_of("0", vec![win("0", vec![pane(0), pane(1)])]);
        snap.sessions[0].windows[0].panes[1].opened_by = Some(PaneId(0));
        let (_, plan) = SessionRegistry::from_snapshot(snap).unwrap();
        assert_eq!(
            plan.panes
                .iter()
                .map(|p| (p.id, p.opened_by))
                .collect::<Vec<_>>(),
            vec![(PaneId(0), None), (PaneId(1), Some(PaneId(0)))],
            "the pane that was asked for comes back knowing who asked, and the one that was not \
             comes back claimed by nobody",
        );
    }

    /// A snapshot written before provenance existed still loads, and every pane in it comes back
    /// claimed by nobody — the additive half of the field, which is what keeps a user's saved
    /// sessions readable across the upgrade.
    #[test]
    fn a_pre_provenance_snapshot_loads_with_no_opener() {
        let stored = serde_json::json!({
            "id": 4,
            "cwd": null,
            "command_label": "sh",
            "argv": ["sh"],
            "cols": 80,
            "rows": 24,
        });
        let decoded: PaneSnapshot = serde_json::from_value(stored).expect("a pre-R294 pane loads");
        assert_eq!(decoded.opened_by, None);
        assert_eq!(decoded.name, None, "and a pre-R295 one comes back unnamed");
    }

    /// A pane's NAME reaches the restore plan, on the provenance test's grounds and one stronger:
    /// the name is an ADDRESS, so a script or an agent that says `--pane build` resolves to nothing
    /// after a reboot unless it comes back with the pane.
    #[test]
    fn a_panes_name_reaches_the_restore_plan() {
        let mut snap = snap_of("0", vec![win("0", vec![pane(0), pane(1)])]);
        snap.sessions[0].windows[0].panes[1].name = Some(crate::PaneName::parse("build").unwrap());
        let (_, plan) = SessionRegistry::from_snapshot(snap).unwrap();
        assert_eq!(
            plan.panes
                .iter()
                .map(|p| (p.id, p.name.as_ref().map(crate::PaneName::as_str)))
                .collect::<Vec<_>>(),
            vec![(PaneId(0), None), (PaneId(1), Some("build"))],
            "the named pane comes back named and the unnamed one comes back unnamed",
        );
    }

    /// The registry-wide uniqueness of a NAME is re-established by the load, exactly as an id's is
    /// — a hand-edited file claiming one name twice would restore into a set where `--pane build`
    /// has two answers, and the resolver would then have to guess.
    #[test]
    fn a_snapshot_naming_two_panes_the_same_is_malformed() {
        let mut snap = snap_of("0", vec![win("0", vec![pane(0), pane(1)])]);
        let name = crate::PaneName::parse("build").unwrap();
        snap.sessions[0].windows[0].panes[0].name = Some(name.clone());
        snap.sessions[0].windows[0].panes[1].name = Some(name);
        let Err(error) = SessionRegistry::from_snapshot(snap) else {
            panic!("a duplicate name is refused");
        };
        assert!(
            matches!(&error, SnapshotError::Malformed(m) if m.contains("build")),
            "and the refusal names the offending name, not just the rule: {error}",
        );
    }

    /// A name the LIVE surfaces refuse cannot enter through the file either. The parse is on
    /// `PaneName`'s own `Deserialize`, so this holds for every reader of a snapshot rather than for
    /// whichever one remembered to check.
    #[test]
    fn a_snapshot_cannot_smuggle_in_a_name_the_front_door_refuses() {
        let forged = serde_json::json!({
            "id": 4,
            "cwd": null,
            "command_label": "sh",
            "argv": ["sh"],
            "name": "build\n9: 80x24  sh",
            "cols": 80,
            "rows": 24,
        });
        assert!(
            serde_json::from_value::<PaneSnapshot>(forged).is_err(),
            "a file is not a trusted input, however this daemon wrote it",
        );
    }

    /// A snapshot whose version this build does not understand is REFUSED — the daemon boots
    /// empty rather than parsing a format it cannot.
    #[test]
    fn an_unknown_version_is_refused() {
        let snap = Snapshot {
            version: SNAPSHOT_VERSION + 99,
            next_id: 0,
            default_size: (80, 24),
            sessions: vec![SessionSnapshot {
                name: "0".to_owned(),
                current_window: "0".to_owned(),
                windows: vec![win("0", vec![])],
            }],
        };
        assert!(matches!(
            SessionRegistry::from_snapshot(snap),
            Err(SnapshotError::Version { .. }),
        ));
    }

    /// The migration's two directions, which are not the same claim: every format back to
    /// [`MIN_READABLE_SNAPSHOT_VERSION`] restores, and one from a NEWER build still does not.
    ///
    /// Worth pinning as a range rather than trusting the constants, because the bump that made
    /// the range necessary would have cost every user their saved sessions had the loader kept
    /// testing for equality — and equality is what it had, and what still passes every other
    /// version test in this file.
    #[test]
    fn every_readable_version_restores_and_a_newer_one_does_not() {
        let snap_at = |version| Snapshot {
            version,
            next_id: 0,
            default_size: (80, 24),
            sessions: vec![SessionSnapshot {
                name: "0".to_owned(),
                current_window: "0".to_owned(),
                windows: vec![win("0", vec![])],
            }],
        };
        for version in MIN_READABLE_SNAPSHOT_VERSION..=SNAPSHOT_VERSION {
            assert!(
                SessionRegistry::from_snapshot(snap_at(version)).is_ok(),
                "a version {version} snapshot must restore, not boot the user away",
            );
        }
        assert!(
            matches!(
                SessionRegistry::from_snapshot(snap_at(SNAPSHOT_VERSION + 1)),
                Err(SnapshotError::Version { .. }),
            ),
            "a snapshot from a newer build is still refused by name",
        );
    }

    /// A malformed shape (here: a `current_window` that names no window) is refused with a
    /// message, not restored into a registry whose current-window pointer would be invalid.
    #[test]
    fn a_current_window_that_names_nothing_is_refused() {
        let snap = Snapshot {
            version: SNAPSHOT_VERSION,
            next_id: 0,
            default_size: (80, 24),
            sessions: vec![SessionSnapshot {
                name: "0".to_owned(),
                current_window: "ghost".to_owned(), // no such window
                windows: vec![win("0", vec![])],
            }],
        };
        assert!(matches!(
            SessionRegistry::from_snapshot(snap),
            Err(SnapshotError::Malformed(_)),
        ));
    }

    /// An empty session list is refused — a registry is never empty, so restoring one would
    /// unresolve the default session.
    #[test]
    fn an_empty_snapshot_is_refused() {
        let snap = Snapshot {
            version: SNAPSHOT_VERSION,
            next_id: 0,
            default_size: (80, 24),
            sessions: vec![],
        };
        assert!(matches!(
            SessionRegistry::from_snapshot(snap),
            Err(SnapshotError::Malformed(_)),
        ));
    }

    /// An unpinned, empty window named `name` with the given panes — for building malformed
    /// snapshots.
    fn win(name: &str, panes: Vec<PaneSnapshot>) -> WindowSnapshot {
        WindowSnapshot {
            name: name.to_owned(),
            layout: LayoutWire::default(),
            floating: vec![],
            panes,
            manual_size: None,
            active: None,
            zoomed: None,
        }
    }

    /// A one-pane restore fact, for populating a window.
    fn pane(id: u64) -> PaneSnapshot {
        PaneSnapshot {
            id: PaneId(id),
            cwd: None,
            command_label: "sh".to_owned(),
            argv: vec!["sh".to_owned()],
            remote: None,
            opened_by: None,
            name: None,
            cols: 80,
            rows: 24,
        }
    }

    /// A snapshot of one session over the given windows — the malformed-shape fixture.
    fn snap_of(current: &str, windows: Vec<WindowSnapshot>) -> Snapshot {
        Snapshot {
            version: SNAPSHOT_VERSION,
            next_id: 9,
            default_size: (80, 24),
            sessions: vec![SessionSnapshot {
                name: "0".to_owned(),
                current_window: current.to_owned(),
                windows,
            }],
        }
    }

    /// A session with two windows sharing a name is refused — a window name is an address, so a
    /// duplicate would make it ambiguous.
    #[test]
    fn a_duplicate_window_name_is_refused() {
        let snap = snap_of("0", vec![win("0", vec![]), win("0", vec![])]);
        assert!(matches!(
            SessionRegistry::from_snapshot(snap),
            Err(SnapshotError::Malformed(_)),
        ));
    }

    /// Two sessions sharing a name is refused (the session-level address analogue).
    #[test]
    fn a_duplicate_session_name_is_refused() {
        let mut snap = snap_of("0", vec![win("0", vec![])]);
        snap.sessions.push(SessionSnapshot {
            name: "0".to_owned(),
            current_window: "0".to_owned(),
            windows: vec![win("0", vec![])],
        });
        assert!(matches!(
            SessionRegistry::from_snapshot(snap),
            Err(SnapshotError::Malformed(_)),
        ));
    }

    /// A session with no windows is refused — a session always has at least one, which is what
    /// makes its current-window total.
    #[test]
    fn a_session_with_no_windows_is_refused() {
        let snap = snap_of("0", vec![]);
        assert!(matches!(
            SessionRegistry::from_snapshot(snap),
            Err(SnapshotError::Malformed(_)),
        ));
    }

    /// Two panes claiming one id is refused — the global-unique-PaneId invariant. A hand-edited
    /// state file is the only way to reach it (sprag's writer mints unique ids), and without this
    /// check `spawn_restored` would push both, leaving id-addressed reads ambiguous.
    #[test]
    fn a_duplicate_pane_id_is_refused() {
        let snap = snap_of("0", vec![win("0", vec![pane(5), pane(5)])]);
        assert!(
            matches!(
                SessionRegistry::from_snapshot(snap),
                Err(SnapshotError::Malformed(_)),
            ),
            "a snapshot with two panes at id 5 must boot empty, not id-collide",
        );
        // …and across DIFFERENT windows, since ids are registry-global, not per-window.
        let snap = snap_of("0", vec![win("0", vec![pane(3)]), win("1", vec![pane(3)])]);
        assert!(matches!(
            SessionRegistry::from_snapshot(snap),
            Err(SnapshotError::Malformed(_)),
        ));
    }

    /// THE upgrade-safety claim: a slice-1 snapshot JSON — a pane with NO `argv` field — still
    /// deserializes (`#[serde(default)]` fills it empty) and restores, so upgrading sprag never
    /// rejects a user's saved state. An empty argv carries into the plan and restores as a shell.
    #[test]
    fn a_pre_argv_snapshot_still_loads_and_restores_as_a_shell() {
        let json = r#"{
            "version": 1,
            "next_id": 1,
            "default_size": [80, 24],
            "sessions": [{
                "name": "0",
                "current_window": "0",
                "windows": [{
                    "name": "0",
                    "layout": {"root": null},
                    "floating": [],
                    "panes": [
                        {"id": 0, "cwd": null, "command_label": "sh", "cols": 80, "rows": 24}
                    ]
                }]
            }]
        }"#;
        let snap: Snapshot = serde_json::from_str(json).expect("a pre-argv snapshot deserializes");
        assert_eq!(
            snap.sessions[0].windows[0].panes[0].argv,
            Vec::<String>::new(),
            "a missing argv defaults to empty — the serde-default upgrade safety",
        );
        let (_registry, plan) = SessionRegistry::from_snapshot(snap).expect("it restores");
        assert_eq!(plan.panes.len(), 1);
        assert!(
            plan.panes[0].argv.is_empty(),
            "the empty argv carries into the plan, so the pane restores as a shell",
        );
    }

    /// **A boundary a user MOVED comes back where they left it.**
    ///
    /// The whole point of storing a share rather than re-deriving one: a resize is not a gesture,
    /// it is a decision about how much room a program needs, and a restore that reverted every
    /// window to an even split would throw that decision away on the one event a durable
    /// multiplexer exists to survive.
    ///
    /// It is asserted on the RESTORED tree rather than on the file, because the file is not what a
    /// user gets back. Every other ratio in this module's fixtures is `0.5`, which is also what a
    /// fresh split mints — so before this, a restore that dropped the stored share and re-minted an
    /// even one would have passed every test here.
    #[test]
    fn a_moved_boundary_comes_back_where_it_was_left() {
        let mut window = win("0", vec![pane(0), pane(1)]);
        // Not 0.5, and not a round number: a restore that re-minted an even split, or one that
        // rounded through a percentage, would land somewhere this cannot be mistaken for.
        let moved = 0.687_5_f32;
        window.layout = LayoutWire::from(&{
            let mut tree = crate::LayoutTree::new();
            tree.append_pane(PaneId(0));
            tree.append_pane(PaneId(1));
            let split = tree
                .divider_on(PaneId(0), crate::SplitDir::Horizontal)
                .at()
                .expect("two appended panes are divided horizontally");
            tree.set_from_wire(
                crate::with_ratio(&LayoutWire::from(&tree), split, moved)
                    .expect("the split is there"),
            )
            .expect("a re-weighted tree is still well-formed");
            tree
        });

        let (registry, _plan) = SessionRegistry::from_snapshot(snap_of("0", vec![window]))
            .expect("a well-formed arrangement restores");
        let restored = registry
            .window("0", "0")
            .expect("the window came back")
            .layout()
            .root()
            .cloned()
            .expect("with its arrangement");
        match restored {
            crate::LayoutNode::Split { ratio, .. } => assert!(
                (ratio - moved).abs() < f32::EPSILON,
                "the stored share came back exactly: {ratio} != {moved}",
            ),
            other => panic!("a two-pane window restores as a split, got {other:?}"),
        }
    }

    /// A stored layout that is not well-formed (here: the same pane twice) is refused as
    /// `SnapshotError::Layout` — the `set_from_wire` validation riding out through `Window::restore`.
    #[test]
    fn a_malformed_stored_layout_is_refused() {
        let mut window = win("0", vec![pane(0), pane(1)]);
        // A tree with pane 0 in two leaves — set_from_wire rejects it as DuplicatePane.
        window.layout = LayoutWire {
            root: Some(crate::LayoutNodeWire::Split {
                id: None,
                dir: crate::SplitDir::Horizontal,
                ratio: 0.5,
                first: Box::new(crate::LayoutNodeWire::Leaf(PaneId(0))),
                second: Box::new(crate::LayoutNodeWire::Leaf(PaneId(0))),
            }),
        };
        let snap = snap_of("0", vec![window]);
        assert!(
            matches!(
                SessionRegistry::from_snapshot(snap),
                Err(SnapshotError::Layout(_)),
            ),
            "a corrupt stored arrangement boots empty via the Layout error, not a bad tree",
        );
    }

    /// THE payoff of the epoch gate: a capture over an IDLE registry encodes nothing at all.
    ///
    /// The save loop runs every few seconds for a daemon's whole life, and an idle pane's history is
    /// identical every time — so re-encoding a full scrollback to discover that was pure waste. Here
    /// the second capture, handed the first's epochs, reports every pane live with `bytes: None`: not
    /// merely "the same bytes", but no encode performed.
    ///
    /// Then a pane PRINTS, and only that pane re-encodes — the gate has to let real change through
    /// per-pane, not just recognise a wholly idle daemon.
    ///
    /// REVERT-PROOF: ignoring `seen` and always encoding leaves every liveness assertion passing and
    /// fails the `is_none` ones; keying the gate on something that never moves (a constant epoch) fails
    /// the print assertion instead.
    #[cfg(target_os = "linux")]
    #[test]
    fn an_idle_capture_encodes_nothing_and_a_printing_pane_still_does() {
        let dir = std::env::temp_dir();
        let reg = Arc::new(Mutex::new(SessionRegistry::new((80, 24))));
        let pool = lock(&reg).workspace_of("0").unwrap();
        let ids: Vec<PaneId> = (0..2)
            .map(|_| {
                lock(&pool)
                    .spawn(cmd_in(&dir), "sh".to_owned(), 80, 24)
                    .unwrap()
            })
            .collect();
        reconcile(&reg, "0", "0");

        // First capture: nothing is known yet, so every pane encodes.
        let seen = HashMap::new();
        let first = pane_histories(&reg, HistoryLimits::text_only(100), &seen);
        assert_eq!(first.len(), 2, "both panes are captured");
        assert!(
            first.iter().all(|entry| entry.bytes.is_some()),
            "an unknown pane must be encoded",
        );
        let seen: HashMap<PaneId, u64> = first.iter().map(|e| (e.id, e.epoch)).collect();

        // Second capture over an untouched registry: every pane still reported, none encoded.
        let idle = pane_histories(&reg, HistoryLimits::text_only(100), &seen);
        assert_eq!(idle.len(), 2, "a skipped pane is still reported LIVE");
        assert!(
            idle.iter().all(|entry| entry.bytes.is_none()),
            "an idle pane's history is not encoded at all",
        );
        assert_eq!(
            idle.iter().map(|e| e.epoch).collect::<Vec<_>>(),
            first.iter().map(|e| e.epoch).collect::<Vec<_>>(),
            "and its epoch is unchanged, so the next tick skips it too",
        );

        // Pane 0 prints. Wait on the CONDITION the assertion reads — the epoch actually moving —
        // rather than on a timer, since the reader thread applies the bytes asynchronously.
        lock(&pool)
            .pane(ids[0])
            .unwrap()
            .pty()
            .write(b"marker\n")
            .expect("write to the pane's pty");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let now = pane_histories(&reg, HistoryLimits::text_only(100), &seen);
            let changed: Vec<PaneId> = now
                .iter()
                .filter(|e| e.bytes.is_some())
                .map(|e| e.id)
                .collect();
            if changed == vec![ids[0]] {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the printing pane never re-encoded (changed: {changed:?})",
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// A pane that retains NO scrollback still has its visible screen captured.
    ///
    /// The guard on a coupling that looks right and is not. A pane now owns a `history-limit`, so
    /// narrowing the save budget to it reads like the tighter, more correct bound — and it is wrong,
    /// because a saved history is the scrollback PLUS the visible screen. At a limit of zero that
    /// narrowing captures nothing, and the pane comes back from a reboot BLANK rather than showing
    /// what was on it.
    ///
    /// REVERT-PROOF: passing `limits.lines.min(pane.pty().history_limit())` to `history_bytes`
    /// instead of `limits` fails this on an empty capture. Nothing else in the suite moves, which is
    /// exactly why it is written here.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_pane_retaining_no_scrollback_still_captures_its_visible_screen() {
        let dir = std::env::temp_dir();
        let reg = Arc::new(Mutex::new(SessionRegistry::new((80, 24))));
        let pool = reg
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .workspace_of("0")
            .unwrap();
        // Zero retention: the setting a user picks for a pane that must remember nothing.
        lock(&pool).set_history_limit_source(Arc::new(|| 0));
        let id = lock(&pool)
            .spawn(cmd_in(&dir), "sh".to_owned(), 80, 24)
            .unwrap();
        reconcile(&reg, "0", "0");
        assert_eq!(
            lock(&pool).pane(id).unwrap().pty().history_limit(),
            0,
            "the pane really was born with no retention",
        );

        // `cat` echoes, so this lands on the VISIBLE screen with nothing scrolling off.
        lock(&pool)
            .pane(id)
            .unwrap()
            .pty()
            .write(b"on the screen\n")
            .expect("write to the pane's pty");

        // Wait on the CONDITION the assertion reads — the text reaching the capture.
        let seen = HashMap::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let captured = pane_histories(&reg, HistoryLimits::text_only(usize::MAX), &seen)
                .into_iter()
                .find(|entry| entry.id == id)
                .and_then(|entry| entry.bytes)
                .unwrap_or_default();
            if String::from_utf8_lossy(&captured).contains("on the screen") {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "a zero-retention pane must still save its visible screen, got {:?}",
                String::from_utf8_lossy(&captured),
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}
