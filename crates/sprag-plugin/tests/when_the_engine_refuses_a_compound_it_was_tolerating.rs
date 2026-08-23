//! **WHEN SCE'S HANDLER-CONSISTENCY CHECK FIRES, DRIVEN AT THE ENGINE'S OWN ALTITUDE** — register
//! item 547.
//!
//! # ⛔⛔⛔ A document that compiled every round was refused for edges it did not touch
//!
//! MEASURED 2026-08-21 (item 534's round): adding `<transition event="hold.expired">` to `held` made
//! SCE refuse the whole of `ai_loop.scxml` — *"Compound state 'orders' has children handling event
//! 'hold' inconsistently … this compound leaves 'resume', 'stand.down' inconsistent too"*. **All
//! three of those had been true since the region was written**, and the file had compiled every
//! round. Renaming the new event to `abandon` compiled again. Nobody could say why the same three
//! were tolerated on one build and fatal on the next, and the register recorded the cost plainly:
//! a future round adding an ordinary edge can be refused by a check about edges it did not touch,
//! **and the message names the OLD ones**, so a reader debugging it looks at the wrong three.
//!
//! # ⚠⚠⚠⚠⚠ It is not order-dependent. It is gated on a PRECONDITION an unrelated edit can flip
//!
//! `scxml_exhaustiveness::collect_gaps` skips a compound entirely unless there exists **one event
//! every transition-carrying sibling matches** — the "common ground" test, whose stated purpose is
//! to let siblings dispatch disjoint event families without being called inconsistent. While no
//! such event exists, every gap under that parent is INVISIBLE. The moment one appears, all of them
//! are reportable at once.
//!
//! And an event can create common ground **without being spelled by any sibling**, because SCXML
//! matching is by PREFIX: a sibling handling `hold` matches `hold.expired`. So the flip is reachable
//! by adding a token that merely descends from one an existing sibling already handles — which is
//! exactly what `hold.expired` did.
//!
//! # ✅ And the escape hatch CAN now be armed in advance — fixed upstream 2026-08-24
//!
//! SCE's own message offers `sce:unhandled="<event>"`, and until pin `084dfdbf` it could not be used
//! BEFORE the flip: `check_declarations` judged a declaration against the same gap map, and while
//! the precondition was off that map had no entry for the parent — so the declaration was
//! `StaleUnhandledDeclaration` and the build was refused FOR THE FIX. **A document could only
//! declare a gap after that gap had already become fatal.**
//!
//! SCE `17f43428` splits the inconsistency FACTS from the reportable SUBSET and judges
//! `sce:unhandled` against the facts. The third gate below is now the CONSUMPTION of that, and it
//! carries the arm that keeps *accepted in advance* from meaning *no longer judged*.
//!
//! ⚠⚠⚠ **THE GATE IS WHAT TOLD US.** It was written as an `expect_err` whose own message said *"if
//! it ever stops being refused, upstream has fixed the thing this gate was filed about"* — and it
//! went red on the pin bump, in the round that moved the pin, which is exactly the bargain the last
//! section of this header describes.
//!
//! # Why this is a gate and not a paragraph in the register
//!
//! Because a rule written down is a rule that rots with no one to say so (register item 416). The
//! engine is a pinned dependency; if a bump changes any of the three behaviours below, these go red
//! and the rule is known stale in the round that moved the pin — rather than in the round that
//! trips over it.

use sce_build::model::{SCXMLModel, State, Transition};
use sce_build::scxml_exhaustiveness::validate;

/// A parent state, at `document_order`.
fn state(id: &str, document_order: u32) -> State {
    State {
        id: id.to_string(),
        document_order,
        ..Default::default()
    }
}

/// A child of `parent`, at `document_order`.
fn child(id: &str, parent: &str, document_order: u32) -> State {
    State {
        id: id.to_string(),
        parent: Some(parent.to_string()),
        document_order,
        ..Default::default()
    }
}

/// An event-carrying transition. The target is never entered here — the check under test is
/// structural, and reads only which events a sibling matches.
fn transition(event: &str) -> Transition {
    Transition {
        event: event.to_string(),
        target: "elsewhere".to_string(),
        ..Default::default()
    }
}

/// `orders` as this repository's loop document really shapes it: `standing` handles the three
/// command events and `held` handles none of them, plus whatever `extra` the caller is adding to
/// `held` this time.
///
/// ⚠ The asymmetry is the REAL one and not a convenience: `standing` has answered `hold`,
/// `stand.down` and `resume` since the region was written, and `held` has answered none of them.
/// A fixture that made the two symmetric would be a fixture with nothing to find.
fn orders_with(extra: &[&str]) -> SCXMLModel {
    let mut model = SCXMLModel {
        initial: "orders".to_string(),
        ..Default::default()
    };
    let parent = state("orders", 0);
    let mut standing = child("standing", "orders", 1);
    for event in ["hold", "stand.down", "resume"] {
        standing.transitions.push(transition(event));
    }
    let mut held = child("held", "orders", 2);
    // Without at least one transition a child is not even considered — `collect_gaps` drops
    // transition-less children before it counts anything.
    held.transitions.push(transition("let.go"));
    for event in extra {
        held.transitions.push(transition(event));
    }
    for s in [parent, standing, held] {
        model.states.insert(s.id.clone(), s);
    }
    model
}

