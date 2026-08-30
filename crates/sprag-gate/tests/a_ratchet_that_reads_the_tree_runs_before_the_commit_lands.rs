//! **A TEST WHOSE INPUT IS THE REPOSITORY RUNS BEFORE A COMMIT LANDS** — register item 784.
//!
//! # ⚠⚠⚠⚠⚠ What went wrong without this
//!
//! `.githooks/pre-commit` ran clippy, rustfmt and the rustdoc gate, and **no test at all**. So the
//! ratchets in this crate — every one of which scans the workspace's own files — were enforced by
//! CI and by nothing else. Measured 2026-08-31: `1f81f93` added a test function to `sprag-tui`
//! whose NAME ended in `held()`, the literal needle
//! `the_only_plugin_that_can_be_held_is_the_one_that_reads_a_hold` scans every source for. The
//! commit passed the hook, the push passed the hook, and **two commits stood red on two platforms
//! for two rounds**. It was caught by a person re-running the gate by hand, which is not a
//! mechanism.
//!
//! # ⚠⚠⚠ Why a LANE, with the numbers that chose it
//!
//! Running the whole suite here is the obvious answer and it was measured before being declined:
//! `cargo test --workspace` is **258.6 s** warm on the build machine (3819 tests) against **22.3 s**
//! for `sprag-gate` alone, both after `cargo build --workspace --all-targets`. Item 457 is already
//! open against this same hook for costing too much per commit; a remedy that triples it trades one
//! debt for another. What runs instead is the tests an unrelated crate's commit can actually turn
//! red — **the ones whose input is the tree**.
//!
//! # ⚠⚠ Why the membership is DERIVED and not a list
//!
//! A hand-kept list of targets is the shape this workspace's rule 6 is about: it goes stale in the
//! safe-looking direction, because a test that acquires the tree-reading property and is left out
//! of it simply keeps passing. So this gate does not check a list against itself. It reads the
//! TREE, decides which test files reach outside their own crate, reads the HOOKS, and requires the
//! two to agree — item 470's rule that a ratchet which cannot rot reads both artefacts. It fires in
//! both directions:
//!
//! * a test reaches the tree and no hook target runs it: the lane has gone stale. Red.
//! * a hook names a target the tree does not have: the reference is dead. Red.
//!
//! # ⚠ The residue, stated rather than hidden
//!
//! [`REACHES`] is a list of SPELLINGS, and a fourth way to reach the tree would escape it. That is
//! not hidden behind a green run: the remedy is to route tree access through
//! [`sprag_gate::sources::workspace_root`], which is the spelling this crate already publishes for
//! it, and until every reader uses it this gate's population is as good as its vocabulary. What it
//! does buy is that the two artefacts cannot drift apart in silence, which is the whole of what was
//! missing.

use sprag_gate::sources::workspace_root;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The hooks that must run the lane, in the order a change meets them.
///
/// ⚠ BOTH, not just `pre-commit`. The commit hook is where item 784's failure landed, but an amend
/// or a `--no-verify` reaches the push with no stamp — and a push is the last place a red can be
/// stopped before CI is the one to say so.
const HOOKS: [&str; 2] = ["pre-commit", "pre-push"];

/// How a test file says it is reading the REPOSITORY rather than its own crate.
///
/// Each entry is a substring that must appear, and an optional second substring that must appear
/// with it. The pair exists for exactly one case and it is the one that decides which crates are in
/// the lane: `CARGO_MANIFEST_DIR` alone is how a test finds its OWN fixtures — `sprag-host`'s
/// `cli.rs` and `wire_client.rs` both do that and neither reads a line this repository's other
/// crates wrote — while `CARGO_MANIFEST_DIR` joined with `".."` is a walk UP, out of the crate and
/// into the tree. Collapsing the two would pull two of the slowest integration suites in the
/// workspace into a hook that runs on every commit, for a property they do not have.
const REACHES: [(&str, Option<&str>); 4] = [
    // The spelling this crate publishes, and the one a new reader should use.
    ("workspace_root", None),
    // The workspace's lockfile, which any crate's dependency edit rewrites.
    ("Cargo.lock", None),
    // Shelling out to cargo, which answers about the workspace graph and not about one crate.
    ("env!(\"CARGO\")", None),
    // A walk up and out of the crate. See this constant's own doc for why the pair is needed.
    ("CARGO_MANIFEST_DIR", Some("\"..\"")),
];

/// One target a hook's lane can name: a whole crate, or a single integration test inside one.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Target {
    /// `cargo test -p <crate>` — every test in the crate.
    Crate(String),
    /// `cargo test -p <crate> --test <file>` — one integration test.
    Test(String, String),
}

impl Target {
    /// Whether this target's run would execute `crates/<krate>/tests/<file>.rs`.
    fn covers(&self, krate: &str, file: &str) -> bool {
        match self {
            Self::Crate(c) => c == krate,
            Self::Test(c, f) => c == krate && f == file,
        }
    }

