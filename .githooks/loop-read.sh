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

# ⛔⛔⛔⛔ WHAT A RUN NOBODY IS DRIVING COSTS -- register item 801, arm 3.
#
# It is not a tidiness complaint. A run whose daemon died stays `finished: false`
# for ever, so every count of *how many runs are going* includes it: measured
# 2026-09-01, fourteen runs answered that question and the true answer was THREE.
# A watcher reading fourteen has no reason to look, which is the same silence item
# 798 is about arriving by a different road.
LOOP_READ_STRANDED_COST="a run whose daemon is gone stays open for ever, so every \
count of how many are going includes it -- fourteen said running on 2026-09-01 \
and three were"

# ⛔⛔⛔⛔ WHAT AN ENDING NOBODY CAN ACT ON COSTS -- register item 867, and the
# number item 827 measured for the half 798 left empty.
#
# 798 bought REACH: the ending leaves the screen and arrives at the push. It said
# in its own body that it was not buying what somebody then DOES about it, and
# 827 priced that: a sprag run ended 2026-09-02 at 08:10:18 and the next driver
# started three hours forty-nine minutes later, while two other repositories were
# re-launched inside three minutes. The ending was on a screen the whole time.
LOOP_READ_NEXT_COST="a run ended at 08:10:18 on 2026-09-02 and the next driver \
started three hours forty-nine minutes later, while two other repositories were \
re-launched inside three minutes -- the ending was readable the whole time and \
what nobody could ask was what to do about it"

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

# The marker: which events somebody has recorded reading, in THIS clone -- or
# EMPTY where this tree has no git dir of its own.
#
# ⛔⛔⛔ EMPTY IS AN ANSWER HERE — register item 804. The first version joined
# `git rev-parse --absolute-git-dir` to the file name unconditionally, and outside
# a repository that answer is the empty string: the marker became
# `/sprag-loop-read`, the FILESYSTEM ROOT. Every caller now has to decide what to
# say about having no marker, which is the point — the old shape let `--gap`
# report *"nobody has laid a baseline"* about a marker it had never been able to
# look at, and those are different facts.
loop_read_marker() {
    local home
    home="$(scratch_guard_marker_home)"
    [ -n "$home" ] || return 0
    printf '%s\n' "${home}/sprag-loop-read"
}

# EVERY EVENT ON DISK A PERSON HAS TO READ ONCE, as `<key> <word>` per line -- or
# the single word `unknown` where the question could not be put.
#
# Two kinds, and they carry DIFFERENT KEYS so reading one never discharges the
# other:
#
# * `<log-stem>#<id> <outcome>` -- the run ENDED (register item 798);
# * `<log-stem>#<id>!stranded no-driver` -- the run is still open and NOTHING IS
#   DRIVING IT (register item 801, arm 3).
#
# ⛔⛔⛔⛔⛔ WHY THE SECOND IS HERE AT ALL, AND WHY THE REGISTER WAS WRONG ABOUT IT.
# Item 801 records that telling a stranded run from a running one *"is a product
# change and needs a daemon promotion -- a hook cannot pay it"*. Re-measured
# 2026-09-01: the run log ALREADY carries `driving`, so the distinction is a
# question about a file this hook already reads. The product half of 801 is real
# and stays open — `query("runs")` renders both as `running`, and
# `Unordered::NoDriver` proves the daemon KNOWS the difference and only says it
# when somebody tries to steer such a run — but the READING half needed no
# promotion at all.
#
# ⚠⚠ AND IT IS THE ONE HALF OF *"no progress"* THAT NEEDS NO CLOCK. Item 801's
# first two arms want timestamps the record simply does not have. A run whose
# driver is gone is a different fact and it is on disk today: eleven of them were,
# against three that were actually going, and every count of *how many are
# running* answered fourteen.
#
# ⚠ A DIRECTORY WALK, NOT A NAMED FILE. The log is keyed by the daemon's socket,
# so naming one file decides in advance which daemon counts -- and the one it
# leaves out is the one nobody is watching (the reason every gate in this tree
# walks). Test leftovers land here too and cost nothing: their logs hold no runs,
# so CONTENT excludes them rather than a filter anybody has to maintain.
#
# ⚠⚠ THREE ANSWERS AND NONE OF THEM A DEFAULT: the lines that were found; empty
# where logs were read and held no event; `unknown` where there was no readable
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
            '.runs[]? | if .finished == true
                        then "\($stem)#\(.id) \(.outcome // "none")"
                        elif .driving == null
                        then "\($stem)#\(.id)!stranded no-driver"
                        else empty end' "$log" 2>/dev/null)
