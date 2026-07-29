//! The gate that makes this crate worth having: `sprag-tui` reaches no GPU crate.
//!
//! [`sprag-client`'s copy of this test](../../sprag-client/tests/gpu_free.rs) states the general
//! rule and the two traps in it (why it shells out to `cargo tree`, and why `-e normal` is what
//! caught its own revert-proof). This one exists SEPARATELY rather than being folded into that
//! file because the packages are different: a GPU dependency added to `sprag-tui`'s own manifest
//! — a status widget that reached for a pinion painter, say — would never appear in
//! `sprag-client`'s tree, and the shared gate would stay green while the shipped binary stopped
//! building on a headless server.
//!
//! The duplication is ~40 lines of test. The alternative was a helper exported from a library for
//! a test's benefit, which widens a public API to save a copy — the wrong trade for a crate whose
//! whole surface is two functions.

use std::process::Command;

/// The crates whose presence would mean a GPU stack came along: the renderer, the graphics
/// abstraction, and the windowing layer. Three rather than one because they enter independently —
/// a client could acquire `winit` (a window) without `vello` (a painter).
const GPU_CRATES: [&str; 3] = ["vello", "wgpu", "winit"];

/// `sprag-tui`'s resolved dependency closure contains no GPU crate.
///
/// **Measured against THIS WORKSPACE**, which is the qualifier that makes the answer the one that
/// ships: `cargo tree -p <crate>` asked in isolation activates that crate's DEFAULT features and
/// answers a different question (`pinion-runtime` reports 16 GPU dependencies standalone and ZERO
/// inside this workspace's tree). Running from the workspace root — as a test in it does — is what
/// pins that.
///
/// The revert-proof: add `pinion-shell = { workspace = true }` under `[dependencies]` in this
/// crate's manifest and the test fails, naming `["winit", "vello", "wgpu", …]`. **Check which
/// table the line lands in** — appended to the end of the file it becomes a DEV dependency, which
/// `-e normal` correctly excludes, and the proof silently passes while proving nothing.
#[test]
fn the_terminal_client_reaches_no_gpu_crate() {
    let output = Command::new(env!("CARGO"))
        .args([
            "tree",
            "-p",
            "sprag-tui",
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
    // path containing "winit" raise a false alarm.
    let found: Vec<&str> = tree
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|name| GPU_CRATES.contains(name))
        .collect();

    assert!(
        found.is_empty(),
        "sprag-tui is the frontend for a machine with no GPU, and its dependency closure now \
         contains {found:?}. Whatever needed that belongs in sprag-gui — or the H1 design has to \
         change before this test does.",
    );

    // The test must be able to FAIL, and a tree that came back empty or unparsed would pass
    // vacuously. `termwiz` is this crate's defining direct dependency, so its absence means the
    // output is not what this test thinks it is reading.
    assert!(
        tree.lines()
            .filter_map(|line| line.split_whitespace().next())
            .any(|name| name == "termwiz"),
        "the tree did not parse as expected, so an empty GPU result proves nothing:\n{tree}",
    );
}
