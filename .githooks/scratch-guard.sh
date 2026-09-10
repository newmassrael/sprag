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
# four strings, so every case can be handed to it.
#
# `$1` the git dir the scratch repository reports; `$2` the scratch directory's
# own PHYSICAL path; `$3` the caller's git dir, or empty where it has none;
# `$4` the git dir the scratch directory DECLARES as its own, or empty where it
# declares none -- see `scratch_guard_own_git_dir`, which is what reads it.
#
# ⚠⚠ THE ORDER IS THE POINT. *No git dir at all* and *a git dir belonging to
# somebody else* are different failures with different remedies, and a caller
# that got one sentence for both would not know which it had.
scratch_guard_refusal() {
    local scratch_git scratch_dir caller_git scratch_own
    scratch_git="${1:-}"
    scratch_dir="${2:-}"
    caller_git="${3:-}"
    scratch_own="${4:-}"
    if [ -z "$scratch_git" ] || [ -z "$scratch_dir" ]; then
        printf 'the scratch repository reports no git dir of its own\n'
        return 0
    fi
    # ⚠ PHYSICAL ON BOTH SIDES. This is the comparison that refused on macOS when
    # one side was logical, and a symlinked TMPDIR is a normal thing there, not a
    # finding. What it still catches is a scratch whose git dir belongs to an
    # ANCESTOR -- a directory that was never `git init`-ed at all.
    #
    # ⛔⛔⛔⛔⛔ IT USED TO SPELL THE RIGHT-HAND SIDE AS `${scratch_dir}/.git`, AND
    # THAT IS A CLONE'S LAYOUT AND NOT GIT'S -- register item 1014. In a LINKED
    # WORKTREE `.git` is a **file** holding `gitdir: <path>`, and the repository's
    # real git dir is `<main>/.git/worktrees/<name>`; the two can never be equal,
    # so every linked worktree read as *an ancestor repository would answer for
    # it* however properly it had been created. MEASURED 2026-09-10 by running
    # this file's own selftest from one: **18/19, and the arm that failed is the
    # live one**, against 19/19 in the main worktree.
    #
    # ⚠⚠ NARROWER, NOT LOOSER, and that matters because this is the clause that
    # keeps a harness out of somebody else's tree. The right-hand side is now what
    # the directory ITSELF declares rather than what a clone would have declared,
    # so a directory that was never `git init`-ed still declares NOTHING, still
    # mismatches, and is still refused with this same sentence. The six fixture
    # pairs below pass `${scratch_dir}/.git` and their answers do not move.
    if [ "$scratch_git" != "$scratch_own" ]; then
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

# ⛔⛔⛔⛔⛔ THE THIRD ROAD INTO THE CALLER'S REPOSITORY, AND THE ONE `git -C`
# CANNOT CLOSE -- register item 965.
#
# Every refusal above compares DIRECTORIES, because the two failures it was
# written for arrived as a directory: an empty `-C`, and a scratch that was the
# caller. An AMBIENT git environment arrives as neither. `git commit --
# <pathspec>` hands its hooks an ABSOLUTE `GIT_INDEX_FILE` -- the temporary
# index it is about to commit -- and that variable outranks `-C`, `cd` and the
# repository discovery all three: `git -C <scratch> add a` finds the file in the
# scratch and writes the entry into the CALLER'S index.
#
# ⚠⚠ MEASURED 2026-09-08, in a throwaway repository, so the numbers are this
# machine's rather than the register's: a scratch holding ONE file answered
# `git -C <scratch> ls-files --cached --others` with **343** paths under an
# ambient index and **1** without it, and driving the three selftests that build
# a repository took the caller's index from **2** entries to **4**, **5** and
# **7**. The caller's index is then what git commits, so where the scratch's
# blobs happened to exist already the commit SUCCEEDED and published the
# scratch's content -- four such commits reached `main` -- and where they did not
# it failed with `invalid object ... for 'src/main.rs'`.
#
# ⚠⚠⚠ AND THE PREMISE THAT LOOKED LIKE SAFETY. An ordinary `git commit` sets the
# variable TOO; it is simply RELATIVE (`.git/index`), so it re-resolves inside
# whatever directory git was given and lands on the scratch's own index by
# accident. Nothing about the ordinary path is defended -- it is one absolute
# value away from this, which is why the cut below is unconditional.

# THE GIT ENVIRONMENT THIS PROCESS WOULD HAND A CHILD -- every exported `GIT_`
# name, one per line, or nothing.
#
# ⛔⛔⛔ THE WHOLE NAMESPACE, NOT A LIST OF THE DANGEROUS ONES. A list of names to
# refuse passes every variable nobody has thought of yet, and the one that cost
# this repository four commits on `main` was already unlisted by the guard that
# exists for exactly this failure. A harness that builds its own repository
# needs NO inherited git environment: `git` finds its exec path, its config, its
# object store and its index from the directory it is handed. So the population
# is the namespace, and there is no exemption list to keep current.
#
# ⚠ EXPORTED, because that is what a child process can read. A `local GIT_DIR`
# in some function reaches no `git` at all, and refusing it would be this guard
# reporting on itself.
scratch_guard_ambient_names() {
    local name
    for name in $(compgen -e 2>/dev/null); do
        case "$name" in
            GIT_*) printf '%s\n' "$name" ;;
        esac
    done
}

