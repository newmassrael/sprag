#!/usr/bin/env bash
# .githooks/hosted-read.sh — how long this clone has gone without reading a
# hosted result.
#
# ⛔⛔⛔⛔⛔ REGISTER ITEM 776, arms (1), (2) and (4). WHAT THIS MEASURES AND
# WHY IT IS NOT A GATE.
#
# This repository's push is pre-authorised: `CLAUDE.md` says push and continue,
# and read the previous run at the START of the next round. The rule existed and
# was not followed -- CI carried 33 consecutive red runs over two days, and the
# thing that made that possible was not a missing rule but a missing SIGNAL: a
# round that never looked and a round that looked and saw green produce exactly
# the same screen.
#
# ⚠⚠⚠⚠ THE AXIS IS NOT "how many reds were published over". A sibling
# repository was GREEN while nobody had read a hosted result for five rounds --
# same structure, and the green was luck. Counting reds scores those five
# rounds as zero. What has to be counted is THE GAP: the commit whose hosted
# result was last read, against HEAD.
#
# ⚠⚠⚠ AND IT REPORTS RATHER THAN REFUSES, deliberately. Item 776's own
# done-when says so in as many words: the ceiling is not zero, because "push and
# continue" is the INTENT here. A hook that refused would be overturning a
# decision this repository made on purpose. So this prints, every push, and the
# thing that cannot happen is publishing without the number being said.
#
# ⚠⚠ Which makes it a REPORT living beside gates, and that shape is exactly what
# `hooks_cannot_pass_in_silence` hunts -- so it is named as one here rather than
# left to look like a check somebody forgot to wire to an exit status. What IS
# gated is this file's own arithmetic: `hosted_read_selftest` drives every arm,
# and `sprag-gate` drives the hook that calls it.
#
# Self-tested: `bash .githooks/hosted-read.sh --selftest`.

# WHERE THE RECORD LIVES, and why it is not tracked.
#
# It is a fact about THIS CLONE's operator -- what they have looked at -- not
# about the tree, and committing it would make one worker's reading look like
# everybody's. A fresh clone therefore has none, and answers so; that is the
# honest state and not a zero.
#
# ⚠ ABSOLUTE, because `--git-dir` answers a RELATIVE path from inside the work
# tree and every caller here is a subshell that has `cd`-ed somewhere: a
# relative answer would be resolved against whoever asked, which in this file's
# own selftest wrote one repository's marker while reading another's.
hosted_read_marker() {
    printf '%s\n' "$(git rev-parse --absolute-git-dir)/sprag-hosted-read"
}

# ⛔⛔⛔⛔⛔ WHAT AN OPEN GAP COSTS, IN THIS REPOSITORY'S OWN NUMBER — register
# item 776, arm (5), and carried ONLY by the sentence for a gap that has opened.
#
# ⚠⚠⚠⚠⚠ WHY THE ZERO-GAP LINE MUST NOT CARRY IT, which is the whole of arm (5).
#
# Arm (2) made the omission audible. A sibling repository's watcher named why
# that is not yet enough: *"the more often transition notifications come, the
# less they get looked at -- there never seems to be a reason."* A report that
# reads the same whether or not anything is owed becomes one more of those. So
# the two states are not one sentence with a different number in it: a gap of
# zero is a RECEIPT and says only that, and a gap that has opened is a DEBT and
# says what the debt cost here the last time it was left alone.
#
# ⚠⚠ THE BOUNDARY IS ZERO AND NOT A TUNED THRESHOLD. A number somebody chooses
# is an escape hatch with a dial on it -- set it high enough and the loud shape
# never appears. *Anything at all is owed* is the only line this file can draw
# without inventing one.
#
# ⚠ The figure is MEASURED and not rhetoric: CI carried 33 consecutive failures
# across two days while this repository's own rule already said to read the
# previous run at the start of every round.
HOSTED_READ_COST="the last time this gap was left to grow it reached 33 rounds \
and two days of unread red"

