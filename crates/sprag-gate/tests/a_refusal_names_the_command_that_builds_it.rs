//! **A REFUSAL MUST NAME A COMMAND THAT BUILDS THE BINARY IT IS REFUSING** — register item 455.
//!
//! # ⚠⚠⚠ What went wrong, and why a reader could not get out of it
//!
//! [`sprag_gate::sibling_bin`] refuses a binary it cannot vouch for, and the refusal is well
//! argued — *"a run that cannot tell must not pass"*. Its remedy was a LITERAL, `cargo build -p
//! sprag-host --bins`, and one of the binaries it refuses is `sprag-mcp`, **which is its own
//! package and which that command does not build**. Measured 2026-08-19 on a build machine: the
//! sweep was run, the advice followed exactly, and the same refusal came back with the same words.
//!
//! A fresh machine has no binaries at all, so this is the FIRST thing a fleet host says, and it
//! sent the reader in a circle. The cost was one wasted suite per host per round.
//!
//! # ⚠⚠ Why the package and not the bin name, which the path already carries
//!
//! `cargo build --bin sprag-mcp` looks like the derivation this wants — the name is right there in
//! the path being refused — and it is **wrong from anywhere but the workspace root**. Measured,
//! both directions, on 2026-08-19:
//!
//! | run from `crates/sprag-host` | result |
//! |---|---|
//! | `cargo build --bin sprag-mcp` | `error: no bin target named 'sprag-mcp' in default-run packages` |
//! | `cargo build -p sprag-mcp --bins` | builds it |
//!
//! `--bin` is scoped to the package the caller is standing in; `-p` reaches any member from any
//! member. A test prints its panic wherever the reader happened to be, so the remedy has to be the
//! one that does not depend on where that was.
//!
//! # ⚠⚠⚠⚠ Where the truth in this file comes from, and why it is not the fix's own answer
//!
//! The fix reads this workspace's MANIFESTS. This gate asks **cargo** — `cargo metadata`, cargo's
//! own stable answer about which package owns which bin target — so the two arrive from opposite
//! directions and agreeing means something. A gate that recomputed the fix's own map would be
//! green by construction, which is register item 441's whole lesson.

use std::path::{Path, PathBuf};
use std::process::Command;

use sprag_gate::Unbuilt;

/// ⚠⚠⚠⚠⚠ **EVERY BINARY THIS WORKSPACE PRODUCES, IN EVERY WAY THE GUARD CAN REFUSE IT.**
///
/// Not `sprag-mcp` alone: a case naming the one binary that was measured wrong would go green on
/// the day somebody spelled a second package's name into the message, which is the shape the
/// register's *"the next binary added earns the same wrong sentence"* is about. The rows are cargo's
/// own list, so a package added tomorrow is checked tomorrow without this file being touched.
#[test]
fn every_refusal_names_the_package_that_owns_the_binary_it_refuses() {
    let owners = cargos_bin_owners();
    assert!(
        owners
            .iter()
            .any(|(bin, owner)| bin == "sprag-mcp" && owner == "sprag-mcp"),
        "⚠ the control: this workspace HAS a binary whose package is not `sprag-host`, which is the \
         whole reason the literal was wrong. A run that cannot see it is pointed at the wrong tree: \
         {owners:?}",
    );
    assert!(
        owners
            .iter()
            .any(|(bin, owner)| bin == "sprag-term" && owner == "sprag-host"),
        "⚠ and the other control: the daemon IS owned by `sprag-host`, so a fix that simply stopped \
         naming a package would be caught here rather than read as clean: {owners:?}",
    );

    for (bin, owner) in &owners {
        let path = PathBuf::from("target/debug").join(bin);
        let wanted = format!("-p {owner} ");
        for (arm, said) in refusals(&path) {
            assert!(
                said.contains(&wanted),
                "⚠⚠⚠⚠⚠ THE {arm} REFUSAL OF {bin} NAMES A COMMAND THAT DOES NOT BUILD IT. cargo \
                 says `{bin}` belongs to `{owner}`, so the remedy has to carry `{wanted}` — a \
                 reader who follows what it does say gets the SAME refusal back, which is register \
                 item 455 measured on a build machine. Said:\n{said}",
            );
            assert!(
                said.contains("cargo build "),
                "⚠⚠ ...and it has to be a build command rather than a package name mentioned in \
                 passing, or the reader has a fact instead of a way out. Said:\n{said}",
            );
        }
    }
}

