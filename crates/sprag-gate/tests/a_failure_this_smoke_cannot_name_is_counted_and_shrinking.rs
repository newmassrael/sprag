//! ⛔⛔⛔⛔⛔ **A FAILURE `sprag-smoke` CANNOT NAME IS COUNTED, AND THE COUNT MAY ONLY FALL** —
//! register item 1121.
//!
//! # ⛔⛔⛔ What a description costs, measured on a red that stood for five runs
//!
//! A register holds a red by NAME: an item claims one, and a later run's report is matched against
//! that claim. **A name only works if it is the same text every run.** Measured 2026-09-15, `pixel
//! (linux)` had been red on five consecutive CI runs with no item holding it, and every one of its
//! six failures was built with `format!` — the check's identity fused with that run's pane ids and
//! PSI percentages:
//!
//! ```text
//! FAILED: the daemon and the client agree on ONE pane to drive (daemon [0, 1, 2, 3, 5], painted Ok([0]))
//! ```
//!
//! Nothing can claim that: the next run spells it differently. So `north-star` printed `0 standing`
//! for five runs about a job that was red every one of them — item 1115.
//!
//! # ⚠⚠⚠⚠⚠ Why a ratchet and not a demand for zero — rule 5
//!
//! 234 call sites still fuse, and converting them is mechanical work no single round should hide
//! inside another item. A gate demanding zero would be red for as long as that took and therefore
//! read by nobody, which is the shape `a_test_this_workspace_switches_off_is_counted_and_says_why`
//! states one file over. So the population admits members that have not left yet, the target is
//! **did not grow**, and a legitimate conversion lowers the number IN THE SAME EDIT.
//!
//! ⛔ **The type is what stops it growing back**: `Report::check` and `Report::checked` take a
//! `&'static str`, so a fused identity cannot reach them at all. `describing` is the only door a
//! fused string fits through, and this counts that door.
//!
//! # ⚠⚠ And the second population: a name that identifies TWO checks
//!
//! A stable identity that two call sites share is a name a claim cannot resolve — it would hold
//! both, which is the coarse-claim defect arriving through the name rather than through the filter.
//! Measured here as its own figure, because it is a different fault from fusing and a single number
//! over the two would let one hide the other.

use std::collections::BTreeMap;

use sprag_gate::sources::workspace_root;

/// The producer this gate is about.
const SMOKE: &str = "crates/sprag-gui/src/bin/sprag-smoke.rs";

/// The reader that has to understand what the producer prints.
const READER: &str = "crates/sprag-gate/src/bin/north-star.rs";

/// What the producer writes before a failure it can NAME, and the reader takes as one.
const NAMED: &str = "FAILED: ";

/// What the producer writes before a failure it can only DESCRIBE, and the reader must not take.
///
/// ⚠ The two are spelled here once and asserted to appear on both sides, which is the only thing
/// holding a protocol whose halves live in two crates.
const DESCRIBED_MARK: &str = "FAILED? ";

/// How many checks still fuse their identity with the run's evidence, measured 2026-09-15.
///
/// ⚠⚠ **AN EQUALITY AND NOT A CEILING**, on `TESTS_SWITCHED_OFF`'s stated reason: a floor above the
/// count is that many conversions admitted in silence. A round that converts one lowers this in the
/// same edit, and the refusal below names the figure to write.
const DESCRIBED: usize = 234;

/// How many stable identities are shared by more than one call site, measured 2026-09-15.
///
/// ⚠ Same shape and the same rule. Zero is reachable here — unlike [`DESCRIBED`] it needs no API
/// change, only two sentences told apart — so this is the one of the two that should empty first.
const IDENTITIES_SHARED: usize = 15;

/// ⛔⛔⛔⛔⛔ **THE TWO MARKS ARE ONE VOCABULARY WITH TWO MOUTHS** — register item 1121, and the
/// hole a mutation found.
///
/// The producer prints `FAILED:` for a name and `FAILED?` for a description; the reader takes the
/// first and refuses on the gap. **Measured while writing this: changing the producer to print
/// `FAILED:` for a description broke nothing** — the reader's gates run on hand-written fixtures,
/// so both sides could be edited apart and each would stay green while the protocol between them
/// was gone. A reader that agreed with a producer only by coincidence is this crate's oldest defect
/// class, and here it would report a run's descriptions as claimable names.
///
/// ⚠⚠ Asserted as SOURCE TEXT rather than by calling either side: the producer's `Report` lives
/// inside a binary and the reader inside another crate's binary, so there is no value to share —
/// what can be held is that neither spelling may move without the other, which is what this asks.
#[test]
fn the_producer_and_the_reader_spell_the_two_marks_the_same_way() {
    let producer = smoke();
    let reader = std::fs::read_to_string(workspace_root().join(READER))
        .unwrap_or_else(|why| panic!("⛔ REGISTER ITEM 1121: cannot read {READER}: {why}"));
    for (mark, what) in [
        (NAMED, "a failure it can NAME"),
        (DESCRIBED_MARK, "a failure it can only DESCRIBE"),
    ] {
        assert!(
            producer.contains(mark),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 1121: {SMOKE} no longer writes `{mark}` for {what}. The \
             reader takes `{NAMED}` as a claimable name and counts `{DESCRIBED_MARK}` among the \
             failures it cannot name — move one spelling and the reader silently reads a \
             description as a name, or stops seeing failures altogether",
        );
        assert!(
            reader.contains(mark),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 1121: {READER} no longer reads `{mark}`, which {SMOKE} still \
             writes for {what}. The two halves of this protocol are in different crates and \
             nothing but this line keeps them in step",
        );
    }
}

