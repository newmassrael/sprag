//! Per-pane scrollback persistence — the CONTENT half of the durability ring.
//!
//! [`durability`](crate::durability) carries a workspace's SHAPE across a reboot: the sessions,
//! windows, layout, working directories and commands. What it cannot carry is what the panes
//! actually SAID — a live PTY dies with the daemon, and a restored pane opens blank. This module
//! is the other half: every pane's retained output, encoded by
//! [`Screen::history_bytes`](sprag_vt::Screen::history_bytes) as replayable terminal bytes and
//! written beside the snapshot, so a restored pane comes back with its scrollback intact and
//! `sprag find` can still search it.
//!
//! ## One raw file per pane, not a field in the snapshot
//!
//! The snapshot is a small, human-readable JSON projection of the workspace shape, rewritten
//! whenever that shape changes. History is neither small nor shaped: it is hundreds of kilobytes
//! per pane that changes on every scroll, and it is a byte stream full of `ESC` — which a JSON
//! string escape would inflate several-fold while destroying the "a user can read their saved
//! layout" property the snapshot file has. Folding it in would also mean a single `cd` rewrites
//! every pane's history along with it.
//!
//! So each pane gets `<state>/sprag/<socket-stem>.history/<pane-id>.hist`, holding exactly the
//! bytes a replay needs — `cat` one and you see the pane. Pane ids are never reused (the snapshot
//! carries the high-water mark), so a file can never be mis-attributed to a later pane.
//!
//! ## What lands on disk, and the knob that stops it
//!
//! This writes a pane's OUTPUT to disk, which is a broader exposure than the snapshot's argv: a
//! printed token, a `git diff` of a secrets file, anything the user read in that pane. The files
//! are written owner-only (0600) inside an owner-only directory (0700) through the ring's one
//! [`write_atomic_private`](crate::durability) policy, the same protection the argv already
//! relies on.
//!
//! `SPRAG_RESTORE_HISTORY` is the operator's control ([`history_limit`]): a line count, or `0` to
//! turn persistence off entirely. Turning it off is not destructive — the daemon simply stops
//! saving and stops restoring; `sprag kill-server --purge` remains the one verb that DESTROYS
//! saved state, exactly as it is for the snapshot.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use sprag_terminal::{PaneId, SessionRegistry, pane_histories};
use sprag_vt::SCROLLBACK_CAP;

use crate::durability::{socket_key, sprag_state_dir, write_atomic_private};

/// How many logical lines of each pane's output survive a restart by default.
///
/// Derived from the emulator's own retention cap rather than restated, so the default is exactly
/// "everything the pane still holds" and cannot drift away from it.
pub const DEFAULT_HISTORY_LINES: usize = SCROLLBACK_CAP;

/// The file extension a pane's history is stored under.
const HISTORY_EXTENSION: &str = "hist";

/// The per-pane history directory for the daemon on `socket` — a sibling of its snapshot,
/// `<state>/sprag/<socket-stem>.history/`.
///
/// Keyed on the same socket identity as [`snapshot_path`](crate::snapshot_path), so two daemons on
/// two sockets keep two independent histories, and both artifacts live or die together under
/// `kill-server --purge`.
#[must_use]
pub fn history_dir(socket: &Path) -> PathBuf {
    sprag_state_dir().join(format!("{}.history", socket_key(socket)))
}

/// Where pane `id`'s history lives inside `dir`.
fn pane_history_path(dir: &Path, id: PaneId) -> PathBuf {
    dir.join(format!("{}.{HISTORY_EXTENSION}", id.0))
}

/// How many logical lines of each pane's output to persist: `SPRAG_RESTORE_HISTORY` if it holds a
/// number, else [`DEFAULT_HISTORY_LINES`]. `0` disables history persistence entirely — the daemon
/// then neither saves nor restores it.
///
/// A malformed value falls back to the default and WARNS rather than refusing to boot: a typo in
/// an environment variable must not cost the operator their daemon.
#[must_use]
pub fn history_limit() -> usize {
    let raw = std::env::var("SPRAG_RESTORE_HISTORY").ok();
    if let Some(limit) = parse_limit(raw.as_deref()) {
        return limit;
    }
    if raw.as_deref().is_some_and(|value| !value.trim().is_empty()) {
        tracing::warn!(
            target: "sprag_host::durability",
            "SPRAG_RESTORE_HISTORY={:?} is not a line count; using {DEFAULT_HISTORY_LINES}",
            raw.unwrap_or_default(),
        );
    }
    DEFAULT_HISTORY_LINES
}