"
    done
    [ "$any" -eq 1 ] || { printf 'unknown\n'; return 0; }
    printf '%s' "$found" | command sed '/^$/d'
}

# The events that are ENDINGS, and the ones that are STRANDED runs -- split by the
# key's own suffix so neither clause has to know how the other is spelt.
#
# ⛔⛔⛔⛔⛔ AN EMPTY ANSWER IS NOT A FAILURE — and getting that wrong turned this
# REPORT into a GATE. `grep` exits 1 when it matches nothing, `set -o pipefail` is
# on in this file, and `pre-push` runs under `set -euo pipefail`: so the first time
# a real `--gap` had endings but no strandings, the assignment carrying this
# function's status killed the hook and **the push was refused**. Measured
# 2026-09-01, the round after the clause shipped: `git push` printed both
# instruments' sentences and then *"failed to push some refs"*, with no other
# diagnosis, and `origin/main` had not moved.
#
# ⚠⚠ THE SELFTEST COULD NOT SEE IT, and that is the lesson worth keeping: the arms
# call these functions WITHOUT `set -e`, so the status was thrown away exactly
# where the hook would have acted on it. The arm added below calls `loop_read_gap`
# the way `pre-push` calls it instead of the way a test finds convenient.
loop_read_ended_only() {
    printf '%s\n' "$1" | command grep -v '!stranded ' | command sed '/^$/d' || true
}

loop_read_stranded_only() {
    printf '%s\n' "$1" | command grep '!stranded ' | command sed '/^$/d' || true
}

# ── WHAT HAPPENS NEXT TO AN ENDING -- register item 867 ───────────────────────
#
# ⛔⛔⛔⛔⛔ THE MAPPING IS NOT IN THIS FILE, AND THAT IS THE WHOLE POINT. The
# endings this hook reads off disk are WORDS (`failed`, `converged`, ...), and
# what to do about each of them is a decision `OutcomeState::disposition` holds
# in the product. Writing a copy of it here -- six words and six answers -- is
# the *one value, two homes* defect items 855 and 864 each paid for once, and
# item 867 refuses it by name. So the product PUBLISHES the table (`sprag
# disposition`) and this file relays what it says, matching on the first field
# and printing the rest verbatim. It never spells an answer.
#
# ⚠⚠ AND IT PRESCRIBES NOTHING. Item 827's prohibition, carried forward by 867:
# *automatically re-launch it* must not be assumed to be the answer. WHICH
# endings a machine may proceed past is the table's own THIRD FIELD since
# register item 872(2) (`same_brief` / `new_brief` / `never`), relayed here
# verbatim with the rest -- this clause says what the product says and does none
# of it, the same shape as every other sentence in this file, which reports and
# never gates.
#
# ⚠⚠ AND WHO IS OWED THE NEXT RUN is the FOURTH FIELD since register item 872(1)
# (`this_runs_opener` / `a_person` / `nobody`). Naming a party is still not a
# prescription: item 872 measured a push whose endings permitted next runs and
# got none, and the gap was that the permission had nobody attached to it, so no
# party had failed. This file relays that word for the same reason it relays the
# other three -- it holds no copy of any of them.
#
# ⛔⛔ THIS COMMENT USED TO TALLY THE ARMS and say a machine may not proceed past
# that many of them -- a number in prose, one of three copies of the same
# sentence, in three files, that nothing anywhere read. A fifth disposition
# would have left all three silently wrong. The rule this file already keeps for
# the MAPPING (never hold a copy) is the rule that sentence broke, one fact over,
# and a gate in `driver.rs` now reads these three sources for it.

# THE REPOSITORY THIS FILE IS IN, derived from the file rather than from `pwd`.
#
# ⛔⛔⛔ RESOLVED AT SOURCE TIME AND NOT PER CALL, and that is a fix rather than a
# saving: `${BASH_SOURCE[0]}` is the path this file was NAMED by, which `pre-push`
# and a person alike give RELATIVELY (`.githooks/loop-read.sh`). Every arm below
# runs from a throwaway repository, and `/`, so a `cd "$(dirname …)"` evaluated
# there is a `cd .githooks` in somebody else's directory -- measured 2026-09-03,
# it printed eight errors and answered the empty string.
LOOP_READ_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

loop_read_repo_root() {
    printf '%s\n' "$LOOP_READ_ROOT"
}

