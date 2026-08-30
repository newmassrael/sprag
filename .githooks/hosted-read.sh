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

# ⛔⛔⛔⛔⛔ WHAT A LOOK THAT FOUND NO VERDICT COSTS — register item 779, and a
# SECOND sentence rather than a number added to the one above.
#
# ⚠⚠⚠⚠⚠ Two debts, two clauses, because the remedies differ. An open gap is
# *nobody looked*; this is *somebody looked and there was nothing there yet*, and
# what it asks for is going BACK to a commit already under the watermark. A
# reader handed one sentence for both would come away thinking the second was
# discharged by the same act as the first, which is exactly the covering this
# item is about.
#
# ⚠ MEASURED, and it is the queue this repository actually has rather than an
# imagined one: on 2026-08-30 three pushes were outstanding at once, the oldest
# run still `in_progress` an hour and three quarters after it was created and
# the two behind it `queued`. Nothing in this repository serialises them --
# `.github/workflows` declares no `concurrency:` group -- so the wait is the
# hosting's and cannot be shortened from here. A watermark that assumes a
# verdict is waiting is therefore wrong on an ORDINARY round, not a rare one.
HOSTED_READ_UNSETTLED_COST="a run that had not spoken when it was looked at is \
not a run that was read, and the next --seen would bury it"

# RECORD what was found when the hosted result for `$1` (default HEAD) was
# looked at: `$2` is `settled` (a verdict was there and was read) or `unsettled`
# (there was none yet).
#
# ⛔⛔⛔⛔⛔ THE VERDICT WORD IS REQUIRED — register item 779, and refusing the
# bare form is the whole repair.
#
# `--seen SHA` used to write a watermark and nothing else, which made *I read a
# verdict* and *I looked and there was none yet* the SAME act. A reader who
# looks at the top of a round and finds `queued` can stamp that honestly -- they
# really did look -- and every run under the mark is then covered for ever. The
# unit was wrong: the file counts COMMITS and what has to be read is RUNS, and
# a push that lands before the last run has spoken makes those two differ.
#
# ⚠⚠ So an unclassified look is REFUSED rather than defaulted to either word.
# Defaulting to `settled` is the shipped defect; defaulting to `unsettled` would
# make an honest reader's mark stop counting and turn the report into noise.
# This workspace's rule is that an unclassified case is RED and not a pass, and
# here the classification is a fact only the reader has.
#
# ⚠ An `unsettled` mark does NOT move the watermark: nothing was read, so the
# distance the gap measures has not changed. It is remembered separately, and
# `hosted_read_gap` keeps naming it until that commit is marked `settled`.
hosted_read_seen() {
    local sha verdict marker kept mark
    sha="$(git rev-parse --verify "${1:-HEAD}^{commit}" 2>/dev/null)" || {
        echo "hosted-read: '${1:-HEAD}' is not a commit in this tree" >&2
        return 1
    }
    verdict="${2:-}"
    case "$verdict" in
        settled|unsettled) ;;
        *)  echo "hosted-read: say WHAT was found -- '--seen ${1:-HEAD} settled'" \
                 "if a verdict was there and you read it, '--seen ${1:-HEAD}" \
                 "unsettled' if the run had not spoken yet. A look that found" \
                 "nothing is not a read, and recording it as one buries that" \
                 "run under the mark for good (register item 779)." >&2
            return 2 ;;
    esac
    marker="$(hosted_read_marker)"
    # ⛔⛔⛔⛔⛔ BOTH HALVES OF THE OLD FILE ARE READ BEFORE ONE BYTE IS WRITTEN.
    # `> "$marker"` truncates when the group STARTS, so a `$(...)` inside it that
    # reads the marker reads an empty file — measured by this file's own selftest
    # on the first run of this arm: an unsettled look wiped the watermark and the
    # clone went back to saying nobody had ever read anything.
    #
    # ⚠ THE WATERMARK STAYS PUT for an unsettled look, and an absent one stays
    # absent: a clone where nothing has ever been READ must go on saying so
    # rather than acquiring a mark from a look that read nothing.
    mark="$(hosted_read_watermark)"
    # ⛔⛔⛔⛔⛔ **THE MARK NEVER MOVES BACKWARD** — register item 779, measured on this repository's
    # own marker the round after it was built. Two runs came back at once and were settled oldest
    # LAST, so `--seen <older> settled` put the mark behind a commit that had already been read and
    # the report said THREE rounds unread where two was the truth. A watermark that can regress is
    # not a watermark; it over-states, which is the safe direction and is why nothing was buried,
    # but a number nobody can trust in either direction stops being read at all.
    #
    # ⚠ Four cases and none of them defaults (this workspace's rule that unclassified is RED):
    # nothing recorded yet takes the new sha; a mark this branch no longer contains is REPLACED,
    # or a rebase would strand the reading for ever with no way back to zero; a sha at or behind
    # the mark leaves it alone — the read still counts, and `kept` below still clears what it owed;
    # anything else is forward and moves it.
    if [ "$verdict" = settled ]; then
        if [ -z "$mark" ] \
           || ! git merge-base --is-ancestor "$mark" HEAD 2>/dev/null \
           || ! git merge-base --is-ancestor "$sha" "$mark" 2>/dev/null; then
            mark="$sha"
        fi
    fi
    # Every commit still owed, MINUS the one being recorded now — whichever word
    # it got, this look is the current word about that commit.
    kept="$(hosted_read_owed | command grep -v "^${sha}$" || true)"
    if [ "$verdict" = unsettled ]; then
        kept="$(printf '%s\n%s\n' "$kept" "$sha" | command grep -v '^$' || true)"
    fi
    {
        printf '%s\n' "$mark"
        printf '%s\n' "$kept" | command sed '/^$/d;s/^/owed /'
    } > "$marker"
    if [ "$verdict" = settled ]; then
        echo "hosted-read: recorded that the hosted result for ${sha:0:7} was read"
    else
        echo "hosted-read: recorded that ${sha:0:7} was looked at and its run had" \
             "not spoken yet -- it stays owed until '--seen ${sha:0:7} settled'"
    fi
}

