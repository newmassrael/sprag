//! ⛔⛔⛔⛔ **THE BRIEF'S TWO SIDES NAME THE SAME WORDS** — register item 1023.
//!
//! # The face item 1022's gate could not reach
//!
//! Item 1022 asked *does anything READ the word this host publishes?* and answered it for the host
//! crate's wire, where a reader is Rust. The register opened this the same day, on a measurement:
//! `outer.rs` spells three of the run's datamodel words as PRIVATE constants — `UNANSWERED_RULE`,
//! `UNREADABLE_RULE`, `UNWELL_RULE` — and **no Rust anywhere reads them**. They are not dead: the
//! reader is `ai_loop.scxml`, which assigns each out of `_event.data` on the `brief` edge. Point
//! 1022's predicate at this crate and all three go red; pass them with an exemption list and the
//! exemption becomes the gate. So the question had to be re-asked with the DOCUMENT admitted as a
//! reader, which is what this gate does.
//!
//! # ⚠⚠⚠⚠⚠ Both directions, and they guard each other's sight
//!
//! * **written and never read** — item 1022's defect on this edge: a value the driver computes,
//!   publishes and nobody lands;
//! * **read and never written** — worse, and the payload's own comment says why: *"a missing key is
//!   a Lua nil rather than an echoed empty string"*, so the document assigns nil over a decision
//!   its author wrote. Item 428 is that chain breaking one link up, and item 492 measured a number
//!   that was 0 on every run this repository had ever driven.
//!
//! A single-direction ratchet can go blind and stay green (item 470). These two cannot go blind
//! quietly: a scan that stopped finding the payload leaves all 28 read words unwritten, and one
//! that stopped finding the transition leaves all 28 written words unread. The pinned vocabulary
//! below is for the one case neither covers — both going blind at once.

use std::collections::BTreeSet;

use sprag_gate::briefing::{DRIVER, EDGE, Key, OPENS, published, read_on_the_edge};
use sprag_gate::loop_shape::DOCUMENT;
use sprag_gate::sources::{rust_sources, workspace_root};

/// The run's datamodel vocabulary as this edge carries it — measured 2026-09-10 at `89f1b5dc`.
///
/// # ⚠⚠⚠⚠ Why an equality and not a floor
///
/// The two gates beside this one already force the ROAD to exist in the same commit as a new word:
/// add a key to the payload and the document must read it, add a read and the payload must write
/// it. What they cannot answer is *how many words are there* — both directions are green over two
/// empty sets, which is exactly what a needle that has gone blind at both ends produces. A floor
/// rots the way item 453 measured; an equality says which way the vocabulary moved.
///
/// ⚠ Measured ABOVE: the run holds a decision it did not before. Good — and `authored`'s gate one
/// file over is what says a KIND can reach it. Measured BELOW: a decision was withdrawn, or this
/// scan stopped seeing part of the edge.
const CARRIED: &[&str] = &[
    "await_person_ms",
    "closing_rules",
    "context_ceiling",
    // ⛔⛔⛔ REGISTER ITEM 1066, AND THE ONE WORD HERE THAT IS NOBODY'S DECISION. Every other key
    // in this list is a caller's or a kind's; this one is a fact about the binary that composes
    // the payload (`sprag_stamp::BUILD`), so `authored`'s gate one file over is deliberately NOT
    // what vouches for it — no kind may reach it, and the driver-side gate that reads it back off
    // the datamodel is what says the road is real.
    "driver_build",
    "handback_still_ms",
    "hold_within_ms",
    "max_turns",
    "may_answer",
    "milestone",
    "milestone_check",
    "north_star",
    "progress_marks",
    "ready_timeout_ms",
    "reaim_max",
    "reask_max",
    "reference",
    "reflect_after_refusals",
    "reflect_every",
    "screen_rules",
    "service_needles",
    "service_retry_ms",
    "service_retry_text",
    "stall_after_steps",
    "successor_check",
    "turn_within_ms",
    "unanswered_rule",
    "unreadable_rule",
    "unwell_rule",
    "working_rules",
];

fn document() -> String {
    let path = workspace_root().join(DOCUMENT);
    std::fs::read_to_string(&path).unwrap_or_else(|why| {
        panic!(
            "{} is this workspace's loop template: {why}",
            path.display()
        )
    })
}

fn keys() -> Vec<Key> {
    let found = published(&rust_sources());
    assert!(
        !found.is_empty(),
        "⚠⚠⚠⚠⚠ NOTHING WAS FOUND WHERE {DRIVER} BUILDS THE BRIEF — the needle is `{OPENS}` \
         (squeezed), and a probe pointed at nothing must never read as clean. Either the payload \
         moved, in which case teach the needle, or this driver stopped briefing the document at \
         all, in which case every decision a kind authors is going nowhere",
    );
    found
}

