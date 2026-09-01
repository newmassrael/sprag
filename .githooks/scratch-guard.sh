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

# THE GIT DIR THIS CLONE'S MARKERS BELONG IN, or EMPTY where there is none.
#
# ⛔⛔⛔⛔⛔ EMPTY RATHER THAN A FALLBACK — register item 804. `git rev-parse
# --absolute-git-dir` prints NOTHING outside a repository, and a caller that
# joined that answer to a file name got `/sprag-loop-read`: the FILESYSTEM ROOT.
# Measured 2026-09-01 by running `loop-read.sh` from `memory/`, which is not a
# repository. Handing back an empty string makes the caller SAY SO instead.
scratch_guard_marker_home() {
    git rev-parse --absolute-git-dir 2>/dev/null || true
}

# WRITE stdin to `$1`, and REFUSE rather than claim success when it did not land.
# `$2` names the instrument, for the sentence.
#
# ⛔⛔⛔⛔⛔ THE DEFECT THIS EXISTS FOR — register item 804, and it is the sharpest
# shape this directory has produced. `loop-read.sh --baseline`, run outside a
# repository, wrote to `/sprag-loop-read`, got *"Permission denied"* from the
# shell, PRINTED *"139 ending(s) already on disk are the baseline"* and EXITED 0.
# A file whose entire subject is WHAT SOMEBODY HAS RECORDED READING cannot report
# a record it did not make: that is not a cosmetic error, it is the instrument
# lying about its own state, and every later `--gap` would then be answering from
# a marker that was never written.
#
# ⚠⚠ AND IT IS NOT ABOUT BEING OUTSIDE A REPOSITORY. That is one road to it; a
# full disk, a read-only mount and a marker somebody chmod-ed are others, and all
# of them arrive as the same silent zero. The status of the write is the fact,
# so the status of the write is what is read.
scratch_guard_write() {
    local at who
    at="${1:-}"
    who="${2:-this instrument}"
    if [ -z "$at" ]; then
        echo "${who}: there is no marker path to write to, so nothing was" \
             "recorded -- this clone has no git dir of its own (item 804)" >&2
        return 1
    fi
    if ! cat > "$at"; then
        echo "${who}: the marker at '${at}' could not be written, so NOTHING was" \
             "recorded -- reporting success here would be this instrument lying" \
             "about its own state (item 804)" >&2
        return 1
    fi
}

# The same, APPENDING. A separate arm because `>>` and `>` fail for the same
# reasons and succeed differently, and a caller that wanted one and got the other
# would lose everything already recorded.
scratch_guard_append() {
    local at who
    at="${1:-}"
    who="${2:-this instrument}"
    if [ -z "$at" ]; then
        echo "${who}: there is no marker path to append to, so nothing was" \
             "recorded -- this clone has no git dir of its own (item 804)" >&2
        return 1
    fi
    if ! cat >> "$at"; then
        echo "${who}: the marker at '${at}' could not be appended to, so NOTHING" \
             "was recorded -- reporting success here would be this instrument" \
             "lying about its own state (item 804)" >&2
        return 1
    fi
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

    # ⛔⛔⛔⛔⛔ THE MARKER-HOME AND WRITE ARMS — register item 804. A write whose
    # status nobody reads is the defect, so these drive the status.
    said="$(scratch_guard_marker_home)"
    case "$said" in
        /*) echo "  ok    inside a repository the marker home is an absolute git dir"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  the marker home here was '$said'"
            fail=$((fail + 1)) ;;
    esac
    # ⚠ `/` is not a repository and has no `.git` above it, so this is the shape a
    # hook run from anywhere else meets. EMPTY, never a path under the root.
    said="$(cd / && scratch_guard_marker_home)"
    if [ -z "$said" ]; then
        echo "  ok    outside a repository the marker home is EMPTY, not a root path"
        pass=$((pass + 1))
    else
        echo "  FAIL  outside a repository the marker home was '$said'"
        fail=$((fail + 1))
    fi

    # ⛔ An empty destination is refused rather than joined to a file name.
    if printf 'x\n' | scratch_guard_write "" "probe" >/dev/null 2>&1; then
        echo "  FAIL  a write to an empty path reported success"
        fail=$((fail + 1))
    else
        echo "  ok    a write with no marker path is refused"
        pass=$((pass + 1))
    fi
    if printf 'x\n' | scratch_guard_append "" "probe" >/dev/null 2>&1; then
        echo "  FAIL  an append to an empty path reported success"
        fail=$((fail + 1))
    else
        echo "  ok    an append with no marker path is refused"
        pass=$((pass + 1))
    fi

    # ⛔⛔ AND A DESTINATION THAT EXISTS BUT CANNOT BE WRITTEN. This is the case the
    # defect actually arrived as: the shell refused, and the caller announced
    # success anyway. A directory has no writable file behind its name.
    if printf 'x\n' | scratch_guard_write "/" "probe" >/dev/null 2>&1; then
        echo "  FAIL  a write that could not land reported success"
        fail=$((fail + 1))
    else
        echo "  ok    a write that could not land is refused, not announced"
        pass=$((pass + 1))
    fi
    if printf 'x\n' | scratch_guard_append "/" "probe" >/dev/null 2>&1; then
        echo "  FAIL  an append that could not land reported success"
        fail=$((fail + 1))
    else
        echo "  ok    an append that could not land is refused, not announced"
        pass=$((pass + 1))
    fi

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
