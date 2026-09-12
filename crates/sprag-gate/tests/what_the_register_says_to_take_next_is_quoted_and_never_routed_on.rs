//! ⛔⛔⛔⛔⛔ **A REFLECTION IS SHOWN THE ORDER, AND NOTHING IN THE DOCUMENT DECIDES WITH IT** —
//! register item 659.
//!
//! # The defect: a run is SCORED by a ranking nothing shows it
//!
//! `OuterLoop::admitted` runs a kind's classifier on **every reflection turn**, so what an agent
//! proposes is marked against a derived order — and until item 659 nothing put that order in front
//! of the agent. Measured 2026-09-12: two proposals in a row refused *"counted and not taken …
//! STEP"*, and the refusal was the first thing that named the set. Choosing blindfolded, then being
//! ranked.
//!
//! # ⚠⚠⚠⚠⚠ Why the SECOND claim is the one that needs a gate most
//!
//! The line reaching the agent is a feature; the line reaching a `cond` would be a defect, and a
//! worse one than the item repairs. `Judgement::explained` states the rule this borrows — *the ban
//! is on deciding with it, not on carrying it* — and here it bites harder, because what is carried
//! is a RANKING: a guard reading it would move *what should this repository work on next* out of
//! the register that derives it and into the loop template that other repositories copy. Item 428's
//! whole architecture is that the party doing the work does not certify it, and this is the same
//! axis one question over.
//!
//! # ⚠⚠ Why the tree and why this crate
//!
//! Both claims are about `ai_loop.scxml`'s TEXT — which sends carry the clause, and whether any
//! guard mentions it — so the input is the document and the gate belongs in the lane register item
//! 784 built for tree-reading tests. `sprag-gate` declares no dependencies by charter, so it reads
//! the document rather than compiling it.

use sprag_gate::loop_shape::{DOCUMENT, uncommented, uncommented_lines};
use sprag_gate::sources::workspace_root;

/// What `reflecting` composes the quoted order into.
const SAID: &str = "order_said";

/// The clause a KIND fills to name the program — register item 659's road, the twin of
/// `successor_check`.
const CHECK: &str = "order_check";

fn raw() -> String {
    let path = workspace_root().join(DOCUMENT);
    std::fs::read_to_string(&path).unwrap_or_else(|why| {
        panic!(
            "{} is this workspace's loop template: {why}",
            path.display()
        )
    })
}

fn document() -> Vec<(usize, String)> {
    uncommented_lines(&raw())
}

/// Every line of the document, comments blanked, that mentions `needle`.
fn mentions<'a>(lines: &'a [(usize, String)], needle: &str) -> Vec<&'a (usize, String)> {
    lines
        .iter()
        .filter(|(_, line)| line.contains(needle))
        .collect()
}

/// ⛔⛔⛔⛔⛔ **THE ORDER REACHES THE AGENT ON EVERY WAY OUT OF `reflecting`.**
///
/// # ⚠⚠⚠ Both sends, and the second is the one a reader would forget
///
/// `reflecting` types one of two things: the reflection, or — when a proposal was turned away — the
/// re-ask, which wraps the same prompt in the refusal's words. The re-ask is the turn that has just
/// been told its choice was not what to take, so it is the turn that most needs the order; and it
/// is composed in a different `<assign>`, which is exactly how one of two roads comes to be missed.
///
/// ⚠⚠ **IT IS ASSERTED ON THE COMPOSITIONS AND NOT ON A COUNT OF MENTIONS**: a document that named
/// the clause in an unrelated `<assign>` and sent neither would satisfy a count, and the run would
/// be holding a sentence it never says.
#[test]
fn both_ways_out_of_a_reflection_carry_what_the_register_said() {
    let lines = document();
    // ⚠⚠ THE COMPOSITION IS ONE PLACE — register item 750 put it in a `<data>` rather than inside
    // a `<param>`, so a caller can preview what a reflection is typed and the briefing door can
    // check it is there. That makes ONE author for the sentence, which is what the second claim
    // below can then be stated against.
    let composing: Vec<&(usize, String)> = mentions(&lines, SAID)
        .into_iter()
        .filter(|(_, line)| line.contains("reflect_prompt"))
        .collect();
    assert_eq!(
        composing.len(),
        1,
        "⛔⛔⛔⛔⛔ REGISTER ITEM 659: `{SAID}` must be composed onto `reflect_prompt` exactly once, \
         into the `<data>` a reflection is typed from. None means the order is carried nowhere; \
         more than one means two authors of one sentence, free to drift — which is the shape this \
         workspace refuses everywhere it appears. Found: {composing:?}",
    );
    // ── ⛔⛔⛔ AND BOTH WAYS OUT OF `reflecting` TYPE WHAT THAT COMPOSED ─────────────────────
    //
    // ⚠⚠⚠ `reflecting` types one of two things: the reflection, or — when a proposal was turned
    // away — the re-ask, which wraps the same prompt in the refusal's words. The re-ask is the turn
    // that has just been told its choice was not what to take, so it is the turn that most needs
    // the order, and it is built in a different `<assign>`. That is exactly how one of two roads
    // comes to be missed.
    //
    // ⚠⚠ READ AS ELEMENTS AND NOT AS LINES, which this gate's first draft got wrong twice: the
    // formatter wraps a long `expr` onto the next line, so the id a send names and the element it
    // belongs to are not on one line. What the claim is about is the ELEMENT.
    // ⚠ TO `</send>` AND NOT TO THE OPENING TAG'S `>`: what a send TYPES is in a `<param>` inside
    // it, so an element cut at the first `>` is the tag and never the text — this gate's third
    // draft counted zero that way while the document was perfectly correct.
    let whole = uncommented(&raw());
    let typed = elements(&whole, "<send", "</send>")
        .into_iter()
        .filter(|element| element.contains("prompt.say"))
        .count();
    let carrying = elements(&whole, "<send", "</send>")
        .into_iter()
        .filter(|element| element.contains(TYPED))
        .count()
        + elements(&whole, "<assign", "/>")
            .into_iter()
            .filter(|element| element.contains("reask_prompt") && element.contains(TYPED))
            .count();
    assert!(
        typed > carrying,
        "⚠⚠⚠⚠⚠ THE FIXTURE'S OWN PRECONDITION: this document must send prompts OTHER than a \
         reflection's, or the count below holds of a template where every send is one",
    );
    assert!(
        carrying >= 2,
        "⛔⛔⛔⛔⛔ REGISTER ITEM 659: `{TYPED}` must reach BOTH ways out of `reflecting` — the \
         ordinary reflection's own send, and the re-ask that wraps it. Fewer than two means one \
         road types a prompt with no order in it, and the likelier one to be missed is the re-ask",
    );
}

