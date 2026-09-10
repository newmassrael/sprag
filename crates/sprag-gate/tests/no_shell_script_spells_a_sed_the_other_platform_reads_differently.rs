//! **NO SHELL SCRIPT MAY SPELL A `sed` SCRIPT BSD READS DIFFERENTLY** — register item 1006.
//!
//! # ⚠⚠⚠⚠⚠ What was uncounted, and what it had already cost
//!
//! This repository already owns a strict `sed`: `tests/doubles/declared-selftest/sed`, which stands
//! in front of a Linux runner so it answers the way the macOS job's own `sed` would. Item 1001
//! measured that it was covering one difference and not the other and repaired it. What neither
//! round asked was **which scripts ever meet it**.
//!
//! The answer, measured 2026-09-10: the strict `sed` is an ENVIRONMENT that
//! [`a_declared_selftest_is_one_this_suite_runs`](../a_declared_selftest_is_one_this_suite_runs.rs)
//! drives DECLARED SELFTESTS under. Five of this repository's hook scripts declare one. **Six do
//! not** — `commit-msg`, `content-gate.sh`, `doc-gate.sh`, `pre-commit`, `pre-push` and
//! `rust-gates.sh` — and every `sed` line in those six was judged by nothing at all. Running them
//! is not available as a remedy: `pre-commit` runs clippy and the test lane, `commit-msg` wants a
//! file, `pre-push` wants refs on stdin. A population defined by *what happens to be runnable* is
//! the escape hatch this workspace's rule 6 is about.
//!
//! And it was not hypothetical. `rust-gates.sh` read the workspace's own memory declaration with
//! `s/^precommit_kb = \([0-9]\+\).*/\1/p`. `\+` is a GNU extension to a BASIC regular expression;
//! a BSD `regex(3)` reads it as a literal `+`, so on macOS that matched nothing, `routed_kb` came
//! back empty, and the hook announced *no usable precommit_kb in the file* about a file that says
//! exactly what it should. **A refusal naming the wrong cause is worse than no refusal**, and no
//! Linux run could see it.
//!
//! # ⛔⛔⛔⛔⛔ Why the rule is not spelled here
//!
//! This gate does not decide what is unportable. It finds the argv of every `sed` command the tree
//! spells and hands it to **that same double**, in a scan-only mode, and reads the status. So there
//! is ONE implementation of *what BSD reads differently* and two populations reach it: the runtime
//! one (a hook under test) and this static one (every file). A second copy of the rule written in
//! Rust is the drift this workspace keeps paying for — and it would drift in the direction that
//! matters, because only one of the two copies would be the one a hook actually meets.
//!
//! # ⚠⚠ What this can and cannot claim
//!
//! [`sprag_gate::shell::commands_named`] is a quote-aware word splitter, not a shell. It cannot see
//! a command assembled at run time, a script arriving on stdin, or one that spans a line break, and
//! it expands nothing — a word keeps its `${variables}`, which is why the double is asked for a
//! VERDICT rather than run. Every one of those limits misses an offender rather than inventing one.

use sprag_gate::doubles::Doubles;
use sprag_gate::shell::{commands_named, shell_sources};
use std::path::PathBuf;
use std::process::Command;

/// The programs whose spellings this judges, each with a stand-in that owns the rule for it.
///
/// ⚠⚠ TWO, since register item 1007, and the reason they are one gate rather than two: `sed` and
/// `grep` hand the same text to the same `regcomp`, so *what BSD reads differently* is one fact
/// about basic regular expressions. `bre-rule.sh` is where that fact is spelled; each stand-in adds
/// only what is true of its own argv (which words are patterns) and, for `sed`, the label rule that
/// belongs to script parsing rather than to regular expressions.
const JUDGED: [&str; 2] = ["sed", "grep"];

/// One command this tree spells, and where.
#[derive(Debug, Clone)]
struct Spelled {
    /// Relative to the workspace root, so a message is a path a person can open.
    file: String,
    /// One-indexed, the way an editor counts.
    line: usize,
    /// What the shell would hand the child, the program itself included.
    argv: Vec<String>,
}

