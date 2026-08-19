//! Which PACKAGE of this workspace owns a binary — register item 455.
//!
//! # ⚠⚠⚠ Why a guard needs this at all
//!
//! [`crate::sibling_bin`] refuses a binary whose freshness it cannot vouch for, and a refusal is
//! only half a gate: the other half is the command that ends it. That command was a LITERAL,
//! `cargo build -p sprag-host --bins`, written when both binaries the guard covered happened to
//! belong to `sprag-host`. `sprag-mcp` is its own package, and **that command does not build it**
//! — measured 2026-08-19 on a build machine, where the advice was followed exactly and the same
//! refusal came back with the same words. A fresh machine has no binaries at all, so this is the
//! first thing a fleet host says, and it sent the reader in a circle.
//!
//! ⚠⚠ **A LITERAL COULD ONLY EVER BE RIGHT FOR THE BINARIES SOMEBODY HAPPENED TO CHECK.** The next
//! package to grow a binary earns the same wrong sentence, silently, and the sentence still reads
//! like a working one. So the package is derived here, from this workspace's own manifests.
//!
//! # ⚠⚠ Why the package, when the path already carries the bin's name
//!
//! `cargo build --bin <name>` needs no map at all and is wrong from anywhere but the workspace
//! root. Measured both ways from `crates/sprag-host` on 2026-08-19: `cargo build --bin sprag-mcp`
//! answers *"error: no bin target named `sprag-mcp` in default-run packages"*, while `cargo build
//! -p sprag-mcp --bins` builds it. `--bin` is scoped to the package the caller is standing in and
//! `-p` reaches any member from any member — and a panic is printed wherever the reader happened
//! to be, not where the tree's root is.
//!
//! # ⚠⚠⚠ Why manifests rather than asking cargo
//!
//! This crate takes no dependencies and runs no nested cargo, for the reason stated across it: a
//! gate that stands outside the suite must not be able to fail because the product failed to
//! build — and a remedy printed from inside a panic is the worst possible place to start a second
//! cargo. The manifests are already on disk and are cargo's own input. That they are read
//! CORRECTLY is not argued here: `tests/a_refusal_names_the_command_that_builds_it.rs` asks
//! `cargo metadata` for the same map and asserts the two agree, so the two answers arrive from
//! opposite directions.

use std::path::{Path, PathBuf};

use crate::sources::workspace_root;

/// The cargo invocation that builds `bin`, whatever package turns out to own it.
///
/// Falls back to `cargo build --workspace --bins` for a binary this workspace does not declare —
/// a temporary directory's stand-in, say. That command always works; it is the second-best answer
/// precisely because it does not point at the one thing that is missing.
#[must_use]
pub fn build_command(bin: &Path) -> String {
    match bin.file_name().and_then(|name| name.to_str()).map(owner_of) {
        Some(Some(package)) => format!("cargo build -p {package} --bins"),
        _ => "cargo build --workspace --bins".to_owned(),
    }
}

/// The package that declares a bin target called `bin`, or `None` when this workspace has none.
#[must_use]
pub fn owner_of(bin: &str) -> Option<String> {
    bin_owners()
        .into_iter()
        .find_map(|(target, package)| (target == bin).then_some(package))
}

/// Every `(bin target, owning package)` this workspace declares.
///
/// Derived from the workspace's `members` list rather than from a walk of `crates/`, because that
/// list is what cargo itself reads: a member kept somewhere else is still found, and a directory
/// that is not a member is correctly not.
#[must_use]
pub fn bin_owners() -> Vec<(String, String)> {
    let root = workspace_root();
    let mut owners = Vec::new();
    for member in members(&root) {
        let dir = root.join(member);
        let manifest = read(&dir.join("Cargo.toml"));
        let Some(package) = value_in(sections(&manifest, "[package]").first(), "name") else {
            continue;
        };
        for bin in bins_of(&dir, &manifest, &package) {
            owners.push((bin, package.clone()));
        }
    }
    owners.sort();
    owners.dedup();
    owners
}

/// The bin targets a member declares — the ones written down, and the ones cargo finds for itself.
///
/// ⚠ BOTH, because a manifest is not obliged to say either way: `sprag-host` declares no `[[bin]]`
/// at all and ships four binaries, while `sprag-mcp` declares one that auto-discovery would have
/// found anyway. Taking only the written ones would lose the first; taking only the discovered
/// ones would lose any `[[bin]]` that names a target after something other than its file.
fn bins_of(dir: &Path, manifest: &str, package: &str) -> Vec<String> {
    let mut bins: Vec<String> = sections(manifest, "[[bin]]")
        .iter()
        .filter_map(|section| value_in(Some(section), "name"))
        .collect();

    // Auto-discovery, as cargo does it: `src/main.rs` is a target named after the package, and each
    // `src/bin/<name>.rs` or `src/bin/<name>/main.rs` is one named after the file.
    if dir.join("src/main.rs").is_file() {
        bins.push(package.to_owned());
    }
    if let Ok(entries) = std::fs::read_dir(dir.join("src/bin")) {
        for entry in entries.flatten() {
            let path = entry.path();
            let discovered = if path.is_dir() && path.join("main.rs").is_file() {
                path.file_name()
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                path.file_stem()
            } else {
                None
            };
            if let Some(name) = discovered.and_then(|name| name.to_str()) {
                bins.push(name.to_owned());
            }
        }
    }

    bins.sort();
    bins.dedup();
    bins
}

