//! The gate this crate exists to pass: `sprag-client` reaches no GPU crate.
//!
//! `sprag-gui` is the ONLY sprag crate allowed a GPU dependency; everything else — the headless
//! daemon, and now the shared client half — stays free of it, which is what makes a terminal
//! frontend (H1) possible at all. That property is mechanical, so it is asserted mechanically
//! rather than left to whoever next adds a dependency to notice.
//!
//! # Why this is a test and not a comment
//!
//! A `Cargo.toml` reviewed once is a promise; a test is a gate. `pinion-core` and
//! `pinion-widget-paint` are GPU-free while `pinion-shell` and `pinion-runtime` are not, and they
//! all share a prefix — so the mistake this catches is a plausible one-word edit, not negligence.
//!
//! # Why it shells out to cargo instead of reading `Cargo.toml`
//!
//! The manifest lists DIRECT dependencies, and the hazard is transitive: a GPU crate arrives
//! through something innocuous. Only the resolver knows the closure, so the resolver is asked.

use std::process::Command;

/// The crates whose presence would mean a GPU stack came along: the renderer, the graphics
/// abstraction, and the windowing layer. Naming three rather than one is deliberate — they enter
/// independently, and a client could acquire `winit` (a window) without `vello` (a painter).
const GPU_CRATES: [&str; 3] = ["vello", "wgpu", "winit"];

/// `sprag-client`'s resolved dependency closure contains no GPU crate.
///
/// **Measured against THIS WORKSPACE, and that qualifier is the test's sharpest edge.** Asking
/// `cargo tree -p <crate>` about a crate in ISOLATION activates that crate's DEFAULT features and
/// answers a different question: `pinion-runtime` reports 16 GPU dependencies standalone and ZERO
/// inside `sprag-host`'s tree, because this workspace's feature unification resolves it without
/// them. Running from the workspace root — as a test in it does — is what makes the answer the one
/// that ships.
///
/// `-e normal` excludes dev- and build-dependencies on purpose: a dev-dependency cannot reach a
/// consumer of this crate, so counting one would fail the test for something that is not the
/// property being claimed.
///
/// **That exclusion is MEASURED, and it caught this test's own revert-proof first.** Adding
/// `pinion-shell` to the manifest and expecting a failure is the obvious way to prove the gate
/// bites — and the first attempt appended it after the `[dev-dependencies]` header, so it landed
/// as a dev-dependency and the test correctly stayed green. The proof had routed around the
/// mechanism it was named after. Moved under `[dependencies]`, it fails as it should, naming
/// `["winit", "vello", "wgpu", …]`. Anyone re-running it must check WHICH table the line went
/// into.
#[test]
fn the_shared_client_reaches_no_gpu_crate() {
    let output = Command::new(env!("CARGO"))
        .args([
            "tree",
            "-p",
            "sprag-client",
            "-e",
            "normal",
            "--prefix",
            "none",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run cargo tree");
    assert!(
        output.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&output.stderr),
    );
    let tree = String::from_utf8_lossy(&output.stdout);

    // A crate NAME, not a substring of a line: `--prefix none` puts one dependency per line as
    // `name version (source)`, so the name is the first token. Matching the whole line would let a
    // path containing "winit" or a crate merely NAMED after one raise a false alarm.
    let found: Vec<&str> = tree
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|name| GPU_CRATES.contains(name))
        .collect();

    assert!(
        found.is_empty(),
        "sprag-client must stay GPU-free so a terminal frontend can share it, but its dependency \
         closure now contains {found:?}. If this is intended, the H1 design (a second frontend \
         that cannot link a GPU stack) has to change first — not this test.",
    );

    // The test must be able to FAIL, and a tree that came back empty or unparsed would pass
    // vacuously while proving nothing. `sprag-host` is a direct dependency, so its absence means
    // the output was not what this test thinks it is reading.
    assert!(
        tree.lines()
            .filter_map(|line| line.split_whitespace().next())
            .any(|name| name == "sprag-host"),
        "the tree did not parse as expected, so an empty GPU result proves nothing:\n{tree}",
    );
}
