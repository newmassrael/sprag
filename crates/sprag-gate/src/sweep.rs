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

/// **WHAT ONE TEST DID, ACROSS EVERY SWEEP THIS TREE HAS A LOG OF** — register item 1109.
///
/// # ⛔⛔⛔⛔⛔ The archive was write-only, and a whole register item was built out of anecdotes
///
/// This workspace keeps every run's log under `target/bx-logs/`, and **nothing has ever read one as
/// data**: `bx-logs` appears in this tree only inside prose. Measured 2026-09-15 over that archive
/// for register item 683's three members — **3,146 recorded outcomes**, of which 73 are failures.
/// The item was assembled from the two or three somebody happened to be watching, and its central
/// claim — *these shake on a BUSY runner* — is refuted by the record it was drawn from:
///
/// | before 2026-09-03 | runs | red | rate |
/// |---|---|---|---|
/// | `RUST_TEST_THREADS >= 26` | 1,245 | 29 | **2.33%** |
/// | `RUST_TEST_THREADS < 26` | 592 | 24 | **4.05%** |
///
/// **The quiet band failed more often.** Three weeks of rounds read *"only on a busy runner"* off
/// the item and none of them could have known, because asking the record meant writing a script
/// nobody had written.
///
/// # ⚠⚠⚠ Why this is a READING and not a gate
///
/// The archive is this machine's: a clone has none, and a gate that demanded one would be red
/// everywhere but here — which is [`crate::north_star`]'s own placement argument for a table of
/// readings. What this buys is that *how often does it really shake* is a command rather than an
/// afternoon, so a claim about a flake can be put to the record on the day it is made.
///
/// ⚠⚠ **A LOG SAYS `ok` AND `FAILED` AND NOTHING ELSE ABOUT A TEST.** A run that never reached the
/// test leaves no line at all, which is exactly right: this counts what was OBSERVED and never
/// infers a pass from silence — the confusion [`unreported`] one function up exists to catch.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Outcomes {
    /// How many times this test was seen to pass.
    pub passed: usize,
    /// How many times it was seen to fail.
    pub failed: usize,
}

impl Outcomes {
    /// How often it failed, per ten thousand runs — [`None`] when it was never seen at all.
    ///
    /// ⚠ Per ten thousand rather than a float, for [`crate::north_star`]'s reason about percentages:
    /// the interesting movement here is fractional (2.33% against 4.05%) and an integer percent
    /// renders both as `2` and `4` while a rate of 0.4% and one of 0 render alike.
    #[must_use]
    pub fn per_myriad(&self) -> Option<u64> {
        let seen = self.passed + self.failed;
        (seen > 0).then(|| (self.failed as u64 * 10_000) / seen as u64)
    }

    /// The two counts of `other`, added to these.
    #[must_use]
    pub const fn and(self, other: Self) -> Self {
        Self {
            passed: self.passed + other.passed,
            failed: self.failed + other.failed,
        }
    }
}

/// **WHAT `test` DID IN `log`** — counted over one run's output.
///
/// ⚠⚠ The needle is the harness's own line, `test <name> ... ok|FAILED`, anchored on BOTH sides.
/// A bare `contains(name)` would count the name where it appears in a failure MESSAGE — every one
/// of item 683's members is named in the very assertion text it prints — so a red would be counted
/// as several, and a run naming it in prose as a pass.
///
/// ⚠ The name may arrive with or without its module path (`ai_loop::tests::x` in a lib target,
/// bare `x` in an integration one), so the match is on the name's own end of the line.
#[must_use]
pub fn outcomes_of(test: &str, log: &str) -> Outcomes {
    let mut seen = Outcomes::default();
    for line in log.lines() {
        let Some(rest) = line.strip_prefix("test ") else {
            continue;
        };
        let Some((named, verdict)) = rest.split_once(" ... ") else {
            continue;
        };
        // ⚠ `ends_with` and then the boundary, so `a_person_keeps_the_pane` cannot be counted for
        // `keeps_the_pane`. A path separator or the whole name are the only two ways it may end.
        let mine = named == test
            || (named.ends_with(test) && named[..named.len() - test.len()].ends_with("::"));
        if !mine {
            continue;
        }
        match verdict.trim() {
            "ok" => seen.passed += 1,
            "FAILED" => seen.failed += 1,
            // ⚠ `ignored` and anything a later harness prints are neither, and saying so is the
            // point: an ignored run is not evidence about whether this test shakes.
            _ => {}
        }
    }
    seen
}

