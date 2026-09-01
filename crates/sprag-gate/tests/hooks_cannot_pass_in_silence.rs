//! **THE FOUR SHAPES IN WHICH A HOOK SAYS NOTHING** — register item 404, second payment; the
//! fourth arrived with register item 792.
//!
//! # ⚠⚠⚠ Why this file exists
//!
//! Both silent-pass defects this project has found in its own hooks were found by a PERSON READING
//! the files. Nothing ran, nothing went red, and each had been sitting there for many commits:
//!
//!   * `cargo clippy` in `pre-push` with no `-D warnings`, so the push gate PRINTED lint findings
//!     and exited 0 — the flag its sibling had gained twelve commits earlier (item 213), and
//!   * `mnemosyne-cli validate-code-refs >/dev/null` in both hooks, discarding 52 violations so
//!     that the one number showing them drift was visible to nobody (item 213 again).
//!
//! [`hooks_enforce_what_they_check`](../hooks_enforce_what_they_check.rs) drives one hook's
//! BEHAVIOUR. This file is the other half: a ratchet over the SHAPES, so the next instance of the
//! class is caught by a machine at the commit that writes it rather than by somebody's eye
//! afterwards. It reads every file in `.githooks/` by directory walk and not from a list, because a
//! list would silently exclude the hook somebody adds next.
//!
//! # ⚠⚠ What a scan cannot claim, said plainly so a green run is not misread
//!
//! This is a LINE SCAN. It does not understand bash: it cannot tell that a checker's exit status is
//! consulted, that a variable holds a command, or that a branch is reachable. It answers three
//! narrow questions about the text, and **a green run means those three shapes are absent — not
//! that the hooks are correct.** The same caveat `ci_builds_what_it_drives` states about `ci.yml`,
//! for the same reason: this crate takes no dependencies by charter, so there is no parser here.

use std::path::PathBuf;
use std::process::Command;

/// The tree this gate is part of — through the one door, register item 809.
///
/// ⚠ A private `env!("CARGO_MANIFEST_DIR")` walk answers about the tree this test was COMPILED in,
/// which stopped being the tree it runs in. `workspace_root` refuses when the two differ.
fn repo_root() -> PathBuf {
    sprag_gate::sources::workspace_root()
}

/// Every file this repository installs as `core.hooksPath`, as `(name, text)`.
///
/// ⚠⚠ Found by WALKING the directory. A hardcoded list is the failure this whole item is about one
/// level up: it decides alone which files are looked at, and the one it leaves out is exactly the
/// one nobody is watching.
fn hook_files() -> Vec<(String, String)> {
    let dir = repo_root().join(".githooks");
    let mut found: Vec<(String, String)> = std::fs::read_dir(&dir)
        .unwrap_or_else(|why| panic!("{} is this repo's hooks: {why}", dir.display()))
        .map(|entry| entry.expect("read a hook directory entry").path())
        .filter(|path| path.is_file())
        .map(|path| {
            let name = path
                .file_name()
                .expect("a hook has a name")
                .to_string_lossy()
                .into_owned();
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|why| panic!("{} is a hook and must be text: {why}", name));
            (name, text)
        })
        .collect();
    assert!(
        !found.is_empty(),
        "{} held no hooks — this gate would then be asserting nothing",
        dir.display(),
    );
    found.sort();
    found
}

/// The lines of a hook that are CODE: comments carry the reasoning, and this crate's own hooks
/// quote their commands in prose, so scanning them would be scanning documentation.
fn code_lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines()
        .enumerate()
        .map(|(index, line)| (index + 1, line.trim()))
        .filter(|(_, line)| !line.is_empty() && !line.starts_with('#'))
}