/// ⚠⚠⚠ **AND THE MAP THE FIX DERIVES IS CARGO'S MAP, BOTH DIRECTIONS.**
///
/// The case above reads only the bins CARGO knows, so a fix that invented an extra owner — a
/// manifest scan that guessed a name for a `[[bin]]` with a `path` of its own, say — would be
/// invisible there. A set equality is what catches that, and it is also what catches the reverse:
/// a member this workspace gained that the scan does not walk.
#[test]
fn the_map_the_refusal_is_derived_from_is_the_one_cargo_publishes() {
    let mut mine = sprag_gate::owners::bin_owners();
    let mut cargos = cargos_bin_owners();
    mine.sort();
    cargos.sort();
    assert_eq!(
        mine, cargos,
        "⚠⚠⚠⚠ the manifests this crate reads and the answer cargo publishes have parted company. \
         Whichever is right, a refusal derived from the left-hand list is now guessing.",
    );
}

/// Each way [`Unbuilt`] can refuse `bin`, tagged with which arm it is.
///
/// All three arms rather than the one that was measured: they are three separate `write!`s with
/// three separately spelled remedies, which is exactly how one of them stayed wrong while the
/// others were read.
fn refusals(bin: &Path) -> Vec<(&'static str, String)> {
    vec![
        ("MISSING", Unbuilt::Missing(bin.to_path_buf()).to_string()),
        (
            "UNRECORDED",
            Unbuilt::Unrecorded {
                bin: bin.to_path_buf(),
                depfile: bin.with_extension("d"),
                why: "No such file or directory".to_owned(),
            }
            .to_string(),
        ),
        (
            "STALE",
            Unbuilt::Stale {
                bin: bin.to_path_buf(),
                edited: vec![PathBuf::from("crates/sprag-vt/src/lib.rs")],
            }
            .to_string(),
        ),
    ]
}

/// `(bin target, owning package)` for every binary this workspace declares — **cargo's own answer**.
///
/// # Panics
///
/// When cargo cannot be asked, or answers with something this cannot read. A probe that cannot see
/// must never read as clean, which is this crate's standing doctrine.
fn cargos_bin_owners() -> Vec<(String, String)> {
    let doc = cargo_metadata();
    let packages = member(&doc, "packages").expect("cargo metadata carries a `packages` array");
    let mut owners = Vec::new();
    for package in elements(packages) {
        let name = unquote(member(package, "name").expect("every package is named"));
        let targets = member(package, "targets").expect("every package lists its targets");
        for target in elements(targets) {
            let kinds = member(target, "kind").expect("every target has a kind");
            if elements(kinds)
                .into_iter()
                .any(|kind| unquote(kind) == "bin")
            {
                let bin = unquote(member(target, "name").expect("every target is named"));
                owners.push((bin.to_owned(), name.to_owned()));
            }
        }
    }
    assert!(
        owners.len() > 3,
        "a metadata read that found {} binaries has not understood the document, and a gate that \
         checks nothing is worse than no gate",
        owners.len(),
    );
    owners
}

/// cargo's metadata for THIS workspace, without its dependency graph.
///
/// ⚠ `--offline` first because a gate must not need a network, and once without it because a
/// machine whose lockfile cargo wants to refresh would otherwise be a red about nothing. A failure
/// of BOTH is a panic carrying cargo's own words — never a skip.
fn cargo_metadata() -> String {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let mut refused = String::new();
    for offline in [true, false] {
        let mut command = Command::new(&cargo);
        command.args(["metadata", "--no-deps", "--format-version", "1"]);
        if offline {
            command.arg("--offline");
        }
        match command.output() {
            Ok(done) if done.status.success() => {
                return String::from_utf8(done.stdout).expect("cargo metadata is utf-8 json");
            }
            Ok(done) => refused.push_str(&String::from_utf8_lossy(&done.stderr)),
            Err(why) => refused.push_str(&why.to_string()),
        }
    }
    panic!("`{cargo} metadata` is this gate's source of truth and it refused:\n{refused}");
}

