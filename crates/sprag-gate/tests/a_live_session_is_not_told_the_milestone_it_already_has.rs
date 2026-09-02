//! ⛔⛔⛔⛔⛔ **ONLY THE PROMPTS THAT GREET OR REFLECT MAY NAME THE MILESTONE** — register item 800.
//!
//! # What the owner asked, and why it was a defect rather than a design
//!
//! *"why is it normal to keep typing the same prompt that means nothing?"* — 2026-09-01, and not
//! the first time: the ledger's own top section already carried an earlier shape of it, *"why is it
//! LOOKING at all? isn't the looking itself the defect?"*. That one was paid for the LOOKING and
//! never for the SAYING.
//!
//! The saying lived in one expression. `turn_prompt` — the text a working turn carries, sent on
//! every turn after the first — opened with `'Continue toward: ' + milestone`, and the milestone is
//! the largest thing the loop is given: measured on this repository's own loop, **1,224 bytes on
//! every turn of runs whose turn counts reach 254**.
//!
//! # ⚠⚠⚠⚠⚠ Why this is a gate and not a sentence in the document
//!
//! The document had ALREADY made this exact argument three times, about three other parts of the
//! same two prompts — `carried` and `working_rules` are in `start_prompt` only because they *"would
//! be re-sent on every turn of a session that has already read it"*, and `standing` is retyped with
//! a stated reason for retyping it. The milestone was the one part nobody wrote a sentence about,
//! and a default nobody re-reads is what this workspace's rule 10 is about. A fix that left the
//! rule in prose would be the same default one round later.
//!
//! ⚠⚠ THE POPULATION IS READ, NOT LISTED. Every `<assign>` whose `location` names a prompt is
//! found, and a prompt this gate has no ruling for is RED rather than a pass — because the next
//! prompt somebody composes is exactly the one that would quietly name the milestone again.

use sprag_gate::loop_shape::{Composed, DOCUMENT, composed_prompts, transitions};
use sprag_gate::sources::workspace_root;

/// The prompts that MAY name the milestone, and the reason each is allowed to.
///
/// # ⚠⚠⚠ Both entries are allowed for a reason the document states about ITSELF
///
/// * `start_prompt` GREETS. Its own comment: it *"greets every session of a run, the first and
///   every replacement `restarting` opens"* — a session that has just been opened holds nothing, so
///   everything it is told is new to it.
/// * `reflect_prompt` ASKS. Its own comment: *"a reflection that asked about a milestone the run
///   had already moved past would collect an answer about the wrong work"* — the milestone is the
///   subject of the question, not a repetition of context.
///
/// ⚠ A THIRD ENTRY IS A DECISION SOMEBODY HAS TO MAKE, which is why this is a list of two rather
/// than a rule about names. Adding one here without the reason beside it is how the default comes
/// back.
/// * `changed_prompt` HANDS OVER, and naming the milestone is the whole of what it is for: it is
///   sent to a live session precisely because that one thing moved. Register item 800's second
///   half — see [`EVERY_DOOR_SAYS_WHICH`], which is what keeps it from being sent to a session that
///   was not there for the change.
/// * `reask_prompt` ASKS TOO, and it is here for `reflect_prompt`'s reason exactly (register item
///   840): it goes to the session that just FINISHED that checkpoint and is being asked what comes
///   after it, so the milestone is the SUBJECT of the question rather than something retyped at
///   somebody who already has it. It also composes `reflect_prompt` IN, so a rule that forbade it
///   would forbid its own contents.
const MAY_NAME_IT: [&str; 4] = [
    "start_prompt",
    "reflect_prompt",
    "changed_prompt",
    "reask_prompt",
];