# WHY A HARNESS MUST NOT RUN WITH THE GIT ENVIRONMENT IT INHERITED, or empty
# where it carries none.
#
# `$1` is that environment as NAMES, whitespace-separated -- a string rather
# than the live environment, so every case can be handed to it. The values are
# deliberately not read: a value that is safe today (`.git/index`, relative) is
# the same variable that was absolute yesterday, and a guard that judged the
# value would have to be right about git's resolution rules forever.
#
# ⚠⚠ WHAT IT CLAIMS IS *NOT CUT*, NOT *REACHES THE SCRATCH*, and the difference
# was measured rather than reasoned: the first draft said the environment
# "reaches past -C into the scratch" and the mutation run printed that sentence
# about `GIT_EDITOR`, which this session exports and which can reach nothing.
# The claim has to be true of every member of the population it is written over,
# and the population is the namespace -- so the fact is that the cut did not
# happen, which is true of `GIT_EDITOR` and of `GIT_INDEX_FILE` alike.
scratch_guard_ambient_refusal() {
    local carried
    carried="$(printf '%s' "${1:-}" | tr '\n' ' ' | tr -s ' ' | sed 's/^ //; s/ $//')"
    if [ -n "$carried" ]; then
        printf 'the git environment this run inherited was not cut -- %s %s\n' \
               "$carried still set, and a scratch harness needs none of it:" \
               "cut it rather than moving the scratch"
    fi
}

# CUT that environment out of THIS shell, so nothing it starts inherits it.
#
# ⚠⚠ `unset` HERE rather than an `env -u` prefix at each call site. A harness
# runs git from subshells, from `cd`-ed blocks and from functions it does not
# own; one that had to remember a prefix everywhere would be one forgotten
# prefix away from the defect coming back. Its callers run it in the dispatch of
# a directly-executed file, never on the sourced path, so a HOOK -- which needs
# the very index this removes -- keeps its own environment untouched.
scratch_guard_cut_ambient() {
    local name
    for name in $(scratch_guard_ambient_names); do
        unset "$name"
    done
}

# The same decision asked about a LIVE directory: prints why `$1` must not be
# used as a scratch repository, or nothing.
#
# ⚠ THE AMBIENT ENVIRONMENT IS ASKED FIRST, and not because it is likelier --
# because its remedy is the only one that is not *use a different directory*.
# A caller told to move its scratch would move it and be contaminated there too.
scratch_guard_check() {
    local dir scratch_git scratch_dir caller_git ambient
    dir="${1:-}"
    ambient="$(scratch_guard_ambient_refusal "$(scratch_guard_ambient_names)")"
    if [ -n "$ambient" ]; then
        printf '%s\n' "$ambient"
        return 0
    fi
    caller_git="$(git rev-parse --absolute-git-dir 2>/dev/null || true)"
    scratch_git="$(git -C "$dir" rev-parse --absolute-git-dir 2>/dev/null || true)"
    scratch_dir="$(cd "$dir" 2>/dev/null && pwd -P || true)"
    scratch_guard_refusal "$scratch_git" "$scratch_dir" "$caller_git" \
        "$(scratch_guard_own_git_dir "$scratch_dir")"
}

