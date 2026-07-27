#!/usr/bin/env bash
# The rustdoc gate, shared by pre-commit and pre-push.
#
# Its own file rather than a block copied into both hooks: the reasoning below
# is the non-obvious part, and a reason duplicated is a reason that drifts. The
# other gates stay inline in each hook — this is the doc gate's home, not a
# general hook library.
#
# ## Why `--document-private-items` is the ONLY pass
#
# The obvious gate is `cargo doc --no-deps`, the published surface. It is the
# weaker of the two and it was measured so, not assumed:
#
#   * a broken link inside a PRIVATE item's doc — public pass 0 errors,
#     private pass 2. The public pass never reads those docs at all, and this
#     crate keeps most of its reasoning in private items, so this is where the
#     breakage actually lands: five dead links had built up across sprag-host,
#     sprag-terminal and sprag-vt before anything ran rustdoc.
#   * `rustdoc::private_intra_doc_links`, a PUBLIC doc pointing at a private
#     item — caught by BOTH. It was worth checking, because the intuition that
#     documenting the private item makes the link resolve is wrong: the lint
#     keys on the item's VISIBILITY, which the flag does not change.
#
# That second result is the general shape. `--document-private-items` widens
# the set of docs rustdoc READS; it does not relax any lint. So the private
# pass sees a superset and a public pass beside it earns nothing but ~3s and
# ~1.3G of its own build directory. BOUND: verified against those two classes,
# not against every rustdoc lint — if one ever turns out to be public-only,
# adding the second pass back is a two-line change.
#
# ## Why a dedicated target directory
#
# Sharing `target/` with the build would make rustdoc and clippy invalidate
# each other's fingerprints, so every commit would re-document the whole
# workspace (~40s) and then rebuild for the next clippy. Given a directory of
# its own the gate is ~3s once warm, and the build artifacts the same commit
# just used stay untouched.
#
# Cost: one ~40s build per clone, then ~3s per commit; ~1.3G of disk, under
# `target/` and so already ignored.
#
# `-D warnings` is what makes it a gate: rustdoc reports a dead link as a
# warning, and a warning nothing reads is how this debt accumulated.

# Document the workspace including private items, failing on any warning.
doc_gate() {
    echo "doc gate: cargo doc --workspace --no-deps --document-private-items ..." >&2
    RUSTDOCFLAGS="-D warnings" CARGO_TARGET_DIR=target/doc-gate \
        cargo doc --workspace --no-deps --document-private-items
}
