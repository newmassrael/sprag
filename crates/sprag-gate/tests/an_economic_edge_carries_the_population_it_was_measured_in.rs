//! **A MEASURED NUMBER MUST KEEP DESCRIBING THE SESSIONS IT WAS MEASURED ON** — register item 493.
//!
//! # What was wrong, and why every gate was green through it
//!
//! `reviewing` replaces a session when `context - floor >= 20 * cold`. Five fixtures drove that
//! edge with one borrowed trio — cold 7,000, floor 38,500, a last reading of 466,013 — taken from a
//! PLAIN agent session. A session with this daemon's MCP tools loaded pays a different standing
//! cost, so in the repository that actually runs this loop the break-even is 600,970 rather than
//! 178,500, and **at 466,013 the trade loses**: the one session five gates priced as *"long past
//! break-even"* would have kept its session and gone back to work.
//!
//! Nothing could go red. The gates asserted the arm was REACHABLE and were right; what they left a
//! reader believing — that it is ORDINARY — is not a proposition any of them held.
//!
//! # ⚠⚠⚠⚠⚠ So the pin is over THREE artefacts, and the sum is recomputed
//!
//! * the DOCUMENT's guards, which is where the multiplier lives;
//! * the TEMPLATE's paragraph, which states the measurement, its date and its rate;
//! * the FIXTURE's trio, which is what every gate drives.
//!
//! A round that re-measures must move the paragraph and the fixture together, and a round that
//! re-prices the trade must move the guards and the paragraph together. Each one alone is red here.
//! ⚠⚠ The rate is required and required to be a MINORITY: *reachable* and *ordinary* are different
//! claims, and it was the second that this file exists to stop being made by silence.
//!
//! ⚠ `sprag_gate::economics`'s own table holds the other end — every derivation this file trusts is
//! exercised there on text that file wrote, so a red HERE is a claim about the product.

use sprag_gate::economics::{self, FIXTURE, SAMPLE, TEMPLATE};
use sprag_gate::sources::workspace_root;

/// Read one of the two artefacts, saying which is missing rather than panicking on an `unwrap`.
fn read(relative: &str) -> String {
    let path = workspace_root().join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|why| panic!("⚠ `{}` must be readable: {why}", path.display()))
}

/// ⚠⚠⚠⚠ **THE DOCUMENT'S TWO GUARDS PRICE A HANDOVER THE SAME WAY, AND THAT PRICE IS WHAT THE
/// PARAGRAPH DOES ITS SUM WITH.**
///
/// `review.done` and `review.none` each carry an economic edge. Two multipliers would make *did the
/// reviewer find a habit* decide what a restart costs — register item 424(a)'s own argument, which
/// the plugin's text gate holds on the guards' SHAPE. This holds the number itself, and hands it to
/// the pin below so that re-pricing the trade cannot leave the prose behind.
#[test]
fn both_guards_price_a_handover_at_one_multiplier() {
    let scxml = read(TEMPLATE);
    let multipliers = economics::guard_multipliers(&scxml);

    assert!(
        !multipliers.is_empty(),
        "⚠⚠⚠⚠⚠ no guard in `{TEMPLATE}` compares `context - floor` against a multiple of `cold`. \
         The economic door is what keeps a loop from replacing a session it should have kept; a \
         document without it decides handovers on capacity alone.",
    );
    assert!(
        multipliers.windows(2).all(|two| two[0] == two[1]),
        "⚠⚠⚠⚠ the guards disagree about what a cache write costs: {multipliers:?}. `review.done` \
         and `review.none` must price a handover identically, or whether the reviewer found a habit \
         silently changes the trade.",
    );

    // ⚠⚠⚠ AND THE FIXTURE'S OWN ARITHMETIC IS AT THE SAME PRICE. `Billed::toll` spells the
    // multiplier — the one number item 493's repair could not derive from anything — so a document
    // re-priced to fifteen would leave every premise assertion in the plugin computing a trade this
    // loop does not make, and each of them would still be green.
    let spelled = economics::fixture_multipliers(&read(FIXTURE));
    assert!(
        !spelled.is_empty(),
        "⚠⚠ `{FIXTURE}` no longer computes a toll (`{}`), so nothing there can state the premise \
         the economics gates are driven on",
        economics::FIXTURE_TOLL,
    );
    assert!(
        spelled
            .iter()
            .all(|priced| Some(priced) == multipliers.first()),
        "⚠⚠⚠⚠⚠ the fixture prices a cache write at {spelled:?} and the document's guards at \
         {multipliers:?}. The fixture's `pays()` would then answer a question the loop is not \
         asking, and every gate written on it would be green about the wrong trade.",
    );
}

