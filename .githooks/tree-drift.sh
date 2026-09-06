#!/usr/bin/env bash
# WHETHER THE WORKING TREE CHANGED WHILE A HOOK WAS RUNNING -- register item 925.
#
# ⛔⛔⛔⛔⛔ WHY THIS EXISTS, AND WHAT IT COST TO LEARN
#
# `pre-commit` and `pre-push` compile THE WORKING TREE, not the index: `cargo clippy
# --workspace`, the rustdoc gate and the ratchet lane all read the files on disk. Those
# gates take six to ten minutes, and a session that has just typed `git commit` wants to
# get on with the next piece of work -- so it edits, and the hook is compiling what the
# edit is halfway through.
#
# The failure that follows is INDISTINGUISHABLE FROM A REAL DEFECT, because it is a real
# compile error, correctly reported, about code that really was on disk. Measured on
# 2026-09-06, paying item 843:
#
#   pre-push: the ratchet lane (tests whose input is the tree) ...
#   error[E0599]: no variant named `UnrunnableRed` found for enum `north_star::Fault`
#   pre-push: a ratchet that reads the tree is red -- fix it before push
#
# The tree at that instant held a line USING a variant that had not been written yet. The
# hook was right. The push was lost, `origin/main` never moved, and NOTHING ANYWHERE SAID
# the cause was the author's own concurrent editing rather than the change being pushed.
# ⚠ The wrapper's completion notice said `exit code 0`; only the `.rc` file and
# `git ls-remote` disagreed.
#
# ⚠⚠⚠⚠⚠ IT SAYS, IT DOES NOT BLOCK -- AND THAT IS THE DESIGN, NOT A WEAKER VERSION OF IT
#
# Refusing to run while the tree moves would be a LOCK, and register item 196 is that this
# working tree has TWO WRITERS: the debt-repayment loop commits into it unattended, as the
# owner. A hook that refuses on someone else's edit would stand that session's round down
# for the duration of this one's gates. The purpose here is DIAGNOSIS -- to hand the reader
# the one fact they cannot recover afterwards -- and a diagnosis that stops the machine is
# a different, worse tool.
#
# ⚠⚠⚠⚠ IT REPORTS ON EVERY EXIT PATH, VIA `trap ... EXIT`, WHICH IS THE WHOLE POINT
#
# The case this is for is a hook that FAILED. Every one of those paths is an `exit 1`
# somewhere in the middle of the hook, so a report written after the last gate would be the
# one report that never printed. The trap also preserves the hook's own status: it saves
# `$?` first and re-exits with it, so installing this can never turn a red into a green.
#
# ⚠⚠⚠ TWO MECHANISMS, BECAUSE EACH MISSES WHAT THE OTHER CATCHES
#
#   * CONTENT (`git hash-object`) catches any edit that changed bytes, whatever happened to
#     the timestamps -- including a file restored from elsewhere with its mtime preserved.
#   * TIMESTAMP (`test -nt` against a marker made at the start) catches an edit that was
#     UNDONE before the hook finished. The bytes are identical at both ends and the content
#     mechanism sees nothing, but the gates compiled the intermediate state, and that is
#     exactly the shape of "I did the next piece and then thought better of it".
#
# A path is reported if EITHER says so. ⚠ The timestamp arm subtracts a BASELINE taken at
# the start rather than comparing against the marker directly, so a file that already
# carried a future mtime -- a skewed clock, an archive restored with its own times -- is in
# both sets and cancels out instead of crying wolf on every run forever.
#
# ⚠⚠ WHAT IT CANNOT SEE, stated so a quiet run is not misread: an edit that restores BOTH
# the bytes and the timestamp is invisible here, and so is a change under a path
# `.gitignore` excludes. The second is deliberate -- `target/` is where these gates
# themselves write, and a fingerprint that included it would report drift on every single
# run, which is the one failure that would make this line worth ignoring.
#
# Self-tested: `bash .githooks/tree-drift.sh --selftest`, run by `crates/sprag-gate/`
# without anybody adding it to a list (register item 799).
set -uo pipefail

