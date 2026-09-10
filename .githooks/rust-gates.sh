#!/usr/bin/env bash
# The Rust gates — clippy, the rustdoc gate and the ratchet lane — and the stamp that records
# which tree they cleared. Shared by `pre-commit`, named by `pre-push`, and runnable by a person.
#
# ## ⛔⛔⛔⛔⛔ Why this is a file and not a block inside `pre-commit` — register item 480
#
# `git push` OPENS THE CONNECTION FIRST and runs `pre-push` after it, so every second the hook
# spends is spent on an open ssh session. Measured 2026-08-20 while paying item 456: the rustdoc
# gate took **7m15s**, GitHub answered *"Connection to github.com closed by remote host"*, and
# `git push` exited **141 (SIGPIPE) after every gate had PASSED** — `git ls-remote origin main`
# confirmed the ref had not moved. A green gate lost the push.
#
# `pre-push` therefore must not RUN these. It reads the stamp this file writes, and where the stamp
# does not cover the tree being pushed it REFUSES and names this file. That is item 480's own
# done-when — *the heavy gates run BEFORE the connection is opened* — and its own warning that
# **a retry loop is not the fix**, because a retry hides which of the three things happened.
#
# ⚠⚠ **AND THAT IS WHY THIS FILE IS RUNNABLE, WHERE `doc-gate.sh` REFUSES TO BE.** A refusal whose
# remedy is not a command is a wall: the stamp is written by `pre-commit`, and after a commit there
# is no `.rs` staged, so nothing a person could type would clear it.
# `bash .githooks/rust-gates.sh --clear` runs the gates and stamps the tree, outside any connection.
# See the dispatch at the foot for why the verb is required.
#
# ⚠⚠⚠ **AND IT MUST NOT BE SPLITTABLE INTO *stamp WITHOUT run*.** A `--stamp` arm would be an
# escape hatch that disables the gate it is part of: anybody could mark a tree cleared that nothing
# had compiled. There is one entry point and it does both, in that order.
#
# ## Why the lane lives here and nowhere else
#
# Two copies of a gate is register item 213, measured in this very repository: `pre-commit` gained
# `-D warnings` in `949dd73` and `pre-push` never did, so the push gate printed findings and exited
# 0 for twelve commits. The commands below are spelled ONCE and both hooks reach them from here.

# ⚠⚠ THE INNER QUOTES ARE SINGLE, AND THAT IS A GATE'S REQUIREMENT RATHER THAN A STYLE.
# `a_fleet_ceiling_is_a_measurement_with_a_date` walks `.githooks/` for `"${BX}"` call sites and
# composes each one's argv from the assignments in the SAME file, reading `.claude/remote-build.toml`
# as TOML basic strings — a `"` inside a value TRUNCATES it there. So the command handed to the
# wrapper carries no double quote, and the two spellings can be the same string. `'-D warnings'` and
# `"-D warnings"` are the same word to sh; the outer quoting is double instead, which is safe here
# because the value holds no `$`, backtick or backslash.
lint_and_doc="cargo clippy --workspace --all-targets -- -D warnings && RUSTDOCFLAGS='-D warnings' CARGO_TARGET_DIR=target/doc-gate cargo doc --workspace --no-deps --document-private-items"

# ⛔⛔⛔⛔⛔ THE RATCHET LANE — register item 784. What runs is the tests whose INPUT IS THE TREE,
# which are exactly the ones an unrelated crate's commit can turn red. `cargo test --workspace` is
# 258.6 s warm against 22.3 s for `sprag-gate` alone (measured 2026-08-31), and item 457 is already
# open against this lane for costing too much per commit.
ratchets='cargo test -p sprag-gate && cargo test -p sprag-rpc --test pins && cargo test -p sprag-tui --test gpu_free && cargo test -p sprag-client --test gpu_free'

# Where the cleared tree is recorded. Empty output means this clone could not be asked.
rust_gates_stamp_path() {
    local git_dir
    git_dir="$(git rev-parse --git-dir)" || return 1
    printf '%s\n' "$git_dir/sprag-rust-gates-passed"
}

