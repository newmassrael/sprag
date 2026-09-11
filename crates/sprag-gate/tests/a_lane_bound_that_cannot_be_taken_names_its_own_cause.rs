//! ⛔⛔⛔⛔⛔ **THREE DIFFERENT FAILURES SPOKE WITH ONE VOICE, AND IT WAS THE WRONG ONE** — register
//! item 1009.
//!
//! `.githooks/rust-gates.sh` bounds its own lanes: it divides this host's free memory by that
//! lane's `[routed]` reading and hands the quotient down as `CARGO_BUILD_JOBS`. Three separate
//! things can stop that, and until 2026-09-10 all three printed *no usable `precommit_kb` in
//! <file>*.
//!
//! ⚠ **THERE ARE TWO LANES SINCE ITEM 1011**, and every arm here is driven against both — see
//! [`LANES`] for why that is item 213 rather than thoroughness.
//!
//! **MEASURED, on a host whose `/proc` answers ENOENT to every path, with this repository's own
//! declaration in place**: the hook's `sed` extracted `9260688` perfectly and the hook announced
//! that the file said nothing. What was missing was `/proc/meminfo`. A person on that host is sent
//! to edit a declaration that is correct — which is the defect item 1006 is named for, eight lines
//! above the line that has it, reached by a different road.
//!
//! # ⚠⚠ The third cause could not be spoken at all
//!
//! `precommit_kb` is `VmHWM` sampled out of procfs (item 1008), so it is **one platform's** memory
//! behaviour. Nothing said whether it was THIS host's. Item 1008 recorded the platform beside the
//! number; this clause is what makes something READ it — item 932's own rule, that a row nobody
//! divides by is a number in a file, applied to the field 1008 added.
//!
//! # ⚠⚠⚠ What this does NOT claim
//!
//! Not that an unverifiable host is bounded some other way by this hook. Every arm below leaves the
//! lane exactly as it ran yesterday. The claim is that the sentence is TRUE: a host that cannot be
//! measured says so, a foreign reading says so, and only a declaration that really says nothing is
//! reported as saying nothing.
//!
//! # ⛔⛔⛔⛔⛔ AND THE WORD «UNBOUNDED» WAS ITSELF UNTRUE — register item 1010
//!
//! Until 2026-09-11 all three arms ended *"it is running UNBOUNDED"*, and **measured that day, that
//! is false whenever the wrapper is in the chain** — which is every commit not run under `env -u
//! BX`. `bx --local` with nothing exported answered `peak 2GB/task -> RUST_TEST_THREADS=10` and the
//! command saw 10. The lane is bounded, by the workspace scalar `peak_gb_per_task` — derived from
//! `[peak_measured]`, whose `platform` the wrapper never reads (`grep -c peak_measured bin/bx`
//! answers **0**). So the arms announced the absence of exactly what was happening, and the number
//! being spent was one platform's, which is the half of item 1010 this repository owns.
//!
//! ⚠ [`no_arm_calls_a_lane_unbounded_while_the_wrapper_is_there_to_bound_it`] and its siblings hold
//! the repair. The word is not banished — [`without_a_wrapper_an_unbounded_lane_is_still_called_
//! unbounded`] is the control that keeps deleting it from being the cheap way to green.
//!
//! ⚠ **THE FIRST CASE IS THE CONTROL AND IT IS NOT DECORATION.** Three arms that all assert *the
//! false sentence is absent* would stay green if the function printed nothing at all, or were
//! deleted. The control drives the arm that must still COMPUTE, with a staged reading and a staged
//! `MemAvailable`, and asserts the quotient.

use std::path::{Path, PathBuf};
use std::process::Command;

use sprag_gate::doubles::Doubles;
use sprag_gate::sources::workspace_root;

/// The library under test. Sourced rather than executed: its dispatch refuses a bare run on
/// purpose (register item 819), and the subject here is one function inside it.
const LIBRARY: &str = ".githooks/rust-gates.sh";

/// The sentence that must only ever be said about a declaration that really says nothing.
const BLAMES_THE_FILE: &str = "no usable reading";

