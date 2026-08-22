//! `sprag stand-down` promises a milestone, and only ONE plugin can reach one — register item 594.
//!
//! # What this gate is holding up
//!
//! This is `the_only_plugin_that_can_be_held_is_the_one_that_reads_a_hold` one order over, and it
//! guards a different claim for a different reason. A stand-down is a thing a person says to ANY
//! run: `RunRegistry::stand_down` forwards it to whatever `RunHandle` the directory holds, and
//! every plugin the daemon serves is handed the same `RunContext`. But **`sprag_plugin::RunContext
//! ::stood_down` has exactly one reader among the plugins — the outer AI loop's driver.** The
//! others do not decline the order as a matter of policy; they contain no code that can see one.
//!
//! What rests on that, and what register item 594 was filed for, is a SENTENCE. `sprag stand-down`
//! prints `sprag_plugin::STAND_DOWN_TAKES_EFFECT` — *"it finishes the turn its agent is in the
//! middle of and stops at the next milestone, banking that work"* — for a run of ANY plugin, and a
//! milestone is `ai_loop.scxml`'s concept and nothing else's. `sprag-host`'s `stand_down_sentence`
//! therefore says only that the order is STANDING while a run is still going, and leaves what it
//! will do to the ending. **That is a decision taken because of the count this gate measures**, and
//! item 539 records the same shape costing a person a false promise on the order beside it.
//!
//! # ⚠⚠⚠⚠⚠ Why the premise needs a gate and not a comment
//!
//! The day a second plugin honours a stand-down — a reasonable thing to build; `orchestrator` runs
//! unattended for hours and *finish this and stop* is exactly what somebody would want — the
//! caution above becomes needless and the promise becomes keepable. Nothing else in this workspace
//! would notice: no address moves, no form changes, no answer word is added, and every gate over
//! the loop goes on passing because the loop is unchanged. The comments would go on describing a
//! product that had moved, which is register item 437's class and item 494's rule for the remedy —
//! **a claim a document makes needs a channel and a ratchet.** This is the ratchet.
//!
//! ⚠ It fires in BOTH directions, which is what item 470 asks of every pin in this crate: a new
//! file asking the question is a red, and a listed file that has stopped asking is a red too.
//!
//! # ⚠⚠ What a text scan can and cannot claim
//!
//! `sprag-gate` takes no dependencies and std has no Rust parser, so this asks a question about
//! SPELLING: which shipping files mention the reader. It cannot prove a plugin does not honour a
//! stand-down by some other route. What it can prove is that the set of files naming the reader has
//! not GROWN — the event that would make the caution stale — and a plugin honouring the order has
//! to name it, because there is no other way to ask.

use sprag_gate::sources::rust_sources;

/// The reader whose callers are the population — spelled as it appears in code.
///
/// ⚠ `stood_down()` and not `stood_down`: the FIELD is spelled that way on `RunSummary`, on
/// `PersistedRun` and on the wire key, and a needle matching those would count the machinery that
/// merely REPUBLISHES the order rather than the code that honours it.
const READER: &str = "stood_down()";

/// The files allowed to ask whether a run has been stood down, and why each one may.
///
/// ⚠⚠ A LIST AND NOT A CRATE PREFIX, deliberately — the hold gate's argument verbatim.
/// `sprag-plugin` holds every plugin in this workspace, so a prefix would permit exactly the change
/// this gate exists to notice.
const READERS: &[&str] = &[
    // THE ONE PLUGIN, and the whole population of things that can OBEY. `pump` carries the order
    // into the loop document at the top of a pass, and the document decides at its own milestone.
    "crates/sprag-plugin/src/outer.rs",
    // ⚠⚠⚠ NOT A PLUGIN AND NOT AN EXEMPTION — the DIRECTORY, which asks in order to REPUBLISH.
    // `RunRegistry::snapshot` reads `RunHandle::stood_down` so a mouth can tell *the order landed*
    // from *the order never landed*, which is item 594's whole payment. It honours nothing: a
    // registry has no turn to finish and no milestone to reach, and the fact travels straight out
    // to `sprag runs` and `list_runs` as a sentence.
    //
    // ⚠ It is listed rather than skipped so the staleness check below covers it too: the day this
    // file stops asking, the published key has silently stopped being answered and a person is
    // back to reading `cancelled` with nothing beside it.
    "crates/sprag-host/src/runs.rs",
];

