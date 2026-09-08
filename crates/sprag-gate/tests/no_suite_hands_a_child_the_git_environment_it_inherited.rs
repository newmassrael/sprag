//! **NO TEST OF THIS CRATE NAMES `git` FOR ITSELF** — register item 965.
//!
//! # ⛔⛔⛔⛔⛔ What it costs, measured
//!
//! `.githooks/pre-commit` runs `cargo test -p sprag-gate`. `git commit -- <pathspec>` — a PARTIAL
//! commit — builds a TEMPORARY index, hands its hooks an **absolute** `GIT_INDEX_FILE` naming it,
//! and commits whatever that file holds when the hooks return. So every child this suite starts
//! inherits the index git is about to commit, and that variable outranks `Command::current_dir`,
//! `git -C` and repository discovery all three.
//!
//! Measured 2026-09-08: `hooks_judge_the_bytes_being_published` builds twenty-one sandbox
//! repositories and stages files in each. Run under an inherited index, its `git add` calls went
//! into the CALLER's index and its parallel cases collided on the caller's `index.lock` — git
//! answering *"another git process seems to be running"* about the operator's own repository. Where
//! a sandbox's blobs happened to exist in the real object store the outer commit then SUCCEEDED and
//! published the sandbox's content; four such commits reached `main`.
//!
//! # ⚠⚠ Why a ratchet rather than the two call sites that did it
//!
//! [`sprag_gate::ambient::git_in`] fixes any call site it is applied to, and nothing makes the NEXT
//! call site take it. The failure a missed one brings is not a red in this crate: it is a
//! contaminated commit in somebody's repository, arriving only when a person types a pathspec —
//! which is why the class went unseen long enough to publish four commits. So the rule is over the
//! TEXT of every test this crate has, and a new one gets it without anybody remembering.
//!
//! # ⚠⚠⚠ The boundary, stated rather than implied
//!
//! **`crates/sprag-gate/tests/` only.** That is what `pre-commit` runs, which is what makes an
//! inherited index reachable at all; it is not a claim about `src/bin/north-star.rs`, which is a
//! program the loop runs from a person's own shell and SHOULD answer about the repository that
//! shell is standing in. Other crates' suites spawn `git` too and are not run by the commit hook —
//! covering them would be a wider claim than anything here has measured.

use sprag_gate::sources::{Source, rust_sources};
use std::path::PathBuf;

/// How a Rust source names the program: `Command::new("git")`, however the caller spelled the path
/// to `Command` and however rustfmt broke the line.
///
/// ⚠ Read off [`Source::squeezed`] rather than off the raw text, because `Command::new(\n "git",\n
/// )` and `Command::new("git")` are the same call and rustfmt chooses between them by line width.
const NAMING_GIT: &str = "Command::new(\"git\")";

/// The one file allowed to name it — the constructor every other caller must come through.
const THE_ONE_PLACE: &str = "crates/sprag-gate/src/ambient.rs";

/// Every test source of this crate.
fn the_suites_pre_commit_runs() -> Vec<Source> {
    rust_sources()
        .into_iter()
        .filter(|source| source.file.starts_with("crates/sprag-gate/tests/"))
        .collect()
}

/// ⛔ **THE GATE.** No test of this crate spawns `git` except through [`sprag_gate::ambient::git_in`].
#[test]
fn no_test_of_this_crate_names_git_for_itself() {
    let suites = the_suites_pre_commit_runs();
    assert!(
        suites.len() > 5,
        "a scan that found only {} test sources in this crate is pointed at the wrong tree, and a \
         probe pointed at nothing must never read as clean",
        suites.len(),
    );
    let needle: String = NAMING_GIT.chars().filter(|c| !c.is_whitespace()).collect();
    let offenders: Vec<String> = suites
        .iter()
        .filter(|source| source.squeezed().contains(&needle))
        .map(|source| source.file.clone())
        .collect();
    assert!(
        offenders.is_empty(),
        "⛔ ITEM 965: a test of this crate builds a `git` child for itself. `pre-commit` runs this \
         suite, so under `git commit -- <pathspec>` that child inherits an ABSOLUTE \
         `GIT_INDEX_FILE` naming the index git is about to commit — and it outranks `current_dir` \
         and `git -C`, so a sandbox's `git add` lands in the operator's repository. Build it with \
         `sprag_gate::ambient::git_in(<dir>)` instead, which cuts the inherited git environment. \
         Found in: {offenders:?}",
    );
}

