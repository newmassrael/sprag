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

// ══ REAPING WHAT AN EARLIER RUN LEFT ═══════════════════════════════════════════════════════════
//
// ⛔⛔⛔⛔⛔ WHY THIS IS HERE — register item 927.
//
// A harness that names its scratch `<prefix>-<pid>-<thread>` can never clean up after a
// PREDECESSOR. Its own `remove_dir_all` at startup deletes only the identical name, and a
// different pid is a different name; its `Drop` at the end belongs to a process that, if it was
// killed, is not there to run it. So one interrupted run deposits its scratch forever.
//
// Measured 2026-09-06 13:48:40 UTC on this workstation: `sprag-promoted-*` stood at **13
// directories holding 2,499.6 MB** — 192 MB each, because each is a fake `bin/` holding copies of
// `sprag-term` (114 MB) and `sprag` (64 MB). Thirteen interrupted sweeps, never collected.
//
// ⚠⚠⚠ THE ONE THING THIS MUST NOT DO IS DELETE A LIVE RUN'S SCRATCH — register item 196: this
// working tree has two writers, and the debt loop runs suites unattended. So the rule is the
// narrowest one that can be justified: **reap only what is PROVABLY abandoned.** An owner that
// might still be alive, a name with no owner in it, a directory belonging to somebody else's
// prefix — all kept. "Every unknown answers no", which is the stance this workspace already takes
// wherever a gate cannot compare.

/// What is known about the process a scratch directory was named after.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    /// The process is still running. Its scratch is in use.
    Alive,
    /// The process is gone, so nothing will ever run its cleanup.
    Gone,
    /// This platform cannot say. **Treated exactly like [`Owner::Alive`]** — see [`may_reap`].
    Unknown,
}

/// Why a sibling was kept, or that it may go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reap {
    /// Provably abandoned: the name carries an owner and that owner is gone.
    Yes,
    /// This run's own directory.
    ItIsMine,
    /// A different harness's scratch.
    NotThisPrefix,
    /// The name carries no owner, so nothing can be proved about it.
    NoOwnerInTheName,
    /// The owner may still be using it.
    OwnerMayBeAlive,
}

/// The pid a scratch name carries, for names shaped `<prefix>-<pid>-<anything>` or `<prefix>-<pid>`.
///
/// ⚠ Returns [`None`] rather than a guess whenever the shape does not hold. A wrong pid here would
/// be read as *some other process*, and the answer to that is *do not touch it* — so a `None` and a
/// wrong answer fail in the same safe direction, but only `None` says so.
#[must_use]
pub fn owner_in(name: &str, prefix: &str) -> Option<u32> {
    let rest = name.strip_prefix(prefix)?.strip_prefix('-')?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    // The pid must be a whole segment: `sprag-promoted-12x` is not pid 12.
    match rest[digits.len()..].chars().next() {
        None | Some('-') => digits.parse().ok(),
        Some(_) => None,
    }
}

/// Whether one sibling of `mine` may be removed. **Pure**, so every case can be handed to it.
///
/// The shape [`scratch_root`] already uses one function up: the impure reads — listing a directory,
/// asking whether a pid is alive — happen at the caller's seam, and the decision is a function of
/// what they found. [`reap_predecessors`] is that caller.
///
/// ⚠⚠ [`Owner::Unknown`] keeps the directory. A platform that cannot answer must not be allowed to
/// turn "I do not know" into a deletion, and the cost of being wrong is asymmetric: keeping a dead
/// run's scratch wastes disk, deleting a live one's breaks somebody's round.
#[must_use]
pub fn may_reap(name: &str, prefix: &str, mine: &str, owner: Owner) -> Reap {
    if name == mine {
        return Reap::ItIsMine;
    }
    if !name.starts_with(prefix) {
        return Reap::NotThisPrefix;
    }
    if owner_in(name, prefix).is_none() {
        return Reap::NoOwnerInTheName;
    }
    match owner {
        Owner::Gone => Reap::Yes,
        Owner::Alive | Owner::Unknown => Reap::OwnerMayBeAlive,
    }
}

