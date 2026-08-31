//! Where a process may put a scratch file — asked ONCE for the whole workspace, and refused when
//! the answer cannot be used.
//!
//! # ⛔⛔⛔⛔⛔ WHAT `std::env::temp_dir()` DOES THAT NOBODY WAS ASKING ABOUT — register item 794
//!
//! It answers a **relative** path when `TMPDIR` is set-and-empty. Measured 2026-08-31 with
//! `rustc`, both arms in one program:
//!
//! ```text
//! TMPDIR unset : temp_dir="/tmp"  joined="/tmp/sprag-probe"  absolute=true
//! TMPDIR=      : temp_dir=""      joined="sprag-probe"       absolute=false
//! ```
//!
//! A relative root is not a smaller version of a temporary one. It is a DIFFERENT DIRECTORY —
//! whatever the process happens to be standing in — and `cargo test` stands every test binary in
//! its own crate directory. So the scratch lands in the repository, and the failure is silent:
//! `create_dir_all` succeeds, `File::create` succeeds, `git worktree add` succeeds (measured:
//! `git -C <repo> worktree add --detach -q sprag-check-probe HEAD` exits **0** and leaves
//! `?? sprag-check-probe/` INSIDE the repository, because git resolves a relative path against
//! its own `-C`).
//!
//! # ⚠⚠⚠⚠ AND IT IS NOT HYPOTHETICAL — the suite was run under it and counted
//!
//! `TMPDIR= cargo test --workspace --all-features --locked --no-fail-fast`, against the same tree
//! that passes clean, 2026-08-31:
//!
//! | | normal `TMPDIR` | `TMPDIR=` |
//! |---|---|---|
//! | exit | 0 | **101** |
//! | wall clock | 207s | **813s** |
//! | tests passed / failed | 3841 / **0** | 3427 / **414** |
//! | untracked entries left under `crates/*/` | **0** | **131** |
//!
//! ⛔⛔⛔⛔⛔ **AND `git status` NAMES NONE OF THE 131.** The first draft of this table counted the
//! litter the way item 794's done-when asked for it — a `git status --porcelain` difference across
//! the run — and wrote **143**. Re-measured afterwards against the tree the run left behind, `git
//! status` reports **zero** of what is demonstrably still sitting there. Three shapes, each checked
//! on a surviving entry before the sweep:
//!
//!   * a bare unix socket (`crates/sprag-client/sprag-skew-up-<pid>-0.sock`) — git does not report
//!     a path that is neither a regular file nor a directory;
//!   * a directory whose only regular file is `.git` (`…-<pid>-0.tree/.git`, holding the text
//!     `gitdir: nowhere`) — git stops at what looks like a nested repository;
//!   * a directory holding only empty directories (`…-bin-<pid>-unset/{config,data,state}`) — git
//!     tracks files, so there is nothing inside for it to name.
//!
//! ⇒ **`git status` is the wrong instrument for this question in two independent ways**: tests tear
//! their own scratch down before anyone can look, AND it cannot see this shape of litter even while
//! the litter is there. The 131 comes from the predicate that does work — entries under `crates/*/`
//! that `git ls-files` does not know, which is `find crates -mindepth 2 -maxdepth 2` minus that
//! list. It answered 133 when it was run, two of which were this crate's own new files, and it
//! answers **0** against the swept tree. Anybody can re-run it, which is what a number in a table
//! has to be.
//!
//! ⚠ And the 414 failures are the same mechanism seen from the other side, not a separate
//! problem: `sweep-coverage: sprag-sweep-769539.log went unread: No such file or directory` and
//! `.githooks/pre-commit must be executable: No such file or directory` — one process wrote a
//! relative path from one directory and another read it from a different one.
//!
//! # WHY A CRATE RATHER THAN A FUNCTION PER CALLER
//!
//! The value is a property of the PROCESS, not of the call site: `temp_dir()` reads one
//! environment variable and every one of this workspace's call sites gets the same answer. Asking
//! at each site would be 164 copies of one question — code lines under `crates/` whose
//! `env::temp_dir()` is a call rather than quoted text: **163** in test, bench and example code,
//! **0** left in product code, and this one. Counted 2026-08-31 by asking the gate rather than a
//! second script, and named for what they are the count OF, because this file has already carried
//! one number that turned out to be counting something else. Asking here makes it a gate's
//! question:
//! `no_product_code_takes_a_scratch_root_unchecked` in `sprag-gate` walks the tree, splits it by
//! Cargo target, and holds that the product half is empty except for this crate.
//!
//! ⚠ It carries NO dependencies for the same reason it exists: every crate in the workspace
//! depends on it, so anything it pulled in would be pulled in everywhere.

use std::path::PathBuf;

