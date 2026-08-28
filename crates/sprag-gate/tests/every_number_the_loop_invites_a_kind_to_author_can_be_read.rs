//! A `<data>` the loop template says a KIND may author must have something that carries it — 494.
//!
//! # What happened twice, and why the second time earned a gate
//!
//! `ai_loop.scxml` writes the sentence *"it is the KIND's to author, like `max_turns` and
//! `reflect_every`"* beside some of its numbers. Item 492 found one of those sentences pointing at
//! a road that did not exist: `context_ceiling` had been authored in `debt_loop.scxml` since
//! 2026-08-18 — argued, dated, measured — and there was no reader, no `Brief` field, no wire key and
//! no `<assign>`, so **the number was 0 on every run this repository had ever driven**. Item 477
//! measured the far end at eight `reviewing` exits out of eight taking the fall-back.
//!
//! Item 494 is that same defect, one `<data>` up, found the next day by sweeping the CLASS instead
//! of the instance. **A premise that produces one defect produces the rest of its class**, and 492
//! had paid the instance while the class stood.
//!
//! # ⚠⚠⚠⚠⚠ Why this cannot be a test inside `sprag-plugin`
//!
//! The claim is about the TEXT of two documents and one Rust file measured against each other, which
//! is what this crate is for — [`sprag_gate::loop_shape`]'s reason, one module over. A test in the
//! plugin could assert that `LoopKind::reflect_after_refusals` returns a number; **nothing there can
//! notice the number nobody has written a reader for yet**, because there is no symbol to name.
//!
//! # ⚠⚠⚠⚠ The pin, and why it is refused from both sides
//!
//! [`CLAIMED`] is an equality rather than a floor. A floor rots exactly the way item 453 measured:
//! rephrase a claim past the needle and the derivation quietly finds one fewer, and a floor never
//! complains about having room. So:
//!
//! * measured ABOVE the pin — the template started claiming a number for a kind. Good, and the road
//!   for it has to exist in the same commit;
//! * measured BELOW the pin — either a claim was withdrawn on purpose, or **the needle went blind**.
//!   The two are indistinguishable from here and both want a person, which is the point.

use std::collections::BTreeSet;

use sprag_gate::authored::{READS, claims, kind_sources, read_ids};
use sprag_gate::loop_shape::DOCUMENT;
use sprag_gate::sources::{rust_sources, workspace_root};

/// Every `<data>` of `ai_loop.scxml` whose own comment invites a KIND to author it, in document
/// order — measured 2026-08-20.
///
/// ⚠ TWO, and that is the whole shape of item 494: the sentence was already written about both when
/// item 492 built the road for one of them.
///
/// # ⚠⚠⚠⚠⚠ FOUR since 2026-08-28, and the two that arrived are not numbers — register item 738
///
/// `reference` and `working_rules` are PROSE, and pinning them here is the moment this gate's
/// subject widened from *a number the template invites a kind to author* to *a decision*. Nothing
/// in the derivation had to change for it, which is the evidence the class was drawn on the right
/// axis: the claim is a sentence about a `<data>`, and what a `<data>` holds was never part of it.
///
/// ⚠⚠ **THE TWO NEW ONES ARE THE DEFECT'S OWN SHAPE ONE LEVEL OUT.** 492 and 494 were decisions a
/// kind could not make because no channel carried them; these were decisions a kind could not make
/// because **the caller was required to make them on every launch** — so they lived in a person's
/// memory, were retyped by hand into each firing, and vanished with the session that held them.
/// The road is the same four steps and this pin is what says a fifth claim cannot arrive quietly.
/// ⚠⚠⚠⚠⚠ FIVE since 2026-08-28's second round, and the fifth arrived because a DIFFERENT gate
/// asked for it — register item 738, layer 1. `Ceiling::ALL` names five things that can end a run,
/// and `sprag-host`'s `every_ceiling_that_can_end_a_run_is_one_this_repositorys_document_set` walks
/// that set with **no exemption arm**. `hold_within_ms` was the one ceiling a kind still could not
/// author, so the honest way to keep that gate strict was to open the channel rather than to write
/// the exemption — and opening it put the claim in the template, where this pin sees it.
const CLAIMED: &[&str] = &[
    "reference",
    "working_rules",
    "hold_within_ms",
    "reflect_after_refusals",
    "context_ceiling",
];