/// The stand-in that owns the rule for `program`.
fn strict(program: &str) -> PathBuf {
    Doubles::of(env!("CARGO_MANIFEST_DIR"))
        .set("declared-selftest")
        .program(program)
}

/// What the stand-in says about one argv: `Ok(())`, or everything it said when it refused.
///
/// ⚠ `SPRAG_SED_SCAN_ONLY` stops it before it execs anything. The subject here is the SPELLING; a
/// script lifted out of source text still carries its `${variables}` and running it would fail for
/// reasons that are nobody's defect.
fn verdict_on(argv: &[String]) -> Result<(), String> {
    let program = strict(&argv[0]);
    let mut run = Command::new(&program);
    run.env("SPRAG_SED_SCAN_ONLY", "1");
    for word in argv.iter().skip(1) {
        run.arg(word);
    }
    let said = run
        .output()
        .unwrap_or_else(|why| panic!("{} must be runnable: {why}", program.display()));
    if said.status.success() {
        return Ok(());
    }
    Err(format!(
        "{}{}",
        String::from_utf8_lossy(&said.stdout),
        String::from_utf8_lossy(&said.stderr),
    ))
}

/// Every judged command spelled in this tree's shell, whole-line comments dropped.
fn spellings() -> Vec<Spelled> {
    let mut found = Vec::new();
    for source in shell_sources() {
        for (line, text) in source.code() {
            for program in JUDGED {
                for argv in commands_named(&text, program) {
                    found.push(Spelled {
                        file: source.file.clone(),
                        line,
                        argv,
                    });
                }
            }
        }
    }
    found
}

/// ⛔⛔⛔⛔⛔ **THE RATCHET.** Every judged command in the tree, judged by its own stand-in.
#[test]
fn no_shell_script_spells_a_sed_the_other_platform_reads_differently() {
    let offenders: Vec<(Spelled, String)> = spellings()
        .into_iter()
        .filter_map(|one| verdict_on(&one.argv).err().map(|why| (one, why)))
        .collect();

    assert!(
        offenders.is_empty(),
        "⛔ ITEMS 1006 AND 1007: {} command(s) in this tree are spelled in a way BSD reads \
         differently, and this is where it is caught because nothing runs the script they live \
         in. Reproduce any one of them with:\n  SPRAG_SED_SCAN_ONLY=1 \
         crates/sprag-gate/tests/doubles/declared-selftest/<program> <the argv below>\n{}",
        offenders.len(),
        offenders
            .iter()
            .map(|(one, why)| format!("  {}:{} — {:?}\n{}", one.file, one.line, one.argv, why))
            .collect::<Vec<_>>()
            .join("\n"),
    );
}