# RECORD that the hosted result for `$1` (default HEAD) has been read.
hosted_read_seen() {
    local sha
    sha="$(git rev-parse --verify "${1:-HEAD}^{commit}" 2>/dev/null)" || {
        echo "hosted-read: '${1:-HEAD}' is not a commit in this tree" >&2
        return 1
    }
    printf '%s\n' "$sha" > "$(hosted_read_marker)"
    echo "hosted-read: recorded that the hosted result for ${sha:0:7} was read"
}

# THE SENTENCE. Four states, and none of them may be silent about which it is.
#
# ⛔ The unreadable and the unknown-commit states are NOT folded into "0", which
# is the whole finding one level up: an absence that renders like a measurement
# gets acted on like one.
hosted_read_gap() {
    local marker head recorded count
    marker="$(hosted_read_marker)"
    head="$(git rev-parse --verify HEAD 2>/dev/null || true)"
    if [ -z "$head" ]; then
        echo "hosted-read: this tree has no HEAD, so the gap cannot be read here"
        return 0
    fi
    if [ ! -r "$marker" ]; then
        echo "hosted-read: NOBODY HAS RECORDED READING a hosted result in this" \
             "clone, so how long this has gone unread cannot be read here --" \
             "run '.githooks/hosted-read.sh --seen' after reading one"
        return 0
    fi
    recorded="$(head -n 1 "$marker" | tr -d '[:space:]')"
    if ! git rev-parse --verify --quiet "${recorded}^{commit}" >/dev/null 2>&1; then
        echo "hosted-read: the recorded read names ${recorded:-<empty>}, which" \
             "this tree does not contain -- the gap cannot be read here"
        return 0
    fi
    if [ "$recorded" = "$head" ]; then
        echo "hosted-read: the hosted result was read at HEAD (${head:0:7}) --" \
             "0 round(s) unread"
        return 0
    fi
    count="$(git rev-list --count "${recorded}..HEAD" 2>/dev/null || echo "")"
    if [ -z "$count" ]; then
        echo "hosted-read: ${recorded:0:7} is not an ancestor of HEAD, so the" \
             "gap cannot be read here"
        return 0
    fi
    echo "hosted-read: ${count} round(s) published since a hosted result was" \
         "read (last read at ${recorded:0:7}) -- ${HOSTED_READ_COST}"
}

