//! **NO SUITE IN THIS WORKSPACE MAY MANUFACTURE THE PROGRAM IT THEN RUNS** — register item 467.
//!
//! # ⚠⚠⚠⚠⚠ Why this file exists
//!
//! `execve` refuses a file any process holds open for writing, with `ETXTBSY` (*"Text file busy"*).
//! Rust's test harness runs its cases on THREADS of one process, so a case that forks to spawn a
//! program inherits every write handle its siblings happen to have open at that instant, and holds
//! each one until its own exec. `O_CLOEXEC` does not close that window — it ends it one exec too
//! late. A case that writes its own stand-in is therefore racing every other case in the binary.
//!
//! Item 465 measured one instance: **10 failures in 30 runs of `sprag-gate` before the fix, 0 in
//! 30 after**, every failure at the same line and every one green again under `--test-threads=1`.
//! That is how it survived as *a flake* rather than being read as what it is. Item 467 was the
//! class — and the class was BIGGER THAN THE LEDGER SAID: it listed nine further sites and there
//! were ten, the tenth (`pane_pty`'s copy of `/bin/sleep`) found by asking the tree instead.
//!
//! **That is the whole reason this gate is a ratchet rather than nine fixed call sites.** A remedy
//! applied to the sites somebody enumerated is a remedy the next site does not get; nothing in a
//! green suite tells anybody the eleventh has arrived, because the failure it brings is a 1-in-3
//! flake in a neighbouring crate.
//!
//! # ⚠⚠ What a LINE SCAN can and cannot claim, said plainly so a green run is not misread
//!
//! This crate takes no dependencies by charter and there is no Rust parser in std, so this cannot
//! understand the code. What it can do is answer two narrow questions about the TEXT — *does any
//! source make a file executable*, and *does any source copy a program out of the system's
//! directories* — which are the two shapes every one of item 467's sites took. A suite that reached
//! for a third shape would walk past this gate, and that is stated here rather than implied.
//!
//! ⚠ It is also why the exemptions are checked BOTH WAYS. An allowlist that is never re-measured is
//! a ledger entry, and this project's standing lesson is that those age: an exemption whose line no
//! longer exists is removed by this gate going red, not by somebody noticing.
//!
//! # What to do instead, when this goes red
//!
//! Put the program in `crates/<crate>/tests/doubles/<suite>/` where git carries it, and reach it
//! through [`sprag_gate::doubles`] — by `PATH` when the product resolves it by name, or by a
//! SYMLINK when the fixture needs it at a path it computed. The per-case parts that used to be
//! substituted into the script (a log path, an exit code, a tail) become DATA files beside the link,
//! which nothing execs. A file nobody writes cannot be busy.

use sprag_gate::sources::{rust_sources, workspace_root};

/// One line that makes a file executable, or copies a program out of the system's directories.
#[derive(Debug)]
struct Manufacture {
    /// Relative to the workspace root, so a message is a path a person can open.
    file: String,
    /// One-indexed, the way an editor counts.
    line: usize,
    /// What the line says, trimmed.
    text: String,
}

/// The sites this gate knowingly permits, each with the reason it is not item 467's shape.
///
/// ⚠⚠⚠ A path here is an exemption for the WHOLE file, which is as coarse as a line scan can be
/// honest about. Every entry is a file nobody spawns a stand-in from, and every one is re-measured by
/// [`every_exemption_is_still_load_bearing`] — an entry whose line has gone is a dead exemption and
/// this gate says so.
const EXEMPT: [(&str, &str); 3] = [
    (
        "crates/sprag-gate/tests/no_suite_runs_a_program_it_wrote.rs",
        "this file, which has to SPELL the shapes it forbids in order to look for them and would \
         otherwise be its own only offender — measured, on the first run of the gate. Splitting the \
         needles so they do not match themselves was the alternative and it is worse: a trick that \
         quietly stops matching is exactly the silent failure this gate exists to prevent",
    ),
    (
        "crates/sprag-gate/src/doubles.rs",
        "the seam itself: its own test STAGES the ETXTBSY window on purpose, holding the write \
         handle open and asserting the refusal, which is how this workspace knows the mechanism is \
         real on the kernel it runs on rather than quoted from a manual page",
    ),
    (
        "crates/sprag-host/src/durability.rs",
        "0o700 on a DIRECTORY, where the execute bit is permission to traverse rather than to run \
         — a directory is never handed to execve, so it carries none of this",
    ),
];