/// Every file a kind's numbers are read in — measured 2026-08-20, register item 498(a).
///
/// ⚠⚠⚠⚠ It is a PIN and not the search. The search is [`kind_sources`], which finds a file by the
/// ROAD its readers travel; this says what that search saw on the day it was written, so that
/// **a second kind arriving is announced** rather than silently vouched for by this one's channels,
/// and **a needle gone blind is announced** rather than reported as a clean tree. A hardcoded path
/// used to be the search itself, which is item 470's *a list with no glob decides alone*.
const KINDS: &[&str] = &["crates/sprag-plugin/src/kind.rs"];

fn document() -> String {
    let path = workspace_root().join(DOCUMENT);
    std::fs::read_to_string(&path).unwrap_or_else(|why| {
        panic!(
            "{} is this workspace's loop template: {why}",
            path.display()
        )
    })
}

/// ⚠⚠⚠⚠⚠ **A NUMBER THE TEMPLATE INVITES A KIND TO AUTHOR MUST HAVE A READER AND A LANDING
/// PLACE** — the class item 492 paid one instance of.
///
/// The two requirements together are exactly *a kind's decision can reach a run*: without a reader
/// the value cannot leave `debt_loop.scxml`, and without the template's own `<assign>` it cannot
/// land in the run's datamodel. Either missing makes the sentence a promise the document cannot
/// keep — and the failure is SILENT, because a `<data>` with a default always reads as a number.
#[test]
fn every_number_the_template_claims_for_a_kind_has_a_reader_and_an_assignment() {
    let scxml = document();
    let sources = rust_sources();
    let kinds = kind_sources(&sources);
    assert!(
        !kinds.is_empty(),
        "⚠⚠⚠⚠⚠ NOTHING IN THIS WORKSPACE READS A KIND'S DOCUMENT (`{READS}`), so every assertion \
         below would pass over an empty set and this gate would be green about nothing — register \
         items 482 and 498(a). Either the road was renamed, in which case teach it here, or the \
         kind side is gone, in which case the template's invitations are all promises.",
    );
    let readable: BTreeSet<String> = kinds
        .iter()
        .flat_map(|file| {
            let kind = sources
                .iter()
                .find(|source| &source.file == file)
                .expect("the walk that found this file still holds it");
            read_ids(&kind.product)
        })
        .collect();

    let mut unheld = Vec::new();
    for claimed in claims(&scxml) {
        if !readable.contains(&claimed.id) {
            unheld.push(format!(
                "`{}` — the template says {:?} and NOTHING IN {kinds:?} READS IT, so no kind can \
                 act on the invitation",
                claimed.id, claimed.said,
            ));
        }
        if !sprag_gate::authored::assigned(&scxml, &claimed.id) {
            unheld.push(format!(
                "`{}` — the template invites a kind to author it and its own `brief` transition \
                 never assigns it, so a carried value would be dropped on arrival",
                claimed.id,
            ));
        }
    }

    assert!(
        unheld.is_empty(),
        "⚠⚠⚠⚠⚠ ITEM 494 — A DECISION NO CHANNEL CARRIES IS A DECISION NOBODY MADE. Each line \
         below is a number this template asks a repository to decide and cannot receive an answer \
         to. It happened for `context_ceiling` and cost item 477's whole measurement (eight of \
         eight `reviewing` exits taking the fall-back, on a live 97-iteration run), then happened \
         again for `reflect_after_refusals` because the round that paid the first one fixed the \
         INSTANCE. The road is four steps and `sprag_plugin::kind::LoopKind::context_ceiling` is \
         the worked example: a reader here, a `Brief` field, the `<assign>`, and — where a caller \
         may override the kind — a wire key.\n{}",
        unheld.join("\n"),
    );
}

