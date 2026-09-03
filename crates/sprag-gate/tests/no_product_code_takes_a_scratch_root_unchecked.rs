//! ⛔⛔⛔⛔⛔ **PRODUCT CODE DOES NOT ASK THE OPERATING SYSTEM FOR A SCRATCH ROOT AND THEN TRUST
//! THE ANSWER** — register item 794.
//!
//! # What `std::env::temp_dir()` does, measured rather than assumed
//!
//! It answers a RELATIVE path when `TMPDIR` is set-and-empty. Measured 2026-08-31 with `rustc`:
//!
//! ```text
//! TMPDIR unset : temp_dir="/tmp"  joined="/tmp/sprag-probe"  absolute=true
//! TMPDIR=      : temp_dir=""      joined="sprag-probe"       absolute=false
//! ```
//!
//! Nothing downstream refuses the result. `git -C <repo> worktree add --detach -q
//! sprag-check-probe HEAD` exits **0** and leaves `?? sprag-check-probe/` INSIDE the repository,
//! because git resolves a relative path against its own `-C`. `create_dir_all` and `File::create`
//! are the same. So a socket, a checkout or a run directory silently lands in whatever directory
//! the process was launched from, and every reader that resolves the same name from somewhere else
//! looks in a different place.
//!
//! # ⚠⚠ WHY THIS IS A SECOND AXIS RATHER THAN A WIDER `hooks_cannot_pass_in_silence`
//!
//! Item 794's own done-when asks that question before answering it, so it was measured:
//!
//! | | that gate (shell) | this one (Rust) |
//! |---|---|---|
//! | population | `.githooks/`: 7 files, 12 `mktemp` lines | product `.rs`: `temp_dir()` calls |
//! | predicate | did the taker read the status | is the root usable where it is taken |
//!
//! The intersection is EMPTY, re-measured 2026-08-31: `.githooks/` holds **zero** `temp_dir` across
//! all 7 of its files, and every `mktemp` under `crates/` sits in a comment or a string literal —
//! that gate's own filter and refusal text, and this paragraph — so **no Rust code calls it**. Its
//! population comes from a directory WALK of `.githooks`, which cannot reach a crate. Widening its
//! name would have made both the file name (`hooks_…`) and the test name (`no_hook_…`) false.
//!
//! # ⛔⛔⛔⛔⛔ WHAT THE FIRST DRAFT OF THIS FILE GOT WRONG, AND HOW IT WAS FOUND
//!
//! It split product from harness on `#[cfg(test)]` alone, and wrote in its own doc that the split
//! came out **8 product / 163 test**. Re-derived before the gate had ever been executed — the run
//! that would have run it died earlier, at `cargo check --locked` — the same predicate answers
//! **77 product**, because `#[cfg(test)]` is a marker of an inline module in `src/`, and an
//! INTEGRATION test has no such marker: every one of `crates/*/tests/*.rs` is test code from its
//! first line and this file called all 74 of those call sites product. The gate would have been
//! red on arrival, at 77 sites of which 76 are not product at all — and three of the 77 were **this
//! file's own filter string**.
//!
//! ⭐ Two lessons, both already written in this repository's register and both re-earned here:
//! prose that nobody re-runs is not evidence (the `8 / 163` was prose), and a gate that has not
//! been EXECUTED has not been measured, however carefully it was read.
//!
//! ⇒ So the split is by CARGO TARGET, which is what actually decides whether a line ships:
//! `src/**` and `build.rs` are compiled into the crate; `tests/`, `benches/` and `examples/` are
//! built only by `--all-targets` and linked into nothing a user runs. Inside a product file,
//! `#[cfg(test)]` still separates the inline harness. **Anything that matches none of those shapes
//! is classified PRODUCT**, so a layout nobody anticipated arrives here as a red line to read
//! rather than as a silent pass.
//!
//! # ⚠⚠⚠ AND WHY THE POPULATION IS PRODUCT CODE RATHER THAN EVERY CALL
//!
//! Rule 5 — is there a path by which this reaches zero? The harness half cannot reach zero in one
//! round: it is 163 call sites whose remedy is a different one (they litter the repository under
//! `TMPDIR=` — a suite run that way on 2026-08-31 left **131** untracked entries under `crates/*/`
//! and failed **414** tests, against 0 and 0 for the same tree with a normal `TMPDIR`). A
//! population that cannot reach zero in one round makes a gate that is red forever and therefore
//! read by nobody, so that half is registered as its own item — and it is COUNTED here rather than
//! waved through, by [`the_harness_half_cannot_grow_without_being_read`], because an exemption
//! nobody measures is how the population grows back.