/// Parse a raw `SPRAG_RESTORE_HISTORY` value: `Some(n)` for a well-formed count, `None` when the
/// caller should use the default (unset, empty, or not a number).
///
/// Split out from the env read so the parse is tested WITHOUT mutating the process environment,
/// which would race any concurrent `getenv`.
fn parse_limit(raw: Option<&str>) -> Option<usize> {
    let raw = raw?.trim();
    if raw.is_empty() {
        return None;
    }
    raw.parse().ok()
}

/// Read pane `id`'s recorded history, or empty when there is none to replay — the FAIL-SAFE load.
///
/// Empty for a missing file (a pane that never had history saved, or persistence disabled) AND for
/// an unreadable one. A pane whose history cannot be read comes back blank, which is the
/// pre-history behaviour; it must never be a reason a restore fails.
#[must_use]
pub fn load_pane_history(dir: &Path, id: PaneId) -> Vec<u8> {
    std::fs::read(pane_history_path(dir, id)).unwrap_or_default()
}

/// One history save STEP: capture every live pane's output at `limit` lines and write the ones
/// that changed, returning how many files were written. `limit == 0` does nothing at all.
///
/// The daemon's save loop is this on a timer, beside
/// [`save_if_changed`](crate::save_if_changed)'s shape half.
///
/// # Errors
///
/// The FIRST [`io::Error`] encountered, after every pane has been attempted — one pane's failure
/// does not abandon the rest.
pub fn save_histories_if_changed(
    dir: &Path,
    registry: &Arc<Mutex<SessionRegistry>>,
    limit: usize,
    last: &mut HashMap<PaneId, Vec<u8>>,
) -> io::Result<usize> {
    if limit == 0 {
        // Persistence disabled: no capture, no write, and nothing destroyed. `pane_histories`
        // refuses a zero limit too — this guard is what lets the step skip the registry walk (and
        // its locks) outright rather than relying on the other crate to return nothing.
        return Ok(0);
    }
    write_histories(dir, pane_histories(registry, limit), last)
}

/// Write `captured` to `dir`, skipping panes whose history is byte-identical to `last` and reaping
/// files for panes that are gone. Returns how many files were written.
///
/// Split from [`save_histories_if_changed`] so the dedup and reap policy is testable against
/// synthetic captures — the registry walk needs live PTYs, these rules do not.
///
/// The dedup keeps the captured bytes in `last` rather than a hash: a hash collision would SKIP a
/// write and silently serve stale history on the next restore, and a few hundred kilobytes of
/// daemon memory is a cheap price for an exact answer. On a write error the pane's `last` entry is
/// left unchanged, so the next tick retries it.
fn write_histories(
    dir: &Path,
    captured: Vec<(PaneId, Vec<u8>)>,
    last: &mut HashMap<PaneId, Vec<u8>>,
) -> io::Result<usize> {
    // A capture with no panes means the registry is EMPTY, which is the ambiguous moment the
    // durability ring deliberately refuses to act on: the last pane exiting may be a deliberate
    // close or a restored program that just died (an `ssh` whose network was not up yet at boot),
    // and the daemon is about to exit either way. The snapshot is preserved across it for exactly
    // that reason, so the history it belongs to must be too. With at least one live pane the
    // registry is an unambiguous authority and a missing pane really is gone.
    if !captured.is_empty() {
        let live: HashSet<PaneId> = captured.iter().map(|(id, _)| *id).collect();
        reap_orphans(dir, &live);
        last.retain(|id, _| live.contains(id));
    }
    let mut wrote = 0usize;
    let mut failure = None;
    for (id, bytes) in captured {
        if last.get(&id).is_some_and(|seen| *seen == bytes) {
            continue; // unchanged since the last save — no redundant write
        }
        match write_atomic_private(&pane_history_path(dir, id), &bytes) {
            Ok(()) => {
                last.insert(id, bytes);
                wrote += 1;
            }
            // Keep sweeping: one unwritable pane must not cost every other pane its history.
            Err(error) => failure = failure.or(Some(error)),
        }
    }
    match failure {
        Some(error) => Err(error),
        None => Ok(wrote),
    }
}