# WHICH BUILD ANSWERS the disposition question, or EMPTY where none can.
#
# ⛔⛔ THE TREE'S BUILD IS PREFERRED OVER `PATH`, and that order is a measurement,
# not a taste: register item 824 is a PROMOTED `sprag` that does not know verbs
# this tree has -- so a `PATH` build asked first would answer about a different
# product than the one being pushed. The tree's own `target/` is the build whose
# classification matches the source in the commit.
#
# ⚠ `$LOOP_READ_SPRAG` is a TEST SEAM and not an escape hatch: pointing it at
# nothing does not silence this report, it makes it say -- loudly -- that nothing
# could be asked. There is no value of it that turns the sentence into a receipt.
loop_read_sprag() {
    local candidate root
    candidate="${LOOP_READ_SPRAG:-}"
    if [ -n "$candidate" ]; then
        [ -x "$candidate" ] && printf '%s\n' "$candidate"
        return 0
    fi
    root="$(loop_read_repo_root)"
    for candidate in "$root/target/debug/sprag" "$root/target/release/sprag"; do
        if [ -x "$candidate" ]; then
            printf '%s\n' "$candidate"
            return 0
        fi
    done
    command -v sprag 2>/dev/null || true
    return 0
}

# THE PRODUCT'S ENDING → NEXT-STEP TABLE, or EMPTY when the build that answered
# does not know the question.
#
# ⛔⛔⛔ AN UNKNOWN VERB IS NOT AN EMPTY TABLE, and both are checked because they
# are answered differently by different builds. Measured 2026-09-03 against this
# tree's binary: `sprag nosuchverb` exits **2** with stdout EMPTY and the usage on
# stderr. A build that instead printed its usage on stdout and exited 0 would hand
# this hook a page of text to look words up in, so the header is checked too --
# `disposition` is the first thing the verb prints and nothing else here is.
loop_read_disposition_table() {
    local sprag said
    sprag="$(loop_read_sprag)"
    [ -n "$sprag" ] || return 0
    said="$("$sprag" disposition 2>/dev/null)" || return 0
    case "$said" in
        disposition*) printf '%s\n' "$said" ;;
    esac
    return 0
}

# THE SENTENCE for each ending in `$1`, grouped by the ending's own word.
#
# ⚠⚠ GROUPED BY THE ENDING AND NOT ONE LINE PER RUN, because the population says
# so: measured 2026-09-03 on this clone, 48 owed endings spoke FOUR distinct words
# (`failed`, `converged`, `cancelled`, `exhausted`). Forty-eight lines would be a
# wall; four are a decision.
#
# ⚠ THE ORDER IS THE PRODUCT'S -- the walk is over the table's rows, not over the
# endings -- so this file decides nothing about which answer comes first either.
loop_read_next_steps() {
    local ended sprag table rows word rest keys ekey eword classified unclassified
    ended="$1"
    [ -n "$ended" ] || return 0
    sprag="$(loop_read_sprag)"
    if [ -z "$sprag" ]; then
        echo "loop-read: and WHAT HAPPENS NEXT TO THEM COULD NOT BE ASKED -- no" \
             "sprag build is reachable under $(loop_read_repo_root)/target or on" \
             "PATH, and this hook keeps no copy of that answer on purpose." \
             "${LOOP_READ_NEXT_COST}"
        return 0
    fi
    table="$(loop_read_disposition_table)"
    if [ -z "$table" ]; then
        echo "loop-read: and WHAT HAPPENS NEXT TO THEM COULD NOT BE ASKED --" \
             "${sprag} does not answer 'disposition', so the build that replied is" \
             "older than the question (register item 824). ${LOOP_READ_NEXT_COST}"
        return 0
    fi
    # The rows and not the header: a row is indented, the header is not.
    rows="$(printf '%s\n' "$table" | command grep '^  ' || true)"
    while read -r word rest; do
        [ -n "$word" ] || continue
        keys=""
        while read -r ekey eword; do
            [ "$eword" = "$word" ] || continue
            keys="$keys $ekey"
        done <<ENDINGS
$ended
ENDINGS
        [ -n "$keys" ] || continue
        echo "loop-read: ${keys# } ended '${word}' -- ${rest}"
    done <<ROWS
$rows
ROWS
    # ⛔⛔⛔⛔⛔ AN ENDING NOTHING CLASSIFIES IS SAID, NEVER SKIPPED. This
    # workspace's rule 6, and the product's own renderer says the same words for
    # the same reason: a word no disposition covers is an ending that reached the
    # log without anybody deciding what follows it -- which is exactly the state
    # item 827 was filed on. Silence would render it as *nothing to do*.
    classified=""
    while read -r word rest; do
        [ -n "$word" ] || continue
        classified="${classified} ${word} "
    done <<ROWS
$rows
ROWS
    unclassified=""
    while read -r ekey eword; do
        [ -n "$ekey" ] || continue
        case "$classified" in
            *" ${eword} "*) continue ;;
        esac
        unclassified="$unclassified ${ekey}(${eword})"
    done <<ENDINGS
