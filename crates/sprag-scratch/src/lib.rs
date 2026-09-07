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
    /// Entries removed — a directory with everything under it, anything else as itself.
    pub removed: Vec<String>,
    /// Entries left, with why — so a caller can say what it did NOT do.
    pub kept: Vec<(String, Reap)>,
    /// Removals that failed, with the reason. Never a panic: this is housekeeping, and a harness
    /// must not fail because somebody else's leftovers are read-only.
    pub refused: Vec<(String, String)>,
}

/// Remove what DEAD runs sharing `prefix` left under `root`.
///
/// `mine` is the file name this run is about to use, which is never removed even if its owner
/// somehow reads as gone.
///
/// # ⚠⚠ A DIRECTORY IS NOT THE ONLY SHAPE A DEAD RUN LEAVES
///
/// The first draft of this collected directories alone, because register item 927's own subject —
/// `sprag-promoted-<pid>-<thread>`, a fake `bin/` holding 192 MB of copied binaries — was one. The
/// same scratch root, measured 2026-09-06T14:24:34Z, held **2,667 entries that are not
/// directories** and are the identical mechanism seen from the side: `sprag-cli-it-<pid>-<n>.lock`
/// 802, `…​.log` 767, `sprag-standin-<pid>` 736, `sprag-skew-up-<pid>-<n>.sock` 342. Each carries a
/// dead owner in the one place [`owner_in`] reads it, and each was kept by a question about its
/// TYPE that has nothing to do with whether anybody still wants it. The predicate is
/// [`may_reap`]'s and it never asked.
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
        // ⚠ A SYMLINK IS NEITHER SHAPE, and the question is asked this way round on purpose:
        // `remove_dir_all` refuses a link that points at a directory, while `remove_file` takes the
        // link itself and leaves whatever it pointed at alone. So *is this a real directory* gets
        // the recursive removal and everything else — file, socket, fifo, link — goes as one entry.
        let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
        let owner = match owner_in(&name, prefix) {
            Some(pid) => owner_of(pid),
            None => Owner::Unknown,
        };
        match may_reap(&name, prefix, mine, owner) {
            Reap::Yes => {
                let taken = if is_dir {
                    std::fs::remove_dir_all(entry.path())
                } else {
                    std::fs::remove_file(entry.path())
                };
                match taken {
                    Ok(()) => reaped.removed.push(name),
                    Err(why) => reaped.refused.push((name, why.to_string())),
                }
            }
            // ⚠ Only this prefix's siblings are worth reporting; the rest of the root is not this
            // caller's business and listing it would drown the answer.
            kept if name.starts_with(prefix) => reaped.kept.push((name, kept)),
            _ => {}
        }
    }
    reaped
}

// ══ THE ONE PLACE A RUN'S SCRATCH IS NAMED ═════════════════════════════════════════════════════
//
// ⛔⛔⛔⛔⛔ WHY NAMING AND REAPING ARE ONE ACT — register item 795, measured 2026-09-06T14:27:10Z
// in this machine's scratch root.
//
// Item 794's remedy — take the root from `scratch_root()` instead of from the operating system —
// answers WHERE THE ROOT CAME FROM. It says nothing about what becomes of the directory
// afterwards, and the COUNT is governed entirely by that second question:
//
// ```text
// sprag-869-<pid>   124 directories standing   and BOTH its call sites already ask scratch_root()
// ```
//
// ⇒ **A conversion to `scratch_root()` is not a fix for the litter.** Reaping is. And reaping
// needs two promises a hand-built name cannot make:
//
//   * the pid must sit where `owner_in` reads it — immediately after the prefix;
//   * the prefix handed to the reaper must be the prefix the name was built from.
//
// The second one is not hypothetical either. `owner_in("sprag-869-2575955", "sprag")` answers
// `Some(869)` — an ITEM NUMBER read as a process id, and on this workstation pid 869 belongs to a
// live system service. Ask with the prefix that built the name and the same string answers
// `Some(2575955)`, which is the run that made it. One function that does both cannot get that pair
// wrong; two call sites that each do one of them can, and only in the direction that deletes
// somebody's live scratch.

