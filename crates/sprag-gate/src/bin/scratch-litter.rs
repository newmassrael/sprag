//! **WHAT THIS MACHINE'S SCRATCH DIRECTORY IS HOLDING, IN TWO NUMBERS** — register item 927.
//!
//! ```text
//! cargo run -q -p sprag-gate --bin scratch-litter
//! cargo run -q -p sprag-gate --bin scratch-litter -- /some/other/root
//! ```
//!
//! # ⛔⛔⛔ What was measured, and why one number was not enough
//!
//! On 2026-09-06 at 13:40:27 UTC this workstation's `/tmp` held **13,870** `sprag-*` directories
//! totalling **19,931 MB**. Twenty-two of them held 98.3% of the bytes; the other 13,848 held
//! 99.84% of the count. Between 12:43 and 13:40 the count rose by seventy-four and the bytes did
//! not move, because what ran in between was a test suite rather than a build.
//!
//! No gate in this workspace counts either: register item 794's predicate measures inside the
//! TREE (`find crates` minus `git ls-files`), and item 886's measures REGISTERED worktrees. A
//! directory under `/tmp` is outside both populations, so it accrued for two weeks with nothing
//! able to say so.
//!
//! # ⚠⚠ It reports, and refuses only a reading it could not make
//!
//! [`sprag_gate::pty_demand`]'s stance and its reason: a threshold picked before the first honest
//! measurement is a number somebody keeps. So there is no size this refuses — but a root that does
//! not exist, is not a directory, or cannot be listed is REFUSED rather than reported as clean.
//! Those are the readings that were never taken, and item 924 is what it costs to let one of them
//! pass as a zero.

use std::collections::HashSet;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

fn main() -> std::process::ExitCode {
    let mut args = std::env::args_os().skip(1);
    // ⛔⛔⛔⛔ THE ROOT IS NAMED BY THE CALLER, AND THAT IS REGISTER ITEM 794's DOING.
    //
    // A first draft defaulted to `std::env::temp_dir()`, guarded by a copy of item 794's
    // `is_absolute` refusal, because this crate's charter keeps `sprag_scratch` out of a
    // `[[bin]]`'s reach. `no_product_code_takes_a_scratch_root_unchecked` refused the copy — and it
    // was right: the choice was never *duplicate the policy* against *break the charter*. An
    // instrument reporting about a directory can simply be TOLD which directory, which is also the
    // only version of this whose output says what it was a reading of.
    let Some(given) = args.next() else {
        eprintln!(
            "scratch-litter: name the root to read — there is no default, because a reading has to \
             say which directory it was a reading OF (register item 794). On this machine that is \
             `/tmp`, or whatever `TMPDIR` names:\n    scratch-litter \"${{TMPDIR:-/tmp}}\""
        );
        return std::process::ExitCode::FAILURE;
    };
    let root = PathBuf::from(given);
    if let Some(extra) = args.next() {
        eprintln!(
            "scratch-litter: one root, and {extra:?} is a second — a reading of two places answers \
             about neither"
        );
        return std::process::ExitCode::FAILURE;
    }

    let found = match entries_under(&root) {
        Ok(found) => found,
        Err(why) => {
            eprintln!("scratch-litter: {why}");
            return std::process::ExitCode::FAILURE;
        }
    };

    let reading = sprag_gate::litter::read(found);
    // ⚠ THE NUMBER SAYS WHAT IT IS THE NUMBER OF — register item 794. "Allocated" is not a
    // decoration: for the family that dominates the COUNT it is nearly the entire figure, and a
    // reader comparing this against `du` has to know they are the same question.
    println!(
        "scratch-litter: {} director(ies), {:.1} MB allocated (blocks, as `du` counts them) under {}",
        reading.dirs,
        reading.bytes as f64 / 1_048_576.0,
        root.display(),
    );

    // ⛔⛔⛔ AND WHEN IT IS NOT ZERO IT SAYS WHICH FAMILY — item 927's clause (2), and register item
    // 811's rule that attribution is only possible in the moment. A total with no families behind
    // it tells the reader that something accumulated and leaves them to find out what.
    if reading.is_clean() {
        println!("scratch-litter: nothing is standing in it");
        return std::process::ExitCode::SUCCESS;
    }
    println!(
        "scratch-litter: {} famil(ies), by count:",
        reading.series.len()
    );
    for series in &reading.series {
        println!(
            "  {:>7} dir(s)  {:>10.1} MB  {}",
            series.dirs,
            series.bytes as f64 / 1_048_576.0,
            series.stem,
        );
    }
    std::process::ExitCode::SUCCESS
}

