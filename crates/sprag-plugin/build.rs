//! Build-time codegen: `orchestration.scxml` -> `OUT_DIR/orchestration_sm.rs`
//! via SCE, mirroring pinion's `compile_scxml` usage.

use std::path::Path;

fn main() {
    sce_build::compile_scxml(&["src/orchestration.scxml"]);

    // `include!` rejects inner attributes (`#![...]`) and inner doc comments
    // (`//!`) in expansion position, so strip them from the generated file
    // (the pinion-core/build.rs post-processing pattern).
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR set by cargo");
    let generated = Path::new(&out_dir).join("orchestration_sm.rs");
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