/// ⛔⛔⛔ **THE TRIGGER: ONE TOKEN, AND A COMPOUND GOES FROM SILENT TO FATAL** — item 547's whole
/// question, answered by driving the engine rather than by reading it.
///
/// ⚠⚠ **THE CONTROL IS THE SAME COMPOUND WITH THE SAME THREE GAPS**, differing only in the name of
/// the event being added. Without it this would be a test that SCE rejects something, which says
/// nothing about WHEN — and *when* is the entire item: three inconsistencies stood for months and
/// then killed a build that did not touch them.
#[test]
fn a_compound_that_compiled_for_months_is_refused_by_an_edge_that_touches_none_of_its_gaps() {
    let quiet = orders_with(&["abandon"]);
    assert!(
        validate(&quiet, "ai_loop.scxml").is_ok(),
        "⚠⚠⚠⚠⚠ THE CONTROL: `standing` handles three events `held` does not, and this build is \
         FINE with that — which is the state this repository's loop document has been in since the \
         region was written. If this ever refuses, the rule item 547 established has changed and \
         the register's account of it is stale",
    );

    // The one difference: an event that DESCENDS from one `standing` already handles.
    let flipped = orders_with(&["hold.expired"]);
    let refused = validate(&flipped, "ai_loop.scxml")
        .expect_err(
            "⛔⛔⛔ ITEM 547: adding `hold.expired` to `held` must be what turns the same three \
             pre-existing gaps fatal. If it compiles, the precondition this item is about is gone \
             and the rule needs re-measuring",
        )
        .to_string();
    for named in ["orders", "hold"] {
        assert!(
            refused.contains(named),
            "⚠⚠⚠ AND THE REFUSAL NAMES THE OLD GAP RATHER THAN THE NEW EDGE, which is the half \
             that costs a reader their afternoon: {named:?} is not in {refused:?}",
        );
    }
    assert!(
        !refused.contains("hold.expired"),
        "⚠⚠⚠⚠⚠ THE EDGE THE AUTHOR ACTUALLY ADDED IS NOWHERE IN THE MESSAGE. That is the sharp \
         end of item 547 and it is asserted rather than assumed: a round is told about `hold`, \
         `resume` and `stand.down`, and the line it wrote was `hold.expired`. Got: {refused:?}",
    );
}

/// **COMMON GROUND CAN BE CREATED BY A TOKEN NO SIBLING SPELLS**, because matching is by prefix.
///
/// ⚠⚠ Its own gate rather than a clause above, because it is the part that makes the hazard
/// unforeseeable by reading: an author scanning `orders` for the literal `hold.expired` finds it
/// nowhere and concludes the event is new to this compound. It is new — and it still matches
/// `standing`'s `hold`, which is what closes the ring.
///
/// The control is a sibling of the SAME shape whose token descends from nothing: `abandon.now` has
/// the same two segments and creates no common ground at all.
#[test]
fn an_event_no_sibling_spells_can_still_create_the_common_ground() {
    assert!(
        validate(&orders_with(&["abandon.now"]), "ai_loop.scxml").is_ok(),
        "⚠⚠⚠⚠⚠ THE CONTROL: a two-segment event whose first segment nothing handles leaves the \
         compound as quiet as it was. Shape is not what does it",
    );
    assert!(
        validate(&orders_with(&["resume.later"]), "ai_loop.scxml").is_err(),
        "⛔⛔⛔ ITEM 547: `resume.later` appears nowhere in this compound, and `standing`'s bare \
         `resume` matches it by prefix — so adding it to `held` gives every sibling one event in \
         common and turns the whole parent reportable. This is the rule a round must apply BEFORE \
         editing: an event is dangerous when a sibling already handles any prefix of it",
    );
}