$ended
ENDINGS
    if [ -n "$unclassified" ]; then
        echo "loop-read: NOTHING IN ${sprag} CLASSIFIES what happens next after" \
             "${unclassified# } -- an ending reached the log and no disposition" \
             "covers it, so what to do about it is recorded nowhere"
    fi
    return 0
}

# The keys (without outcomes) of every ending on disk, or empty when unknown.
loop_read_keys() {
    local said
    said="$(loop_read_endings)"
    [ "$said" = unknown ] && return 0
    printf '%s\n' "$said" | command sed '/^$/d;s/ .*//'
}

# The keys this clone has already accounted for -- baseline and read alike.
#
# ⛔⛔⛔⛔⛔ TWO EXPRESSIONS, NOT ONE ALTERNATION. `\|` inside a BRE is a GNU
# EXTENSION: BSD `sed` reads it as a literal `|`, matches nothing, and this
# function then answers "nothing has been accounted for" -- which silently voids
# every baseline and every `--seen`. Measured on macOS 2026-09-01, by the gate
# register item 799 built: `loop-read.sh --selftest` failed there on its first
# CI exposure with *"a baselined ending was still owed"*, while the same file
# passed on every Linux job. Reproduced here with `sed --posix`, which turns the
# extension off in GNU sed and prints nothing for exactly this expression.
loop_read_accounted() {
    local marker
    marker="$(loop_read_marker)"
    [ -r "$marker" ] || return 0
    command sed -n -e 's/^baseline //p' -e 's/^read //p' "$marker"
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
    # ⛔⛔ THE RECORD IS MADE BEFORE IT IS ANNOUNCED, and the status of making it
    # is read -- register item 804. This block used to print its count whatever the
    # write did.
    marker="$(loop_read_marker)"
    printf '%s\n' "$keys" | command sed '/^$/d;s/^/baseline /' \
        | scratch_guard_write "$marker" "loop-read" || return 1
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
    # ⛔⛔ APPENDED FIRST, ANNOUNCED SECOND — register item 804. Outside a
    # repository this line wrote to `/sprag-loop-read`, was refused, and the
    # sentence below still said the reading had been recorded.
    printf 'read %s\n' "$key" \
        | scratch_guard_append "$marker" "loop-read" || return 1
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
    local said owed count listed ended stranded
    said="$(loop_read_endings)"
    if [ "$said" = unknown ]; then
        echo "loop-read: NO RUN LOG COULD BE READ in $(loop_read_state_dir), so" \
             "whether a run ended unwatched cannot be read here --" \
             "${LOOP_READ_BLIND_COST}"
        return 0
    fi
    # ⛔⛔⛔⛔⛔ *NO MARKER TO LOOK AT* AND *NO BASELINE IN IT* ARE DIFFERENT FACTS —
    # register item 804, and the fifth time this directory has paid for folding two
    # states into one sentence. Run from a directory that is not a repository, the
    # old shape said *"nobody has laid a baseline"* about a marker it had never been
    # able to open, and the remedy it then offered (`--baseline`) wrote to the
    # filesystem root and claimed success.
    if [ -z "$(loop_read_marker)" ]; then
        count="$(loop_read_keys | command grep -c . || true)"
        echo "loop-read: THIS TREE HAS NO GIT DIR, so this clone has no marker and" \
             "which of the ${count} event(s) in $(loop_read_state_dir) went unread" \
             "cannot be read here -- that is NOT the same as no baseline having" \
             "been laid, and no baseline can be laid from here either"
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
        echo "loop-read: every run that has ended or lost its driver since the" \
             "baseline was read (baseline: $(loop_read_baseline_count) event(s)" \
             "declared unread)"
        return 0
    fi
    # ⛔⛔ TWO CLAUSES, NEVER ONE WITH A BIGGER NUMBER IN IT. *It ended* and *its
    # driver is gone* are different facts with different remedies, and this file's
    # whole family (items 776, 779, 781, 790, 793) is about what folding two
    # states into one sentence costs. Each clause is silent when its own list is
    # empty, so neither turns into a line that reads the same either way.
    ended="$(loop_read_ended_only "$owed")"
    if [ -n "$ended" ]; then
        count="$(printf '%s\n' "$ended" | command grep -c .)"
        listed="$(printf '%s\n' "$ended" | command tr '\n' ' ')"
        echo "loop-read: ${count} run(s) ENDED AND NOBODY HAS RECORDED READING" \
             "THEM (${listed% }) -- ${LOOP_READ_UNREAD_COST}"
        # ⛔⛔⛔⛔ AND WHAT TO DO ABOUT EACH -- register item 867. The line above is
        # item 798's: the ending REACHES somebody. This one is item 827's other
        # half: it says what follows. They are separate sentences because they are
        # separate facts, and folding them is what every item in this file's family
        # (776, 779, 781, 790, 793) was filed on.
        loop_read_next_steps "$ended"
    fi
    stranded="$(loop_read_stranded_only "$owed")"
    if [ -n "$stranded" ]; then
        count="$(printf '%s\n' "$stranded" | command grep -c .)"
        listed="$(printf '%s\n' "$stranded" | command sed 's/!stranded no-driver//' \
                  | command tr '\n' ' ')"
        echo "loop-read: ${count} run(s) ARE OPEN WITH NOTHING DRIVING THEM and" \
             "nobody has recorded reading that (${listed% }) --" \
             "${LOOP_READ_STRANDED_COST}"
    fi
    # ⛔ A REPORT ENDS 0. Said as a statement rather than left to whatever the last
    # branch happened to be, because `pre-push` runs this under `set -e` and the
    # difference between those two is whether a push happens.
    return 0
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

    # (3) A log whose only run is GOING is a real zero, and is NOT unknown.
    # ⚠ `driving` is what makes it going rather than stranded — an open run with no
    # driver is its own event now (register item 801, arm 3), so the fixture has to
    # say which kind it is instead of leaving the key out.
    cat > "$tmp/state/sprag/probe.runs.json" <<'EMPTY'
{"version":1,"runs":[{"id":1,"finished":false,"outcome":null,"driving":5}]}
EMPTY
    said="$(loop_read_gap)"
    case "$said" in
        *"NO RUN LOG COULD BE READ"*)
            echo "  FAIL  a readable log with no ending was called unreadable: $said"
            fail=$((fail + 1)) ;;
        *"NOBODY HAS LAID A BASELINE"*)
            echo "  ok    a log whose runs are all still going is not an unread event"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  a log with no ending said: $said"
            fail=$((fail + 1)) ;;
    esac

    # (4) An ending with no baseline is named as unbaselined, not as owed.
    cat > "$tmp/state/sprag/probe.runs.json" <<'ENDED'
{"version":1,"runs":[{"id":1,"finished":true,"outcome":"failed"},
                     {"id":2,"finished":false,"outcome":null,"driving":5}]}
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

    # (10a) ⛔⛔⛔⛔⛔ A RUN NOBODY IS DRIVING IS ITS OWN EVENT — register item 801,
    # arm 3, and the arm that proves the register's *"a hook cannot pay it"* wrong
    # for this half. `driving` is on disk already; no promotion was needed.
    cat > "$tmp/state/sprag/probe.runs.json" <<'STRANDED'
{"version":1,"runs":[{"id":1,"finished":true,"outcome":"failed"},
                     {"id":2,"finished":true,"outcome":"converged"},
                     {"id":3,"finished":false,"outcome":null,"driving":9},
                     {"id":4,"finished":false,"outcome":null,"driving":null}]}
