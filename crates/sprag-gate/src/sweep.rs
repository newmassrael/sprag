//! **WHETHER A SWEEP ACTUALLY SWEPT** — register item 585, and a claim no test in the sweep can
//! make about the sweep it is part of.
//!
//! # ⛔⛔⛔ The command succeeded and nineteen suites never ran
//!
//! MEASURED 2026-08-22, same commit, same machine, back to back:
//!
//! * `cargo test --workspace --exclude sprag-gui` stopped at a flake in `sprag-tui` and printed
//!   **61** `test result:` lines.
//! * The same command with `--no-fail-fast` printed **80**, all green.
//!
//! The nineteen suites in the difference were not red and not green — **they never ran**, and the
//! round that ran the first command was one sentence away from reporting a sweep. The only reason
//! anybody noticed was that somebody counted the lines by hand, which is a coincidence rather than
//! a ritual.
//!
//! ⚠⚠⚠ **THE RULE ALREADY EXISTED AND DID NOT REACH.** The round-ritual notes had carried
//! *"`cargo test -p X` stops at the first failing binary — `--no-fail-fast`"* since R255, written
//! about BINARIES inside one crate. Nobody wrote it about CRATES inside a workspace, so nobody
//! attached it to the sweep. A rule that has to be attached by hand is attached by whoever
//! remembers.
//!
//! # ⚠⚠⚠⚠⚠ Why the expectation is DERIVED and never written down
//!
//! *"The sweep must print 80 `test result:` lines"* is a number, and a number in a file rots the
//! moment a crate is added — register items 492 and 519 are two payments for exactly that. It also
//! rots INVISIBLY, because a stale expectation that is too LOW passes forever.
//!
//! So the expectation is read off the workspace itself: [`crate::sweep::members`] parses the root
//! manifest's own member list, and a crate added to that list is expected from the next run onward
//! without anybody editing this file. What would rot here is the DERIVATION RULE, and that has
//! gates.
//!
//! ⚠ The link above is written whole — `crate::sweep::members` — and the three shorter spellings
//! are not an option: `members`, `self::members` and `members()` each fail the doc gate with
//! *no item named `members` in scope* AND NO FILE LOCATION, which reads as a defect somewhere else
//! entirely. Measured 2026-08-23, three runs.
//!
//! # ⚠⚠ Why a crate name and not a target count
//!
//! A crate's target COUNT depends on `[[bin]]`, `[[test]]`, `doctest = false` and `harness = false`
//! — four manifest decisions this crate has no parser for and would guess at. A crate's PRESENCE
//! does not: `cargo test` names every target's binary under `target/debug/deps/<crate>-<hash>`,
//! with the package's name underscored, so a package that ran at all is in the log and one that did
//! not is missing. That is the shape the measurement above was about — nineteen whole suites — and
//! a checker that answered a harder question less reliably would be worse at it.

/// The workspace members named by the root manifest, as PACKAGE NAMES.
///
/// Parses the `members = [ … ]` array by hand rather than with a TOML crate, which is this crate's
/// no-dependencies rule (see its manifest): a gate that runs `if: always()` must not need the
/// dependency graph that may be what failed. The array's entries are paths (`"crates/sprag-vt"`),
/// and a package's name is its directory — true for every member of this workspace and asserted
/// against the real file by `the_derivation_finds_every_crate_this_workspace_has` below.
///
/// ⚠ That gate is named rather than linked: it is a `#[cfg(test)]` item, so an intra-doc link to it
/// is a link rustdoc cannot resolve in the build the doc gate runs — the class register item 591's
/// round paid for one file over, where three links pointed at an item that had moved.
///
/// ⚠ Comment lines are skipped, and this workspace's member list is full of them — the manifest
/// explains each crate where it is declared. A `#` inside a path is not a thing that can happen
/// here, so the rule is *a line whose first non-blank character is `#`*.
#[must_use]
pub fn members(manifest: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut inside = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        if !inside {
            // `members = [` opens it, and an inline `members = ["a", "b"]` is handled by falling
            // through to the same scan on the remainder of this very line.
            if line.starts_with("members") && line.contains('[') {
                inside = true;
            } else {
                continue;
            }
        }
        for quoted in line.split('"').skip(1).step_by(2) {
            if let Some(name) = quoted.rsplit('/').next()
                && !name.is_empty()
            {
                names.push(name.to_owned());
            }
        }
        if line.contains(']') {
            break;
        }
    }
    names
}

