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

# Where the checkout of the index these gates compile is kept.
#
# ⚠ UNDER `target/`, which this repository's TRACKED `.gitignore` excludes, so it is invisible to
# `git status`, to `git ls-files --others` and therefore to `tree-drift.sh`'s fingerprint — the arm
# of that selftest about a gate writing under an ignored path is exactly this case, one directory
# over. It also puts the second `target/` cargo fills on the build-cache volume rather than beside
# the repository, since `target` here is a symlink onto it.
rust_gates_mirror_path() {
    local root
    root="$(git rev-parse --show-toplevel)" || return 1
    printf '%s\n' "$root/target/index-gates"
}

# Where the cleared tree is recorded. Empty output means this clone could not be asked.
rust_gates_stamp_path() {
    local git_dir
    git_dir="$(git rev-parse --git-dir)" || return 1
    printf '%s\n' "$git_dir/sprag-rust-gates-passed"
}

# ── The other two gates a push must not spend its connection on — register item 1005.
#
# ⛔⛔⛔⛔⛔ ITEM 480 MOVED THE RUST GATES OUT AND SAID SO IN GENERAL TERMS: *the hook cannot spend
# an open GitHub connection on a multi-minute gate*. Two gates stayed behind, and the gate that paid
# 480 wrote them down in its own header rather than papering over them — `run_pixel_smoke`, which is
# `cargo build --release` over three crates, and `run_hook_suite`, whose comment measured its SUITE
# at under a second and not the BUILD underneath it, which is minutes on a cold tree.
#
# ⚠⚠ THEY ARE RARER THAN THE RUST GATES AND BITE THE SAME WAY. Each is owed only when a push
# touches the paths it reads, so most pushes never reach them — and the push that does is the one
# that pays 7m15s on an open ssh session and loses the ref after passing (item 456's measurement).
#
# ⚠⚠⚠ EACH STAMPS WHAT IT ACTUALLY READS, which is where this differs from 480 rather than copies
# it. `paths_tree_of` in `content-gate.sh` carries that argument in full.
pixel_smoke_stamp_path() {
    local git_dir
    git_dir="$(git rev-parse --git-dir)" || return 1
    printf '%s\n' "$git_dir/sprag-pixel-smoke-passed"
}

hook_suite_stamp_path() {
    local git_dir
    git_dir="$(git rev-parse --git-dir)" || return 1
    printf '%s\n' "$git_dir/sprag-hook-suite-passed"
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
# ⛔⛔⛔⛔⛔ THE ARITHMETIC AND THE THREE REFUSALS, SPELLED ONCE FOR BOTH LANES — register item 213,
# which is this repository's own measurement that a reason duplicated is a reason that drifts
# (`-D warnings` added to one clippy line and not its twin, for twelve commits). Item 1011 split
# this hook's one routed command into two, and copying this block would have been that defect
# arriving by the shortest road available.
#
#   $1  the reading, in kB, as `[routed]` recorded it — empty where there is none
#   $2  the platform that reading was taken on — empty where the row does not say
#   $3  the lane's name in `[routed]`, which is also the argument to `measure-peak`
rust_gates_bound_from() {
    local routed_kb="$1" routed_platform="$2" lane="$3"
    local host_platform free_kb routed_jobs meminfo routed_decl root
    root="${repo_root:-$(git rev-parse --show-toplevel 2>/dev/null || printf '.')}"
    routed_decl="$root/.claude/remote-build.toml"
    host_platform=$(uname -s 2>/dev/null || printf 'unknown')
    meminfo="${RUST_GATES_MEMINFO:-/proc/meminfo}"
    free_kb=$(awk '/^MemAvailable:/{print $2}' "$meminfo" 2>/dev/null || true)

    if [ -z "$routed_kb" ] || [ "$routed_kb" -le 0 ] 2>/dev/null; then
        # The declaration is there and says nothing about this lane — the original case, unchanged.
        [ -f "$routed_decl" ] && echo "rust-gates: ⚠ no usable reading for the $lane lane in $routed_decl — this hook divides by nothing (register item 932)" >&2
        rust_gates_say_what_spends_instead "$lane" unbound "$host_platform" "$routed_decl"
        return 0
    fi
    if [ -n "$routed_platform" ] && [ "$routed_platform" != "$host_platform" ]; then
        echo "rust-gates: ⚠ the $lane lane's reading was taken on $routed_platform and this host is $host_platform — that number is one platform's memory behaviour, so this hook does not divide by it (register item 1009). Take a reading here: bash crates/sprag-gate/tests/doubles/declared-verify/measure-peak $lane" >&2
        rust_gates_say_what_spends_instead "$lane" unbound "$host_platform" "$routed_decl"
        return 0
    fi
    if [ -z "$free_kb" ]; then
        echo "rust-gates: ⚠ cannot read this host's free memory from $meminfo — the reading in [routed] is fine and it is the HOST that cannot be measured, so this hook divides by nothing (register item 1009)" >&2
        rust_gates_say_what_spends_instead "$lane" unbound "$host_platform" "$routed_decl"
        return 0
    fi
    routed_jobs=$(( free_kb / routed_kb ))
    [ "$routed_jobs" -ge 1 ] || routed_jobs=1
    CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-$routed_jobs}"
    RUST_TEST_THREADS="${RUST_TEST_THREADS:-$routed_jobs}"
    export CARGO_BUILD_JOBS RUST_TEST_THREADS
    echo "rust-gates: the $lane lane peaks at $((routed_kb / 1024))MB/task (declared in [routed], measured on $host_platform); ${free_kb}kB free -> CARGO_BUILD_JOBS=$CARGO_BUILD_JOBS" >&2
    rust_gates_say_what_spends_instead "$lane" bound "$host_platform" "$routed_decl"
}