STRANDED
    said="$(loop_read_gap)"
    case "$said" in
        *"ARE OPEN WITH NOTHING DRIVING THEM"*"probe#4"*)
            echo "  ok    an open run with no driver is named as stranded"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  a stranded run said: $said"
            fail=$((fail + 1)) ;;
    esac
    case "$said" in
        *"probe#3"*)
            echo "  FAIL  a run that IS being driven was called stranded: $said"
            fail=$((fail + 1)) ;;
        *)  echo "  ok    a run with a driver is not called stranded"
            pass=$((pass + 1)) ;;
    esac

    # (10b) ⛔⛔ THE TWO EVENTS CARRY DIFFERENT KEYS, so reading one never
    # discharges the other. `probe#2`'s ending was recorded at arm (8); if the keys
    # collided, `probe#4`'s stranding would already look accounted for.
    loop_read_seen 'probe#4!stranded' >/dev/null
    said="$(loop_read_gap)"
    case "$said" in
        *"ARE OPEN WITH NOTHING DRIVING THEM"*)
            echo "  FAIL  a read stranding stayed owed: $said"
            fail=$((fail + 1)) ;;
        *)  echo "  ok    reading a stranding clears it, and only it"
            pass=$((pass + 1)) ;;
    esac
    # ⚠ And the ending of that same run is STILL its own unread event.
    cat > "$tmp/state/sprag/probe.runs.json" <<'CLOSED'
{"version":1,"runs":[{"id":1,"finished":true,"outcome":"failed"},
                     {"id":2,"finished":true,"outcome":"converged"},
                     {"id":3,"finished":false,"outcome":null,"driving":9},
                     {"id":4,"finished":true,"outcome":"failed"}]}