# ⛔⛔⛔⛔⛔ THIS LANE CARRIES ITS OWN RAM BOUND, DERIVED FROM ITS OWN READING — register item 932.
#
# The wrapper divides a host's free RAM by `peak_gb_per_task` from `.claude/remote-build.toml`, and
# that scalar is derived from `[commands]` — two commands, neither of them this one. Measured with
# this repository's own instrument (`measure-peak precommit`, cold target directory): this lane
# peaks at **9,260,688 kB, 8.83 GiB**, so a host handed the workspace scalar is given ELEVEN tasks
# of a job that wants nine each.
#
# ⚠⚠ RAISING THE SCALAR IS NOT THE FIX AND THAT WAS MEASURED TOO: it would divide the same RAM into
# two for every command, including `build`. So the reading lives in `[routed]`, where it cannot
# reach the scalar, and the bound is applied HERE — which also covers the branch below where there
# is no wrapper at all and nothing has ever bounded this lane.
#
# ⛔⛔⛔⛔⛔ THE FILE IS TESTED BEFORE IT IS READ, AND `set -euo pipefail` IS WHY. `sprag-gate` links
# these hooks into a THROWAWAY repository and drives them (register item 467), and that repository
# has no `.claude/remote-build.toml`. A first draft read it anyway, with `sed … 2>/dev/null | head`:
# `sed` exits 2 on a file that is not there, `pipefail` makes the PIPELINE fail, the assignment
# carries that status, and `set -e` kills the hook **at that line** with nothing said.
#
# ⚠ A checkout without the declaration is the same case as one without `$BX`: the absence is the old
# behaviour, silently. The warning is for a declaration that IS there and says nothing about this
# lane — the case somebody has to fix (rule 6).
#
# ⛔⛔⛔⛔⛔ AND **THREE DIFFERENT FAILURES SPOKE WITH ONE VOICE, WHICH WAS THE WRONG ONE** — register
# item 1009. Until 2026-09-10 anything that left this lane unbounded printed *no usable
# `precommit_kb` in <file>*, and MEASURED on a host whose `/proc` answers ENOENT to every path, with
# this repository's own declaration in place: the `sed` above extracted `9260688` perfectly, and
# this function announced that the file said nothing. `/proc/meminfo` was what was missing, and the
# sentence blamed the declaration — the exact defect item 1006 is named for eight lines up, *a
# refusal naming the wrong cause is worse than none*, reached by a different road.
#
# ⚠⚠ **AND THE THIRD CAUSE COULD NOT BE SPOKEN AT ALL: the reading is a ONE-PLATFORM number.**
# `precommit_kb` is `VmHWM` sampled out of procfs (item 1008), so it describes the memory behaviour
# of the platform it was taken on and nothing says it describes this host. Reading
# `precommit_platform` beside it is what makes that recorded field a row somebody USES — item 932's
# own rule, that a row nobody divides by is a number in a file, applied to the field item 1008 added.
#
# ⇒ So the arms are named separately and each says which of the three it is. ⚠ None of them changes
# what happens: an unverifiable host runs the lane exactly as it ran it yesterday. What changes is
# that the sentence is true, and a person on that host is pointed at the host rather than at a file
# that is correct.
#
# ⚠ `RUST_GATES_MEMINFO` is the seam the gates drive, and it fails CLOSED: pointing it anywhere that
# does not answer makes this lane UNBOUNDED and loud, and no value of it can raise the job count.
rust_gates_bound_this_lane() {
    local routed_decl routed_kb routed_platform host_platform free_kb routed_jobs root meminfo
    root="${repo_root:-$(git rev-parse --show-toplevel 2>/dev/null || printf '.')}"
    routed_decl="$root/.claude/remote-build.toml"
    routed_kb=""
    routed_platform=""
    if [ -f "$routed_decl" ]; then
        # ⛔ `[0-9][0-9]*` AND NOT `[0-9]\+` -- register item 1006. `\+` is a GNU
        # extension to a basic regular expression; BSD reads it as a LITERAL `+`,
        # so on macOS this matched nothing, `routed_kb` came back empty, and the
        # branch below announced *no usable precommit_kb in the file* about a
        # file that says exactly what it should. A refusal naming the wrong cause
        # is worse than none. Measured 2026-09-10 with a FreeBSD regex(3) built
        # here: `1234` under GNU, empty under BSD, and `1+` matched instead.
        routed_kb=$(sed -n 's/^precommit_kb = \([0-9][0-9]*\).*/\1/p' "$routed_decl" | head -1)
        # ⚠ The same basic-regex rule as the line above: `[^"]*` and no `\+` anywhere.
        routed_platform=$(sed -n 's/^precommit_platform = "\([^"]*\)".*/\1/p' "$routed_decl" | head -1)
    fi
    host_platform=$(uname -s 2>/dev/null || printf 'unknown')
    meminfo="${RUST_GATES_MEMINFO:-/proc/meminfo}"
    free_kb=$(awk '/^MemAvailable:/{print $2}' "$meminfo" 2>/dev/null || true)

    if [ -z "$routed_kb" ] || [ "$routed_kb" -le 0 ] 2>/dev/null; then
        # The declaration is there and says nothing about this lane — the original case, unchanged.
        [ -f "$routed_decl" ] && echo "rust-gates: ⚠ no usable precommit_kb in $routed_decl — this lane is running UNBOUNDED (register item 932)" >&2
        return 0
    fi
    if [ -n "$routed_platform" ] && [ "$routed_platform" != "$host_platform" ]; then
        echo "rust-gates: ⚠ precommit_kb was measured on $routed_platform and this host is $host_platform — that number is one platform's memory behaviour, so this lane is running UNBOUNDED rather than bounded by somebody else's peak (register item 1009). Take a reading here: bash crates/sprag-gate/tests/doubles/declared-verify/measure-peak precommit" >&2
        return 0
    fi
    if [ -z "$free_kb" ]; then
        echo "rust-gates: ⚠ cannot read this host's free memory from $meminfo — the reading in [routed] is fine and it is the HOST that cannot be measured, so this lane is running UNBOUNDED (register item 1009)" >&2
        return 0
    fi
    routed_jobs=$(( free_kb / routed_kb ))
    [ "$routed_jobs" -ge 1 ] || routed_jobs=1
    CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-$routed_jobs}"
    RUST_TEST_THREADS="${RUST_TEST_THREADS:-$routed_jobs}"
    export CARGO_BUILD_JOBS RUST_TEST_THREADS
    echo "rust-gates: this lane peaks at $((routed_kb / 1024))MB/task (declared in [routed], measured on $host_platform); ${free_kb}kB free -> CARGO_BUILD_JOBS=$CARGO_BUILD_JOBS" >&2
}