# ── THE MEASUREMENT. Writes `.paths`, `.hash` and `.newer` under the prefix given as `$3`.
#
# ⚠⚠⚠ IT REFUSES RATHER THAN ANSWERING EMPTY. A tree it cannot enumerate, a path list it
# cannot round-trip, an enumeration that finds NOTHING -- each returns non-zero with a
# reason on stderr. An empty population that read as "no drift" is register item 924's
# defect exactly: a gate that is green because it looked at nothing.
tree_drift_measure() {
    local root="${1:?tree_drift_measure needs a repository root}"
    local marker="${2:?tree_drift_measure needs a marker file}"
    local dest="${3:?tree_drift_measure needs a destination prefix}"
    local count_z count_lines path

    if ! git -C "$root" rev-parse --show-toplevel >/dev/null 2>&1; then
        echo "$root is not a git repository, so its tree cannot be fingerprinted" >&2
        return 1
    fi
    if ! git -C "$root" ls-files -z --cached --others --exclude-standard >"$dest.z" 2>/dev/null; then
        echo "git could not enumerate the tree at $root" >&2
        return 1
    fi

    # ⚠⚠ A PATH HOLDING A NEWLINE WOULD SILENTLY SPLIT INTO TWO, and every comparison below
    # is line-oriented. The two counts are compared rather than trusted, and a mismatch is
    # an unknown -- which this file reports as an unknown, never as a clean tree.
    count_z="$(tr -dc '\0' <"$dest.z" | wc -c | tr -d ' ')"
    tr '\0' '\n' <"$dest.z" >"$dest.paths"
    count_lines="$(wc -l <"$dest.paths" | tr -d ' ')"
    rm -f "$dest.z"
    if [ "$count_z" -ne "$count_lines" ]; then
        echo "a tracked path contains a newline ($count_z entries became $count_lines lines), so this fingerprint cannot be trusted" >&2
        return 1
    fi
    if [ "$count_lines" -eq 0 ]; then
        echo "the tree at $root enumerated no files at all -- a fingerprint of nothing cannot report drift" >&2
        return 1
    fi

    # ── CONTENT, and TIMESTAMP, in one pass of the path list.
    #
    # ⚠ `-nt` is a bash builtin, so this loop forks nothing: three hundred `stat` calls
    # would cost more than every gate this file reports on. It is also the portable
    # answer -- `stat -c` is GNU and this repository's hooks run on macOS too, where the
    # same flag means something else entirely.
    : >"$dest.newer"
    : >"$dest.exists"
    : >"$dest.hash"
    while IFS= read -r path; do
        if [ -e "$root/$path" ]; then
            printf '%s\n' "$path" >>"$dest.exists"
            if [ "$root/$path" -nt "$marker" ]; then
                printf '%s\n' "$path" >>"$dest.newer"
            fi
        else
            printf 'ABSENT %s\n' "$path" >>"$dest.hash"
        fi
    done <"$dest.paths"

    if [ -s "$dest.exists" ]; then
        # ⚠⚠ ONE `git hash-object` FOR THE WHOLE LIST, not one per file. `--stdin-paths`
        # answers in the order it was asked, which is what lets `paste` re-attach the names.
        if ! (cd "$root" && git hash-object --stdin-paths <"$dest.exists") >"$dest.blobs" 2>/dev/null; then
            echo "git could not hash the working-tree files under $root" >&2
            return 1
        fi
        if [ "$(wc -l <"$dest.blobs" | tr -d ' ')" -ne "$(wc -l <"$dest.exists" | tr -d ' ')" ]; then
            echo "git answered a different number of hashes than it was given paths" >&2
            return 1
        fi
        paste -d' ' "$dest.blobs" "$dest.exists" >>"$dest.hash"
    fi

    LC_ALL=C sort -o "$dest.hash" "$dest.hash"
    LC_ALL=C sort -o "$dest.newer" "$dest.newer"
    return 0
}

