//! **NO SOURCE MAY SPELL A `/bin/<name>` THAT ONLY ONE OF THIS WORKSPACE'S PLATFORMS HAS.**
//!
//! # ⚠⚠⚠⚠⚠ Why this file exists
//!
//! The two platforms the suites run on do not agree about what `/bin` holds. On Linux `/bin` is a
//! symlink to `/usr/bin`, so it holds EVERYTHING and any `/bin/<name>` a source spells resolves.
//! macOS's `/bin` is a real directory of about thirty programs, and `true` and `false` are not
//! among them — they are in `/usr/bin`, and there is no `/bin/true` at all.
//!
//! So `/bin/<name>` is exactly the shape that is **green on every Linux run and `NotFound` on the
//! macOS job**: a red that lands hours after the push, in a crate whose author was editing
//! something else, wearing the wrong cause.
//!
//! ⚠⚠⚠ **This workspace had learned that three times and applied it none of them.** It is written
//! on a `pty` doctest, where it was first paid for; it is written into register item 467's own
//! ledger entry, which recorded copying `/bin/true`; and then it came back anyway as the macOS red
//! on `28fb1a6`, which item 467's `doubles` tests and item 471's `feeding` test carried in on the
//! same push — two rounds whose local sweeps were entirely green, because a Linux machine cannot
//! see this defect at all.
//!
//! **A lesson recorded beside one call site does not reach the next one.** That is the whole reason
//! this is a ratchet and not five fixed lines.
//!
//! # ⚠⚠ What this can and cannot claim
//!
//! This crate takes no dependencies by charter and std has no Rust parser, so this cannot
//! understand the code: it answers a question about SPELLING — does any source contain a
//! double-quoted literal starting `/bin/`. A path assembled at run time (`format!("/bin/{name}")`)
//! walks past it, and that is said here rather than implied.
//!
//! ⚠⚠ What it reads out of such a literal is the FIRST PATH COMPONENT and no more, which is not a
//! shortcut — it is what makes the rule true of the three shapes this tree really contains. A
//! literal is sometimes a bare path (`Command::new("/bin/sh")`), sometimes a whole command line
//! handed to a shell (`"/bin/echo YES"`, where the shell execs `/bin/echo` and the rest is
//! argument), and sometimes a path with something appended by the test around it
//! (`"/bin/sh/none"`). In all three the program named is `/bin/<component>`, so that is what is
//! judged. **The first version of this gate read to the closing quote instead and reported
//! fourteen offenders, every one of them a false positive** — which is the measurement that chose
//! this rule.
//!
//! ⚠ It is scoped to `/bin` alone, deliberately. `/usr/bin` is where macOS keeps nearly everything,
//! so a `/usr/bin/<name>` literal is not this defect — and `durability.rs` spells `/usr/bin/vim` as
//! test DATA that is never executed, which a rule reaching into `/usr/bin` would have to either
//! exempt by hand or go red on for a reason that is not true.
//!
//! # What to do instead, when this goes red
//!
//! Reach the program through [`sprag_gate::doubles::system`], which asks `PATH` where this machine
//! keeps it — the same question a shell asks, and the same answer the product's own children get.

use sprag_gate::sources::rust_sources;

/// A double-quoted literal under `/bin/`, and where it is spelled.
#[derive(Debug)]
struct Spelled {
    /// Relative to the workspace root, so a message is a path a person can open.
    file: String,
    /// One-indexed, the way an editor counts.
    line: usize,
    /// The path itself, `/bin/…`.
    path: String,
}

/// The start of the literal this gate looks for.
///
/// ⚠ Spelled whole rather than assembled from pieces. Item 467's ratchet measured the alternative
/// and recorded it: a needle split so it cannot match itself is a trick that quietly stops
/// matching, and that is the silent failure these gates exist to prevent. This file needs no
/// exemption for carrying it, because what follows the prefix HERE is `\` — no path component at
/// all — and the allowlist entries it also spells are, by construction, allowed.
const OPENER: &str = "\"/bin/";

