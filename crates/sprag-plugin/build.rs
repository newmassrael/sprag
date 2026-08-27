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
    // ⚠⚠ THE FIFTH PROBE: what happens to an `error.*` the document does not ANSWER. W3C SCXML
    // 3.12.2 ignores it, so a host that did not write the document cannot see its executable
    // content failing — and `ai_loop.scxml` + `debt_loop.scxml` carry ZERO error transitions
    // between them (measured 2026-08-20). SCE grew `unhandled_error_events` for exactly this;
    // consuming it needs a document that raises one and leaves it unanswered, which is this file,
    // and `probe_send_type` is its control because that one ANSWERS. See `probe_unanswered.scxml`.
    "probe_unanswered",
    // ⚠⚠ THE SIXTH PROBE, and the corner `probe_unanswered` cannot reach: a macrostep that CANNOT
    // END. W3C SCXML 3.13 allows a document whose eventless chain is infinite; SCE stops it at a
    // microstep ceiling and reports through `truncated_macrosteps`, because every other reading a
    // host takes says the run is fine. Register item 551 — `document::faults` read two of the
    // engine's loss signals and this is the third, the same *"core at full tilt with a
    // configuration that never moves"* picture with NO ERRORS IN IT, so neither counter it already
    // read can see it. `datamodel="null"`, so the document has nothing that could raise one.
    // See `probe_truncated.scxml`.
    "probe_truncated",
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
    stamp_fingerprint(&sources);
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

/// ⚠⚠⚠⚠⚠ **WHICH DOCUMENTS THIS BINARY WAS COMPILED FROM**, as one word, emitted as
/// `SPRAG_STATECHARTS_FINGERPRINT` for `crate::STATECHARTS_FINGERPRINT` to bake in.
///
/// # Why a run's recorded position is a TRAP without this — register items 543 and 544
///
/// A run that persists *"it was in `reflecting`"* has written down a name whose meaning lives in a
/// document. Restart into a build whose `ai_loop.scxml` changed and that name may mean a different
/// state, or none — and **the restart that motivates persisting a run at all is a document
/// change**, so the dangerous case is the common one rather than the rare one. Item 544 states the
/// remedy as a structural property: a changed document makes a NEW run, deliberately. That is only
/// enforceable if the record says which documents its words came from.
///
/// ⚠⚠⚠⚠ **IT IS NOT `wire::BUILD`, AND THE DIFFERENCE IS THE WHOLE VALUE.** A build stamp changes
/// when ANY file in the tree does, so at that granularity every promotion discards every run —
/// which is exactly the cost item 543 was filed to remove (*"a restart that resumes runs is a
/// promotion nobody has to schedule around"*). This changes only when a `.scxml` does.
///
/// ⚠⚠ FNV-1a, WRITTEN OUT, and deliberately not `DefaultHasher`: that one's output is explicitly
/// not stable across Rust releases, so the same document built by two toolchains would fingerprint
/// differently and each upgrade would silently discard every run — the failure this exists to
/// prevent, reintroduced by the hash. The bytes in, the number out, decided here and nowhere else.
///
/// ⚠ Content, not paths or timestamps: a document that MOVED is the same document, and one whose
/// mtime changed under an unchanged body has not changed.
fn stamp_fingerprint(sources: &[String]) {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = OFFSET;
    for path in sources {
        // ⚠ THE STEM IS HASHED TOO, so two documents swapping bodies is a change. Concatenation
        // alone would report the pair unmoved.
        let body = std::fs::read(path)
            .unwrap_or_else(|why| panic!("statechart source {path} must be readable: {why}"));
        for byte in path.as_bytes().iter().chain(body.iter()) {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(PRIME);
        }
        println!("cargo:rerun-if-changed={path}");
    }
    println!("cargo:rustc-env=SPRAG_STATECHARTS_FINGERPRINT={hash:016x}");
    // ⚠⚠⚠ WHAT WENT INTO IT, so the crate's own gate can RECOMPUTE the number instead of trusting
    // it. A fingerprint nothing checks is a constant, and a constant compared against itself would
    // report every document unchanged for ever — which is precisely the skew it exists to catch,
    // wearing its own name. Emitted rather than re-listed in the test: one list, one place.
    println!(
        "cargo:rustc-env=SPRAG_STATECHART_SOURCES={}",
        sources.join(",")
    );
}
