//! 🎯🎯🎯🎯🎯 **THE MOVEMENT WORD THIS REPOSITORY'S CLASSIFIER PRINTS IS ONE THE DRIVER KNOWS** —
//! register item 844's ⑶.
//!
//! # ⛔⛔⛔⛔⛔ Two spellings of one contract, and the drift would be SILENT
//!
//! `Chain::marker()` in `sprag-plugin` is what the driver matches a reply against. `north-star`
//! prints those words. **Nothing held the two together**, and a rename on either side does not
//! break a build: the classifier goes on answering, the verdict is still read, and the movement
//! word lands as `Chain::Unsaid` — which is CHARGED as a step. That is item 844's own failure mode
//! arriving through the door item 844 did not name.
//!
//! ⚠⚠ **THEY CANNOT BE ONE SPELLING, AND THAT IS ITEM 841's DOING RATHER THAN AN OVERSIGHT.** The
//! classifier's crate declares no dependencies on purpose — a gate that stands outside the suite
//! must not be able to fail because the product failed to compile — so it cannot import the
//! driver's vocabulary. What is available instead is this: read the AUTHORITY off the driver's
//! source, and read the classifier's answer off a real RUN of it.
//!
//! # ⚠⚠⚠ Why the classifier is RUN rather than scanned
//!
//! A second scan would compare this file's reading of one source against its reading of another,
//! and both readings would be this file's. Running the program answers what a caller actually
//! receives — the same argument `a_proposal_that_could_be_about_two_things_is_refused` makes about
//! the mode it drives.

use sprag_gate::sources::workspace_root;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Command;

/// Where the driver spells the movement words it will match a reply against.
const DRIVER: &str = "crates/sprag-plugin/src/outer.rs";

/// The function in [`DRIVER`] whose arms ARE the vocabulary.
const AUTHORITY: &str = "pub const fn marker(self) -> Option<&'static str> {";

/// A register with two unrelated roots and one debt the first of them created, so that a classifier
/// has both movements available to print: an unrelated root is `FRESH`, and a debt the work in hand
/// made is `STEP`.
///
/// ⚠ All three are `@sev: critical`, so all three are admissible — a withheld item would be refused
/// before any movement was read, and this file would be about a refusal instead.
const LEDGER: &str = "\
# Ledger
## A. THE SHARPEST THINGS OPEN
@ns-unclassified: 0
@sev-unclassified: 0
@from-unclassified: 0
@paid-uncommitted: 0
@witness-floor: 0
@finish-unclassified: 0

900. **One root this register is holding a run to**
     @ns: open — it stops the loop dead
     @sev: critical — it stops the loop dead
     @from: none

890. **Another root, unrelated to the one above**
     @ns: open — it stops the loop dead too
     @sev: critical — it stops the loop dead too
     @from: none

880. **A debt the first root created**
     @ns: open — made by 900
     @sev: critical — it stops the loop dead too
     @from: 900
";

/// The movement words the DRIVER will match a reply against, read off its own source.
///
/// ⚠ The arms of ONE FUNCTION and nothing wider, and the reason is a number rather than a feeling:
/// measured 2026-09-08, a scan for all-capital words across [`DRIVER`] collects **1866** distinct
/// ones — `ABANDONED`, `ABOUT`, `ABSOLUTE` — because this workspace shouts in its prose. A
/// vocabulary that size admits everything a classifier could print, which is a gate that cannot
/// fail.
///
/// ⚠ 1866 and not 1851: the mark `crate::judge::marked_words` applies counts a SINGLE capital as a
/// capitalised word too, and this file's own doc said 1851 until that was re-read. Fifteen of them
/// are letters — `A`, `I`, `N` — which is exactly the kind of word a laxer scan would hand back.
///
/// ⚠⚠ And a reading of FEWER THAN TWO words is refused where this is called, on this crate's
/// standing doctrine: a probe that cannot see must never read as clean.
fn driver_vocabulary() -> BTreeSet<String> {
    let path = workspace_root().join(DRIVER);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|why| panic!("read the driver at {}: {why}", path.display()));
    let at = text.find(AUTHORITY).unwrap_or_else(|| {
        panic!(
            "⛔⛔⛔ ITEM 844: {DRIVER} no longer spells `{AUTHORITY}`, so this gate is reading for \
             a function that has moved and would pass about nothing. Point it at the new one."
        )
    });
    let body = &text[at + AUTHORITY.len()..];
    let end = body
        .find("\n    }")
        .unwrap_or_else(|| panic!("⛔ the authority's body has no close brace: {body:.200}"));
    body[..end]
        .split("Some(\"")
        .skip(1)
        .filter_map(|rest| rest.split('"').next())
        .map(ToOwned::to_owned)
        .collect()
}

