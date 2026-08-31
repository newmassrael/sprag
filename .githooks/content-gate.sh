#!/usr/bin/env bash
# The gates that judge CONTENT, and the mirror they share.
#
# Its own file for the reason `doc-gate.sh` is one: two hooks need it, and a
# reason duplicated is a reason that drifts. That is not a hypothetical here —
# register item 213 was exactly that shape, `-D warnings` added to one copy of a
# clippy line and not the other, with nothing comparing the two for twelve
# commits.
#
# ## Why these gates MATERIALISE the content instead of reading the file on disk
#
# `git diff --cached --name-only` answers with NAMES. The bytes behind those
# names in the working tree are a DIFFERENT THING, and the gates this replaced
# handed those to their checkers while `pre-commit`'s header claimed it "checks
# exactly what is being committed, and nothing else". Driven as a program on
# 2026-08-17 (register item 404) the rustfmt gate was wrong in BOTH directions:
#
#   * stage unformatted Rust, then tidy the file — an editor's format-on-save, a
#     `cargo fmt` — and the commit carried unformatted content and PASSED;
#   * stage formatted Rust, then keep editing, and the commit was REFUSED for a
#     file it was not carrying.
#
# ⚠⚠⚠ AND THE ACTIONLINT GATE HAD THE SAME DEFECT, one gate over, which is why
# both live here now rather than only the one that was noticed first. Fixing one
# copy of a shape and leaving its neighbour is item 213 exactly.
#
# Neither is exotic in this repository: work here stages by PATH and reads
# `git diff --cached` precisely because index and working tree diverge (item
# 196, two writers in one tree), so divergence is the normal state.
#
# ## Why the mirror is the WHOLE tree and not just the paths under judgement
#
# ⚠⚠⚠⚠ MEASURED, AND IT IS THE DIFFERENCE BETWEEN A GATE AND AN OUTAGE.
# rustfmt FOLLOWS `mod` declarations. Handed `crates/sprag-vt/src/lib.rs` in a
# mirror holding only that one file it answers
#
#   Error writing files: failed to resolve mod `closed_set`
#
# and exits nonzero — so a mirror built from the named paths alone would REFUSE
# almost every commit this repository makes, having judged nothing. The first
# draft of this file did exactly that and passed its own fixtures, because a
# fixture written for the format rule has no `mod` in it. It was caught by
# running the gate against real tracked sources.
#
# Laying out the whole tree also settles two smaller questions for free: the
# children a checker descends into are the COMMITTED bytes too, not whatever is
# on disk; and any `rustfmt.toml` arrives where rustfmt's upward walk will find
# it, so the mirror is judged by this project's rules rather than rustfmt's
# defaults. (There is no such file in this tree today — measured.)
#
# Cost, measured on this tree: 239 files, 19M, ~95ms.

# Lay out a whole tree and print where it went. The caller owns the directory
# and must remove it.
#
#   $1  EMPTY for the index, otherwise a commit-ish
#
# Answers nonzero, having printed nothing, if the layout fails.
content_mirror() {
    local rev="$1" mirror index
    # ⛔⛔⛔⛔⛔ THE SCRATCH IS CHECKED IN THE STATEMENT THAT TAKES IT — register
    # item 792. `mktemp` exits 127 when it is not on PATH and the variable is
    # then the EMPTY STRING, which nothing below would notice: `--prefix="$mirror/"`
    # collapses to `--prefix="/"`, and that is `checkout-index` writing this
    # commit's whole tree at the FILESYSTEM ROOT.
    mirror="$(mktemp -d)" || return 1

    if [ -n "$rev" ]; then
        # A scratch index so the layout is that commit's tree and nothing else —
        # in particular not whatever this clone happens to have staged.
        #
        # ⛔⛔⛔⛔⛔ AND THE SAME CHECK, FOR A SHARPER REASON — register item 792.
        # `GIT_INDEX_FILE=""` is read by git as UNSET, which is the REAL index, so
        # an unchecked `mktemp` here would have `git read-tree` overwrite exactly
        # what the operator had staged — the thing this scratch index exists to
        # avoid touching.
        index="$(mktemp)" || { rm -rf "$mirror"; return 1; }
        if ! GIT_INDEX_FILE="$index" git read-tree "$rev" ||
            ! GIT_INDEX_FILE="$index" git checkout-index -a --prefix="$mirror/"; then
            rm -rf "$mirror" "$index"
            return 1
        fi
        rm -f "$index"
    elif ! git checkout-index -a --prefix="$mirror/"; then
        rm -rf "$mirror"
        return 1
    fi

    printf '%s' "$mirror"
}