# The watermark line, or empty where no read has ever been recorded.
hosted_read_watermark() {
    local marker
    marker="$(hosted_read_marker)"
    [ -r "$marker" ] || return 0
    command head -n 1 "$marker" | command tr -d '[:space:]'
}

# Every commit LOOKED AT whose run had not spoken, one sha per line — and only
# those this tree still contains on HEAD's own history.
#
# ⛔ THE PRUNE IS A CLASSIFIED DROP AND NOT A CONVENIENCE. A commit that is no
# longer an ancestor of HEAD was rebased or reset away, so no verdict about it
# is a verdict about anything this branch will publish — and without the prune
# the owed list could never reach empty, which is the shape that makes a count
# stop being actionable. It is the same reading `hosted_read_gap` gives a
# watermark this tree does not contain.
hosted_read_owed() {
    local marker sha
    marker="$(hosted_read_marker)"
    [ -r "$marker" ] || return 0
    command sed -n 's/^owed //p' "$marker" | while read -r sha; do
        [ -n "$sha" ] || continue
        git merge-base --is-ancestor "$sha" HEAD 2>/dev/null && printf '%s\n' "$sha"
    done
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
             "run '.githooks/hosted-read.sh --seen HEAD settled' after reading one"
        return 0
    fi
    recorded="$(head -n 1 "$marker" | tr -d '[:space:]')"
    if [ -z "$recorded" ]; then
        # ⛔ A MARKER THAT HOLDS ONLY OWED COMMITS — register item 779. A clone
        # where the only look so far found no verdict has read NOTHING, and
        # saying otherwise is the covering this item exists to stop. It reads as
        # the unrecorded state, plus what is owed.
        echo "hosted-read: NOBODY HAS RECORDED READING a hosted result in this" \
             "clone, so how long this has gone unread cannot be read here" \
             "--$(hosted_read_owed_clause)"
        return 0
    fi
    if ! git rev-parse --verify --quiet "${recorded}^{commit}" >/dev/null 2>&1; then
        echo "hosted-read: the recorded read names ${recorded:-<empty>}, which" \
             "this tree does not contain -- the gap cannot be read here"
        return 0
    fi
    if [ "$recorded" = "$head" ]; then
        # ⛔⛔⛔⛔⛔ A RECEIPT ONLY WHEN NOTHING IS OWED — register item 779. The
        # gap being zero says every commit since the mark was read; it says
        # nothing about a commit UNDER the mark whose run had not spoken when
        # somebody looked. Those two were one sentence, and that is how a run
        # that never answered got buried by the next `--seen`.
        echo "hosted-read: the hosted result was read at HEAD (${head:0:7}) --" \
             "0 round(s) unread$(hosted_read_owed_clause)"
        return 0
    fi
    count="$(git rev-list --count "${recorded}..HEAD" 2>/dev/null || echo "")"
    if [ -z "$count" ]; then
        echo "hosted-read: ${recorded:0:7} is not an ancestor of HEAD, so the" \
             "gap cannot be read here"
        return 0
    fi
    echo "hosted-read: ${count} round(s) published since a hosted result was" \
         "read (last read at ${recorded:0:7}) -- ${HOSTED_READ_COST}$(hosted_read_owed_clause)"
}