/// This gate's own path, because [`ALLOWED`] spells every path it permits and the scan finds those
/// too.
///
/// ⚠⚠⚠⚠⚠ **WITHOUT THIS, [`every_allowed_path_is_still_spelled_somewhere`] IS VACUOUS** — an entry
/// would prove itself still in use by existing, and the check could never go red no matter how dead
/// the entry was. Measured on the first run of these mutations, which is the only reason it is
/// here: the test was GREEN and meant nothing.
const THIS_FILE: &str =
    "crates/sprag-gate/tests/no_source_spells_a_bin_path_the_other_platform_lacks.rs";

/// The characters a `/bin` path's one component is made of.
///
/// Everything else ENDS it: a quote closes the literal, a space starts an argument, a backslash
/// starts an escape (`/proc` fixture bytes are written `\x00`-separated), and a second slash is
/// something the test appended.
fn is_component(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
}

/// The `/bin` paths this workspace is allowed to spell, each with why it is portable.
///
/// ⚠⚠⚠⚠⚠ **EVERY ENTRY IS RE-MEASURED ON THE MACHINE RUNNING THIS GATE** by
/// [`every_allowed_path_is_really_on_this_machine`]. That is what makes this list a measurement
/// rather than a claim: the day somebody adds `/bin/true` here to quiet a red, the macOS job goes
/// red AT THE LIST, naming the fix. A Linux run can never prove an entry portable, so a Linux run
/// is not what is trusted to.
const ALLOWED: [(&str, &str); 5] = [
    (
        "/bin/sh",
        "POSIX puts the shell at this path and both platforms honour it; it is this workspace's \
         standard pty child and by far the commonest of these",
    ),
    (
        "/bin/bash",
        "macOS ships bash 3.2 at exactly this path, old but present",
    ),
    ("/bin/sleep", "in macOS's /bin, one of the thirty"),
    ("/bin/echo", "in macOS's /bin, one of the thirty"),
    ("/bin/cat", "in macOS's /bin, one of the thirty"),
];

/// The files that must SPELL the shape in order to look for it, with the reason each does.
///
/// ⚠ A whole-file exemption is as coarse as a line scan can honestly be. The entry is checked the
/// other way round by [`every_exemption_is_still_load_bearing`], because this project's standing
/// lesson is that a list ages and nothing tells it so.
///
/// ⚠⚠ The two gates that must spell the prefix in order to search for it — this one and item 467's
/// — need no entry here, and that is not an oversight. What follows the prefix in both is `\`,
/// which is no path component at all, so the scan reads nothing out of them.
const EXEMPT: [(&str, &str); 1] = [(
    "crates/sprag-terminal/src/procfs.rs",
    "synthetic `/proc/<pid>/cmdline` bytes, written `\\x00`-separated as the kernel lays them out. \
     Those strings are the INPUT to a parser and are never handed to the OS — nothing here spawns \
     them, and rewriting the one that says `/bin/true` would change what the parser is being asked \
     about rather than fix anything",
)];

/// Every `/bin/…` literal in the workspace's Rust, exemptions included.
///
/// ⚠ The walk is [`sprag_gate::sources`], shared with items 467 and 471 — and it is what drops the
/// COMMENT lines, which matters more here than anywhere: the `pty` doctest that first recorded this
/// fact says `/bin/true` in prose, and a gate that read its own lesson as the offence would go red
/// on the very note that teaches the fix.
fn spellings() -> Vec<Spelled> {
    let mut found = Vec::new();
    for source in rust_sources() {
        for (line, text) in &source.code {
            let mut rest = text.as_str();
            while let Some(at) = rest.find(OPENER) {
                let after = &rest[at + OPENER.len()..];
                let component: String =
                    after.chars().take_while(|one| is_component(*one)).collect();
                if !component.is_empty() {
                    found.push(Spelled {
                        file: source.file.clone(),
                        line: *line,
                        path: format!("/bin/{component}"),
                    });
                }
                rest = &after[component.len()..];
            }
        }
    }
    assert!(
        found.len() > 50,
        "a scan that found only {} `/bin/…` spellings has stopped matching — this workspace's pty \
         fixtures alone spell more than that, and a probe that reads nothing must never read as \
         clean",
        found.len(),
    );
    found
}