/// The crate this gate lives in, skipped from the population.
///
/// ⚠⚠⚠ The hold gate's second reason is the one that bites here too: this FILE necessarily spells
/// the needle it hunts — `READER` is a `const` and not a comment, so `Source::product` keeps it —
/// so without this the gate reports ITSELF as a plugin that can be stood down. A hunter that finds
/// its own weapon is register item 453's shape arriving as a false red, and the fix belongs in the
/// POPULATION rather than in the needle: a cleverly-spelled `concat!` would hide the needle from a
/// human reader as well as from the scan.
const NOT_A_PLUGIN: &str = "crates/sprag-gate/";

#[test]
fn the_only_plugin_that_can_be_stood_down_is_the_one_that_reads_the_order() {
    let sources = rust_sources();

    // ⚠⚠⚠⚠⚠ THE CONTROL FIRST, because every assertion below is a comparison against a measurement
    // and a comparison against an EMPTY measurement is satisfied by the walk having failed. A gate
    // that pointed at nothing would report *nobody reads a stand-down*, which is the most
    // reassuring possible way to say the ratchet is broken (register item 453).
    assert!(
        sources.len() > 20,
        "this walk must reach the workspace's sources and found only {}",
        sources.len(),
    );

    let asking: Vec<&str> = sources
        .iter()
        .filter(|source| !source.file.starts_with(NOT_A_PLUGIN))
        .filter(|source| source.product.iter().any(|(_, line)| line.contains(READER)))
        .map(|source| source.file.as_str())
        .collect();

    assert!(
        !asking.is_empty(),
        "⚠⚠⚠⚠⚠ NOTHING IN THIS WORKSPACE ASKS WHETHER A RUN HAS BEEN STOOD DOWN. Either the needle \
         {READER:?} has stopped matching the product — which makes this gate green for ever about a \
         question it is no longer asking — or `stand_down` has become an order nothing reads, in \
         which case `sprag stand-down` accepts a person's word and no run can ever act on it.",
    );

    let unexpected: Vec<&&str> = asking
        .iter()
        .filter(|file| !READERS.contains(file))
        .collect();

    assert!(
        unexpected.is_empty(),
        "⛔⛔⛔⛔ A SECOND PLUGIN CAN NOW BE STOOD DOWN, AND TWO SENTENCES HAVE STOPPED BEING TRUE.\n  \
         files asking {READER:?} that this gate does not expect: {unexpected:?}\n\n\
         `sprag-host/src/plugins.rs`'s `stand_down_sentence` deliberately tells a person only that \
         the order is STANDING while a run is still going, because *it stops at its next milestone* \
         is `ai_loop`'s promise and was false for every other plugin. And this gate's own header \
         states that count as a fact. If a plugin now honours the order, BOTH may say more: give \
         the running arm the promise back, and rewrite the header — it is a claim about the \
         product, and it has just stopped being true.\n\n\
         If the new site is not a plugin honouring an order — a directory republishing it, say — \
         add it to `READERS` with the reason, as `sprag-host/src/runs.rs` is added.",
    );

    // ⚠⚠⚠ AND THE LIST MUST NOT ROT IN THE OTHER DIRECTION — item 470's rule for every pin in this
    // crate. A file that stops reading the order is a file this list should stop excusing, or the
    // next reader added to it inherits an exemption nobody decided to give it.
    let stale: Vec<&&str> = READERS
        .iter()
        .filter(|file| !asking.contains(*file))
        .collect();

    assert!(
        stale.is_empty(),
        "⚠⚠⚠ THIS GATE IS EXCUSING FILES THAT NO LONGER ASK: {stale:?}. Drop them, so the exemption \
         is a decision somebody makes rather than a leftover a later reader inherits. ⚠ If \
         `crates/sprag-host/src/runs.rs` is in that list, the run's published `stood_down` key has \
         stopped being answered and item 594 has come back.",
    );
}
