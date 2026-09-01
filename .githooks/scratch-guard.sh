#!/usr/bin/env bash
# WHETHER A HARNESS IS STANDING IN ITS OWN SCRATCH REPOSITORY -- one decision,
# shared by every selftest in this directory, and DRIVABLE.
#
# ⛔⛔⛔⛔⛔ WHY THIS IS A FILE AND NOT THREE LINES IN EACH HARNESS. Register item
# 792: a `local PATH` left `$tmp` empty, `git -C ""` was read as *stay where you
# are*, and `hosted-read.sh --selftest` re-initialised the REAL repository,
# replaced the operator's git identity and overwrote the marker it exists to
# keep. The check that stops that was then written inline -- and inline, in
# `loop-read.sh`, it is a DEAD CONTROL: that harness always inits into a
# subdirectory of its own scratch, so no real run can produce the case. Item 799
# paid this exact lesson one round earlier in Rust and the shape is the same:
# when the population cannot produce the case a guard is for, do not assert
# around the guard -- extract the decision and hand it the case.
#
# ⛔⛔⛔ AND THE FOLD THAT COST A PLATFORM. The first version made TWO claims as
# one comparison -- *am I inside my own scratch* and *it is not the caller's* --
# by testing `git rev-parse --absolute-git-dir` against `pwd`. On macOS `mktemp
# -d` answers under `/var`, a symlink to `/private/var`, so the logical and
# physical paths disagree and the harness REFUSED TO RUN, every time, on every
# macOS job. Nobody saw it for as long as nobody ran the selftest; the gate item
# 799 built ran it, and CI went red the next push. The claims are separate here,
# and the arms below drive the exact pair that refused.
#
# Self-tested: `bash .githooks/scratch-guard.sh --selftest`, run by
# `crates/sprag-gate/` without anybody adding it to a list (register item 799).
set -uo pipefail

# WHY a harness must NOT run, or empty when nothing is wrong -- a pure function of
# three strings, so every case can be handed to it.
#
# `$1` the git dir the scratch repository reports; `$2` the scratch directory's
# own PHYSICAL path; `$3` the caller's git dir, or empty where it has none.
#
# ⚠⚠ THE ORDER IS THE POINT. *No git dir at all* and *a git dir belonging to
# somebody else* are different failures with different remedies, and a caller
# that got one sentence for both would not know which it had.
scratch_guard_refusal() {
    local scratch_git scratch_dir caller_git
    scratch_git="${1:-}"
    scratch_dir="${2:-}"
    caller_git="${3:-}"
    if [ -z "$scratch_git" ] || [ -z "$scratch_dir" ]; then
        printf 'the scratch repository reports no git dir of its own\n'
        return 0
    fi
    # ⚠ PHYSICAL ON BOTH SIDES. This is the comparison that refused on macOS when
    # one side was logical, and a symlinked TMPDIR is a normal thing there, not a
    # finding. What it still catches is a scratch whose git dir belongs to an
    # ANCESTOR -- a directory that was never `git init`-ed at all.
    if [ "$scratch_git" != "${scratch_dir}/.git" ]; then
        printf 'the scratch git dir is not the scratch directory own -- %s\n' \
               "an ancestor repository would answer for it"
        return 0
    fi
    # ⛔ AND THE ONE THAT KEEPS SOMEBODY ELSE'S MARKER SAFE. Measured 2026-09-01
    # by deleting this clause and injecting a scratch that WAS the caller: the
    # harness ran to completion, reported 39/39, and its own `rm -rf` deleted the
    # caller's repository. That is item 792 reproduced on demand.
    if [ -n "$caller_git" ] && [ "$scratch_git" = "$caller_git" ]; then
        printf 'the scratch repository IS the caller repository\n'
        return 0
    fi
}