/// ⛔⛔⛔⛔⛔ **AND THE POPULATION IS FILES, WHICH IS THE WHOLE OF ITEM 1006.**
///
/// The gate above passes by finding nothing wrong. So would a walk that reached only the scripts
/// the strict `sed` ALREADY met, which is the state this item is about — the ratchet would be a
/// second, slower copy of a measurement that already existed.
///
/// ⚠ The arm is derived, not a filename: at least one judged command must live in a script that
/// declares no `--selftest`. If a day comes when every shell script in this tree declares one, this
/// goes red and its remedy is to delete it, because the two populations will have converged.
#[test]
fn the_walk_reaches_scripts_no_declared_selftest_ever_runs() {
    let judged = spellings();
    let undriven: Vec<&Spelled> = shell_sources()
        .iter()
        .filter(|source| !source.code_cut_at_hash().contains("--selftest"))
        .flat_map(|source| judged.iter().filter(|one| one.file == source.file))
        .collect();
    assert!(
        judged.len() > 100,
        "a scan that found only {} command(s) has stopped matching — this workspace's shell spelled \
         121 of them on 2026-09-10 (32 `sed`, 89 `grep`), {} in scripts nothing drives, and a \
         probe that reads nothing must never read as clean. ⚠ The floor is 100 because every way \
         this scan has actually broken took a big bite: judging only the declared scripts left 21, \
         a splitter blind to `\"$( … )\"` left 24, and judging `sed` alone left 32. If the tree \
         really did lose that many, lower the floor IN THE SAME EDIT and say which script went. \
         Judged: {:?}",
        judged.len(),
        undriven.len(),
        judged
            .iter()
            .map(|one| format!("{}:{}", one.file, one.line))
            .collect::<Vec<_>>(),
    );
    assert!(
        !undriven.is_empty(),
        "⛔ ITEM 1006: every command this gate judged lives in a script that declares a \
         `--selftest`, so all of them were already being driven under the strict stand-ins and \
         this ratchet is measuring nothing new. Either the walk has narrowed, or the two \
         populations have converged and this gate can go. Judged: {} command(s) in {} file(s).",
        judged.len(),
        judged
            .iter()
            .map(|one| one.file.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
    );
}

/// ⛔⛔⛔⛔⛔ **AND THE VERDICT IS A MEASUREMENT, NOT A MODE THAT RETURNED ZERO.**
///
/// `SPRAG_SED_SCAN_ONLY` is an early exit. An early exit that stopped scanning would make the
/// ratchet above pass over anything at all, silently and forever — which is this crate's own
/// subject one level down. So the two answers are driven directly, through the same door the gate
/// uses.
///
/// ⚠⚠ **BOTH DIRECTIONS.** A double that refused everything would satisfy the refusal arm and turn
/// every hook in this repository into a wall.
#[test]
fn the_scan_only_verdict_refuses_and_admits_the_two_spellings_it_was_measured_on() {
    // ⛔ THE `grep` HALF FIRST — register item 1007, and it is driven with the tree's OWN pattern.
    // Measured 2026-09-10 with a FreeBSD grep(1) built on a FreeBSD regex(3): this counts 2 under
    // GNU and 0 under BSD, in an instrument whose entire output is counts.
    let refused_grep = verdict_on(&[
        "grep".to_owned(),
        "-ac".to_owned(),
        r"^error\[\|could not compile".to_owned(),
    ]);
    assert!(
        refused_grep.is_err(),
        "⛔ ITEM 1007: the strict grep ADMITTED `\\|`, the GNU extension a BSD regex(3) reads as a \
         literal `|`. `grep -c` then counts ZERO on that platform and 0 is indistinguishable from \
         *nothing went wrong* — which is precisely what `probe-unrun-crates` excluded an \
         explanation with.",
    );
    let admitted_grep = verdict_on(&[
        "grep".to_owned(),
        "-ac".to_owned(),
        "-e".to_owned(),
        r"^error\[".to_owned(),
        "-e".to_owned(),
        "could not compile".to_owned(),
    ]);
    assert!(
        admitted_grep.is_ok(),
        "⛔ ITEM 1007: the strict grep REFUSED `-e A -e B`, the POSIX spelling both implementations \
         answer 2 to (measured 2026-09-10). A rule that refuses the repair leaves nothing that can \
         be written, saying:\n{}",
        admitted_grep.unwrap_err(),
    );

    let refused = verdict_on(&[
        "sed".to_owned(),
        "-n".to_owned(),
        r"s/^precommit_kb = \([0-9]\+\).*/\1/p".to_owned(),
    ]);
    assert!(
        refused.is_err(),
        "⛔ ITEM 1006: the strict sed ADMITTED `\\+`, the GNU extension a BSD regex(3) reads as a \
         literal `+` — measured 2026-09-10 as the spelling that made `rust-gates.sh` announce the \
         wrong cause on macOS. The ratchet beside this is then walking every command in the tree \
         and asking a question that has no wrong answer.",
    );
    let admitted = verdict_on(&[
        "sed".to_owned(),
        "-n".to_owned(),
        r"s/^precommit_kb = \([0-9][0-9]*\).*/\1/p".to_owned(),
    ]);
    assert!(
        admitted.is_ok(),
        "⛔ ITEM 1006: the strict sed REFUSED the POSIX spelling of the same expression, which \
         every implementation measured on 2026-09-10 answers identically to. A rule that refuses \
         the repair as well as the defect leaves nothing that can be written, saying:\n{}",
        admitted.unwrap_err(),
    );
}
