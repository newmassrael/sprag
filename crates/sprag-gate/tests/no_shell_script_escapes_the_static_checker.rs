//! **EVERY SHELL SCRIPT THIS TREE CARRIES PASSES THE PINNED SHELLCHECK** — register item 1086.
//!
//! # What was measured
//!
//! `.githooks/` holds every gate a commit and a push must clear, about 4,600 lines of shell, and
//! carried 28 ShellCheck directives while nothing in the tree ran ShellCheck (2026-09-12). Run over
//! the whole walk on 2026-09-13 it reported seventeen findings in eight files: four `info` in the
//! hooks, where the intent was exactly what the check questions, and thirteen in the test doubles
//! and an instrument — a sourced library with no dialect, sources it could not follow, a string
//! assignment that reads as a command, and a trap handler it lost.
//!
//! # Why a test in this crate rather than a step in a hook
//!
//! `.githooks/pre-commit`'s ratchet lane runs `cargo test -p sprag-gate` in a checkout of the INDEX,
//! and both headless CI jobs sweep this crate. So this runs on the bytes a commit will carry, on
//! the bytes a push publishes, and on a machine that is not the author's — which a hook step would
//! not, and which is why every other gate over `.githooks/` lives here too.
//!
//! # The population is the walk, not a list
//!
//! [`sprag_gate::shell::shell_sources`] is what the selftest and `sed` gates already read. A script
//! added anywhere tomorrow is checked without anybody remembering it, and a hook the walk does not
//! classify as shell is a red below rather than a file nothing checks.

use sprag_gate::shellcheck::{self, Verdict};
use std::path::PathBuf;
use std::process::Command;

/// The tree this gate is part of — through the one door, register item 809.
fn repo_root() -> PathBuf {
    sprag_gate::sources::workspace_root()
}

/// Refuses, with [`shellcheck::tool_refusal`]'s sentence, unless the ShellCheck on PATH is the
/// pinned one; answers that version otherwise.
fn require_the_pinned_checker() -> String {
    let pin = repo_root().join(shellcheck::PIN);
    let text = std::fs::read_to_string(&pin)
        .unwrap_or_else(|why| panic!("{} pins the checker: {why}", pin.display()));
    let pinned = shellcheck::pinned_version(&text).unwrap_or_else(|why| panic!("ITEM 1086: {why}"));
    let probe = shellcheck::version_probe()
        .output()
        .map(|said| {
            (
                said.status.code(),
                format!(
                    "{}{}",
                    String::from_utf8_lossy(&said.stdout),
                    String::from_utf8_lossy(&said.stderr),
                ),
            )
        })
        .map_err(|why| why.kind());
    if let Some(why) = shellcheck::tool_refusal(probe, &pinned) {
        panic!("ITEM 1086: {why}");
    }
    pinned
}

/// What one run came to, read by [`shellcheck::verdict`].
fn verdict_of(run: &mut Command) -> Verdict {
    let said = run
        .output()
        .unwrap_or_else(|why| panic!("the pinned ShellCheck answered its probe and now {why}"));
    shellcheck::verdict(
        said.status.code(),
        &String::from_utf8_lossy(&said.stdout),
        &String::from_utf8_lossy(&said.stderr),
    )
}

