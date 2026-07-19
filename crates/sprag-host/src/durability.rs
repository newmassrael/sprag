//! On-disk persistence for the durability ring — WHERE a [`Snapshot`] lives and how it is
//! written and read.
//!
//! The projection (registry → [`Snapshot`]) and the rebuild ([`Host::restore`](crate::Host)) are
//! elsewhere; this module is only the file: resolving a persistent path, an ATOMIC save, and a
//! FAIL-SAFE load.
//!
//! ## Why the path is not beside the socket
//!
//! The daemon's socket and its `.lock` / `.log` siblings live in `$XDG_RUNTIME_DIR`, which is
//! tmpfs — CLEARED on the very reboot this ring exists to survive. So the snapshot lives in the
//! persistent STATE dir (`$XDG_STATE_HOME`, else `~/.local/state`) instead, keyed on the socket's
//! identity so two daemons on two sockets (tmux's per-socket-server model) keep two snapshots.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use sprag_terminal::{SessionRegistry, Snapshot, snapshot};

/// The persistent snapshot path for the daemon on `socket`.
///
/// `$XDG_STATE_HOME/sprag/<socket-stem>.snapshot.json`, falling back to
/// `~/.local/state/sprag/…` then `/tmp/sprag/…`. Keyed on the socket's file stem (e.g.
/// `sprag-host.sock` → `sprag-host`), so it mirrors the `.lock` / `.log` derivation but lives in
/// the persistent dir rather than the ephemeral runtime one — the whole point being to outlive a
/// reboot.
#[must_use]
pub fn snapshot_path(socket: &Path) -> PathBuf {
    let state_dir = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    let key = socket
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("sprag-host");
    state_dir.join("sprag").join(format!("{key}.snapshot.json"))
}

/// Write `snapshot` to `path` ATOMICALLY: serialize to a sibling temp, then rename it over the
/// target. A crash mid-write leaves the temp (harmless) or the previous good snapshot, never a
/// half-written file a restore would choke on. Creates the parent dir if absent.
///
/// # Errors
///
/// An [`io::Error`] if the directory cannot be created, the temp cannot be written, or the
/// rename fails (serialization failure is surfaced as [`io::ErrorKind::Other`]).
pub fn save_snapshot(path: &Path, snapshot: &Snapshot) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_vec_pretty(snapshot).map_err(io::Error::other)?;
    // A per-target temp in the SAME directory, so the rename is atomic (same filesystem). One
    // daemon owns this path (the single-instance flock), so a fixed `.tmp` suffix cannot collide.
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, &json)?;
    std::fs::rename(&tmp, path)
}