/// ⚠⚠⚠⚠ **THE FLAG WITHOUT WHICH CLIPPY IS A PRINTER.** Without `-D warnings` the line reports its
/// findings and exits 0, which is how two `type_complexity` and one `manual_contains` rode through
/// several commits — and how `pre-push` went on doing it for twelve more after `pre-commit` was
/// fixed, since nothing compared the two.
#[test]
fn every_hook_that_runs_clippy_makes_it_deny() {
    let mut silent: Vec<String> = Vec::new();
    for (name, text) in hook_files() {
        for (number, line) in code_lines(&text) {
            if line.contains("cargo clippy") && !line.contains("-D warnings") {
                silent.push(format!(".githooks/{name}:{number}: {line}"));
            }
        }
    }
    assert!(
        silent.is_empty(),
        "clippy without `-D warnings` PRINTS its findings and exits 0, so the gate says \
         something and then lets it through:\n{}",
        silent.join("\n"),
    );
}

/// The checks whose whole value is what they SAY. Named rather than inferred, because a scan cannot
/// tell a checker from any other command.
const CHECKERS: &[&str] = &[
    "validate-code-refs",
    "validate-workspace",
    "cargo clippy",
    "cargo test",
    "rustfmt",
    "actionlint",
    "doc_gate",
];

/// ⚠⚠⚠⚠ **A CHECK NOTHING READS ACCUMULATES.** `validate-code-refs` was redirected to `/dev/null`
/// in two hooks while it had 52 violations to report; the exit code was right, and the number that
/// would have shown it drifting reached nobody. A hook may DECIDE on the status — that is a
/// configuration question — but throwing away the report is not a decision, it is a loss.
///
/// ⚠⚠ **THE `command -v` EXCLUSION IS MEASURED, NOT ASSUMED.** Run without it, this gate reds
/// `pre-commit`'s `command -v actionlint >/dev/null 2>&1` — an availability PROBE, whose output is
/// noise by construction and whose absence is already reported in a word two lines below it. That
/// is the one line in today's hooks that matches, and it is the reason the exclusion exists.
#[test]
fn no_hook_throws_away_what_a_checker_told_it() {
    let mut discarded: Vec<String> = Vec::new();
    for (name, text) in hook_files() {
        for (number, line) in code_lines(&text) {
            if line.contains("command -v") || !line.contains("/dev/null") {
                continue;
            }
            if let Some(checker) = CHECKERS.iter().find(|checker| line.contains(**checker)) {
                discarded.push(format!(".githooks/{name}:{number}: {checker} — {line}"));
            }
        }
    }
    assert!(
        discarded.is_empty(),
        "a checker's report is the one thing a person needs when it refuses, and \
         `/dev/null` is where it went:\n{}",
        discarded.join("\n"),
    );
}

