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
//! The SHAPE survives: sessions (their order IS the default), each session's windows and which
//! is current, each window's [`LayoutTree`](crate::LayoutTree) arrangement, its float set, and —
//! per pane — its id,
//! working directory, launch label and size. The global pane-id high-water mark rides too, so a
//! restore never reissues a retired id.
//!
//! A live PTY, its child process, its scrollback and a running agent's in-memory state do NOT —
//! a reboot ends them. On restore each pane re-spawns a fresh shell IN ITS RECORDED CWD (slice 1;
//! re-running the exact command is a later, allowlisted increment), which is the honest cmux
//! analogue: the pane and its directory come back, and an agent resumes its own state through its
//! own tool. The snapshot carries `command_label` for display and for that future increment.
//!
//! ## Homes are not persisted (a documented bound)
//!
//! A window's [`FloatHome`](crate::layout::FloatHome) sidecar — where a floated pane docks back —
//! is a non-authoritative memo with a defined graceful fallback (dock back at the END). It is
//! deliberately left out of the snapshot in slice 1: the pane's FLOAT membership survives, so the
//! user's choice to float it does; only the exact dock-back slot degrades to an append after a
//! reboot, which is the same "a home is a memo, not a promise" fallback the live path already
//! honors.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use crate::layout::LayoutWire;
use crate::registry::SessionRegistry;
use crate::workspace::{Pane, PaneId};

/// The on-disk snapshot format version. Bumped when the shape changes incompatibly; a loader
/// that reads a version it does not understand refuses (`SnapshotError::Version`) and the daemon
/// falls back to an EMPTY boot rather than crashing on a format it cannot parse.
pub const SNAPSHOT_VERSION: u32 = 1;

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
    /// The session's windows, in creation order.
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
}

/// One pane's restore facts: enough to re-spawn a shell where the pane was and address it by its
/// old id. NOT the live PTY — that dies with the reboot.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PaneSnapshot {
    /// The pane's registry-global id — restored EXACTLY, because the layout tree, float set and
    /// homes all reference the pane by it (see
    /// [`spawn_with_dirty_id`](crate::Workspace::spawn_with_dirty_id)).
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
    /// [`spawn_with_dirty_id`](crate::Workspace::spawn_with_dirty_id)).
    pub id: PaneId,
    /// Where to spawn; `None` falls back to the daemon's cwd.
    pub cwd: Option<PathBuf>,
    /// The full argv (`[program, args…]`) — re-run exactly for an allowlisted program, else a
    /// shell in the cwd. Empty (a pre-argv snapshot) restores a shell. The restored pane's display
    /// label is DERIVED from what actually re-ran (`restore_command`), so the recorded
    /// `command_label` is not carried into the plan.
    pub argv: Vec<String>,
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

    /// THE load-bearing durability claim: a live registry's whole shape — two sessions, the
    /// windows, which is current, the tiled arrangement, a floated pane, and every pane's cwd —
    /// serializes to JSON and rebuilds a structurally identical registry plus a plan naming each
    /// pane to re-spawn under its OLD id. A dropped field would restore a DIFFERENT desktop.
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
                windows: vec![WindowSnapshot {
                    name: "0".to_owned(),
                    layout: LayoutWire::default(),
                    floating: vec![],
                    panes: vec![],
                }],
            }],
        };
        assert!(matches!(
            SessionRegistry::from_snapshot(snap),
            Err(SnapshotError::Version { .. }),
        ));
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
                windows: vec![WindowSnapshot {
                    name: "0".to_owned(),
                    layout: LayoutWire::default(),
                    floating: vec![],
                    panes: vec![],
                }],
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

    /// An empty window named `name` with the given panes — for building malformed snapshots.
    fn win(name: &str, panes: Vec<PaneSnapshot>) -> WindowSnapshot {
        WindowSnapshot {
            name: name.to_owned(),
            layout: LayoutWire::default(),
            floating: vec![],
            panes,
        }
    }

    /// A one-pane restore fact, for populating a window.
    fn pane(id: u64) -> PaneSnapshot {
        PaneSnapshot {
            id: PaneId(id),
            cwd: None,
            command_label: "sh".to_owned(),
            argv: vec!["sh".to_owned()],
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
    /// check `spawn_with_dirty_id` would push both, leaving id-addressed reads ambiguous.
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
}
