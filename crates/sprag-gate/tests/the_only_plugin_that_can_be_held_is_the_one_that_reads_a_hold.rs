//! The `abandoned` verdict costs no wire number BECAUSE only `ai_loop` can produce it — item 534.
//!
//! # What this gate is holding up
//!
//! `hold_run` is a thing a person says to ANY run: the host raises one flag on a
//! `RunContext`, and every plugin the daemon serves is handed that same context. But a flag
//! nobody reads changes nothing, and **`sprag_plugin::RunContext::held` has exactly one reader in
//! this workspace — the outer AI loop's driver.** The other plugins do not decline to honour a
//! hold as a matter of policy; they contain no code that can see one.
//!
//! That is not a curiosity. It is the load-bearing premise of a decision recorded in
//! `sprag-host/src/wire.rs`: the ninth verdict word, `abandoned`, was added to the value-space pin
//! **without raising `sprag_rpc::WIRE_PROTOCOL`**, on R384's escape (*"only `ai_loop` produces it,
//! and no old client can select that plugin"*) rather than on R394's (*"`orchestrator` produces it
//! too, so an old client meets it"*). `peer_gone` cost the number for exactly that difference.
//!
//! # ⚠⚠⚠⚠⚠ Why the premise needs a gate and not a comment
//!
//! The day a second plugin honours a hold — a perfectly reasonable thing to build; `orchestrator`
//! runs unattended for hours and a person may well want to pause one — that plugin can end a run as
//! abandoned, through a `plugin` word **every version of this wire can send**. At that moment the
//! escape stops being true and the number falls due. Nothing else in this workspace can see that
//! happen: no address moves, no form changes, no canonical value re-renders, and the value-space pin
//! is already satisfied because the word is already in it.
//!
//! So the comment in `wire.rs` would go on saying *only `ai_loop` produces it* while it had stopped
//! being true — which is register item 437's class exactly (a sentence that outlived its subject),
//! and item 494's rule for the remedy: **a claim a document makes needs a channel and a ratchet.**
//! This is the ratchet.
//!
//! # ⚠⚠ What a text scan can and cannot claim
//!
//! `sprag-gate` takes no dependencies and std has no Rust parser, so this asks a question about
//! SPELLING: which shipping files mention the reader. It cannot prove a plugin does not honour a
//! hold by some other route. What it can prove is that the set of files naming the reader has not
//! GROWN, which is the event that would invalidate the escape — and a new plugin honouring a hold
//! has to name it, because there is no other way to ask.

use sprag_gate::sources::rust_sources;

/// The reader whose callers are the population — spelled as it appears in code.
///
/// ⚠ `held()` and not `held` — the FIELD `holding` and the local `held` bindings are everywhere in
/// this workspace, and a needle that matched them would measure nothing about the order.
const READER: &str = "held()";

/// The files allowed to ask whether a run is held, and why each one may.
///
/// ⚠⚠ A LIST AND NOT A CRATE PREFIX, deliberately. `sprag-plugin` holds every plugin in this
/// workspace — `pipe`, `orchestrator`, `dialogue`, `screen` — so a prefix would permit exactly the
/// change this gate exists to notice. The list is at FILE granularity because that is the coarsest
/// unit that still separates the outer loop from its siblings.
const READERS: &[&str] = &[
    // THE ONE PLUGIN, and the whole population. `attend`'s held arm parks the loop and, since item
    // 534, ends it; the pass-top arm reads the same order to raise `hold` and `resume`.
    //
    // ⚠⚠ `run.rs` IS NOT HERE, THOUGH IT DEFINES `RunContext::held`, AND THAT IS MEASURED RATHER
    // THAN ASSUMED: a definition reads `fn held(&self)` and a CALL reads `.held()`, so the needle
    // does not match the definition — and every call in that file is inside its own `#[cfg(test)]`
    // module, which `Source::product` drops. Listing it would have made the staleness check below
    // red on a file that genuinely does not ask.
    "crates/sprag-plugin/src/outer.rs",
];

/// The crate this gate lives in, skipped from the population.
///
/// ⚠⚠⚠ TWO REASONS, AND THE SECOND IS THE ONE THAT BITES. `sprag-gate` ships no plugin and holds no
/// `RunContext`, so nothing here could honour an order. And this FILE necessarily spells the needle
/// it hunts — `READER` is a `const` and not a comment, so `Source::product` keeps it — which made
/// the gate's first run report ITSELF as a second plugin that can be held. A hunter that finds its
/// own weapon is register item 453's shape arriving as a false red rather than a false green, and
/// the fix belongs in the POPULATION rather than in the needle: a cleverly-spelled `concat!` would
/// have hidden the needle from the reader as well as from the scan.
const NOT_A_PLUGIN: &str = "crates/sprag-gate/";

#[test]
fn the_only_plugin_that_can_be_held_is_the_one_that_reads_a_hold() {
    let sources = rust_sources();

    // ⚠⚠⚠⚠⚠ THE CONTROL FIRST, because every assertion below is an equality against a measurement
    // and an equality against an EMPTY measurement is satisfied by the walk having failed. A gate
    // that pointed at nothing would report *nobody reads a hold*, which is the most reassuring
    // possible way to say the ratchet is broken (register item 453).
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
        "⚠⚠⚠⚠⚠ NOTHING IN THIS WORKSPACE ASKS WHETHER A RUN IS HELD. Either the needle {READER:?} \
         has stopped matching the product — which makes this gate green for ever about a question \
         it is no longer asking — or `hold_run` has become a flag no plugin reads, in which case a \
         person's order does nothing at all and the `abandoned` ending is unreachable.",
    );

    let unexpected: Vec<&&str> = asking
        .iter()
        .filter(|file| !READERS.contains(file))
        .collect();

    assert!(
        unexpected.is_empty(),
        "⛔⛔⛔⛔ A SECOND PLUGIN CAN NOW BE HELD, AND THE WIRE NUMBER FALLS DUE.\n  \
         files asking {READER:?} that this gate does not expect: {unexpected:?}\n\n\
         `sprag-host/src/wire.rs` adds the verdict word `abandoned` to its value-space pin WITHOUT \
         raising `sprag_rpc::WIRE_PROTOCOL`, on the ground that only `ai_loop` can produce it — a \
         plugin no older client can select. A plugin that honours a hold can end a run as \
         abandoned, and if it is reachable through a `plugin` word older clients already send, an \
         old client receives a verdict its decoder has nowhere to put and serde fails the WHOLE \
         document. That is `peer_gone`'s case, and `peer_gone` cost the number.\n\n\
         If that is what you are building: raise `WIRE_PROTOCOL`, re-stamp `PINNED_VALUES`, and \
         rewrite the escape paragraph beside `\"verdict:abandoned\"` — it is a claim about the \
         product, and it has just stopped being true. If the new site is not a plugin honouring an \
         order, add it here with the reason.",
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
         is a decision somebody makes rather than a leftover a later reader inherits.",
    );
}