# ⛔⛔⛔⛔⛔ IS THE WRAPPER IN THE CHAIN — ASKED IN ONE PLACE — register item 213.
#
# Three sites need this question and two of them used to spell it inline. The answer decides what is
# TRUE of a lane this hook did not bound, so a second spelling would be a second thing to keep true
# about the same seam. `rust_gates_say_what_spends_instead` is the third, and it is the one that
# made the duplication matter: it prints a sentence whose truth depends on this answer.
rust_gates_wrapper_present() {
    [ -n "${BX:-}" ] && [ -x "${BX}" ]
}

# ⛔⛔⛔⛔⛔ **WHAT SPENDS THIS LANE'S MEMORY WHEN THIS HOOK DOES NOT** — register item 1010, and the
# sentence three arms above used to get WRONG.
#
# Each arm of `rust_gates_bound_from` used to end *"it is running UNBOUNDED"*. **MEASURED 2026-09-11
# and that is false whenever the wrapper is in the chain**, which is every commit that does not run
# under `env -u BX`:
#
#     bx --local -- bash -c 'echo "SAW ${RUST_TEST_THREADS:-unset}"'   # nothing exported
#     -> bx: local: 25 free core(s) of 32, 21GB available, peak 2GB/task -> RUST_TEST_THREADS=10
#     -> SAW 10
#
# The lane is not unbounded. It is bounded by `peak_gb_per_task` — the workspace scalar this file's
# own header explains it must NOT be bounded by, since this lane peaks at 8.83 GiB and that scalar
# hands out eleven tasks of it. So the arms announced the absence of the very thing that happened,
# which is item 1006's rule (*a refusal naming the wrong cause is worse than none*) reached for the
# third time — and this time by item 1009's own repair.
#
# ⚠⚠ **AND IT IS THE PLATFORM QUESTION, WHICH IS WHY THIS IS ITEM 1010 RATHER THAN A TYPO.** The
# scalar is derived from `[peak_measured]`, whose `platform` field item 1008 added — and MEASURED:
# `grep -c peak_measured ~/.claude/remote-build/bin/bx` answers **0**, so the wrapper never reads
# that table and cannot know whose platform the number describes. Item 932's rule says a row nobody
# divides by is a number in a file; this hook is the only reader this repository can give it.
#
# ⚠ TWO DIRECTIONS, and the wrapper's own source is what separates them — `bin/bx`:
#   * local (1749): `RUST_TEST_THREADS="${RUST_TEST_THREADS:-$lthreads}"` — an exported bound WINS,
#     measured above by exporting 3 and reading 3 back.
#   * remote (2786): `RUST_TEST_THREADS=$threads CARGO_BUILD_JOBS=$threads` — unconditional, and ssh
#     carries no environment across, so a bound this hook set is REPLACED on a build machine.
# So "this hook bounded the lane" is itself only true on one side of that seam, and the bound arm
# says so rather than letting a reader assume the number travels.
#
# ⚠ This changes nothing about what runs. The wrapper's budget is the wrapper's to compute and this
# repository cannot reach it — the fleet half of item 1010 is its owner's. What changes is that the
# sentence is true and names the platform the spent number came from.
#
#   $1  the lane's name in `[routed]`
#   $2  `bound` if this hook exported a budget for it, `unbound` otherwise
#   $3  this host's platform, as `uname -s` answered it
#   $4  the declaration to read the wrapper's divisor out of
rust_gates_say_what_spends_instead() {
    local lane="$1" state="$2" host_platform="$3" routed_decl="$4"
    local peak_gb="" measured_platform=""

    if ! rust_gates_wrapper_present; then
        # No wrapper, no budget: the old sentence, now said only where it is true.
        [ "$state" = unbound ] && echo "rust-gates: ⚠ and nothing else bounds the $lane lane — no wrapper is in the chain, so it is running UNBOUNDED (register item 1010)" >&2
        return 0
    fi
    # ⛔ TESTED BEFORE READ, for the reason this file's header gives at length: `sed` on an absent
    # file exits 2, `pipefail` carries it to the assignment and `set -e` kills the hook at that line
    # in silence. Item 467's throwaway repository has no declaration at all.
    if [ -f "$routed_decl" ]; then
        # ⛔ `[0-9][0-9]*` AND NOT `[0-9]\+` — item 1006: `\+` is a GNU extension and BSD reads it as
        # a literal `+`, which is how a correct declaration came back empty on macOS.
        peak_gb=$(sed -n 's/^peak_gb_per_task = \([0-9][0-9]*\).*/\1/p' "$routed_decl" | head -1)
        # ⚠ `[peak_measured] platform` — the TABLE's one platform, the way `date` and `host` are one
        # for it. A bare `^platform = ` is unambiguous because `[routed]` spells its own per-lane
        # fields `<name>_platform`; `a_declaration_names_one_platform_for_the_wrappers_divisor`
        # holds that there is exactly one such line, so this stays a reading rather than a guess.
        measured_platform=$(sed -n 's/^platform = "\([^"]*\)".*/\1/p' "$routed_decl" | head -1)
    fi
    if [ -z "$peak_gb" ]; then
        echo "rust-gates: ⚠ the wrapper is in the chain and $routed_decl gives it no peak_gb_per_task, so it bounds the $lane lane by free cores alone and nothing bounds its MEMORY (register item 1010)" >&2
        return 0
    fi
    # ⚠⚠ EVERY ARM FROM HERE NAMES THREE THINGS: the scalar, the platform it was measured on, and
    # the platform this host IS. A reader cannot tell whether a figure applies without all three,
    # and the BOUND arm needs them exactly as much as the unbound one does — the wrapper replaces
    # a bound of this hook's own the moment it ships the lane, so a foreign scalar reaches a remote
    # host whether or not this hook managed to divide here.
    local whose="measured on ${measured_platform:-a platform [peak_measured] does not name}, this host is $host_platform"
    if [ -n "$measured_platform" ] && [ "$measured_platform" != "$host_platform" ]; then
        whose="⚠ measured on $measured_platform and this host is $host_platform — the wrapper never reads [peak_measured], so nothing over there can tell"
    fi
    if [ "$state" = bound ]; then
        echo "rust-gates: the wrapper honours that bound when it runs the $lane lane here, and replaces it with peak_gb_per_task = ${peak_gb}GB/task ($whose) when it ships the lane to a build machine (bin/bx:2786 sets it unconditionally; register item 1010)" >&2
        return 0
    fi
    # ⚠ THE WORD «UNBOUNDED» IS NOT SPELLED HERE, IN ANY CASE, and that is deliberate rather than
    # incidental: this is the branch where it would be false, and a sentence that says it only to
    # deny it reads as the old one to anybody scanning. The lane IS bounded — by the wrapper.
    echo "rust-gates: ⚠ so the $lane lane is bounded after all, by the wrapper: peak_gb_per_task = ${peak_gb}GB/task ($whose; register item 1010)" >&2
}