/// The name [`scratch_for`] gives this run: `<prefix>-<pid>`, plus `-<tail>` when `tail` is not
/// empty.
///
/// ⛔ **The pid sits immediately after the prefix, which is the one place [`owner_in`] reads it.**
/// That is the contract: a name built here is reapable by construction, whatever the tail says.
///
/// Split out from [`scratch_for`] for the reason `root_from` is split out of [`scratch_root`] —
/// the decision is pure and can be driven directly, while the seam that touches the filesystem
/// stays one function up.
#[must_use]
pub fn scratch_name(prefix: &str, tail: &str) -> String {
    let pid = std::process::id();
    if tail.is_empty() {
        format!("{prefix}-{pid}")
    } else {
        format!("{prefix}-{pid}-{tail}")
    }
}

/// Whether this process still owes `prefix` a sweep — true the first time it is asked, false after.
///
/// ⚠⚠ **Reaping is a property of the PROCESS, not of the call.** A predecessor is a run that was
/// already dead when this one started; a second sweep of the same prefix can only find what the
/// first one decided to KEEP, and every one of those was kept because its owner may be alive. So a
/// repeat scan costs a full listing of the scratch root — thirteen thousand entries on the machine
/// register item 927 measured — to re-derive an answer it already has.
///
/// ⚠ Poisoning is absorbed rather than propagated. A caller is asking where to put a file, and a
/// panic in some unrelated thread that happened to hold this lock is not a reason to take that
/// caller down; the worst an absorbed poison can do here is sweep one prefix twice.
fn first_time_for(prefix: &str) -> bool {
    static SWEPT: std::sync::Mutex<std::collections::BTreeSet<String>> =
        std::sync::Mutex::new(std::collections::BTreeSet::new());
    let mut swept = SWEPT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    swept.insert(prefix.to_string())
}

/// **Where this run may put its scratch, with whatever DEAD runs left under the same prefix
/// collected first** — the call every harness in this workspace makes.
///
/// The name is [`scratch_name`]'s, so [`owner_in`] can always read this run's own pid back out of
/// it, and the sweep is asked with the same `prefix` — see the block above for the pair of
/// promises that makes, and for the measurement that says why `scratch_root()` alone does not.
///
/// ⚠ The path is NOT created. Callers differ on what belongs there — a directory, a unix socket, a
/// file with an extension on it — and creating one shape here would make most callers delete it
/// again.
///
/// ⚠ The sweep's outcome is discarded on purpose. This is housekeeping: a leftover this process may
/// not delete belongs to somebody else and is not a reason to fail the work the caller came to do.
/// [`reap_predecessors`] stays public for the caller that does want to say what it collected.
///
/// # Panics
///
/// When the machine's scratch root is not absolute — [`scratch_root`].
#[must_use]
pub fn scratch_for(prefix: &str, tail: &str) -> PathBuf {
    let root = scratch_root();
    let mine = scratch_name(prefix, tail);
    if first_time_for(prefix) {
        let _ = reap_predecessors(&root, prefix, &mine);
    }
    root.join(mine)
}

/// ⛔⛔⛔⛔⛔ **THE TIGHTEST `sun_path` A PLATFORM THIS PROJECT RUNS ON HAS** — register item 950 ⑴,
/// and the ceiling a scratch path under which somebody binds a socket has to fit.
///
/// # ⚠⚠⚠⚠ Why a number here rather than a `cfg` at each bind site
///
/// A `cfg(target_os)` would make every check pass on the machine it was written on, which is the
/// whole defect: **a socket path is refused by the platform it is bound on, and this workspace is
/// developed on the one with the LOOSER limit.** Linux gives 108 bytes and macOS gives 104, so a
/// path measured against the running platform is measured against the wrong one on every developer
/// machine. The tightest of them is the only number a claim can be made with.
///
/// ⚠⚠ MEASURED rather than quoted: the macOS CI runner refused
/// `a_live_daemons_residue_is_never_removable_however_empty_its_files_are` with *"path must be
/// shorter than SUN_LEN"* on 2026-09-07 (runs 34128530278 and 34132101073), and the same face had
/// already been paid once by hand in `sprag-host`'s `cli.rs` — *"measured at 109 bytes, five
/// over"* — which is why this is here and not a third copy of the arithmetic.
///
/// ⚠ 104 is the size of the FIELD; one byte of it is the terminator, so [`socket_fits`] compares
/// against it strictly.
pub const TIGHTEST_SUN_PATH: usize = 104;