/// Whether a process is still running, as far as this platform can say.
///
/// ⛔⛔ **Linux answers from `/proc`; every other platform answers [`Owner::Unknown`]**, which
/// [`may_reap`] treats as alive. That is a stated limit, not a gap to fill later with a guess: this
/// crate carries no dependencies by charter, `std` has no `kill(pid, 0)`, and spawning a process
/// per candidate from a crate every other crate depends on is a worse trade than reaping nothing.
/// The 19.9 GB item 927 measured is on Linux.
///
/// ⚠ A recycled pid reads as `Alive` and the directory is kept. That is the safe direction, and it
/// is why this asks *may it be alive* rather than *is it the same process*.
#[must_use]
pub fn owner_of(pid: u32) -> Owner {
    if !cfg!(target_os = "linux") {
        return Owner::Unknown;
    }
    if std::path::Path::new(&format!("/proc/{pid}")).exists() {
        Owner::Alive
    } else {
        Owner::Gone
    }
}

/// What one call to [`reap_predecessors`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reaped {
    /// Directories removed.
    pub removed: Vec<String>,
    /// Directories left, with why — so a caller can say what it did NOT do.
    pub kept: Vec<(String, Reap)>,
    /// Removals that failed, with the reason. Never a panic: this is housekeeping, and a harness
    /// must not fail because somebody else's leftovers are read-only.
    pub refused: Vec<(String, String)>,
}