# ── TAKE THE FIRST READING AND ARM THE REPORT. Called by a hook, once, near the top.
#
# ⚠⚠⚠ THE MARKER IS MADE BEFORE THE FINGERPRINT, so an edit landing between the two is
# caught by the timestamp arm rather than baked into the baseline. That errs towards saying
# "this may have moved", which is the direction a diagnostic should err in.
tree_drift_begin() {
    # ⚠⚠⚠ NO APOSTROPHE IN THIS MESSAGE, and that is a fix rather than a style. Bash treats
    # a single quote inside `${...}` as QUOTING even when the whole expansion is already
    # inside double quotes, so `the hook's name` opened a quote that swallowed the rest of
    # this file: every function below it silently stopped being defined, and `bash -n`
    # stayed green because the text re-balanced at the next apostrophe. Measured while
    # writing this file -- `declare -F` listed two functions out of seven.
    TREE_DRIFT_HOOK="${1:?tree_drift_begin needs the name of the calling hook}"
    TREE_DRIFT_ROOT="${2:-$(git rev-parse --show-toplevel 2>/dev/null || echo .)}"
    TREE_DRIFT_WHY=""
    TREE_DRIFT_DIR=""
    TREE_DRIFT_MARK=""

    # ⛔⛔⛔ THE STATUS IS READ IN THE SAME STATEMENT — register item 792, and the spelling
    # `hooks_cannot_pass_in_silence` requires rather than one that merely works. `mktemp`
    # exits 127 when it is not on PATH, the variable is then the empty string, and this
    # file would go on to hand `git -C ""` the CALLER's own repository. ⚠ Written as `|| `
    # and not as `if ! x="$(mktemp …)"`: the two are equally safe here, but the gate scans
    # for the assignment-with-`||`, and conforming is cheaper than teaching it a synonym.
    TREE_DRIFT_DIR="$(mktemp -d "${TMPDIR:-/tmp}/sprag-tree-drift.XXXXXX" 2>/dev/null)" || TREE_DRIFT_DIR=""
    if [ -z "$TREE_DRIFT_DIR" ] || [ ! -d "$TREE_DRIFT_DIR" ]; then
        TREE_DRIFT_DIR=""
        TREE_DRIFT_WHY="no scratch directory could be made to hold the fingerprint"
    else
        TREE_DRIFT_MARK="$TREE_DRIFT_DIR/mark"
        : >"$TREE_DRIFT_MARK"
        if ! TREE_DRIFT_WHY="$(tree_drift_measure "$TREE_DRIFT_ROOT" "$TREE_DRIFT_MARK" "$TREE_DRIFT_DIR/before" 2>&1 >/dev/null)"; then
            : "${TREE_DRIFT_WHY:=the tree could not be fingerprinted}"
        else
            TREE_DRIFT_WHY=""
        fi
    fi

    # ⚠ INSTALLED EVEN WHEN THE FIRST READING FAILED, because "this hook cannot tell you"
    # is itself the report. Silence would be indistinguishable from "the tree held still".
    # ⛔⛔⛔⛔ AND THIS ONE TRAP IS ENOUGH FOR THE INTERRUPTED RUN TOO, WHICH WAS MEASURED
    # RATHER THAN ASSUMED. These gates take six to ten minutes — the whole reason this file
    # exists is that a person waiting that long goes and does something else — so Ctrl-C is
    # a likely ending, not an edge case, and an ending that reported nothing and left a
    # scratch directory behind would be feeding register item 927 (13,796 directories and
    # 19.9 GB under `/tmp`, counted by no gate).
    #
    # ⚠⚠⚠ A FIRST VERSION ADDED `trap 'exit 130' INT` AND ITS TWO SIBLINGS, AND THEY WERE
    # DEAD CODE. Removing all three left the arm below GREEN, which is this workspace's
    # signal that a mutation is not reaching the fixture — so the question was measured
    # instead: bash runs a set EXIT trap when it dies of an untrapped INT, TERM or HUP
    # (2026-09-06, all three). The three lines bought nothing, and a gate that cannot go red
    # for them is worse than their absence. ⚠ SIGKILL is catchable by nothing and is the one
    # ending that still leaks; said here rather than left to be found.
    trap 'tree_drift_at_exit' EXIT
}

# ── THE TRAP BODY. Preserves the hook's own exit status, always.
tree_drift_at_exit() {
    local status=$?
    trap - EXIT
    tree_drift_report "${TREE_DRIFT_HOOK:-hook}"
    exit "$status"
}

