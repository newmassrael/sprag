//! **A HOOK IS A PROGRAM, AND UNTIL THIS FILE NOTHING IN THIS REPOSITORY EVER RAN ONE** — register
//! item 404.
//!
//! # ⚠⚠⚠ Why this file exists
//!
//! `.githooks/` holds every rule this project enforces on itself before a commit or a push, and the
//! rules were enforced by nobody: no test in any crate executed one, and `ci.yml` names them only
//! to explain why it does not repeat their work. Two silent-pass defects had to be found by a
//! person READING the files rather than by a red —
//!
//!   * `validate-code-refs` discarded 52 violations into `/dev/null` in two hooks (item 213), and
//!   * `commit-msg` implemented *no emoji* and *English only* with `grep -qP … 2>/dev/null`, so on
//!     any grep without PCRE — **BSD grep, the macOS default** — both rules passed every message
//!     (item 403).
//!
//! The second is what this file gates, because a hook's whole surface is an exit code and that is
//! precisely what can be driven: give it a message file, read its status. It is the same shape as
//! [`ambient_home_guard`](../ambient_home_guard.rs) driving the guard BINARY rather than the walk
//! behind it — a unit test on a rule is not a test that the hook enforces it.
//!
//! # ⚠⚠ What this file does NOT cover, said plainly so a green run is not misread
//!
//! Only `commit-msg`. It is the one hook that is hermetic: bash and grep, a file in, a status out.
//! `pre-commit` and `pre-push` need `mnemosyne-cli`, a cargo toolchain, and (on a paint change) an
//! X server, so driving them belongs to item 404's later payments. **Nothing here says those two
//! are enforced.**

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The tree this gate is part of — `crates/sprag-gate/` is two levels down from it.
fn repo_root() -> PathBuf {
    [env!("CARGO_MANIFEST_DIR"), "..", ".."].iter().collect()
}

/// The hook under test, as git would invoke it.
fn hook() -> PathBuf {
    repo_root().join(".githooks").join("commit-msg")
}

/// A scratch directory of this test's own, named for the case that owns it.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sprag-gate-hook-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create a scratch directory");
    dir
}

/// The real `grep`, found the way a shell finds it.
///
/// Resolved rather than hardcoded because the double below has to DELEGATE: `commit-msg` calls grep
/// four more times for rules that have nothing to do with PCRE, and a double that broke those would
/// be staging a different failure than the one under test.
fn real_grep() -> PathBuf {
    let path = std::env::var_os("PATH").expect("a PATH to find grep on");
    std::env::split_paths(&path)
        .map(|dir| dir.join("grep"))
        .find(|candidate| candidate.is_file())
        .expect("grep on PATH — every rule in the hook under test is implemented with it")
}

/// A PATH directory holding a `grep` that answers `-P` the way BSD grep does: a complaint on
/// stderr and exit 2. Everything else is handed to the real one.
///
/// ⚠⚠ This is the macOS default grep, standing in for a machine this suite does not run on. The
/// alternative — believing the manual page — is the assumption item 403 was hiding behind.
fn grep_without_pcre(dir: &Path) -> PathBuf {
    let bin = dir.join("grep");
    let mut file = std::fs::File::create(&bin).expect("write the grep double");
    writeln!(
        file,
        "#!/usr/bin/env bash\n\
         # A grep with no PCRE, as BSD grep and busybox answer.\n\
         for arg in \"$@\"; do\n\
         \x20   case \"$arg\" in\n\
         \x20       --) break ;;\n\
         \x20       -*P*) echo \"grep: invalid option -- P\" >&2; exit 2 ;;\n\
         \x20   esac\n\
         done\n\
         exec {} \"$@\"",
        real_grep().display(),
    )
    .expect("write the grep double");
    drop(file);
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755))
        .expect("make the grep double executable");
    dir.to_path_buf()
}

/// How this run's grep is chosen.
enum Grep {
    /// The developer's own — GNU grep on the machines this suite runs on.
    AsInstalled,
    /// The macOS default, standing in.
    WithoutPcre,
}

/// Put `message` in a file and hand it to the hook, exactly as git does.
fn judge(tag: &str, message: &str, grep: Grep) -> Output {
    let dir = scratch(tag);
    let msg_file = dir.join("COMMIT_EDITMSG");
    std::fs::write(&msg_file, message).expect("write the message under test");

    let mut command = Command::new(hook());
    command.arg(&msg_file);
    if let Grep::WithoutPcre = grep {
        let shim = grep_without_pcre(&dir);
        let inherited = std::env::var_os("PATH").unwrap_or_default();
        let mut dirs = vec![shim];
        dirs.extend(std::env::split_paths(&inherited));
        command.env(
            "PATH",
            std::env::join_paths(dirs).expect("a PATH with the double in front"),
        );
    }
    let run = command.output().unwrap_or_else(|why| {
        panic!(
            "{} is the hook under test and it must be executable: {why}",
            hook().display()
        )
    });
    let _ = std::fs::remove_dir_all(&dir);
    run
}

fn said(run: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    )
}

/// A message this project's own format rules accept.
const WELL_FORMED: &str = "fix(gate): a hook is driven as the program it is\n\
                           \n\
                           - the commit-msg hook now has a gate that runs it\n";

/// The same message with a party-popper (U+1F389) in its bullet.
///
/// ⚠ Written as an escape rather than the character, because a literal emoji in source is the very
/// thing this project forbids — and the codepoint is what the hook's range is stated in anyway.
const CARRIES_AN_EMOJI: &str = "fix(gate): a hook is driven as the program it is\n\
                                \n\
                                - \u{1F389} the commit-msg hook now has a gate\n";

