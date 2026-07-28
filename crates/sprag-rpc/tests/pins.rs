//! The workspace's upstream PINS, asserted against the RESOLVED lockfile.
//!
//! A pin bump is a nine-line edit plus an SCE line, and the two ways it goes wrong are both silent:
//! leave one `pinion-*` line behind and cargo happily builds two revisions of pinion into one
//! binary; let sprag's SCE rev drift from the one pinion itself pins and the "single shared SCE
//! instance" the root `Cargo.toml` exists to preserve quietly becomes two. Neither shows up as a
//! build error, and the ritual note that says "bump all nine plus SCE" is a thing to remember
//! rather than a thing that is checked.
//!
//! So this checks it. `Cargo.lock` is the right witness precisely because it is the RESOLVED graph:
//! reading the manifests back would only re-state what was typed, while the lock says what cargo
//! actually chose — and it is the lock, not the manifest, that decides how many copies get linked.
//!
//! Hosted in `sprag-rpc` because this crate is where the pin is load-bearing (it consumes
//! `pinion-rpc-transport` directly), but the invariant is the whole workspace's; the test walks up
//! to the workspace root rather than assuming a crate-local file.

use std::collections::BTreeSet;
use std::path::PathBuf;

/// The workspace root's `Cargo.lock`, found by walking up from this crate rather than by a relative
/// path guess, so moving the crate cannot silently make the test read nothing.
fn lockfile() -> String {
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        let candidate = dir.join("Cargo.lock");
        if candidate.is_file() {
            return std::fs::read_to_string(candidate).expect("read the workspace Cargo.lock");
        }
        assert!(
            dir.pop(),
            "no Cargo.lock above {}",
            env!("CARGO_MANIFEST_DIR")
        );
    }
}

/// Every `?rev=<sha>` fragment the lock resolved for `repo`, deduplicated.
///
/// The terminator set includes `)` and `,` for a reason worth keeping: a lock names a git source
/// two ways — a package's own `source = "git+…?rev=…#sha"`, and, ONLY when two revisions make a
/// name ambiguous, a disambiguating `"crate 0.1.0 (git+…?rev=…#sha)"` inside a `dependencies` list.
/// The parenthesised form therefore appears exactly in the case this file exists to catch, so a
/// terminator set that missed `)` would parse fine every day and mis-parse on the one run that
/// matters — reporting a trailing-paren rev as a malformed sha instead of naming the drift. Found
/// by running the revert-proof, not by reading.
fn revs_for(lock: &str, repo: &str) -> BTreeSet<String> {
    let needle = format!("{repo}.git?rev=");
    lock.lines()
        .filter_map(|line| line.split_once(&needle))
        .filter_map(|(_, tail)| tail.split(['#', '"', ' ', ')', ',']).next())
        .filter(|rev| !rev.is_empty())
        .map(str::to_owned)
        .collect()
}

#[test]
fn the_whole_workspace_resolves_one_pinion_revision() {
    let revs = revs_for(&lockfile(), "pinion");
    assert!(
        !revs.is_empty(),
        "the lock names no pinion revision at all — this test is reading the wrong file"
    );
    assert_eq!(
        revs.len(),
        1,
        "a partial pin bump: cargo resolved {} pinion revisions into one graph — {revs:?}",
        revs.len(),
    );
}

#[test]
fn the_whole_workspace_resolves_one_sce_revision() {
    // The shared-instance rule the root Cargo.toml states: sprag's SCE rev must be the one pinion
    // pins, or the two do not share an instance. One resolved revision IS that property, observed
    // rather than restated.
    let revs = revs_for(&lockfile(), "scxml-core-engine");
    assert!(
        !revs.is_empty(),
        "the lock names no SCE revision at all — this test is reading the wrong file"
    );
    assert_eq!(
        revs.len(),
        1,
        "sprag's SCE pin has drifted from pinion's: {} revisions — {revs:?}",
        revs.len(),
    );
}

#[test]
fn every_pinned_upstream_crate_names_a_full_sha() {
    // A short rev resolves today and is ambiguous later; the pin block is written as full SHAs and
    // this keeps it that way.
    let lock = lockfile();
    for repo in ["pinion", "scxml-core-engine"] {
        for rev in revs_for(&lock, repo) {
            assert_eq!(
                rev.len(),
                40,
                "{repo} is pinned to a non-full-length rev {rev:?}"
            );
            assert!(
                rev.chars().all(|c| c.is_ascii_hexdigit()),
                "{repo} rev {rev:?} is not a hex sha"
            );
        }
    }
}
