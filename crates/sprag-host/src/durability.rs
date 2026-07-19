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

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use sprag_terminal::{
    CommandBuilder, SessionRegistry, Snapshot, command_from_parts, default_shell_command, snapshot,
};

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

/// The default set of program BASENAMES an exact-command restore re-runs — safe-to-rerun
/// interactive programs (re-running has no side effect beyond opening the same view). Editors,
/// pagers/monitors, remote sessions, and AI agents (sprag's focus — the cmux "agents come back").
///
/// Deliberately NO build tools, package managers, or one-shots: re-running `cargo build` or
/// `rm -rf` on every reboot is the anti-pattern tmux-resurrect walked back. And NO shells — a
/// shell is handled separately (a plain shell in the cwd), never re-run with its recorded argv,
/// so a `bash -c '<destructive>'` cannot be re-executed.
const DEFAULT_RESTORE_PROGRAMS: &[&str] = &[
    "vi", "vim", "nvim", "emacs", "nano", "helix", "hx", "kak", // editors
    "less", "more", "man", "tail", "top", "htop", "btop", // pagers / monitors
    "ssh", "mosh",   // remote sessions
    "claude", // AI agents
];

/// Shell basenames a restore NEVER re-runs with their recorded argv — it opens a plain shell in
/// the cwd instead. This is a structural safety backstop: even if a user adds a shell to the
/// allowlist, `<shell> -c '<anything>'` is never re-executed on a reboot.
const SHELLS: &[&str] = &[
    "sh", "bash", "zsh", "fish", "dash", "ash", "ksh", "tcsh", "csh",
];

/// The exact-command restore allowlist: `SPRAG_RESTORE_PROGRAMS` (comma-separated basenames) if
/// set, else the built-in `DEFAULT_RESTORE_PROGRAMS`. An EMPTY value disables exact-command entirely
/// (every pane comes back as a shell in its cwd — the slice-1 behaviour), which a cautious
/// operator can set to opt out.
#[must_use]
pub fn restore_allowlist() -> HashSet<String> {
    match std::env::var("SPRAG_RESTORE_PROGRAMS") {
        Ok(list) => list
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect(),
        Err(_) => DEFAULT_RESTORE_PROGRAMS
            .iter()
            .map(|s| (*s).to_owned())
            .collect(),
    }
}

/// The last path component of `program` (`/usr/bin/vim` → `vim`), or `None` if it has none.
fn basename(program: &str) -> Option<&str> {
    Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
}

/// Build the command to restore a pane from its recorded `argv`, spawning in `cwd`.
///
/// Re-runs the EXACT argv only for a NON-SHELL program whose basename is in `allowlist`
/// (`vim foo`, `ssh host`, `claude`); a shell, an empty argv, or a non-allowlisted program
/// (`cargo build`, `rm …`) restores a PLAIN shell in the cwd — the slice-1 fallback, which never
/// re-executes a recorded command with side effects. Env is NOT restored from disk: the built
/// command inherits the DAEMON's environment (via `command_from_parts` / `default_shell_command`),
/// so a restored agent gets its API keys from the daemon, not from a plaintext state file.
///
/// Returns the [`CommandBuilder`] (with `cwd` set) and the pane's display label.
#[must_use]
pub fn restore_command(
    argv: &[String],
    cwd: Option<&Path>,
    allowlist: &HashSet<String>,
) -> (CommandBuilder, String) {
    let (mut command, label) = exact_or_shell(argv, allowlist);
    if let Some(cwd) = cwd {
        command.cwd(cwd);
    }
    (command, label)
}