# Run clippy, the rustdoc gate and the ratchet lane, routed through the build-machine wrapper when
# one is configured.
#
# ⚠⚠ AND THE WRAPPER'S OWN VERDICT IS FINAL — THERE IS NO SECOND FALLBACK, and an earlier version
# of this block had one that did real harm. It read "the wrapper printed no `bx: exit=` line" as
# "infrastructure went away" and re-ran the pair locally — but that line is absent for EVERY reason
# the wrapper declines to start, all of which are the author's to fix. Measured 2026-08-20: six
# consecutive commits took that path while the box sat at load 13-25 with 11GB in swap.
#
# THE WRAPPER ALREADY FAILS OPEN BY ITSELF: when no build machine is a candidate and this one can
# hold the work, it runs it here and reports `where=local`. It refuses only when it cannot proceed
# safely — and a refusal is a verdict, not an outage.
#
# ⚠ THE TWO TRAVEL TOGETHER so the tree is synced once rather than twice, and `&&` keeps clippy's
# fail-fast: a clippy failure never reaches the doc gate.
rust_gates_run() {
    rust_gates_bound_this_lane
    echo "rust-gates: cargo clippy --workspace --all-targets -- -D warnings && doc gate && the ratchet lane ..." >&2
    if [ -n "${BX:-}" ] && [ -x "${BX}" ]; then
        "${BX}" --label pre-commit-lint -- bash -c "$lint_and_doc && $ratchets"
    else
        bash -c "$lint_and_doc && $ratchets"
    fi
}

