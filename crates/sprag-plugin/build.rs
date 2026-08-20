//! Build-time codegen: each control statechart -> `OUT_DIR/<stem>_sm.rs` via
//! SCE, mirroring pinion's `compile_scxml` usage.
//!
//! Three machines today (the dogfood generalizing to N): `orchestration.scxml`
//! (the Driver's run lifecycle), `session.scxml` (an endpoint's server-session
//! lifecycle) and `ai_loop.scxml` (the OUTER loop that drives an inner agent
//! session). `compile_scxml` emits one `<stem>_sm.rs` per input; `lib.rs`
//! `include!`s each into its own `sm::<stem>` submodule.
//!
//! ⚠⚠ A STATECHART LEFT OUT OF THE LIST BELOW IS A DOCUMENT, NOT A MACHINE.
//! `ai_loop.scxml` sat in the tree for eleven rounds enforced by nothing while
//! eight Rust doc comments cited it as an authority, because it was the one file
//! this array did not name. The list is the only thing that decides, so a new
//! `.scxml` under `src/` is inert until it is added here — there is deliberately
//! no glob, so that adding a machine is a decision somebody makes rather than a
//! side effect of creating a file.
//!
//! ⚠⚠ A NON-`null` DATAMODEL NEEDS `log` IN THE CONSUMER, AND THAT IS PR-86.
//! SCE's Rust codegen emits bare `log::error!` on every datamodel error path
//! (data init, assign, guard evaluation) — 42 sites for `ai_loop.scxml` — so a
//! crate compiling a script-datamodel machine without a `log` dependency gets 42
//! unresolved-crate errors out of generated code it did not write. Neither the
//! runtime's own `sce_log_error!` facade nor any README states that contract.
//! Filed as `claudedocs/PINION-PR86-*.md`; the dependency in `Cargo.toml` is an
//! interim that comes out on delivery.
//!
//! ⚠⚠ AND A COMPILED MACHINE JOINS THE SCE PIN'S BLAST RADIUS. Until this round
//! an SCE bump could only break two `null` machines, which use none of the
//! datamodel, scripting or guard codegen. `ai_loop.scxml` uses all three, so the
//! next `615cb3f6` bump has a surface it did not have before — and the gates in
//! `ai_loop.rs` are what will say so, since they drive a real engine rather than
//! asserting over the generated text.

/// The control statecharts, by file stem (the generated file is `<stem>_sm.rs`, and what `lib.rs`
/// includes is the engine's own `<stem>_sm.include.rs` index beside it — SCE-PR88).
const STATECHARTS: &[&str] = &[
    "orchestration",
    "session",
    "ai_loop",
    // ⚠⚠ THE DECISIONS A DEBT RUN RUNS UNDER — sprag's own, held apart from the template so that a
    // repository copying `ai_loop.scxml` does not inherit this repository's standing yesses. A
    // SIBLING rather than a parent, because a driver cannot reach an `<invoke>`d child (measured;
    // see `probe.rs`). See `debt_loop.scxml`.
    "debt_loop",
    // ⚠⚠ A PROBE, and it is in the shipped list on purpose: the question it asks is *does
    // `<invoke>` compile and run in THIS crate*, and a document compiled by some other harness
    // would answer about that harness. See `probe_parent.scxml`.
    "probe_child",
    "probe_parent",
    // ⚠⚠ THE SECOND PROBE, for the same reason and about `<parallel>`: nothing shipped here has
    // ever used one, so *"several loops at once"* rests on a construct this crate has never
    // executed. ⚠⚠⚠ It aims at the failure fixtures miss — a self-transition swallowing the
    // parallel root — which SCE's own suite records shipping once, invisible to every W3C fixture
    // because they are one region deep. See `probe_parallel.scxml`.
    "probe_parallel",
    // ⚠⚠ THE THIRD PROBE, and the smallest: does a `<data>` DECLARED WITH NO VALUE reach the
    // datamodel, and is a `cond` that short circuits on it safe? The owner has asked for a debt loop
    // with no turn budget, so *"no bound"* needs a spelling, and the alternative to this one is a
    // boolean beside the number — one decision in two places. See `probe_absent.scxml`.
    "probe_absent",
    // ⚠⚠ WHAT A RUN LEARNED ABOUT ITS OWN SESSIONS — a child machine, and inert until it is here.
    "context_review",
    // ⚠⚠ THE FOURTH PROBE, and register item 470's second stage rests entirely on it: can a HOST
    // register its own `<send>` / `<invoke>` TYPE, so a document can name an act this crate carries
    // out? Compiled here rather than reasoned about, because this list is also the question.
    //
    // ⚠⚠⚠⚠⚠ THE ANSWER CHANGED WITH THE ENGINE — it was NO at rev `a80b06d0` (item 483, measured
    // by compiling and running this file) and is YES at `e0fdd46b`, where SCE grew a
    // host-registrable Event I/O Processor and invoker. The type is DECLARED below and the handlers
    // are registered at run time; both halves are required and the probe holds each on its own.
    // See `probe_send_type.scxml`.
    "probe_send_type",
];

/// The Event I/O Processor / invoker types THIS CRATE serves, declared to codegen so a
/// `<send type="…">` naming one emits a dispatch instead of a refusal — W3C SCXML 6.2.5, and
/// `Engine::register_event_processor` is the other half.
///
/// ⚠⚠⚠⚠ **A DECLARATION IS NOT A HANDLER.** A type named here and registered by nobody still
/// raises `error.execution` at the send, which is the engine's own decision and the right one: an
/// act nobody performed is one fact, and reporting it as success would be the failure that the
/// whole registry exists to prevent. `probe::tests` drives BOTH sides of that.
///
/// ⚠ Two lists rather than one, because they are two contracts: serving an EVENT is not being able
/// to run an invoked process with a lifecycle (`done.invoke.<id>`, cancellation on state exit).
const HOST_TYPES: [&str; 1] = ["x-sprag-host"];

fn main() {
    let sources: Vec<String> = STATECHARTS
        .iter()
        .map(|stem| format!("src/{stem}.scxml"))
        .collect();
    let declared: Vec<String> = HOST_TYPES.iter().map(|kind| (*kind).to_owned()).collect();
    sce_build::compile_scxml_with_host_processors(
        &sources.iter().map(String::as_str).collect::<Vec<_>>(),
        &declared,
        &declared,
    );

    // ⚠⚠⚠⚠⚠ NOTHING IS POST-PROCESSED HERE ANY MORE — SCE-PR88, consumed 2026-08-20.
    //
    // This loop used to strip every `#![…]` and `//!` from each generated machine, because
    // `include!` refuses both in expansion position. That worked and it threw away what SCE had
    // measured per fixture: an audited suppression budget, replaced by a blanket
    // `#![allow(warnings, clippy::all, …)]` in the consuming module. `pinion-core/build.rs` carried
    // the byte-identical predicate, arrived at independently — two consumers, one folk contract.
    //
    // The engine now writes the other half itself: `{stem}_sm.include.rs`, a two-line index
    // (`#[path = "…"] mod {stem}_sm; pub use {stem}_sm::*;`) naming the machine ABSOLUTELY. A
    // consumer cannot write that line — a built-in attribute takes a string literal, so
    // `#[path = concat!(env!("OUT_DIR"), …)]` does not expand — which is why `build.rs` is where it
    // had to come from. `lib.rs` includes the index; the machine keeps its budget.
    //
    // ⚠ `sce-build`'s own doc says it in as many words: **"Do not strip lines from
    // `{name}_sm.rs`."** Stripping is now a defect rather than a workaround.
}