# The clause naming what was LOOKED AT and had not spoken — empty when nothing
# is owed, which is what keeps a receipt a receipt (register item 776, arm 5).
hosted_read_owed_clause() {
    local owed count listed
    owed="$(hosted_read_owed)"
    [ -n "$owed" ] || return 0
    count="$(printf '%s\n' "$owed" | command grep -c .)"
    listed="$(printf '%s\n' "$owed" | command cut -c1-7 | command tr '\n' ' ')"
    printf '%s' "; and ${count} commit(s) were looked at before their runs had \
spoken (${listed% }) -- ${HOSTED_READ_UNSETTLED_COST}"
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

    ( cd "$tmp" && hosted_read_seen HEAD settled >/dev/null )
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
    ( cd "$tmp" && hosted_read_seen HEAD settled >/dev/null )
    case "$( cd "$tmp" && hosted_read_gap )" in
        *"$HOSTED_READ_COST"*)
            echo "  FAIL  a settled gap is scolding about a debt it does not owe"
            fail=$((fail + 1)) ;;
        *)  echo "  ok    and a settled gap is a receipt, not a debt"
            pass=$((pass + 1)) ;;
    esac

    # ⛔⛔⛔⛔⛔ REGISTER ITEM 779 — THE SIX ARMS THE WATERMARK ALONE CANNOT HOLD
    # (a seventh, the prune, has its own block below).
    #
    # The mark used to say only THAT something was read. A reader at the top of a
    # round who finds the run still `queued` has looked honestly and can stamp it
    # honestly, and every run under the mark is then covered for good. So the
    # word is required, an unsettled look does not move the mark, and a later
    # settled read does not bury it.
    case "$( cd "$tmp" && hosted_read_seen HEAD 2>&1 >/dev/null )" in
        *"say WHAT was found"*)
            echo "  ok    a look that does not say what it found is refused"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  a bare --seen was accepted"
            fail=$((fail + 1)) ;;
    esac
    # ⚠ THE MARK IS STAGED BY WRITING THE FILE, not by `--seen`, and that is not a shortcut: the
    # mark never moves BACKWARD (see `hosted_read_seen`), so a recorder call cannot put this clone
    # behind where it already is. The file IS the state, which is what the `sprag-gate` suite
    # stages with too.
    ( cd "$tmp" && printf '%s\n' "$base" > "$(hosted_read_marker)" )
    ( cd "$tmp" && hosted_read_seen "$tip" unsettled >/dev/null )
    said="$( cd "$tmp" && hosted_read_gap )"
    case "$said" in
        *"2 round(s) published since"*)
            echo "  ok    a look that found no verdict does not move the mark"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  an unsettled look moved the watermark: $said"
            fail=$((fail + 1)) ;;
    esac
    ( cd "$tmp" && hosted_read_seen "$tip" settled >/dev/null )
    said="$( cd "$tmp" && hosted_read_gap )"
    case "$said" in
        *"0 round(s) unread"*)
            echo "  ok    reading that verdict later settles it"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  a settled read did not clear its own owed mark: $said"
            fail=$((fail + 1)) ;;
    esac
    case "$said" in
        *"$HOSTED_READ_UNSETTLED_COST"*)
            echo "  FAIL  a cleared debt is still being scolded about: $said"
            fail=$((fail + 1)) ;;
        *)  echo "  ok    and nothing is owed once it has been read"
            pass=$((pass + 1)) ;;
    esac

    # ⛔⛔⛔ THE ARM THIS ITEM IS: A LATER READ MUST NOT BURY AN EARLIER LOOK.
    ( cd "$tmp" && hosted_read_seen "$base" unsettled >/dev/null )
    ( cd "$tmp" && hosted_read_seen "$tip" settled >/dev/null )
    said="$( cd "$tmp" && hosted_read_gap )"
    case "$said" in
        *"0 round(s) unread"*"$HOSTED_READ_UNSETTLED_COST"*)
            echo "  ok    a gap of zero still names a run that never spoke"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  a later read buried an unsettled look: $said"
            fail=$((fail + 1)) ;;
    esac
    case "$said" in
        *"${base:0:7}"*)
            echo "  ok    and it names WHICH commit is still owed"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  the owed commit is not named: $said"
            fail=$((fail + 1)) ;;
    esac

    # ⛔⛔⛔ AND THE MARK NEVER GOES BACKWARD — register item 779's own residue, measured on this
    # repository's marker the round after it shipped: two runs finished together, the older was
    # settled LAST, and the report claimed a round more unread than there was.
    ( cd "$tmp" && hosted_read_seen "$tip" settled >/dev/null )
    ( cd "$tmp" && hosted_read_seen "$base" settled >/dev/null )
    said="$( cd "$tmp" && hosted_read_gap )"
    case "$said" in
        *"0 round(s) unread"*)
            echo "  ok    settling an older commit does not move the mark back"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  an older settled read moved the mark backward: $said"
            fail=$((fail + 1)) ;;
    esac

    # ⛔⛔ AND THE DEBT CAN REACH ZERO BY A ROAD THAT IS NOT A READ — an owed
    # commit this branch no longer contains. Without the prune the list could
    # never empty after a rebase, and a count that cannot reach zero is one
    # nobody can act on. It is a CLASSIFIED drop, so it gets an arm of its own
    # rather than being a silent filter.
    ( cd "$tmp" && printf '%s\nowed %s\n' "$tip" \
        "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef" > "$(hosted_read_marker)" )
    said="$( cd "$tmp" && hosted_read_gap )"
    case "$said" in
        *"$HOSTED_READ_UNSETTLED_COST"*)
            echo "  FAIL  a commit this tree does not contain is owed for ever: $said"
            fail=$((fail + 1)) ;;
        *)  echo "  ok    an owed commit this branch dropped stops being owed"
            pass=$((pass + 1)) ;;
    esac
    ( cd "$tmp" && hosted_read_seen "$base" settled >/dev/null )

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
    ( cd "$tmp" && hosted_read_seen "$tip" settled >/dev/null )
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
        --seen) shift; hosted_read_seen "${1:-HEAD}" "${2:-}" ;;
        --gap|"") hosted_read_gap ;;
        *) echo "usage: hosted-read.sh [--gap|--seen SHA <settled|unsettled>|--selftest]" >&2
           exit 2 ;;
    esac
fi