/// The directory this machine offers for scratch files, or a panic naming why it cannot be used.
///
/// Use this everywhere `std::env::temp_dir()` would have been called. The answer is identical on
/// any correctly configured machine — that is the point: the difference only shows up in the one
/// configuration where the bare call would have quietly written into the caller's own directory.
///
/// # Panics
///
/// When the answer is not an absolute path — see `root_from` below for why that is the right
/// response rather than a fallback.
///
/// ⚠ `root_from` is deliberately named here WITHOUT an intra-doc link: it is private, and a public
/// doc linking a private item resolves only under `--document-private-items`. This repository's doc
/// gate passes exactly that flag, so the link worked there and would have broken for anyone
/// building the published docs — `rustdoc::private_intra_doc_links` said so, and it is only audible
/// because the gate runs with `-D warnings`.
#[must_use]
pub fn scratch_root() -> PathBuf {
    root_from(std::env::temp_dir())
}

/// [`scratch_root`]'s policy, with the environment's answer injected so it is testable.
///
/// The shape this workspace already uses for env-dependent policy (`sprag_rpc`'s
/// `resolve_socket_path` takes its `XDG_RUNTIME_DIR` the same way): the impure read happens at one
/// seam and the decision is a pure function of what it read, so a test can drive the case the
/// machine will not produce on demand.
///
/// # ⛔⛔⛔ WHY IT PANICS RATHER THAN FALLING BACK TO `/tmp`
///
/// A fallback would be this crate deciding it knows better than the environment, and it would hide
/// exactly the misconfiguration that produced item 794: the operator would get a working process
/// and no reason to fix `TMPDIR`. The harm being prevented is a SILENT wrong directory, and a
/// silent right one is the same shape of answer. So the process stops and says which variable is
/// wrong — CLAUDE.md's "fail fast and fail clearly", applied to the one input that cannot be
/// recovered from without guessing.
///
/// ⚠ The question is `is_absolute`, not `is_empty`. An empty path is merely the case that was
/// measured; any relative root has the same consequence, and one question covers both rather than
/// two that could drift apart.
///
/// # Panics
///
/// When `raw` is not absolute.
fn root_from(raw: PathBuf) -> PathBuf {
    assert!(
        raw.is_absolute(),
        "⛔ ITEM 794: the temporary directory this machine offers is {raw:?}, which is NOT an \
         absolute path — `TMPDIR` is set to an empty value. Every scratch file would be created \
         relative to whatever directory this process is standing in (for a test binary, its own \
         crate directory inside the repository), and nothing downstream refuses it: \
         `create_dir_all`, `File::create` and even `git worktree add` all succeed. Unset `TMPDIR` \
         or give it an absolute path.",
    );
    raw
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An absolute root is handed back unchanged — the case every correctly configured machine is.
    #[test]
    fn an_absolute_root_is_the_answer() {
        assert_eq!(
            root_from(PathBuf::from("/tmp")),
            PathBuf::from("/tmp"),
            "a usable root must survive the check unchanged, or every caller would be reading a \
             path this crate invented",
        );
    }

    /// ⛔⛔ **THE CASE THAT WAS MEASURED**, driven directly rather than through the environment.
    ///
    /// `std::env::set_var` is process-global and these tests run as threads of one binary, so
    /// setting `TMPDIR` here would decide the answer for every sibling. Injecting it is what makes
    /// the case drivable at all — and it is the reason [`root_from`] is split out of
    /// [`scratch_root`] rather than being one function that reads the environment itself.
    #[test]
    #[should_panic(expected = "ITEM 794")]
    fn an_empty_root_is_refused() {
        let _ = root_from(PathBuf::new());
    }

    /// ⚠ **AND AN ORDINARY RELATIVE ROOT TOO** — the guard asks `is_absolute`, so a non-empty
    /// relative path must be refused by the same question. A test that only drove the empty case
    /// would pass against an `is_empty` check, which is a narrower guard that drifts the moment
    /// anything else sets `TMPDIR` to a relative value.
    #[test]
    #[should_panic(expected = "ITEM 794")]
    fn a_relative_root_is_refused_by_the_same_question() {
        let _ = root_from(PathBuf::from("some/relative/dir"));
    }

    /// The public entry point answers on this machine, and what it answers is absolute.
    ///
    /// ⚠ This is NOT a self-referential assertion — it does not recompute the expected value with
    /// the same call. It states the PROPERTY the whole crate exists to guarantee, which is the
    /// thing `sprag_rpc`'s own fallback test failed to do: that one read
    /// `assert_eq!(path, std::env::temp_dir().join(name))`, so under `TMPDIR=` both sides became
    /// the same relative path and it passed while the socket moved into the working directory.
    #[test]
    fn the_entry_point_answers_an_absolute_path_on_this_machine() {
        assert!(
            scratch_root().is_absolute(),
            "the whole point of this crate is that its answer can be joined onto and written to \
             from any directory",
        );
    }
}