/// ⛔⛔⛔⛔⛔ **A DECISION THIS DRIVER PUBLISHES AND THE DOCUMENT NEVER LANDS IS ONE NOBODY MADE.**
#[test]
fn every_word_the_brief_publishes_is_read_on_the_edge_that_lands_it() {
    let read = read_on_the_edge(&document());
    let unread: Vec<String> = keys()
        .iter()
        .filter(|key| {
            key.word
                .as_ref()
                .is_none_or(|word| !read.contains_key(word))
        })
        .map(|key| {
            format!(
                "  {DRIVER}:{} — spelled `{}`, which stands for {}",
                key.line,
                key.spelling,
                key.word.as_ref().map_or_else(
                    || "NOTHING THIS SCAN CAN RESOLVE".to_owned(),
                    |word| format!("`{word}` and the `brief` transition never reads it")
                ),
            )
        })
        .collect();

    assert!(
        unread.is_empty(),
        "⛔⛔⛔⛔ REGISTER ITEM 1023: {} key(s) below cross to the document on the brief and are \
         never read there. That is item 1022's defect on the one edge its gate cannot see — a \
         value computed, put on an event, and dropped by the machine that was handed it — and the \
         run goes on holding whatever the template shipped.\n\
         ⚠ A key this scan cannot RESOLVE is listed too, and it is a red rather than a skip: a \
         spelling nothing declares is one nobody can check, which is what an exemption looks like \
         before it has been written down.\n{}",
        unread.len(),
        unread.join("\n"),
    );
}

/// ⛔⛔⛔⛔⛔ **AND A WORD THE DOCUMENT READS THAT NOBODY WRITES ASSIGNS NIL OVER A DECISION.**
///
/// The payload's own comment is the argument: *"the template ships `''`, so a kind that holds its
/// runs to nothing has its own empty string echoed back rather than nil assigned over what an
/// author wrote"*. An omitted key is not a smaller brief; it is the document deleting what it had.
#[test]
fn every_word_the_edge_reads_is_one_this_driver_writes() {
    let written: BTreeSet<String> = keys().into_iter().filter_map(|key| key.word).collect();
    let read = read_on_the_edge(&document());
    assert!(
        !read.is_empty(),
        "⚠⚠⚠⚠⚠ THE `{EDGE}` TRANSITION WAS NOT FOUND IN {DOCUMENT}, so this gate and the one \
         beside it would both pass over an empty set — the one blindness the two directions cannot \
         announce for each other",
    );

    let unwritten: Vec<String> = read
        .iter()
        .filter(|(word, _)| !written.contains(*word))
        .map(|(word, line)| format!("  {DOCUMENT}:{line} — `_event.data.{word}`"))
        .collect();

    assert!(
        unwritten.is_empty(),
        "⛔⛔⛔⛔ REGISTER ITEM 1023, the direction that costs a run its author's decision: {} \
         read(s) below take a key the brief payload does not carry. In this datamodel that is a \
         Lua NIL assigned over whatever the template shipped — not a smaller brief, a DELETED \
         decision — and nothing at run time says so.\n{}",
        unwritten.len(),
        unwritten.join("\n"),
    );
}

/// ⚠⚠⚠⚠⚠ **AND WHAT THE EDGE CARRIES IS PINNED**, because two empty sets agree perfectly.
#[test]
fn what_this_edge_carries_is_what_this_gate_can_still_see() {
    let found: Vec<String> = keys()
        .into_iter()
        .filter_map(|key| key.word)
        .collect::<BTreeSet<String>>()
        .into_iter()
        .collect();
    let pinned: Vec<String> = CARRIED.iter().map(|word| (*word).to_owned()).collect();

    assert_eq!(
        found, pinned,
        "⚠⚠⚠⚠⚠ EITHER THIS RUN'S VOCABULARY MOVED OR THIS SCAN STOPPED SEEING PART OF IT, and it \
         cannot tell which.\n\
         MORE than the pin: the brief carries a decision it did not. The two gates beside this one \
         have already made the document read it; what is left is a person's glance at a run \
         holding something new, and this line.\n\
         FEWER than the pin: a decision was withdrawn — which comes with the document's read \
         deleted in the same commit — or the payload walk lost a spelling, which is this gate \
         quietly ceasing to work and is register item 453's whole finding.\n\
         ⚠ ORDER IS ALPHABETICAL, not the payload's, so a key that merely moved says nothing here.",
    );
}