/// Every `sprag-*` directory directly under `root`, with what it holds.
///
/// # ⚠⚠ Errors are the point of the return type
///
/// A root that cannot be listed is not a root with nothing in it. This is the one place that
/// distinction can be made, and [`sprag_gate::litter::read`] deliberately cannot make it.
fn entries_under(root: &Path) -> Result<Vec<(String, u64)>, String> {
    if !root.exists() {
        return Err(format!(
            "{} does not exist — that is a reading nobody took, not a machine with no litter on it",
            root.display(),
        ));
    }
    if !root.is_dir() {
        return Err(format!("{} is not a directory", root.display()));
    }
    let listing = std::fs::read_dir(root)
        .map_err(|why| format!("{} could not be listed: {why}", root.display()))?;

    // ⚠⚠ ONE `seen` SET FOR THE WHOLE READING, not one per directory — `du` given many arguments
    // counts a hardlinked inode once across all of them, and a target directory is full of
    // hardlinks. A set per directory would count each link again and report a total larger than
    // the disk holds.
    let mut seen: HashSet<(u64, u64)> = HashSet::new();
    let mut found = Vec::new();
    for entry in listing {
        let entry =
            entry.map_err(|why| format!("{} gave an unreadable entry: {why}", root.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("sprag-") {
            continue;
        }
        // ⚠ Judged by KIND rather than silently: `file_type` here does not follow symlinks, so a
        // symlink out of the root is not walked as if it were a directory in it.
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if !kind.is_dir() {
            continue;
        }
        found.push((name, held_by(&entry.path(), &mut seen)));
    }
    Ok(found)
}

/// What one directory occupies, in bytes of ALLOCATED BLOCKS, walked without following symlinks.
///
/// # ⛔⛔⛔⛔⛔ Blocks, not file lengths — and the difference is most of the answer
///
/// Register item 794's rule is that a number must say what it is the number OF, and this one was
/// measured twice before it said the right thing. Summing `metadata.len()` — the apparent size of
/// the files — reports **0.0 MB** for the 13,848 small directories that make up 99.84% of the
/// count, because they hold almost no file bytes: what they occupy is the 4 KB block each
/// directory costs merely by existing. `du` counts those and this now counts them too, so the two
/// answer the same question and a later round comparing them is comparing like with like.
///
/// ⚠⚠ Inodes are counted ONCE (`seen`), which is `du`'s rule for hardlinks. Without it a cargo
/// target directory reports several times what it occupies. Measured before the fix: this walk
/// said 32,449 MB where `du` said 19,931 MB over the same tree plus 8.3 GB it had excluded.
///
/// ⚠ Unreadable children contribute ZERO rather than aborting the walk: one permission-denied
/// subdirectory on a developer's machine must not turn the whole reading into a refusal. The total
/// is therefore a FLOOR, which is stated rather than hidden.
fn held_by(dir: &Path, seen: &mut HashSet<(u64, u64)>) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(at) = stack.pop() {
        // The directory's own blocks. For this population that is nearly the whole number.
        if let Ok(meta) = std::fs::symlink_metadata(&at)
            && seen.insert((meta.dev(), meta.ino()))
        {
            total += meta.blocks() * 512;
        }
        let Ok(listing) = std::fs::read_dir(&at) else {
            continue;
        };
        for entry in listing.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                stack.push(entry.path());
                continue;
            }
            if let Ok(meta) = std::fs::symlink_metadata(entry.path())
                && seen.insert((meta.dev(), meta.ino()))
            {
                total += meta.blocks() * 512;
            }
        }
    }
    total
}