# THE GIT DIR A DIRECTORY DECLARES AS ITS OWN, physically, or EMPTY where it
# declares none -- register item 1014.
#
# ⛔⛔⛔⛔⛔ `.git` HAS TWO LAYOUTS AND THIS FILE KNEW ONE. In a clone it is a
# DIRECTORY. In a linked worktree it is a FILE holding `gitdir: <path>`, and that
# path is the worktree's own git dir -- exclusively its own, with its own index
# and its own HEAD, which is exactly the property the clause above is asking
# about. Reading only the first layout made every linked worktree look like a
# directory nobody had ever `git init`-ed.
#
# ⚠ PHYSICAL, because the clause compares against `git rev-parse
# --absolute-git-dir`, which answers physically -- the macOS `/var` -> `/private/var`
# pair at the top of this file is the whole reason that comparison is careful.
# `cd` + `pwd -P` is how the rest of this file spells that.
#
# ⚠⚠ EMPTY IS AN ANSWER AND NOT A FAILURE: a directory that declares no git dir
# of its own is the ancestor case, and the caller REFUSES on it. So every path
# out of here that cannot answer answers empty, and nothing here may invent one.
scratch_guard_own_git_dir() {
    local dir pointer
    dir="${1:-}"
    [ -n "$dir" ] || return 0
    if [ -d "$dir/.git" ]; then
        (cd "$dir/.git" 2>/dev/null && pwd -P) || true
        return 0
    fi
    [ -f "$dir/.git" ] || return 0
    # ⛔ `: *` AND NOT `: \+` -- register item 1006. `\+` is a GNU extension to a
    # basic regular expression and BSD reads it as a literal `+`, so on macOS this
    # would match nothing and every worktree would go back to being refused -- the
    # very platform whose symlinked TMPDIR this file already carries a scar from.
    pointer="$(sed -n 's/^gitdir: *//p' "$dir/.git" 2>/dev/null | head -1)"
    [ -n "$pointer" ] || return 0
    # ⚠ git writes an absolute path when the worktree was added by absolute path
    # and a relative one otherwise; a relative pointer is relative to the file.
    case "$pointer" in
        /*) ;;
        *) pointer="$dir/$pointer" ;;
    esac
    (cd "$pointer" 2>/dev/null && pwd -P) || true
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
    local pass fail said wt_tmp wt_own
    pass=0
    fail=0

    # ⛔ THE PAIR THAT REFUSED ON macOS, driven directly. `/var` is a symlink to
    # `/private/var` there, so git answers physically and `pwd` answered
    # logically. With both sides physical this must be SILENT.
    said="$(scratch_guard_refusal '/private/var/x/.git' '/private/var/x' '/repo/.git' '/private/var/x/.git')"
    if [ -z "$said" ]; then
        echo "  ok    a scratch under a symlinked TMPDIR is not a finding"
        pass=$((pass + 1))
    else
        echo "  FAIL  a physical pair was refused: $said"
        fail=$((fail + 1))
    fi
    # ⚠ And the logical/physical pair itself, so the reason `pwd -P` is written
    # that way is a PREDICATE rather than a sentence in a comment.
    said="$(scratch_guard_refusal '/private/var/x/.git' '/var/x' '/repo/.git' '/var/x/.git')"
    if [ -n "$said" ]; then
        echo "  ok    a logical path against a physical git dir is refused"
        pass=$((pass + 1))
    else
        echo "  FAIL  the macOS pair passed, so pwd -P is doing nothing"
        fail=$((fail + 1))
    fi

    # ⛔ The case no harness in this tree can produce, which is why it is here.
    said="$(scratch_guard_refusal '/repo/.git' '/repo' '/repo/.git' '/repo/.git')"
    case "$said" in
        *"IS the caller"*)
            echo "  ok    a scratch that is the caller's own repository is refused"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  the caller's own repository said: '$said'"
            fail=$((fail + 1)) ;;
    esac

    # ⚠ An uninitialised scratch answers with an ANCESTOR's git dir.
    said="$(scratch_guard_refusal '/repo/.git' '/repo/scratch' '/elsewhere/.git' '')"
    case "$said" in
        *"not the scratch directory own"*)
            echo "  ok    an ancestor's git dir is refused"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  an ancestor git dir said: '$said'"
            fail=$((fail + 1)) ;;
    esac

    # ⛔⛔⛔⛔⛔ THE LINKED WORKTREE, WHICH THIS FILE READ AS AN ANCESTOR'S FOR AS
    # LONG AS IT EXISTED -- register item 1014. Its `.git` is a FILE, so the old
    # right-hand side `${scratch_dir}/.git` could never equal what git reports,
    # and a worktree created entirely properly was refused every time.
    said="$(scratch_guard_refusal '/repo/.git/worktrees/wt' '/scratch/wt' '/elsewhere/.git' '/repo/.git/worktrees/wt')"
    if [ -z "$said" ]; then
        echo "  ok    a linked worktree is its own repository, not an ancestor's"
        pass=$((pass + 1))
    else
        echo "  FAIL  a properly created linked worktree was refused: $said"
        fail=$((fail + 1))
    fi
    # ⛔⛔ AND THE PROTECTION IS STILL THERE FOR IT, which is the half that makes
    # the arm above safe to have. Teaching this clause about worktrees must not
    # teach it to hand one over: a worktree that IS the caller is the same danger
    # as a clone that is -- item 792's `rm -rf` does not care which layout it is
    # deleting -- and it gets the same sentence.
    said="$(scratch_guard_refusal '/repo/.git/worktrees/wt' '/scratch/wt' '/repo/.git/worktrees/wt' '/repo/.git/worktrees/wt')"
    case "$said" in
        *"IS the caller"*)
            echo "  ok    a linked worktree that IS the caller is still refused"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  the caller's own worktree said: '$said'"
            fail=$((fail + 1)) ;;
    esac

    # ⚠ Nothing at all is its OWN sentence, not folded into the one above.
    said="$(scratch_guard_refusal '' '' '/repo/.git' '')"
    case "$said" in
        *"no git dir of its own"*)
            echo "  ok    a scratch with no git dir keeps its own words"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  an absent git dir said: '$said'"
            fail=$((fail + 1)) ;;
    esac

    # ⚠ A caller with NO repository must not make every scratch look like it.
    said="$(scratch_guard_refusal '/tmp/s/.git' '/tmp/s' '' '/tmp/s/.git')"
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

    # ⛔⛔⛔⛔⛔ AND THE READER, DRIVEN ON A REAL WORKTREE — register item 1014.
    #
    # The two string arms above are strings: nothing in them says this file can
    # actually FIND a worktree's git dir, and `gitdir: <path>` is a FILE FORMAT
    # rather than something to assume — git writes it absolute or relative
    # depending on how the worktree was added. So a `git worktree` is made here
    # and asked. Without this arm the pure pair would go green over a reader that
    # answers empty for every worktree on earth, which is the old behaviour
    # wearing a new argument.
    #
    # ⛔ THE SCRATCH IS CHECKED IN THE STATEMENT THAT TAKES IT — register item
    # 792, the rule this whole file exists for: `mktemp` exits 127 when it is not
    # on PATH, the variable is then EMPTY, and the `rm -rf` below would be handed
    # `/linked`.
    wt_tmp="$(mktemp -d "${TMPDIR:-/tmp}/scratch-guard-worktree.XXXXXX" 2>/dev/null)" || wt_tmp=""
    if [ -z "$wt_tmp" ] || [ ! -d "$wt_tmp" ]; then
        echo "  FAIL  no scratch directory could be made for the worktree arms"
        fail=$((fail + 2))
    else
        (
            cd "$wt_tmp" || exit 1
            git init -q main || exit 1
            cd main || exit 1
            git config user.email selftest@example.invalid
            git config user.name "scratch-guard selftest"
            printf 'x\n' >file
            git add file
            git commit -q -m "fixture"
            git worktree add -q --detach ../linked HEAD
        ) >/dev/null 2>&1
        wt_own="$(scratch_guard_own_git_dir "$(cd "$wt_tmp/linked" 2>/dev/null && pwd -P || true)")"
        case "$wt_own" in
            */worktrees/linked)
                echo "  ok    a real linked worktree declares its own git dir, and it is found"
                pass=$((pass + 1)) ;;
            *)  echo "  FAIL  a real linked worktree's own git dir read as '$wt_own'"
                fail=$((fail + 1)) ;;
        esac
        # ⚠ THE WHOLE DECISION, not just the reader — this is the sentence that was
        # actually wrong, and the one a harness standing in a worktree acts on.
        said="$(scratch_guard_check "$wt_tmp/linked")"
        case "$said" in
            *"not the scratch directory own"*)
                echo "  FAIL  the live form still calls a real linked worktree an ancestor's"
                fail=$((fail + 1)) ;;
            *)  echo "  ok    the live form does not call a real linked worktree an ancestor's"
                pass=$((pass + 1)) ;;
        esac
        rm -rf "$wt_tmp"
    fi

    # ⛔⛔⛔⛔⛔ THE AMBIENT-ENVIRONMENT ARMS — register item 965, and they are
    # driven as a PURE function for the reason this whole file exists: the
    # dispatch below cuts that environment before any harness in this tree can
    # meet it, so no real run here can produce the case. Handed the case, the
    # decision is measured; asserted around a process that cannot misbehave, it
    # would be the dead control item 792 left behind.
    said="$(scratch_guard_ambient_refusal '')"
    if [ -z "$said" ]; then
        echo "  ok    a process carrying no git environment is not a finding"
        pass=$((pass + 1))
    else
        echo "  FAIL  an empty git environment was refused: $said"
        fail=$((fail + 1))
    fi
    # ⛔ The variable that cost four commits on `main`, by name in the refusal --
    # a sentence that named no variable would send the reader looking for a
    # directory, which is the one thing that is not wrong here.
    said="$(scratch_guard_ambient_refusal 'GIT_INDEX_FILE')"
    case "$said" in
        *GIT_INDEX_FILE*cut\ it*)
            echo "  ok    an inherited index is refused, and the remedy is to cut it"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  an inherited index said: '$said'"
            fail=$((fail + 1)) ;;
    esac
    # ⚠ EVERY name is carried into the sentence, not just the first: a harness
    # told about one of three would cut one of three and meet the next round.
    said="$(scratch_guard_ambient_refusal 'GIT_DIR