/// The workspace's member paths, from the root manifest's `members` array.
fn members(root: &Path) -> Vec<PathBuf> {
    let manifest = read(&root.join("Cargo.toml"));
    let Some(array) = sections(&manifest, "[workspace]")
        .first()
        .and_then(|workspace| workspace.split_once("members"))
        .and_then(|(_, rest)| rest.split_once('['))
        .and_then(|(_, rest)| rest.split_once(']'))
        .map(|(entries, _)| entries.to_owned())
    else {
        return Vec::new();
    };

    let mut found = Vec::new();
    for entry in quoted(&array) {
        // A `crates/*` member is a directory OF members, which is how a workspace says "everything
        // under here" — expanded rather than skipped, or a tree written that way has no members at
        // all and every refusal falls back to the workspace-wide answer.
        if let Some(parent) = entry.strip_suffix("/*") {
            let Ok(entries) = std::fs::read_dir(root.join(parent)) else {
                continue;
            };
            found.extend(
                entries
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| path.join("Cargo.toml").is_file())
                    .filter_map(|path| path.strip_prefix(root).ok().map(Path::to_path_buf)),
            );
        } else {
            found.push(PathBuf::from(entry));
        }
    }
    found
}

/// The `key = "value"` of a section body, comments dropped.
fn value_in(section: Option<&String>, key: &str) -> Option<String> {
    section?
        .lines()
        .filter_map(|line| line.split_once('='))
        .find(|(name, _)| name.trim() == key)
        .and_then(|(_, value)| quoted(value).first().map(|found| (*found).to_owned()))
}

/// The body of each `header` section of a TOML document, up to the next header, comment lines gone.
///
/// ⚠ Deliberately not a TOML parser: this crate has no dependencies, and the two shapes it needs —
/// a header on its own line, `key = "value"` under it — are the whole of what a `Cargo.toml` spells
/// them as. Anything cleverer would be a second implementation of something the gate in `tests/`
/// already checks against cargo itself.
fn sections(document: &str, header: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut body: Option<String> = None;
    for line in document.lines() {
        let trimmed = line.trim();
        let uncommented = trimmed.split('#').next().unwrap_or_default().trim();
        if trimmed.starts_with('[') && uncommented.ends_with(']') {
            if let Some(collected) = body.take() {
                found.push(collected);
            }
            if uncommented == header {
                body = Some(String::new());
            }
        } else if let Some(collected) = body.as_mut() {
            // ⚠ A comment is dropped whole rather than truncated at its `#`, so a `#` inside a
            // quoted value stays where it is.
            if !trimmed.starts_with('#') {
                collected.push_str(line);
                collected.push('\n');
            }
        }
    }
    found.extend(body);
    found
}

/// Every double-quoted run in `text`.
fn quoted(text: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some((_, after)) = rest.split_once('"') {
        let Some((value, tail)) = after.split_once('"') else {
            break;
        };
        found.push(value);
        rest = tail;
    }
    found
}

/// A manifest's text, or empty when there is none — a member without one simply declares no bins.
fn read(manifest: &Path) -> String {
    std::fs::read_to_string(manifest).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠⚠⚠⚠ **THE BINARY THAT WAS MEASURED WRONG, AND THE ONE THAT WAS ALWAYS RIGHT** — so a fix
    /// that swapped one literal for another is caught here rather than read as clean.
    #[test]
    fn the_remedy_names_the_package_that_owns_the_binary_it_is_about() {
        assert_eq!(
            build_command(Path::new("target/debug/sprag-mcp")),
            "cargo build -p sprag-mcp --bins",
            "⚠⚠⚠⚠⚠ item 455 itself: `sprag-mcp` is its OWN package, and the remedy that named \
             `sprag-host` was followed exactly on a build machine and changed nothing",
        );
        assert_eq!(
            build_command(Path::new("target/debug/sprag-term")),
            "cargo build -p sprag-host --bins",
            "and the daemon really does belong to `sprag-host`, so this is not a fix that stopped \
             naming packages",
        );
    }

    /// ⚠⚠ **A BINARY THIS WORKSPACE DOES NOT DECLARE STILL GETS A COMMAND THAT WORKS.**
    ///
    /// The alternative — printing `-p` and a guess — would be the same defect wearing the fix's
    /// clothes: a command that cannot do what it is offered for.
    #[test]
    fn a_binary_no_member_declares_falls_back_to_the_answer_that_always_works() {
        assert_eq!(
            build_command(Path::new("/tmp/whatever/not-a-target-of-this-tree")),
            "cargo build --workspace --bins",
            "an unknown binary has no package to name, and a workspace build is what a fresh host \
             wants anyway",
        );
    }

    /// ⚠⚠⚠ **AUTO-DISCOVERY IS HALF THE MAP, AND IT IS THE HALF `sprag-host` LIVES IN.**
    ///
    /// `crates/sprag-host/Cargo.toml` declares no `[[bin]]` whatsoever and ships four binaries, so
    /// a scan that read only what is written down would find no owner for the daemon at all — and
    /// the fallback would then quietly hand back the workspace-wide command for every refusal this
    /// gate exists to sharpen. Asserted here because it is invisible in the two cases above.
    #[test]
    fn the_written_bins_and_the_discovered_ones_are_both_found() {
        let owners = bin_owners();
        let of = |bin: &str| {
            owners
                .iter()
                .find(|(target, _)| target == bin)
                .map(|(_, package)| package.clone())
        };
        assert_eq!(
            of("sprag-agent-peer"),
            Some("sprag-host".to_owned()),
            "a `src/bin/*.rs` under a manifest with no `[[bin]]` section at all: {owners:?}",
        );
        assert_eq!(
            of("sprag-gui"),
            Some("sprag-gui".to_owned()),
            "a written `[[bin]]` whose path is `src/main.rs`: {owners:?}",
        );
        assert!(
            owners.len() > 3,
            "a map this small has not found the members, and a probe pointed at nothing must never \
             read as clean: {owners:?}",
        );
    }
}