/// The producer's source, read as text.
///
/// ⛔ **AN UNREADABLE FILE IS A PANIC, NEVER AN EMPTY COUNT** — this crate's standing rule: a
/// probe pointed at nothing reads exactly like a producer with nothing left to convert, and that is
/// the one conclusion a ratchet must never draw on its own.
fn smoke() -> String {
    let path = workspace_root().join(SMOKE);
    std::fs::read_to_string(&path).unwrap_or_else(|why| {
        panic!(
            "⛔ REGISTER ITEM 1121: this gate counts the checks {} cannot name and could not read \
             it: {why}. An unreadable producer must not report as a converted one",
            path.display(),
        )
    })
}

/// Every call of one of the three doors, as `(door, byte offset)`.
///
/// ⚠ A scan for the method name rather than a parse: this crate declares no dependencies, and what
/// has to be true is only which door a call went through — a fact the spelling carries whole.
fn calls(src: &str, door: &str) -> Vec<usize> {
    let needle = format!(".{door}(");
    src.match_indices(&needle).map(|(at, _)| at).collect()
}

/// ⛔⛔⛔⛔⛔ **THE CHECKS THIS PRODUCER CANNOT NAME ARE COUNTED, AND DID NOT GROW.**
#[test]
fn every_check_that_fuses_its_evidence_into_its_identity_is_counted() {
    let src = smoke();
    let described = calls(&src, "describing").len();

    // ⚠⚠⚠ THE PREMISE, BEFORE THE FIGURE: a file this scan found no checks at all in would make
    // every claim below true of nothing, which is how a renamed method retires a gate in silence.
    assert!(
        calls(&src, "check").len() > 100,
        "⚠⚠⚠ REGISTER ITEM 1121: this scan found almost no `check` calls in {SMOKE}, so the count \
         below is about a file this gate can no longer read — the doors were renamed, or the \
         producer moved. Re-derive the three names before trusting any number here",
    );

    assert_eq!(
        described, DESCRIBED,
        "⛔⛔⛔⛔⛔ REGISTER ITEM 1121: {described} check(s) still fuse their identity with the \
         run's evidence, and this gate was written at {DESCRIBED}. A register holds a red by NAME, \
         and a description is different text every run — which is why `pixel (linux)` could be red \
         for five consecutive runs with no item able to hold it (item 1115). ⇒ If you CONVERTED \
         one, write {described} here in the same edit. If you ADDED one, do not: move the \
         interpolated part into `Report::checked`'s `seen` and leave the sentence a literal",
    );
}

/// ⛔⛔⛔ **AND A NAME THAT IDENTIFIES TWO CHECKS IS NOT A NAME** — register item 1121's second
/// population.
///
/// A claim naming a shared identity holds both checks, so one of them going green retires a claim
/// about the other. That is the coarse-claim defect arriving through the NAME, where no `--exact`
/// can reach it.
#[test]
fn every_stable_identity_this_smoke_prints_belongs_to_one_check() {
    let src = smoke();
    // ⚠ The identity is the literal that opens a `check`/`checked` call, which is exactly the set
    // the `&'static str` parameter admits — so this reads the same population the type does.
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for door in ["check", "checked"] {
        for at in calls(&src, door) {
            let after = &src[at + door.len() + 2..];
            let rest = after.trim_start();
            let Some(body) = rest.strip_prefix('"') else {
                continue;
            };
            let Some(end) = body.find('"') else {
                continue;
            };
            *seen.entry(body[..end].to_owned()).or_default() += 1;
        }
    }
    assert!(
        seen.len() > 100,
        "⚠⚠⚠ THE PREMISE: this reader found {} identity literal(s), which is too few to be this \
         producer — it has stopped seeing the shape it counts",
        seen.len(),
    );
    let shared: Vec<(&String, &usize)> = seen.iter().filter(|(_, count)| **count > 1).collect();
    assert_eq!(
        shared.len(),
        IDENTITIES_SHARED,
        "⛔⛔⛔ REGISTER ITEM 1121: {} stable identit(ies) are shared by more than one check, and \
         this gate was written at {IDENTITIES_SHARED}. A claim naming one holds both, so one going \
         green retires the claim about the other. ⇒ Tell the two apart by what each is ABOUT, and \
         write the new figure here in the same edit. Shared: {shared:?}",
        shared.len(),
    );
}
