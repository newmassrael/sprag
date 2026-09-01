//! ⛔⛔⛔⛔⛔ **A SELFTEST NOBODY RUNS IS GREEN FOREVER** — register item 799, found while paying
//! item 793, and already written down as item 776's own unmeasured residue.
//!
//! ⚠ The number was re-MEASURED rather than counted, and it moved while this file was being
//! written: a first draft said 796, and by the time the register was read again 796, 797 and 798
//! belonged to another session. This register has more than one writer, and `max + 1` is a
//! measurement, not a memory.
//!
//! # What was measured, and when
//!
//! `.githooks/hosted-read.sh` opens with *"What IS gated is this file's own arithmetic:
//! `hosted_read_selftest` drives every arm"*. That sentence is true about what the FUNCTION does
//! and false about anything running it. Measured 2026-09-01, `git grep -- --selftest` over the
//! whole tracked tree: **11 hits, and not one of them executes anything** — comments, the `case`
//! arm that dispatches it, and a usage string. `.githooks/ident-gate.sh` declares one too, with the
//! same result.
//!
//! So two shell harnesses carrying **52 arms between them** ran only when a person typed the
//! command. The round that found this had just ADDED six arms to one of them, which is the shape
//! worth naming: the repair for item 793 would have shipped as a suite nothing runs, and its own
//! mutation proof would have rested entirely on the author's keyboard.
//!
//! # ⚠⚠ WHY A GATE THAT EXECUTES, RATHER THAN ONE THAT READS
//!
//! A gate asserting *some file mentions `--selftest`* is the defect one level up: it would pass on
//! a `case` arm that dispatches to a function somebody deleted. This one RUNS each declared
//! selftest and reads its status, the same way
//! [`hooks_enforce_what_they_check`](../hooks_enforce_what_they_check.rs) drives `commit-msg`
//! rather than re-implementing its rules. A hook's whole surface is an exit code; so is a
//! selftest's.
//!
//! # ⚠ AND WHY IT WALKS
//!
//! The population is a directory walk, not a list, for the reason every gate in this crate walks:
//! a list decides alone which files are looked at, and the one it leaves out is the one nobody is
//! watching. A `.githooks/` script that grows a selftest tomorrow is driven here without anybody
//! remembering to add it.

use sprag_gate::doubles::Doubles;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The tree this gate is part of — `crates/sprag-gate/` is two levels down from it.
fn repo_root() -> PathBuf {
    [env!("CARGO_MANIFEST_DIR"), "..", ".."].iter().collect()
}

