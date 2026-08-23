//! **THE COMMAND THIS REPOSITORY DECLARES AS ITS VERIFY MUST NOT BE ABLE TO PASS A SUBSET** —
//! register item 620, and the defect register item 585 measured, found sitting in the one place
//! nobody had looked.
//!
//! # ⛔⛔⛔ The comment above it claimed the opposite of what it did
//!
//! `.claude/remote-build.toml` introduces its `verify` with *"THE SAME FOUR GATES THIS PROJECT RUNS
//! LOCALLY, in one command **so a remote run cannot pass a subset**"*. The command under that
//! sentence was four clauses joined by `&&`, none of them carrying `--no-fail-fast` — a subset in
//! two independent ways:
//!
//! * **within a clause**: `cargo test` stops at the first failing binary, so a red in an early
//!   crate leaves every later crate unrun. Measured 2026-08-22: 61 of 80 suites ran, and the exit
//!   code said nothing about the nineteen.
//! * **between clauses**: `&&` means a red first clause skips the other three, so a failing
//!   workspace suite silently withdraws clippy and rustdoc from the run.
//!
//! Both come back as the exit code a full run gives.
//!
//! # ⚠⚠⚠⚠⚠ The joining is DRIVEN, not grepped — the first version of this file grepped it
//!
//! That version refused any `&&` or `||` in the command. It was wrong twice over: `||` is how a
//! clause reports failure WITHOUT withdrawing the rest (`cmd || rc=1`), so the rule forbade the
//! repair; and a rule that reads shell control flow out of substrings is register item 611's defect
//! — *a substring of prose is not a declaration*. Worse, plain `;` alone would have satisfied it
//! while making the command's status the LAST clause's, so a red suite would have exited 0.
//!
//! So the property is put to a SHELL: the declared text is run with a tracked double `cargo` on `PATH` that
//! records each invocation and can be told to fail. What is asserted is what the sentence in the
//! file claims — every clause runs, and a failure anywhere is still a failure at the end.
//!
//! ⚠ **THE RESIDUE, STATED**: nothing forces a person to run the DECLARED command rather than
//! typing their own four. That is register item 620's remaining half; what this closes is that the
//! declared one is honest, which it was not.

use std::path::PathBuf;

/// The declaration under test — the same file `a_fleet_ceiling_is_a_measurement_with_a_date` reads,
/// which is what makes an edit to it something two gates notice.
const DECL: &str = ".claude/remote-build.toml";

fn workspace_root() -> PathBuf {
    [env!("CARGO_MANIFEST_DIR"), "..", ".."].iter().collect()
}

/// The value of `verify` under `[commands]`, as the file spells it.
///
/// Hand-parsed for this crate's no-dependencies rule, and narrowly: the key is a single-line
/// double-quoted string, and a `verify` that stopped being one PANICS here rather than reading as
/// absent — an absence is what a gate silently passes on, and deleting the command is the cheapest
/// way to make every claim below green.
fn declared_verify() -> String {
    let path = workspace_root().join(DECL);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|why| panic!("⚠ cannot read `{}`: {why}", path.display()));

    let mut in_commands = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            in_commands = name.trim() == "commands";
            continue;
        }
        if !in_commands {
            continue;
        }
        if let Some(rest) = line.strip_prefix("verify") {
            let rest = rest.trim_start();
            let Some(rest) = rest.strip_prefix('=') else {
                continue;
            };
            let rest = rest.trim();
            let quoted = rest
                .strip_prefix('"')
                .and_then(|r| r.strip_suffix('"'))
                .unwrap_or_else(|| {
                    panic!(
                        "⚠ `[commands] verify` is not the single-line quoted string this gate \
                         reads: {rest:?}. A shape this cannot parse must not read as ABSENT — \
                         teach it the new shape rather than letting the claims below go unchecked"
                    )
                });
            // TOML's own escape for a quote inside a basic string. Nothing else is unescaped: a
            // command needing more than this is a command this gate should be taught about.
            return quoted.replace("\\\"", "\"");
        }
    }
    panic!("⚠ `{DECL}` declares no `[commands] verify`, so there is nothing to hold to anything");
}