CLOSED
    said="$(loop_read_gap)"
    case "$said" in
        *"ENDED AND NOBODY HAS RECORDED READING"*"probe#4 failed"*)
            echo "  ok    a stranded run that later ends is a SECOND unread event"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  the ending of a read-as-stranded run said: $said"
            fail=$((fail + 1)) ;;
    esac
    loop_read_seen 'probe#4' >/dev/null

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

    # (10c) ⛔⛔⛔⛔⛔ RUN FROM SOMEWHERE THAT IS NOT A REPOSITORY — register item
    # 804. The marker cannot be reached there, and *that* must not be reported as
    # *nobody has laid a baseline* — the old sentence sent a reader to run
    # `--baseline`, which wrote to the FILESYSTEM ROOT and announced success.
    #
    # ⛔⛔⛔ AND THE PLACE IS `/`, NOT `$tmp`. The first version used `$tmp` and
    # passed on the author's machine and FAILED IN CI, because whether `$tmp` is
    # inside a repository is a fact about the BOX: here `target/` is a symlink to
    # `~/.buildcache/sprag` (outside), on the runner it is a real directory under
    # the checkout (inside), and `CARGO_TARGET_TMPDIR` puts the symlinked-TMPDIR
    # environment under it. An arm whose premise is an accident of the filesystem
    # measures the filesystem. `/` is the one place that is guaranteed not to be
    # this clone, and `scratch-guard.sh`'s own arm already uses it.
    said="$( cd / && loop_read_gap )"
    case "$said" in
        *"THIS TREE HAS NO GIT DIR"*"NOT the same as no baseline"*)
            echo "  ok    with no git dir the gap says so, and says it is not the other thing"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  outside a repository the gap said: $said"
            fail=$((fail + 1)) ;;
    esac
    if ( cd / && loop_read_baseline >/dev/null 2>&1 ); then
        echo "  FAIL  --baseline reported success with nowhere to write"
        fail=$((fail + 1))
    else
        echo "  ok    --baseline outside a repository records nothing and says so"
        pass=$((pass + 1))
    fi
    if ( cd / && loop_read_seen 'probe#1' >/dev/null 2>&1 ); then
        echo "  FAIL  --seen reported success with nowhere to write"
        fail=$((fail + 1))
    else
        echo "  ok    --seen outside a repository records nothing and says so"
        pass=$((pass + 1))
    fi

    # (10d) ⛔⛔⛔⛔⛔ THE REPORT IS CALLED THE WAY THE HOOK CALLS IT — under
    # `set -euo pipefail`. This is the arm that would have caught the round where a
    # `grep` matching nothing made `--gap` exit non-zero and `git push` was REFUSED.
    #
    # ⛔⛔ AND THE STATE IT IS DRIVEN IN IS THE WHOLE ARM. The first version of this
    # arm used a fixture with BOTH kinds of event in it, so every `grep` matched and
    # a mutation deleting the `|| true` stayed GREEN — a dead control, measured as
    # one within minutes of being written. What breaks is a list where ONE kind is
    # missing, so each kind is driven ALONE, and the pair is what makes the arm
    # real: with only endings the stranded filter matches nothing, and with only
    # strandings the ending filter does.
    # ⛔⛔⛔⛔⛔ AND IT IS RUN IN A CHILD PROCESS, NOT `if ( set -e; … )`. That shape
    # was the SECOND dead control here and it was measured as one: `set -e` is
    # SUPPRESSED for any command in a condition — `if`, `while`, `&&`, `||`, `!` —
    # and the suppression reaches INSIDE a subshell written there, so the guard the
    # arm was trying to observe was switched off by the way the arm asked. A
    # separate `bash -c` and a status read into a variable is the only shape that
    # observes it.
    cat > "$tmp/state/sprag/probe.runs.json" <<'ENDSONLY'
{"version":1,"runs":[{"id":50,"finished":true,"outcome":"failed"}]}
ENDSONLY
    bash -c 'set -euo pipefail; . "$1"; loop_read_gap >/dev/null' _ "$here/loop-read.sh"
    rc=$?
    if [ "$rc" -eq 0 ]; then
        echo "  ok    endings with no stranding exit 0 under set -euo pipefail"
        pass=$((pass + 1))
    else
        echo "  FAIL  a list of endings alone exits ${rc} under set -euo pipefail," \
             "which is a REFUSED PUSH rather than a sentence"
        fail=$((fail + 1))
    fi
    cat > "$tmp/state/sprag/probe.runs.json" <<'STRANDSONLY'
{"version":1,"runs":[{"id":51,"finished":false,"outcome":null,"driving":null}]}
STRANDSONLY
    bash -c 'set -euo pipefail; . "$1"; loop_read_gap >/dev/null' _ "$here/loop-read.sh"
    rc=$?
    if [ "$rc" -eq 0 ]; then
        echo "  ok    strandings with no ending exit 0 under set -euo pipefail"
        pass=$((pass + 1))
    else
        echo "  FAIL  a list of strandings alone exits ${rc} under set -euo pipefail"
        fail=$((fail + 1))
    fi
    loop_read_seen 'probe#51!stranded' >/dev/null
    bash -c 'set -euo pipefail; . "$1"; loop_read_gap >/dev/null' _ "$here/loop-read.sh"
    rc=$?
    if [ "$rc" -eq 0 ]; then
        echo "  ok    and it exits 0 with nothing owed either"
        pass=$((pass + 1))
    else
        echo "  FAIL  the empty report exits ${rc} under set -euo pipefail"
        fail=$((fail + 1))
    fi

    # (12) ⛔⛔⛔⛔⛔ WHAT HAPPENS NEXT TO EACH ENDING -- register item 867.
    #
    # ⚠⚠ THE PRODUCT IS DOUBLED HERE, and the reason is `hosted-read.sh`'s for
    # `gh`: a selftest that asked the REAL binary would measure whether this tree
    # happens to have been built, and would go green on a build that answered
    # nothing. The double is reached through `$LOOP_READ_SPRAG`, so the SUBJECT --
    # the lookup, the grouping, and the three ways it can fail to get an answer --
    # is what is under test. That the real binary's rows have the shape this
    # matches on is held by `cli.rs`, which runs THIS hook against THAT binary.
    mkdir -p "$tmp/bin"
    cat > "$tmp/bin/sprag" <<'ASKED'