/// ⛔⛔⛔⛔⛔ **BOTH LANES, AND THAT IS NOT THOROUGHNESS** — register items 1011 and 213.
///
/// Item 1011 split this hook's one routed command in two, because the two legs stopped having one
/// subject: clippy and the rustdoc gate compile a checkout of the index, the ratchet lane runs in
/// the working tree. Each now reads its OWN row, and each reader spells its own key literally —
/// `rust-gates.sh` says why that is forced rather than chosen. Two spellings of one rule is exactly
/// the shape item 213 was opened for and measured in this very directory (`-D warnings` added to
/// one clippy line and not its twin, for twelve commits), so every arm below is driven against
/// both. A file that exercised only the first would go green over a `ratchets_kb` misspelt into a
/// key nothing writes, and the lane would run unbounded in silence.
const LANES: [(&str, &str); 2] = [
    ("rust_gates_bound_lint_lane", "precommit"),
    ("rust_gates_bound_ratchet_lane", "ratchets"),
];

/// What one drive of a lane's bound reader said and what it left behind.
struct Bound {
    said: String,
    jobs: String,
}

/// Drive the hook function against a staged tree.
///
/// ⚠⚠ `set -euo pipefail` is set here because the hook that calls this function has it, and this
/// function has already been killed once by that combination: `sed` on an absent file exits 2,
/// `pipefail` carries it to the assignment and `set -e` ends the hook at that line in silence
/// (register item 467's throwaway repository, which has no declaration). A driver without those
/// options would be a different program from the one that runs.
fn drive(tree: &Path, meminfo: &Path, reader: &str) -> Bound {
    drive_with(tree, meminfo, reader, None)
}

/// Drive a lane's bound reader with the wrapper either staged or absent.
///
/// ⛔⛔⛔⛔⛔ **`BX` IS REMOVED WHENEVER IT IS NOT STAGED, AND THAT IS THE FIXTURE'S WHOLE
/// DETERMINISM SINCE ITEM 1010.** The readers now ask `rust_gates_wrapper_present` and print a
/// different sentence either way, and this suite runs inside a commit hook that exports `BX` on
/// every developer machine in the fleet. Inheriting it would make every arm below say whatever the
/// person running it happened to have configured — green here, red on the next box, and neither
/// result about the code.
fn drive_with(tree: &Path, meminfo: &Path, reader: &str, wrapper: Option<&Path>) -> Bound {
    // ⚠ The reader is named as an ARGUMENT of the script rather than pasted into it, so a name that
    // does not exist is bash refusing a command rather than this file composing a new program.
    const SCRIPT: &str = "set -euo pipefail\n\
         . \"$1\"\n\
         repo_root=\"$2\"\n\
         \"$3\"\n\
         printf 'JOBS=%s\\n' \"${CARGO_BUILD_JOBS:-unset}\"\n";
    let mut command = Command::new("bash");
    command
        .arg("-c")
        .arg(SCRIPT)
        .arg("_")
        .arg(workspace_root().join(LIBRARY))
        .arg(tree)
        .arg(reader)
        .env("RUST_GATES_MEMINFO", meminfo)
        // ⚠ The function defaults these from the environment (`${CARGO_BUILD_JOBS:-…}`), so a
        // developer who exports them would otherwise change what this gate reads.
        .env_remove("CARGO_BUILD_JOBS")
        .env_remove("RUST_TEST_THREADS")
        .current_dir(workspace_root());
    match wrapper {
        Some(path) => command.env("BX", path),
        None => command.env_remove("BX"),
    };
    let out = command
        .output()
        .expect("the hook library is sourced by a shell");
    assert!(
        out.status.success(),
        "⚠ THE STAGING, NOT THE CLAIM: driving `{reader}` exited {:?}. Under the \
         hook's own `set -euo pipefail` this function must always return cleanly — a non-zero here \
         is the hook dying at a line, which is register item 467's failure.\nstderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr),
    );
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    Bound {
        said: String::from_utf8_lossy(&out.stderr).to_string(),
        jobs: stdout
            .lines()
            .find_map(|line| line.strip_prefix("JOBS="))
            .unwrap_or("missing")
            .to_string(),
    }
}