/// ⛔⛔⛔⛔⛔ **EVERY TEST `log` REPORTS ON, AND WHAT EACH DID** — register item 1128, and the
/// inverse of [`outcomes_of`].
///
/// # ⛔⛔⛔ Why asking BY NAME was not enough
///
/// [`outcomes_of`] answers about a test somebody already suspected, so the population it can speak
/// about is a HAND LIST — and this register's own rule is that a hand list leaks (items 80, 762,
/// 823: *마크가 이긴다*). **It leaked here, measured**: register item 683 named a class of tests
/// that shake, carried three members for three weeks, and its FOURTH was found only because it
/// happened to fail during the round that first used this instrument. Nothing had ever enumerated
/// the archive; a member nobody suspected could not be found by asking.
///
/// So the class becomes a DERIVATION: ask the record which tests have ever failed, rather than ask
/// the record about the tests somebody remembers.
///
/// # ⚠⚠ Keyed on the name's LAST segment, which is what [`outcomes_of`] matches on
///
/// The harness prints `ai_loop::tests::x` in a lib target and a bare `x` in an integration one, and
/// [`outcomes_of`] deliberately matches either. Keying this on the full printed name would split
/// one test into two rows and make the two readers of one archive disagree — this crate's oldest
/// defect class. So both sides see one test.
///
/// ⚠ **The residue, stated**: two tests in different modules sharing a leaf name merge into one
/// row. They merge for [`outcomes_of`] too, so the readings agree; what neither can do is tell them
/// apart. A day that matters is the day a leaf name is reused, and the honest answer then is to
/// give one of them a different name.
#[must_use]
pub fn outcomes_by_test(log: &str) -> std::collections::BTreeMap<String, Outcomes> {
    let mut seen: std::collections::BTreeMap<String, Outcomes> = std::collections::BTreeMap::new();
    for line in log.lines() {
        let Some(rest) = line.strip_prefix("test ") else {
            continue;
        };
        let Some((named, verdict)) = rest.split_once(" ... ") else {
            continue;
        };
        let leaf = named.rsplit("::").next().unwrap_or(named).trim();
        if leaf.is_empty() {
            continue;
        }
        // ⚠⚠ THE ROW IS CREATED ONLY BY A VERDICT THAT COUNTS — `entry().or_default()` ahead of
        // this match gave an ignored-only test a `0 ok, 0 FAILED` row, so it appeared among the
        // tests this archive *reports on* while `outcomes_of` says it never ran. Two readers of one
        // archive disagreeing is what this function exists not to do; the gate caught it.
        match verdict.trim() {
            "ok" => seen.entry(leaf.to_owned()).or_default().passed += 1,
            "FAILED" => seen.entry(leaf.to_owned()).or_default().failed += 1,
            // ⚠ `ignored` is neither, exactly as `outcomes_of` has it — an ignored run is not
            // evidence about whether a test shakes.
            _ => {}
        }
    }
    seen
}