/// ⚠⚠⚠⚠⚠ **THE MEASUREMENT, THE ARITHMETIC AND THE FIXTURE ARE ONE CLAIM** — the pin item 493
/// leaves behind.
///
/// It fails in each of the four ways the claim can rot, and says which:
///
/// | what moved alone | what a reader would have believed |
/// |---|---|
/// | the fixture's `cold` or `floor` | the gates drive a population no paragraph describes |
/// | the paragraph's numbers | the fixture prices a trade the prose denies |
/// | the guards' multiplier | the stated break-even is arithmetic nobody redid |
/// | the fixture's `context` | the economics gates stand short of the door they demonstrate |
#[test]
fn the_stated_break_even_is_the_fixtures_own_and_the_sum_is_redone() {
    let scxml = read(TEMPLATE);
    let rust = read(FIXTURE);

    let said = economics::stated(&scxml).unwrap_or_else(|why| panic!("{why}"));
    let fixture = economics::sample(&rust).unwrap_or_else(|why| panic!("{why}"));
    let multipliers = economics::guard_multipliers(&scxml);
    let priced = *multipliers
        .first()
        .expect("`both_guards_price_a_handover_at_one_multiplier` holds this end");

    assert_eq!(
        said.multiplier, priced,
        "⚠⚠⚠⚠⚠ the paragraph does its sum at {} to one and the document's guards trade at {priced} \
         to one. Re-pricing the trade means re-doing the arithmetic beside it — otherwise the \
         template teaches a break-even the loop does not use. Sentence: {}",
        said.multiplier, said.sentence,
    );
    assert_eq!(
        (said.cold, said.floor),
        (fixture.cold, fixture.floor),
        "⚠⚠⚠⚠⚠ ITEM 493: `{TEMPLATE}` says it measured cold {} and floor {}, and `{SAMPLE}` in \
         `{FIXTURE}` drives cold {} and floor {}. Whichever moved, the gates are now demonstrating \
         a population the prose does not describe — which is the defect this pin exists for, in \
         the direction that used to be silent. Sentence: {}",
        said.cold,
        said.floor,
        fixture.cold,
        fixture.floor,
        said.sentence,
    );
    assert_eq!(
        said.sum,
        fixture.break_even(priced),
        "⚠⚠⚠⚠ the stated break-even is not what its own numbers come to: {} * {} + {} = {}, and \
         the paragraph says {}. ⚠ A sum written by hand ages the moment either number is \
         re-measured, and no reader recomputes one. Sentence: {}",
        priced,
        fixture.cold,
        fixture.floor,
        fixture.break_even(priced),
        said.sum,
        said.sentence,
    );
    assert!(
        fixture.context > said.sum,
        "⚠⚠⚠⚠⚠ the fixture's session has read {} against a break-even of {}, so it does NOT reach \
         the economic door — and the gates that drive that arm would be measuring the fall-back \
         while reading as if they measured the trade. This is item 493's own inversion, arriving \
         from the other side.",
        fixture.context,
        said.sum,
    );
}

/// ⚠⚠⚠⚠ **AND THE DOOR IS SAID TO BE A MINORITY ONE, IN NUMBERS** — the half no reachability gate
/// can hold.
///
/// A vocabulary gate proves each word is reachable and cannot notice that one is RARE; item 477
/// measured the same blind spot one step over, where a reachable word stood in for two facts. The
/// rate is what a reader needs in order to know whether a handover they are looking at is the
/// ordinary case or the exception — so it is required, and required to be honest in both
/// directions: a door nothing reaches is dead, and one everything reaches is not a decision.
#[test]
fn the_rate_says_how_rare_that_door_is_and_is_dated() {
    let scxml = read(TEMPLATE);
    let said = economics::stated(&scxml).unwrap_or_else(|why| panic!("{why}"));

    assert!(
        said.population > 0 && said.reached <= said.population,
        "⚠⚠⚠ the rate is not a rate: {} of {}. Sentence: {}",
        said.reached,
        said.population,
        said.sentence,
    );
    assert!(
        said.reached > 0,
        "⚠⚠⚠⚠ no session in the measured population ever reached the economic door. That is not a \
         rare arm, it is a dead one, and the document should say what the edge is FOR before a \
         loop is shipped deciding on it. Sentence: {}",
        said.sentence,
    );
    assert!(
        said.reached * 2 < said.population,
        "⚠⚠⚠⚠⚠ {} of {} sessions reach the economic door, which is no longer the MINORITY case the \
         paragraph and the fixture are written around. That is a finding, not a failure — but it \
         means the fixture's `context` is now the ordinary session rather than the exceptional one, \
         and item 493's argument has to be re-made rather than re-asserted. Sentence: {}",
        said.reached,
        said.population,
        said.sentence,
    );
    assert!(
        said.dated.starts_with("20"),
        "⚠⚠ a measurement carries the day it was taken — register item 456. Got {:?}",
        said.dated,
    );
}