# ⚠⚠⚠⚠⚠ RECORD WHICH TREE THESE GATES CLEARED, so `pre-push` need not run them again on the same
# content — register item 443, raised by the owner asking why a push takes minutes.
#
# MEASURED: the rustdoc gate is **250 s** on a change to a crate the workspace depends on (a separate
# `CARGO_TARGET_DIR` means it re-documents every dependent), and it ran TWICE per change. Clippy by
# then is cached at 0.2 s, so the doubling is entirely this one gate.
#
# ⚠⚠⚠ `git write-tree` IS THE TREE THIS COMMIT WILL CARRY: the index, written as an object. A push
# whose tip names that exact tree is a push of content these gates read; anything else — an amend, a
# rebase, a second commit, a `--no-verify`, a stamp this clone cannot read — will not match.
#
# ⚠⚠⚠⚠ AND THE STAMP IS WITHHELD UNLESS THE WORKING TREE IS THE INDEX for everything these gates
# compile. Clippy and rustdoc read the WORKING TREE, not the index, so an unstaged edit or an
# untracked module means what they judged is not what the commit carries — and skipping on that
# basis would publish a tree nothing had ever compiled.
#
# ⚠⚠ EVERY QUERY THAT FAILS READS AS *not clean*, spelled as a word rather than left empty: an
# unreadable index is exactly the case item 404 measured passing as *nothing to check*, and here it
# would hand a push permission to skip on evidence nobody has.
rust_gates_stamp() {
    local gate_paths unstaged untracked staged_tree stamp
    gate_paths='*.rs *.scxml Cargo.toml Cargo.lock build.rs rust-toolchain.toml'
    # shellcheck disable=SC2086  # intentional word-split of the pathspec list
    unstaged="$(git diff --name-only -- $gate_paths)" || unstaged="unreadable"
    # shellcheck disable=SC2086  # intentional word-split of the pathspec list
    untracked="$(git ls-files --others --exclude-standard -- $gate_paths)" || untracked="unreadable"
    staged_tree="$(git write-tree)" || staged_tree=""
    stamp="$(rust_gates_stamp_path)" || stamp=""
    if [ -z "$unstaged" ] && [ -z "$untracked" ] && [ -n "$staged_tree" ] && [ -n "$stamp" ]; then
        printf '%s\n' "$staged_tree" >"$stamp"
        return 0
    fi
    if [ -n "$stamp" ]; then
        rm -f "$stamp"
    fi
    echo "rust-gates: the working tree is not what the index carries, so no tree was stamped and \
the push will ask for these gates again" >&2
    return 1
}

# ⛔⛔⛔⛔⛔ RUN DIRECTLY, THIS FILE DOES THE WORK — but only when it is ASKED BY NAME, and the
# reason a bare invocation must not do it was found by a neighbouring gate rather than reasoned out.
#
# `no_hook_library_run_with_no_arguments_is_silently_successful` (register item 819) runs every
# `.githooks/*.sh` **bare** and requires it not to exit 0 in silence. A first draft of this file did
# the work on a bare invocation — which would have made that gate run the ratchet lane, and the
# ratchet lane is `cargo test -p sprag-gate`, which is the suite that gate is IN. The library would
# have driven itself, from inside itself, for as long as anybody let it.
#
# ⇒ So the dispatch answers two different questions. Bare: say what this file is, non-zero, in the
# shape item 819 asks for. Asked by name: clear the gates and stamp the tree.
#
# ⚠⚠ **THERE IS NO `--stamp`, AND THERE MUST NOT BE.** A verb that marked a tree cleared without
# compiling it would be an escape hatch that disables the gate it belongs to — this workspace's
# rule 6, and the reason `pre-push` can trust the stamp at all. One verb; it does both, in order.
if [ "${BASH_SOURCE[0]}" = "${0}" ]; then
    set -euo pipefail
    case "${1:-}" in
        --clear)
            cd "$(git rev-parse --show-toplevel)" || exit 1
            rust_gates_run || exit 1
            rust_gates_stamp || exit 1
            echo "rust-gates: cleared, and this tree is stamped — push again." >&2
            ;;
        *)
            echo "rust-gates.sh runs clippy, the rustdoc gate and the ratchet lane, and stamps the \
tree they cleared so a push need not run them inside its own connection (register item 480)." >&2
            echo "To clear this tree: bash .githooks/rust-gates.sh --clear" >&2
            exit 2
            ;;
    esac
fi