    /// The crate this target is about, whichever shape it has.
    fn krate(&self) -> &str {
        match self {
            Self::Crate(c) | Self::Test(c, _) => c,
        }
    }
}

/// Every `cargo test -p …` the hook at `path` runs, read as targets.
///
/// ⚠ Parsed from the file's own words rather than from a constant this test also owns. A gate that
/// compared its own copy of the lane against its own copy of the population would agree with itself
/// for ever — the artefact being read has to be the one git runs.
fn lane_of(path: &Path) -> BTreeSet<Target> {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read the hook at {}: {e}", path.display()));
    let mut found = BTreeSet::new();
    for tail in text.split("cargo test -p ").skip(1) {
        let mut words = tail.split_whitespace().map(unshell);
        let Some(krate) = words.next() else { continue };
        // `--test <file>` narrows it; anything else ends the target.
        match (words.next().as_deref(), words.next()) {
            (Some("--test"), Some(file)) => {
                found.insert(Target::Test(krate, file));
            }
            _ => {
                found.insert(Target::Crate(krate));
            }
        }
    }
    found
}

/// A word from a hook, with the shell punctuation that can sit against it removed.
///
/// ⚠⚠ MEASURED, on this gate's own first run: the lane is one single-quoted string, so its LAST
/// target parsed as `gpu_free'` and the gate reported that `sprag-client`'s ratchet was outside a
/// lane that named it. A parser reading a shell script has to read shell — and this failure is the
/// benign direction only by luck: a quote on the FIRST target would have made a covered test look
/// covered under a name nothing else uses.
fn unshell(word: &str) -> String {
    word.trim_matches(|c| matches!(c, '\'' | '"' | ';' | '&' | '`'))
        .to_owned()
}

/// This file's own stem. It quotes every entry of [`REACHES`] as data, so it matches all of them and
/// can be the sole match for a spelling that has stopped meaning anything —
/// see [`every_spelling_of_reaching_the_tree_is_load_bearing`].
const SELF: &str = "a_ratchet_that_reads_the_tree_runs_before_the_commit_lands";

/// Which entries of [`REACHES`] put `text` in the population.
fn spellings_matching(text: &str) -> Vec<usize> {
    REACHES
        .iter()
        .enumerate()
        .filter(|(_, (needle, also))| {
            text.contains(needle) && also.is_none_or(|second| text.contains(second))
        })
        .map(|(at, _)| at)
        .collect()
}

/// Every `crates/<crate>/tests/<file>.rs` whose input is the tree, by [`REACHES`].
fn tests_that_read_the_tree() -> BTreeSet<(String, String)> {
    let crates = workspace_root().join("crates");
    let mut found = BTreeSet::new();
    let entries = std::fs::read_dir(&crates)
        .unwrap_or_else(|e| panic!("read {}: {e}", crates.display()))
        .map(|e| e.expect("a directory entry under crates/").path())
        .collect::<Vec<PathBuf>>();
    for krate in entries {
        let tests = krate.join("tests");
        let Ok(files) = std::fs::read_dir(&tests) else {
            continue;
        };
        for file in files {
            let path = file
                .expect("a directory entry under a crate's tests/")
                .path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if !spellings_matching(&text).is_empty() {
                let name = |p: &Path| {
                    p.file_stem()
                        .and_then(|s| s.to_str())
                        .expect("a utf-8 file name")
                        .to_owned()
                };
                found.insert((name(&krate), name(&path)));
            }
        }
    }
    found
}

/// **THE GATE: every test whose input is the tree is run by every hook's lane, and every target a
/// lane names exists** — register item 784.
#[test]
fn every_test_that_reads_the_tree_is_run_by_the_hooks() {
    let population = tests_that_read_the_tree();
    assert!(
        !population.is_empty(),
        "⚠⚠⚠ THIS GATE'S OWN PREMISE FAILED: no test in this workspace reads the tree, which would \
         make every assertion below vacuous. The file walk is broken, not the hooks",
    );

    for hook in HOOKS {
        let path = workspace_root().join(".githooks").join(hook);
        let lane = lane_of(&path);
        assert!(
            !lane.is_empty(),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 784: `.githooks/{hook}` runs NO test at all. That is the state \
             this item was filed for — the hook ran clippy, rustfmt and rustdoc, a commit broke a \
             ratchet in another crate, and CI was the only thing that ever said so",
        );

        for (krate, file) in &population {
            assert!(
                lane.iter().any(|t| t.covers(krate, file)),
                "⛔⛔⛔⛔⛔ REGISTER ITEM 784: `crates/{krate}/tests/{file}.rs` reads the REPOSITORY, \
                 so a commit to any crate can turn it red — and `.githooks/{hook}` does not run it. \
                 A ratchet outside the lane is enforced by CI alone, which is two rounds of red \
                 nobody can see. The lane is {lane:?}",
            );
        }

        for target in &lane {
            let dir = workspace_root().join("crates").join(target.krate());
            assert!(
                dir.is_dir(),
                "⛔⛔⛔⛔ REGISTER ITEM 784: `.githooks/{hook}` names {target:?}, and \
                 `crates/{}` is not in this tree. A lane pointing at a crate that has been renamed \
                 or removed is the same rot as a stale list, facing the other way — and it fails \
                 the hook for a reason that has nothing to do with the commit",
                target.krate(),
            );
            if let Target::Test(krate, file) = target {
                let test = dir.join("tests").join(format!("{file}.rs"));
                assert!(
                    test.is_file(),
                    "⛔⛔⛔⛔ REGISTER ITEM 784: `.githooks/{hook}` names `-p {krate} --test \
                     {file}`, and `crates/{krate}/tests/{file}.rs` is not in this tree",
                );
            }
        }
    }
}