# ⚠⚠⚠⚠⚠ EVERY ARM, against throwaway repositories -- because a report reachable
# only from a hook cannot otherwise be told apart from one that always says the
# same thing, which is the defect this whole item is about.
hosted_read_selftest() {
    local here tmp pass fail said base tip
    here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    tmp="$(mktemp -d)"
    pass=0
    fail=0

    git -C "$tmp" init -q -b main
    git -C "$tmp" config user.email "probe@example.com"
    git -C "$tmp" config user.name "Probe"
    ( cd "$tmp" && : > a && git add a && git commit -qm base )
    base="$(git -C "$tmp" rev-parse HEAD)"

    said="$( cd "$tmp" && hosted_read_gap )"
    case "$said" in
        *"NOBODY HAS RECORDED READING"*)
            echo "  ok    an unrecorded clone says so rather than answering 0"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  an unrecorded clone said: $said"
            fail=$((fail + 1)) ;;
    esac

    ( cd "$tmp" && hosted_read_seen HEAD >/dev/null )
    said="$( cd "$tmp" && hosted_read_gap )"
    case "$said" in
        *"0 round(s) unread"*)
            echo "  ok    a read recorded at HEAD is 0 round(s)"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  a read at HEAD said: $said"
            fail=$((fail + 1)) ;;
    esac

    ( cd "$tmp" && : > b && git add b && git commit -qm second )
    ( cd "$tmp" && : > c && git add c && git commit -qm third )
    tip="$(git -C "$tmp" rev-parse HEAD)"
    said="$( cd "$tmp" && hosted_read_gap )"
    case "$said" in
        *"2 round(s) published since"*)
            echo "  ok    two commits later reads as 2 round(s)"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  two commits later said: $said"
            fail=$((fail + 1)) ;;
    esac
    case "$said" in
        *"${base:0:7}"*)
            echo "  ok    and it names the commit that was read"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  it does not name ${base:0:7}: $said"
            fail=$((fail + 1)) ;;
    esac

    # ⛔⛔⛔ ARM (5): AN OPEN GAP IS A DEBT AND SAYS WHAT IT COSTS -- and the
    # receipt must NOT, or the report reads the same either way and becomes one
    # more line nobody has a reason to look at.
    case "$said" in
        *"$HOSTED_READ_COST"*)
            echo "  ok    an open gap says what it cost here last time"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  an open gap does not name the cost: $said"
            fail=$((fail + 1)) ;;
    esac
    ( cd "$tmp" && hosted_read_seen HEAD >/dev/null )
    case "$( cd "$tmp" && hosted_read_gap )" in
        *"$HOSTED_READ_COST"*)
            echo "  FAIL  a settled gap is scolding about a debt it does not owe"
            fail=$((fail + 1)) ;;
        *)  echo "  ok    and a settled gap is a receipt, not a debt"
            pass=$((pass + 1)) ;;
    esac
    ( cd "$tmp" && hosted_read_seen "$base" >/dev/null )

    # ⛔ THE UNCLASSIFIED STATE IS NOT A ZERO. A marker naming something this
    # tree does not have is the shape a rebase, a reset or a copied clone
    # leaves behind, and reading it as "0 unread" would be the silent pass.
    ( cd "$tmp" \
      && printf '%s\n' "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef" \
         > "$(hosted_read_marker)" )
    said="$( cd "$tmp" && hosted_read_gap )"
    case "$said" in
        *"cannot be read here"*)
            echo "  ok    a marker this tree does not contain says so"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  an unknown marker said: $said"
            fail=$((fail + 1)) ;;
    esac
    case "$said" in
        *"0 round(s)"*)
            echo "  FAIL  an unknown marker rendered as a zero"
            fail=$((fail + 1)) ;;
        *)  echo "  ok    and it does not render as a zero"
            pass=$((pass + 1)) ;;
    esac

    # ⚠ AND THE FOUR STATES ARE FOUR SENTENCES. One wearing another's words
    # would put a reader in the wrong one without anything going wrong.
    ( cd "$tmp" && hosted_read_seen "$tip" >/dev/null )
    if [ "$( cd "$tmp" && hosted_read_gap )" != "$said" ]; then
        echo "  ok    recording again changes what it says"
        pass=$((pass + 1))
    else
        echo "  FAIL  recording a read did not change the sentence"
        fail=$((fail + 1))
    fi

    # ⛔⛔⛔ CODE ONLY, WITH TRAILING COMMENTS CUT. This file's own reasoning
    # names the function it is about, and `pre-push` quotes its commands in
    # prose the way every hook here does -- so a whole-file grep is satisfied by
    # a COMMENT. Measured while writing this, twice: replacing the call with
    # `: # MUTATION: hosted_read_gap removed` reddened the suite that DRIVES the
    # hook and left this arm green, and dropping only WHOLE comment lines did
    # not fix it because that mutation's comment is a TRAILING one.
    # ⚠ `sed` and not a parser: a `#` inside a quoted string would be cut too.
    # That is this scan's stated limit, the same one
    # `hooks_cannot_pass_in_silence` writes down -- it is why the gate that
    # DRIVES the hook is the one that decides, and this arm is a wiring check.
    if command sed 's/#.*//' "$here/pre-push" \
       | command grep -q 'hosted_read_gap'; then
        echo "  ok    the pre-push hook calls the gap arm"
        pass=$((pass + 1))
    else
        echo "  FAIL  pre-push never calls the gap arm"
        fail=$((fail + 1))
    fi

    rm -rf "$tmp"
    echo "hosted-read selftest: ${pass}/$((pass + fail)) arm(s) pass"
    [ "$fail" -eq 0 ]
}

if [ "${BASH_SOURCE[0]}" = "${0}" ]; then
    case "${1:-}" in
        --selftest) hosted_read_selftest ;;
        --seen) shift; hosted_read_seen "${1:-HEAD}" ;;
        --gap|"") hosted_read_gap ;;
        *) echo "usage: hosted-read.sh [--gap|--seen [SHA]|--selftest]" >&2; exit 2 ;;
    esac
fi
