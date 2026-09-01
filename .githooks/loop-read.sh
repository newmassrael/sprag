#!/usr/bin/env bash
# WHAT A RUN'S ENDING COSTS WHEN NOBODY IS WATCHING THE SCREEN -- register item
# 798, and the SIXTH member of the family `hosted-read.sh` holds for CI.
#
# ⛔⛔⛔⛔⛔ THE MEASUREMENT THIS FILE EXISTS FOR. On 2026-09-01 `run134` ended
# `failed` at 23:26:53 and the watcher did not know for over an hour. What told
# it was the OWNER asking. Five runs ended that day -- 110, 122, 127, 133, 134 --
# and every one of them had to be re-launched by a person. The ending is on the
# outer pane's screen every time, and a screen only reaches somebody who is
# looking at it. The north star is a loop that runs WITHOUT a watcher.
#
# ⚠⚠ AND WHAT THE REPAIR IS NOT. This file does NOT re-launch anything and does
# not classify a `failed` as retryable. Item 798 is explicit that telling those
# apart comes BEFORE any such prescription, and that what it asks for is REACH,
# not a restart. What is added here is one sentence, off the screen, in the one
# place this repository has already proved a sentence gets read: the push.
#
# ⚠ WHY NOT IN `hosted-read.sh`. The two ask different worlds -- that one asks
# GitHub about a commit, this one reads the daemon's own run log off disk. They
# share a SHAPE, not a source, and folding two sources into one file is how a
# reader ends up believing one act discharged both (item 779's whole subject).
#
# Self-tested: `bash .githooks/loop-read.sh --selftest` drives every arm, and
# `crates/sprag-gate/` RUNS that selftest without anybody adding it to a list
# (register item 799) -- this file is the first script that walk picked up.
set -uo pipefail

# ⛔ THE SCRATCH-SAFETY DECISION IS NOT WRITTEN TWICE -- `scratch-guard.sh` holds
# it and drives the cases this harness cannot produce (register items 792, 799).
# shellcheck source-path=SCRIPTDIR
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/scratch-guard.sh"

# ⛔⛔⛔⛔⛔ WHAT AN UNREADABLE LOG COSTS -- and why it is not reported as zero.
#
# Item 790 paid this exact lesson for CI: *a commit with no run* and *a run that
# had not spoken* were one sentence, and the reader went looking and stopped. The
# same two states are here and they are sharper, because the state directory a
# hook sees need NOT be the one the loop writes: the loop exports its own
# `XDG_STATE_HOME`, a hook usually inherits none, and the fallback directory is a
# real directory holding real (empty) logs from integration tests. So "0 endings
# unread" is exactly what a clone looking in the wrong place would print.
LOOP_READ_BLIND_COST="a directory with no run log at all is not a loop that \
ended nothing -- the loop exports its own XDG_STATE_HOME and a hook inherits \
none, so a zero here is what looking in the wrong place prints"

# ⛔⛔⛔⛔ WHAT AN UNREAD ENDING COSTS -- register item 798's own number.
LOOP_READ_UNREAD_COST="run134 ended failed at 23:26:53 and the watcher learnt of \
it over an hour later, from the owner asking -- five runs ended that day and a \
person had to re-launch every one"

# ⚠⚠ WHAT A BASELINE COSTS, SAID OUT LOUD RATHER THAN SWALLOWED. The endings
# already on disk when this instrument arrives cannot all be read by hand -- there
# were 127 of them on the day it was written -- and a debt that cannot reach zero
# stops being acted on (rule 5). So they are written down as a baseline WITH THEIR
# COUNT, which is the difference between a declared residue and a quiet drop.
LOOP_READ_BASELINE_COST="the endings already on disk when this instrument \
arrived were never read by anybody -- they are declared, not discharged"

