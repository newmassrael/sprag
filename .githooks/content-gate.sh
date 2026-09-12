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

# What a set of PATHS contains at one revision, as a line a stamp can hold.
#
#   $1    a commit-ish or a tree hash (`git write-tree`'s output does)
#   $2..  the paths, exactly as the caller spells them
#
# ## ⛔⛔⛔⛔⛔ Why a path-scoped tree and not the whole one — register item 1005
#
# Item 480 stamped THE TIP TREE, which is right for gates that compile the
# workspace: change anything and they owe another look. The pixel smoke and the
# hook suite are not those. Each is owed only when a push touches the paths it
# reads (`PIXEL_PATHS`, `HOOK_PATHS`), and a tip-tree stamp would go stale on
# every unrelated commit — so a person would re-run a minute-scale gate to
# publish a change it cannot see.
#
# ⚠⚠ That is not merely wasteful, it is the failure mode item 688 is about
# wearing different clothes: a gate that charges every push is the gate that
# gets waived, and this repository has already paid for one being waived. So
# the stamp is scoped to what the gate actually reads.
#
# ## ⚠⚠ An ABSENT path is a value, not an error
#
# `.githooks` exists in every tree this repository has, but `PIXEL_PATHS` names
# two crates and a tree could carry one and not the other. A missing path prints
# `-`, so *the crate was deleted* and *the crate is unchanged* cannot compare
# equal — which is the direction that costs a re-run rather than a waiver.
#
# Answers nonzero, having printed nothing, if this clone cannot be asked at all.
paths_tree_of() {
    local rev="$1" out="" path hash
    shift
    [ -n "$rev" ] || return 1
    for path in "$@"; do
        hash="$(git rev-parse --verify --quiet "${rev}:${path}")" || hash="-"
        [ -n "$hash" ] || hash="-"
        out="${out}${path}=${hash} "
    done
    [ -n "$out" ] || return 1
    printf '%s\n' "$out"
}

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

# Check the INDEX out at a STABLE path as a WORKING TREE OF THIS REPOSITORY, and print where it
# went. Only the paths that moved since the last call are rewritten.
#
#   $1  where the checkout lives
#   $2  the tree to check out, as `git write-tree` answered it
#
# Answers nonzero, having printed nothing, if the checkout cannot be made or cannot be trusted.
#
# ## ⛔⛔⛔⛔⛔ Why not `content_mirror`, which is right there — register item 1011
#
# Two reasons, and the second was MEASURED after the first draft of this function had already
# been written the other way.
#
# **⑴ It must PERSIST.** A throwaway directory is right for rustfmt, which reads each file once:
# the layout costs 180ms and nothing is carried between runs. The Rust gates COMPILE what they
# are handed, and a compiler's cache is keyed on paths and file timestamps — so a fresh
# temporary directory every commit is a COLD BUILD every commit. Measured on this tree,
# 2026-09-10: **1m17s cold against 11.9s warm**, the working tree's own warm lane being 12.8s.
# That is the whole difference between *the gates judge the right bytes* and *the gates cost six
# minutes*. `git checkout` writes only the paths that differ, so every other file keeps the
# timestamp cargo's freshness check reads. Measured: one file moved in the index, one file's
# mtime moved in the checkout, the other 350 left alone.
#
# **⑵ ⛔⛔⛔ IT MUST BE A REPOSITORY, and a plain directory of files is not one.** The ratchet
# lane runs here too, and the tests in it ask git about the tree they are standing in — item
# 809's whole subject. A first draft laid the bytes out with `read-tree -m -u` into an ordinary
# directory and **two targets went red at the commit that shipped it**: `scratch-guard.sh`'s
# selftest asserts *inside a repository the marker home is an absolute git dir*, and outside one
# there is no such answer. Those arms were right and the mirror was wrong. ⚠ The repair that
# suggests itself — teach the selftest to stand down where there is no repository — is the one
# this workspace refuses on principle: it would disable a gate exactly where nobody is looking.
#
# `git worktree` is the mechanism git already has for *a second checkout of this repository at a
# given state*, and it gives the checkout its OWN index and HEAD, so nothing running in it can
# reach the index the operator is committing (register item 965, which is that failure).
#
# ⚠⚠ THE COMMIT OBJECT IS SCAFFOLDING. `worktree add` and `checkout` take a commit-ish and the
# subject here is a bare tree, so one is written for it. It is unreferenced, never pushed, and
# collected by `git gc` like any other dangling object. Its identity is spelled here rather than
# read from the caller's config: a hook that refused because somebody had not set `user.email`
# would be refusing for a reason that has nothing to do with the commit being made.
#
# ## ⚠⚠ AND THE CHECKOUT IS VERIFIED RATHER THAN TRUSTED
#
# `--force` is what makes a re-checkout of the SAME commit a REPAIR: measured 2026-09-10,
# corrupt a file by hand and the forced checkout puts it back, where the first draft's
# `read-tree -m -u` exited 0 and left the wrong bytes there. The verification stays anyway,
# because a repair nobody checks is a claim: `diff-files` must read clean before the path is
# printed, and anything else is *this could not be laid out* rather than a mirror.
#
# ⚠ WHAT THAT DOES NOT REACH, said rather than hidden: a corruption preserving BOTH the size and
# the timestamp is invisible to git here exactly as it is to `git status` anywhere else. This
# checkout is written by nothing but this function, so the case that has to be caught is an
# interrupted one — and that moves the stat.
index_mirror() {
    local mirror="$1" tree="$2" commit
    if ! commit="$(index_mirror_git -c user.name='sprag index gates' \
        -c user.email='index-gates@invalid' \
        commit-tree "$tree" -m 'the bytes a commit of this index would carry')"; then
        return 1
    fi

    if [ -e "$mirror/.git" ]; then
        index_mirror_git -C "$mirror" checkout --detach --force --quiet "$commit" || return 1
    else
        rm -rf "$mirror"
        # ⚠⚠ A WORKTREE WHOSE DIRECTORY SOMEBODY DELETED STAYS REGISTERED, and `add` then refuses
        # a path it believes it already holds. This lives under `target/`, which is a build cache
        # and the first thing anybody empties when a disk fills up, so that is the ordinary case
        # rather than an exotic one.
        index_mirror_git worktree prune
        index_mirror_git worktree add --detach --quiet "$mirror" "$commit" || return 1
    fi

    index_mirror_git -C "$mirror" diff-files --quiet || return 1

    printf '%s' "$mirror"
}

