//! Stamps THIS IMAGE's own identity into the crate, so a running daemon can say which build it is.
//!
//! # ⚠⚠⚠⚠⚠ Why a build script exists here at all (register item 438)
//!
//! `WIRE_PROTOCOL` is the only identity this wire has ever carried, and it is exactly the number
//! that cannot answer *"which code produced this walk"*: it moves when a SHAPE moves, so a fix that
//! changes behaviour without changing a key, an argument, an answer word or a value earns no bump —
//! and daemon and client agree across it. Measured 2026-08-18: a run's whole walk was produced by a
//! daemon that predated the fix under test, and it read identically to one that carried it. The
//! only probe that worked was `grep` over `/proc/<pid>/exe`.
//!
//! So the value has to come off the RUNNING IMAGE rather than off the tree, because the tree is what
//! is already ahead. A constant compiled INTO the image is that: whatever built this binary is what
//! this binary says, and no later `cargo build` can change its mind.
//!
//! # ⚠⚠⚠ Why the COMMIT and nothing else — the dirty flag is refused on purpose
//!
//! The obvious second field is *"was the tree dirty when this was built"*, and it is left out
//! rather than shipped, because in the one ritual it exists for it would LIE. This repository's
//! promotion is `git stash push -- <files>` → build → `git stash pop`: the build is taken from a
//! CLEAN tree at an unmoved HEAD, so a flag computed when the stamp regenerates would report the
//! state of neither. A field that is wrong exactly where it is needed is worse than an absent one —
//! this register's oldest disease is a wrong reading that looks like a right one.
//!
//! ⚠ **The residue, stated rather than hidden**: two builds from the same commit with different
//! uncommitted edits are indistinguishable here. What would close it is hashing the sources this
//! crate is compiled from, which is a bigger instrument than the failure has yet earned.
//!
//! # Why `rerun-if-changed` watches HEAD and its ref
//!
//! Without it cargo decides on this package's files, none of which move when a commit lands, so the
//! stamp would age silently — the very failure it is built to name. Watching `HEAD` catches a
//! checkout and watching the ref `HEAD` names catches a commit on the branch already checked out.

use std::path::PathBuf;
use std::process::Command;

/// The env var the crate reads with `env!`. Named for the product rather than the crate: it is the
/// IMAGE's identity, and every binary that links this crate is stamped with the same one.
const STAMP: &str = "SPRAG_BUILD";

/// What a build with no git to ask answers. A word rather than an empty string, so a reader who
/// sees it in a daemon's reply learns *"this image cannot say"* instead of finding a blank where a
/// value should be — the same distinction the wire's own absent-key rule turns on.
const UNKNOWN: &str = "unknown";

fn main() {
    let commit = git(&["rev-parse", "--short=12", "HEAD"]).unwrap_or_else(|| UNKNOWN.to_owned());
    println!("cargo:rustc-env={STAMP}={commit}");
    for path in watched() {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

/// Run `git` with `args` from this package's directory, answering its trimmed stdout — `None` for
/// any failure at all (no git, no repository, a packaged source tree), which is what [`UNKNOWN`]
/// is for.
///
/// The cwd is the manifest dir rather than the process cwd so the answer is about THIS checkout,
/// including the worktrees this repository's own workflow uses.
fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?.trim().to_owned();
    (!text.is_empty()).then_some(text)
}

/// The files whose change must regenerate the stamp: the git dir's `HEAD`, and the ref `HEAD`
/// names when it is a symbolic one.
///
/// Resolved THROUGH git rather than assembled from `../../.git`, because that path is a FILE and
/// not a directory inside a worktree, and this repository works in worktrees. A resolution that
/// fails yields no watches at all, which pairs with the [`UNKNOWN`] stamp: nothing to say, nothing
/// to watch for.
fn watched() -> Vec<PathBuf> {
    let Some(dir) = git(&["rev-parse", "--absolute-git-dir"]).map(PathBuf::from) else {
        return Vec::new();
    };
    let mut paths = vec![dir.join("HEAD")];
    // ⚠ A DETACHED HEAD names no ref, and that is not a failure: `HEAD` itself then holds the
    // commit and the one watch above is complete.
    if let Some(reference) = git(&["symbolic-ref", "--quiet", "HEAD"]) {
        paths.push(dir.join(reference));
    }
    paths
}
