//! What a round may take is written TWICE in this repository's kind document, and the two must
//! not be able to drift — register item 833(3).
//!
//! # ⛔⛔⛔⛔⛔ The fork item 833 left open, and where its answer ended up
//!
//! Item 833 is an owner's decision, and its first clause left a choice the ledger recorded as owed:
//! rank the register BY SEVERITY, or make severity READABLE and show what a round passed over.
//! Item 659 had argued against the first — a loop that always chases the sharpest thing never
//! finishes an axis, and items sharing a seam are cheap because they are worked together.
//!
//! The answer exists and is **neither**: `Reading::admits` says so where it is implemented (*"this
//! is a gate, not a sort"*, item 659's counter-argument kept), and `debt_loop.scxml` says so beside
//! `successor_check`. What nothing held is that the document states the same decision in **two
//! forms** — a predicate a run must pass (`successor_check`) and four prose rules the agent is
//! greeted with (`working_rules` 11 to 14) — and `successor_check`'s own comment says why that is
//! dangerous: the prose is *"one of six fragments concatenated into `start_prompt`; no `cond` reads
//! it, no gate holds a run to it"*.
//!
//! So today a person could re-point the predicate at another program and the rules would go on
//! telling the agent to consult the old one; or rewrite the rules to name a different authority
//! while the predicate kept refusing by its own. **One decision, two homes** — the shape items 855
//! and 864 each paid for, sitting on the decision that chooses every round's work.
//!
//! # ⚠⚠⚠ Why this cannot be a test inside `sprag-plugin`
//!
//! Its subject is the TEXT of one document measured against itself, which is what this crate is
//! for. And `sprag-gate` deliberately declares no dependency on the product, so the document is
//! read as bytes rather than through `LoopKind` — a gate that could only run when the product
//! compiles is not the gate this claim wants.
//!
//! # ⚠⚠ What it does NOT assert, on purpose
//!
//! Not that the rules *say* anything in particular. A gate whose subject is prose gets its needles
//! widened until somebody deletes it (register item 872(2) paid for exactly that). What is held
//! here is a JOIN: the authority the predicate runs must be one the rules name, derived from the
//! document on both sides and spelled in neither.
//!
//! ⚠ And not the composition itself — `Reading::admits` already has three gates over *while
//! anything is critical the admissible set is those*, the cap not being liftable by a severity
//! mark, and the fall-through. Restating those here would be a second authority on one fact.

use sprag_gate::sources::workspace_root;

/// This repository's own loop-kind document — the one `sprag orchestrate --loop_kind debt` opens,
/// and the one whose two recordings this gate joins.
const KIND: &str = "crates/sprag-plugin/src/debt_loop.scxml";

fn document() -> String {
    let path = workspace_root().join(KIND);
    std::fs::read_to_string(&path).unwrap_or_else(|why| {
        panic!(
            "{} is this repository's kind document: {why}",
            path.display()
        )
    })
}

/// The text of the `<data id="…">` named, with its `expr` attribute's content returned whole.
///
/// ⚠⚠ The value is taken as the RAW SLICE between the `expr="` and its closing quote rather than
/// parsed as XML: what this gate needs is *what tokens the document put there*, and a parser that
/// normalised entities or joined the `+`-concatenated fragments would be deciding, on this gate's
/// behalf, what the document said.
fn authored(scxml: &str, id: &str) -> String {
    let opening = format!("<data id=\"{id}\"");
    let at = scxml
        .find(&opening)
        .unwrap_or_else(|| panic!("this repository's kind document authors `{id}`"));
    let rest = &scxml[at..];
    let from = rest
        .find("expr=\"")
        .unwrap_or_else(|| panic!("`{id}` is a `<data>` with an `expr`"))
        + "expr=\"".len();
    let to = rest[from..]
        .find("\"/>")
        .unwrap_or_else(|| panic!("`{id}`'s `expr` is closed"));
    rest[from..from + to].to_owned()
}