// ── the smallest JSON reader that answers the two questions above ───────────────────────────────
//
// ⚠⚠ This crate takes no dependencies by charter (*"a gate that stands outside the suite must not
// be able to fail because the product failed to compile"*), so `serde_json` is not available to it
// even in a test. What is needed is two operations — a named member of an object, the elements of
// an array — and both are a depth walk that knows a brace inside a string is not a brace. That is
// what is here, and no more: it does not unescape, because no package or target name in any
// manifest cargo will accept contains an escape.

/// The text of the top-level member `key` of the JSON object `object`.
fn member<'a>(object: &'a str, key: &str) -> Option<&'a str> {
    let wanted = format!("\"{key}\"");
    elements(object).into_iter().find_map(|entry| {
        let colon = at_top(entry, ':')?;
        (entry[..colon].trim() == wanted).then(|| entry[colon + 1..].trim())
    })
}

/// The top-level elements of the JSON array or object `value`, each as its own text.
fn elements(value: &str) -> Vec<&str> {
    let value = value.trim();
    let inner = value
        .strip_prefix(['[', '{'])
        .and_then(|rest| rest.strip_suffix([']', '}']))
        .unwrap_or(value);
    let mut parts = Vec::new();
    let mut rest = inner;
    while let Some(comma) = at_top(rest, ',') {
        parts.push(rest[..comma].trim());
        rest = &rest[comma + 1..];
    }
    let tail = rest.trim();
    if !tail.is_empty() {
        parts.push(tail);
    }
    parts
}

/// The byte offset of the first `wanted` that is not nested and not inside a string.
fn at_top(text: &str, wanted: char) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (at, char) in text.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if char == '\\' {
                escaped = true;
            } else if char == '"' {
                in_string = false;
            }
            continue;
        }
        match char {
            '"' => in_string = true,
            '{' | '[' => depth += 1,
            '}' | ']' => depth = depth.saturating_sub(1),
            _ if char == wanted && depth == 0 => return Some(at),
            _ => {}
        }
    }
    None
}

/// The contents of a JSON string, or the text itself when it is not one.
fn unquote(text: &str) -> &str {
    text.trim().trim_matches('"')
}

/// ⚠⚠⚠ **THE READER ABOVE IS ITSELF A CLAIM, SO IT IS MEASURED RATHER THAN TRUSTED.**
///
/// Its whole job is to not be fooled by punctuation inside a string, and a walker that ignored
/// strings would answer *most* of `cargo metadata` correctly — which is the kind of nearly-right
/// that makes a gate green for the wrong reason. The literal here carries every trap the real
/// document has: a comma and a brace inside a value, a nested array, and a repeated key name at a
/// deeper level than the one being asked about.
#[test]
fn the_json_reader_is_not_fooled_by_punctuation_inside_a_string() {
    let doc = r#"{"packages":[{"name":"a, {not} a brace","targets":[
        {"kind":["lib"],"name":"deep"},{"kind":["bin","cdylib"],"name":"the-bin"}]}]}"#;
    let packages = member(doc, "packages").expect("the array is found");
    let package = elements(packages)[0];
    assert_eq!(
        unquote(member(package, "name").expect("the package name")),
        "a, {not} a brace",
        "a comma and a brace inside a string are text, not structure",
    );
    let targets: Vec<_> = elements(member(package, "targets").expect("the targets"))
        .into_iter()
        .filter(|target| {
            elements(member(target, "kind").expect("a kind"))
                .into_iter()
                .any(|kind| unquote(kind) == "bin")
        })
        .map(|target| unquote(member(target, "name").expect("a target name")))
        .collect();
    assert_eq!(
        targets,
        vec!["the-bin"],
        "the nested `name` belongs to the target being read, not to the package around it",
    );
}