# ── THE SECOND READING, AND THE VERDICT.
tree_drift_report() {
    local hook="${1:?tree_drift_report needs the name of the calling hook}"
    local why moved fingerprinted

    if [ -z "${TREE_DRIFT_DIR:-}" ] || [ -n "${TREE_DRIFT_WHY:-}" ]; then
        printf '%s: ⚠ the tree fingerprint could not be taken (%s) — so this hook cannot say whether the working tree was edited while it ran\n' \
            "$hook" "${TREE_DRIFT_WHY:-no reading was started}" >&2
        if [ -n "${TREE_DRIFT_DIR:-}" ]; then
            rm -rf "$TREE_DRIFT_DIR"
        fi
        TREE_DRIFT_DIR=""
        return 0
    fi

    if ! why="$(tree_drift_measure "$TREE_DRIFT_ROOT" "$TREE_DRIFT_MARK" "$TREE_DRIFT_DIR/after" 2>&1 >/dev/null)"; then
        printf '%s: ⚠ the tree could not be re-read at the end (%s) — so this hook cannot say whether it was edited while it ran\n' \
            "$hook" "$why" >&2
        rm -rf "$TREE_DRIFT_DIR"
        TREE_DRIFT_DIR=""
        return 0
    fi

    # A path is reported when its CONTENT differs between the two readings, or when it
    # became newer than the marker during the run. `comm -13` on the two `newer` sets is
    # the baseline subtraction: what was already future-dated at the start drops out.
    moved="$(
        {
            LC_ALL=C comm -3 "$TREE_DRIFT_DIR/before.hash" "$TREE_DRIFT_DIR/after.hash" |
                sed -e 's/^[[:space:]]*//' -e 's/^[^ ]* //'
            LC_ALL=C comm -13 "$TREE_DRIFT_DIR/before.newer" "$TREE_DRIFT_DIR/after.newer"
        } | LC_ALL=C sort -u
    )"
    fingerprinted="$(wc -l <"$TREE_DRIFT_DIR/after.paths" | tr -d ' ')"

    if [ -n "$moved" ]; then
        printf '%s: ⚠⚠ THE WORKING TREE CHANGED WHILE THIS HOOK RAN — %s of %s fingerprinted path(s):\n' \
            "$hook" "$(printf '%s\n' "$moved" | wc -l | tr -d ' ')" "$fingerprinted" >&2
        printf '%s\n' "$moved" | sed "s|^|$hook:     |" >&2
        printf '%s: ⚠⚠ these gates compile the WORKING TREE and not the index, so a failure above may be about an edit made while they ran rather than about what you are committing. Re-run on a still tree to tell the two apart.\n' \
            "$hook" >&2
    else
        printf '%s: the working tree held still while this hook ran (%s path(s) fingerprinted)\n' \
            "$hook" "$fingerprinted" >&2
    fi

    rm -rf "$TREE_DRIFT_DIR"
    TREE_DRIFT_DIR=""
    return 0
}

# ══ THE SELFTEST ══════════════════════════════════════════════════════════════════════
#
# ⛔⛔⛔⛔⛔ EVERY ARM DRIVES THE REAL EXIT PATH. A scenario runs `tree_drift_begin` in a
# SUBSHELL and then lets the subshell end, so what is being read is what a hook would
# print -- through the `trap ... EXIT` and not by calling the reporter by hand. An arm that
# invoked `tree_drift_report` directly would leave the one mechanism this file adds
# (reporting from inside a failing hook) untested, which is register item 799's whole
# lesson: a gate that does not run the thing is green about something else.
#
# ⚠ The arms that MUST see a report are mutations in the sense this workspace means: each
# one changes the tree in one specific way and fails if the line does NOT appear. Item 925
# asks for exactly that -- "훅 중간에 파일을 건드리면 그 줄이 «안» 나오면 red".

# The clock has to have MOVED past the marker, or an arm about "edited after the start"
# would be asserting against a timestamp equal to its own baseline.
#
# ⚠⚠ MEASURED RATHER THAN SLEPT: this bash compares `-nt` with sub-second precision, but a
# platform that compares whole seconds would turn the edit-and-revert arm into a flake
# that passes on the author's machine. Spinning until the filesystem actually reports a
# newer stamp is bounded by one tick of whatever granularity is in force, and is correct
# on both. A `sleep 1` would be a guess that costs a second every run.
tree_drift_tick_past() {
    local marker="${1:?tree_drift_tick_past needs the marker}"
    local probe="${marker}.tick" spins=0
    while :; do
        : >"$probe"
        if [ "$probe" -nt "$marker" ]; then
            break
        fi
        spins=$((spins + 1))
        if [ "$spins" -gt 100000 ]; then
            rm -f "$probe"
            return 1
        fi
    done
    rm -f "$probe"
}