# WHERE THE DAEMON'S RUN LOGS ARE, derived the way the PRODUCT derives it.
#
# ⛔⛔ THE SAME THREE STEPS AS `durability::sprag_state_dir`, in the same order,
# including the `is_absolute` filter -- a second derivation that drifts is how two
# artifacts end up in two directories, which that function's own doc says. It is
# re-derived here rather than asked of the binary because a hook must answer with
# no build in the tree, and `crates/sprag-gate/` holds the two to each other.
loop_read_state_dir() {
    local state home
    state="${XDG_STATE_HOME:-}"
    case "$state" in
        /*) printf '%s\n' "${state}/sprag"; return 0 ;;
    esac
    home="${HOME:-}"
    case "$home" in
        /*) printf '%s\n' "${home}/.local/state/sprag"; return 0 ;;
    esac
    printf '%s\n' "/tmp/sprag"
}

# The marker: which endings somebody has recorded reading, in THIS clone.
loop_read_marker() {
    printf '%s\n' "$(git rev-parse --absolute-git-dir)/sprag-loop-read"
}

# EVERY ENDING ON DISK, as `<log-stem>#<id> <outcome>` per line -- or the single
# word `unknown` where the question could not be put.
#
# ⚠ A DIRECTORY WALK, NOT A NAMED FILE. The log is keyed by the daemon's socket,
# so naming one file decides in advance which daemon counts -- and the one it
# leaves out is the one nobody is watching (the reason every gate in this tree
# walks). Test leftovers land here too and cost nothing: their logs hold no runs,
# so CONTENT excludes them rather than a filter anybody has to maintain.
#
# ⚠⚠ THREE ANSWERS AND NONE OF THEM A DEFAULT: the lines that were found; empty
# where logs were read and held no ending; `unknown` where there was no readable
# log at all, or no `jq` to read one with.
loop_read_endings() {
    local dir log stem found any
    command -v jq >/dev/null 2>&1 || { printf 'unknown\n'; return 0; }
    dir="$(loop_read_state_dir)"
    [ -d "$dir" ] || { printf 'unknown\n'; return 0; }
    any=0
    found=""
    for log in "$dir"/*.runs.json; do
        [ -r "$log" ] || continue
        any=1
        stem="$(basename "$log" .runs.json)"
        found="$found$(jq -r --arg stem "$stem" \
            '.runs[]? | select(.finished == true)
             | "\($stem)#\(.id) \(.outcome // "none")"' "$log" 2>/dev/null)
"
    done
    [ "$any" -eq 1 ] || { printf 'unknown\n'; return 0; }
    printf '%s' "$found" | command sed '/^$/d'
}

# The keys (without outcomes) of every ending on disk, or empty when unknown.
loop_read_keys() {
    local said
    said="$(loop_read_endings)"
    [ "$said" = unknown ] && return 0
    printf '%s\n' "$said" | command sed '/^$/d;s/ .*//'
}

# The keys this clone has already accounted for -- baseline and read alike.
loop_read_accounted() {
    local marker
    marker="$(loop_read_marker)"
    [ -r "$marker" ] || return 0
    command sed -n 's/^\(baseline\|read\) //p' "$marker"
}

# Whether a baseline has been laid down here.
loop_read_has_baseline() {
    local marker
    marker="$(loop_read_marker)"
    [ -r "$marker" ] || return 1
    command grep -q '^baseline ' "$marker"
}

# How many endings the baseline swallowed, or 0.
loop_read_baseline_count() {
    local marker
    marker="$(loop_read_marker)"
    [ -r "$marker" ] || { printf '0\n'; return 0; }
    command grep -c '^baseline ' "$marker"
}

# THE ENDINGS NOBODY HAS RECORDED READING -- `<key> <outcome>` per line.
loop_read_owed() {
    local said accounted line key
    said="$(loop_read_endings)"
    [ "$said" = unknown ] && return 0
    accounted="$(loop_read_accounted)"
    printf '%s\n' "$said" | command sed '/^$/d' | while read -r line; do
        key="${line%% *}"
        printf '%s\n' "$accounted" | command grep -qx -- "$key" || printf '%s\n' "$line"
    done
}