/// The members of `members` that `log` carries no test run for — **the crates a sweep did not
/// reach**, in the order they were declared.
///
/// A package that ran ANY target is in the log as `deps/<package with `-` as `_`>-<hash>`, which is
/// the path `cargo test` prints for every binary it starts. That makes this a question about the
/// artefact rather than about a count somebody maintains.
///
/// ⚠⚠ **`log` IS THE WHOLE SWEEP, WHICH IS MORE THAN ONE COMMAND.** This workspace sweeps in two
/// (`--workspace --exclude sprag-gui`, then `-p sprag-gui`), because the GPU crate cannot run
/// beside the rest — so a caller passes the logs of BOTH concatenated. A checker that judged one
/// command at a time would report the excluded crate missing on every honest sweep, and a rule that
/// cries wolf on the common path is one people learn to pass a flag to.
#[must_use]
pub fn unreported(members: &[String], log: &str) -> Vec<String> {
    members
        .iter()
        .filter(|name| !log.contains(&format!("deps/{}-", name.replace('-', "_"))))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The derivation reads THIS workspace's real manifest and finds every crate that is there.
    ///
    /// ⚠⚠⚠⚠⚠ **THE DIRECTORY LISTING IS THE SECOND ARTEFACT** — register item 470's shape. A
    /// gate that compared the parse against a list written in this file would be comparing the
    /// manifest to something this file believes; comparing it to `crates/` asks whether the
    /// manifest and the tree agree, and disagreement in EITHER direction is a finding.
    ///
    /// ⚠⚠ BOTH ARTEFACTS ARE READ AT RUN TIME — register item 809. The manifest used to arrive by
    /// `include_str!` and the listing by a `concat!` on the manifest directory, so BOTH were facts
    /// about the tree this crate was COMPILED in. Under the defect that item measured, this gate
    /// would have compared another workspace's manifest against another workspace's `crates/` and
    /// reported green about neither of them. Through [`crate::sources::workspace_root`] the two
    /// artefacts are the running tree's, and a skew is refused instead of read.
    #[test]
    fn the_derivation_finds_every_crate_this_workspace_has() {
        let root = crate::sources::workspace_root();
        let manifest = std::fs::read_to_string(root.join("Cargo.toml"))
            .expect("this workspace's own manifest");
        let mut declared = members(&manifest);
        declared.sort();

        let mut on_disk: Vec<String> = std::fs::read_dir(root.join("crates"))
            .expect("the crates directory this crate lives in")
            .filter_map(|entry| {
                let entry = entry.ok()?;
                entry
                    .path()
                    .join("Cargo.toml")
                    .exists()
                    .then(|| entry.file_name().to_string_lossy().into_owned())
            })
            .collect();
        on_disk.sort();

        assert_eq!(
            declared, on_disk,
            "⚠⚠⚠⚠⚠ THE MANIFEST AND THE TREE DISAGREE ABOUT WHAT THIS WORKSPACE IS. Either the \
             parse below is wrong, or a crate exists that no sweep would ever build — and the \
             second is worse, because a crate outside the member list is a crate no gate in this \
             repository has ever looked at",
        );
    }

    /// A crate the log never mentions is named, and one it does is not.
    ///
    /// ⚠⚠ The control is the SAME log: a checker that named everything would satisfy the first
    /// half while saying nothing about any sweep, and one that named nothing would satisfy the
    /// second half the same way. The claim is that it separates them.
    #[test]
    fn a_crate_with_no_run_in_the_log_is_named_and_one_with_a_run_is_not() {
        let log = "     Running unittests src/lib.rs (target/debug/deps/sprag_vt-16927952d693c6)\n\
                   test result: ok. 3 passed; 0 failed\n";
        let members = ["sprag-vt".to_owned(), "sprag-host".to_owned()];

        assert_eq!(
            unreported(&members, log),
            vec!["sprag-host".to_owned()],
            "⛔⛔⛔ ITEM 585: this log carries a run for one of these crates and nothing at all \
             for the other, and a sweep that stopped early looks exactly like that",
        );
    }

    /// The `-`/`_` translation is driven, because cargo does it and nothing else here would.
    ///
    /// ⚠ Its own test rather than a line above: a checker that forgot it would report EVERY
    /// hyphenated crate — which is all fifteen of them — as unreported, and a gate that is wrong
    /// about everything is one somebody switches off rather than fixes.
    #[test]
    fn a_packages_binary_is_found_under_the_name_cargo_actually_writes() {
        let log = "Running tests/cli.rs (target/debug/deps/sprag_host-0af1)\n";
        assert!(
            unreported(&["sprag-host".to_owned()], log).is_empty(),
            "cargo underscores a package name in the deps path, and a checker that looked for the \
             hyphenated spelling would find nothing anywhere",
        );
    }
}