#!/usr/bin/env bash
echo "disposition  — what happens next to a run that ended this way"
echo "  converged   next_work  DOUBLE SAYS: a next run, carrying different work"
echo "  failed      person     DOUBLE SAYS: a person, and nothing else until then"
ASKED
    cat > "$tmp/bin/sprag-old" <<'REFUSED'
#!/usr/bin/env bash
echo "sprag: unknown command \"${1:-}\"" >&2
exit 2
REFUSED
    cat > "$tmp/bin/sprag-chatty" <<'CHATTY'
#!/usr/bin/env bash
echo "usage: sprag <command> [arguments]"
echo "  sessions"
echo "    ls"
CHATTY
    cat > "$tmp/bin/sprag-partial" <<'PARTIAL'
#!/usr/bin/env bash
echo "disposition  — what happens next to a run that ended this way"
echo "  converged   next_work  DOUBLE SAYS: a next run, carrying different work"
PARTIAL
    chmod +x "$tmp/bin/sprag" "$tmp/bin/sprag-old" "$tmp/bin/sprag-partial" \
             "$tmp/bin/sprag-chatty"
    cat > "$tmp/state/sprag/probe.runs.json" <<'NEXTSTEPS'
{"version":1,"runs":[{"id":60,"finished":true,"outcome":"failed"},
                     {"id":61,"finished":true,"outcome":"converged"}]}
