//! The debt-repayment loop's decisions must move OUT of the driver, never back in — item 470.
//!
//! # What this gate is for
//!
//! The loop is driven by `ai_loop.scxml` and a Rust driver. Item 470 measured that the DECISIONS
//! are in the driver: a table keyed by the document's own states, which is a second copy of the
//! topology. Stages 2 and 3 of the repayment are refuted at the pinned SCE (item 483 — a host
//! cannot register its own `<send>`/`<invoke>` type), so the decisions cannot all move yet.
//!
//! ⚠⚠⚠⚠⚠ **AND MEANWHILE IT GREW.** The register recorded 153 state-keyed sites on 2026-08-19;
//! this gate measured more the next day, added by the very rounds that were paying the item down.
//! Nothing said so, because nothing was counting.
//!
//! # ⚠⚠⚠⚠⚠ Why the pin is EXACT rather than a ceiling
//!
//! A ceiling rots. Pay the debt down and the ceiling stays where it was, floating above the
//! measurement, biting nothing — green forever in the voice of a working gate, which is register
//! item 453's finding in a different costume. There is no moment at which anybody is told the
//! ceiling has gone slack, because a ceiling never complains about having room.
//!
//! So the pin is an equality and it is refused from BOTH sides:
//!
//! * measured ABOVE the pin — a decision moved INTO the driver. That is the regression, and the
//!   round that did it must say why in the register.
//! * measured BELOW the pin — a decision moved OUT of it. That is the debt being PAID, and the pin
//!   comes down with it in the same commit, so the gate is never looser than the truth.
//!
//! ⚠ The cost is stated rather than hidden: an ordinary driver edit that adds or drops a mention of
//! a state goes red here and wants one number changed. That is the point. `build.rs` in the plugin
//! makes the same trade in its own words — *there is deliberately no glob, so that adding a machine
//! is a decision somebody makes rather than a side effect of creating a file.*

use sprag_gate::loop_shape::{
    DOCUMENT, StateKeyed, declared_acts, document_states, state_keyed, tally,
};
use sprag_gate::sources::{rust_sources, workspace_root};

/// Every state of `ai_loop.scxml` and how many places this workspace's SHIPPING Rust keys
/// behaviour on it — measured 2026-08-20, and every row is a debt.
///
/// ⚠ States at `0` are the goal, not filler: a state the document decides entirely by itself needs
/// no line of Rust naming it. `converged`, `exhausted` and `failed` reaching zero would mean the
/// loop's ENDINGS are the document's alone.
const DRIVER_ARMS: &[(&str, usize)] = &[
    ("awaiting_human", 10),
    ("blocked", 8),
    ("cancelled", 9),
    ("closing", 8),
    ("converged", 11),
    ("disputing", 8),
    ("exhausted", 13),
    ("failed", 11),
    ("held", 8),
    ("idle", 9),
    ("judging", 14),
    ("orders", 8),
    ("peer_gone", 8),
    ("priming", 9),
    ("redirecting", 8),
    ("reflecting", 8),
    ("restarting", 8),
    ("resuming", 8),
    ("reviewing", 8),
    ("running", 8),
    ("screening", 8),
    ("service_down", 8),
    ("standing", 8),
    ("standing_down", 8),
    ("stopping", 8),
    ("work", 9),
    ("working", 10),
];

/// How many acts `ai_loop.scxml` declares for itself — one per `<onentry>`.
///
/// ⚠⚠ This side must GROW. A ceiling on the Rust alone can be satisfied by DELETING behaviour
/// instead of moving it, and a loop that decides less is not a loop that decides in its document.
const DECLARED_ACTS: usize = 11;

fn document() -> String {
    let path = workspace_root().join(DOCUMENT);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|why| panic!("{} is this loop's document: {why}", path.display()))
}

fn measured() -> (Vec<String>, Vec<StateKeyed>) {
    let states = document_states(&document());
    let sites = state_keyed(&rust_sources(), &states);
    (states, sites)
}