/// **HOW MANY THREADS THE HARNESS WAS GIVEN**, or [`None`] for a log that does not say.
///
/// ⚠⚠ It is what the WRAPPER decided, printed once per run, and it is the axis register item 683
/// rests its whole claim on — so a reading that could not recover it could not have refuted that
/// claim. [`None`] is *this log does not say*, never *one thread*.
#[must_use]
pub fn threads_in(log: &str) -> Option<u32> {
    log.split("RUST_TEST_THREADS=").skip(1).find_map(|rest| {
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        digits.parse().ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⛔⛔⛔⛔⛔ **ONE TEST'S HISTORY IS COUNTED OFF THE HARNESS'S OWN LINE** — register item 1109.
    ///
    /// # ⚠⚠⚠ The arm that matters is the one that must NOT count
    ///
    /// Every member of register item 683 prints its own name inside the assertion text it fails
    /// with, so a reader written as `log.contains(name)` counts one red several times and reads a
    /// run that merely mentioned the test as a pass. That is not a corner: it is the shape of every
    /// log this archive holds, and it would have made the rate this reading exists to produce
    /// wrong in both directions at once.
    #[test]
    fn a_tests_history_is_read_off_the_verdict_line_and_not_off_its_own_name() {
        let log = "\
running 3 tests
test a_person_keeps_the_pane ... ok
test tui::tests::a_person_keeps_the_pane ... FAILED
test a_person_keeps_the_pane_and_more ... FAILED
test keeps_the_pane ... ok
test something_else ... ignored

failures:

---- a_person_keeps_the_pane stdout ----
thread 'a_person_keeps_the_pane' panicked at x.rs:1:1:
a_person_keeps_the_pane ... FAILED is quoted right here in the message
";
        assert_eq!(
            outcomes_of("a_person_keeps_the_pane", log),
            Outcomes {
                passed: 1,
                failed: 1
            },
            "⛔⛔⛔⛔⛔ the bare name and the module-qualified one are the SAME test and both count; \
             a longer name that merely starts with it is a different test; and the name quoted in \
             a panic message is not a verdict at all",
        );
        assert_eq!(
            outcomes_of("keeps_the_pane", log),
            Outcomes {
                passed: 1,
                failed: 0
            },
            "⚠⚠⚠ AND A SUFFIX IS NOT A MATCH: `a_person_keeps_the_pane` ends with this name and is \
             not it. Without the `::` boundary one test's reds are filed against another's rate, \
             which is the arithmetic this reading exists to make trustworthy",
        );
        assert_eq!(
            outcomes_of("never_ran", log).per_myriad(),
            None,
            "⚠⚠ A TEST NOBODY RAN HAS NO RATE, and it must not read as a perfect one: silence is \
             the absence of evidence, which is `unreported`'s rule one function up",
        );
    }

    /// ⛔⛔⛔⛔⛔ **THE POPULATION IS DERIVED, NOT REMEMBERED** — register item 1128.
    ///
    /// # ⛔⛔⛔ The hand list leaked, measured
    ///
    /// [`outcomes_of`] can only confirm a name somebody already suspected, so the set of tests that
    /// shake was a HAND LIST — and this register's own rule is that a hand list leaks (items 80,
    /// 762, 823). **It did**: register item 683 carried three members for three weeks and its
    /// fourth was found only because it happened to fail during the round that first used this
    /// instrument. Asked of the whole archive on 2026-09-16, the worst-shaking test is at **3,469
    /// per 10k** and not one of 683's four members (75–351 per 10k) is near the top.
    ///
    /// ⚠⚠ **THE SAME MATCHING RULE AS [`outcomes_of`]**, asserted on the same fixture: two readers
    /// of one archive free to disagree is this crate's oldest defect class, and here it would mean
    /// the enumeration and the by-name query reporting different histories for one test.
    #[test]
    fn every_test_a_log_reports_on_is_tallied_the_way_asking_by_name_would() {
        let log = "\
running 3 tests
test a_person_keeps_the_pane ... ok
test tui::tests::a_person_keeps_the_pane ... FAILED
test a_person_keeps_the_pane_and_more ... FAILED
test keeps_the_pane ... ok
test something_else ... ignored

failures:

---- a_person_keeps_the_pane stdout ----
thread 'a_person_keeps_the_pane' panicked at x.rs:1:1:
a_person_keeps_the_pane ... FAILED is quoted right here in the message
";
        let seen = outcomes_by_test(log);
        assert_eq!(
            seen.get("a_person_keeps_the_pane"),
            Some(&Outcomes {
                passed: 1,
                failed: 1
            }),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 1128: the enumeration must agree with `outcomes_of` on the \
             SAME log — the bare and module-qualified lines are one test, and the name quoted in a \
             panic message is not a verdict",
        );
        assert_eq!(
            seen.get("a_person_keeps_the_pane_and_more"),
            Some(&Outcomes {
                passed: 0,
                failed: 1
            }),
            "⚠⚠ AND A LONGER NAME IS ITS OWN TEST, kept apart here exactly as the by-name reading \
             keeps it apart — otherwise one test's reds land on another's rate",
        );
        assert_eq!(
            seen.get("something_else"),
            None,
            "⚠⚠⚠ AN IGNORED RUN IS NOT EVIDENCE either way, so the test does not appear at all — \
             the same answer `outcomes_of` gives, and the reason a rate may never be read off a \
             switched-off test",
        );
        assert!(
            !seen.contains_key("never_ran"),
            "⚠ and a test this log never reports on is absent rather than zero: {seen:?}",
        );
        assert_eq!(
            Outcomes {
                passed: 97,
                failed: 3
            }
            .per_myriad(),
            Some(300),
            "⚠ per ten thousand, because 0.4% and 0% render alike as an integer percent",
        );
    }

    /// ⚠⚠ **THE LOAD AXIS IS RECOVERABLE**, or register item 683's claim could not have been put to
    /// the record at all — see [`Outcomes`], where the refutation is.
    #[test]
    fn the_thread_count_a_run_was_given_is_read_back_out_of_its_log() {
        assert_eq!(
            threads_in("bx: remote: 30 free core(s) -> RUST_TEST_THREADS=30\nrunning\n"),
            Some(30),
        );
        assert_eq!(
            threads_in("RUST_TEST_THREADS=2 and later RUST_TEST_THREADS=29"),
            Some(2),
            "the FIRST is what the run was given; a later line is another run's or an echo",
        );
        assert_eq!(
            threads_in("a log that never says"),
            None,
            "⚠ `None` is *this log does not say*, never *one thread* — a band chosen off a \
             fabricated 1 would file every silent log in the quiet band and invent the very \
             correlation this axis is being asked about",
        );
    }

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
