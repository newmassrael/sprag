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
//! **`crates/sprag-gate/tests/` only**, for the Rust half. That is what `pre-commit` runs, which is
//! what makes an inherited index reachable at all; it is not a claim about `src/bin/north-star.rs`,
//! which is a program the loop runs from a person's own shell and SHOULD answer about the
//! repository that shell is standing in. Other crates' suites spawn `git` too and are not run by
//! the commit hook — covering them would be a wider claim than anything here has measured.
//!
//! # ⛔⛔⛔⛔⛔ AND THE HOOKS THEMSELVES, WHICH IS THE HALF THAT WENT MISSING — register item 1082
//!
//! The paragraph above says *a rule kept in one test file is a rule the next one does not get*, and
//! the identical sentence was true one layer down and nobody wrote it. `.githooks/content-gate.sh`
//! built the constructor (`index_mirror_git`, item 1017) and **no ratchet made the next call site
//! take it** — so when item 1014 put two `"${BX}"` lanes inside the mirror, every routed commit in
//! this tree died with `fatal: .git/index: index file open failed: Not a directory` and the
//! operating answer became *type `env -u BX`*.
//!
//! ⚠⚠ **AND THAT LOUD FAILURE IS THE LUCKY HALF.** Measured 2026-09-12 against a linked worktree
//! made to differ from its main repository: a child there carrying a RELATIVE `GIT_INDEX_FILE` (a
//! plain commit) dies rc=128, and one carrying an ABSOLUTE one (`git commit -- <pathspec>`, which
//! is what this file's own opening paragraph is about) answers **rc=0 with the operator's index** —
//! a file staged after the mirror was cut present, one the mirror holds absent. Silent, plausible
//! and wrong, which is exactly the shape that published four commits above.
//!
//! ⚠ So the hook rule is `.githooks/` only, and it is about ENTERING THE MIRROR rather than about
//! naming `git`: a hook's own git calls go through `index_mirror_git`, and everything a hook hands
//! to a CHILD goes through `enter_the_mirror`.

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

/// Where the hooks live, and the two names the rule below is written in.
const HOOKS_DIR: &str = ".githooks";
/// The only way a hook may stand in the mirror — see `.githooks/content-gate.sh`.
const THE_ONLY_WAY_IN: &str = "enter_the_mirror";

/// Every shell file under `.githooks/`, as text, newest-sorted for a stable message.
fn hook_sources() -> Vec<(String, String)> {
    let dir = sprag_gate::sources::workspace_root().join(HOOKS_DIR);
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|why| panic!("{} must be readable: {why}", dir.display()))
        .map(|entry| entry.expect("a hook entry").path())
        .filter(|path| path.is_file())
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|path| {
            let name = format!(
                "{HOOKS_DIR}/{}",
                path.file_name().unwrap_or_default().to_string_lossy()
            );
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|why| panic!("{name} must be text: {why}"));
            (name, text)
        })
        .collect()
}

