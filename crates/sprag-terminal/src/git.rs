//! Deriving the current git branch of a directory — the display fact the session sidebar (and
//! `sprag ls`) shows beside each session's cwd.
//!
//! Pure filesystem reads, never a `git` subprocess: a switcher repaints this per session on every
//! scene change, so shelling out per session per repaint would be far too heavy. Reading
//! `.git/HEAD` (following a `gitdir:` pointer for a linked worktree / submodule) is a couple of
//! small file reads and gives the same answer for the common case. Derived HOST-side from a pane's
//! live cwd, so a display client carries only the resulting string, never the path logic.

use std::fs;
use std::path::{Path, PathBuf};

/// The branch checked out in the git repository containing `cwd`, or `None` if `cwd` is not inside
/// a work tree (or `HEAD` is unreadable / malformed).
///
/// Walks up from `cwd` to the first ancestor holding a `.git` — a directory for a normal clone, or
/// a `gitdir:` pointer FILE for a linked worktree / submodule — then reads its `HEAD`:
/// `ref: refs/heads/NAME` yields `NAME` (slashes kept, e.g. `feature/x`); a detached HEAD (a raw
/// commit id) yields its 7-character short form in parentheses, e.g. `(1a2b3c4)`.
pub(crate) fn branch(cwd: &Path) -> Option<String> {
    let mut dir = Some(cwd);
    while let Some(d) = dir {
        if let Some(gitdir) = resolve_git_dir(&d.join(".git")) {
            return branch_from_head(&gitdir);
        }
        dir = d.parent();
    }
    None
}

/// Resolve a `.git` entry to the directory that actually holds `HEAD`: `.git` IS that directory for
/// a normal clone; for a linked worktree / submodule it is a FILE whose `gitdir: PATH` line points
/// at the real git dir (`PATH` may be relative to the `.git` file's own directory). `fs::metadata`
/// follows a symlink, so a symlinked `.git` resolves as its target.
fn resolve_git_dir(dot_git: &Path) -> Option<PathBuf> {
    let meta = fs::metadata(dot_git).ok()?;
    if meta.is_dir() {
        return Some(dot_git.to_path_buf());
    }
    let contents = fs::read_to_string(dot_git).ok()?;
    let target = contents
        .lines()
        .find_map(|line| line.strip_prefix("gitdir:"))?
        .trim();
    let target = Path::new(target);
    if target.is_absolute() {
        Some(target.to_path_buf())
    } else {
        Some(dot_git.parent()?.join(target))
    }
}

/// Parse `HEAD` under `gitdir` into a branch name (or a short detached-HEAD id). `None` if `HEAD`
/// is missing, empty, or neither a symbolic ref nor a hex commit id.
fn branch_from_head(gitdir: &Path) -> Option<String> {
    let head = fs::read_to_string(gitdir.join("HEAD")).ok()?;
    let head = head.trim();
    if let Some(reference) = head.strip_prefix("ref:") {
        // `ref: refs/heads/feature/x` -> `feature/x` (a branch name may contain slashes).
        let reference = reference.trim();
        let name = reference.strip_prefix("refs/heads/").unwrap_or(reference);
        (!name.is_empty()).then(|| name.to_owned())
    } else if head.len() >= 7 && head.bytes().all(|b| b.is_ascii_hexdigit()) {
        // A detached HEAD is a raw commit id — show the 7-char short form, parenthesised so it
        // reads as "not on a branch" rather than as an oddly-named one.
        Some(format!("({})", &head[..7]))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A unique temp directory removed on drop — so a test leaves nothing behind (the
    /// workspace-hygiene rule) even if it panics. Uniqueness from the pid plus a process-wide
    /// counter (no randomness needed).
    struct TmpRepo(PathBuf);

    impl TmpRepo {
        fn new(tag: &str) -> Self {
            static N: AtomicU32 = AtomicU32::new(0);
            let n = N.fetch_add(1, Ordering::Relaxed);
            let dir =
                std::env::temp_dir().join(format!("sprag-git-{tag}-{}-{n}", std::process::id()));
            fs::create_dir_all(&dir).expect("create temp repo dir");
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TmpRepo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(path, contents).expect("write file");
    }

    #[test]
    fn a_ref_head_yields_the_branch_name() {
        let repo = TmpRepo::new("ref");
        write(&repo.path().join(".git/HEAD"), "ref: refs/heads/main\n");
        assert_eq!(branch(repo.path()).as_deref(), Some("main"));
    }

    #[test]
    fn a_branch_name_keeps_its_slashes() {
        let repo = TmpRepo::new("slash");
        write(
            &repo.path().join(".git/HEAD"),
            "ref: refs/heads/feature/nice\n",
        );
        assert_eq!(branch(repo.path()).as_deref(), Some("feature/nice"));
    }

    #[test]
    fn a_detached_head_yields_a_short_id() {
        let repo = TmpRepo::new("detached");
        write(
            &repo.path().join(".git/HEAD"),
            "1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b\n",
        );
        assert_eq!(branch(repo.path()).as_deref(), Some("(1a2b3c4)"));
    }

    #[test]
    fn a_subdirectory_finds_the_repo_above_it() {
        let repo = TmpRepo::new("walkup");
        write(&repo.path().join(".git/HEAD"), "ref: refs/heads/dev\n");
        let deep = repo.path().join("a/b/c");
        fs::create_dir_all(&deep).expect("nested dirs");
        assert_eq!(branch(&deep).as_deref(), Some("dev"));
    }

    #[test]
    fn a_gitdir_pointer_file_is_followed() {
        // A linked worktree: `.git` is a FILE naming the real git dir (relative here).
        let repo = TmpRepo::new("worktree");
        write(&repo.path().join("real-git/HEAD"), "ref: refs/heads/wt\n");
        write(&repo.path().join(".git"), "gitdir: real-git\n");
        assert_eq!(branch(repo.path()).as_deref(), Some("wt"));
    }

    #[test]
    fn a_malformed_head_has_no_branch() {
        let repo = TmpRepo::new("bad");
        write(&repo.path().join(".git/HEAD"), "not a ref and not a sha\n");
        assert_eq!(branch(repo.path()), None);
    }

    #[test]
    fn a_non_repo_directory_has_no_branch() {
        // A plain temp dir with no `.git` in it or (normally) any ancestor.
        let plain = TmpRepo::new("plain");
        assert_eq!(branch(plain.path()), None);
    }
}