# LAY THE BASELINE: everything that had already ended when this instrument
# arrived is written down, with its count, and is NOT counted as read.
#
# ⛔⛔⛔⛔⛔ IT REFUSES A SECOND TIME, and that refusal is the whole of rule 6
# here. A baseline that can be re-laid is an exemption list spelled as a command:
# one call would swallow every ending nobody got round to, and the sentence would
# go back to reading like a receipt. Once laid, the only way an ending leaves the
# owed list is `--seen <key>`, one key at a time, typed by whoever read it.
loop_read_baseline() {
    local keys marker count
    if loop_read_has_baseline; then
        echo "loop-read: a baseline is already laid here ($(loop_read_baseline_count)" \
             "ending(s)) -- laying a second one would swallow every ending nobody" \
             "has read yet, which is the covering this instrument exists to stop." \
             "Read them and record each with '--seen <key>' (register item 798)." >&2
        return 2
    fi
    keys="$(loop_read_keys)"
    if [ -z "$keys" ]; then
        echo "loop-read: there is no ending on disk to lay a baseline over --" \
             "$(loop_read_state_dir) holds no readable run log with a finished" \
             "run in it. ${LOOP_READ_BLIND_COST}" >&2
        return 1
    fi
    marker="$(loop_read_marker)"
    printf '%s\n' "$keys" | command sed '/^$/d;s/^/baseline /' > "$marker"
    count="$(printf '%s\n' "$keys" | command grep -c .)"
    echo "loop-read: ${count} ending(s) already on disk are the baseline --" \
         "${LOOP_READ_BASELINE_COST}. Anything that ends from now on is owed" \
         "until '--seen <key>' says somebody read it."
}

# RECORD that the ending `$1` was read.
#
# ⛔⛔ ONE KEY AT A TIME AND IT MUST EXIST. There is deliberately no `--seen all`:
# the act being recorded is a person reading one run's ending, and a form that can
# discharge a list in one word records something nobody did. A key that is not on
# disk is refused rather than stored, because a marker that accumulates keys
# nothing produced is a list that never shrinks.
loop_read_seen() {
    local key marker
    key="${1:-}"
    if [ -z "$key" ]; then
        echo "loop-read: name the ending that was read --" \
             "'--seen <log-stem>#<id>', one key at a time (register item 798)." >&2
        return 2
    fi
    if ! loop_read_keys | command grep -qx -- "$key"; then
        echo "loop-read: '${key}' is not an ending in $(loop_read_state_dir) --" \
             "recording it would put a key in this clone's marker that no run log" \
             "produced. Ask '--owed' for the ones that are there." >&2
        return 1
    fi
    marker="$(loop_read_marker)"
    if loop_read_accounted | command grep -qx -- "$key"; then
        echo "loop-read: ${key} was already accounted for here"
        return 0
    fi
    printf 'read %s\n' "$key" >> "$marker"
    echo "loop-read: recorded that ${key}'s ending was read"
}

# THE SENTENCE. Four states, and none of them may be silent about which it is.
#
# ⛔ The blind state is NOT folded into "0 unread", which is the whole of item
# 790's lesson arriving in a second place: a hook that inherits no
# `XDG_STATE_HOME` reads a DIFFERENT directory from the one the loop writes, and
# that directory really does hold run logs -- empty ones, from integration tests.
#
# ⚠⚠ A GAP OF ZERO IS A RECEIPT AND SAYS ONLY THAT. `hosted_read_gap`'s arm (5)
# argued this and it holds here for the same reason: a line that reads the same
# whether or not anything is owed becomes one more notification nobody looks at.
loop_read_gap() {
    local said owed count listed
    said="$(loop_read_endings)"
    if [ "$said" = unknown ]; then
        echo "loop-read: NO RUN LOG COULD BE READ in $(loop_read_state_dir), so" \
             "whether a run ended unwatched cannot be read here --" \
             "${LOOP_READ_BLIND_COST}"
        return 0
    fi
    if ! loop_read_has_baseline; then
        count="$(loop_read_keys | command grep -c . || true)"
        echo "loop-read: NOBODY HAS LAID A BASELINE in this clone, so which of the" \
             "${count} ending(s) in $(loop_read_state_dir) went unread cannot be" \
             "read here -- run 'loop-read.sh --baseline' to write them down as" \
             "already-past, or read them"
        return 0
    fi
    owed="$(loop_read_owed)"
    if [ -z "$owed" ]; then
        echo "loop-read: every run that has ended since the baseline was read" \
             "(baseline: $(loop_read_baseline_count) ending(s) declared unread)"
        return 0
    fi
    count="$(printf '%s\n' "$owed" | command grep -c .)"
    listed="$(printf '%s\n' "$owed" | command tr '\n' ' ')"
    echo "loop-read: ${count} run(s) ENDED AND NOBODY HAS RECORDED READING THEM" \
         "(${listed% }) -- ${LOOP_READ_UNREAD_COST}"
}