/// ⛔⛔⛔⛔ **THE LONGEST SCRATCH ROOT THIS PROJECT MUST TOLERATE**, measured on the runner that has
/// the tightest limit — register item 950 ⑴.
///
/// Read out of the macOS CI log of 2026-09-07 rather than guessed — see [`MEASURED_TIGHT_ROOT`],
/// which is the evidence this is the LENGTH OF. Linux's `/tmp` is 4, so a path checked against the
/// local root is checked against a root twelve times shorter than the one that refuses it.
///
/// ⚠⚠ **DERIVED AND NOT TYPED, which is register item 946's finding turned on this constant**: a
/// measurement written as a bare number is the escape hatch of every check that rests on it — a
/// round that found this inconvenient could lower it and every socket path would pass again in
/// silence. Measured here, by mutation: setting it to `4` by hand left the whole suite green.
/// Computed from the evidence, lowering it means shortening a path that is asserted to have the
/// shape a macOS scratch root has.
///
/// ⚠ It is a BUDGET and not a fact about this machine: [`socket_fits`] asks whether the part of a
/// path BELOW the scratch root would still fit if the root were that long. That is what makes the
/// answer the same on every machine, which is the only way a developer here can be told.
pub const LONGEST_SCRATCH_ROOT: usize = MEASURED_TIGHT_ROOT.len();

/// ⛔⛔⛔⛔ **THE EVIDENCE [`LONGEST_SCRATCH_ROOT`] IS THE LENGTH OF** — register item 950 ⑴, read
/// out of the macOS CI log of 2026-09-07 (runs 34128530278 and 34132101073).
///
/// macOS gives every session an opaque scratch root of this shape, and its length is the thing
/// that matters: two path components of fixed width and a `/T`. It is kept as the PATH rather than
/// as its length so the number above cannot be lowered without falsifying something a test can
/// check — `a_measured_root_is_the_shape_macos_actually_gives` asserts exactly that.
pub const MEASURED_TIGHT_ROOT: &str = "/var/folders/d8/hvxvltxn0fl4rmnd52sncbth0000gn/T";

/// Whether `path` may be bound as a unix socket on **every** platform this project runs on.
///
/// The part of `path` below [`scratch_root`] is measured against
/// [`LONGEST_SCRATCH_ROOT`] + [`TIGHTEST_SUN_PATH`], so the answer does not depend on the machine
/// asking. A path outside the scratch root is measured whole — there is no budget to reason about.
///
/// # ⚠⚠ What it does NOT do, said plainly
///
/// It does not create anything and it does not bind. A caller that wants a path this answers `true`
/// for shortens its own names — the technique its neighbour in `cli.rs` already uses: a short
/// prefix plus a per-call counter, never an embedded file name.
/// `path` back, or a panic naming why no platform-portable socket can live there — register item
/// 955.
///
/// # ⛔⛔⛔⛔⛔ Why a CONSTRUCTOR and not an assertion each bind site remembers
///
/// Measured 2026-09-08: this workspace binds a unix socket at **21 sites**, and the paths come from
/// about six FACTORIES — `socket_path()` in two test files, `sock_path(tag)` feeding ten binds in
/// one, and a handful of `dir.join(…)`. A check at each bind is twenty-one things to remember and
/// the twenty-second is the one that breaks; a check at the factory is one thing that CANNOT be
/// forgotten, because the caller has to take the value back to use it.
///
/// ⚠⚠ It PANICS rather than answering a `Result`, on `sibling_bin`'s rule one crate over: a caller
/// that carried on would bind a path the platform refuses, and the run that followed would be
/// about the wrong thing. [`socket_fits`] is the predicate for a caller that wants to ask.
///
/// # Panics
///
/// When [`socket_fits`] is false — the message names the path, the ceiling, and the repair.
#[must_use]
pub fn may_bind(path: &std::path::Path) -> PathBuf {
    assert!(
        socket_fits(path),
        "⛔ REGISTER ITEM 955: {} cannot hold a unix socket on every platform this project runs \
         on — `sun_path` is {TIGHTEST_SUN_PATH} bytes at its tightest and the longest scratch root \
         this project must tolerate is {LONGEST_SCRATCH_ROOT} ({MEASURED_TIGHT_ROOT}). Shorten the \
         names this path is built from: a short prefix and a per-call counter, never an embedded \
         file name",
        path.display(),
    );
    path.to_path_buf()
}