/// The lines that are CODE: comments carry this repository's reasoning, and that reasoning names
/// `--selftest` while explaining it. `.githooks/pre-push` mentions the flag in a comment and has no
/// arm for it, so a whole-file grep would send this gate to run a hook with an argument it does not
/// understand.
///
/// ⚠ `#` and not a parser: a `#` inside a quoted string is cut too. That is this scan's stated
/// limit, the same one `hooks_cannot_pass_in_silence` writes down — and it errs toward seeing LESS
/// code, which for the companion test below is the direction that costs a red rather than a pass.
fn code_of(text: &str) -> String {
    text.lines()
        .map(|line| match line.find('#') {
            Some(at) => &line[..at],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every `.githooks/` script whose CODE declares a `--selftest`, as `(name, path)`.
fn declared_selftests() -> Vec<(String, PathBuf)> {
    let hooks = repo_root().join(".githooks");
    let entries = std::fs::read_dir(&hooks)
        .unwrap_or_else(|why| panic!("{} must be readable: {why}", hooks.display()));
    let mut found: Vec<(String, PathBuf)> = entries
        .map(|entry| entry.expect("a directory entry").path())
        .filter(|path| path.is_file())
        .filter(|path| {
            std::fs::read_to_string(path)
                .map(|text| code_of(&text).contains("--selftest"))
                .unwrap_or(false)
        })
        .map(|path| {
            let name = path
                .file_name()
                .expect("a file has a name")
                .to_string_lossy()
                .into_owned();
            (name, path)
        })
        .collect();
    found.sort();
    found
}

/// How many arms a selftest says it drove, from its own summary line, or [`None`] where it never
/// said.
///
/// ⛔⛔⛔⛔⛔ **RULE 6 LIVES HERE, AND IT IS A FUNCTION BECAUSE IT HAD TO BE DRIVABLE.** A status of
/// 0 cannot tell *every arm passed* from *there were no arms*, so the gate below requires the
/// harness's own count as well. Written inline, that requirement was a DEAD CONTROL and a mutation
/// measured it as one: deleting the check left the gate GREEN, because every selftest in this tree
/// does print a count and nothing could drive the case the check exists for.
///
/// ⇒ Split out and driven directly by
/// [`a_run_that_named_no_arms_is_not_a_pass`](a_run_that_named_no_arms_is_not_a_pass). The general
/// shape, learned twice in this repository now: when the real population cannot produce the case a
/// guard is for, do not assert around the guard — extract the decision and hand it the case.
fn arms_reported(said: &str) -> Option<usize> {
    said.lines()
        .find_map(|line| {
            line.split_once('/')
                .filter(|_| line.contains("arm(s) pass"))
        })
        .and_then(|(_, tail)| tail.split_whitespace().next())
        .and_then(|count| count.parse::<usize>().ok())
}

/// What is wrong with one selftest's run, or [`None`] where nothing is — the whole verdict, as a
/// pure function of what the run said.
///
/// ⛔⛔⛔⛔⛔ **THE SECOND ATTEMPT AT KILLING A DEAD CONTROL, BECAUSE THE FIRST ONE MOVED IT.**
/// Extracting [`arms_reported`] made the READER drivable and left the DECISION unobserved: a
/// mutation replacing the gate's `Some(n) if n > 0 => {}` with `_ => {}` was still GREEN, because
/// no `.githooks/` selftest can produce a run with no count for the loop to refuse.
///
/// ⇒ The decision comes here, where a test can hand it the shapes the real population never
/// produces. The impure part — spawning the process — stays in the gate, which is the same seam
/// this workspace uses for every env-dependent policy: read at one place, decide in a function of
/// what was read.
///
/// ⛔⛔⛔⛔⛔ **AND THE THIRD ATTEMPT, BECAUSE THE SECOND ONE KILLED ONLY HALF OF IT.** Measured
/// 2026-09-01 in the round that shipped this file: with the three shapes below driven, deleting the
/// requirement WHOLE went red — but a mutation deleting only the boundary, `Some(n) if n > 0` to
/// `Some(_n)`, was still **GREEN**. Nothing handed this function a run that *said* zero, so the
/// `> 0` was a dead control of its own while the arm around it was alive.
///
/// ⇒ ⚠⚠ The cause was the fold, not the missing case: *never said* and *said zero* shared one
/// `_` arm and one sentence, which is the exact covering the 776 family is about — two states, one
/// word, and the difference is what nobody looks at again. They are separate arms now, they say
/// different things, and [`the_verdict_refuses_the_shapes_the_population_cannot_produce`] hands
/// this function BOTH.
fn refusal_for(name: &str, exited: Option<i32>, ok: bool, said: &str) -> Option<String> {
    if !ok {
        return Some(format!(
            "{name}: exited {exited:?}, saying:\n{}",
            said.trim_end(),
        ));
    }
    // ⛔⛔ RULE 6. A selftest that drove no arm is not a selftest that passed, and a status of 0
    // cannot tell those apart — the count is the harness's own word for how much it did.
    match arms_reported(said) {
        Some(n) if n > 0 => None,
        // ⛔ IT SAID ZERO. A harness whose arms all vanished still prints its summary line, so this
        // is a run that answered the question honestly with the answer this item is about.
        Some(_) => Some(format!(
            "{name}: exited 0 having driven ZERO arm(s) by its own count — a harness whose arms \
             all vanished still prints a summary, and its 0 is exactly the green nothing else \
             would catch, saying:\n{}",
            said.trim_end(),
        )),
        // ⛔ IT NEVER SAID. A different fact and a different remedy: this one cannot be believed
        // either way, and folding it into the line above would hand a reader one act for two.
        None => Some(format!(
            "{name}: exited 0 but never said how many arms it drove — a harness that drove none \
             exits 0 too, saying:\n{}",
            said.trim_end(),
        )),
    }
}

/// One environment a selftest is driven under: a name for the refusal message, and the variables
/// that make it.
type Environment = (String, Vec<(String, String)>);

/// A scratch directory of this suite's own, under the one cargo already hands every integration
/// test.
///
/// ⛔⛔⛔ NOT `std::env::temp_dir()`, and not `sprag_scratch::scratch_root()` either. The first is
/// what register item 794's ratchet counts — this file added two call sites and the ratchet said so
/// by name, 165 against 163 recorded. The second is the fix everywhere else and is unavailable
/// HERE: `sprag-gate` declares no dependencies on purpose, because a gate that stands outside the
/// suite must not be able to fail when the product fails to compile.
///
/// ⇒ `CARGO_TARGET_TMPDIR` satisfies both. Cargo guarantees it for integration tests, it lives
/// under `target/` rather than in the tree or in the user's state, and it is a compile-time
/// constant rather than a call the ratchet is counting.
fn scratch_under(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
}

/// Whether THIS machine's `sed` reads `\|` inside a basic regular expression as alternation.
///
/// ⛔⛔ It is the GNU extension that voided a marker on macOS: BSD `sed` reads the `\|` as a
/// literal, `loop_read_accounted` matched nothing, and every baseline and every `--seen` became
/// invisible while the file still exited 0 on Linux. The question is asked of the machine rather
/// than assumed from the platform name, because what matters is the behaviour.
fn sed_takes_gnu_alternation() -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(r"printf 'baseline x\n' | sed -n 's/^\(baseline\|read\) //p'")
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim() == "x")
        .unwrap_or(false)
}

/// The absolute path of the real `sed`, so a shim can call it without calling itself.
fn real_sed() -> Option<String> {
    Command::new("sh")
        .arg("-c")
        .arg("command -v sed")
        .output()
        .ok()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned())
        .filter(|found| !found.is_empty())
}