/// ⛔⛔⛔⛔⛔ **A HARNESS THAT CANNOT GET ITS SCRATCH MUST NOT GO ON** — register item 792, and the
/// fourth shape of saying nothing: the other three let a VERDICT vanish, and this one lets the
/// harness carry on **against the caller's own repository** with no verdict involved at all.
///
/// # ⚠⚠⚠⚠⚠ Measured, in this repository, on 2026-08-31
///
/// `hosted-read.sh --selftest` opened with `tmp="$(mktemp -d)"` and nothing read the status. A
/// `local PATH` inside that function started the variable EMPTY — `local` initialises, it does not
/// save — so `mktemp` was not on PATH, exited 127, and `$tmp` became the empty string. Every line
/// below then ran against the REAL clone, in this order: `mkdir -p "$tmp/bin"` became
/// `mkdir -p /bin`; `git -C "$tmp" init` re-initialised this repository; `git -C "$tmp" config
/// user.email probe@example.com` wrote a `[user]` section into its `.git/config`, replacing the
/// operator's identity on a tree whose next commit would have carried it; and the arms overwrote
/// the operator's own marker file, advancing its watermark onto a commit whose run had not spoken.
///
/// **Nothing warned.** It was found because an unrelated `git add` printed four empty files the
/// harness had left staged.
///
/// # ⛔⛔ Why an empty scratch is not a harmless empty string
///
/// `git -C ""` is read by git as *stay where you are*, so a scratch command silently becomes a
/// command against the caller's repository. `GIT_INDEX_FILE=""` is read as *unset*, which is the
/// REAL index — so `content-gate.sh`'s scratch index, unchecked, would have had `git read-tree`
/// overwrite whatever the operator had staged. And `--prefix="$mirror/"` collapses to
/// `--prefix="/"`, which is `checkout-index` writing the tree at the filesystem root.
///
/// # ⚠⚠ What is required, and what is deliberately NOT
///
/// The status must be read **in the same statement** (`|| …`). That is the narrowest thing that
/// catches what actually happened: a missing `mktemp` exits 127, and a `||` on the assignment ends
/// it there. A stronger guard below the assignment is welcome — `hosted-read.sh` keeps one that
/// also asks whether the path is a directory — but it may not be the ONLY thing, because the
/// statement that assigns is the one place no later edit can drift away from.
///
/// ⚠⚠ **THE RUST SIDE CARRIES A DIFFERENT, WEAKER SHAPE — AND WHAT WAS FIRST WRITTEN HERE WAS
/// WRONG.** This paragraph began as *`std::env::temp_dir()` returns a path unconditionally and
/// cannot yield the empty string*, which was prose nobody had measured. Measured with `rustc` on
/// 2026-08-31: with `TMPDIR` merely SET AND EMPTY it answers `""`, and `join` then yields a
/// RELATIVE path. So the 51 Rust harnesses CAN lose their scratch.
///
/// What follows from it is not the same hazard, which is why they are not in this population: a
/// relative path makes a directory under the CURRENT one, which is then removed — nothing takes the
/// caller's repository as its SUBJECT the way `git -C ""` and `GIT_INDEX_FILE=""` do. It is
/// recorded on its own axis rather than folded in here, because folding two hazards into one gate
/// is how the weaker one sets the bar. (And `sprag-gate`'s `Sandbox` pins `core.hooksPath` against
/// a THIRD thing again: its own commits running the real hooks.)
///
/// ⚠ **THE CONDITION IS ONE THIS REPOSITORY DOES NOT MAKE** — `git grep -n TMPDIR` over the whole
/// tracked tree finds 22 mentions outside item 794's own files, and every one of them is PROSE:
/// comments about macOS's `/var/folders` symlink, and reasoning that names the variable while
/// explaining it. Nothing here assigns it. ⛔ The first draft of this line ran
/// `grep -rn TMPDIR .github/ .githooks/ scripts/`, and **`scripts/` does not exist in this
/// repository** — an emptiness that was partly its own missing argument. Said as measured, not as
/// safety: *this repository does not make the condition* is a smaller claim than *it cannot
/// happen*, and the difference is the whole reason the next paragraph exists.
///
/// ⛔⛔ **AND WHERE IT WOULD LAND, IT IS NOT WEAK.** `sprag_host::checkout::IsolatedCheckout::of`
/// takes its temporary root from `temp_dir()`, and `remote_access`'s caller doc promises that root
/// is *not a directory of the repository's* — because a copy inside the tree being copied is
/// litter a checker wanders into, which is register item 705's confusion re-created by the repair
/// for it. Measured 2026-08-31 in a throwaway repository: `git -C <repo> worktree add --detach -q
/// sprag-check-probe HEAD` **exits 0** and leaves `?? sprag-check-probe/` INSIDE the repository,
/// because git resolves a relative path against its own `-C`. So that site now refuses a
/// non-absolute root itself, and its own test drives it — which is why the Rust side stays out of
/// THIS gate rather than being waved through: it is guarded where the harm is, on its own terms.
///
/// ⚠ So the population this walk reaches is the SHELL harnesses — and it reaches the whole of it,
/// which is what makes zero a number this gate can actually arrive at.
#[test]
fn no_hook_uses_a_scratch_it_never_checked_it_got() {
    let mut unchecked: Vec<String> = Vec::new();
    for (name, text) in hook_files() {
        for (number, line) in code_lines(&text) {
            // ⚠ The CALL, not the word: these hooks name `mktemp` in their own refusal messages,
            // and a scan that matched those would red on the very sentence this gate wants written.
            if !line.contains("$(mktemp") && !line.contains('`') {
                continue;
            }
            if line.contains('`') && !line.contains("`mktemp") {
                continue;
            }
            if line.contains("||") {
                continue;
            }
            unchecked.push(format!(".githooks/{name}:{number}: {line}"));
        }
    }
    assert!(
        unchecked.is_empty(),
        "⛔ ITEM 792: a scratch was taken and nothing read whether it arrived. `mktemp` exits 127 \
         when it is not on PATH, the variable is then the empty string, and `git -C \"\"`, \
         `GIT_INDEX_FILE=\"\"` and `--prefix=\"/\"` all read that as the CALLER'S OWN repository or \
         filesystem root. Put the check in the same statement — `x=\"$(mktemp -d)\" || return \
         1`:\n{}",
        unchecked.join("\n"),
    );
}