use std::path::{Path, PathBuf};

use sprag_gate::sources::outside_strings;

/// ⛔ **The harness half, as it stood when item 794's product half reached zero.**
///
/// ⛔⛔ **HELD EXACTLY, NOT AS A CEILING.** A ceiling rots: pay ten sites down and the slack it
/// leaves admits ten new ones with nothing going red — which is this repository's own lesson that a
/// blind ratchet is green forever. Equality makes a move in EITHER direction a line somebody edits
/// and a reason somebody reads. Item 795's done-when is to walk this to 0 and delete the constant
/// and its test along with it.
///
/// ⚠ **ASKED OF THE GATE, NOT DERIVED BY A SECOND SCRIPT.** A first draft of this line said 165,
/// from an `awk` that blanked quoted spans with `"[^"]*"` and so stopped at the first ESCAPED quote
/// in this file's own `"…\"env::temp_dir()\"…"`. The number here is what
/// [`the_harness_half_cannot_grow_without_being_read`] printed when the ceiling was set to zero on
/// purpose: a ceiling with slack in it is an exemption that can grow silently, which is the one
/// thing this constant exists to prevent.
///
/// # 163 → 161, 2026-09-01, register item 802 paying down item 795
///
/// `sprag-tui`'s PTY gate took both of its roots from the operating system, and one of them became
/// the `XDG_STATE_HOME` handed to every daemon, client and CLI run that file spawns. Under
/// `TMPDIR=` that home is relative — which the daemon must IGNORE — so the file's isolation was
/// undone silently and 67 daemons persisted into the tester's own state home. Converted to
/// `sprag_scratch::scratch_root()`, which refuses the root where it is taken.
///
/// ⚠ AND THE NUMBER MOVED BY ONE LESS THAN THE CONVERSION, WHICH IS WORTH THE SENTENCE: a new
/// assertion message in `sprag-host` spelled the call inside a `\`-continued string literal, and
/// [`outside_strings`] keeps a line that ENDS inside a string as code — deliberately, in the
/// direction of a red to read. The prose was rephrased rather than the filter widened; naming the
/// call in words costs nothing and an exemption for "it was only a message" costs the gate.
/// ⚠ AND BY ONE ON 2026-09-03: register item 871's gate spawns a daemon of its own, so it takes a
/// state root like every other daemon gate here. It wanted a SECOND root as well — a directory to
/// stand a fake agent binary in — and that one was folded under the first instead, so the gate
/// costs this population one site rather than two and `DaemonGuard` takes both away together. The
/// ratchet is what asked the question; the answer was to litter less, not to record more.
const HARNESS_SITES_REGISTERED: usize = 162;

/// The tree this ratchet counts — through the one door, register item 809.
///
/// ⚠⚠ IT MATTERS MOST HERE. This gate's whole verdict is an EQUALITY against
/// [`HARNESS_SITES_REGISTERED`], so a walk of the wrong tree does not merely mis-report: it is the
/// one artefact that could answer *which tree was walked* and it would be answering about a tree
/// nobody asked for. That equality is what proved, after the fact, that the 2026-09-01 rounds had
/// judged this workspace — the number 161 exists in no other tree — and the proof only works while
/// the walk goes through the door that checks.
fn repo_root() -> PathBuf {
    sprag_gate::sources::workspace_root()
}

/// Whether a file's lines are compiled into something a user runs.
///
/// ⛔⛔⛔⛔ **RULE 6 LIVES HERE.** This is the only thing that can excuse a call site, so it is
/// deliberately wrong in the direction that costs a false RED: every shape it does not recognise
/// is [`Where::Product`]. A crate laid out some way this workspace has never used arrives as a line
/// in the failure message — a person reading one path — instead of a call site riding through.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Where {
    /// Compiled into the crate: `src/**`, and `build.rs`, which runs on whoever is building.
    Product,
    /// Built only by `--all-targets` and linked into nothing shipped: `tests/`, `benches/`,
    /// `examples/`.
    Harness,
}

/// Classify a repo-relative `crates/…` path by the Cargo target it belongs to.
fn where_it_lives(rel: &str) -> Where {
    // `crates` / `<crate>` / `<target-dir-or-file>` / …
    match Path::new(rel).components().nth(2).map(|c| c.as_os_str()) {
        Some(dir) if dir == "tests" || dir == "benches" || dir == "examples" => Where::Harness,
        _ => Where::Product,
    }
}