/// Delete history files in `dir` whose pane is no longer `live`. Best-effort: a file that cannot
/// be removed is left, and a non-history file is never touched.
fn reap_orphans(dir: &Path, live: &HashSet<PaneId>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return; // no directory yet — nothing to reap
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some(HISTORY_EXTENSION) {
            continue;
        }
        let id = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .and_then(|stem| stem.parse::<u64>().ok())
            .map(PaneId);
        // A `.hist` whose stem is not a pane id is not ours to delete.
        if id.is_some_and(|id| !live.contains(&id)) {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Destroy every saved pane history for a daemon — the history half of `kill-server --purge`.
///
/// Best-effort and idempotent: a missing directory is already purged.
pub fn purge_histories(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory unique to the calling test, so parallel tests never share one.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sprag-hist-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn capture(entries: &[(u64, &str)]) -> Vec<(PaneId, Vec<u8>)> {
        entries
            .iter()
            .map(|(id, text)| (PaneId(*id), text.as_bytes().to_vec()))
            .collect()
    }

    /// History sits beside the snapshot under the same socket identity, so the two artifacts of one
    /// daemon are found together and a second daemon on its own socket keeps its own.
    #[test]
    fn history_dir_is_a_sibling_of_the_snapshot_keyed_on_the_socket() {
        let prior = std::env::var_os("XDG_STATE_HOME");
        // SAFETY: single-threaded test; no other thread reads the environment concurrently.
        unsafe { std::env::set_var("XDG_STATE_HOME", "/state") };
        let socket = Path::new("/run/user/1000/sprag-host.sock");
        assert_eq!(
            history_dir(socket),
            Path::new("/state/sprag/sprag-host.history"),
        );
        assert_eq!(
            crate::snapshot_path(socket).parent(),
            history_dir(socket).parent(),
            "the snapshot and the history live in one directory",
        );
        assert_eq!(
            history_dir(Path::new("/tmp/sp99.sock")),
            Path::new("/state/sprag/sp99.history"),
            "a second daemon keeps its own history",
        );
        unsafe {
            match prior {
                Some(value) => std::env::set_var("XDG_STATE_HOME", value),
                None => std::env::remove_var("XDG_STATE_HOME"),
            }
        }
    }

    /// The write-if-changed dedup: the first step writes every pane, an unchanged capture writes
    /// nothing, and a pane whose output grew writes again — ALONE, not the whole set.
    #[test]
    fn a_save_writes_each_pane_once_then_only_the_ones_that_changed() {
        let dir = scratch("dedup");
        let mut last = HashMap::new();

        let first =
            write_histories(&dir, capture(&[(1, "a"), (2, "b")]), &mut last).expect("write");
        assert_eq!(first, 2, "the first step writes both panes");
        let again =
            write_histories(&dir, capture(&[(1, "a"), (2, "b")]), &mut last).expect("no-op");
        assert_eq!(again, 0, "an unchanged capture rewrites nothing");
        let changed =
            write_histories(&dir, capture(&[(1, "a"), (2, "b!")]), &mut last).expect("write");
        assert_eq!(
            changed, 1,
            "only the pane whose output changed is rewritten"
        );

        assert_eq!(load_pane_history(&dir, PaneId(2)), b"b!".to_vec());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A pane that is gone takes its history file with it, so a long-lived daemon's history
    /// directory tracks its live panes instead of growing without bound.
    #[test]
    fn a_departed_panes_history_is_reaped() {
        let dir = scratch("reap");
        let mut last = HashMap::new();
        write_histories(&dir, capture(&[(1, "a"), (2, "b")]), &mut last).expect("write");
        write_histories(&dir, capture(&[(1, "a")]), &mut last).expect("write");

        assert!(
            load_pane_history(&dir, PaneId(2)).is_empty(),
            "pane 2 reaped"
        );
        assert_eq!(load_pane_history(&dir, PaneId(1)), b"a".to_vec(), "1 kept");
        assert!(
            !last.contains_key(&PaneId(2)),
            "and its dedup entry with it"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// THE data-loss guard: an EMPTY capture reaps nothing.
    ///
    /// No live panes is the ambiguous moment — a deliberate close, or a restored program that just
    /// exited — and it is the moment the daemon is about to end. The ring deliberately preserves
    /// the snapshot across it; deleting the history it belongs to would leave a restorable
    /// workspace whose panes all come back blank.
    #[test]
    fn an_empty_capture_never_reaps() {
        let dir = scratch("empty");
        let mut last = HashMap::new();
        write_histories(&dir, capture(&[(1, "a")]), &mut last).expect("write");
        let wrote = write_histories(&dir, Vec::new(), &mut last).expect("no-op");

        assert_eq!(wrote, 0);
        assert_eq!(
            load_pane_history(&dir, PaneId(1)),
            b"a".to_vec(),
            "the last pane exiting must not destroy the history the preserved snapshot needs",
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A file that is not a pane history is not ours to delete, even inside our own directory.
    #[test]
    fn reaping_leaves_foreign_files_alone() {
        let dir = scratch("foreign");
        let mut last = HashMap::new();
        write_histories(&dir, capture(&[(1, "a")]), &mut last).expect("write");
        let foreign = dir.join("notes.txt");
        std::fs::write(&foreign, b"not ours").expect("write foreign");
        let named = dir.join("scratch.hist");
        std::fs::write(&named, b"not a pane id").expect("write named");
        // The case the extension check alone catches: a stem that DOES read as a pane id, on a
        // file that is not one of ours.
        let lookalike = dir.join("7.txt");
        std::fs::write(&lookalike, b"pane-id stem, foreign kind").expect("write lookalike");

        write_histories(&dir, capture(&[(2, "b")]), &mut last).expect("write");

        assert!(foreign.exists(), "a non-history file survived");
        assert!(named.exists(), "a .hist without a pane-id stem survived");
        assert!(
            lookalike.exists(),
            "a pane-id-stemmed foreign file survived"
        );
        assert!(
            load_pane_history(&dir, PaneId(1)).is_empty(),
            "pane 1 reaped"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A missing or unreadable history is empty, never an error: a pane whose history cannot be
    /// read comes back blank, which is exactly the pre-history behaviour.
    #[test]
    fn a_missing_history_loads_as_empty() {
        let dir = scratch("missing");
        assert!(load_pane_history(&dir, PaneId(7)).is_empty(), "no dir");
        std::fs::create_dir_all(&dir).expect("mkdir");
        assert!(load_pane_history(&dir, PaneId(7)).is_empty(), "no file");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `--purge` destroys the saved histories, and doing it twice is not an error.
    #[test]
    fn purge_removes_every_history_and_is_idempotent() {
        let dir = scratch("purge");
        let mut last = HashMap::new();
        write_histories(&dir, capture(&[(1, "a")]), &mut last).expect("write");
        purge_histories(&dir);
        assert!(!dir.exists(), "the history directory is gone");
        purge_histories(&dir); // no panic on an already-purged directory
    }

    /// The limit parser: a count is taken as written, `0` is the explicit disable, and anything
    /// unusable defers to the default. Tested WITHOUT touching the process env, so there is no
    /// `set_var`/`getenv` race with a concurrent sibling test.
    #[test]
    fn parse_limit_takes_a_count_and_defers_otherwise() {
        assert_eq!(parse_limit(Some("500")), Some(500));
        assert_eq!(parse_limit(Some(" 500 ")), Some(500), "trimmed");
        assert_eq!(parse_limit(Some("0")), Some(0), "0 disables, not defaults");
        assert_eq!(parse_limit(None), None, "unset defers to the default");
        assert_eq!(parse_limit(Some("")), None, "empty defers to the default");
        assert_eq!(parse_limit(Some("lots")), None, "malformed defers");
        assert_eq!(parse_limit(Some("-1")), None, "a negative count defers");
    }

    /// Disabling persistence is not destructive: nothing is written and nothing already saved is
    /// removed. `kill-server --purge` stays the one verb that destroys saved state.
    #[test]
    fn a_zero_limit_writes_nothing_and_destroys_nothing() {
        let dir = scratch("disabled");
        let mut last = HashMap::new();
        write_histories(&dir, capture(&[(1, "a")]), &mut last).expect("write");

        let registry = Arc::new(Mutex::new(SessionRegistry::new((80, 24))));
        let wrote = save_histories_if_changed(&dir, &registry, 0, &mut last).expect("no-op");

        assert_eq!(wrote, 0, "persistence is off");
        assert_eq!(
            load_pane_history(&dir, PaneId(1)),
            b"a".to_vec(),
            "turning persistence off must not delete what is already saved",
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