/// **THE GATE.** One run over the whole walk, and its verdict is clean.
#[test]
fn every_shell_script_passes_the_pinned_shellcheck() {
    let pinned = require_the_pinned_checker();
    let root = repo_root();
    let files: Vec<String> = sprag_gate::shell::shell_sources()
        .into_iter()
        .map(|source| source.file)
        .collect();

    // RULE 6. Every file in `.githooks/` is a hook or a library a hook sources, so each one the walk
    // does not classify as shell is a hook nothing here checks — refused, not skipped.
    let hooks = root.join(".githooks");
    let mut unclassified: Vec<String> = std::fs::read_dir(&hooks)
        .unwrap_or_else(|why| panic!("{} must be readable: {why}", hooks.display()))
        .map(|entry| entry.expect("a directory entry").path())
        .filter(|path| path.is_file())
        .map(|path| {
            format!(
                ".githooks/{}",
                path.file_name()
                    .expect("a file has a name")
                    .to_string_lossy()
            )
        })
        .filter(|hook| !files.contains(hook))
        .collect();
    unclassified.sort();
    assert!(
        unclassified.is_empty(),
        "ITEM 1086: these files in `.githooks/` are not in the shell walk, so ShellCheck never \
         reads them. Give each a shell shebang or a `.sh` name, or move it out of `.githooks/`: \
         {unclassified:?}",
    );
    // THE CONTROL on the other half: the walk reaches past `.githooks/`, where thirteen of the
    // seventeen findings this gate was opened on stood.
    assert!(
        files.iter().any(|file| file.starts_with("crates/")),
        "the walk reached no script under `crates/`, so the test doubles went unchecked: {files:?}",
    );

    match verdict_of(&mut shellcheck::invocation(&root, &files)) {
        Verdict::Clean => {
            eprintln!(
                "ShellCheck {pinned}: {} script(s) checked, no finding",
                files.len()
            );
        }
        Verdict::Findings(found) => panic!(
            "ITEM 1086: ShellCheck {pinned} has {} finding(s) over the {} shell script(s) this tree \
             carries. Fix the script, or — where the code means exactly what the check questions — \
             put `# shellcheck disable=SCnnnn  # <why>` on the line above that command:\n{}",
            found.len(),
            files.len(),
            found
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        Verdict::Unread(why) => panic!(
            "ITEM 1086: the ShellCheck run over {} script(s) cannot be believed either way — it {why}",
            files.len(),
        ),
    }
}

/// **NO SCRIPT, AND NOT THE PROJECT'S CONFIGURATION, SWITCHES THE CHECKER OFF BY ITS OWN TEXT.**
///
/// The gate above is only as strong as its exemptions: a `disable=all`, a `disable=` at the head of
/// a file, or a `disable=` in `.shellcheckrc` each turn a finding into silence while the run stays
/// honest. Those are refused here, and so is a `disable=` that gives no reason.
#[test]
fn no_script_or_setting_switches_the_checker_off() {
    let sources = sprag_gate::shell::shell_sources();
    let mut refused: Vec<String> = Vec::new();
    let mut read = 0_usize;
    for source in &sources {
        read += shellcheck::disables(&source.text).len();
        refused.extend(
            shellcheck::hatches_in(&source.file, &source.text)
                .iter()
                .map(ToString::to_string),
        );
    }
    // THE CONTROL. A scan that recognised no directive would find no hatch, and `pre-push` is where
    // the nine `disable=`s this item was opened on stood.
    let pre_push = sources
        .iter()
        .find(|source| source.file == ".githooks/pre-push")
        .expect("`.githooks/pre-push` is in the shell walk");
    assert!(
        !shellcheck::disables(&pre_push.text).is_empty(),
        "the directive scan found no `disable=` in `.githooks/pre-push`, which carries several — it \
         has stopped reading directives, and the check below would pass on anything",
    );

    let rc = repo_root().join(shellcheck::RC);
    let settings = std::fs::read_to_string(&rc).unwrap_or_else(|why| {
        panic!(
            "ITEM 1086: {} is the configuration the gate names with `--rcfile`: {why}",
            rc.display()
        )
    });
    refused.extend(shellcheck::rc_refusals(&settings));

    assert!(
        refused.is_empty(),
        "ITEM 1086: {read} `disable=` directive(s) read across {} script(s), and these switch the \
         checker off further than one explained command:\n{}",
        sources.len(),
        refused.join("\n"),
    );
}

/// **A CONFIGURATION NEARER THE SCRIPT CANNOT CHANGE THE VERDICT.**
///
/// ShellCheck uses the first `.shellcheckrc` it finds walking up from each script, then the home
/// directory's — so a stray one below the root, or a person's own, would silence a check for every
/// script it reaches. `invocation` names the project's with `--rcfile`; this runs a real script
/// against a nearer configuration both ways.
///
/// THE CONTROL COMES FIRST: without `--rcfile`, ShellCheck must obey the nearer file. If it did not,
/// the fixture would measure nothing and the second half would be green for free.
///
/// THE PROJECT ROOT IS THIS ARM'S OWN, and a mutation is why. With the tree's real root, the verdict
/// also depended on what the real `.shellcheckrc` says: a `disable=SC2086` put there (which the gate
/// beside this one refuses, correctly) made this arm red too, with a message blaming the nearer file
/// for what the named one did. Its subject is the mechanism — the named file wins over discovery —
/// so it names a root whose configuration disables nothing, and the real file's content stays the
/// other gate's question.
#[test]
fn a_configuration_nearer_the_script_cannot_change_the_verdict() {
    require_the_pinned_checker();
    // Under the directory cargo hands every integration test: not the tree's walk (`target/` is
    // never walked), not the machine's temp directory (register item 794's ratchet). The script is
    // READ by ShellCheck and never executed, so register item 467's window does not arise.
    let scratch = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("shellcheck-nearer-rc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let below = scratch.join("below");
    std::fs::create_dir_all(&below).expect("the scratch directories must be creatable");
    std::fs::write(
        scratch.join(shellcheck::RC),
        "# this arm's own project configuration: it disables nothing\nexternal-sources=true\n",
    )
    .expect("the named configuration must be writable");
    std::fs::write(below.join(".shellcheckrc"), "disable=SC2086\n")
        .expect("the nearer configuration must be writable");
    let script = below.join("splits.sh");
    std::fs::write(&script, "#!/bin/sh\nsay() { echo $1; }\nsay x\n")
        .expect("the fixture script must be writable");
    let files = [script.display().to_string()];

    let obeyed = verdict_of(
        Command::new("shellcheck")
            .env_remove("SHELLCHECK_OPTS")
            .arg("--format=gcc")
            .args(&files),
    );
    let judged = verdict_of(&mut shellcheck::invocation(&scratch, &files));
    let _ = std::fs::remove_dir_all(&scratch);

    assert_eq!(
        obeyed,
        Verdict::Clean,
        "THE CONTROL FAILED: with no `--rcfile`, the `.shellcheckrc` beside the script did not \
         silence its SC2086, so this fixture measures nothing about the gate's run",
    );
    match judged {
        Verdict::Findings(found) if found.iter().any(|finding| finding.code == "SC2086") => {}
        other => panic!(
            "ITEM 1086: the gate's own invocation obeyed a `.shellcheckrc` nearer the script than \
             the project's and came to {other:?} — a file dropped anywhere below the root would \
             silence that check for every script beside it",
        ),
    }
}