GIT_INDEX_FILE GIT_WORK_TREE')"
    case "$said" in
        *GIT_DIR*GIT_INDEX_FILE*GIT_WORK_TREE*)
            echo "  ok    three inherited variables are all named"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  three inherited variables said: '$said'"
            fail=$((fail + 1)) ;;
    esac
    # ⛔⛔ AND THE LIVE PAIR, which is what makes the cut a measurement rather
    # than a claim: a subshell that exports the variable is seen, and the same
    # subshell after `scratch_guard_cut_ambient` is not.
    said="$( export GIT_INDEX_FILE=/nowhere/index
             scratch_guard_ambient_names | tr '\n' ' ' )"
    case "$said" in
        *GIT_INDEX_FILE*)
            echo "  ok    an exported git variable is seen by the live reading"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  an exported git variable was invisible: '$said'"
            fail=$((fail + 1)) ;;
    esac
    said="$( export GIT_INDEX_FILE=/nowhere/index GIT_DIR=/nowhere/.git
             scratch_guard_cut_ambient
             scratch_guard_ambient_names | tr '\n' ' ' )"
    if [ -z "${said// /}" ]; then
        echo "  ok    the cut leaves nothing for a child process to inherit"
        pass=$((pass + 1))
    else
        echo "  FAIL  the cut left '$said' behind"
        fail=$((fail + 1))
    fi
    # ⚠ A variable that is SET but not exported reaches no `git`, so it is not a
    # finding -- otherwise this guard would report on its own local variables.
    said="$( GIT_INDEX_FILE=/nowhere/index
             scratch_guard_ambient_names | tr '\n' ' ' )"
    if [ -z "${said// /}" ]; then
        echo "  ok    an unexported git variable is not a finding"
        pass=$((pass + 1))
    else
        echo "  FAIL  an unexported git variable was refused: '$said'"
        fail=$((fail + 1))
    fi

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
    # ⛔⛔⛔⛔⛔ THE CUT IS HERE AND NOT IN THE FUNCTION — register item 965. This
    # branch is the ONE place a file in this directory is a command rather than a
    # library, so it is the one place where removing the caller's git environment
    # is certainly right. Inside the selftest function it would also run when
    # `pre-commit` sources this file, and a hook stripped of `GIT_INDEX_FILE` is a
    # hook that judges the wrong index — the defect, reflected.
    scratch_guard_cut_ambient
    case "${1:-}" in
        --selftest) scratch_guard_selftest ;;
        --check)    shift; scratch_guard_check "${1:-}" ;;
        *) echo "usage: scratch-guard.sh [--check DIR|--selftest]" >&2
           exit 2 ;;
    esac
fi