/// Run the declared command with a tracked `cargo` double that logs every invocation and fails the
/// `fail_nth`-th one (1-based; `0` fails none).
///
/// Returns `(each cargo subcommand in the order it was invoked, the whole command's exit status)`.
///
/// ⚠⚠ **THE DOUBLE IS `cargo` AND NOT A REWRITE OF THE TEXT.** The declared string is handed to `sh`
/// verbatim; only what `cargo` resolves to changes. So the control flow under test is the real
/// one — its separators, its redirections, its `exit` — rather than a paraphrase this file built.
fn drive(fail_nth: usize) -> (Vec<String>, i32) {
    // ⚠⚠⚠⚠⚠ ONE COUNTER FILE PER CALL, and it took two wrong answers to get here. Keyed on the pid
    // alone, two drives in the same test appended to one file and the second read the first's
    // invocations too — five clauses compared against ten. Adding `fail_nth` fixed that pair and
    // broke a worse one: the tests in this file run in PARALLEL and two of them drive `0`, so they
    // shared a path and each cleared the other mid-run, leaving a drive with an empty counter and
    // no explanation. A counter unique per CALL is the only key that is actually unique — the two
    // before it were unique per *something the caller happened to differ in*.
    static NTH: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let call = NTH.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let calls = std::env::temp_dir().join(format!(
        "sprag-verify-shape-{}-{call}.calls",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&calls);
    std::fs::File::create(&calls).expect("a counter for the double to append to");

    // ⚠⚠⚠⚠ THE `cargo` IS A TRACKED DOUBLE, NOT A FILE THIS TEST WRITES — register item 467, and
    // `no_suite_runs_a_program_it_wrote` caught the first version of this gate doing exactly that
    // in the sweep of the round that added it. A file a process holds open for writing cannot be
    // executed, and this workspace's harness forks from threads.
    //
    // ⚠ The version before that failed the OTHER way and quietly: written with `\`-continued Rust
    // string lines, whose continuation eats the next line's leading whitespace, the script became
    // one line glued onto its own shebang. `sh` then found no interpreter, fell through to the REAL
    // `cargo`, and the gate compiled the workspace for over a minute before anyone asked why a stub
    // was slow. Tracking the program removes both failures at once: it is a file, checked in, that
    // a shell has always been able to run.
    let doubles =
        sprag_gate::doubles::Doubles::of(env!("CARGO_MANIFEST_DIR")).set("declared-verify");
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg(declared_verify())
        .env("PATH", doubles.ahead_of_inherited())
        .env("DOUBLE_CALLS", &calls)
        .env("DOUBLE_FAIL_NTH", fail_nth.to_string())
        .current_dir(workspace_root())
        .output()
        .expect("the declared command runs under a shell");

    let seen = std::fs::read_to_string(&calls).unwrap_or_default();
    let subcommands: Vec<String> = seen.lines().map(str::to_owned).collect();
    let _ = std::fs::remove_file(&calls);
    (subcommands, out.status.code().unwrap_or(-1))
}

/// ⛔⛔⛔ **A RED IN AN EARLY CLAUSE MUST NOT WITHDRAW THE LATER ONES, AND MUST STILL BE A RED** —
/// item 620, both halves in one drive because a command can fail either of them alone.
///
/// ⚠⚠ **THE SECOND HALF IS THE ONE A NAIVE REPAIR BREAKS.** Joining with plain `;` makes every
/// clause run and hands back the LAST clause's status — so a failing suite exits 0, which is worse
/// than the defect being repaired. The declared command has to collect the failure and end on it.
#[test]
fn a_failing_clause_leaves_the_others_running_and_the_verdict_red() {
    let (all_ran, _) = drive(0);
    assert!(
        all_ran.len() >= 4,
        "⚠ THE STAGING, NOT THE CLAIM: the declared command has to invoke cargo at least four \
         times for this to be about anything. Saw {all_ran:?}",
    );

    let (seen, status) = drive(1);
    assert_eq!(
        seen, all_ran,
        "⛔⛔⛔ ITEM 620: the FIRST clause failed and the run no longer reached everything it \
         reaches when nothing fails — so a red suite silently withdraws clippy, rustdoc, or the \
         coverage check from the verification. The comment above this command claims it exists so \
         a run 'cannot pass a subset'; joined that way it IS a subset. Collect the failure \
         (`cmd || rc=1`) instead of chaining on it",
    );
    assert_ne!(
        status, 0,
        "⛔⛔⛔⛔ AND THE WHOLE COMMAND MUST STILL BE RED. Every clause ran, which is half the \
         claim — but the status came back 0 with a failing clause in it, which is what plain `;` \
         gives you: the last clause's verdict and nothing else. That is a WORSE defect than the \
         one being repaired, because it reports success over a failure rather than over silence",
    );
}

/// ⚠⚠⚠⚠⚠ **THE CONTROL, AND WITHOUT IT THE GATE ABOVE PASSES AGAINST A COMMAND THAT ALWAYS FAILS.**
///
/// `assert_ne!(status, 0)` is satisfied by `exit 1`. What makes the pair a measurement is that the
/// same text, with nothing failing, comes back green.
#[test]
fn the_declared_verify_is_green_when_nothing_under_it_fails() {
    let (seen, status) = drive(0);
    assert_eq!(
        status, 0,
        "⚠⚠⚠⚠⚠ THE CONTROL: every clause of the declared command succeeded and it still reported \
         failure, so the gate beside this one is measuring nothing. Saw these cargo subcommands: \
         {seen:?}",
    );
}

/// ⛔⛔⛔ **AND NO CLAUSE MAY STOP AT ITS OWN FIRST FAILING BINARY** — item 585's measurement, which
/// is what made this a number rather than a worry: 61 suites of 80.
///
/// ⚠⚠ A SPELLING QUESTION, and it is asserted as one: `--no-fail-fast` is a flag `cargo test` has
/// and the others do not, so only the test clauses are held to it. The drive above cannot see this
/// — a `cargo` double has no binaries to stop at — which is why the two live side by side.
#[test]
fn every_test_clause_of_the_declared_verify_runs_to_the_end() {
    let verify = declared_verify();
    let clauses: Vec<&str> = verify.split(';').map(str::trim).collect();
    let tests: Vec<&&str> = clauses
        .iter()
        .filter(|c| c.contains("cargo test"))
        .collect();

    assert!(
        !tests.is_empty(),
        "⚠ THE STAGING: a `verify` with no `cargo test` clause has nothing for this gate to be \
         about. Got: {verify:?}",
    );
    for clause in tests {
        assert!(
            clause.contains("--no-fail-fast"),
            "⛔⛔⛔ ITEM 585, one file over: `cargo test` stops at its first failing binary, so \
             this clause reports one failure and leaves the rest of its crates UNRUN — not red and \
             not green, and the exit code cannot tell you which. Measured 2026-08-22 on this \
             workspace: 61 suites of 80, noticed only by counting lines by hand. Clause: {clause:?}",
        );
    }
}

/// ⛔⛔⛔ **AND THE RUN MUST JUDGE ITS OWN COVERAGE, NOT ONLY ITS EXIT CODE** — item 620's point.
///
/// `--no-fail-fast` and a collected status make every clause RUN and every failure COUNT; neither
/// says a word about whether the crates a clause was supposed to reach reported anything. That is
/// what `sweep-coverage` answers, derived from the workspace's own member list. Putting the call
/// inside the declared command is what makes it unskippable for whoever runs that command —
/// *"I remember to run it afterwards"* is the prose register item 456 refuses.
#[test]
fn the_declared_verify_ends_by_asking_which_crates_it_reached() {
    let verify = declared_verify();
    assert!(
        verify.contains("sweep-coverage"),
        "⛔⛔⛔ ITEM 620: this command runs the workspace's tests and then reports an exit code, \
         which says nothing about the crates it never reached. A run that does not call \
         `sweep-coverage` is a run whose coverage nobody judged. Got: {verify:?}",
    );
}