/// ⚠⚠⚠⚠⚠ **THE RATCHET.** Nothing may spell a `/bin` path that is not on the allowlist.
#[test]
fn no_source_spells_a_bin_path_the_other_platform_lacks() {
    let offenders: Vec<_> = spellings()
        .into_iter()
        .filter(|one| !EXEMPT.iter().any(|(path, _)| one.file == *path))
        .filter(|one| !ALLOWED.iter().any(|(path, _)| one.path == *path))
        .collect();

    assert!(
        offenders.is_empty(),
        "⚠⚠⚠⚠⚠ {} source line(s) spell a `/bin` path this workspace has not confirmed on BOTH \
         platforms. Linux's /bin is a symlink to /usr/bin and holds everything, so a Linux sweep \
         cannot see this — it surfaces as a macOS-only `NotFound` hours later. Reach the program \
         through `sprag_gate::doubles::system(\"<name>\")`, which asks PATH where this machine \
         keeps it.\n{}",
        offenders.len(),
        offenders
            .iter()
            .map(|one| format!("  {}:{} — {}", one.file, one.line, one.path))
            .collect::<Vec<_>>()
            .join("\n"),
    );
}

/// ⚠⚠⚠⚠⚠ **AND THE ALLOWLIST IS A MEASUREMENT, TAKEN WHERE IT COUNTS.**
///
/// On the macOS job this is the whole gate: an entry that is not really there fails HERE, naming
/// itself, instead of somewhere downstream as a fixture that could not spawn. On Linux it is much
/// weaker — /bin holds everything — and that asymmetry is the point rather than a shortcoming.
#[test]
fn every_allowed_path_is_really_on_this_machine() {
    for (path, why) in ALLOWED {
        let found = std::path::Path::new(path)
            .metadata()
            .unwrap_or_else(|absent| {
                panic!(
                    "⚠⚠⚠⚠⚠ {path} is on this gate's allowlist — permitted because {why} — and THIS \
                 MACHINE DOES NOT HAVE IT ({absent}). The allowlist is what every source in this \
                 workspace may spell, so an entry that is not portable is the defect itself, one \
                 step earlier. Take it off the list and send its call sites through \
                 `sprag_gate::doubles::system`.",
                )
            });
        assert!(
            found.is_file(),
            "⚠ {path} is allowed because {why}, and on this machine it is not a file at all",
        );
    }
}

/// ⚠⚠⚠⚠ **AN ALLOWLIST ENTRY NOTHING USES IS REMOVED BY A RED, NOT BY SOMEBODY NOTICING.**
///
/// A permission held open for a path no source spells any more is a hole the next line written
/// there would pass through unremarked — and it is also a claim about macOS that nothing in this
/// workspace still needs to be true.
#[test]
fn every_allowed_path_is_still_spelled_somewhere() {
    let found: Vec<_> = spellings()
        .into_iter()
        .filter(|one| one.file != THIS_FILE)
        .collect();
    for (path, why) in ALLOWED {
        assert!(
            found.iter().any(|one| one.path == path),
            "⚠ {path} is allowed ({why}) and no source spells it any more. The entry is dead — \
             delete it, so the list stays the set of claims this workspace is actually making.",
        );
    }
}

/// ⚠⚠⚠ **AND SO IS AN EXEMPTION.** The same rule as item 467's ratchet, for the same reason.
#[test]
fn every_exemption_is_still_load_bearing() {
    let found = spellings();
    for (path, why) in EXEMPT {
        assert!(
            found.iter().any(|one| one.file == path),
            "⚠ {path} is exempted ({why}) and no longer spells a `/bin` path. The exemption is \
             dead — delete it from EXEMPT, so the next line written there is caught.",
        );
    }
}