/// ⚠⚠⚠⚠⚠ **A PROBE POINTED AT NOTHING MUST NEVER READ AS CLEAN.** Every other gate here is an
/// equality, and an equality against an empty measurement is satisfied by the measurement having
/// failed. This is the one that says the walk found the loop at all.
#[test]
fn the_measurement_reaches_the_loop_it_is_judging() {
    let (states, sites) = measured();

    assert!(
        states.len() > 20,
        "`{DOCUMENT}` declares the loop's states and this walk found only {}: a reader pointed at \
         the wrong file answers about the wrong file",
        states.len(),
    );
    assert!(
        sites.len() > 50,
        "the driver keys behaviour on this document's states in dozens of places and this walk \
         found only {}: the ratchet is measuring nothing and would be green forever",
        sites.len(),
    );

    let files: std::collections::BTreeSet<&str> =
        sites.iter().map(|site| site.file.as_str()).collect();
    assert!(
        files.contains("crates/sprag-plugin/src/outer.rs"),
        "the loop's driver is where item 470 measured the defect and the walk must reach it: {files:?}",
    );
}

/// ⚠⚠⚠⚠ The pin names the DOCUMENT's states, so a state added there cannot slip in unpinned.
///
/// Without this, a new state's arms would be counted by nobody: `DRIVER_ARMS` is a list, and a list
/// with no glob decides alone. Here the document is the glob.
#[test]
fn the_pin_names_this_documents_states_and_no_others() {
    let (states, _) = measured();
    let pinned: std::collections::BTreeSet<&str> =
        DRIVER_ARMS.iter().map(|(state, _)| *state).collect();
    let declared: std::collections::BTreeSet<&str> = states.iter().map(String::as_str).collect();

    let unpinned: Vec<&&str> = declared.difference(&pinned).collect();
    let stale: Vec<&&str> = pinned.difference(&declared).collect();

    assert!(
        unpinned.is_empty() && stale.is_empty(),
        "`{DOCUMENT}` and this pin have to name the same states.\n  \
         the document declares and the pin does not mention: {unpinned:?}\n  \
         the pin mentions and the document no longer declares: {stale:?}\n\
         A state the pin does not name is a state whose driver arms nothing counts.",
    );
}

/// ⚠⚠⚠⚠⚠ The ratchet itself — refused from BOTH sides, and the refusal says which.
#[test]
fn no_decision_moves_back_into_the_driver_and_the_pin_follows_the_ones_that_leave() {
    let (states, sites) = measured();
    let counted = tally(&sites, &states);

    let mut grew = Vec::new();
    let mut shrank = Vec::new();
    for (state, pinned) in DRIVER_ARMS {
        let now = counted.get(*state).copied().unwrap_or_default();
        if now == *pinned {
            continue;
        }
        let where_at: Vec<String> = sites
            .iter()
            .filter(|site| site.state == *state)
            .map(|site| format!("{}:{}", site.file, site.line))
            .collect();
        let line = format!("  {state}: pinned {pinned}, measured {now}  {where_at:?}");
        if now > *pinned {
            grew.push(line)
        } else {
            shrank.push(line)
        }
    }

    assert!(
        grew.is_empty(),
        "⚠⚠⚠ A DECISION MOVED INTO THE DRIVER. Item 470's whole finding is that the loop's \
         behaviour lives in Rust rather than in `{DOCUMENT}`, and these states gained arms:\n\
         {}\n\
         If the decision belongs in the document, say it there — a `cond` is the datamodel's and \
         needs no registry (item 470 stage 1, `8ac134a`). If it is an EFFECT and genuinely belongs \
         here, raise the pin in the same commit and say why in the register.",
        grew.join("\n"),
    );

    assert!(
        shrank.is_empty(),
        "The debt got SMALLER and the pin did not follow, so this gate is now looser than the \
         truth and would not notice the next regression:\n\
         {}\n\
         Lower these numbers in the same commit. A ratchet that only ever refuses increases drifts \
         above the measurement and goes quietly green forever (item 453).",
        shrank.join("\n"),
    );
}

/// ⚠⚠ The other side of the trade: the document must not decide LESS than it does today.
#[test]
fn the_document_keeps_every_act_it_has_taken_over() {
    let acts = declared_acts(&document());

    assert!(
        acts >= DECLARED_ACTS,
        "`{DOCUMENT}` declared {acts} acts and the pin says {DECLARED_ACTS}. Behaviour left the \
         document, which is item 470 running backwards: a ceiling on the driver's arms can be \
         satisfied by deleting an act instead of moving one.",
    );
    assert_eq!(
        acts, DECLARED_ACTS,
        "the document declares MORE acts than the pin, which is the debt being paid — raise \
         `DECLARED_ACTS` to {acts} in the same commit so the floor stays under the truth",
    );
}