/// The mode literal in `from_mode(0o…)` or `.mode(0o…)`, if the line carries one.
///
/// ⚠ Only OWNER-execute is read. A mode is written for the owner first and this gate is about
/// whether the file can be handed to `execve` by the process that made it.
fn executable_mode_on(line: &str) -> Option<u32> {
    for opener in ["from_mode(0o", ".mode(0o"] {
        let Some(at) = line.find(opener) else {
            continue;
        };
        let digits: String = line[at + opener.len()..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        let mode = u32::from_str_radix(&digits, 8).ok()?;
        if mode & 0o100 != 0 {
            return Some(mode);
        }
    }
    None
}

/// A copy whose source is a program out of the system's own directories — `/bin/cat`, `/bin/sleep`.
///
/// The result is an executable file this process wrote, which is item 467 exactly; a link is the
/// remedy, and it also keeps the basename the fixture wanted.
fn copies_a_system_program(line: &str) -> bool {
    line.contains("fs::copy(") && (line.contains("\"/bin/") || line.contains("\"/usr/bin/"))
}

/// Every line in the workspace that manufactures an executable, exemptions included.
///
/// ⚠ The walk itself is [`sprag_gate::sources`], shared with item 471's ratchet — and it is what
/// drops the COMMENT lines, so a warning that names one of these shapes is not read as the shape.
fn manufactures() -> Vec<Manufacture> {
    let mut found = Vec::new();
    for source in rust_sources() {
        for (line, text) in &source.code {
            if executable_mode_on(text).is_some() || copies_a_system_program(text) {
                found.push(Manufacture {
                    file: source.file.clone(),
                    line: *line,
                    text: text.clone(),
                });
            }
        }
    }
    found
}

/// ⚠⚠⚠⚠⚠ **THE RATCHET.** Nothing outside the files [`EXEMPT`] names may make a file executable.
///
/// The remedy is never a retry on `ETXTBSY` — that is the workaround item 465 refused, and it turns
/// a race into a slower race while leaving the next site to find it again.
#[test]
fn no_source_in_this_workspace_manufactures_a_program_it_could_then_run() {
    let offenders: Vec<_> = manufactures()
        .into_iter()
        .filter(|found| !EXEMPT.iter().any(|(path, _)| found.file == *path))
        .collect();

    assert!(
        offenders.is_empty(),
        "⚠⚠⚠⚠⚠ {} line(s) manufacture an executable, which is register item 467's defect: a file \
         any process holds open for writing cannot be executed, and this workspace's harness forks \
         from threads. Track the program under `crates/<crate>/tests/doubles/<suite>/` and reach it \
         through `sprag_gate::doubles` — by PATH, or by a symlink when the fixture needs it \
         somewhere it computed. NEVER by retrying the exec.\n{}",
        offenders.len(),
        offenders
            .iter()
            .map(|found| format!("  {}:{} — {}", found.file, found.line, found.text))
            .collect::<Vec<_>>()
            .join("\n"),
    );
}

/// ⚠⚠⚠⚠ **AND AN EXEMPTION THAT NO LONGER APPLIES IS REMOVED BY A RED, NOT BY SOMEBODY NOTICING.**
///
/// This project's standing lesson is that a list ages and nothing tells it so. An allowlist entry
/// whose line has gone is a hole held open for a file that no longer needs one — the next write into
/// that file would pass unremarked.
#[test]
fn every_exemption_is_still_load_bearing() {
    let found = manufactures();
    for (path, why) in EXEMPT {
        assert!(
            found.iter().any(|one| one.file == path),
            "⚠ {path} is exempted ({why}) and no longer manufactures anything. The exemption is \
             dead — delete it from EXEMPT, so the next line written there is caught.",
        );
    }
}

/// ⚠⚠⚠ **EVERY TRACKED DOUBLE CARRIES ITS MODE IN THE INDEX**, which is what a fresh checkout gets.
///
/// A double added with mode `100644` is a double every case using it reports the product refusing
/// over — item 384's shape, and the reason [`sprag_gate::doubles::Doubles::program`] checks the
/// filesystem at use. This checks the SOURCE of that mode instead: git's own index, which is what
/// CI and every other machine will actually receive.
///
/// ⚠ Asked of git rather than of the working tree, because the working tree's bit can be right on
/// the machine that added the file and absent everywhere else.
#[test]
fn every_tracked_double_is_executable_in_the_index() {
    let listed = std::process::Command::new("git")
        .args(["ls-files", "-s", "--", "crates"])
        .current_dir(workspace_root())
        .output()
        .expect("git lists what this repository carries");
    assert!(
        listed.status.success(),
        "git could not list this repository's files ({}), and a probe that read nothing must not \
         report it clean: {}",
        listed.status,
        String::from_utf8_lossy(&listed.stderr),
    );
    let listed = String::from_utf8(listed.stdout).expect("git's listing is utf-8");

    let doubles: Vec<(&str, &str)> = listed
        .lines()
        .filter_map(|row| {
            let (mode, rest) = row.split_once(' ')?;
            let path = rest.split('\t').nth(1)?;
            path.contains("/tests/doubles/").then_some((mode, path))
        })
        .collect();
    assert!(
        doubles.len() >= 10,
        "item 467 left this workspace ten tracked doubles and this found {} — a listing that lost \
         them would make this gate vacuous",
        doubles.len(),
    );
    for (mode, path) in doubles {
        assert_eq!(
            mode, "100755",
            "⚠ {path} is a double a suite EXECUTES and the index carries it as {mode}. On a fresh \
             checkout it arrives without the bit and every case that uses it fails for the wrong \
             reason.",
        );
    }
}
