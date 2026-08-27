//! **NOTHING IN THIS WORKSPACE MAY TAKE A CHILD'S STANDARD INPUT AND THEN DIE BECAUSE THE CHILD
//! REFUSED BEFORE READING IT** — register item 471.
//!
//! # ⚠⚠⚠⚠⚠ Why this file exists
//!
//! A pipe whose reader has gone answers a write with `EPIPE`. A fixture that spawns a program,
//! writes its input and treats a write failure as fatal is therefore asserting something nobody
//! promised — *that the program will read* — and the cases most worth writing are exactly the ones
//! where it will not: a hook that refuses at its first guard, a `grep -q` that exits on the first
//! match, a CLI that rejects its arguments before touching stdin.
//!
//! It is a RACE while the payload is small, because the bytes fit in the pipe's buffer and the
//! write only fails if the child happens to have gone first. Measured 2026-08-19:
//! `hooks_judge_the_bytes_being_published`'s guard case failed that way ONCE in the first seven
//! runs of a 30-run loop on a loaded build machine, and passed every run on its own — the same
//! shape as item 465's `ETXTBSY`, and the same reason it survived: *a flake*.
//!
//! **The one place that survives it is [`sprag_gate::feeding`]**, which writes, tolerates the
//! child having gone, and closes the handle so a child that DOES read sees end-of-file.
//!
//! # ⚠⚠ What this gate can and cannot claim
//!
//! It is a TEXT scan — this crate takes no dependencies by charter and std has no Rust parser — so
//! it answers one narrow question: *does anything outside the exempt files take a child's stdin
//! handle for itself*. Taking the handle is the step every instance of this defect began with, and
//! it is the step the helper exists to own. A fixture that reached a child's stdin some third way
//! would walk past this, and that is stated here rather than implied.
//!
//! ⚠ The needles are matched against the source with its WHITESPACE REMOVED, because rustfmt
//! chooses between `child.stdin.take()` and the same expression split over three lines by line
//! width alone. A gate that read only one of those spellings would be defeated by formatting.

use sprag_gate::sources::rust_sources;

/// Taking a child's stdin handle, in the two spellings std offers.
const NEEDLES: [&str; 2] = [".stdin.take()", ".stdin.as_mut()"];

/// The sites this gate knowingly permits, each with the reason it is not item 471's shape.
///
/// ⚠⚠⚠ Every entry is re-measured by [`every_exemption_is_still_load_bearing`]: an exemption whose
/// line has gone is a hole held open for a file that no longer needs one, and this project's
/// standing lesson is that a list nobody re-measures ages silently.
const EXEMPT: [(&str, &str); 6] = [
    (
        "crates/sprag-host/src/checkout.rs",
        "feeding a patch to `git apply` when cutting a check its own working copy (register item \
         705). PRODUCT code, so `feeding::feed` is the wrong door for the reason the daemon's own \
         entry below gives — and this child is one that really does refuse mid-write, because a \
         patch git rejects ends the process while the write is still going. It carries what this \
         gate is about: the BrokenPipe arm is matched by name and forgiven so the EXIT STATUS is \
         what decides, every other error answers `false`, and the handle is dropped before the \
         wait",
    ),
    (
        "crates/sprag-gate/src/feeding.rs",
        "the one place: it takes the handle, tolerates the child having gone, and closes it so a \
         child that does read sees end-of-file",
    ),
    (
        "crates/sprag-gate/tests/no_suite_dies_when_a_child_refuses_first.rs",
        "this file, which has to SPELL what it forbids in order to look for it — splitting the \
         needles so they do not match themselves is a trick that quietly stops matching, which is \
         the silent failure this gate exists to prevent",
    ),
    (
        "crates/sprag-mcp/tests/mcp_stdio.rs",
        "a CONVERSATION rather than a feed: it holds the server's stdin open across many requests \
         and a write that fails there means the SERVER DIED, which is a different remedy (say so, \
         with the server's stderr) and not the tolerance this gate is about",
    ),
    (
        "crates/sprag-host/src/bin/sprag-agent-peer.rs",
        "product code that already carries the error rather than panicking on it — `map_err(...)?` \
         on the request write, `let _ = ...` on the hook payload, both deliberate",
    ),
    (
        "crates/sprag-host/src/lib.rs",
        "the daemon handing a run's driver its request (register item 650). PRODUCT code, so \
         `feeding::feed` is the wrong door — that one panics on a write error and a daemon must \
         not. It carries what this gate is about instead: the BrokenPipe arm is matched and \
         tolerated by name, every other error is returned, and the handle is dropped so a driver \
         that does read sees end-of-file",
    ),
];

/// Every file that takes a child's stdin handle, exemptions included.
fn grabs() -> Vec<(String, &'static str)> {
    let mut found = Vec::new();
    for source in rust_sources() {
        let squeezed = source.squeezed();
        for needle in NEEDLES {
            if squeezed.contains(needle) {
                found.push((source.file.clone(), needle));
            }
        }
    }
    found
}

/// ⚠⚠⚠⚠⚠ **THE RATCHET.** A child's stdin is fed through the one place that survives a refusal.
#[test]
fn no_suite_takes_a_childs_stdin_for_itself() {
    let offenders: Vec<_> = grabs()
        .into_iter()
        .filter(|(file, _)| !EXEMPT.iter().any(|(path, _)| file == path))
        .collect();

    assert!(
        offenders.is_empty(),
        "⚠⚠⚠⚠⚠ {} file(s) take a child's stdin for themselves, which is where register item 471's \
         defect begins: a child may REFUSE before it reads, the write then meets a closed pipe, and \
         a fixture that treats that as fatal reports `Broken pipe` instead of the exit status it \
         came for. Feed it through `sprag_gate::feeding::feed`, which tolerates the child having \
         gone and closes the handle for the child that does read.\n{}",
        offenders.len(),
        offenders
            .iter()
            .map(|(file, needle)| format!("  {file} — {needle}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
}

/// ⚠⚠⚠⚠ **AN EXEMPTION THAT NO LONGER APPLIES IS REMOVED BY A RED, NOT BY SOMEBODY NOTICING.**
#[test]
fn every_exemption_is_still_load_bearing() {
    let found = grabs();
    for (path, why) in EXEMPT {
        assert!(
            found.iter().any(|(file, _)| file == path),
            "⚠ {path} is exempted ({why}) and no longer takes a child's stdin at all. The \
             exemption is dead — delete it from EXEMPT, so the next one written there is caught.",
        );
    }
}
