//! Build-time codegen: each control statechart -> `OUT_DIR/<stem>_sm.rs` via
//! SCE, mirroring pinion's `compile_scxml` usage.
//!
//! Two machines today (the dogfood generalizing to N): `orchestration.scxml`
//! (the Driver's run lifecycle) and `session.scxml` (an endpoint's server-session
//! lifecycle). `compile_scxml` emits one `<stem>_sm.rs` per input; `lib.rs`
//! `include!`s each into its own `sm::<stem>` submodule.

use std::path::Path;

/// The control statecharts, by file stem (the generated file is `<stem>_sm.rs`).
const STATECHARTS: &[&str] = &["orchestration", "session"];

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