# ⛔⛔⛔⛔⛔ THE ARMS. Driven against a THROWAWAY state directory, never the real
# one -- `hosted-read.sh` learnt that the hard way (register item 792: a harness
# that can write outside its subject eventually does), and the same rule is
# written into this one before it has had the chance.
loop_read_selftest() {
    local here tmp pass fail said saved_state saved_home rc
    local scratch_refusal
    here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    # ⛔⛔⛔⛔⛔ CHECKED IN THE SAME STATEMENT IT IS TAKEN -- register item 792, and
    # this file did NOT do it in its first draft: the check was one line below, and
    # `crates/sprag-gate/` refused the hook by name the first time it ran it. That
    # is the gate working, and the rule is stricter than it looks -- `mktemp` exits
    # 127 when it is not on PATH, the variable is then empty, and every later
    # `"$tmp"/...` is a command about the CALLER'S filesystem root.
    tmp="$(mktemp -d)" || {
        echo "loop-read: no scratch directory could be taken, so this selftest" \
             "would drive the caller's own state directory -- refusing" >&2
        return 1
    }
    # ⚠ AND CHECKED AGAIN FOR ABSOLUTENESS, which is not the same claim: the pair
    # is `hosted-read.sh`'s, for its reason -- a `mktemp` that succeeds with a
    # RELATIVE path still makes `rm -rf "$tmp"` a command about the working
    # directory, and a status of 0 says nothing about that.
    case "$tmp" in
        /*) ;;
        *)  echo "loop-read: mktemp gave no absolute path, so this selftest would" \
                 "drive the caller's own state directory -- refusing" >&2
            return 1 ;;
    esac
    pass=0
    fail=0

    saved_state="${XDG_STATE_HOME:-}"
    saved_home="${HOME:-}"
    mkdir -p "$tmp/state/sprag" "$tmp/repo"
    git -C "$tmp/repo" init -q -b main
    export XDG_STATE_HOME="$tmp/state"

    # ⛔⛔⛔⛔⛔ THE SUBJECT IS THE SCRATCH REPOSITORY OR THIS DOES NOT RUN AT ALL —
    # `hosted-read.sh`'s guard, here because register item 792's lesson belongs to
    # every harness in this directory and not to the one file that paid for it.
    # The marker is reached through `loop_read_marker`, which answers WHATEVER
    # repository the process is standing in, so the one thing making these arms
    # safe is that the process is standing in `$tmp/repo`. It is checked, not
    # assumed, and a failure STOPS rather than scoring an arm.
    #
    # ⚠⚠ TWO CLAIMS, KEPT APART — and their folding is not hypothetical: the same
    # check written as one comparison refused on macOS every time, because
    # `mktemp -d` there answers under `/var`, a symlink to `/private/var`, so
    # `pwd` and git's answer disagreed. PHYSICAL paths on both sides for *am I
    # inside my own scratch*, and a separate equality for *it is not the
    # caller's* — the second is what actually keeps somebody else's marker safe.
    scratch_refusal="$(scratch_guard_check "$tmp/repo")"
    if [ -n "$scratch_refusal" ]; then
        echo "loop-read selftest: REFUSING to run -- ${scratch_refusal}, so" \
             "every arm would read and WRITE" \
             "$(git rev-parse --absolute-git-dir 2>/dev/null \
                || echo "some other repository")'s marker instead" >&2
        rm -rf "$tmp"
        return 1
    fi

    # ⚠ The marker lives in a git dir, so the arms run inside the throwaway repo.
    cd "$tmp/repo" || return 1

    # (1) The derivation follows the product's three steps, in order.
    said="$(loop_read_state_dir)"
    if [ "$said" = "$tmp/state/sprag" ]; then
        echo "  ok    an absolute XDG_STATE_HOME decides the directory"
        pass=$((pass + 1))
    else
        echo "  FAIL  state dir was $said"
        fail=$((fail + 1))
    fi
    # ⛔⛔ THE OVERRIDE IS PENNED IN A SUBSHELL, and the first draft of this arm was
    # not -- `A=1 B=2 var=$(...)` is THREE ASSIGNMENTS, not a command prefix, so
    # `XDG_STATE_HOME` stayed changed and every arm below it read the wrong
    # directory. Five of them went red and two went GREEN having measured nothing,
    # which is the shape this whole file is about: a wrong place answering.
    said="$(XDG_STATE_HOME="not/absolute" HOME="$tmp/home" loop_read_state_dir)"
    if [ "$said" = "$tmp/home/.local/state/sprag" ]; then
        echo "  ok    a relative XDG_STATE_HOME is skipped, exactly as the product skips it"
        pass=$((pass + 1))
    else
        echo "  FAIL  a relative XDG_STATE_HOME gave $said"
        fail=$((fail + 1))
    fi

    # (2) An empty directory is UNKNOWN, never zero.
    said="$(loop_read_gap)"
    case "$said" in
        *"NO RUN LOG COULD BE READ"*)
            echo "  ok    a directory with no run log says so instead of zero"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  an empty state dir said: $said"
            fail=$((fail + 1)) ;;
    esac

    # (3) A log with no finished run is a real zero, and is NOT unknown.
    cat > "$tmp/state/sprag/probe.runs.json" <<'EMPTY'
{"version":1,"runs":[{"id":1,"finished":false,"outcome":null}]}
EMPTY
    said="$(loop_read_gap)"
    case "$said" in
        *"NO RUN LOG COULD BE READ"*)
            echo "  FAIL  a readable log with no ending was called unreadable: $said"
            fail=$((fail + 1)) ;;
        *"NOBODY HAS LAID A BASELINE"*)
            echo "  ok    a log whose runs are all still going is not an unread ending"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  a log with no ending said: $said"
            fail=$((fail + 1)) ;;
    esac

    # (4) An ending with no baseline is named as unbaselined, not as owed.
    cat > "$tmp/state/sprag/probe.runs.json" <<'ENDED'
{"version":1,"runs":[{"id":1,"finished":true,"outcome":"failed"},
                     {"id":2,"finished":false,"outcome":null}]}
ENDED
    said="$(loop_read_gap)"
    case "$said" in
        *"NOBODY HAS LAID A BASELINE"*"1 ending(s)"*)
            echo "  ok    an ending with no baseline says how many there are"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  an unbaselined ending said: $said"
            fail=$((fail + 1)) ;;
    esac

    # (5) The baseline says its own count out loud.
    said="$(loop_read_baseline)"
    case "$said" in
        *"1 ending(s) already on disk are the baseline"*)
            echo "  ok    a baseline names how many it declared unread"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  the baseline said: $said"
            fail=$((fail + 1)) ;;
    esac
    said="$(loop_read_gap)"
    case "$said" in
        *"ENDED AND NOBODY HAS RECORDED READING"*)
            echo "  FAIL  a baselined ending was still owed: $said"
            fail=$((fail + 1)) ;;
        *)  echo "  ok    what the baseline covered is not owed"
            pass=$((pass + 1)) ;;
    esac

    # (6) ⛔ A SECOND BASELINE IS REFUSED -- the escape hatch rule 6 forbids.
    loop_read_baseline >/dev/null 2>&1
    rc=$?
    if [ "$rc" -eq 0 ]; then
        echo "  FAIL  a second baseline was accepted"
        fail=$((fail + 1))
    else
        echo "  ok    a second baseline is refused rather than swallowing the owed"
        pass=$((pass + 1))
    fi

    # (7) A NEW ending after the baseline is owed, by name.
    cat > "$tmp/state/sprag/probe.runs.json" <<'MORE'
{"version":1,"runs":[{"id":1,"finished":true,"outcome":"failed"},
                     {"id":2,"finished":true,"outcome":"converged"}]}
MORE
    said="$(loop_read_gap)"
    case "$said" in
        *"ENDED AND NOBODY HAS RECORDED READING"*"probe#2 converged"*)
            echo "  ok    an ending after the baseline is owed and named"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  a new ending said: $said"
            fail=$((fail + 1)) ;;
    esac

    # (8) Reading it clears it, so this list can reach zero.
    loop_read_seen "probe#2" >/dev/null
    said="$(loop_read_gap)"
    case "$said" in
        *"ENDED AND NOBODY HAS RECORDED READING"*)
            echo "  FAIL  a read ending stayed owed: $said"
            fail=$((fail + 1)) ;;
        *)  echo "  ok    reading an ending clears it"
            pass=$((pass + 1)) ;;
    esac

    # (9) ⛔ A key nothing produced is refused, and so is no key at all.
    if loop_read_seen "probe#99" >/dev/null 2>&1; then
        echo "  FAIL  --seen accepted a key no run log produced"
        fail=$((fail + 1))
    else
        echo "  ok    --seen refuses a key no run log produced"
        pass=$((pass + 1))
    fi
    if loop_read_seen >/dev/null 2>&1; then
        echo "  FAIL  --seen accepted no key at all"
        fail=$((fail + 1))
    else
        echo "  ok    --seen refuses a look that names nothing"
        pass=$((pass + 1))
    fi

    # (10) ⚠ THE WALK REACHES A SECOND LOG. A named file would have decided in
    # advance which daemon counts.
    cat > "$tmp/state/sprag/other.runs.json" <<'OTHER'
{"version":1,"runs":[{"id":7,"finished":true,"outcome":"exhausted"}]}
OTHER
    said="$(loop_read_gap)"
    case "$said" in
        *"other#7 exhausted"*)
            echo "  ok    the walk reaches a log nobody named"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  a second log said: $said"
            fail=$((fail + 1)) ;;
    esac

    # (11) The pre-push hook calls this instrument -- the reach item 798 asks for
    # is a sentence at the push, and a hook that stopped calling it is silence.
    # ⛔⛔ *UNREADABLE* AND *NOT WIRED* ARE SEPARATE FINDINGS — see the same arm in
    # `hosted-read.sh` and register item 803: its folded version named the wrong
    # cause on the one occasion it ever fired.
    if [ ! -r "$here/pre-push" ]; then
        echo "  FAIL  pre-push is not readable at $here -- the wiring cannot be" \
             "judged, and this is NOT the same finding as it not calling the arm"
        fail=$((fail + 1))
    elif command grep -q 'loop_read_gap' "$here/pre-push"; then
        echo "  ok    the pre-push hook calls the gap arm"
        pass=$((pass + 1))
    else
        echo "  FAIL  pre-push is readable and never calls loop_read_gap"
        fail=$((fail + 1))
    fi

    cd / || true
    rm -rf "$tmp"
    if [ -n "$saved_state" ]; then export XDG_STATE_HOME="$saved_state"; else unset XDG_STATE_HOME; fi
    if [ -n "$saved_home" ]; then export HOME="$saved_home"; fi
    echo "loop-read selftest: ${pass}/$((pass + fail)) arm(s) pass"
    [ "$fail" -eq 0 ]
}

if [ "${BASH_SOURCE[0]}" = "${0}" ]; then
    case "${1:-}" in
        --selftest)  loop_read_selftest ;;
        --baseline)  loop_read_baseline ;;
        --seen)      shift; loop_read_seen "${1:-}" ;;
        --owed)      loop_read_owed ;;
        --gap|"")    loop_read_gap ;;
        *) echo "usage: loop-read.sh [--gap|--owed|--baseline|--seen KEY|--selftest]" >&2
           exit 2 ;;
    esac
fi