# ⚠⚠ THE TWO THIN READERS BELOW EXIST TO HOLD THE KEY NAMES AS LITERAL TEXT, and that is a
# requirement rather than a style. `every_command_this_repository_hands_the_wrapper_is_one_it_
# measured` asserts that the hook behind each `"${BX}"` call site CONTAINS `<name>_kb = ` and
# `<name>_platform = ` — the key and its `=`, not the bare name — precisely so a hook cannot
# MENTION a reading while dividing by something else. A single reader taking `${lane}_kb` would put
# no such text in this file and the gate would be right to refuse it.

# `[routed] precommit` — clippy and the rustdoc gate, which compile the index checkout.
rust_gates_bound_lint_lane() {
    local kb="" platform="" routed_decl root
    root="${repo_root:-$(git rev-parse --show-toplevel 2>/dev/null || printf '.')}"
    routed_decl="$root/.claude/remote-build.toml"
    if [ -f "$routed_decl" ]; then
        # ⛔ `[0-9][0-9]*` AND NOT `[0-9]\+` -- register item 1006. `\+` is a GNU
        # extension to a basic regular expression; BSD reads it as a LITERAL `+`,
        # so on macOS this matched nothing, the reading came back empty, and the
        # branch above announced *no usable reading* about a file that says
        # exactly what it should. A refusal naming the wrong cause is worse than
        # none. Measured 2026-09-10 with a FreeBSD regex(3) built here: `1234`
        # under GNU, empty under BSD, and `1+` matched instead.
        kb=$(sed -n 's/^precommit_kb = \([0-9][0-9]*\).*/\1/p' "$routed_decl" | head -1)
        # ⚠ The same basic-regex rule as the line above: `[^"]*` and no `\+` anywhere.
        platform=$(sed -n 's/^precommit_platform = "\([^"]*\)".*/\1/p' "$routed_decl" | head -1)
    fi
    rust_gates_bound_from "$kb" "$platform" "precommit"
}

