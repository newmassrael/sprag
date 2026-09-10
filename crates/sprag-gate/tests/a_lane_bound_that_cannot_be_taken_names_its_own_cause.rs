//! ⛔⛔⛔⛔⛔ **THREE DIFFERENT FAILURES SPOKE WITH ONE VOICE, AND IT WAS THE WRONG ONE** — register
//! item 1009.
//!
//! `.githooks/rust-gates.sh` bounds its own lane: it divides this host's free memory by
//! `[routed] precommit_kb` and hands the quotient down as `CARGO_BUILD_JOBS`. Three separate things
//! can stop that, and until 2026-09-10 all three printed *no usable `precommit_kb` in <file>*.
//!
//! **MEASURED, on a host whose `/proc` answers ENOENT to every path, with this repository's own
//! declaration in place**: the hook's `sed` extracted `9260688` perfectly and the hook announced
//! that the file said nothing. What was missing was `/proc/meminfo`. A person on that host is sent
//! to edit a declaration that is correct — which is the defect item 1006 is named for, eight lines
//! above the line that has it, reached by a different road.
//!
//! # ⚠⚠ The third cause could not be spoken at all
//!
//! `precommit_kb` is `VmHWM` sampled out of procfs (item 1008), so it is **one platform's** memory
//! behaviour. Nothing said whether it was THIS host's. Item 1008 recorded the platform beside the
//! number; this clause is what makes something READ it — item 932's own rule, that a row nobody
//! divides by is a number in a file, applied to the field 1008 added.
//!
//! # ⚠⚠⚠ What this does NOT claim
//!
//! Not that an unverifiable host is bounded some other way. Every arm below leaves the lane exactly
//! as it ran yesterday — unbounded, and loud about it. The claim is that the sentence is TRUE:
//! a host that cannot be measured says so, a foreign reading says so, and only a declaration that
//! really says nothing is reported as saying nothing.
//!
//! ⚠ **THE FIRST CASE IS THE CONTROL AND IT IS NOT DECORATION.** Three arms that all assert *the
//! false sentence is absent* would stay green if the function printed nothing at all, or were
//! deleted. The control drives the arm that must still COMPUTE, with a staged reading and a staged
//! `MemAvailable`, and asserts the quotient.

use std::path::{Path, PathBuf};
use std::process::Command;

use sprag_gate::sources::workspace_root;

/// The library under test. Sourced rather than executed: its dispatch refuses a bare run on
/// purpose (register item 819), and the subject here is one function inside it.
const LIBRARY: &str = ".githooks/rust-gates.sh";

/// The sentence that must only ever be said about a declaration that really says nothing.
const BLAMES_THE_FILE: &str = "no usable precommit_kb";

/// What one drive of `rust_gates_bound_this_lane` said and what it left behind.
struct Bound {
    said: String,
    jobs: String,
}

/// Drive the hook function against a staged tree.
///
/// ⚠⚠ `set -euo pipefail` is set here because the hook that calls this function has it, and this
/// function has already been killed once by that combination: `sed` on an absent file exits 2,
/// `pipefail` carries it to the assignment and `set -e` ends the hook at that line in silence
/// (register item 467's throwaway repository, which has no declaration). A driver without those
/// options would be a different program from the one that runs.
fn drive(tree: &Path, meminfo: &Path) -> Bound {
    const SCRIPT: &str = "set -euo pipefail\n\
         . \"$1\"\n\
         repo_root=\"$2\"\n\
         rust_gates_bound_this_lane\n\
         printf 'JOBS=%s\\n' \"${CARGO_BUILD_JOBS:-unset}\"\n";
    let out = Command::new("bash")
        .arg("-c")
        .arg(SCRIPT)
        .arg("_")
        .arg(workspace_root().join(LIBRARY))
        .arg(tree)
        .env("RUST_GATES_MEMINFO", meminfo)
        // ⚠ The function defaults these from the environment (`${CARGO_BUILD_JOBS:-…}`), so a
        // developer who exports them would otherwise change what this gate reads.
        .env_remove("CARGO_BUILD_JOBS")
        .env_remove("RUST_TEST_THREADS")
        .current_dir(workspace_root())
        .output()
        .expect("the hook library is sourced by a shell");
    assert!(
        out.status.success(),
        "⚠ THE STAGING, NOT THE CLAIM: driving `rust_gates_bound_this_lane` exited {:?}. Under the \
         hook's own `set -euo pipefail` this function must always return cleanly — a non-zero here \
         is the hook dying at a line, which is register item 467's failure.\nstderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr),
    );
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    Bound {
        said: String::from_utf8_lossy(&out.stderr).to_string(),
        jobs: stdout
            .lines()
            .find_map(|line| line.strip_prefix("JOBS="))
            .unwrap_or("missing")
            .to_string(),
    }
}