# `git`, with the environment `git commit` hands its hooks CUT — register item 965, reached by a
# different road.
#
# ⛔⛔⛔⛔⛔ MEASURED BY DRIVING A REAL COMMIT, AND NOTHING IN THIS REPOSITORY'S SUITE COULD HAVE
# FOUND IT. `git commit` exports `GIT_INDEX_FILE` (and `GIT_DIR`) to its hooks, RELATIVE to the
# repository root, and every command above would resolve them against whatever directory it is
# handed. So `worktree add` went looking for `<checkout>/.git/index` — and in a linked worktree
# `.git` is a FILE:
#
#     fatal: .git/index: index file open failed: Not a directory
#
# The gate then refused a commit that was perfectly fine, which is the *fails closed* half working
# and the *is right* half not.
#
# ⚠⚠ THE SUITE WAS BLIND TO THIS BY CONSTRUCTION AND IS NOT ANY MORE — register item 1017.
# `hooks_judge_the_bytes_being_published` used to run every hook through `sprag_gate::ambient::cut`
# and stop there, which removed these variables before the hook started: right, because a sandbox
# must not inherit the OPERATOR's index (item 965), and not enough, because a cut environment is a
# state `git commit` never produces. It now supplies the sandbox's OWN, and deleting the `env -u`
# below takes **8 of its 31 cases** red with the same `fatal: .git/index … Not a directory` a real
# commit gave. What found it originally was driving `git commit` by hand; what holds it now is a
# gate.
#
# ⛔⛔ AND THE LIST WAS A GUESS UNTIL IT WAS MEASURED. This cut `GIT_DIR`, `GIT_WORK_TREE` and
# `GIT_OBJECT_DIRECTORY`, none of which `git commit` sets. Measured 2026-09-10 by committing to a
# scratch repository with a hook that printed its own environment: **seven** variables, and the two
# that name what is being committed are `GIT_INDEX_FILE` and `GIT_PREFIX`. The rest — `GIT_AUTHOR_*`
# and `GIT_EDITOR`/`GIT_EXEC_PATH` — are the identity and the caller's installation, and cutting
# `GIT_EXEC_PATH` in particular would break the very git calls below. So the cut is now the measured
# pair rather than four names somebody thought of.
#
# ⚠⚠⚠ IT CANNOT BE CUT FOR THE WHOLE HOOK, which is why this is a wrapper rather than a line at the
# top of the file: `rust_gates_run` asks `git write-tree`, and THAT call must read exactly the index
# git is about to commit. The same variable is load-bearing three lines up and poison here.
index_mirror_git() {
    ( leave_the_commits_index_behind; git "$@" )
}