# `[routed] ratchets` — the tests whose input is the tree, which still run in the working tree.
rust_gates_bound_ratchet_lane() {
    local kb="" platform="" routed_decl root
    root="${repo_root:-$(git rev-parse --show-toplevel 2>/dev/null || printf '.')}"
    routed_decl="$root/.claude/remote-build.toml"
    if [ -f "$routed_decl" ]; then
        # ⛔ The same basic-regex rule as its twin above — item 1006.
        kb=$(sed -n 's/^ratchets_kb = \([0-9][0-9]*\).*/\1/p' "$routed_decl" | head -1)
        platform=$(sed -n 's/^ratchets_platform = "\([^"]*\)".*/\1/p' "$routed_decl" | head -1)
    fi
    rust_gates_bound_from "$kb" "$platform" "ratchets"
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
#
# ## ⛔⛔⛔⛔⛔ CLIPPY AND THE RUSTDOC GATE COMPILE THE INDEX, NOT THE WORKING TREE — item 1011
#
# (The ratchet lane below does NOT, and the note on it says why and what that still owes.)
#
# `git commit` takes the INDEX; these gates used to compile the files on disk. Those are two
# different things in this repository more often than not — work here stages by path and reads
# `git diff --cached` precisely because index and working tree diverge (item 196, two writers in
# one tree) — so a commit could pass every gate green and carry bytes NOTHING HAD EVER COMPILED.
#
# ⚠ Not a hypothetical, and not inferred from reading this file. Driven end to end 2026-09-10
# against a real workspace with a real cargo: stage Rust that fails `-D warnings`, leave clean
# Rust on disk, `git commit` → **rc=0**, and `cargo clippy` on the committed tree → **rc=101,
# `error: unused variable`**. Register item 404 had already judged this exact shape for rustfmt
# and moved it onto the staged content; the two larger gates were left behind, which is item
# 213's face — one rule, two spellings, and only one of them fixed.
#
# ⚠⚠ AND IT IS THE SAME FAMILY OF FIX, not a new mechanism: `index_mirror` sits in
# `content-gate.sh` beside the one rustfmt already uses. It differs in two ways, each of which was
# measured rather than reasoned out — it PERSISTS, because a compiler has a cache and rustfmt has
# not, and it is a real WORKING TREE of this repository, because the ratchet lane's tests ask git
# about the tree they are standing in. See that function for both measurements.
#
# ⚠⚠⚠ THE TREE IS WRITTEN OUT BEFORE ANYTHING IS COMPILED AND REMEMBERED, so `rust_gates_stamp`
# records what these gates actually read rather than asking git a second time. The index can move
# under a running hook — item 196 again — and a stamp taken afterwards would name a tree nothing
# had compiled, which is the very defect one paragraph up wearing the push's clothes.
rust_gates_run() {
    local mirror where
    RUST_GATES_COMPILED_TREE=""
    if ! where="$(rust_gates_mirror_path)"; then
        echo "rust-gates: this clone cannot say where its own root is, so the committed bytes cannot be checked out to compile" >&2
        return 1
    fi
    if ! RUST_GATES_COMPILED_TREE="$(git write-tree)"; then
        RUST_GATES_COMPILED_TREE=""
        echo "rust-gates: the index could not be written out as a tree — a gate cannot judge what it cannot read" >&2
        return 1
    fi
    if ! mirror="$(index_mirror "$where" "$RUST_GATES_COMPILED_TREE")"; then
        RUST_GATES_COMPILED_TREE=""
        echo "rust-gates: the index could not be checked out at $where — a gate cannot judge what it cannot read" >&2
        return 1
    fi
    echo "rust-gates: cargo clippy --workspace --all-targets -- -D warnings && doc gate && the ratchet lane ..." >&2
    echo "rust-gates: on the INDEX — tree $RUST_GATES_COMPILED_TREE, checked out at $mirror, which is what this commit will carry" >&2
    # ⚠⚠ A SUBSHELL, so the caller's working directory is untouched and the `"${BX}"` line below
    # stays the one word-for-word command `a_fleet_ceiling_is_a_measurement_with_a_date` composes
    # from this file's own assignments. A `(cd … && "${BX}" … )` one-liner would put a `)` on the
    # end of that argv and the clause reading it could no longer say what the wrapper is given.
    (
        cd "$mirror" || exit 1
        # ⚠ INSIDE THE SUBSHELL, so the bound this exports belongs to this lane and does not leak
        # onto the one below it, which has its own reading and a peak an order of magnitude smaller.
        rust_gates_bound_lint_lane
        if rust_gates_wrapper_present; then
            "${BX}" --label pre-commit-lint -- bash -c "$lint_and_doc"
        else
            bash -c "$lint_and_doc"
        fi
    ) || return 1

    # ⛔⛔⛔⛔⛔ AND THE RATCHET LANE COMPILES THE INDEX TOO, SINCE 2026-09-10 — register item 1014.
    #
    # It stayed on the disk for one round, and the reason was measured rather than assumed: two of
    # its targets are ABOUT THE REPOSITORY THEY STAND IN, and a checkout of the index is a LINKED
    # WORKTREE. `scratch-guard.sh` compared a scratch's git dir against `<dir>/.git`, which is a
    # clone's layout — in a worktree that path is a FILE pointing elsewhere, so every worktree read
    # as *an ancestor repository would answer for it* and the selftest scored 18/19 from inside one
    # against 19/19 outside. Item 1014 taught that guard the second layout; it now scores 23/23 in
    # both, and this lane can stand where the bytes are.
    #
    # ⚠⚠ IT IS STILL A SECOND WRAPPER CALL WITH A ROW OF ITS OWN, and rejoining it to the lane
    # above would be a regression rather than tidying. The two peaks differ SEVENFOLD — 9,260,688 kB
    # against 1,267,736 kB — and one `[routed]` row would bound the tests by the doc gate's peak:
    # 2 jobs where its own reading allows 17. That is the *too HIGH* failure `.claude/remote-build.
    # toml` names beside the swap storm, and it is the reason that file argued for per-command rows
    # in the first place. Same subject now; still two commands, because they cost different things.
    echo "rust-gates: the ratchet lane, on the INDEX as well — register item 1014 ..." >&2
    (
        cd "$mirror" || exit 1
        rust_gates_bound_ratchet_lane
        if rust_gates_wrapper_present; then
            "${BX}" --label pre-commit-ratchets -- bash -c "$ratchets"
        else
            bash -c "$ratchets"
        fi
    )
}

# ⚠⚠⚠⚠⚠ RECORD WHICH TREE THESE GATES CLEARED, so `pre-push` need not run them again on the same
# content — register item 443, raised by the owner asking why a push takes minutes.
#
# MEASURED: the rustdoc gate is **250 s** on a change to a crate the workspace depends on (a separate
# `CARGO_TARGET_DIR` means it re-documents every dependent), and it ran TWICE per change. Clippy by
# then is cached at 0.2 s, so the doubling is entirely this one gate.
#
# ⚠⚠⚠ THE TREE STAMPED IS THE TREE COMPILED, and `rust_gates_run` is the only thing that can say
# what that was. A push whose tip names that exact tree is a push of content these gates read;
# anything else — an amend, a rebase, a second commit, a `--no-verify`, a stamp this clone cannot
# read — will not match.
#
# ⛔⛔⛔⛔⛔ AND THE WITHHOLDING IS GONE BECAUSE ITS REASON IS — register item 1011. This used to
# refuse to stamp whenever the working tree differed from the index over `*.rs`, `Cargo.toml` and
# four other patterns, and the reason it gave was true: *clippy and rustdoc read the WORKING TREE,
# not the index*. They now read the index, so the tree they cleared is the tree the commit carries
# and there is nothing left to withhold.
#
# ⚠⚠ THAT DELETED A WALL AS WELL AS A LIE. `bash .githooks/rust-gates.sh --clear` is the command
# `pre-push` names when it refuses, and it could not clear a tree that had ANY unstaged edit under
# those patterns — which after a commit, in a tree with two writers, is the ordinary state. The
# remedy a refusal named was one the refusing condition forbade.
#
# ⚠⚠⚠ AN UNSET `RUST_GATES_COMPILED_TREE` IS A REFUSAL, NEVER A STAMP OF WHATEVER GIT SAYS NOW.
# Asking `git write-tree` again here would read the index as it stands at THIS instant, and item
# 196 is that a second writer stages into this tree while these gates run — so the stamp would name
# a tree nothing had compiled and hand the push permission to skip on evidence nobody has.
rust_gates_stamp() {
    local stamp
    stamp="$(rust_gates_stamp_path)" || stamp=""
    if [ -n "${RUST_GATES_COMPILED_TREE:-}" ] && [ -n "$stamp" ]; then
        printf '%s\n' "$RUST_GATES_COMPILED_TREE" >"$stamp"
        return 0
    fi
    if [ -n "$stamp" ]; then
        rm -f "$stamp"
    fi
    echo "rust-gates: no tree was compiled in this run, so nothing was stamped and the push will \
ask for these gates again" >&2
    return 1
}

# ── The pixel smoke, run where a connection is not open — register item 1005.
#
# ⚠⚠ THE BODY IS `pre-push`'s, MOVED RATHER THAN REWRITTEN, down to the refusals: a gate that
# quietly steps aside on a machine lacking its tools is absent exactly where nobody notices, and a
# fresh XDG home because the smoke writes user config and a run that read the developer's would be
# asserting about their machine.
pixel_smoke_run() {
    local home
    if ! command -v xvfb-run >/dev/null 2>&1; then
        echo "rust-gates: the pixel smoke needs xvfb-run — install xvfb" >&2
        return 1
    fi
    if ! ls /usr/share/vulkan/icd.d/lvp_icd*.json >/dev/null 2>&1; then
        echo "rust-gates: the pixel smoke needs the lavapipe Vulkan ICD — install \
mesa-vulkan-drivers" >&2
        return 1
    fi
    echo "rust-gates: cargo build --release (gui, host, tui) ..." >&2
    if ! cargo build --release -p sprag-gui -p sprag-host -p sprag-tui; then
        echo "rust-gates: the smoke set did not build" >&2
        return 1
    fi
    # ⛔⛔⛔ THE SCRATCH IS CHECKED WHERE IT IS TAKEN — register item 792.
    home="$(mktemp -d)" || return 1
    mkdir -p "$home/config" "$home/data" "$home/state"
    echo "rust-gates: pixel smoke ..." >&2
    if ! XDG_CONFIG_HOME="$home/config" XDG_DATA_HOME="$home/data" XDG_STATE_HOME="$home/state" \
        xvfb-run -a ./target/release/sprag-smoke; then
        rm -rf "$home"
        echo "rust-gates: the pixel smoke failed — the checks it prints are the diagnosis" >&2
        return 1
    fi
    rm -rf "$home"
}

# ── Record what a path-scoped gate cleared.
#
#   $1  where the stamp goes
#   $2  the line `paths_tree_of` answered, taken BEFORE the gate ran
#
# ⚠⚠⚠ THE READING IS TAKEN BEFORE THE WORK AND HANDED IN, for `rust_gates_stamp`'s reason exactly:
# a second writer stages into this tree while a minute-scale gate runs (item 196), and a reading
# taken afterwards would name content nothing had looked at.
push_gate_stamp() {
    local stamp="$1" cleared="$2"
    if [ -n "$cleared" ] && [ -n "$stamp" ]; then
        printf '%s\n' "$cleared" >"$stamp"
        return 0
    fi
    if [ -n "$stamp" ]; then
        rm -f "$stamp"
    fi
    echo "rust-gates: nothing was read in this run, so nothing was stamped and the push will ask \
for this gate again" >&2
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
            # ⚠ THE MIRROR THESE GATES COMPILE IS `content-gate.sh`'s — register item 1011. A hook
            # has already sourced it; run by a person this file has not, and a `--clear` that fell
            # over an undefined function would be a refusal whose remedy is the command that just
            # failed.
            # shellcheck source-path=SCRIPTDIR
            . "$(dirname "${BASH_SOURCE[0]}")/content-gate.sh"
            rust_gates_run || exit 1
            rust_gates_stamp || exit 1
            echo "rust-gates: cleared, and this tree is stamped — push again." >&2
            ;;
        # ⚠⚠ ONE VERB EACH, AND EACH DOES BOTH IN ORDER — register item 1005, on `--clear`'s own
        # terms. A verb that stamped without running would be the escape hatch that disables the
        # gate it belongs to, which is this workspace's rule 6.
        --clear-pixel)
            cd "$(git rev-parse --show-toplevel)" || exit 1
            # shellcheck source-path=SCRIPTDIR
            . "$(dirname "${BASH_SOURCE[0]}")/content-gate.sh"
            PIXEL_CLEARED="$(paths_tree_of HEAD crates/sprag-gui crates/sprag-grid)" || exit 1
            pixel_smoke_run || exit 1
            push_gate_stamp "$(pixel_smoke_stamp_path)" "$PIXEL_CLEARED" || exit 1
            echo "rust-gates: the pixel smoke cleared what this tree paints — push again." >&2
            ;;
        --clear-hooks)
            cd "$(git rev-parse --show-toplevel)" || exit 1
            # shellcheck source-path=SCRIPTDIR
            . "$(dirname "${BASH_SOURCE[0]}")/content-gate.sh"
            HOOK_CLEARED="$(paths_tree_of HEAD .githooks)" || exit 1
            echo "rust-gates: cargo test -p sprag-gate ..." >&2
            cargo test -p sprag-gate || exit 1
            push_gate_stamp "$(hook_suite_stamp_path)" "$HOOK_CLEARED" || exit 1
            echo "rust-gates: the hook suite cleared these hooks — push again." >&2
            ;;
        *)
            echo "rust-gates.sh runs clippy, the rustdoc gate and the ratchet lane, and stamps the \
tree they cleared so a push need not run them inside its own connection (register item 480). It \
holds the pixel smoke and the hook suite on the same terms (register item 1005)." >&2
            echo "To clear this tree: bash .githooks/rust-gates.sh --clear" >&2
            echo "What this tree paints: bash .githooks/rust-gates.sh --clear-pixel" >&2
            echo "These hooks:           bash .githooks/rust-gates.sh --clear-hooks" >&2
            exit 2
            ;;
    esac
fi