/// ⛔⛔⛔⛔⛔ **THE PREDICATE AND THE PROSE NAME ONE AUTHORITY** — register item 833(3).
///
/// # ⚠⚠⚠ The join, derived from the document on both sides
///
/// The program is read out of `successor_check`'s own command line and then looked for in
/// `working_rules`. Nothing here spells its name, so this gate cannot be satisfied by editing the
/// gate, and it goes red from EITHER side: re-point the predicate, and the rules no longer name
/// what decides; rewrite the rules to name something else, and the run is refused by a program the
/// agent was never told to consult.
#[test]
fn the_rules_and_the_predicate_name_one_authority() {
    let scxml = document();
    let predicate = authored(&scxml, "successor_check");
    let rules = authored(&scxml, "working_rules");

    // ── ① THE CONTROL: THIS DOCUMENT AUTHORS BOTH ──────────────────────────────────────────
    //
    // ⚠ A kind that authored neither would satisfy every join below vacuously, and that is a real
    // shape rather than a hypothetical: the template ships `''` for the slots a kind may fill.
    // ⛔⛔ AN EMPTY STRING LITERAL IS EMPTY, and this line is a mutation's finding. The template
    // ships `''` for the slots a kind may fill, so a document that declines one still carries the
    // `<data>` — and the first draft of this control read `''` as two characters of content and
    // waved it through. `LoopKind::working_rules` has the same rule (*declared but empty is this
    // document holds its runs to nothing*), which is why it is the reading and not a special case.
    let declined = |said: &str| -> bool {
        let said = said.trim();
        said.is_empty() || said == "''" || said == "\"\""
    };
    assert!(
        !declined(&predicate) && !declined(&rules),
        "⚠ THE CONTROL: this repository's kind document must author BOTH the predicate that \
         refuses a proposal and the rules the agent is greeted with, or this gate joins nothing.\n  \
         successor_check: {predicate:?}\n  working_rules is {} bytes",
        rules.len(),
    );

    // ── ② THE PROGRAM THE PREDICATE RUNS IS ONE THE RULES NAME ────────────────────────────
    //
    // ⛔⛔⛔⛔⛔ THE WHOLE CLAIM. `successor_check` invokes a binary by name; the rules tell the
    // agent which instrument decides. Those are one decision, and until this gate they were two
    // strings nothing compared — `successor_check`'s own comment says the prose half is read by no
    // `cond` and held by no gate.
    let authority = predicate
        .split_whitespace()
        .skip_while(|word| *word != "--bin")
        .nth(1)
        .unwrap_or_else(|| {
            panic!(
                "⛔⛔⛔ REGISTER ITEM 833(3): `successor_check` names no `--bin`, so what decides \
                 what a round may take cannot be read out of the document at all. It said: \
                 {predicate:?}"
            )
        });
    assert!(
        rules.contains(authority),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 833(3): the predicate that REFUSES a proposal runs `{authority}` \
         and the working rules never name it, so the agent is being told to consult one authority \
         while another one decides. Item 833's fork was answered *a gate, not a sort*, and a gate \
         nobody was told about is a refusal out of nowhere.\n  successor_check: {predicate:?}",
    );

    // ── ③ AND IT RUNS THE MODE WHOSE COMPOSITION THE RULES DESCRIBE ───────────────────────
    //
    // ⚠⚠ The flag is looked for in the NAMED BINARY'S OWN SOURCE rather than compared with a
    // literal this file keeps: a mode renamed on one side and not the other is precisely the drift
    // this gate exists for, and a constant here would be a third copy of the same fact.
    // ⛔⛔⛔⛔⛔ AFTER THE BARE `--`, and that separator is the reason this reads the way it does.
    // The first draft took *the first `--`-prefixed word that is not a cargo flag* and picked up
    // cargo's own argument separator, so the needle became `--` and the check below was satisfied
    // by any source with a comment in it. A mutation that renamed the mode went GREEN, which is
    // how this line came to be written: what is wanted is the first argument the PROGRAM gets, not
    // the first that looks like a flag.
    let mode = predicate
        .split_whitespace()
        .skip_while(|word| *word != "--")
        .find(|word| word.starts_with("--") && *word != "--")
        .unwrap_or_else(|| {
            panic!(
                "⛔⛔ REGISTER ITEM 833(3): `successor_check` passes no mode to `{authority}`, so \
                 it asks for whatever that program does by default — which is the tally, not the \
                 admissibility question. It said: {predicate:?}"
            )
        });
    let binary = workspace_root().join(format!("crates/sprag-gate/src/bin/{authority}.rs"));
    let source = std::fs::read_to_string(&binary).unwrap_or_else(|why| {
        panic!(
            "⛔⛔⛔ REGISTER ITEM 833(3): the document names `{authority}` and this workspace has \
             no such gate binary at {}: {why}",
            binary.display(),
        )
    });
    // ⚠⚠ THE QUOTED SPELLING, not a bare substring: a flag named only in a comment is a program
    // that does not handle it, and the first draft of this assertion could not tell those apart.
    let declared = format!("\"{mode}\"");
    assert!(
        source.contains(&declared),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 833(3): the document asks `{authority}` for `{mode}` and that \
         program's own source declares no such argument ({declared} appears nowhere in it). The \
         loop would go on asking, get whatever an unknown flag produces, and the rules would still \
         say the instrument decides.",
    );
}