#[must_use]
pub fn socket_fits(path: &std::path::Path) -> bool {
    let shown = path.as_os_str().as_encoded_bytes().len();
    let root = scratch_root();
    let below = path
        .strip_prefix(&root)
        .map_or(shown, |rest| rest.as_os_str().as_encoded_bytes().len() + 1);
    LONGEST_SCRATCH_ROOT + below < TIGHTEST_SUN_PATH
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⛔⛔⛔⛔⛔ **THE MEASUREMENT BEHIND THE SOCKET BUDGET IS THE SHAPE macOS ACTUALLY GIVES** —
    /// register item 950 ⑴, and the assertion that keeps [`LONGEST_SCRATCH_ROOT`] from being a
    /// number somebody can lower.
    ///
    /// # ⛔⛔⛔ Why the evidence is checked and not just the number
    ///
    /// Measured by mutation while this was written: with the budget typed as a bare `4`, the whole
    /// suite stayed **green** — nothing in this workspace was asking whether it was right. That is
    /// register item 946's finding one crate over: *the measurement constant is the escape hatch of
    /// every check built on it.* So the budget is the LENGTH OF A PATH, and this asserts the path
    /// is the thing it claims to be — two opaque components under `/var/folders` and a `/T`. A
    /// round that wants a smaller budget has to falsify that, which is visible.
    ///
    /// ⚠ It does NOT assert this is the root of the machine running it — on Linux it never is. The
    /// claim is about what the tightest platform hands out, which is why a `cfg` would be wrong
    /// here for the reason [`TIGHTEST_SUN_PATH`] states.
    #[test]
    fn a_measured_root_is_the_shape_macos_actually_gives() {
        let parts: Vec<&str> = MEASURED_TIGHT_ROOT.split('/').skip(1).collect();
        assert_eq!(
            (parts.first().copied(), parts.last().copied(), parts.len()),
            (Some("var"), Some("T"), 5),
            "⛔ REGISTER ITEM 950 ⑴: the budget behind `socket_fits` is the LENGTH of \
             {MEASURED_TIGHT_ROOT:?}, and that is supposed to be a macOS session scratch root — \
             `/var/folders/<two opaque components>/T`. It is not one any more, so the number it \
             feeds is no longer a measurement of anything",
        );
        assert!(
            LONGEST_SCRATCH_ROOT + "/x.sock".len() < TIGHTEST_SUN_PATH,
            "⚠⚠ THE BUDGET LEAVES NO ROOM AT ALL: a root of {LONGEST_SCRATCH_ROOT} bytes under a \
             {TIGHTEST_SUN_PATH}-byte ceiling cannot hold even a one-character socket name, so \
             `socket_fits` would refuse every path and say nothing useful about any of them",
        );
        assert_eq!(
            LONGEST_SCRATCH_ROOT, 48,
            "⚠ AND THE NUMBER IS PINNED, because the two constants have to move together: a \
             re-measurement on a different runner is a decision somebody takes, and it changes \
             this line as well as the path above it",
        );
    }

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

    /// ⛔⛔⛔⛔⛔ **A PREFIX MUST NOT EAT ITS OWN SIBLINGS** — register item 930, driven on the real
    /// names one binary puts in one root.
    ///
    /// `sprag-latency` writes four shapes beside each other: its own socket, a `-mute-` directory,
    /// a `-poll-` socket, and TWO FIXED-NAME FILES it reads back as input. A sweep under the short
    /// prefix sees all five. If `owner_in` were looser — if it took the first digits anywhere, or
    /// allowed the pid to follow a word — the instrument would delete the manifests it is about to
    /// read, and it would do it only on the runs where those files happened to be stale.
    ///
    /// ⚠ Driven here rather than reasoned about in a comment: this pair of prefixes was checked by
    /// hand in a shell first, which is a second implementation of the rule and therefore no check
    /// at all (this workspace's rule 10). The rule has one implementation and this asks it.
    #[test]
    fn a_prefix_does_not_reach_into_its_siblings() {
        let short = "sprag-latency";
        assert_eq!(owner_in("sprag-latency-12345-0.sock", short), Some(12345));
        for kept in [
            "sprag-latency-mute-12345",
            "sprag-latency-poll-12345-7.sock",
            "sprag-latency-manifests.toml",
            "sprag-latency-no-such-manifests.toml",
        ] {
            assert_eq!(
                owner_in(kept, short),
                None,
                "{kept:?} carries no owner UNDER {short:?}, and a sweep there must leave it: two \
                 of these are files the binary reads back as input",
            );
        }
        // ⛔⛔⛔⛔⛔ AND A DOT DOES NOT CONTINUE THE SEGMENT — the case that made item 930 rename a
        // socket. `sprag-latency-<pid>.sock` was the name before, and it is UNREADABLE here: `.`
        // ends the run of digits without being the `-` this rule requires, so the owner is None
        // and the socket could never be collected. That is why the site now asks for the tail
        // `"0.sock"` and gets `sprag-latency-<pid>-0.sock`.
        //
        // ⚠⚠ THIS ASSERTION EXISTS BECAUSE A MUTATION CAME BACK GREEN. Adding `Some('.')` to the
        // accepting arm of `owner_in` changed nothing any test could see: every name checked above
        // fails earlier, on having no digits at all. A rule this file's own callers were renamed
        // for was going unmeasured.
        assert_eq!(
            owner_in("sprag-latency-12345.sock", short),
            None,
            "a dot is not a segment separator, so this shape can never be reaped — which is the \
             whole reason the site was renamed",
        );

        // ...and each sibling is readable under the prefix that actually built it.
        assert_eq!(
            owner_in("sprag-latency-mute-12345", "sprag-latency-mute"),
            Some(12345)
        );
        assert_eq!(
            owner_in("sprag-latency-poll-12345-7.sock", "sprag-latency-poll"),
            Some(12345)
        );
        assert_eq!(
            owner_in("sprag-live-turn-12345-99", "sprag-live-turn"),
            Some(12345),
            "the tag belongs in the PREFIX, which is why `live_agent` passes it there",
        );
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

    /// ⛔⛔⛔⛔⛔ **THE INVARIANT THE SEAM EXISTS FOR** — register item 795: whatever a caller asks
    /// for, [`owner_in`] reads this run back out of the answer.
    ///
    /// Driven over the tail shapes this workspace actually uses, and one that is deliberately
    /// hostile: a tail that opens with digits. A name built by hand as `<prefix>-<n>-<pid>` is the
    /// mistake this test would catch, and the reason the pid's position is the seam's promise
    /// rather than each call site's habit.
    #[test]
    fn a_name_this_crate_builds_is_a_name_it_can_read() {
        let me = std::process::id();
        for tail in [
            "",
            "0.tree",
            "clean-ThreadId(4)",
            "written",
            "7-still-digits",
        ] {
            let name = scratch_name("sprag-nametest", tail);
            assert_eq!(
                owner_in(&name, "sprag-nametest"),
                Some(me),
                "{name:?} must name this run where the reaper looks, or it can never be collected",
            );
            assert_eq!(
                may_reap(&name, "sprag-nametest", &name, Owner::Gone),
                Reap::ItIsMine,
                "{name:?} is this run's own and must survive its own sweep",
            );
        }
    }

    /// ⛔⛔⛔ **THE HAZARD THAT MAKES NAMING AND SWEEPING ONE CALL** — measured on a real name from
    /// this machine's scratch root, 2026-09-06T14:27:10Z.
    ///
    /// `sprag-869-<pid>` is a fixture named after register item 869. Swept under the prefix that
    /// built it, its owner is the run. Swept under a SHORTER prefix, the item number reads as the
    /// owner — and pid 869 is a live service on this workstation, so the answer would be *kept for
    /// the wrong reason today* and *deleted for the wrong reason* on the day that pid is free.
    /// Neither call site can see the mismatch; [`scratch_for`] cannot make it.
    #[test]
    fn a_prefix_shorter_than_the_one_that_built_the_name_names_the_wrong_process() {
        assert_eq!(owner_in("sprag-869-2575955", "sprag-869"), Some(2_575_955));
        assert_eq!(
            owner_in("sprag-869-2575955", "sprag"),
            Some(869),
            "the shorter prefix reads the ITEM NUMBER as a process id — this is the answer the \
             seam exists to make unaskable, not one it corrects",
        );
    }

    /// ⚠ The sweep is owed once per prefix per process — [`first_time_for`]'s whole contract, and
    /// the reason a helper called five hundred times does not list a thirteen-thousand-entry root
    /// five hundred times.
    #[test]
    fn a_prefix_is_swept_once_per_process() {
        assert!(
            first_time_for("sprag-oncetest-and-nobody-else"),
            "the first ask owes the sweep",
        );
        assert!(
            !first_time_for("sprag-oncetest-and-nobody-else"),
            "the second ask does not — every entry a first sweep left was left because its owner \
             may be alive, and asking again cannot change that",
        );
        assert!(
            first_time_for("sprag-oncetest-a-different-prefix"),
            "and the answer is per PREFIX, not a single latch for the process",
        );
    }

    /// ⛔⛔ **THE MUTATION FOR THE SHAPE THE FIRST DRAFT SKIPPED**: a dead run's socket and its lock
    /// file go, exactly like its directory, and a live run's stay.
    ///
    /// Item 927 collected directories because its own subject was one. 2,667 entries in this
    /// machine's scratch root are not, and a reaper that walked past them would pass every test
    /// written about directories.
    #[test]
    fn a_dead_runs_socket_and_lock_go_the_way_its_directory_does() {
        let root = scratch_root().join(format!("sprag-scratch-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("a scratch root of this test's own");

        // Pid 0 is never a running process on Linux, so `/proc/0` does not exist.
        let dead_lock = "sprag-filetest-0-4.lock".to_string();
        let dead_dir = "sprag-filetest-0-4.tree".to_string();
        let live_lock = format!("sprag-filetest-{}-4.lock", std::process::id());
        let mine = scratch_name("sprag-filetest", "9.tree");
        for name in [&dead_lock, &live_lock] {
            std::fs::write(root.join(name), b"x").expect("a planted file");
        }
        std::fs::create_dir_all(root.join(&dead_dir)).expect("a planted directory");
        std::fs::create_dir_all(root.join(&mine)).expect("this run's own");

        let reaped = reap_predecessors(&root, "sprag-filetest", &mine);

        if cfg!(target_os = "linux") {
            // ⚠ Both sides sorted: `read_dir` answers in whatever order the filesystem holds, and
            // an assertion that depended on it would be a flake wearing a gate's clothes.
            let mut removed = reaped.removed.clone();
            removed.sort();
            let mut expected = vec![dead_dir, dead_lock];
            expected.sort();
            assert_eq!(removed, expected, "{reaped:?}");
        } else {
            assert!(
                reaped.removed.is_empty(),
                "this platform answers Unknown, so it reaps nothing and says so: {reaped:?}",
            );
        }
        assert!(
            root.join(&live_lock).exists(),
            "a live owner's lock is in use whatever its type is: {reaped:?}",
        );
        assert!(
            root.join(&mine).exists(),
            "this run's own scratch is untouched: {reaped:?}",
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
