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

use std::path::Path;

/// The control statecharts, by file stem (the generated file is `<stem>_sm.rs`).
const STATECHARTS: &[&str] = &["orchestration", "session", "ai_loop"];

fn main() {
    let sources: Vec<String> = STATECHARTS
        .iter()
        .map(|stem| format!("src/{stem}.scxml"))
        .collect();
    sce_build::compile_scxml(&sources.iter().map(String::as_str).collect::<Vec<_>>());

    // `include!` rejects inner attributes (`#![...]`) and inner doc comments
    // (`//!`) in expansion position, so strip them from every generated file
    // (the pinion-core/build.rs post-processing pattern), one per statechart.
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR set by cargo");
    for stem in STATECHARTS {
        let generated = Path::new(&out_dir).join(format!("{stem}_sm.rs"));
        let cleaned: String = std::fs::read_to_string(&generated)
            .expect("read generated state machine")
            .lines()
            .filter(|line| {
                let trimmed = line.trim_start();
                !trimmed.starts_with("#![") && !trimmed.starts_with("//!")
            })
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&generated, cleaned).expect("write cleaned state machine");
    }
}
