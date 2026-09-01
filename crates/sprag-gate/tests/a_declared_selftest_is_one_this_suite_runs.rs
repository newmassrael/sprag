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

/// ⛔ **THE GATE.** Every declared selftest runs here, and every one of them passes.
///
/// Two things are required and the second is why this is not merely a runner: the status must be
/// zero, AND the run must say how many arms it drove. A harness that exits 0 having driven nothing
/// is exactly the green this item is about, and it is indistinguishable from a real pass by status
/// alone.
#[test]
fn every_declared_selftest_runs_and_passes_here() {
    let declared = declared_selftests();
    let mut refused: Vec<String> = Vec::new();
    for (name, path) in &declared {
        let run = Command::new("bash")
            .arg(path)
            .arg("--selftest")
            .current_dir(repo_root())
            .output()
            .unwrap_or_else(|why| panic!("{name} --selftest must be runnable: {why}"));
        let said = format!(
            "{}{}",
            String::from_utf8_lossy(&run.stdout),
            String::from_utf8_lossy(&run.stderr),
        );
        if let Some(why) = refusal_for(name, run.status.code(), run.status.success(), &said) {
            refused.push(why);
        }
    }
    assert!(
        refused.is_empty(),
        "⛔ ITEM 799: a `.githooks/` script declares a `--selftest` and it does not pass when this \
         suite runs it. Until this gate existed nothing executed either of them — 52 arms that ran \
         only when a person typed the command:\n{}",
        refused.join("\n"),
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