/// A tree holding exactly the declaration text given.
///
/// ⚠ Through [`sprag_scratch`] and never `std::env::temp_dir()` — register item 794: the bare call
/// answers a RELATIVE path when `TMPDIR` is set-and-empty, and this fixture would then write a
/// declaration inside this crate's own directory.
fn tree_declaring(tag: &str, decl: &str) -> PathBuf {
    let tree = sprag_scratch::scratch_for("sprag-lane-bound", tag);
    std::fs::create_dir_all(tree.join(".claude")).expect("a staging tree for the hook");
    std::fs::write(tree.join(".claude/remote-build.toml"), decl).expect("a declaration to read");
    tree
}

/// This host's platform, asked the way the hook asks it rather than mapped from a Rust constant.
fn host_platform() -> String {
    let out = Command::new("uname")
        .arg("-s")
        .output()
        .expect("uname -s answers on every platform this workspace runs on");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// ⭐ **THE CONTROL: when the reading is this platform's and the host can be measured, the bound is
/// TAKEN** — and the quotient is asserted, not just the absence of a complaint.
#[test]
fn a_reading_from_this_platform_bounds_the_lane_by_the_free_memory_it_measured() {
    for (reader, key) in LANES {
        let tree = tree_declaring(
            &format!("control-{key}"),
            &format!(
                "[routed]\n{key}_kb = 5000000\n{key}_platform = \"{}\"\n",
                host_platform(),
            ),
        );
        let meminfo = tree.join("meminfo");
        std::fs::write(
            &meminfo,
            "MemTotal:       32000000 kB\nMemAvailable:   20000000 kB\n",
        )
        .expect("a staged MemAvailable for the arm that divides");

        let bound = drive(&tree, &meminfo, reader);
        let _ = std::fs::remove_dir_all(&tree);

        assert_eq!(
            bound.jobs, "4",
            "⛔ ITEM 1009's CONTROL, for `{reader}`: 20,000,000 kB free divided by a 5,000,000 kB \
             peak is 4 jobs, and the lane handed down `{}`. Every other case in this file asserts \
             that a SENTENCE is absent; if this arm stops computing, those would all still pass \
             over a function that does nothing.\nsaid:\n{}",
            bound.jobs, bound.said,
        );
        assert!(
            bound.said.contains(&host_platform()),
            "⛔ ITEM 1009: `{reader}` took the bound and the line does not say which platform's \
             reading it was taken from. The number is one platform's memory behaviour and the line \
             that spends it is where a reader finds out which.\nsaid:\n{}",
            bound.said,
        );
    }
}

/// ⛔⛔⛔ **A HOST WHOSE FREE MEMORY CANNOT BE READ IS NOT A DECLARATION THAT SAYS NOTHING.**
///
/// This is the arm that was measured wrong: procfs absent, the reading perfectly extractable, and
/// the hook blaming the file. On macOS this arm is what a developer's commit reaches for real —
/// there is no `/proc/meminfo` there at all.
#[test]
fn a_host_that_cannot_be_measured_says_so_instead_of_blaming_the_declaration() {
    for (reader, key) in LANES {
        let tree = tree_declaring(
            &format!("no-meminfo-{key}"),
            &format!(
                "[routed]\n{key}_kb = 5000000\n{key}_platform = \"{}\"\n",
                host_platform(),
            ),
        );
        let absent = tree.join("no-meminfo-here");

        let bound = drive(&tree, &absent, reader);
        let _ = std::fs::remove_dir_all(&tree);

        assert!(
            !bound.said.contains(BLAMES_THE_FILE),
            "⛔ ITEM 1009: `{key}_kb` is right there and extractable, and `{reader}` reported that \
             the declaration has none. The person reading this is sent to edit a file that is \
             correct while the host is what could not answer — item 1006's rule, that a refusal \
             naming the wrong cause is worse than none.\nsaid:\n{}",
            bound.said,
        );
        assert!(
            bound.said.contains("free memory"),
            "⛔ ITEM 1009: `{reader}` went unbounded and did not say that reading this HOST's free \
             memory is what failed. An unbounded lane is acceptable; an unexplained one sends the \
             next person to the wrong file.\nsaid:\n{}",
            bound.said,
        );
        assert_eq!(
            bound.jobs, "unset",
            "⚠ ITEM 1009: nothing may be handed down as a job count when the divisor could not be \
             measured — a guessed bound is the swap storm item 456 opened on.\nsaid:\n{}",
            bound.said,
        );
    }
}

/// ⛔⛔⛔ **A READING TAKEN SOMEWHERE ELSE DOES NOT BOUND THIS HOST**, and this is what makes the
/// platform item 1008 recorded a row somebody USES.
#[test]
fn a_reading_from_another_platform_does_not_bound_this_host_and_says_which() {
    for (reader, key) in LANES {
        let tree = tree_declaring(
            &format!("foreign-{key}"),
            &format!("[routed]\n{key}_kb = 5000000\n{key}_platform = \"Plan9\"\n"),
        );
        let meminfo = tree.join("meminfo");
        std::fs::write(&meminfo, "MemAvailable:   20000000 kB\n").expect("a staged MemAvailable");

        let bound = drive(&tree, &meminfo, reader);
        let _ = std::fs::remove_dir_all(&tree);

        assert!(
            !bound.said.contains(BLAMES_THE_FILE),
            "⛔ ITEM 1009: the declaration named its reading's platform and `{reader}` reported \
             that it says nothing.\nsaid:\n{}",
            bound.said,
        );
        assert!(
            bound.said.contains("Plan9"),
            "⛔ ITEM 1009: `{reader}` never read `{key}_platform`, so a peak measured on another \
             platform was either spent as if it were this host's or dropped without a word. The \
             field exists precisely so this line can name it.\nsaid:\n{}",
            bound.said,
        );
        assert_eq!(
            bound.jobs, "unset",
            "⛔ ITEM 1009: a foreign reading must not be divided by. Free memory was staged and \
             readable here, so a job count appearing means the platform was not consulted at all \
             and another host's peak was spent as if it were this one's.\nsaid:\n{}",
            bound.said,
        );
    }
}

/// ⚠ **AND THE ORIGINAL SENTENCE STILL HAS ITS OWN CASE.** Splitting a message three ways is only
/// an improvement if the first one still gets said when it is true — a repair that removed it would
/// make a declaration nobody finished look exactly like a host nobody could measure.
#[test]
fn a_declaration_that_really_says_nothing_about_this_lane_is_still_told_so() {
    for (reader, key) in LANES {
        let tree = tree_declaring(&format!("silent-{key}"), "[routed]\n");
        let meminfo = tree.join("meminfo");
        std::fs::write(&meminfo, "MemAvailable:   20000000 kB\n").expect("a staged MemAvailable");

        let bound = drive(&tree, &meminfo, reader);
        let _ = std::fs::remove_dir_all(&tree);

        assert!(
            bound.said.contains(BLAMES_THE_FILE),
            "⚠ ITEM 932's own case is gone: a `[routed]` table with no `{key}_kb` has to be \
             reported as exactly that by `{reader}`, or the lane running unbounded is a silence \
             again.\nsaid:\n{}",
            bound.said,
        );
        assert!(
            bound.said.contains(key),
            "⛔ ITEMS 1011 and 932: the refusal does not say WHICH lane went unbounded. There are \
             two of them now, with two readings and two rows to go and edit, and a sentence that \
             names neither sends a reader to look at both.\nsaid:\n{}",
            bound.said,
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ⛔⛔⛔⛔⛔ WHAT SPENDS THE LANE WHEN THIS HOOK DOES NOT — register item 1010
// ─────────────────────────────────────────────────────────────────────────────

/// The `peak_gb_per_task` every staged declaration below offers the wrapper.
const STAGED_PEAK_GB: &str = "7";

/// The whole phrase the hook prints when it names the wrapper's divisor.
///
/// ⛔⛔⛔⛔⛔ **THE PHRASE AND NOT THE DIGIT, AND THE FIRST DRAFT LEARNED THAT FROM ITS OWN MUTATION
/// BATTERY.** The arms below assert both that the figure APPEARS and that it does NOT, and the
/// bare `"7"` made the second of those a coin toss: the hook prints the staging tree's path in
/// every message, and [`sprag_scratch`] builds that path out of a pid. Four unrelated mutations
/// came back red on the control for no reason but a `7` in a directory name — a false red, which
/// is a gate saying nothing while looking like it says everything.
const SPENDS_PHRASE: &str = "peak_gb_per_task = 7GB/task";

/// A staged declaration: the rows `[routed]` carries, and the scalar the wrapper would divide by.
///
/// ⚠ `peak_gb_per_task` is a TOP-LEVEL key and `platform` belongs to `[peak_measured]` — the same
/// shape as the real file, because the hook reads both with line-anchored `sed` and a fixture that
/// nested them differently would be testing a parser this repository does not have.
fn decl_offering(lane_rows: &str, measured_platform: &str) -> String {
    format!(
        "peak_gb_per_task = {STAGED_PEAK_GB}\n\
         [peak_measured]\n\
         platform = \"{measured_platform}\"\n\
         [routed]\n{lane_rows}",
    )
}

/// The wrapper, staged as a path that exists, is executable, and is never run.
///
/// ⚠ Through [`Doubles`] and never a file this test writes — register item 467. The readers under
/// test hand it nothing: `rust_gates_wrapper_present` is `[ -n "$BX" ] && [ -x "$BX" ]`, so what
/// the fixture owes is a path, not a program.
fn staged_wrapper() -> PathBuf {
    Doubles::of(env!("CARGO_MANIFEST_DIR"))
        .set("lane-bound")
        .program("bx")
}

/// ⛔⛔⛔⛔⛔ **A LANE THE WRAPPER WILL BOUND IS NOT ANNOUNCED AS UNBOUNDED** — register item 1010.
///
/// All three arms of `rust_gates_bound_from` end without a bound of this hook's own, and all three
/// used to say the lane was therefore running UNBOUNDED. **Measured 2026-09-11**: with the wrapper
/// in the chain it is not — `bx --local`, nothing exported, answered `peak 2GB/task ->
/// RUST_TEST_THREADS=10` and the command saw 10. The sentence denied the thing that was happening,
/// which is item 1006's rule reached for the third time.
///
/// ⚠⚠ **EVERY ARM, BOTH LANES, AND THAT IS ITEM 213 RATHER THAN THOROUGHNESS.** The three arms are
/// three separate `echo` lines in one function; a repair applied to the one somebody was looking at
/// is exactly the drift that item's own measurement recorded.
#[test]
fn no_arm_calls_a_lane_unbounded_while_the_wrapper_is_there_to_bound_it() {
    let wrapper = staged_wrapper();
    for (reader, key) in LANES {
        let arms: [(&str, String, &str); 3] = [
            (
                "silent-declaration",
                String::new(),
                "no reading at all for this lane",
            ),
            (
                "foreign-reading",
                format!("{key}_kb = 5000000\n{key}_platform = \"Plan9\"\n"),
                "a reading taken on another platform",
            ),
            (
                "unmeasurable-host",
                format!(
                    "{key}_kb = 5000000\n{key}_platform = \"{}\"\n",
                    host_platform()
                ),
                "a host whose free memory cannot be read",
            ),
        ];
        for (arm, rows, why) in arms {
            let tree = tree_declaring(
                &format!("wrapped-{arm}-{key}"),
                &decl_offering(&rows, "Plan9"),
            );
            let meminfo = tree.join("meminfo");
            // ⚠ The unmeasurable arm is staged by leaving the seam pointed at a file that never
            // appears; the other two need a readable one so that THEIR arm is the one reached.
            if arm != "unmeasurable-host" {
                std::fs::write(&meminfo, "MemAvailable:   20000000 kB\n")
                    .expect("a staged MemAvailable");
            }

            let bound = drive_with(&tree, &meminfo, reader, Some(&wrapper));
            let _ = std::fs::remove_dir_all(&tree);

            assert!(
                !bound.said.contains("UNBOUNDED"),
                "⛔ ITEM 1010: `{reader}` met {why} and told the operator the {key} lane is \
                 running UNBOUNDED. The wrapper is in the chain, so it is not: `bx` divides this \
                 host's free RAM by `peak_gb_per_task` for whatever this hook left unbounded — \
                 measured 2026-09-11 at `peak 2GB/task -> RUST_TEST_THREADS=10`. Say what spends \
                 the lane instead of denying that anything does.\narm: {arm}\nsaid:\n{}",
                bound.said,
            );
            assert!(
                bound.said.contains(SPENDS_PHRASE),
                "⛔ ITEM 1010: `{reader}` met {why} and never named `peak_gb_per_task`, which is \
                 what the wrapper will actually spend on the {key} lane. A reader is left thinking \
                 nothing bounds it, and the figure that does is one this repository declares and \
                 can therefore name.\narm: {arm}\nsaid:\n{}",
                bound.said,
            );
            assert_eq!(
                bound.jobs, "unset",
                "⛔ ITEM 1009: naming the wrapper's divisor must not become a bound of this \
                 hook's own. The arm still refuses to divide; only the sentence changed.\narm: \
                 {arm}\nsaid:\n{}",
                bound.said,
            );
        }
    }
}

/// ⭐ **THE CONTROL: WITHOUT A WRAPPER THE LANE REALLY IS UNBOUNDED, AND STILL SAYS SO.**
///
/// ⚠⚠ **Without this arm the clause above is passed by DELETING the word**, and the hook would then
/// keep its silence for the case the operating rules reach most often — `env -u BX`, which item
/// 808's local-only runs use on every commit. A sentence that is true only when a wrapper happens
/// to be configured is the escape hatch rule 6 is about.
#[test]
fn without_a_wrapper_an_unbounded_lane_is_still_called_unbounded() {
    for (reader, key) in LANES {
        let tree = tree_declaring(&format!("bare-{key}"), &decl_offering("", "Plan9"));
        let meminfo = tree.join("meminfo");
        std::fs::write(&meminfo, "MemAvailable:   20000000 kB\n").expect("a staged MemAvailable");

        let bound = drive(&tree, &meminfo, reader);
        let _ = std::fs::remove_dir_all(&tree);

        assert!(
            bound.said.contains("UNBOUNDED"),
            "⛔ ITEM 1010: with no wrapper in the chain the {key} lane IS unbounded — nothing \
             divides anything — and `{reader}` no longer says so. The repair to the wrapped case \
             must not be the word's deletion.\nsaid:\n{}",
            bound.said,
        );
        assert!(
            !bound.said.contains(SPENDS_PHRASE),
            "⛔ ITEM 1010: `{reader}` named `peak_gb_per_task` with no wrapper in the chain. \
             Nothing will spend it: the scalar is the WRAPPER's divisor and this branch runs the \
             lane in this shell. A figure named where it does not apply is the same defect as a \
             figure unnamed where it does.\nsaid:\n{}",
            bound.said,
        );
    }
}

/// ⛔⛔⛔⛔⛔ **THE SCALAR THE WRAPPER WILL SPEND NAMES THE PLATFORM IT WAS MEASURED ON** — item 1010,
/// and this is the half of the fleet question this repository owns.
///
/// `peak_gb_per_task` is derived from `[peak_measured]`, which is `VmHWM` out of procfs (item 1008)
/// — one platform's memory behaviour. **Measured 2026-09-11**: `grep -c peak_measured
/// ~/.claude/remote-build/bin/bx` answers **0**, so the wrapper cannot know whose. Item 932's rule
/// is that a row nobody divides by is a number in a file; this hook is the only reader this
/// repository can give the field item 1008 recorded.
///
/// ⛔⛔ **BOTH BRANCHES, AND THE BOUND ONE IS NOT DECORATION — it is where the first draft of this
/// repair leaked.** It printed the scalar without its platform whenever this hook had managed to
/// divide, on the reasoning that a bounded lane has nothing to worry about. That reasoning is
/// refuted by `bin/bx:2786`: the remote side overwrites the bound unconditionally, so a foreign
/// scalar reaches a build machine whether or not this host could be measured. Driving only the
/// unbound branch would have called that draft finished.
#[test]
fn the_scalar_the_wrapper_will_spend_names_the_platform_it_was_measured_on() {
    let wrapper = staged_wrapper();
    let here = host_platform();
    for (reader, key) in LANES {
        // (what makes this branch the one taken, the rows `[routed]` carries)
        let branches: [(&str, String); 2] = [
            ("unbound", String::new()),
            (
                "bound",
                format!("{key}_kb = 5000000\n{key}_platform = \"{here}\"\n"),
            ),
        ];
        for (branch, rows) in branches {
            let tree = tree_declaring(
                &format!("foreign-scalar-{branch}-{key}"),
                &decl_offering(&rows, "Plan9"),
            );
            let meminfo = tree.join("meminfo");
            std::fs::write(&meminfo, "MemAvailable:   20000000 kB\n")
                .expect("a staged MemAvailable");

            let bound = drive_with(&tree, &meminfo, reader, Some(&wrapper));
            let _ = std::fs::remove_dir_all(&tree);

            assert!(
                bound.said.contains("Plan9"),
                "⛔ ITEM 1010: `{reader}` told the operator what the wrapper will spend on the \
                 {key} lane and not which platform that figure describes. `[peak_measured] \
                 platform` is there to be read — the wrapper never reads it, so a sentence that \
                 omits it leaves the field with no reader at all.\nbranch: {branch}\nsaid:\n{}",
                bound.said,
            );
            assert!(
                bound.said.contains(&here),
                "⛔ ITEM 1010: the sentence names the platform the scalar came from and not the \
                 one this host IS, so a reader cannot tell whether the two differ. Item 1009's own \
                 arm names both for the same reason.\nbranch: {branch}\nsaid:\n{}",
                bound.said,
            );
        }
    }
}

/// ⚠⚠ **A BOUND THIS HOOK DID SET IS NOT CARRIED TO A BUILD MACHINE, AND THE OPERATOR IS TOLD.**
///
/// The two sides of the wrapper differ and its own source is what separates them — `bin/bx` local
/// (1749) takes `RUST_TEST_THREADS="${RUST_TEST_THREADS:-$lthreads}"`, so an exported bound WINS,
/// measured 2026-09-11 by exporting 3 and reading 3 back; remote (2786) writes
/// `RUST_TEST_THREADS=$threads` unconditionally into the ssh payload, and no environment crosses
/// that seam. So *this hook bounded the lane* is true on one side only, and the successful arm says
/// so rather than leaving a reader to assume the number travels.
#[test]
fn a_bound_this_hook_set_says_the_wrapper_replaces_it_on_a_build_machine() {
    let wrapper = staged_wrapper();
    for (reader, key) in LANES {
        let tree = tree_declaring(
            &format!("bounded-{key}"),
            &decl_offering(
                &format!(
                    "{key}_kb = 5000000\n{key}_platform = \"{}\"\n",
                    host_platform()
                ),
                &host_platform(),
            ),
        );
        let meminfo = tree.join("meminfo");
        std::fs::write(&meminfo, "MemAvailable:   20000000 kB\n").expect("a staged MemAvailable");

        let bound = drive_with(&tree, &meminfo, reader, Some(&wrapper));
        let _ = std::fs::remove_dir_all(&tree);

        assert_eq!(
            bound.jobs, "4",
            "⚠ THE STAGING, NOT THE CLAIM: this arm must be the one that COMPUTES — 20,000,000 / \
             5,000,000 — or what follows is asserted about the wrong branch.\nsaid:\n{}",
            bound.said,
        );
        assert!(
            bound.said.contains(SPENDS_PHRASE),
            "⛔ ITEM 1010: `{reader}` bounded the {key} lane and never said that the wrapper \
             discards that bound the moment the lane is shipped — `bin/bx:2786` sets \
             `RUST_TEST_THREADS` unconditionally on the remote side, and ssh carries no \
             environment across. An operator reading only the quotient believes a number that \
             does not travel.\nsaid:\n{}",
            bound.said,
        );
    }
}

/// ⚠⚠ **THE DECLARATION NAMES EXACTLY ONE PLATFORM FOR THE WRAPPER'S DIVISOR.**
///
/// The hook reads it with a line-anchored `sed` for `^platform = `, which is unambiguous only while
/// `[peak_measured]` is the one table spelling the key bare — `[routed]` spells its own per-lane
/// fields `<name>_platform`. A second bare `platform` arriving in any table would make that reading
/// a guess whose answer depends on file order, and nothing else in the tree would notice.
#[test]
fn a_declaration_names_one_platform_for_the_wrappers_divisor() {
    let text = std::fs::read_to_string(workspace_root().join(".claude/remote-build.toml"))
        .expect("this repository's declaration");
    let bare: Vec<&str> = text
        .lines()
        .filter(|line| line.starts_with("platform = "))
        .collect();
    assert_eq!(
        bare.len(),
        1,
        "⛔ ITEM 1010: `.claude/remote-build.toml` has {} line(s) starting `platform = ` and \
         `rust_gates_say_what_spends_instead` takes the FIRST. With more than one the hook names \
         whichever the file happens to list first, which is a guess wearing a reading's clothes; \
         with none it can name no platform at all.\nlines: {bare:?}",
        bare.len(),
    );
}