/// The prompts that MUST NOT, and what each is for instead.
///
/// ⚠⚠ `turn_prompt` is the one register item 800 is about; the other four never named the milestone
/// and are pinned here so that a round which "tidies" one of them by adding context has to argue
/// with this list. Two of them — `dispute_prompt` and `unverified_prompt` — compose `turn_prompt`
/// IN, so they carried it indirectly until this item; that is why they are named rather than
/// assumed.
const MUST_NOT: [&str; 5] = [
    "turn_prompt",
    "end_prompt",
    "stop_prompt",
    "dispute_prompt",
    "unverified_prompt",
];

/// How many prompts the document composes.
///
/// # ⚠⚠ AN EQUALITY, NOT A CEILING — register item 794's rule
///
/// A ceiling lets the population grow silently, and a new prompt is precisely where the milestone
/// would reappear. An equality also makes the arms below a POSITIVE CONTROL: rename the convention
/// and the walk finds nothing, and "no forbidden prompt names the milestone" is trivially true of
/// no prompts at all — the vacuous green register item 799 measured.
///
/// **8, measured 2026-09-01**: `end_prompt`, `start_prompt`, `turn_prompt`, `changed_prompt`,
/// `reflect_prompt`, `dispute_prompt`, `unverified_prompt`, `stop_prompt`.
///
/// **9 since 2026-09-03**, register item 840: `reask_prompt`, composed by `reflecting`'s entry on
/// the road that comes back into that state — a run whose checkpoint is finished asking its agent
/// again after the successor it named was turned away.
const COMPOSED_PROMPTS: usize = 9;

/// The datamodel id this gate is about.
///
/// ⚠ Matched as a WORD. `milestone_age` and `milestone_at` are different data and a prompt may
/// carry them: the age is a fact about how long this milestone has stood, which a session cannot
/// know, and `'Milestone: '` is a label rather than the value.
const MILESTONE: &str = "milestone";

/// Whether `expr` reads the milestone's VALUE, as opposed to saying the word in a sentence.
///
/// # ⛔⛔⛔⛔⛔ Two separations, and a reader that made only one of them is confidently wrong
///
/// * **The literals go first.** This document's prompts are prose glued to data, and the prose says
///   the word: `done_instruction` is *"When the milestone is fully reached AND verified"* and goes
///   into every prompt there is, on every turn, and always has. A reader that searched the whole
///   expression would report every prompt as retyping the milestone and this gate would have to be
///   deleted on the day it was written.
/// * **Then the identifier boundary.** `milestone_age`, `milestone_at` and `milestone_check` are
///   different data — the age in particular is a fact a session CANNOT know about itself — and a
///   substring match would forbid the greeting from carrying them.
///
/// ⚠ An escaped quote inside a literal is handled although this document has none today (measured
/// 2026-09-01: zero). A parser that got that wrong would read the rest of the expression as prose
/// and answer *nothing here reads the milestone* about an expression that does.
fn reads_the_milestone(expr: &str) -> bool {
    let mut code = String::with_capacity(expr.len());
    let mut chars = expr.chars();
    let mut quoted = false;
    while let Some(char) = chars.next() {
        match char {
            '\\' if quoted => {
                chars.next();
            }
            '\'' => quoted = !quoted,
            _ if quoted => {}
            _ => code.push(char),
        }
    }

    let mut rest = code.as_str();
    while let Some(at) = rest.find(MILESTONE) {
        let before = rest[..at].chars().next_back();
        let after = rest[at + MILESTONE.len()..].chars().next();
        let bounded =
            |char: Option<char>| !char.is_some_and(|char| char.is_alphanumeric() || char == '_');
        if bounded(before) && bounded(after) {
            return true;
        }
        rest = &rest[at + MILESTONE.len()..];
    }
    false
}