# The same decision asked about a LIVE directory: prints why `$1` must not be
# used as a scratch repository, or nothing.
scratch_guard_check() {
    local dir scratch_git scratch_dir caller_git
    dir="${1:-}"
    caller_git="$(git rev-parse --absolute-git-dir 2>/dev/null || true)"
    scratch_git="$(git -C "$dir" rev-parse --absolute-git-dir 2>/dev/null || true)"
    scratch_dir="$(cd "$dir" 2>/dev/null && pwd -P || true)"
    scratch_guard_refusal "$scratch_git" "$scratch_dir" "$caller_git"
}

scratch_guard_selftest() {
    local pass fail said
    pass=0
    fail=0

    # ⛔ THE PAIR THAT REFUSED ON macOS, driven directly. `/var` is a symlink to
    # `/private/var` there, so git answers physically and `pwd` answered
    # logically. With both sides physical this must be SILENT.
    said="$(scratch_guard_refusal '/private/var/x/.git' '/private/var/x' '/repo/.git')"
    if [ -z "$said" ]; then
        echo "  ok    a scratch under a symlinked TMPDIR is not a finding"
        pass=$((pass + 1))
    else
        echo "  FAIL  a physical pair was refused: $said"
        fail=$((fail + 1))
    fi
    # ⚠ And the logical/physical pair itself, so the reason `pwd -P` is written
    # that way is a PREDICATE rather than a sentence in a comment.
    said="$(scratch_guard_refusal '/private/var/x/.git' '/var/x' '/repo/.git')"
    if [ -n "$said" ]; then
        echo "  ok    a logical path against a physical git dir is refused"
        pass=$((pass + 1))
    else
        echo "  FAIL  the macOS pair passed, so pwd -P is doing nothing"
        fail=$((fail + 1))
    fi

    # ⛔ The case no harness in this tree can produce, which is why it is here.
    said="$(scratch_guard_refusal '/repo/.git' '/repo' '/repo/.git')"
    case "$said" in
        *"IS the caller"*)
            echo "  ok    a scratch that is the caller's own repository is refused"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  the caller's own repository said: '$said'"
            fail=$((fail + 1)) ;;
    esac

    # ⚠ An uninitialised scratch answers with an ANCESTOR's git dir.
    said="$(scratch_guard_refusal '/repo/.git' '/repo/scratch' '/elsewhere/.git')"
    case "$said" in
        *"not the scratch directory own"*)
            echo "  ok    an ancestor's git dir is refused"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  an ancestor git dir said: '$said'"
            fail=$((fail + 1)) ;;
    esac

    # ⚠ Nothing at all is its OWN sentence, not folded into the one above.
    said="$(scratch_guard_refusal '' '' '/repo/.git')"
    case "$said" in
        *"no git dir of its own"*)
            echo "  ok    a scratch with no git dir keeps its own words"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  an absent git dir said: '$said'"
            fail=$((fail + 1)) ;;
    esac

    # ⚠ A caller with NO repository must not make every scratch look like it.
    said="$(scratch_guard_refusal '/tmp/s/.git' '/tmp/s' '')"
    if [ -z "$said" ]; then
        echo "  ok    an empty caller git dir refuses nothing"
        pass=$((pass + 1))
    else
        echo "  FAIL  an empty caller git dir refused: $said"
        fail=$((fail + 1))
    fi

    # ⚠ And the live form agrees with the pure one about THIS repository, which
    # is what stops the two from drifting apart.
    said="$(scratch_guard_check "$(git rev-parse --show-toplevel 2>/dev/null || echo /nonexistent)")"
    case "$said" in
        *"IS the caller"*)
            echo "  ok    the live form calls this repository the caller's own"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  the live form said: '$said'"
            fail=$((fail + 1)) ;;
    esac

    echo "scratch-guard selftest: ${pass}/$((pass + fail)) arm(s) pass"
    [ "$fail" -eq 0 ]
}

if [ "${BASH_SOURCE[0]}" = "${0}" ]; then
    case "${1:-}" in
        --selftest) scratch_guard_selftest ;;
        --check)    shift; scratch_guard_check "${1:-}" ;;
        *) echo "usage: scratch-guard.sh [--check DIR|--selftest]" >&2
           exit 2 ;;
    esac
fi