/// Read the snapshot at `path`, or `None` if there is none to restore — the FAIL-SAFE load.
///
/// `None` for a missing file (first boot, or nothing saved yet) AND for an unreadable or
/// unparseable one (a truncated write from a crash, a foreign file). Either way the daemon boots
/// EMPTY rather than propagating an error: a bad snapshot must never brick the daemon. Version
/// mismatch is NOT rejected here — that is
/// [`SessionRegistry::from_snapshot`](sprag_terminal::SessionRegistry::from_snapshot)'s job, so an
/// operator sees a specific "unsupported version" log rather than a silent empty boot.
#[must_use]
pub fn load_snapshot(path: &Path) -> Option<Snapshot> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// One durability save STEP: project `registry` to a [`Snapshot`] and write it to `path` ONLY if
/// it differs from `last` (the previously-saved one), updating `last` on a successful write.
/// Returns whether it wrote.
///
/// The daemon's save loop is just this on a timer; splitting it out makes the WRITE-IF-CHANGED
/// dedup — the thing that keeps an idle daemon from rewriting an identical file every tick —
/// testable without a running daemon. On a write error `last` is left unchanged (the `?` returns
/// before it is updated), so the loop retries the same shape next tick rather than silently
/// dropping it.
///
/// # Errors
///
/// The [`io::Error`] from [`save_snapshot`] if the write fails.
pub fn save_if_changed(
    path: &Path,
    registry: &Arc<Mutex<SessionRegistry>>,
    last: &mut Option<Snapshot>,
) -> io::Result<bool> {
    let snap = snapshot(registry);
    if last.as_ref() == Some(&snap) {
        return Ok(false); // nothing changed since the last save — no redundant write
    }
    save_snapshot(path, &snap)?;
    *last = Some(snap);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sprag_terminal::{LayoutWire, SNAPSHOT_VERSION, SessionSnapshot, WindowSnapshot};

    fn a_snapshot() -> Snapshot {
        Snapshot {
            version: SNAPSHOT_VERSION,
            next_id: 3,
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
        }
    }

    /// The path lands in the persistent STATE dir, not the ephemeral runtime dir, and is keyed on
    /// the socket's stem — so a reboot (which wipes the runtime dir) leaves it standing.
    #[test]
    fn snapshot_path_is_keyed_on_the_socket_stem_under_the_state_dir() {
        // Save and restore the prior value so this test does not leak env state into others.
        let prior = std::env::var_os("XDG_STATE_HOME");
        // SAFETY: single-threaded test; no other thread reads the environment concurrently.
        unsafe { std::env::set_var("XDG_STATE_HOME", "/state") };
        let path = snapshot_path(Path::new("/run/user/1000/sprag-host.sock"));
        assert_eq!(path, Path::new("/state/sprag/sprag-host.snapshot.json"));
        // A second daemon on its own socket gets its own snapshot.
        let other = snapshot_path(Path::new("/tmp/sp99.sock"));
        assert_eq!(other, Path::new("/state/sprag/sp99.snapshot.json"));
        unsafe {
            match prior {
                Some(value) => std::env::set_var("XDG_STATE_HOME", value),
                None => std::env::remove_var("XDG_STATE_HOME"),
            }
        }
    }

    /// A save then load round-trips the snapshot, and the temp is renamed away (not left as
    /// litter). This checks the round-trip and the no-litter half; the ATOMICITY property proper
    /// (a crash mid-write leaves the previous good file, never a half-written one) is by
    /// construction — write-a-sibling-temp-then-rename, `save_snapshot` — and cannot be exercised
    /// here without injecting a crash between the two syscalls.
    #[test]
    fn save_then_load_round_trips_and_renames_the_temp_away() {
        let dir = std::env::temp_dir().join(format!("sprag-dura-{}", std::process::id()));
        let path = dir.join("sprag-host.snapshot.json");
        let snap = a_snapshot();

        save_snapshot(&path, &snap).expect("save");
        let back = load_snapshot(&path).expect("load");
        assert_eq!(back, snap, "the snapshot round-trips through disk");

        let tmp = {
            let mut t = path.as_os_str().to_owned();
            t.push(".tmp");
            PathBuf::from(t)
        };
        assert!(
            !tmp.exists(),
            "the temp was renamed onto the target, not left behind"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The write-if-changed dedup: the first step WRITES, an unchanged shape SKIPS, and a real
    /// change WRITES again. This is what keeps an idle daemon from rewriting an identical file
    /// every tick. Reverting the `last == Some(snap)` guard makes the second call write (returning
    /// `true`), so the skip assertions are non-vacuous.
    #[test]
    fn save_if_changed_writes_once_then_skips_until_the_shape_changes() {
        let reg = Arc::new(Mutex::new(SessionRegistry::new((80, 24))));
        let dir = std::env::temp_dir().join(format!("sprag-dura-chg-{}", std::process::id()));
        let path = dir.join("s.json");
        let mut last: Option<Snapshot> = None;

        assert!(
            save_if_changed(&path, &reg, &mut last).expect("write"),
            "the first step writes (nothing saved yet)",
        );
        assert!(
            !save_if_changed(&path, &reg, &mut last).expect("skip"),
            "an unchanged shape skips the write",
        );
        // Change the shape — add a session.
        reg.lock().unwrap().new_session(Some("work")).unwrap();
        assert!(
            save_if_changed(&path, &reg, &mut last).expect("write"),
            "a changed shape writes again",
        );
        assert!(
            !save_if_changed(&path, &reg, &mut last).expect("skip"),
            "and then skips once more",
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A missing file and a corrupt file both load as `None` — the fail-safe that boots the daemon
    /// empty rather than crashing it on a truncated write.
    #[test]
    fn a_missing_or_corrupt_snapshot_loads_as_none() {
        let dir = std::env::temp_dir().join(format!("sprag-dura-bad-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let missing = dir.join("nope.json");
        assert!(load_snapshot(&missing).is_none(), "no file -> None");

        let corrupt = dir.join("corrupt.json");
        std::fs::write(&corrupt, b"{ this is not json").expect("write corrupt");
        assert!(
            load_snapshot(&corrupt).is_none(),
            "an unparseable file -> None, not a panic",
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