/// Why the POSIX-sed environment is, or is not, in the list on this machine -- a sentence, always,
/// because a measurement that quietly did not happen is the shape this whole file is about.
fn posix_sed_note(sed_is_gnu: bool, shim_built: bool) -> String {
    match (sed_is_gnu, shim_built) {
        (true, true) => "this machine's sed takes the GNU alternation, so a strict one was \
                         injected and both readings are covered"
            .to_owned(),
        (true, false) => "this machine's sed takes the GNU alternation and NO strict one could be \
                          built, so the BSD reading went unmeasured here"
            .to_owned(),
        (false, _) => {
            "this machine's sed already refuses the GNU alternation, so the plain run IS \
                       that measurement"
                .to_owned()
        }
    }
}

/// The environments every declared selftest is driven under, and the sentence about the last one.
///
/// ⛔⛔⛔⛔⛔ **TWO OF THESE REDDENED CI, AND NEITHER IS THE PLATFORM'S NAME.** Both macOS failures
/// this gate found were behaviours a Linux runner can be made to have:
///
/// * `mktemp -d` answers under `/var`, a symlink to `/private/var`, so a guard comparing a logical
///   path to git's physical one refused every run;
/// * `sed` is BSD, so `\|` in a BRE is a literal and a marker silently matched nothing.
///
/// ⇒ Injected rather than waited for. A repository whose macOS job runs once per push cannot
/// afford to learn these one round at a time, and the injection makes the LINUX job catch them.
///
/// ⚠ It does not claim to cover BSD. It covers the two differences that have actually cost this
/// repository a red, and it says which it covered.
fn environments(scratch: &Path) -> (Vec<Environment>, String) {
    let mut envs: Vec<Environment> = vec![("as configured".to_owned(), Vec::new())];

    let real = scratch.join("tmp-real");
    std::fs::create_dir_all(&real).expect("the scratch TMPDIR target must be creatable");
    let link = scratch.join("tmp-link");
    std::os::unix::fs::symlink(&real, &link).expect("a symlink must be creatable in the scratch");
    envs.push((
        "a symlinked TMPDIR".to_owned(),
        vec![("TMPDIR".to_owned(), link.display().to_string())],
    ));

    let sed_is_gnu = sed_takes_gnu_alternation();
    // ⚠ The subject is only looked up when there is something to make strict — a machine whose own
    // `sed` already refuses the extension needs no double, and asking for one would read as though
    // the environment were missing rather than unnecessary.
    let strict_subject = if sed_is_gnu { real_sed() } else { None };
    let mut shim_built = false;
    if let Some(sed) = strict_subject {
        // ⛔ THE STRICT `sed` IS A TRACKED DOUBLE, NOT A FILE THIS SUITE WRITES — register item
        // 467, whose gate refused the first draft of this block by name: a program a process holds
        // open for writing cannot be executed, and this harness forks from threads. `program` also
        // checks the execute bit survived the checkout, so a staging failure reads as one instead
        // of as the hook refusing.
        let doubles = Doubles::of(env!("CARGO_MANIFEST_DIR")).set("declared-selftest");
        let _ = doubles.program("sed");
        envs.push((
            "a POSIX sed".to_owned(),
            vec![
                (
                    "PATH".to_owned(),
                    doubles.ahead_of_inherited().to_string_lossy().into_owned(),
                ),
                // ⚠ The double sits at the FRONT of that PATH, so it cannot look its own subject up
                // by name — it would exec itself. It is named instead.
                ("SPRAG_REAL_SED".to_owned(), sed),
            ],
        ));
        shim_built = true;
    }
    (envs, posix_sed_note(sed_is_gnu, shim_built))
}