/// A tree holding exactly the declaration text given.
///
/// ⚠ Through [`sprag_scratch`] and never `std::env::temp_dir()` — register item 794: the bare call
/// answers a RELATIVE path when `TMPDIR` is set-and-empty, and this fixture would then write a
/// declaration inside this crate's own directory.
fn tree_declaring(tag: &str, decl: &str) -> PathBuf {
    let tree = sprag_scratch::scratch_for("sprag-lane-bound", tag);
    std::fs::create_dir_all(tree.join(".claude")).expect("a staging tree for the hook");
    std::fs::write(tree.join(".claude/remote-build.toml"), decl).expect("a declaration to read");
    tree
}

/// This host's platform, asked the way the hook asks it rather than mapped from a Rust constant.
fn host_platform() -> String {
    let out = Command::new("uname")
        .arg("-s")
        .output()
        .expect("uname -s answers on every platform this workspace runs on");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// ⭐ **THE CONTROL: when the reading is this platform's and the host can be measured, the bound is
/// TAKEN** — and the quotient is asserted, not just the absence of a complaint.
#[test]
fn a_reading_from_this_platform_bounds_the_lane_by_the_free_memory_it_measured() {
    let tree = tree_declaring(
        "control",
        &format!(
            "[routed]\nprecommit_kb = 5000000\nprecommit_platform = \"{}\"\n",
            host_platform(),
        ),
    );
    let meminfo = tree.join("meminfo");
    std::fs::write(
        &meminfo,
        "MemTotal:       32000000 kB\nMemAvailable:   20000000 kB\n",
    )
    .expect("a staged MemAvailable for the arm that divides");

    let bound = drive(&tree, &meminfo);
    let _ = std::fs::remove_dir_all(&tree);

    assert_eq!(
        bound.jobs, "4",
        "⛔ ITEM 1009's CONTROL: 20,000,000 kB free divided by a 5,000,000 kB peak is 4 jobs, and \
         the lane handed down `{}`. Every other case in this file asserts that a SENTENCE is \
         absent; if this arm stops computing, those would all still pass over a function that does \
         nothing.\nsaid:\n{}",
        bound.jobs, bound.said,
    );
    assert!(
        bound.said.contains(&host_platform()),
        "⛔ ITEM 1009: the bound was taken and the line does not say which platform's reading it \
         was taken from. The number is one platform's memory behaviour and the line that spends it \
         is where a reader finds out which.\nsaid:\n{}",
        bound.said,
    );
}

/// ⛔⛔⛔ **A HOST WHOSE FREE MEMORY CANNOT BE READ IS NOT A DECLARATION THAT SAYS NOTHING.**
///
/// This is the arm that was measured wrong: procfs absent, the reading perfectly extractable, and
/// the hook blaming the file. On macOS this arm is what a developer's commit reaches for real —
/// there is no `/proc/meminfo` there at all.
#[test]
fn a_host_that_cannot_be_measured_says_so_instead_of_blaming_the_declaration() {
    let tree = tree_declaring(
        "no-meminfo",
        &format!(
            "[routed]\nprecommit_kb = 5000000\nprecommit_platform = \"{}\"\n",
            host_platform(),
        ),
    );
    let absent = tree.join("no-meminfo-here");

    let bound = drive(&tree, &absent);
    let _ = std::fs::remove_dir_all(&tree);

    assert!(
        !bound.said.contains(BLAMES_THE_FILE),
        "⛔ ITEM 1009: `precommit_kb` is right there and extractable, and the lane reported that \
         the declaration has none. The person reading this is sent to edit a file that is correct \
         while the host is what could not answer — item 1006's rule, that a refusal naming the \
         wrong cause is worse than none.\nsaid:\n{}",
        bound.said,
    );
    assert!(
        bound.said.contains("free memory"),
        "⛔ ITEM 1009: the lane went unbounded and did not say that reading this HOST's free \
         memory is what failed. An unbounded lane is acceptable; an unexplained one sends the next \
         person to the wrong file.\nsaid:\n{}",
        bound.said,
    );
    assert_eq!(
        bound.jobs, "unset",
        "⚠ ITEM 1009: nothing may be handed down as a job count when the divisor could not be \
         measured — a guessed bound is the swap storm item 456 opened on.\nsaid:\n{}",
        bound.said,
    );
}

/// ⛔⛔⛔ **A READING TAKEN SOMEWHERE ELSE DOES NOT BOUND THIS HOST**, and this is what makes the
/// platform item 1008 recorded a row somebody USES.
#[test]
fn a_reading_from_another_platform_does_not_bound_this_host_and_says_which() {
    let tree = tree_declaring(
        "foreign",
        "[routed]\nprecommit_kb = 5000000\nprecommit_platform = \"Plan9\"\n",
    );
    let meminfo = tree.join("meminfo");
    std::fs::write(&meminfo, "MemAvailable:   20000000 kB\n").expect("a staged MemAvailable");

    let bound = drive(&tree, &meminfo);
    let _ = std::fs::remove_dir_all(&tree);

    assert!(
        !bound.said.contains(BLAMES_THE_FILE),
        "⛔ ITEM 1009: the declaration named its reading's platform and the lane reported that it \
         says nothing.\nsaid:\n{}",
        bound.said,
    );
    assert!(
        bound.said.contains("Plan9"),
        "⛔ ITEM 1009: the lane never read `precommit_platform`, so a peak measured on another \
         platform was either spent as if it were this host's or dropped without a word. The field \
         exists precisely so this line can name it.\nsaid:\n{}",
        bound.said,
    );
    assert_eq!(
        bound.jobs, "unset",
        "⛔ ITEM 1009: a foreign reading must not be divided by. Free memory was staged and \
         readable here, so a job count appearing means the platform was not consulted at all and \
         another host's peak was spent as if it were this one's.\nsaid:\n{}",
        bound.said,
    );
}

/// ⚠ **AND THE ORIGINAL SENTENCE STILL HAS ITS OWN CASE.** Splitting a message three ways is only
/// an improvement if the first one still gets said when it is true — a repair that removed it would
/// make a declaration nobody finished look exactly like a host nobody could measure.
#[test]
fn a_declaration_that_really_says_nothing_about_this_lane_is_still_told_so() {
    let tree = tree_declaring("silent", "[routed]\n");
    let meminfo = tree.join("meminfo");
    std::fs::write(&meminfo, "MemAvailable:   20000000 kB\n").expect("a staged MemAvailable");

    let bound = drive(&tree, &meminfo);
    let _ = std::fs::remove_dir_all(&tree);

    assert!(
        bound.said.contains(BLAMES_THE_FILE),
        "⚠ ITEM 932's own case is gone: a `[routed]` table with no `precommit_kb` has to be \
         reported as exactly that, or the lane running unbounded is a silence again.\nsaid:\n{}",
        bound.said,
    );
}