/// ⚠⚠ **AND THE CONSTRUCTOR IS ACTUALLY REACHED** — the arm that stops the gate above from being
/// green because this crate stopped running `git` at all.
///
/// A rule whose population has emptied is a rule that passes by reading nothing, which is the shape
/// this repository has paid for more than once. Two facts are asserted, and they are not one: the
/// one place still names `git`, and the suites still come through it.
#[test]
fn the_one_place_exists_and_the_suites_come_through_it() {
    let sources = rust_sources();
    let one = sources
        .iter()
        .find(|source| source.file == THE_ONE_PLACE)
        .unwrap_or_else(|| {
            panic!("{THE_ONE_PLACE} is where the decision lives, and this gate points at it")
        });
    let needle: String = NAMING_GIT.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(
        one.squeezed().contains(&needle),
        "⛔ {THE_ONE_PLACE} no longer names `git`, so the gate above forbids something nothing \
         does and every suite could be spawning git by another name entirely",
    );
    let users: Vec<String> = the_suites_pre_commit_runs()
        .iter()
        .filter(|source| source.squeezed().contains("ambient::git_in("))
        .map(|source| source.file.clone())
        .collect();
    assert!(
        !users.is_empty(),
        "⛔ no test of this crate calls `ambient::git_in`, so the rule above is a rule over an \
         empty population and passes by reading nothing",
    );
}

/// ⛔⛔⛔ **AND THE CUT REACHES THE CHILD** — the arm that makes the two rules above measurements
/// rather than an agreement about where to type a function name.
///
/// `git rev-parse --absolute-git-dir` answers `GIT_DIR` when the child has one and discovers from
/// the directory when it does not, so it reports which of the two the child was actually given.
///
/// ⚠⚠ THE CONTROL IS FIRST, AND IT IS THE SAME COMMAND. A child carrying the variable must answer
/// `/nowhere/.git`; without that, a git that ignored it — or a typo in the name — would make the
/// second assertion pass while measuring nothing. The pair is what tells *the cut worked* from
/// *there was nothing to cut*.
///
/// ⚠ The variable is set ON THE CHILD, never on this process. `sprag-gate`'s cases run on threads
/// of one process, so a `set_var` here would be a `set_var` for every case beside it — which is why
/// `ambient::cut` takes its names instead of reading them.
///
/// ⚠ `GIT_DIR` naming a REAL second repository, not a path that does not exist: git refuses a
/// `GIT_DIR` it cannot open, and a refusal on stderr is not the same measurement as a child
/// answering about the wrong repository. The wrong repository is the failure this is about.
///
/// ⚠ `GIT_DIR` and not `GIT_INDEX_FILE`, because it is the one whose effect a child can be asked
/// about in a single command. The population is the namespace either way, and
/// `ambient::is_git_environment` is where that classification is driven.
#[test]
fn a_child_does_not_receive_what_the_cut_took_away() {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("ambient-cut");
    let _ = std::fs::remove_dir_all(&root);
    let here = root.join("here");
    let elsewhere = root.join("elsewhere");
    for dir in [&here, &elsewhere] {
        std::fs::create_dir_all(dir).expect("the scratch must be creatable");
        let made = sprag_gate::ambient::git_in(dir)
            .args(["init", "-q", "-b", "main", "."])
            .output()
            .expect("git must be runnable");
        assert!(
            made.status.success(),
            "the scratch repository must be creatable: {}",
            String::from_utf8_lossy(&made.stderr),
        );
    }
    let other = elsewhere.join(".git");
    let asked = ["rev-parse", "--absolute-git-dir"];

    let mut carrying = sprag_gate::ambient::git_in(&here);
    carrying.args(asked).env("GIT_DIR", &other);
    let uncut = carrying.output().expect("git must be runnable");

    let mut cut_of_it = sprag_gate::ambient::git_in(&here);
    cut_of_it.args(asked).env("GIT_DIR", &other);
    sprag_gate::ambient::cut(&mut cut_of_it, &["GIT_DIR".into()]);
    let cut = cut_of_it.output().expect("git must be runnable");

    let _ = std::fs::remove_dir_all(&root);
    assert!(
        String::from_utf8_lossy(&uncut.stdout).contains("elsewhere"),
        "⛔ THE CONTROL FAILED: a GIT_DIR carried by a child did not decide where it stood, so \
         this machine cannot show the difference and the arm below is green for free. git said: \
         {}{}",
        String::from_utf8_lossy(&uncut.stdout),
        String::from_utf8_lossy(&uncut.stderr),
    );
    assert!(
        String::from_utf8_lossy(&cut.stdout).contains("here"),
        "⛔ ITEM 965: a child of a CUT command still answered about the repository the variable \
         named rather than the directory it was handed, so the cut removes nothing and every \
         sandbox in this suite is one pathspec away from writing the operator's index. git said: \
         {}{}",
        String::from_utf8_lossy(&cut.stdout),
        String::from_utf8_lossy(&cut.stderr),
    );
}