/// The state whose prompt depends on WHO IS READING, and the datamodel word its edges must write.
///
/// # ⛔⛔⛔⛔⛔ Why the rule is on the EDGE and not on the state
///
/// `priming` sends the greeting or the handover, and SCXML gives a state no way to ask how it was
/// entered. Three kinds of arrival reach it and only two are a new session, so the edge is the only
/// party that knows. An edge that writes nothing gets the greeting — the safe direction, and the
/// wrong answer for a live session, which is register item 800 exactly.
const GREETING_STATE: &str = "priming";
/// The word an edge into [`GREETING_STATE`] must write. ⚠ Matched as the assign's LOCATION, so a
/// comment that merely mentions it is not an edge that sets it.
const EVERY_DOOR_SAYS_WHICH: &str = "location=\"entered_by\"";

/// The document's text, read from the tree this run is standing in.
fn document_text() -> String {
    let path = workspace_root().join(DOCUMENT);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|why| panic!("{DOCUMENT} is this workspace's loop: {why}"))
}

/// The document, read from the tree this run is standing in.
fn document() -> Vec<Composed> {
    composed_prompts(&document_text())
}

#[test]
fn no_prompt_a_live_session_receives_retypes_the_milestone_it_was_already_given() {
    let composed = document();

    // ⚠⚠ THE POSITIVE CONTROL COMES FIRST. If the greeting stopped naming the milestone, the needle
    // is wrong or the document was rewritten, and every verdict below holds of nothing.
    let greeting = composed
        .iter()
        .find(|one| one.prompt == "start_prompt")
        .unwrap_or_else(|| panic!("{DOCUMENT} composes no `start_prompt`: {composed:?}"));
    assert!(
        reads_the_milestone(&greeting.expr),
        "⚠⚠ THE SCAN IS BLIND: `start_prompt` is the prompt that GREETS a session with nothing in \
         it, and this reader could not find the milestone in it. Either the needle is no longer \
         the document's spelling, or the walk is reading a tree that is not this one (register \
         item 809) — and every verdict below is worthless either way. Read: {}",
        greeting.expr,
    );

    let mut unruled = Vec::new();
    let mut retyping = Vec::new();
    for one in &composed {
        if MAY_NAME_IT.contains(&one.prompt.as_str()) {
            continue;
        }
        if !MUST_NOT.contains(&one.prompt.as_str()) {
            unruled.push(one.prompt.clone());
            continue;
        }
        if reads_the_milestone(&one.expr) {
            retyping.push(one.prompt.clone());
        }
    }

    assert_eq!(
        composed.len(),
        COMPOSED_PROMPTS,
        "⛔ ITEM 800: this document composes {} prompts and {COMPOSED_PROMPTS} are registered. \
         GROWN: a prompt was added — decide in the same commit whether a session receiving it is \
         one that already holds the milestone, put it in the right list here, and raise this \
         number. SHRUNK: a prompt went away, or the naming convention changed and this gate has \
         been measuring nothing since. Found: {:?}",
        composed.len(),
        composed.iter().map(|one| &one.prompt).collect::<Vec<_>>(),
    );

    // ⛔⛔ AN UNRULED PROMPT IS RED AND NOT A PASS, which is this workspace's rule 6. A new prompt
    // that nobody classified is exactly the one that would name the milestone again by default.
    assert!(
        unruled.is_empty(),
        "⛔⛔ ITEM 800: {unruled:?} are composed by {DOCUMENT} and this gate has no ruling for \
         them. Whether a prompt may name the milestone is a decision about WHO READS IT: a session \
         that was just opened holds nothing, a session that has been working holds the greeting it \
         got. Put each in `MAY_NAME_IT` with the reason, or in `MUST_NOT`.",
    );

    assert!(
        retyping.is_empty(),
        "⛔⛔⛔ ITEM 800: {retyping:?} name the milestone, and every session that reads them was \
         already told it by `start_prompt`. That is 1,224 bytes retyped on turns that reach 254 in \
         this repository's own runs, and the owner asked why it is normal. The document makes this \
         same argument for `carried`, for `working_rules` and (in the other direction, with its \
         reason) for `standing` — a prompt that needs the milestone owes the sentence saying why.",
    );
}