/// Every tracked `.rs` file under `crates/`, as `(repo-relative path, text)`.
///
/// ⚠⚠ Found by WALKING, for the reason the shell gate walks `.githooks/`: a hardcoded list decides
/// alone which files are looked at, and the one it leaves out is exactly the one nobody is
/// watching. A crate added tomorrow is in this population without anyone remembering to add it.
fn rust_files() -> Vec<(String, String)> {
    let root = repo_root();
    let mut found = Vec::new();
    let mut stack = vec![root.join("crates")];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir)
            .unwrap_or_else(|why| panic!("{} must be readable: {why}", dir.display()));
        for entry in entries {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                // `target` is build output, not source anybody wrote.
                if path.file_name().is_some_and(|name| name == "target") {
                    continue;
                }
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let text = std::fs::read_to_string(&path)
                    .unwrap_or_else(|why| panic!("{} must be text: {why}", path.display()));
                let rel = path
                    .strip_prefix(&root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .into_owned();
                found.push((rel, text));
            }
        }
    }
    assert!(
        !found.is_empty(),
        "crates/ held no Rust files — this gate would then be asserting nothing",
    );
    found.sort();
    found
}

/// The lines that are CODE. Comments carry the reasoning, and this repository's reasoning quotes
/// the very call being hunted — four doc lines name `std::env::temp_dir()` while explaining why it
/// is dangerous, and scanning those would red on the documentation that justifies this gate.
fn code_lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines()
        .enumerate()
        .map(|(index, line)| (index + 1, line.trim()))
        .filter(|(_, line)| {
            !line.is_empty()
                && !line.starts_with("//")
                && !line.starts_with("///")
                && !line.starts_with("//!")
                && !line.starts_with('*')
        })
}

// ⚠⚠ `outside_strings` — *a code line with its double-quoted strings blanked out* — was written
// here and now lives in [`sprag_gate::sources`], imported above. It moved the day a SECOND gate
// needed it (register item 818, the append-in-one-call scan), which is the moment a copy would have
// been made and the two would have begun to drift. Its own reason is on it, there.

/// Where a file's `#[cfg(test)]` module begins, if it has one.
///
/// ⚠ This separates the INLINE harness inside a product file. It is not what tells a test file from
/// a product one — [`where_it_lives`] is, and the first draft of this gate confusing the two is
/// what made it red at 77 sites. The answer here is conservative the same way: the FIRST
/// `#[cfg(test)]`, so a module spelled some other way leaves its calls classified as product.
fn test_module_starts_at(text: &str) -> Option<usize> {
    text.lines()
        .enumerate()
        .find(|(_, line)| line.trim().starts_with("#[cfg(test)]"))
        .map(|(index, _)| index + 1)
}

/// Every `env::temp_dir()` call site under `crates/`, as `(Where, "path:line: text")`.
///
/// The seam is not in here at all: `sprag-scratch` is the one crate that may make the call, because
/// it is the one that checks the answer.
fn call_sites() -> Vec<(Where, String)> {
    let mut sites = Vec::new();
    for (name, text) in rust_files() {
        if Path::new(&name).starts_with("crates/sprag-scratch") {
            continue;
        }
        let inline_harness_from = test_module_starts_at(&text);
        let file = where_it_lives(&name);
        for (number, line) in code_lines(&text) {
            let code = outside_strings(line);
            if !code.contains("env::temp_dir()") {
                continue;
            }
            let placed = match file {
                Where::Harness => Where::Harness,
                Where::Product if inline_harness_from.is_some_and(|start| number > start) => {
                    Where::Harness
                }
                Where::Product => Where::Product,
            };
            sites.push((placed, format!("{name}:{number}: {line}")));
        }
    }
    sites
}