# rustfmt --check the given paths as `rev` holds them.
#
#   $1   label for the messages — the calling hook's name
#   $2   where to read the content: EMPTY for the index, otherwise a commit-ish
#   $3…  repo-relative paths
#
# Answers nonzero when any of them is unformatted, when the tree cannot be laid
# out, or when rustfmt is not installed. That last one is item 403's settled
# answer for this repository: a style rule whose only tool is missing must
# REFUSE rather than let the content it was written to stop go by.
#
# ⚠ rustfmt prints ABSOLUTE paths, measured rather than assumed: it reports the
# file it opened, not the relative name it was handed. So the mirror's location
# is announced and the repo path is the tail after it.
fmt_gate() {
    local label="$1" rev="$2"
    shift 2
    [ "$#" -gt 0 ] || return 0

    if ! command -v rustfmt >/dev/null 2>&1; then
        echo "$label: rustfmt is not on PATH — install it (rustup component add rustfmt)" >&2
        return 1
    fi

    local mirror status=0
    if ! mirror="$(content_mirror "$rev")"; then
        echo "$label: could not lay out the content to judge — a gate cannot judge what it cannot read" >&2
        return 1
    fi

    echo "$label: rustfmt --check on the content being published ..." >&2
    echo "$label: (mirrored under $mirror — the repo path is the tail after that prefix)" >&2
    if ! (cd "$mirror" && rustfmt --edition 2024 --check "$@"); then
        status=1
    fi
    rm -rf "$mirror"
    return "$status"
}

# actionlint the given workflow paths as `rev` holds them.
#
# Same arguments, and the same reason for the mirror: this gate too used to take
# staged NAMES and read WORKING-TREE bytes.
#
# ⚠⚠ A WORKFLOW FILE IS THE ONE THING CI CANNOT GATE. Every other check has a
# second chance on the runner; a workflow whose expression is invalid does not
# fail a step, it never STARTS — no jobs, no log, and a run that ends in the
# same second it began. R343 shipped exactly that (`runner.temp` in a job-level
# `env:`, where the `runner` context does not exist) and burned a push finding
# out. `yaml.safe_load` cannot see it: the file PARSES.
#
# Skipped with a WORD rather than silently when the tool is absent, because a
# gate that vanishes quietly is one people stop expecting. That is a weaker
# stance than the rustfmt gate's refusal above, and deliberately so: actionlint
# is not part of any toolchain this project pins, so demanding it would block
# every commit on a fresh clone.
workflow_gate() {
    local label="$1" rev="$2"
    shift 2
    [ "$#" -gt 0 ] || return 0

    if ! command -v actionlint >/dev/null 2>&1; then
        echo "$label: actionlint NOT INSTALLED — a workflow change is going out UNCHECKED." >&2
        echo "$label: install it (https://github.com/rhysd/actionlint) before touching CI again." >&2
        return 0
    fi

    local mirror status=0
    if ! mirror="$(content_mirror "$rev")"; then
        echo "$label: could not lay out the content to judge — a gate cannot judge what it cannot read" >&2
        return 1
    fi

    echo "$label: actionlint on the workflows being published ..." >&2
    echo "$label: (mirrored under $mirror — the repo path is the tail after that prefix)" >&2
    if ! (cd "$mirror" && actionlint "$@"); then
        status=1
    fi
    rm -rf "$mirror"
    return "$status"
}