/// ⛔ **THE GATE.** Every declared selftest runs here, under every environment, and every one of
/// them passes.
///
/// Two things are required and the second is why this is not merely a runner: the status must be
/// zero, AND the run must say how many arms it drove. A harness that exits 0 having driven nothing
/// is exactly the green this item is about, and it is indistinguishable from a real pass by status
/// alone.
#[test]
fn every_declared_selftest_runs_and_passes_here() {
    let declared = declared_selftests();
    let scratch = scratch_under(&format!("selftest-envs-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).expect("the scratch root must be creatable");
    let (envs, note) = environments(&scratch);
    let mut refused: Vec<String> = Vec::new();
    for (env_name, vars) in &envs {
        for (name, path) in &declared {
            let mut run = Command::new("bash");
            run.arg(path).arg("--selftest").current_dir(repo_root());
            for (key, value) in vars {
                run.env(key, value);
            }
            let run = run
                .output()
                .unwrap_or_else(|why| panic!("{name} --selftest must be runnable: {why}"));
            let said = format!(
                "{}{}",
                String::from_utf8_lossy(&run.stdout),
                String::from_utf8_lossy(&run.stderr),
            );
            let under = format!("{name} [under {env_name}]");
            if let Some(why) = refusal_for(&under, run.status.code(), run.status.success(), &said) {
                refused.push(why);
            }
        }
    }
    // ⚠ REMOVED BEFORE THE ASSERT, so a red does not also leave litter behind — register item 802
    // is open about a harness that grows a user's state directory without ever tidying it.
    let _ = std::fs::remove_dir_all(&scratch);
    assert!(
        refused.is_empty(),
        "⛔ ITEM 799: a `.githooks/` script declares a `--selftest` and it does not pass when this \
         suite runs it. Until this gate existed nothing executed either of them — 52 arms that ran \
         only when a person typed the command. Environments driven: {}; {note}:\n{}",
        envs.iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        refused.join("\n"),
    );
}

/// ⚠⚠⚠ **THE ENVIRONMENTS ARE NOT AN EMPTY LIST, AND THE ONE THAT CANNOT BE BUILT IS SAID.**
///
/// The gate above passes by finding nothing wrong, which a list of ZERO environments would also
/// do — and the symlinked TMPDIR is the one that caught a red, so its presence is asserted rather
/// than hoped for. The POSIX-sed one is conditional by construction: on a machine whose `sed` is
/// already strict, the plain run IS that measurement, and `posix_sed_note` has to say which case
/// this machine is rather than leaving the reader to guess.
#[test]
fn the_driven_environments_include_the_one_that_reddened_ci() {
    let scratch = scratch_under(&format!("env-probe-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).expect("the scratch root must be creatable");
    let (envs, note) = environments(&scratch);
    let names: Vec<&str> = envs.iter().map(|(name, _)| name.as_str()).collect();
    let _ = std::fs::remove_dir_all(&scratch);
    assert!(
        names.contains(&"as configured") && names.contains(&"a symlinked TMPDIR"),
        "the symlinked TMPDIR is the environment that caught a macOS red, and a gate driving \
         only the plain one would go back to learning it a round at a time. Found: {names:?}",
    );
    assert!(
        !note.is_empty(),
        "the POSIX-sed environment is conditional, so the reason it is or is not present must be \
         a sentence rather than a silence",
    );
    // ⛔ The note's three cases cannot all be reached on one machine, so they are driven directly —
    // the lesson `refusal_for` above had to learn twice.
    assert!(
        posix_sed_note(true, true).contains("both readings are covered"),
        "a machine with GNU sed and a shim covers both readings and must say so",
    );
    assert!(
        posix_sed_note(true, false).contains("went unmeasured"),
        "⛔ a machine with GNU sed and NO shim left the BSD reading UNMEASURED, and that is not \
         the same as covering it — an unclassified case is red here, not a pass",
    );
    assert!(
        posix_sed_note(false, false).contains("plain run IS that measurement"),
        "a machine whose sed is already strict needs no shim, and the note must say why",
    );
}

/// ⛔⛔ **THE CASES THE REAL POPULATION CANNOT PRODUCE, DRIVEN DIRECTLY** — the arm that stops
/// [`arms_reported`]'s requirement from being decoration.
///
/// Both selftests in this tree print a NON-ZERO count on every run, so the gate above can never
/// reach either branch that refuses one. That is precisely why a mutation deleting them left it
/// green, and why the decision is a function taking its input rather than a shape asserted around a
/// process nobody can make misbehave.
///
/// ⚠⚠ **FOUR SHAPES, AND THE LAST TWO ARE NOT ONE SHAPE.** *It said zero* and *it never said* are
/// different facts about a run, and a mutation measured the difference: with only the *never said*
/// case driven, `Some(n) if n > 0` could be weakened to `Some(_n)` and this suite stayed GREEN.
/// Each is asserted here by the WORDS its refusal must carry, so a fold that gave them one sentence
/// again would red rather than pass.
#[test]
fn the_verdict_refuses_the_shapes_the_population_cannot_produce() {
    assert_eq!(
        refusal_for(
            "ident-gate.sh",
            Some(0),
            true,
            "ident-gate selftest: 13/13 arm(s) pass"
        ),
        None,
        "an honest selftest must pass, or this gate refuses every script in the tree instead of \
         the empty one",
    );
    let failed = refusal_for(
        "hosted-read.sh",
        Some(1),
        false,
        "  FAIL  something\n38/39 arm(s) pass",
    )
    .expect("⛔ a selftest that exited non-zero must be refused — that is the gate's first job");
    assert!(
        failed.contains("exited Some(1)"),
        "the refusal must name the status it read, or a reader cannot tell a failed arm from a \
         missing binary: {failed}",
    );
    let silent = refusal_for("quiet.sh", Some(0), true, "  ok    nothing at all\n").expect(
        "⛔ ITEM 799: a selftest that exits 0 without saying how many arms it drove is exactly \
             the green this item is about, and the gate must refuse it. A mutation deleting this \
             requirement was GREEN twice — once inline, and once after the reader alone was \
             extracted — because no real selftest can produce this shape",
    );
    assert!(
        silent.contains("never said how many arms"),
        "the refusal must say WHICH of the two things went wrong: {silent}",
    );
    // ⛔⛔⛔⛔⛔ THE FOURTH SHAPE, AND THE ONE THAT WAS MISSING. A harness whose arms all vanished
    // still prints its summary, so it does not land on the branch above — it answers the question,
    // with the answer this item exists for. Until this call existed, `Some(n) if n > 0` could be
    // weakened to `Some(_n)` with the whole suite staying green (measured 2026-09-01).
    let empty = refusal_for(
        "emptied.sh",
        Some(0),
        true,
        "emptied selftest: 0/0 arm(s) pass",
    )
    .expect(
        "⛔ ITEM 799: a selftest whose own count is ZERO is a harness that drove nothing, and \
             it exits 0 like any other. The `> 0` in the verdict is the only thing between that \
             and a green, and nothing else in this tree can produce the case",
    );
    assert!(
        empty.contains("ZERO arm(s)"),
        "a run that SAID zero and one that never said are different facts and must not share a \
         sentence — that fold is what let the boundary rot unmeasured: {empty}",
    );
}

#[test]
fn a_run_that_named_no_arms_is_not_a_pass() {
    assert_eq!(
        arms_reported("hosted-read selftest: 39/39 arm(s) pass"),
        Some(39),
        "the real summary line is the one shape this must read, or the gate refuses every honest \
         selftest instead of the empty one",
    );
    assert_eq!(
        arms_reported("  ok    something\nall good\n"),
        None,
        "a run that exits 0 saying nothing about arms is exactly the green item 799 is about, and \
         it must not be readable as a count",
    );
    assert_eq!(
        arms_reported("selftest: 0/0 arm(s) pass"),
        Some(0),
        "a harness whose arms all vanished still prints a summary — the count has to come back as \
         zero so the gate's `n > 0` can refuse it, rather than as None which would read the same \
         as a missing line",
    );
}

/// ⚠⚠⚠ **THE POPULATION IS NOT EMPTY, AND COMMENTS ARE NOT IN IT.**
///
/// The gate above passes by finding nothing wrong. So would a walk that reached no files, or a
/// `code_of` that stripped every line away — and each of those has happened to a gate in this
/// repository. It would ALSO pass by finding `.githooks/pre-push`, which names the flag in a
/// comment and has no arm for it: running that would be this suite executing a push hook.
#[test]
fn the_walk_finds_the_declared_ones_and_not_the_ones_that_only_mention_it() {
    let declared = declared_selftests();
    let names: Vec<&str> = declared.iter().map(|(name, _)| name.as_str()).collect();
    assert!(
        names.contains(&"hosted-read.sh") && names.contains(&"ident-gate.sh"),
        "the two scripts measured as declaring a selftest on 2026-09-01 are not both in the \
         population — the walk or the code filter has stopped reaching them, and the gate beside \
         this one would then run nothing. Found: {names:?}",
    );
    assert!(
        !names.contains(&"pre-push"),
        "`pre-push` names `--selftest` in a COMMENT and has no arm for it, so a scan that puts it \
         in the population would have this suite invoking a push hook. Found: {names:?}",
    );
    assert!(
        Path::new(&repo_root().join(".githooks").join("pre-push")).is_file(),
        "the file that makes the check above meaningful is not there any more — if `pre-push` was \
         renamed, this test is asserting nothing about comment-only mentions",
    );
}

/// Whether a file this walk reached is a SHELL SCRIPT — the only kind of file a `--selftest` arm
/// can live in, and the decision the boundary gate below rests on.
///
/// ⚠ By NAME or by SHEBANG, and the second is not decoration: `.githooks/` names its hooks
/// `pre-push`, not `pre-push.sh`, so a stray selftest is most likely to be written in exactly the
/// style a name-only filter cannot see.
fn is_shell_script(name: &str, text: &str) -> bool {
    name.ends_with(".sh")
        || text
            .lines()
            .next()
            .is_some_and(|first| first.starts_with("#!") && first.contains("sh"))
}

/// Every shell script IN THE TREE whose code declares a `--selftest`, as repo-relative paths — the
/// whole population, not the part [`declared_selftests`] runs.
///
/// ⚠ `.git/` and `target/` are not source and are not walked. Nothing else is skipped, which is the
/// property this function exists for: a walk that can be told where not to look is a walk whose
/// exemption list is the answer.
fn shell_selftests_in_tree() -> Vec<String> {
    let root = repo_root();
    let mut stack = vec![root.clone()];
    let mut found: Vec<String> = Vec::new();
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir)
            .unwrap_or_else(|why| panic!("{} must be readable: {why}", dir.display()));
        for entry in entries {
            let path = entry.expect("a directory entry").path();
            let name = path
                .file_name()
                .expect("a directory entry has a name")
                .to_string_lossy()
                .into_owned();
            if path.is_dir() {
                if name != ".git" && name != "target" {
                    stack.push(path);
                }
                continue;
            }
            // A file with some OTHER extension is not a shell script, and skipping it here is what
            // keeps this from reading every `.rs` in the workspace to learn that.
            if path.extension().is_some_and(|ext| ext != "sh") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if is_shell_script(&name, &text) && code_of(&text).contains("--selftest") {
                found.push(
                    path.strip_prefix(&root)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
    }
    found.sort();
    found
}

/// ⛔⛔⛔⛔⛔ **RULE 6 — THE RUNNER LOOKS IN `.githooks/`, SO A SELFTEST ANYWHERE ELSE IS ONE
/// NOTHING RUNS.** The gate above fixes *nobody executes these two*; without this one it leaves the
/// escape hatch open in the same breath, because a script that grows a `--selftest` outside that
/// directory is not refused — it is simply never looked at, which is item 799 again in a new place.
///
/// ⇒ The boundary is a PREDICATE, not a sentence in a doc comment. Either a declared selftest is
/// somewhere this suite runs it, or this is red.
///
/// ⚠⚠ The remedy when it reds is a choice and the message says so: move the script into
/// `.githooks/`, or widen [`declared_selftests`] to run it where it lives. What is not available is
/// leaving it declared and undriven.
#[test]
fn no_selftest_is_declared_where_the_runner_never_looks() {
    let everywhere = shell_selftests_in_tree();
    // ⚠ THE POSITIVE CONTROL COMES FIRST. A walk that reached nothing would satisfy the boundary
    // below by finding no strays, which is the exact green this whole file is written against.
    assert!(
        everywhere.iter().any(|at| at == ".githooks/hosted-read.sh")
            && everywhere.iter().any(|at| at == ".githooks/ident-gate.sh"),
        "the tree-wide walk did not reach the two scripts measured as declaring a selftest on \
         2026-09-01, so its emptiness proves nothing about strays. Found: {everywhere:?}",
    );
    let stray: Vec<&String> = everywhere
        .iter()
        .filter(|at| !at.starts_with(".githooks/"))
        .collect();
    assert!(
        stray.is_empty(),
        "⛔ ITEM 799: a shell script declares a `--selftest` where this suite's runner never \
         looks, so nothing executes it — the same green the gate beside this one was written to \
         end. Move it under `.githooks/`, or widen `declared_selftests` to reach it: {stray:?}",
    );
}

/// ⚠⚠ **THE SHEBANG HALF, DRIVEN DIRECTLY** — every script in this tree today ends in `.sh` or
/// lives in `.githooks/`, so the walk above can never exercise the shebang branch on a stray, and a
/// requirement the population cannot reach is the dead control this file has already paid for
/// twice.
#[test]
fn the_script_filter_tells_a_hook_from_a_document() {
    assert!(
        is_shell_script("tidy.sh", "echo hi\n"),
        "a `.sh` name is a shell script whatever its first line says, or a stray with no shebang \
         walks straight past the boundary gate",
    );
    assert!(
        is_shell_script("pre-push", "#!/usr/bin/env bash\nexit 0\n"),
        "`.githooks/` names its hooks WITHOUT an extension, so a stray written in that style is \
         the likeliest one there is — and only the shebang can see it",
    );
    assert!(
        !is_shell_script("NOTES", "# --selftest is discussed here\n"),
        "a document that merely talks about the flag is not a script, and treating it as one \
         would make the boundary gate red on prose",
    );
}