// ⚠⚠⚠⚠ A SET-EQUALITY TEST BETWEEN THE TWO HOOKS WAS WRITTEN HERE AND TAKEN OUT AGAIN, because
// measuring it disproved its premise. `pre-push` ALREADY ran `cargo test -p sprag-gate` before item
// 784's repair — line 220, under `hook_suite_is_owed`, and only when the pushed range changes a
// hook. So the push hook's `cargo test` invocations are TWO lanes with different scopes, and
// requiring them to equal `pre-commit`'s would turn any future change to the narrow one into a red
// about the wide one. What both hooks are actually required to do is COVER THE POPULATION, and the
// test above asks each of them that separately — a target dropped from either is red on that hook's
// own name.
//
// ⚠ It is also why item 784's own sentence — *"the hook runs clippy, fmt and doc and does not run
// `cargo test`"* — is true of `pre-commit` and only conditionally true of `pre-push`.

/// **EVERY SPELLING IN [`REACHES`] IS THE SOLE REASON SOME TEST IS IN THE POPULATION** — register
/// item 784, and the guard that replaced a threshold after the threshold was measured dead.
///
/// # ⛔⛔⛔⛔⛔ What the threshold did
///
/// The first version of this gate guarded its own premise with `population.len() >= 10`, a number
/// chosen by looking at the tree. Breaking one of the four spellings — the mutation that asks
/// whether [`REACHES`] is doing anything — left **exactly 10** files, and the guard came back
/// GREEN by one. A threshold picked by eye sat one step from the cliff, which is this workspace's
/// rule against absorbing a fault into a bound: the number said nothing, and a green run said the
/// vocabulary was fine when a quarter of it had stopped matching.
///
/// # ⚠⚠ The positive form, and why it can be driven
///
/// Measured 2026-08-31: **every test file in this workspace matches exactly ONE** of the four
/// spellings — there is no overlap at all. So each one carries files nothing else would find, and a
/// spelling that stops matching anything is either a rename nobody followed or a needle that never
/// worked. Both are red, and neither is a count.
///
/// ⚠ [`SELF`] is excluded, and that exclusion is the whole reason this works: this file quotes all
/// four spellings as data, so it matches every one of them and would keep any dead needle looking
/// alive. It stays in the population for coverage — it really does read the tree — and out of this
/// tally.
#[test]
fn every_spelling_of_reaching_the_tree_is_load_bearing() {
    let crates = workspace_root().join("crates");
    let mut carried = [0usize; REACHES.len()];
    let dirs = std::fs::read_dir(&crates)
        .unwrap_or_else(|e| panic!("read {}: {e}", crates.display()))
        .map(|e| e.expect("a directory entry under crates/").path())
        .collect::<Vec<PathBuf>>();
    for krate in dirs {
        let Ok(files) = std::fs::read_dir(krate.join("tests")) else {
            continue;
        };
        for file in files {
            let path = file
                .expect("a directory entry under a crate's tests/")
                .path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs")
                || path.file_stem().and_then(|s| s.to_str()) == Some(SELF)
            {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            for at in spellings_matching(&text) {
                carried[at] += 1;
            }
        }
    }

    for (at, (needle, also)) in REACHES.iter().enumerate() {
        assert!(
            carried[at] > 0,
            "⛔⛔⛔⛔⛔ REGISTER ITEM 784: the spelling {needle:?}{} matches no test in this \
             workspace, so it contributes nothing to the population the hooks' lane is checked \
             against — a needle that never worked, or a rename nobody followed. Either way the \
             lane is being justified by a vocabulary that is a quarter dead, and the tally is \
             {carried:?}",
            match also {
                Some(second) => format!(" (with {second:?})"),
                None => String::new(),
            },
        );
    }
}