/// ⛔ **THE HOOK LAYER'S RATCHET** — register item 1082, and item 965's own sentence applied where
/// it had not been: *"nothing makes the NEXT call site take it."*
///
/// The rule is over the TEXT of every hook, for the reason the header gives: a missed site's
/// failure is not a red here, it is a refused commit on somebody's machine — or, on a partial
/// commit, a child quietly answering about the operator's index.
///
/// ⛔⛔ **THE RULE IS OVER THE DIRECTORY CHANGE, NOT OVER ONE SPELLING OF IT** — and the first
/// draft of this arm was over the spelling, which is the hole item 1084 is about, met again one
/// round later in my own gate. `cd "${mirror}"`, `cd $mirror` and `pushd` all reach the same
/// place, and a rule keyed on the quoting would have called each of them clean.
///
/// ⚠ Measured before this was tightened: the hooks contain no `git -C "$mirror"` outside
/// [`index_mirror_git`] and no second spelling of the change — so the population is complete today
/// and this widening costs nothing. It is what the NEXT edit will meet.
///
/// ⚠⚠ The constructor's own body is not an offender by accident but by construction: it takes its
/// destination as `$1`, so the one line in this repository that may `cd` there does not name the
/// mirror at all.
///
/// # ⛔⛔⛔⛔⛔ And "is this a directory change" is asked of the WORDS — register item 1085
///
/// This arm used to find a `cd ` at the start of a line or behind `(`, `; `, `&& `, `|| `, `then `,
/// `else `, `do ` or `{ `, and it excused every line that so much as mentioned the door. That is a
/// list of leads whose default is *not a directory change*: **measured 2026-09-13**, replacing the
/// ratchet lane's door with `if cd "$mirror"; then :; fi` left this file at `7 passed`. So the line
/// is split by [`sprag_gate::shell::simple_commands`] and ANY `cd` or `pushd` word on a line whose
/// words name the mirror is an offender — the command itself, or an operand of `builtin`,
/// `command` or anything else, where whether it changes directory is not this arm's to guess. The
/// door is excused by not being a directory change, not by being mentioned.
#[test]
fn no_hook_enters_the_mirror_without_leaving_the_commits_index_behind() {
    let hooks = hook_sources();
    assert!(
        hooks.len() > 5,
        "a scan that found only {} file(s) under {HOOKS_DIR} is pointed at the wrong tree, and a \
         probe pointed at nothing must never read as clean",
        hooks.len(),
    );
    // ⚠ Split so this arm's own source does not answer its own question: spelled whole, the scan
    // would find the line you are reading.
    let names_it = concat!("mir", "ror");
    let offenders: Vec<String> = hooks
        .iter()
        .flat_map(|(name, text)| {
            text.lines()
                .enumerate()
                .filter(|(_, line)| {
                    let code = line.trim();
                    if code.starts_with('#') {
                        return false;
                    }
                    let commands = sprag_gate::shell::simple_commands(code);
                    let mut words = commands.iter().flatten();
                    let enters = words
                        .clone()
                        .any(|word| ["cd", "pushd"].contains(&word.text.as_str()));
                    enters && words.any(|word| word.raw.contains(names_it))
                })
                .map(move |(index, _)| format!("{name}:{}", index + 1))
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "⛔ ITEM 1082: a hook stands in the mirror without going through `{THE_ONLY_WAY_IN}`. The \
         mirror is a LINKED WORKTREE, so the `GIT_INDEX_FILE` a commit exported stops being true \
         there — relative on a plain commit (the child dies rc=128, which refused every routed \
         commit in this tree) and an absolute `next-index` lock on a partial one (the child \
         answers rc=0 about the OPERATOR's index, which is item 965's four published commits one \
         layer down). Use `{THE_ONLY_WAY_IN} \"$mirror\"`. Found at: {offenders:?}",
    );
}

/// ⚠⚠ **AND THAT CONSTRUCTOR EXISTS AND IS REACHED** — the arm that stops the ratchet above from
/// passing because the hooks stopped entering the mirror at all.
#[test]
fn the_only_way_into_the_mirror_exists_and_the_hooks_come_through_it() {
    let hooks = hook_sources();
    let defines = hooks
        .iter()
        .any(|(_, text)| text.contains(&format!("{THE_ONLY_WAY_IN}() {{")));
    assert!(
        defines,
        "⛔ no hook defines `{THE_ONLY_WAY_IN}`, so the rule above forbids something nothing does \
         and a hook could be standing in the mirror by any other spelling",
    );
    let callers: Vec<&String> = hooks
        .iter()
        .filter(|(_, text)| {
            text.lines().any(|line| {
                let code = line.trim();
                !code.starts_with('#')
                    && code.contains(THE_ONLY_WAY_IN)
                    && !code.contains(&format!("{THE_ONLY_WAY_IN}() {{"))
            })
        })
        .map(|(name, _)| name)
        .collect();
    assert!(
        callers.len() >= 2,
        "⛔ only {} hook file(s) call `{THE_ONLY_WAY_IN}`. Both the format gate and the Rust lanes \
         stand in the mirror, so a population of fewer than two is a rule that has stopped reading \
         one of them: {callers:?}",
        callers.len(),
    );
}

/// ⛔⛔⛔⛔⛔ **AND THE DOOR ACTUALLY SHUTS** — register item 1082, and the arm without which the
/// two above check a NAME rather than what the name does.
///
/// # ⛔⛔⛔⛔⛔ Measured by mutation, on the gate I had just written
///
/// `enter_the_mirror` was reduced to a bare `cd` — the whole repair undone, the constructor still
/// called from all four sites — and `cargo test -p sprag-gate` came back **rc=0**. The ratchet
/// above was green because every call site still SAYS `enter_the_mirror`, and
/// `hooks_judge_the_bytes_being_published` was green because the eight cases that notice a cut
/// reach it through `index_mirror_git`, never through this door. So the fix for item 1082 was held
/// by a spelling, which is the escape hatch this repository calls rule 6 — met, one round after
/// registering it against somebody else's gate, in my own.
///
/// ⚠ It drives the LIBRARY, not a copy of it: the child sources `.githooks/content-gate.sh` and
/// calls the real function, so a repair that stops repairing is red here whatever it is spelled.
///
/// ⚠⚠ THE CONTROL IS FIRST AND IS THE SAME SHELL. A plain `cd` must let the pair through, or a
/// bash that dropped the environment by itself would make the arm below pass while measuring
/// nothing.
///
/// ⚠ The variables are set ON THE CHILD, this file's rule throughout.
#[test]
fn entering_the_mirror_leaves_the_commits_index_behind() {
    let root = sprag_gate::sources::workspace_root();
    let into = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("entering-the-mirror");
    let _ = std::fs::remove_dir_all(&into);
    std::fs::create_dir_all(&into).expect("the scratch must be creatable");

    // `$1` is the workspace root, `$2` where to stand, `$3` whether to use the door.
    let probe = r#"
        set -euo pipefail
        . "$1/.githooks/content-gate.sh"
        if [ "$3" = door ]; then
            enter_the_mirror "$2"
        else
            cd "$2"
        fi
        printf 'INDEX=[%s]\n' "${GIT_INDEX_FILE-<gone>}"
        printf 'PREFIX=[%s]\n' "${GIT_PREFIX-<gone>}"
        printf 'WHERE=[%s]\n' "$(pwd -P)"
    "#;
    let ask = |how: &str| {
        let done = std::process::Command::new("bash")
            .args([
                "-c",
                probe,
                "probe",
                root.to_str().expect("a utf-8 workspace root"),
                into.to_str().expect("a utf-8 scratch path"),
                how,
            ])
            .env("GIT_INDEX_FILE", ".git/index")
            .env("GIT_PREFIX", "crates/")
            .output()
            .expect("bash must be runnable");
        assert!(
            done.status.success(),
            "the probe must run ({how}): {}{}",
            String::from_utf8_lossy(&done.stdout),
            String::from_utf8_lossy(&done.stderr),
        );
        String::from_utf8_lossy(&done.stdout).into_owned()
    };

    let plain = ask("plain");
    let door = ask("door");
    let _ = std::fs::remove_dir_all(&into);

    assert!(
        plain.contains("INDEX=[.git/index]") && plain.contains("PREFIX=[crates/]"),
        "⛔ THE CONTROL FAILED: a plain `cd` was supposed to carry the commit's index across, so \
         this shell cannot show the difference and the assertion below is green for free. The \
         probe said: {plain:?}",
    );
    assert!(
        door.contains("INDEX=[<gone>]") && door.contains("PREFIX=[<gone>]"),
        "⛔ ITEM 1082: `enter_the_mirror` stood in the target and the commit's index came with it. \
         The four call sites all NAME this function, so nothing else in this repository would \
         notice — the ratchet reads the name and `hooks_judge_the_bytes_being_published` reaches \
         the cut only through `index_mirror_git`. Driven: with the cut removed from this function \
         the whole of `sprag-gate` was rc=0. The probe said: {door:?}",
    );
    assert!(
        door.contains("WHERE=") && !door.contains("WHERE=[]"),
        "⛔ and it must still ENTER: a door that cuts the environment and does not move is the \
         other half of this function's job. The probe said: {door:?}",
    );
}

/// ⛔⛔⛔ **AND THE BOUNDARY IS WORTH CROSSING — BOTH FAILURES, DRIVEN** — register item 1082.
///
/// The two arms above are about where a name is typed. This is the measurement they stand on, and
/// it is the one that says the LOUD failure this repository met is the lucky half.
///
/// A real linked worktree is built whose tree deliberately differs from its main repository's
/// index — a file staged after the worktree was cut, and a file the worktree holds that the index
/// no longer does — and `git ls-files` is asked from inside it three ways:
///
///   * carrying a RELATIVE `GIT_INDEX_FILE`, which is what a plain `git commit` exports: dies.
///   * carrying an ABSOLUTE one, which is what `git commit -- <pathspec>` exports: **succeeds, and
///     answers about the operator's index.** No error, no clue, wrong list.
///   * with the pair left behind: answers about the tree it is standing in.
///
/// ⚠ The variables are set ON THE CHILD, never on this process — this file's own rule three arms
/// up, because `sprag-gate`'s cases share one process.
#[test]
fn a_child_in_the_mirror_answers_about_the_mirror_only_once_the_index_is_left_behind() {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mirror-index");
    let _ = std::fs::remove_dir_all(&root);
    let main = root.join("main");
    let mirror = root.join("mirror");
    std::fs::create_dir_all(&main).expect("the scratch must be creatable");

    let git = |at: &std::path::Path, args: &[&str]| {
        let done = sprag_gate::ambient::git_in(at)
            .args(args)
            .output()
            .expect("git must be runnable");
        assert!(
            done.status.success(),
            "the scratch must be buildable ({args:?}): {}",
            String::from_utf8_lossy(&done.stderr),
        );
        String::from_utf8_lossy(&done.stdout).trim().to_owned()
    };
    git(&main, &["init", "-q", "-b", "main", "."]);
    git(&main, &["config", "user.email", "probe@invalid"]);
    git(&main, &["config", "user.name", "probe"]);
    std::fs::write(main.join("SHARED.txt"), b"shared").expect("a file to commit");
    git(&main, &["add", "SHARED.txt"]);
    git(&main, &["commit", "-qm", "base"]);
    std::fs::write(main.join("IN_MIRROR_ONLY.txt"), b"only there").expect("a file to commit");
    git(&main, &["add", "IN_MIRROR_ONLY.txt"]);
    git(&main, &["commit", "-qm", "second"]);
    let cut_at = git(&main, &["rev-parse", "HEAD"]);
    // ⚠ The two indexes must DIFFER, or every answer below is the same answer and the arm proves
    // nothing — item 196's two writers are exactly this state in the real tree.
    git(&main, &["rm", "-q", "--cached", "IN_MIRROR_ONLY.txt"]);
    std::fs::write(main.join("STAGED_LATER.txt"), b"after the cut").expect("a file to stage");
    git(&main, &["add", "STAGED_LATER.txt"]);
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            mirror.to_str().expect("a utf-8 scratch path"),
            &cut_at,
        ],
    );

    let asked = |index: Option<&str>| {
        let mut run = sprag_gate::ambient::git_in(&mirror);
        run.args(["ls-files"]);
        if let Some(named) = index {
            run.env("GIT_INDEX_FILE", named);
        }
        let done = run.output().expect("git must be runnable");
        (
            done.status.success(),
            String::from_utf8_lossy(&done.stdout).into_owned(),
        )
    };
    let (relative_ok, relative_said) = asked(Some(".git/index"));
    let absolute = main.join(".git/index");
    let (absolute_ok, absolute_said) =
        asked(Some(absolute.to_str().expect("a utf-8 scratch path")));
    let (left_ok, left_said) = asked(None);
    let _ = std::fs::remove_dir_all(&root);

    assert!(
        !relative_ok,
        "⛔ THE LOUD CONTROL FAILED: a child in a linked worktree carrying a RELATIVE \
         `GIT_INDEX_FILE` succeeded, so this machine cannot show the failure that refused every \
         routed commit in this tree and the arms below measure nothing. It said: {relative_said:?}",
    );
    assert!(
        absolute_ok && absolute_said.contains("STAGED_LATER.txt"),
        "⛔ THE SILENT CONTROL FAILED: a child carrying an ABSOLUTE `GIT_INDEX_FILE` was supposed \
         to answer about the OPERATOR's index — that is the whole danger — and it did not, so the \
         claim in this file's header is not true of this git. ok={absolute_ok}, it said: \
         {absolute_said:?}",
    );
    assert!(
        !absolute_said.contains("IN_MIRROR_ONLY.txt"),
        "⛔ the silent control is not discriminating: the operator's index and the mirror's tree \
         must differ, or *answered about the wrong one* cannot be told from *answered correctly*. \
         It said: {absolute_said:?}",
    );
    assert!(
        left_ok
            && left_said.contains("IN_MIRROR_ONLY.txt")
            && !left_said.contains("STAGED_LATER.txt"),
        "⛔ ITEM 1082: with the commit's index left behind, a child in the mirror must answer about \
         the MIRROR — the tree the gates were told to judge. ok={left_ok}, it said: {left_said:?}",
    );
}