/// The exact-vs-shell decision (cwd applied by [`restore_command`]).
fn exact_or_shell(argv: &[String], allowlist: &HashSet<String>) -> (CommandBuilder, String) {
    if let Some(program) = argv.first()
        && let Some(base) = basename(program)
        && !SHELLS.contains(&base)
        && allowlist.contains(base)
    {
        return command_from_parts(program, &argv[1..]);
    }
    default_shell_command()
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

    /// The argv of a built command, as owned strings — for asserting what a restore re-runs.
    fn argv_of(command: &CommandBuilder) -> Vec<String> {
        command
            .get_argv()
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn cwd_of(command: &CommandBuilder) -> Option<String> {
        command.get_cwd().map(|c| c.to_string_lossy().into_owned())
    }

    /// An allowlisted NON-shell program re-runs EXACTLY (the whole argv, an absolute path resolved
    /// by basename), in the recorded cwd — the cmux "agents come back" payoff.
    #[test]
    fn restore_command_reruns_an_allowlisted_program_exactly() {
        let allow = restore_allowlist(); // default includes vim
        let argv = vec!["/usr/bin/vim".to_owned(), "notes.txt".to_owned()];
        let (command, label) = restore_command(&argv, Some(Path::new("/tmp")), &allow);
        assert_eq!(label, "/usr/bin/vim");
        assert_eq!(argv_of(&command), argv, "the exact argv is re-run");
        assert_eq!(
            cwd_of(&command).as_deref(),
            Some("/tmp"),
            "in the recorded cwd"
        );
    }

    /// THE footgun guard: even with a shell EXPLICITLY allowlisted, a recorded `<shell> -c <cmd>`
    /// is NOT re-run — a plain shell opens instead, so a reboot never re-executes it.
    #[test]
    fn restore_command_never_reruns_a_shell_dash_c_even_if_allowlisted() {
        let allow: HashSet<String> = ["bash".to_owned()].into_iter().collect();
        let argv = vec![
            "bash".to_owned(),
            "-c".to_owned(),
            "rm -rf /tmp/precious".to_owned(),
        ];
        let (command, _label) = restore_command(&argv, None, &allow);
        let got = argv_of(&command);
        assert!(
            !got.contains(&"-c".to_owned()),
            "the -c is not re-run: {got:?}"
        );
        assert!(
            !got.iter().any(|a| a.contains("rm")),
            "no destructive command survives the restore: {got:?}",
        );
    }

    /// A non-allowlisted program (a build, a one-shot) falls back to a plain shell — never re-run.
    #[test]
    fn restore_command_falls_back_to_a_shell_for_a_non_allowlisted_program() {
        let allow = restore_allowlist(); // default: cargo is NOT in it
        let argv = vec!["cargo".to_owned(), "build".to_owned()];
        let (command, _) = restore_command(&argv, None, &allow);
        assert!(
            !argv_of(&command).iter().any(|a| a == "build"),
            "cargo build is not re-run",
        );
    }

    /// An empty argv (a pre-argv slice-1 snapshot) restores a shell in the cwd.
    #[test]
    fn restore_command_restores_a_shell_for_empty_argv() {
        let allow = restore_allowlist();
        let (command, _) = restore_command(&[], Some(Path::new("/tmp")), &allow);
        assert!(!argv_of(&command).is_empty(), "a shell has an argv");
        assert_eq!(cwd_of(&command).as_deref(), Some("/tmp"));
    }

    /// `SPRAG_RESTORE_PROGRAMS` REPLACES the default allowlist; an empty value disables exact
    /// restore entirely (every pane comes back as a shell).
    #[test]
    fn the_allowlist_honors_the_env_override() {
        let prior = std::env::var_os("SPRAG_RESTORE_PROGRAMS");
        // SAFETY: single-threaded test.
        unsafe { std::env::set_var("SPRAG_RESTORE_PROGRAMS", "vim, mycli ,ssh") };
        let allow = restore_allowlist();
        assert!(allow.contains("vim") && allow.contains("mycli") && allow.contains("ssh"));
        assert!(
            !allow.contains("claude"),
            "the override REPLACES the default"
        );

        unsafe { std::env::set_var("SPRAG_RESTORE_PROGRAMS", "") };
        assert!(
            restore_allowlist().is_empty(),
            "an empty override disables exact restore (all panes -> shells)",
        );
        unsafe {
            match prior {
                Some(v) => std::env::set_var("SPRAG_RESTORE_PROGRAMS", v),
                None => std::env::remove_var("SPRAG_RESTORE_PROGRAMS"),
            }
        }
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