/// ✅✅✅✅ **AND THE FIX SCE OFFERS CAN NOW BE APPLIED BEFORE IT IS NEEDED — item 547's residue,
/// DELIVERED UPSTREAM AND CONSUMED HERE.**
///
/// # What this gate said until 2026-08-24, and what moved
///
/// The refusal's own text says to *"declare the gap on the non-handling child with
/// `sce:unhandled="<event>"` if it is intentional"*. A document sitting on latent gaps wants exactly
/// that — armed in advance, so no later edge can surprise it. **It could not be**: declarations were
/// judged against the same gap map the precondition gates, so while the compound was quiet the
/// declaration described a gap the engine did not believe existed, and the build was refused FOR THE
/// FIX. This gate held that as a `expect_err` and said in its own message *"if it ever stops being
/// refused, upstream has fixed the thing this gate was filed about"*.
///
/// It stopped. SCE `17f43428` — *"Let a gap be declared before the report asks for it: split the
/// inconsistency facts from the reportable subset; judge `sce:unhandled` against the facts, the lint
/// against the subset"* — reached this tree at pin `084dfdbf`, taken from `pinion@38c908b2` as the
/// shared-instance rule requires. **The gate went red on the pin bump, in the round that moved it,
/// which is the whole reason it was written as a gate rather than a paragraph.**
///
/// # ⚠⚠⚠⚠⚠ SO IT IS INVERTED, AND THE THIRD ARM IS WHAT KEEPS IT A GATE
///
/// *Accepted in advance* is also what a build that had stopped judging declarations at all would
/// answer, and that would be strictly worse than the residue: a document could then name any event
/// it liked and be told nothing. So the arm that matters is the **FALSE declaration** — an event no
/// sibling handles inconsistently, declared as a gap — which must still be refused. Upstream's own
/// split is exactly this: the facts are what a declaration is judged against, and a non-fact is
/// still stale.
///
/// ⚠ The second arm — the same declaration AFTER the flip — is kept unchanged. It was the control
/// that made the old statement about TIMING rather than about the attribute, and it is now the
/// control that says the hatch did not merely move.
#[test]
fn a_gap_can_be_declared_before_the_edge_that_makes_it_fatal_arrives() {
    let declare = |extra: &[&str]| {
        let mut model = orders_with(extra);
        let held = model
            .states
            .get_mut("held")
            .expect("the non-handling child");
        held.unhandled = ["hold", "stand.down", "resume"]
            .iter()
            .map(|e| (*e).to_string())
            .collect();
        // ⚠⚠ THE REPAIR IS THE WHOLE COMPOUND'S, which is what SCE's own message asks for — *"an
        // author repairing this compound wants the whole picture in one pass"*. `held`'s three are
        // the gaps item 547 was measured on; this is the MIRROR gap the fixture's own shape makes
        // (`held` must carry some transition, and any transition `standing` lacks is a gap of its
        // own). Declaring it too is what keeps the arm below a statement about TIMING rather than
        // about a fourth event. The gate's first run is what found this.
        let standing = model
            .states
            .get_mut("standing")
            .expect("the handling child");
        standing.unhandled = vec!["let.go".to_string()];
        model
    };

    let early = validate(&declare(&[]), "ai_loop.scxml");
    assert!(
        early.is_ok(),
        "⛔⛔⛔ ITEM 547's RESIDUE IS BACK. This is a document declaring, TRUTHFULLY, the gaps it \
         has — before any edge has made them fatal — which is precisely what SCE's own refusal \
         text tells an author to do. It was refused until `17f43428`, so a document could only be \
         made safe AFTER it had already broken, and every author met the check for the first time \
         in the round that tripped it. If this is red, the engine pin moved backwards or the split \
         between the inconsistency FACTS and the reportable SUBSET was undone: {}",
        early
            .as_ref()
            .map_or_else(|why| why.to_string(), |()| String::new()),
    );

    let after = validate(&declare(&["hold.expired"]), "ai_loop.scxml");
    assert!(
        after.is_ok(),
        "⚠⚠⚠⚠⚠ THE CONTROL THAT SAYS THE HATCH DID NOT MERELY MOVE: the SAME declarations must go \
         on being accepted once the edge that flips the precondition is present. ⚠ The engine's \
         own words are carried here rather than left in a log, because the first thing a reader \
         needs is WHICH gap is complained about — this gate's first run failed on a fourth one \
         nobody had counted: {}",
        after.map_or_else(|why| why.to_string(), |()| String::new()),
    );

    // ── ⛔ THE ARM THAT KEEPS THIS A GATE: a declaration that is NOT TRUE is still refused ──
    //
    // ⚠⚠⚠⚠⚠ Without it, *accepted in advance* is exactly what a build that had stopped judging
    // declarations AT ALL would answer — and that is strictly worse than the residue this replaces,
    // because a document could then name any event it liked and be told nothing. Upstream's own
    // split is this arm: `sce:unhandled` is judged against the inconsistency FACTS, and an event no
    // sibling handles inconsistently is not one of them.
    let mut invented = declare(&[]);
    invented
        .states
        .get_mut("held")
        .expect("the non-handling child")
        .unhandled
        .push("nobody.handles.this".to_string());
    let stale = validate(&invented, "ai_loop.scxml")
        .expect_err(
            "⛔⛔⛔⛔⛔ A DECLARATION NAMING AN EVENT THAT IS NOT A GAP WAS ACCEPTED. Then \
             `sce:unhandled` has stopped being judged rather than been made reachable, and every \
             declaration in every document here is now unchecked text — the failure mode that \
             makes this whole hatch worthless. It must still be stale",
        )
        .to_string();
    assert!(
        stale.contains("nobody.handles.this"),
        "⚠⚠ and the refusal must NAME the invented event, or it is refusing something else and \
         this arm is passing by accident: {stale:?}",
    );
}