NEXTSTEPS

    # (12a) ⭐ EACH ENDING GETS ITS OWN ANSWER, and they are DIFFERENT answers.
    # ⛔ The pair is the arm: one sentence asserted alone would stay green on a
    # relay that printed the first row for everything, which is the defect item
    # 867 exists to prevent -- a classification that makes no difference.
    said="$(LOOP_READ_SPRAG="$tmp/bin/sprag" loop_read_gap)"
    case "$said" in
        *"probe#60 ended 'failed' -- person     DOUBLE SAYS: a person"*)
            echo "  ok    an ended run is told what happens next, in the product's words"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  the next step for an ending said: $said"
            fail=$((fail + 1)) ;;
    esac
    case "$said" in
        *"probe#61 ended 'converged' -- next_work  DOUBLE SAYS: a next run"*)
            echo "  ok    a DIFFERENT ending gets a DIFFERENT next step"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  a second ending's next step said: $said"
            fail=$((fail + 1)) ;;
    esac
    # ⛔⛔ AND NEITHER GROUP HOLDS THE OTHER'S RUN. The two checks above are about
    # what is PRESENT, and both survive a relay that files every run under every
    # row -- measured 2026-09-03 by driving exactly that mutation, which left 30
    # of 31 arms green. A classification that separates nothing is the defect.
    #
    # ⚠ ONE LINE, not the whole report: the groups are printed in the product's
    # order, so a whole-blob test for *`probe#61` near `failed`* matches whenever
    # the converged group happens to be printed first -- which it is.
    said="$(printf '%s\n' "$said" | command grep "ended 'failed'" || true)"
    case "$said" in
        *"probe#61"*)
            echo "  FAIL  the converged run was filed under 'failed' too: $said"
            fail=$((fail + 1)) ;;
        *)  echo "  ok    a run appears under its OWN ending and no other"
            pass=$((pass + 1)) ;;
    esac

    # (12b) ⛔ NO BUILD TO ASK IS SAID OUT LOUD, never left silent -- the sentence
    # would otherwise read exactly like a run whose ending needs nothing done.
    said="$(LOOP_READ_SPRAG="$tmp/bin/nosuch" loop_read_gap)"
    case "$said" in
        *"COULD NOT BE ASKED"*"no sprag build is reachable"*)
            echo "  ok    with no build to ask, the gap says so instead of falling silent"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  with no build to ask, the gap said: $said"
            fail=$((fail + 1)) ;;
    esac

    # (12c) ⛔⛔ A BUILD THAT DOES NOT KNOW THE QUESTION IS ITS OWN FINDING --
    # register item 824, which is a PROMOTED `sprag` that does not speak this
    # tree's verbs. Folding it into (12b) would name the wrong repair.
    said="$(LOOP_READ_SPRAG="$tmp/bin/sprag-old" loop_read_gap)"
    case "$said" in
        *"COULD NOT BE ASKED"*"older than the question"*)
            echo "  ok    a build that refuses the verb is named as older, not as absent"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  an older build's answer said: $said"
            fail=$((fail + 1)) ;;
    esac

    # (12c2) ⛔⛔⛔ AND A BUILD THAT ANSWERS SOMETHING ELSE IS NOT A TABLE. An
    # unknown verb exits 2 with an EMPTY stdout on this tree's binary (measured
    # 2026-09-03), but that is a fact about today's binary, not a property of the
    # design: a build that printed its usage on stdout and exited 0 would hand this
    # hook a page of text to look endings up in, and every ending would come back
    # *nothing classifies it* -- a false finding, which is worse than none.
    said="$(LOOP_READ_SPRAG="$tmp/bin/sprag-chatty" loop_read_gap)"
    case "$said" in
        *"COULD NOT BE ASKED"*"older than the question"*)
            echo "  ok    an answer that is not this table is refused, not searched"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  a build answering something else said: $said"
            fail=$((fail + 1)) ;;
    esac

    # (12d) ⛔⛔⛔ AN ENDING NOTHING CLASSIFIES IS A FINDING, NOT A PASS -- rule 6,
    # and the state item 827 was filed on: a word that reached the log with nobody
    # having decided what follows it.
    said="$(LOOP_READ_SPRAG="$tmp/bin/sprag-partial" loop_read_gap)"
    case "$said" in
        *"CLASSIFIES what happens next after"*"probe#60(failed)"*)
            echo "  ok    an ending no disposition covers is named rather than skipped"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  an unclassified ending said: $said"
            fail=$((fail + 1)) ;;
    esac
    case "$said" in
        *"probe#61 ended 'converged'"*)
            echo "  ok    and the endings that ARE classified still get their answer"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  a classified ending beside an unclassified one said: $said"
            fail=$((fail + 1)) ;;
    esac

    # (12e) ⛔⛔⛔⛔⛔ AND IT IS CALLED THE WAY `pre-push` CALLS IT -- arm (10d)'s
    # lesson, which this file paid a REFUSED PUSH for once: a `grep` that matched
    # nothing killed the hook under `set -euo pipefail`. This clause has two of
    # them.
    bash -c 'set -euo pipefail; . "$1"; LOOP_READ_SPRAG="$2" loop_read_gap >/dev/null' \
        _ "$here/loop-read.sh" "$tmp/bin/sprag"
    rc=$?
    if [ "$rc" -eq 0 ]; then
        echo "  ok    the next-step clause exits 0 under set -euo pipefail"
        pass=$((pass + 1))
    else
        echo "  FAIL  the next-step clause exits ${rc} under set -euo pipefail," \
             "which is a REFUSED PUSH rather than a sentence"
        fail=$((fail + 1))
    fi
    loop_read_seen 'probe#60' >/dev/null
    loop_read_seen 'probe#61' >/dev/null

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
        --next)      loop_read_next_steps "$(loop_read_ended_only "$(loop_read_owed)")" ;;
        --gap|"")    loop_read_gap ;;
        *) echo "usage: loop-read.sh [--gap|--owed|--next|--baseline|--seen KEY|--selftest]" >&2
           exit 2 ;;
    esac
fi