# ⛔⛔⛔⛔⛔ **THE PAIR, SPELLED ONCE, AND CUT FOR EVERYTHING THAT RUNS AFTER THIS LINE** — register
# item 1082, and the half of item 1017 a per-call wrapper cannot reach.
#
# # ⛔⛔⛔⛔⛔ Item 1017 built the constructor and item 965 already said what comes next
#
# That item wrapped the hook's OWN git calls, which is right and is not enough: `rust_gates_run`
# then hands work to a CHILD — `"${BX}"`, or the `bash -c` beside it — and a child inherits what the
# wrapper cannot reach. Item 965 wrote the general form of this a month earlier, about the Rust
# layer: *"`git_in` fixes any call site it is applied to, and nothing makes the NEXT call site take
# it."* It answered that with a ratchet over its own text. **The hook layer got the constructor and
# never got the ratchet**, so when item 1014 added two `"${BX}"` call sites inside the mirror they
# took neither — which is item 1082.
#
# # ⛔⛔⛔⛔ Measured 2026-09-12, and the failure this repository MET is the lucky half
#
# A child standing in the mirror — a linked worktree — carrying what a commit exports:
#
#   * **plain commit** (`GIT_INDEX_FILE` is RELATIVE, `.git/index`): `git ls-files` → **rc=128**,
#     `fatal: .git/index: index file open failed: Not a directory`. Loud. This is what refused
#     every routed commit in this tree.
#   * **partial commit** (`git commit -- <pathspec>`, so the variable is ABSOLUTE and names a
#     `next-index-NNN.lock`): the same call → **rc=0**, listing **the operator's index** rather than
#     the tree it is standing in. Measured against a worktree deliberately made to differ: the
#     answer carried a file staged after the mirror was cut and omitted one the mirror holds.
#     **Silent, plausible, and wrong** — item 965's shape, one layer down, and that item's own cost
#     was four contaminated commits reaching `main`.
#   * with this function called first: rc=0 and the mirror's own tree.
#
# ⚠⚠ THE PAIR AND NOT THE NAMESPACE, unlike [`sprag_gate::ambient::is_git_environment`] one layer
# up, and the difference is measured rather than a disagreement: a commit exports **seven** names
# (re-measured 2026-09-12 — `GIT_AUTHOR_DATE`/`_EMAIL`/`_NAME`, `GIT_EDITOR`, `GIT_EXEC_PATH`,
# `GIT_INDEX_FILE`, `GIT_PREFIX`), and only the last two NAME WHAT IS BEING COMMITTED. The Rust
# layer cuts the whole namespace because it builds a sandbox that wants none of them; a hook is
# still the commit and must keep the identity it is committing under.
#
# ⚠⚠⚠ IT CANNOT BE CUT FOR THE WHOLE HOOK — `rust_gates_run` asks `git write-tree`, and THAT call
# must read exactly the index git is about to commit. The same variable is load-bearing at the top
# of that function and poison inside its subshells, which is why this is a boundary a caller crosses
# rather than a line at the top of a file.
leave_the_commits_index_behind() {
    unset GIT_INDEX_FILE GIT_PREFIX
}

# ⛔⛔⛔⛔⛔ **THE ONLY WAY INTO THE MIRROR** — register item 1082, and item 965's constructor
# argument in this layer's own vocabulary.
#
# `cd` and the cut are ONE operation because the second is only needed on account of the first: the
# mirror is not the repository root, so a path a commit exported relative to that root stops being
# true the moment this runs. A caller that could `cd` without cutting is a caller that can forget,
# and the forgetting is silent on exactly the commit shape that matters (see above).
#
# ⚠ `no_hook_enters_the_mirror_without_leaving_the_commits_index_behind` is what makes the next
# caller take it — the ratchet the hook layer did not have.
#
# ⚠⚠ It does NOT run the work: callers keep their own subshell, so the `"${BX}"` lines stay in
# command position word-for-word. `every_command_this_repository_hands_the_wrapper_is_one_it_
# measured` reads those lines and counts a site only when the wrapper leads a command — at the start
# of a line or after `&&`, `||`, `;`, `then`, `else`, `do`, `(` or `{` — so an `env -u … "${BX}" …`
# here would drop both routed lanes out of that population SILENTLY, which is the escape hatch that
# clause exists to close. ⚠ DRIVEN rather than read off its rule: see that clause's own gate arm.
enter_the_mirror() {
    cd "$1" || return 1
    leave_the_commits_index_behind
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
    if ! (enter_the_mirror "$mirror" && rustfmt --edition 2024 --check "$@"); then
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
    if ! (enter_the_mirror "$mirror" && actionlint "$@"); then
        status=1
    fi
    rm -rf "$mirror"
    return "$status"
}

# ⛔⛔⛔⛔⛔ RUN DIRECTLY, THIS FILE SAYS WHAT IT IS — register item 819, and `doc-gate.sh` carries
# the measurement that made this a debt: running a SOURCED library exits 0 having done nothing, and
# a person checking their work by hand reads that as a pass.
#
# ⚠ There is no arm that can be offered here: every function in this file takes a label, a revision
# and a list of paths that only a hook knows. So the answer is the refusal `scratch-guard.sh` has
# always given — a usage line and a non-zero status — and the usage names the functions rather than
# a command, because a command is the thing this file does not have.
if [ "${BASH_SOURCE[0]}" = "${0}" ]; then
    echo "content-gate.sh is a LIBRARY, not a command: it is sourced by pre-commit and pre-push." >&2
    echo "It offers fmt_gate / workflow_gate, each taking <label> <rev> <path>... from a hook," >&2
    echo "and index_mirror <root> <tree>, which lays the committed bytes out for the Rust gates." >&2
    exit 2
fi