# Run one scenario and print what a hook's reader would see. `$1` is the repository, `$2`
# the status the body exits with, and the rest is the body -- run BETWEEN the two readings,
# which is where a session's stray edit lands.
tree_drift_scenario() {
    local repo="$1" want="$2"
    shift 2
    (
        tree_drift_begin "probe" "$repo"
        tree_drift_tick_past "$TREE_DRIFT_MARK" || true
        "$@"
        exit "$want"
    ) 2>&1
}

tree_drift_selftest() {
    local pass=0 fail=0 tmp repo out status scratch_refusal

    # ⛔ THE SAME STATEMENT, for the same reason — item 792. Every arm below WRITES into
    # what this variable names, so an empty one would edit and delete in the caller's tree.
    tmp="$(mktemp -d "${TMPDIR:-/tmp}/tree-drift-selftest.XXXXXX")" || tmp=""
    if [ -z "$tmp" ] || [ ! -d "$tmp" ]; then
        echo "tree-drift selftest: no scratch directory could be made" >&2
        return 1
    fi
    repo="$tmp/repo"
    mkdir -p "$repo"
    (
        cd "$repo" || exit 1
        git init -q .
        git config user.email selftest@example.invalid
        git config user.name "tree-drift selftest"
        printf 'target/\n' >.gitignore
        mkdir -p src target
        printf 'fn main() {}\n' >src/main.rs
        printf 'keep\n' >src/other.rs
        git add .gitignore src
        git commit -q -m "selftest fixture"
    ) >/dev/null 2>&1

    # ⛔⛔⛔⛔⛔ THE SUBJECT IS THE SCRATCH REPOSITORY OR THIS DOES NOT RUN AT ALL — register
    # item 792, whose selftest re-initialised the REAL repository and replaced the
    # operator's git identity. The arms below WRITE into whatever `$repo` names.
    scratch_refusal="$(scratch_guard_check "$repo" 2>/dev/null || true)"
    if [ -n "$scratch_refusal" ]; then
        echo "tree-drift selftest: REFUSING to run -- ${scratch_refusal}, so every arm would" \
             "edit and delete files in $(git rev-parse --absolute-git-dir 2>/dev/null \
                || echo "some other repository") instead" >&2
        rm -rf "$tmp"
        return 1
    fi

    # ⑴ A STILL TREE SAYS SO, and says how much it looked at.
    out="$(tree_drift_scenario "$repo" 0 true)"
    if printf '%s' "$out" | command grep -q 'held still'; then
        if printf '%s' "$out" | command grep -qE '\(3 path\(s\) fingerprinted\)'; then
            echo "  ok    a still tree says it held still, and names the population it read"
            pass=$((pass + 1))
        else
            echo "  FAIL  a still tree said so without saying how many paths it read: $out"
            fail=$((fail + 1))
        fi
    else
        echo "  FAIL  a still tree did not report holding still: $out"
        fail=$((fail + 1))
    fi

    # ⑵ ⛔ THE MUTATION ITEM 925 ASKS FOR: an edit landing mid-run is REPORTED.
    out="$(tree_drift_scenario "$repo" 1 bash -c "printf 'fn main() { changed }\n' >'$repo/src/main.rs'")"
    if printf '%s' "$out" | command grep -q 'THE WORKING TREE CHANGED WHILE THIS HOOK RAN' &&
        printf '%s' "$out" | command grep -q 'src/main.rs'; then
        echo "  ok    a file edited while the hook ran is reported, by name"
        pass=$((pass + 1))
    else
        echo "  FAIL  an edit made during the run was NOT reported: $out"
        fail=$((fail + 1))
    fi
    git -C "$repo" checkout -q -- src/main.rs

    # ⑶ A FILE CREATED mid-run is reported — the untracked half of the population.
    out="$(tree_drift_scenario "$repo" 0 bash -c "printf 'new\n' >'$repo/src/added.rs'")"
    if printf '%s' "$out" | command grep -q 'THE WORKING TREE CHANGED' &&
        printf '%s' "$out" | command grep -q 'src/added.rs'; then
        echo "  ok    a file created while the hook ran is reported"
        pass=$((pass + 1))
    else
        echo "  FAIL  a file created during the run was NOT reported: $out"
        fail=$((fail + 1))
    fi
    rm -f "$repo/src/added.rs"

    # ⑷ A FILE DELETED mid-run is reported — the `ABSENT` half of the fingerprint.
    out="$(tree_drift_scenario "$repo" 0 rm -f "$repo/src/other.rs")"
    if printf '%s' "$out" | command grep -q 'THE WORKING TREE CHANGED' &&
        printf '%s' "$out" | command grep -q 'src/other.rs'; then
        echo "  ok    a file deleted while the hook ran is reported"
        pass=$((pass + 1))
    else
        echo "  FAIL  a file deleted during the run was NOT reported: $out"
        fail=$((fail + 1))
    fi
    git -C "$repo" checkout -q -- src/other.rs

    # ⑸ ⛔ THE SECOND MUTATION, and the one that proves the TIMESTAMP arm is load-bearing:
    # an edit UNDONE before the hook finished. The bytes are identical at both readings, so
    # a content-only fingerprint reports nothing — while the gates compiled the middle.
    out="$(tree_drift_scenario "$repo" 0 bash -c "
        printf 'fn main() { transient }\n' >'$repo/src/main.rs'
        printf 'fn main() {}\n' >'$repo/src/main.rs'")"
    if printf '%s' "$out" | command grep -q 'THE WORKING TREE CHANGED' &&
        printf '%s' "$out" | command grep -q 'src/main.rs'; then
        echo "  ok    an edit undone before the end is still reported (the timestamp arm)"
        pass=$((pass + 1))
    else
        echo "  FAIL  an edit-and-revert during the run was NOT reported: $out"
        fail=$((fail + 1))
    fi
    git -C "$repo" checkout -q -- src/main.rs

    # ⑹ ⛔⛔ AND THE ARM THAT KEEPS THIS LINE WORTH READING: a write under an IGNORED path
    # is NOT reported. `target/` is where these gates themselves write — clippy, the
    # rustdoc gate and the ratchet lane all do, on every run — so a fingerprint that
    # included it would cry wolf every single time and the report would be tuned out.
    out="$(tree_drift_scenario "$repo" 0 bash -c "printf 'artifact\n' >'$repo/target/built'")"
    if printf '%s' "$out" | command grep -q 'held still'; then
        echo "  ok    a gate writing under an ignored path does not read as drift"
        pass=$((pass + 1))
    else
        echo "  FAIL  an ignored path was reported as drift, which would make this line noise: $out"
        fail=$((fail + 1))
    fi
    rm -f "$repo/target/built"

    # ⑺ AN UNMEASURABLE TREE SAYS SO, and must NOT read as a still one — the escape hatch
    # this workspace's rule 6 is about: unclassified is not a pass.
    out="$(tree_drift_scenario "$tmp" 0 true)"
    if printf '%s' "$out" | command grep -q 'could not be taken' &&
        ! printf '%s' "$out" | command grep -q 'held still'; then
        echo "  ok    a tree that cannot be fingerprinted says so rather than reading as still"
        pass=$((pass + 1))
    else
        echo "  FAIL  a non-repository did not report an unknown: $out"
        fail=$((fail + 1))
    fi

    # ⑻ AN EMPTY POPULATION REFUSES. Register item 924: a gate that is green because it
    # read nothing is the shape that survives longest without anybody noticing.
    mkdir -p "$tmp/empty"
    (cd "$tmp/empty" && git init -q .) >/dev/null 2>&1
    out="$(tree_drift_scenario "$tmp/empty" 0 true)"
    if printf '%s' "$out" | command grep -q 'enumerated no files' &&
        ! printf '%s' "$out" | command grep -q 'held still'; then
        echo "  ok    a fingerprint of nothing refuses instead of reporting no drift"
        pass=$((pass + 1))
    else
        echo "  FAIL  an empty tree read as a still tree: $out"
        fail=$((fail + 1))
    fi

    # ⑼ ⛔⛔⛔ THE TRAP NEVER CHANGES THE HOOK'S VERDICT. This runs on every commit and
    # every push; if it could turn an `exit 1` into a zero it would be the worst defect in
    # this directory — a gate silently disarmed by its own diagnostic.
    tree_drift_scenario "$repo" 3 true >/dev/null
    status=$?
    if [ "$status" -eq 3 ]; then
        echo "  ok    the report preserves the hook's own exit status"
        pass=$((pass + 1))
    else
        echo "  FAIL  a hook exiting 3 came out as $status — the trap changed the verdict"
        fail=$((fail + 1))
    fi

    # ⑽ ⛔⛔ AN INTERRUPTED HOOK STILL REPORTS AND STILL TIDIES UP — register item 927, and
    # the case a six-minute gate actually meets: somebody presses Ctrl-C. The scenario is
    # driven with a REAL signal rather than a simulated one, because "the trap is installed"
    # is a claim about text and this is a claim about what the process does.
    #
    # ⚠⚠ WHAT THIS ARM CAN AND CANNOT FALSIFY, since it cost a wrong answer to find out. It
    # does NOT discriminate an explicit `trap ... TERM` — removing three such traps left it
    # green, which is what showed them to be dead code. What it pins is the EXIT trap and
    # the cleanup: delete either and this arm goes red while the still-tree arms stay green.
    local before_dirs after_dirs sig_out sig_pid
    before_dirs="$(find "${TMPDIR:-/tmp}" -maxdepth 1 -name 'sprag-tree-drift.*' -type d 2>/dev/null | wc -l | tr -d ' ')"
    sig_out="$tmp/signalled"
    (
        tree_drift_begin "interrupted" "$repo"
        printf 'ready\n' >"$tmp/ready"
        sleep 30
    ) >"$sig_out" 2>&1 &
    sig_pid=$!
    while [ ! -f "$tmp/ready" ]; do
        if ! kill -0 "$sig_pid" 2>/dev/null; then
            break
        fi
    done
    kill -TERM "$sig_pid" 2>/dev/null
    wait "$sig_pid" 2>/dev/null
    after_dirs="$(find "${TMPDIR:-/tmp}" -maxdepth 1 -name 'sprag-tree-drift.*' -type d 2>/dev/null | wc -l | tr -d ' ')"
    if command grep -q 'held still' "$sig_out" && [ "$after_dirs" -eq "$before_dirs" ]; then
        echo "  ok    an interrupted hook still reports, and leaves no scratch behind"
        pass=$((pass + 1))
    else
        echo "  FAIL  a TERMed hook reported $(cat "$sig_out" 2>/dev/null | tr '\n' ' ')" \
             "and left $before_dirs -> $after_dirs scratch director(ies)"
        fail=$((fail + 1))
    fi

    # ⑾ AND THE HOOKS ACTUALLY USE IT. `ident-gate.sh`'s arms, for its reason: a library
    # nothing sources is a gate that runs nowhere.
    local here
    here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    for hook in pre-commit pre-push; do
        if command grep -q 'tree-drift\.sh"' "$here/$hook" &&
            command grep -q 'tree_drift_begin' "$here/$hook"; then
            echo "  ok    the ${hook} hook sources tree-drift.sh and arms the report"
            pass=$((pass + 1))
        else
            echo "  FAIL  the ${hook} hook does not source tree-drift.sh and arm the report"
            fail=$((fail + 1))
        fi
    done

    rm -rf "$tmp"
    echo "tree-drift selftest: ${pass}/$((pass + fail)) arm(s) pass"
    [ "$fail" -eq 0 ]
}

# ⛔⛔⛔⛔⛔ AND A BARE RUN IS ANSWERED, NOT PASSED IN SILENCE — register item 819. A library
# that exits 0 having printed nothing defeats the person checking their work by hand, which
# is worse than a gate that fails to check something: the tooling answers *success* to a
# question it never heard.
if [ "${BASH_SOURCE[0]}" = "${0}" ]; then
    # shellcheck source-path=SCRIPTDIR
    . "$(dirname "${BASH_SOURCE[0]}")/scratch-guard.sh"
    case "${1:-}" in
        --selftest) tree_drift_selftest; exit $? ;;
        *) echo "tree-drift.sh is a LIBRARY, not a command: it is sourced by pre-commit and pre-push." >&2
           echo "usage: tree-drift.sh --selftest   (the report itself needs a hook's run to bracket)" >&2
           exit 2 ;;
    esac
fi