/// A scratch copy of [`LEDGER`] — [`sprag_scratch`] and never `std::env::temp_dir()`, item 794.
fn ledger_on_disk() -> PathBuf {
    let tail = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.subsec_nanos());
    let path = sprag_scratch::scratch_for("sprag-chain-fixture", &format!("{tail}"));
    std::fs::write(&path, LEDGER)
        .expect("the fixture register is written where this run may write");
    path
}

/// Put one proposal to the shipped classifier and hand back the SECOND marked word of its reply —
/// the movement word, which is what the driver reads off `Judgement::explained`.
fn movement(ledger: &std::path::Path, holding: &str, proposal: &str) -> String {
    let run = Command::new(env!("CARGO"))
        .args([
            "run",
            "-q",
            "--locked",
            "-p",
            "sprag-gate",
            "--bin",
            "north-star",
            "--",
            "--admits",
        ])
        .arg(ledger)
        .arg(holding)
        .arg(proposal)
        .current_dir(workspace_root())
        .output()
        .expect("the classifier runs");
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr),
    );
    let mut words = said.split_whitespace();
    assert_eq!(
        words.next(),
        Some("YES"),
        "⚠⚠ THE STAGING: this proposal has to be ADMITTED for a movement word to exist at all. It \
         said:\n{said}",
    );
    words
        .next()
        .unwrap_or_else(|| panic!("⚠ the reply stopped after its verdict:\n{said}"))
        .to_owned()
}

/// ⛔⛔⛔⛔⛔ **BOTH WORDS THE CLASSIFIER PRINTS ARE IN THE DRIVER'S VOCABULARY** — item 844 ⑶.
#[test]
fn the_movement_word_a_classifier_prints_is_one_the_driver_knows() {
    let known = driver_vocabulary();
    assert!(
        known.len() >= 2,
        "⚠⚠ THE STAGING, NOT THE CLAIM: a vocabulary of fewer than two words cannot tell a \
         classifier that names the wrong one from a classifier that names the only one. Read out \
         of {DRIVER}: {known:?}",
    );

    let ledger = ledger_on_disk();
    let holding = "Take item 900";
    let unrelated = movement(&ledger, holding, "항목 890 을 갚아라");
    let created = movement(&ledger, holding, "항목 880 을 갚아라");

    assert!(
        known.contains(&unrelated) && known.contains(&created),
        "⛔⛔⛔⛔⛔ ITEM 844 ⑶: this repository's classifier printed {unrelated:?} and \
         {created:?}, and the driver matches a reply against {known:?}. A word outside that set is \
         dropped as `Chain::Unsaid` — **which is charged as a step** — with the verdict beside it \
         read perfectly and nothing anywhere going red. Rename on one side, rename on both.",
    );
    assert_ne!(
        unrelated, created,
        "⚠⚠⚠ THE CONTROL: a classifier that printed ONE word for both movements would satisfy the \
         assertion above and tell the driver nothing. An unrelated root and a debt the work in \
         hand created are opposite movements, and this register's fixture makes both reachable",
    );

    let _ = std::fs::remove_file(&ledger);
}