/// ⚠⚠⚠ The label is not the value, and a rule that could not tell them apart would force the
/// document to stop saying `Milestone:` out loud — which is the sentence a person reads first.
#[test]
fn the_needle_reads_an_identifier_and_not_the_word_in_a_sentence() {
    assert!(
        reads_the_milestone("'Milestone: ' + milestone + milestone_age"),
        "the value, read as an identifier, is what this gate is about",
    );
    assert!(
        !reads_the_milestone("'When the milestone is fully reached AND verified'"),
        "the WORD inside a rule the agent must follow is not the milestone being retyped: \
         `done_instruction` says it on every prompt and always has",
    );
    assert!(
        !reads_the_milestone("milestone_age + milestone_at + milestone_check"),
        "data whose names START with it are different data, and a prompt may carry them",
    );
    assert!(
        !reads_the_milestone("'Continue toward the milestone this session was given.'"),
        "TELLING an agent to go on toward what it holds is not handing it the thing again — and if \
         this ever fails, the repair item 800 made has been undone by a rewording",
    );
    assert!(
        reads_the_milestone("'a session\\'s work: ' + milestone"),
        "an escaped quote must not end the literal early: a reader that thought the prose had \
         stopped would go on reading prose as code, and one that thought it had not would read \
         code as prose and answer that nothing here names the milestone",
    );
    assert!(
        !reads_the_milestone("'it said: \\'milestone\\' plainly'"),
        "and the same escape the other way round — the word is still inside the prose",
    );
}

/// ⛔⛔⛔⛔⛔ **EVERY DOOR INTO THE GREETING STATE SAYS WHICH DOOR IT IS** — register item 800's
/// second half, and the escape hatch the rule above cannot see.
///
/// # ⚠⚠⚠⚠ Why this is a separate arm and not another clause
///
/// The rule above asks WHICH PROMPTS may name the milestone. `changed_prompt` may — that is what it
/// is for. So the whole weight of item 800's second half rests on it reaching only the reader it
/// was written for, and that is decided one level down: by whether the edge that entered `priming`
/// wrote which kind of arrival it was. An edge that writes nothing gets the greeting, silently, and
/// this workspace's rule is that an unclassified case is RED rather than a pass.
///
/// ⚠ A SELF-CLOSING EDGE IS THE SHAPE THIS CATCHES. Both doors that were wrong before this item
/// were `<transition event="…" target="priming"/>` with no body at all — the form somebody writes
/// without thinking about it, which is the point.
#[test]
fn every_transition_into_the_greeting_state_says_which_kind_of_arrival_it_is() {
    let edges = transitions(&document_text());
    let doors: Vec<&sprag_gate::loop_shape::Transition> = edges
        .iter()
        .filter(|edge| edge.target == GREETING_STATE)
        .collect();

    // ⚠⚠ THE POSITIVE CONTROL: a walk that found no doors would hold this claim of nothing, which
    // is the green a renamed state or a broken reader would produce.
    assert!(
        doors.len() >= 3,
        "⚠⚠ THE SCAN IS BLIND: `{GREETING_STATE}` is reached by a `start`, by a review that kept \
         the session and by the `session.ready` of a replacement, so at least three edges must be \
         found. Found {}: {doors:?}",
        doors.len(),
    );

    let silent: Vec<&sprag_gate::loop_shape::Transition> = doors
        .iter()
        .copied()
        .filter(|door| !door.body.contains(EVERY_DOOR_SAYS_WHICH))
        .collect();
    assert!(
        silent.is_empty(),
        "⛔⛔⛔ ITEM 800: {} edge(s) into `{GREETING_STATE}` do not say which kind of arrival they \
         are, so the session they bring gets the GREETING by default — the north star, the working \
         rules and the reference, retyped at an agent that may already be holding all of it. Write \
         `{EVERY_DOOR_SAYS_WHICH}` on each: `'start'` and `'restart'` open a session that holds \
         nothing, `'review'` is a live one that has only just moved. Silent: {silent:?}",
        silent.len(),
    );
}