/// What a fresh clone would get, as git records it — not what this developer's filesystem happens
/// to say. The two can differ, and it is the recorded bit that decides on everybody else's machine.
fn recorded_modes() -> Vec<(String, String)> {
    let listing = Command::new("git")
        .args(["ls-files", "-s", ".githooks/"])
        .current_dir(repo_root())
        .output()
        .expect("git on PATH — the recorded mode is the subject and only git knows it");
    assert!(
        listing.status.success(),
        "git could not list this repo's hooks: {}",
        String::from_utf8_lossy(&listing.stderr),
    );
    let text = String::from_utf8(listing.stdout).expect("git speaks utf-8 here");
    let rows: Vec<(String, String)> = text
        .lines()
        .filter_map(|line| {
            let mode = line.split_whitespace().next()?;
            let path = line.rsplit('\t').next()?;
            let name = path.rsplit('/').next()?;
            Some((name.to_owned(), mode.to_owned()))
        })
        .collect();
    assert!(
        !rows.is_empty(),
        "git recorded no hooks, so this gate would be asserting nothing",
    );
    rows
}

/// ⚠⚠⚠⚠ **A HOOK WITHOUT ITS EXECUTABLE BIT IS SKIPPED BY GIT WITHOUT A WORD** — the same class as
/// the two defects above, one layer down: not a gate that fails to enforce, but a gate that never
/// runs at all, on every clone but the one where the file was written.
///
/// So each file is one of exactly two things, and this says which: EXECUTABLE (git invokes it), or
/// SOURCED by one that is (a library, like `doc-gate.sh`). A file that is neither is either a hook
/// nobody will run or a fragment nobody reads, and both are worth a red.
#[test]
fn every_hook_is_either_executable_or_sourced_by_one_that_is() {
    let files = hook_files();
    let modes = recorded_modes();
    let mut orphaned: Vec<String> = Vec::new();

    for (name, _) in &files {
        let mode = modes
            .iter()
            .find(|(recorded, _)| recorded == name)
            .map(|(_, mode)| mode.as_str())
            .unwrap_or_else(|| panic!(".githooks/{name} is untracked — a clone would not have it"));
        if mode.ends_with("755") {
            continue;
        }
        let sourced = files.iter().any(|(other, text)| {
            other != name
                && code_lines(text).any(|(_, line)| {
                    (line.starts_with(". ") || line.starts_with("source "))
                        && line.contains(name.as_str())
                })
        });
        if !sourced {
            orphaned.push(format!(".githooks/{name} (mode {mode})"));
        }
    }

    assert!(
        orphaned.is_empty(),
        "git SILENTLY SKIPS a hook without its executable bit, and nothing reads a library \
         nobody sources — either way the file's checks do not happen:\n{}",
        orphaned.join("\n"),
    );
}