/// Remove the scratch directories of DEAD runs sharing `prefix`, under `root`.
///
/// `mine` is the file name this run is about to use, which is never removed even if its owner
/// somehow reads as gone.
///
/// ⚠⚠ Errors are collected rather than raised. A harness calling this at startup is tidying, and a
/// permission error on a stranger's leftovers must not take the test with it.
pub fn reap_predecessors(root: &std::path::Path, prefix: &str, mine: &str) -> Reaped {
    let mut reaped = Reaped {
        removed: Vec::new(),
        kept: Vec::new(),
        refused: Vec::new(),
    };
    let Ok(listing) = std::fs::read_dir(root) else {
        return reaped;
    };
    for entry in listing.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let owner = match owner_in(&name, prefix) {
            Some(pid) => owner_of(pid),
            None => Owner::Unknown,
        };
        match may_reap(&name, prefix, mine, owner) {
            Reap::Yes => match std::fs::remove_dir_all(entry.path()) {
                Ok(()) => reaped.removed.push(name),
                Err(why) => reaped.refused.push((name, why.to_string())),
            },
            // ⚠ Only this prefix's siblings are worth reporting; the rest of the root is not this
            // caller's business and listing it would drown the answer.
            kept if name.starts_with(prefix) => reaped.kept.push((name, kept)),
            _ => {}
        }
    }
    reaped
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

    // ══ REAPING — register item 927 ════════════════════════════════════════════════════════════

    /// The pid is a whole segment or it is not a pid.
    #[test]
    fn the_owner_is_read_from_the_name_or_not_at_all() {
        assert_eq!(
            owner_in("sprag-promoted-1016056-ThreadId(331)", "sprag-promoted"),
            Some(1_016_056)
        );
        assert_eq!(owner_in("sprag-promoted-77", "sprag-promoted"), Some(77));
        assert_eq!(
            owner_in(
                "sprag-promoted-under-guests-77-ThreadId(1)",
                "sprag-promoted"
            ),
            None,
            "`under` is not a pid, and guessing one here would name a process at random",
        );
        assert_eq!(owner_in("sprag-gate-77-clean", "sprag-promoted"), None);
        assert_eq!(owner_in("sprag-promoted-", "sprag-promoted"), None);
    }

    /// ⛔⛔⛔⛔⛔ **THE SAFETY PROPERTY — register item 196.** Two writers share this machine, and
    /// the debt loop runs suites unattended. Every answer except *provably gone* keeps the
    /// directory, and this drives all of them rather than the happy path.
    #[test]
    fn nothing_but_a_provably_dead_owner_is_ever_reaped() {
        let mine = "sprag-promoted-500-ThreadId(1)";
        assert_eq!(
            may_reap(
                "sprag-promoted-499-ThreadId(9)",
                "sprag-promoted",
                mine,
                Owner::Gone
            ),
            Reap::Yes,
            "a dead predecessor is the one case this exists for",
        );
        for (name, owner, expected, why) in [
            (
                mine,
                Owner::Gone,
                Reap::ItIsMine,
                "this run's own scratch is never a predecessor",
            ),
            (
                "sprag-promoted-499-ThreadId(9)",
                Owner::Alive,
                Reap::OwnerMayBeAlive,
                "a running suite's scratch is in use",
            ),
            (
                "sprag-promoted-499-ThreadId(9)",
                Owner::Unknown,
                Reap::OwnerMayBeAlive,
                "a platform that cannot say must not have its silence read as permission",
            ),
            (
                "sprag-gate-499-clean",
                Owner::Gone,
                Reap::NotThisPrefix,
                "another harness's leftovers are not this caller's to delete",
            ),
            (
                "sprag-promoted-under-guests-1-ThreadId(2)",
                Owner::Gone,
                Reap::NoOwnerInTheName,
                "no owner in the name means nothing can be proved about it",
            ),
        ] {
            assert_eq!(
                may_reap(name, "sprag-promoted", mine, owner),
                expected,
                "{why}"
            );
        }
    }

    /// ⛔ **THE MUTATION ITEM 927 ASKS FOR, ON THE REAPER**: plant a predecessor and it must go —
    /// and plant a live one beside it, which must stay. A reaper that removed both would pass a
    /// test that only planted the first.
    #[test]
    fn a_dead_predecessor_goes_and_a_live_one_stays() {
        let root = scratch_root().join(format!("sprag-scratch-reap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("a scratch root of this test's own");

        // This process is alive by construction, so its pid is the live owner.
        let live = format!("sprag-reaptest-{}-ThreadId(1)", std::process::id());
        // Pid 0 is never a running process on Linux, so `/proc/0` does not exist.
        let dead = "sprag-reaptest-0-ThreadId(2)".to_string();
        let mine = format!("sprag-reaptest-{}-ThreadId(9)", std::process::id());
        let stranger = "sprag-elsewhere-0-ThreadId(3)".to_string();
        for name in [&live, &dead, &mine, &stranger] {
            std::fs::create_dir_all(root.join(name)).expect("a planted directory");
            std::fs::write(root.join(name).join("held"), b"x").expect("something inside it");
        }

        let reaped = reap_predecessors(&root, "sprag-reaptest", &mine);

        // ⚠ On a platform that cannot read liveness nothing is reapable, and this asserts the
        // PLATFORM's contract rather than skipping: the property being driven is different there,
        // and saying so is what keeps a green run on macOS from reading as a green reaper.
        if cfg!(target_os = "linux") {
            assert_eq!(reaped.removed, vec![dead.clone()], "{reaped:?}");
            assert!(
                !root.join(&dead).exists(),
                "the dead owner's scratch is gone"
            );
        } else {
            assert!(
                reaped.removed.is_empty(),
                "this platform answers Unknown, so it reaps nothing and says so: {reaped:?}",
            );
        }
        assert!(
            root.join(&live).exists(),
            "a live owner's scratch is untouched: {reaped:?}"
        );
        assert!(
            root.join(&mine).exists(),
            "this run's own scratch is untouched: {reaped:?}"
        );
        assert!(
            root.join(&stranger).exists(),
            "another prefix is not this caller's business: {reaped:?}",
        );
        assert!(
            !reaped.kept.iter().any(|(name, _)| name == &stranger),
            "and it is not even reported — the answer is about this prefix: {reaped:?}",
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// ⚠ A root that cannot be listed answers "nothing done" rather than panicking. A harness
    /// tidying up at startup must not fail because the tidying could not happen.
    #[test]
    fn an_unreadable_root_is_not_a_failure() {
        let absent = scratch_root().join("sprag-scratch-reap-no-such-directory-927");
        let reaped = reap_predecessors(&absent, "sprag-reaptest", "sprag-reaptest-1");
        assert!(reaped.removed.is_empty() && reaped.kept.is_empty() && reaped.refused.is_empty());
    }
}