/// ⚠⚠⚠⚠⚠ **AND WHAT THE TEMPLATE CLAIMS IS PINNED, so a needle that goes blind says so.**
///
/// # Why the gate above cannot answer this
///
/// Item 470's finding, and it is the sharpest thing that round produced: blinding the needle left
/// **the ratchet itself green**, because a needle that sees nothing reports no offences. *"Does the
/// gate pass?"* and *"does the gate still SEE the shape?"* are different questions, and only a
/// pinned measurement answers the second.
///
/// The template already spells the same claim two ways — `IT IS THE KIND'S TO AUTHOR` and `It is
/// the KIND's to author` — so this is not a hypothetical: one exact phrase was never a safe needle.
#[test]
fn what_the_template_claims_for_a_kind_is_what_this_gate_can_still_see() {
    let found: Vec<String> = claims(&document())
        .into_iter()
        .map(|claimed| claimed.id)
        .collect();
    let pinned: Vec<String> = CLAIMED.iter().map(|id| (*id).to_owned()).collect();

    assert_eq!(
        found, pinned,
        "⚠⚠⚠⚠⚠ EITHER THE TEMPLATE CHANGED OR THIS GATE WENT BLIND, and it cannot tell which.\n\
         MORE than the pin: a `<data>` acquired the claim. Add the reader, the `<assign>` and the \
         pin in the SAME commit — the gate beside this one holds the first two.\n\
         FEWER than the pin: a claim was withdrawn, OR a comment was rephrased past both readings \
         of the needle (the phrase `kind's to author`, and naming both of `max_turns` and \
         `reflect_every`). A withdrawal comes with the reader's deletion; a rephrasing is this \
         gate quietly stopping to work, which is register item 453's whole finding.\n\
         ⚠ ORDER IS DOCUMENT ORDER, so a `<data>` that moved shows up here too — cheap to fix and \
         worth knowing.",
    );
}

/// ⚠⚠⚠⚠⚠ **AND WHICH FILES THE READERS WERE FOUND IN IS PINNED TOO** — register item 498(a), the
/// half the gate above cannot answer.
///
/// # Why a glob needed a pin of its own
///
/// The reader search used to be ONE HARDCODED PATH, and its two failure directions were not
/// symmetric. A reader that MOVED made the gate call a working channel missing — noisy, and it
/// announces itself. A SECOND KIND reading its numbers somewhere else made the gate vouch for
/// channels that kind does not have — **silent**, which is the exact defect item 494 exists to
/// prevent, arriving through the gate built to prevent it.
///
/// The search is now the ROAD (`OuterLoop::authored_…_in`), so a kind in a file nobody has written
/// yet is found. But a glob answers a question the gate above never asks: *how many subjects are
/// there?* Its union is green whether it read two kinds or one, so the SET is pinned — a new kind
/// arriving is a person's decision to make, not a fact for a union to absorb.
#[test]
fn which_files_a_kinds_numbers_are_read_in_is_what_this_gate_can_still_see() {
    let found = kind_sources(&rust_sources());
    let pinned: Vec<String> = KINDS.iter().map(|file| (*file).to_owned()).collect();

    assert_eq!(
        found, pinned,
        "⚠⚠⚠⚠⚠ EITHER THE KIND SIDE CHANGED OR THIS SEARCH WENT BLIND, and it cannot tell which.\n\
         MORE than the pin: a second kind reads the template's numbers. Every claim the gate \
         beside this one holds is now a claim about TWO readers, and the union it checks would go \
         on passing while one of them lacked a channel — check that the new kind reads every \
         claimed id, then add it here.\n\
         FEWER than the pin: the readers moved, were deleted, or the accessor family was renamed \
         past `{READS}`. The first two are a change somebody meant; the third is this search \
         quietly stopping to work, which is register item 453's finding and the reason this pin \
         exists rather than a bare count.\n\
         ⚠ The judge's own crate is never here: it declares no dependencies and cannot hold a \
         reader, while its text quotes every needle this hunts.",
    );
}