/// What a reflection is actually typed — `reflect_prompt` with this turn's order added.
const TYPED: &str = "reflect_asking";

/// Every `open …  close` element of `text`, whole, so a claim about an element is not a claim about
/// whichever line the formatter happened to wrap it onto.
fn elements<'a>(text: &'a str, open: &str, close: &str) -> Vec<&'a str> {
    text.match_indices(open)
        .map(|(at, _)| {
            let rest = &text[at..];
            &rest[..rest
                .find(close)
                .map_or(rest.len(), |ends| ends + close.len())]
        })
        .collect()
}

/// ⛔⛔⛔⛔⛔ **AND NOTHING IN THE DOCUMENT DECIDES WITH IT.**
///
/// # ⚠⚠⚠⚠⚠ This is the claim that must not be allowed to rot quietly
///
/// A `cond` reading the order would make this template a second author of a ranking the register
/// derives — and the failure would look like an improvement, because a loop that routed on *what
/// to take next* would appear to be obeying it. What it would actually be doing is taking the
/// choice away from the agent AND from the register at once, which is item 428's architecture
/// inverted: the party that must not certify its own work would instead be the party that no longer
/// chooses it.
///
/// ⚠⚠ The needle is both words, because either spelling would do it: a guard could read the clause
/// the kind authored (`order_check`) or the sentence composed from its answer (`order_said`).
///
/// ⚠ `uncommented_lines` first, on this crate's standing rule — this document explains itself at
/// length and quotes its own ids in prose, so a scan over the raw text would report the commentary.
#[test]
fn no_guard_in_the_document_reads_the_order() {
    let lines = document();
    let deciding: Vec<&(usize, String)> = mentions(&lines, SAID)
        .into_iter()
        .chain(mentions(&lines, CHECK))
        .filter(|(_, line)| line.contains("cond="))
        .collect();
    assert!(
        deciding.is_empty(),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 659: a guard reads the order. The rule is \
         `Judgement::explained`'s — *the ban is on deciding with it, not on carrying it* — and it \
         binds harder here, because what this carries is a RANKING: a `cond` over it moves *what \
         this repository works on next* out of the register that derives it and into a template \
         other repositories copy. Found: {deciding:?}",
    );
}

/// ⚠⚠⚠⚠ **AND THE CLAUSE IS ONLY EVER FILLED FROM THE EVENT** — register item 659, the arm that
/// keeps the two gates above from holding of a frozen sentence.
///
/// # ⛔⛔⛔ What a baked value costs, measured
///
/// `reflect_prompt` is composed ONCE, in `priming`, for the whole session — the document says so at
/// its own send. An order written there freezes: measured 2026-09-12, one session's two reflections
/// carried identical numbers because the prompt they came from predated both. An order is the one
/// thing in this prompt that MOVES, since every item paid or opened changes it.
///
/// ⚠⚠ So `order_said` must be assigned from `_event.data` and from nothing else. An assignment
/// that computed it from another datamodel value would be a snapshot wearing the fix's clothes.
#[test]
fn the_order_is_taken_from_the_event_and_never_composed_from_the_datamodel() {
    // ⚠⚠⚠ READ AS ELEMENTS AND NOT AS LINES, which this gate's first draft got wrong and went red
    // for: an `<assign>` whose `expr` the formatter wrapped onto the next line has a first line
    // carrying only the `location`, so a line-wise reading reports the document's own house style
    // as a defect. What the claim is about is where the VALUE comes from, and that is the element.
    let whole = uncommented(&raw());
    let filled: Vec<&str> = whole
        .match_indices(&format!("location=\"{SAID}\""))
        .map(|(at, _)| {
            let rest = &whole[at..];
            &rest[..rest.find("/>").map_or(rest.len(), |ends| ends + 2)]
        })
        .collect();
    assert!(
        !filled.is_empty(),
        "⚠⚠⚠⚠⚠ NOTHING FILLS `{SAID}` AT ALL, so the two gates above hold of a clause that is \
         always empty — which is a document that composes a sentence it can never say",
    );
    let stale: Vec<&&str> = filled
        .iter()
        .filter(|element| !element.contains("_event.data") && !element.contains("''"))
        .collect();
    assert!(
        stale.is_empty(),
        "⛔⛔⛔⛔ REGISTER ITEM 659: `{SAID}` is filled from something other than this turn's own \
         event. The register moves every time an item is paid or opened, so an order taken from \
         the datamodel is a snapshot — the exact failure that ruled out baking it into \
         `reflect_prompt`, arriving one `<assign>` later. Found: {stale:?}",
    );
}
