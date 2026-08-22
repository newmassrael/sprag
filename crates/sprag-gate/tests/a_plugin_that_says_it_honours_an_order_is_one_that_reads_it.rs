//! A plugin's `honours` answer is what the daemon REFUSES on — register items 539, 597 and their
//! residue.
//!
//! # What this gate is holding up
//!
//! Those items closed a real lie: `sprag hold-run` and `sprag stand-down` were handed to every run
//! and read by one plugin, so a person holding an `orchestrator` to read its pane was told the pane
//! had gone still, and it had not. The fix asks the plugin instead of keeping a table —
//! `sprag_plugin::Plugin::honours` defaults to `false`, and the host refuses an order the answer
//! declines. **The day a second plugin grows a reader, its own answer lifts its own refusal and
//! nothing anywhere needs updating.** That is the property, and it is worth having.
//!
//! ⚠⚠⚠⚠⚠ **AND IT REINTRODUCES THE ORIGINAL LIE IF AN ANSWER CAN BE WRONG.** A plugin that answers
//! `true` and contains no reader is accepted at the door, told nothing, and drives straight on —
//! which is *exactly* the pairing items 539 and 597 were filed about, now reachable by a one-word
//! edit instead of by an architectural gap. The refusal is only as honest as the answer, and
//! nothing was watching the answer.
//!
//! # Why a whitelist rather than a proof
//!
//! There is no way for a text scan to show that a plugin's `honours` agrees with its own code:
//! `AiLoop` answers in one file and READS in another (`OuterLoop::pump`), so a file-level equality
//! between answerers and readers is false today and would have to be weakened to pass — a gate
//! weakened until it passes is register item 453's shape.
//!
//! What a list buys instead is that **adding an answer becomes an edit somebody has to argue for**.
//! The reason column is where the argument goes, and its content is the claim: *this plugin's
//! reader is here*. That is the same trade the two sibling ratchets in this directory make.
//!
//! ⚠ It fires in BOTH directions, which is item 470's rule for every pin in this crate: a new file
//! answering is a red, and a listed file that has stopped answering is a red too.

use sprag_gate::sources::rust_sources;

/// The question, spelled as it appears in code.
///
/// ⚠ `fn honours(` and not `honours` — a bare name would match every CALL of it, and the callers
/// are the host's door and its tests, which decide nothing. What this gate is about is who ANSWERS.
const ANSWER: &str = "fn honours(";

/// The files allowed to answer, and why each one may.
const ANSWERERS: &[&str] = &[
    // THE TRAIT'S OWN DEFAULT, which answers `false`. It is listed rather than skipped because it
    // is the safe answer's only home: the day somebody flips that default, every plugin in the
    // workspace starts claiming to honour every order at once, and this list is what notices the
    // file was touched at all.
    "crates/sprag-plugin/src/plugin.rs",
    // ⚠⚠⚠ THE ONE PLUGIN THAT ANSWERS `true`, AND ITS READER IS REAL: `OuterLoop::pump` carries
    // both orders into the loop document at the top of every pass — `RunContext::stood_down` and
    // `RunContext::held` — and the document decides at its own next milestone. The two sibling
    // ratchets in this directory count those readers, so between the three of them the answer and
    // the reader are each pinned, even though no single scan can pair them.
    //
    // ⚠⚠ The answer lives in `ai_loop.rs` and the reader in `outer.rs`, which is why this gate
    // cannot be an equality. A second plugin adding itself here is asserting the same pairing for
    // itself, and the reviewer's job is to check it.
    "crates/sprag-plugin/src/ai_loop.rs",
    // ⚠⚠ NOT A PLUGIN AND NOT AN EXEMPTION — the DIRECTORY, which asks in order to FORWARD.
    // `RunHandle::honours` replays what the plugin answered at submit, because by then the plugin
    // has moved into its worker thread and there is nothing left to ask. It decides nothing: a
    // registry has no turn to finish and no pane to leave alone.
    //
    // ⚠ Listed rather than skipped so the staleness check below covers it: the day this file stops
    // answering, the door has stopped refusing and the lie is back with no other sign of it.
    "crates/sprag-host/src/runs.rs",
];

/// The crate this gate lives in, skipped from the population.
///
/// ⚠⚠⚠ Its siblings' reason verbatim: this FILE necessarily spells the needle it hunts, because
/// `ANSWER` is a `const` and not a comment, so without this the gate reports ITSELF as a plugin
/// that answers. A hunter that finds its own weapon is register item 453's shape arriving as a
/// false red, and the fix belongs in the POPULATION rather than in a cleverly-spelled needle that
/// would hide it from a human reader too.
const NOT_A_PLUGIN: &str = "crates/sprag-gate/";

#[test]
fn a_plugin_that_says_it_honours_an_order_is_one_that_reads_it() {
    let sources = rust_sources();

    // ⚠⚠⚠⚠⚠ THE CONTROL FIRST, because every assertion below is a comparison against a measurement
    // and a comparison against an EMPTY measurement is satisfied by the walk having failed.
    assert!(
        sources.len() > 20,
        "this walk must reach the workspace's sources and found only {}",
        sources.len(),
    );

    let answering: Vec<&str> = sources
        .iter()
        .filter(|source| !source.file.starts_with(NOT_A_PLUGIN))
        .filter(|source| source.product.iter().any(|(_, line)| line.contains(ANSWER)))
        .map(|source| source.file.as_str())
        .collect();

    assert!(
        !answering.is_empty(),
        "⚠⚠⚠⚠⚠ NOTHING IN THIS WORKSPACE ANSWERS WHETHER IT HONOURS AN ORDER. Either the needle \
         {ANSWER:?} has stopped matching the product — which makes this gate green for ever about a \
         question it is no longer asking — or the question is gone, in which case the host's door \
         is refusing on nothing and `sprag hold-run` is back to promising a still pane it cannot \
         deliver.",
    );

    let unexpected: Vec<&&str> = answering
        .iter()
        .filter(|file| !ANSWERERS.contains(file))
        .collect();

    assert!(
        unexpected.is_empty(),
        "⛔⛔⛔⛔ A NEW FILE ANSWERS WHETHER IT HONOURS A STANDING ORDER: {unexpected:?}\n\n\
         If this is a plugin that now READS the order, that is good news and the item this gate \
         guards is paid: add the file here with the reader's own location in the reason, and check \
         that `sprag hold-run` / `sprag stand-down` on that plugin's runs now do what their \
         sentences promise.\n\n\
         If it answers `true` and has no reader, STOP — that is the exact defect register items \
         539 and 597 closed, arriving through the door built to close it: the host will accept the \
         order, the run will drive straight on, and the person will be told their pane has gone \
         still.",
    );

    // ⚠⚠⚠ AND THE LIST MUST NOT ROT IN THE OTHER DIRECTION — item 470's rule. A file that stops
    // answering is a file this list should stop excusing, or the next answerer added to it
    // inherits an exemption nobody decided to give it.
    let stale: Vec<&&str> = ANSWERERS
        .iter()
        .filter(|file| !answering.contains(*file))
        .collect();

    assert!(
        stale.is_empty(),
        "⚠⚠⚠ THIS GATE IS EXCUSING FILES THAT NO LONGER ANSWER: {stale:?}. Drop them, so the \
         exemption is a decision somebody makes rather than a leftover a later reader inherits.",
    );
}