/// The same message with two Hangul syllables (U+D55C U+AE00) in its bullet.
const CARRIES_NON_ENGLISH: &str = "fix(gate): a hook is driven as the program it is\n\
                                   \n\
                                   - \u{D55C}\u{AE00} the commit-msg hook now has a gate\n";

/// ⚠⚠⚠ **THE CONTROL, AND IT IS NOT A FORMALITY.** Every other case here asserts a REFUSAL, and a
/// hook that refused everything would satisfy all of them while making the repository
/// uncommittable. This is the half that says the gate can go green.
#[test]
fn a_message_that_follows_the_format_is_accepted() {
    let run = judge("well-formed", WELL_FORMED, Grep::AsInstalled);
    assert!(
        run.status.success(),
        "a well-formed message must pass, or the gates below prove nothing: {}",
        said(&run),
    );
}

#[test]
fn a_message_carrying_an_emoji_is_refused_and_the_rule_is_named() {
    let run = judge("emoji", CARRIES_AN_EMOJI, Grep::AsInstalled);
    assert!(
        !run.status.success(),
        "an emoji is forbidden by COMMIT_FORMAT.md and the hook must say so: {}",
        said(&run),
    );
    let told = said(&run);
    assert!(
        told.contains("Emoji"),
        "the refusal must name the rule it enforced, not merely fail: {told}",
    );
}

#[test]
fn a_message_carrying_a_non_english_line_is_refused_and_the_line_is_shown() {
    let run = judge("non-english", CARRIES_NON_ENGLISH, Grep::AsInstalled);
    assert!(
        !run.status.success(),
        "a non-English body is forbidden by COMMIT_FORMAT.md: {}",
        said(&run),
    );
    let told = said(&run);
    assert!(
        told.contains("Non-English"),
        "the refusal must name the rule: {told}",
    );
    assert!(
        told.contains("3:"),
        "and point at the offending LINE, which is the only part a person can act on: {told}",
    );
}

/// ⚠⚠⚠⚠ **THE FIXTURE ASSERTS ITS OWN STAGING** — item 384's lesson, which cost this project a
/// timing gate that measured something else for rounds.
///
/// The two cases below claim *"the hook refuses when its tool cannot do the job"*. They would pass
/// just as well if the double were simply BROKEN — unwritten, not executable, delegating nowhere —
/// and then they would be asserting nothing about PCRE at all. So the double is measured first: an
/// ordinary POSIX `grep -E` still works through it, and only `-P` is gone.
///
/// ⚠⚠⚠⚠ **MEASURED, NOT FEARED.** Pointing the double's `exec` at a name that does not exist was
/// run: `a_style_rule_whose_only_tool_is_missing_refuses_rather_than_passing` **still passed** —
/// the hook died at its FIRST grep, four rules earlier, and a refusal is a refusal to an assertion
/// that only reads the status. This case is what went red, and it is the reason it exists.
#[test]
fn the_double_removes_pcre_and_nothing_else() {
    let dir = scratch("double");
    let shim = grep_without_pcre(&dir);
    let grep = shim.join("grep");

    let ere = Command::new(&grep)
        .args(["-q", "-E", "x"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .and_then(|mut child| {
            child
                .stdin
                .take()
                .expect("the double's stdin")
                .write_all(b"x\n")?;
            child.wait()
        })
        .expect("run the double with a POSIX pattern");
    assert!(
        ere.success(),
        "the double must still be a working grep for every rule that is not PCRE, \
         or the cases below are staging a different failure",
    );

    let pcre = Command::new(&grep)
        .args(["-q", "-P", "x"])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run the double with a PCRE pattern");
    assert_eq!(
        pcre.status.code(),
        Some(2),
        "and it must refuse -P the way BSD grep does: {}",
        String::from_utf8_lossy(&pcre.stderr),
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// ⚠⚠⚠⚠ **ITEM 403's GATE.** Before the fix this returned **exit 0, silently**: the `2>/dev/null`
/// swallowed grep's complaint, the `if` read FALSE, and a message carrying a forbidden character
/// walked straight through the rule that forbids it.
#[test]
fn a_style_rule_whose_only_tool_is_missing_refuses_rather_than_passing() {
    let run = judge("no-pcre-bad", CARRIES_AN_EMOJI, Grep::WithoutPcre);
    assert!(
        !run.status.success(),
        "with no PCRE the emoji rule cannot run, and a rule that cannot run must REFUSE \
         rather than let the message it was written to stop go by: {}",
        said(&run),
    );
}

/// ⚠⚠⚠ **AND THE REFUSAL IS ABOUT CAPABILITY, NOT CONTENT** — the same clean message that passes
/// above is refused here, and the refusal names the tool a person has to install.
///
/// ⚠⚠ This is the case the sibling above cannot make on its own: a hook that happened to refuse
/// every message under the double would satisfy it while saying nothing about WHY.
#[test]
fn a_hook_that_cannot_run_its_rules_says_which_tool_is_missing() {
    let run = judge("no-pcre-clean", WELL_FORMED, Grep::WithoutPcre);
    assert!(
        !run.status.success(),
        "a hook that cannot enforce its rules must not accept the message anyway: {}",
        said(&run),
    );
    let told = said(&run);
    assert!(
        told.contains("-P"),
        "the refusal must name the missing capability: {told}",
    );
    assert!(
        told.contains("grep"),
        "and the tool that carries it, since installing it is the whole remedy: {told}",
    );
}