/// ⛔ **THE GATE.** No product line takes a scratch root from the operating system directly.
///
/// The one place that may is `sprag-scratch`, which asks the question this gate exists to enforce
/// — `is_absolute` — and panics naming `TMPDIR` when the answer is no. That exemption is a single
/// crate, named in [`call_sites`], and its own tests drive both arms (`an_empty_root_is_refused`,
/// `a_relative_root_is_refused_by_the_same_question`).
#[test]
fn no_product_code_takes_a_scratch_root_unchecked() {
    let sites = call_sites();
    let unchecked: Vec<&str> = sites
        .iter()
        .filter(|(placed, _)| *placed == Where::Product)
        .map(|(_, site)| site.as_str())
        .collect();
    assert!(
        unchecked.is_empty(),
        "⛔ ITEM 794: product code took a scratch root from the operating system and trusted it. \
         `std::env::temp_dir()` answers a RELATIVE path when `TMPDIR` is set-and-empty, and \
         nothing downstream refuses one — `create_dir_all`, `File::create` and `git worktree add` \
         all succeed against the process's own working directory, silently. Call \
         `sprag_scratch::scratch_root()` instead: it asks `is_absolute` where the root is taken \
         and panics naming the variable when it cannot be used:\n{}",
        unchecked.join("\n"),
    );
}

/// ⛔⛔ **THE EXEMPTION IS COUNTED, BECAUSE AN EXEMPTION NOBODY MEASURES IS HOW A POPULATION GROWS
/// BACK** — rule 6.
///
/// The gate above reaches zero only because the harness half is out of its population. That half is
/// real: under `TMPDIR=` every one of these sites writes into whatever directory `cargo test` stood
/// its binary in, which is the crate's own directory inside this repository. Measured 2026-08-31 —
/// 131 untracked entries left under `crates/*/`, 414 tests failed, against 0 and 0 for the same
/// tree with a normal `TMPDIR`.
///
/// A move in either direction is a red: a new test calling `std::env::temp_dir()` is a new place
/// the suite litters, and a site converted away is item 795 being paid down. Both are worth one
/// line of somebody's attention, and neither is worth a number that quietly stops being true.
#[test]
fn the_harness_half_cannot_grow_without_being_read() {
    let harness = call_sites()
        .into_iter()
        .filter(|(placed, _)| *placed == Where::Harness)
        .count();
    assert_eq!(
        harness, HARNESS_SITES_REGISTERED,
        "⛔ ITEM 794's harness half moved: {harness} call sites take a scratch root from the \
         operating system in test, bench and example code, against {HARNESS_SITES_REGISTERED} \
         recorded when the product half reached zero. Each one writes into the crate's own \
         directory inside this repository when `TMPDIR` is set-and-empty. If it GREW, call \
         `sprag_scratch::scratch_root()` from the new site instead of the bare std call. If it \
         SHRANK, that is item 795 being paid down: set this constant to {harness} and say so in \
         the register. It is held exactly rather than as a ceiling so the number cannot rot into \
         slack that admits new sites in silence",
    );
}

/// ⚠⚠⚠ **THE MACHINERY REACHES THE CODE, AND IT ANSWERS BOTH WAYS.**
///
/// The gate above passes by finding nothing. So would a walk that reached no files, a `code_lines`
/// that filtered everything away, an `outside_strings` that blanked whole lines, or a classifier
/// that called everything [`Where::Harness`] — and a version of each has happened to a gate in this
/// repository. This test fails if any of them stops working, which is the one failure a green
/// cannot tell apart from success.
#[test]
fn the_walk_reaches_this_workspace_and_the_classifier_answers_both_ways() {
    let files = rust_files();
    assert!(
        files.len() > 100,
        "the walk found only {} Rust files under crates/ — the gate beside this one would pass on \
         an empty population",
        files.len(),
    );

    let product = files
        .iter()
        .filter(|(name, _)| where_it_lives(name) == Where::Product)
        .count();
    let harness = files.len() - product;
    assert!(
        product > 0 && harness > 0,
        "the classifier put all {} files on one side (product {product}, harness {harness}) — a \
         classifier with one answer exempts everything or exempts nothing, and either way it is \
         not reading the layout",
        files.len(),
    );

    let seam = files
        .iter()
        .find(|(name, _)| name == "crates/sprag-scratch/src/lib.rs")
        .expect("the scratch seam is a Rust file under crates/ and the walk must reach it");
    assert!(
        code_lines(&seam.1).any(|(_, line)| outside_strings(line).contains("env::temp_dir()")),
        "the filters no longer see the one call this workspace is allowed to make — whatever they \
         are dropping now, they would drop a violation the same way",
    );
    assert!(
        !outside_strings("    if !code.contains(\"env::temp_dir()\") {")
            .contains("env::temp_dir()"),
        "`outside_strings` stopped blanking string contents, so this gate reds on its own filter \
         and on every message that quotes the call while explaining it",
    );
}
