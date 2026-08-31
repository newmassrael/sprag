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

# ⛔⛔⛔⛔⛔ WHAT A COMMIT THE MARK JUMPED OVER COSTS — register item 781, and a
# THIRD sentence for the same reason the second one exists: the remedy differs
# again. An open gap is *nobody looked*; an unsettled mark is *somebody looked
# and there was nothing there*; this is *a verdict was there, it was never
# looked at, and the mark went past it anyway*.
#
# ⚠⚠⚠⚠⚠ THE UNIT IS WRONG ONE MORE TIME, and that is the whole of this item. Item
# 779 separated COMMITS from RUNS. What is left is that a watermark is a LINE and
# what has to be settled is a SET: `--seen <sha> settled` covers everything under
# it by construction, so a commit between the old mark and the new one is
# declared read by an act that never looked at it. **Any design that moves a line
# has this hole** — so the line is kept for the distance it measures, and the
# commits it steps over are recorded one by one.
#
# ⚠ MEASURED, on this repository's own marker: `7b71077`'s macOS job was RED (a
# pty-exhaustion refusal and a plugin readiness assertion), the mark advanced to
# `69a46db`, the commits in between were `success` so the distance was zero — and
# the report read `0 round(s) unread` with the red nowhere in it. It was found by
# a person following a rule, which is exactly the thing this file exists to stop
# being the only mechanism.
HOSTED_READ_SKIPPED_COST="a verdict that was there and was never looked at is \
the one this mark used to step over in silence -- 7b71077's macOS job was red \
while the gap read 0"

# ⛔⛔⛔⛔⛔ WHAT A COMMIT WITH NO RUN OF ITS OWN COSTS — register item 790, and a
# FOURTH sentence for the reason the third one exists: the remedy differs again,
# and this time there is no remedy at all to hand the reader.
#
# ⚠⚠⚠⚠⚠ THE CORRESPONDENCE IS EMPTY, which is the whole of this item and the
# fourth way the unit has been wrong. Item 779 separated COMMITS from RUNS
# (one-to-many); item 781 separated a LINE from the SET beneath it (order). Here
# the commit maps to NOTHING: GitHub hangs a workflow run on the TIP of a push,
# so a commit published underneath one never gets a run at all.
#
# ⚠⚠ AND NEITHER WORD CAN PAY IT OFF. `settled` would claim a verdict that does
# not exist; `unsettled` waits for a run that will never speak, so the commit
# sits in the list FOR EVER -- and a count that cannot reach zero stops being
# acted on, which is this file's own reason for pruning elsewhere.
#
# ⚠ MEASURED on this repository, 2026-08-31: `0642aa7` was pushed together with
# `c772057`, `actions/runs?head_sha=` answers `total_count` 0 for it and 1 for
# the tip, and the push still read *nobody has looked at their runs at all* --
# a true sentence about an absence, pointing a reader at nothing to look at.
HOSTED_READ_NORUN_COST="a commit published underneath another one never gets a \
run, so neither word can retire it -- 0642aa7 answered total_count 0 while the \
report sent readers to look at its run"

# ⛔⛔⛔ WHAT AN UNANSWERED QUESTION COSTS — register item 790, and the arm that
# keeps the repair from becoming its own escape hatch.
#
# ⚠ *It had no run* and *nobody could ask* are different facts, and this
# workspace's rule is that an unclassified case is RED rather than a pass. So a
# commit the asking could not reach keeps its place in the list and says why,
# instead of being folded into either of the two answers that were measured.
HOSTED_READ_UNASKED_COST="a commit nobody could ask about is not a commit that \
had no run -- folding the two together is the covering this file exists to stop"

# WHETHER `$1` EVER HAD A HOSTED RUN AT ALL: a count, or `unknown` where the
# question could not be put — register item 790.
#
# ⛔⛔ IT ASKS RATHER THAN RESTING ON A RULE. "push one commit at a time" is a
# mitigation leaning on a person's memory, which item 790 rules out in as many
# words. Whether a run exists is GitHub's fact, so this asks GitHub for it.
#
# ⚠⚠ THREE ANSWERS AND NONE OF THEM A DEFAULT. A number is what was found; `0`
# is a measured absence; `unknown` is no answer at all — an absent `gh`, a
# refused call, a reply that is not a count. The third keeps its own word so a
# caller cannot mistake *not asked* for *asked and found none*.
#
# ⚠ `{owner}/{repo}` is `gh`'s own substitution from the checkout it is run in,
# so this file names no repository and works in a clone under any name.
hosted_read_runs_for() {
    local sha count
    sha="$1"
    command -v gh >/dev/null 2>&1 || { printf 'unknown\n'; return 0; }
    count="$(gh api "repos/{owner}/{repo}/actions/runs?head_sha=${sha}" \
                 --jq '.total_count' 2>/dev/null)" || count=""
    case "$count" in
        '' | *[!0-9]*) printf 'unknown\n' ;;
        *)             printf '%s\n' "$count" ;;
    esac
}

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
    local sha verdict marker kept mark was passed skipped one acc gone listed
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
    was="$mark"
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
    passed=""
    if [ "$verdict" = settled ]; then
        if [ -z "$mark" ] \
           || ! git merge-base --is-ancestor "$mark" HEAD 2>/dev/null \
           || ! git merge-base --is-ancestor "$sha" "$mark" 2>/dev/null; then
            # ⛔⛔⛔⛔⛔ EVERY COMMIT THE MARK STEPS OVER IS NAMED — register item
            # 781. A line that advances declares everything beneath it read, and
            # the act that advanced it looked at exactly ONE run. So the jump is
            # enumerated here, at the only moment the two endpoints are both
            # known, and each commit in between becomes its own debt.
            #
            # ⚠ ONLY FOR AN ORDINARY FORWARD MOVE. A clone with no mark is
            # starting to track, not stepping over its own history; a mark this
            # branch no longer contains was rebased away, and no commit can be
            # attributed across that. Both take the mark and record nothing —
            # which is a CLASSIFIED silence, not a default.
            if [ -n "$was" ] \
               && git merge-base --is-ancestor "$was" "$sha" 2>/dev/null; then
                passed="$(git rev-list "${was}..${sha}" 2>/dev/null \
                          | command grep -v "^${sha}$" || true)"
            fi
            mark="$sha"
        fi
    fi
    # Every commit still owed, MINUS the one being recorded now — whichever word
    # it got, this look is the current word about that commit.
    kept="$(hosted_read_owed | command grep -v "^${sha}$" || true)"
    if [ "$verdict" = unsettled ]; then
        kept="$(printf '%s\n%s\n' "$kept" "$sha" | command grep -v '^$' || true)"
    fi
    # Every commit still stepped over, plus the ones this move just stepped over.
    #
    # ⚠ A commit that was LOOKED AT is not one nobody looked at, so `kept` wins:
    # the two debts have different remedies and a commit carrying both would be
    # reported twice for one absence. And the one being recorded now leaves the
    # list whichever word it got — this look is the current word about it.
    skipped="$(hosted_read_skipped | command grep -v "^${sha}$" || true)"
    acc=""
    gone=""
    while read -r one; do
        [ -n "$one" ] || continue
        case "$acc" in *"$one"*) continue ;; esac
        printf '%s\n' "$kept" | command grep -qx "$one" && continue
        # ⛔⛔⛔⛔⛔ A COMMIT THAT NEVER HAD A RUN LEAVES THE LIST HERE — register
        # item 790, and this is the only place the list can reach zero. Neither
        # word retires such a commit (see `HOSTED_READ_NORUN_COST`), so it would
        # otherwise sit here for ever pointing readers at nothing.
        #
        # ⚠⚠ IT IS DROPPED OUT LOUD, never in silence: the sentence below names
        # each one. A silent prune here would be item 781's own defect wearing
        # this item's clothes — the mark quietly declaring something handled.
        #
        # ⚠ ONLY A MEASURED `0` DROPS ANYTHING. `unknown` stays, because *not
        # asked* is not *asked and found none*, and an unclassified case is RED
        # in this workspace rather than a pass.
        if [ "$(hosted_read_runs_for "$one")" = 0 ]; then
            case "$gone" in *"$one"*) continue ;; esac
            gone="${gone}${one}
"
            continue
        fi
        acc="${acc}${one}
"
    done <<SEEN
$skipped
$passed
SEEN
    skipped="$acc"
    {
        printf '%s\n' "$mark"
        printf '%s\n' "$kept" | command sed '/^$/d;s/^/owed /'
        printf '%s\n' "$skipped" | command sed '/^$/d;s/^/skipped /'
    } > "$marker"
    if [ "$verdict" = settled ]; then
        echo "hosted-read: recorded that the hosted result for ${sha:0:7} was read"
    else
        echo "hosted-read: recorded that ${sha:0:7} was looked at and its run had" \
             "not spoken yet -- it stays owed until '--seen ${sha:0:7} settled'"
    fi
    # ⚠ SAID SEPARATELY FROM THE RECORD ABOVE, because it is a different act: the
    # line above is what this reader looked at, and this is what the asking found
    # about commits nobody can ever look at. Silent would be the defect.
    if [ -n "$gone" ]; then
        listed="$(printf '%s\n' "$gone" | command sed '/^$/d' \
                  | command cut -c1-7 | command tr '\n' ' ')"
        echo "hosted-read: $(printf '%s\n' "$gone" | command grep -c .) commit(s)" \
             "the mark stepped over never had a hosted run of their own" \
             "(${listed% }), so there is nothing to read and they are dropped --" \
             "${HOSTED_READ_NORUN_COST}"
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

# Every commit THE MARK STEPPED OVER whose run nobody ever looked at, one sha per
# line — register item 781, and the same ancestry prune `hosted_read_owed` gets
# and for the same reason: a commit this branch dropped is not a debt, and a
# count that cannot reach zero stops being acted on.
hosted_read_skipped() {
    local marker sha
    marker="$(hosted_read_marker)"
    [ -r "$marker" ] || return 0
    command sed -n 's/^skipped //p' "$marker" | while read -r sha; do
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
             "--$(hosted_read_owed_clause)$(hosted_read_skipped_clause)"
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
             "0 round(s) unread$(hosted_read_owed_clause)$(hosted_read_skipped_clause)"
        return 0
    fi
    count="$(git rev-list --count "${recorded}..HEAD" 2>/dev/null || echo "")"
    if [ -z "$count" ]; then
        echo "hosted-read: ${recorded:0:7} is not an ancestor of HEAD, so the" \
             "gap cannot be read here"
        return 0
    fi
    echo "hosted-read: ${count} round(s) published since a hosted result was" \
         "read (last read at ${recorded:0:7}) -- ${HOSTED_READ_COST}$(hosted_read_owed_clause)$(hosted_read_skipped_clause)"
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

# The clause naming what the mark WENT PAST — empty when nothing was stepped
# over, for the same reason the owed clause is (register item 776, arm 5): a
# sentence that reads the same either way is one nobody has a reason to read.
#
# ⚠ A THIRD CLAUSE AND NOT A THIRD NUMBER IN THE FIRST ONE. The three states are
# *nobody looked*, *somebody looked at a run that had not spoken*, and *the mark
# went past a run that had*. A reader handed one sentence for two of them comes
# away thinking one act discharges both, which is the covering item 779 was
# about — and this is its third face, not a bigger version of it.
#
# ⛔⛔⛔⛔⛔ AND IT IS THREE CLAUSES, NOT ONE — register item 790. *Nobody looked
# at a run that is there*, *there is no run to look at*, and *nobody could ask*
# are three different states with three different remedies, and the first
# sentence was printed over all three. A reader sent to read `0642aa7`'s run
# finds nothing, stops, and the commit stays in the list for ever.
hosted_read_skipped_clause() {
    local classified
    classified="$(hosted_read_skipped_classified)"
    [ -n "$classified" ] || return 0
    hosted_read_skipped_arm "$classified" unread \
        "were STEPPED OVER by the mark and nobody has looked at their runs at all" \
        "$HOSTED_READ_SKIPPED_COST"
    hosted_read_skipped_arm "$classified" norun \
        "the mark stepped over never had a hosted run of their own, so there is nothing to read" \
        "$HOSTED_READ_NORUN_COST"
    hosted_read_skipped_arm "$classified" unasked \
        "the mark stepped over could not be asked whether they ever had a run" \
        "$HOSTED_READ_UNASKED_COST"
}

# Every stepped-over commit paired with what ASKING found — `unread`, `norun` or
# `unasked`, one `<word> <sha>` per line (register item 790).
#
# ⚠ The three words are the three answers `hosted_read_runs_for` can give, so a
# commit cannot fall between them and arrive in no clause at all.
hosted_read_skipped_classified() {
    local sha found
    hosted_read_skipped | while read -r sha; do
        [ -n "$sha" ] || continue
        found="$(hosted_read_runs_for "$sha")"
        case "$found" in
            0)       printf 'norun %s\n' "$sha" ;;
            unknown) printf 'unasked %s\n' "$sha" ;;
            *)       printf 'unread %s\n' "$sha" ;;
        esac
    done
}

# One clause of `hosted_read_skipped_clause`, or nothing where that word claims
# no commit — the emptiness rule arm (5) established, applied per state.
hosted_read_skipped_arm() {
    local classified word saying cost list count listed
    classified="$1"
    word="$2"
    saying="$3"
    cost="$4"
    list="$(printf '%s\n' "$classified" | command sed -n "s/^${word} //p")"
    [ -n "$list" ] || return 0
    count="$(printf '%s\n' "$list" | command grep -c .)"
    listed="$(printf '%s\n' "$list" | command cut -c1-7 | command tr '\n' ' ')"
    printf '%s' "; and ${count} commit(s) ${saying} (${listed% }) -- ${cost}"
}

# ⚠⚠⚠⚠⚠ EVERY ARM, against throwaway repositories -- because a report reachable
# only from a hook cannot otherwise be told apart from one that always says the
# same thing, which is the defect this whole item is about.
hosted_read_selftest() {
    # ⛔⛔⛔ `PATH` IS SAVED AND RESTORED, NEVER `local` — measured on the first
    # run of the double below: `local PATH` starts the variable EMPTY, so `here`
    # was computed with no `dirname` on it and every arm then read the real
    # repository instead of the throwaway one. The arms went green-ish against
    # the wrong subject, which is the shape this whole file is about.
    local here tmp pass fail said base tip saved_path FAKE_GH_TOTAL FAKE_GH_FAIL
    saved_path="$PATH"
    here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    tmp="$(mktemp -d)"
    pass=0
    fail=0

    # ⛔⛔⛔⛔⛔ NOTHING BELOW RUNS BEFORE THERE IS A SCRATCH DIRECTORY TO RUN IN —
    # measured 2026-08-31, and it is FIRST in the function because every line
    # after it treats `$tmp` as a place and writes there.
    #
    # A `local PATH` in this function started the variable EMPTY, so `mktemp` was
    # not found and `$tmp` was the empty string. What followed, in order, against
    # the REAL repository this process was standing in: `mkdir -p "$tmp/bin"`
    # became `mkdir -p /bin`; `git -C "" init` RE-INITIALISED it; `git -C ""
    # config user.email probe@example.com` wrote a `[user]` section into its
    # `.git/config`, replacing the operator's identity with the harness's on a
    # tree whose next commit would have carried it; and the arms then overwrote
    # the operator's marker, moving the watermark onto a commit whose run had not
    # spoken and erasing a stepped-over one.
    #
    # ⚠⚠ `git -C ""` is the mechanism worth naming: git reads an empty `-C` as
    # *stay where you are*, so a scratch-repository command silently becomes a
    # command against the caller's own repository. Nothing warns.
    #
    # ⚠ It STOPS rather than scoring an arm: a run that is not standing in its
    # own subject has no verdict to give about anything.
    if [ -z "$tmp" ] || [ ! -d "$tmp" ]; then
        echo "hosted-read selftest: REFUSING to run -- mktemp gave no scratch" \
             "directory ('${tmp:-<empty>}'), and every line below would write" \
             "into $(pwd) instead" >&2
        return 1
    fi

    # ⛔⛔⛔⛔⛔ THE ASKING IS A DOUBLE HERE — register item 790. `hosted_read_runs_for`
    # puts its question to `gh`, and a selftest that let it reach GitHub would
    # answer differently on a laptop with no network, in a throwaway repository
    # with no origin, and under a rate limit — which is a harness that measures
    # the weather. The double is a `gh` on PATH, so the SUBJECT is untouched: the
    # file names no environment variable of its own and there is no arm to
    # disable the asking with. `1` is the default because it is the world the
    # arms written before this item assume — a stepped-over commit whose run is
    # there and unread.
    mkdir -p "$tmp/bin"
    cat > "$tmp/bin/gh" <<'ASKED'
#!/usr/bin/env bash
# The double `hosted-read.sh --selftest` asks instead of GitHub (item 790).
[ "${FAKE_GH_FAIL:-0}" = 1 ] && exit 1
printf '%s\n' "${FAKE_GH_TOTAL:-1}"
ASKED
    chmod +x "$tmp/bin/gh"
    PATH="$tmp/bin:$PATH"
    FAKE_GH_TOTAL=1
    FAKE_GH_FAIL=0
    export PATH FAKE_GH_TOTAL FAKE_GH_FAIL

    git -C "$tmp" init -q -b main
    git -C "$tmp" config user.email "probe@example.com"
    git -C "$tmp" config user.name "Probe"
    ( cd "$tmp" && : > a && git add a && git commit -qm base )
    base="$(git -C "$tmp" rev-parse HEAD)"

    # ⛔⛔⛔⛔⛔ THE SUBJECT IS THE SCRATCH REPOSITORY OR THIS DOES NOT RUN AT ALL
    # — measured 2026-08-31, and it is the SECOND time this file has written
    # somebody else's marker (`hosted_read_marker`'s own note records the first,
    # which is why that path is absolute).
    #
    # A `local PATH` in this function started the variable EMPTY, so `mktemp` and
    # `git` were not found the way every line above assumes, and the arms ran
    # against the REAL repository: they overwrote the operator's marker, moved
    # the watermark onto a commit whose run had not spoken, and erased a
    # stepped-over commit. **A harness that can write outside its subject can
    # destroy exactly the record this file exists to keep** — and it did.
    #
    # ⚠⚠ IT IS CHECKED, NOT ASSUMED. Every arm below reaches the marker through
    # `hosted_read_marker`, which answers whatever repository the process is
    # standing in; the one thing that makes those answers safe is that this
    # process is standing in `$tmp`. So that is asserted once, here, before any
    # arm writes — and a failure STOPS rather than scoring, because a run that is
    # not testing what it thinks it is has no verdict to give.
    #
    # ⚠ The comparison is the git dir the scratch repository itself reports, so
    # a `$tmp` that is a symlink or was never initialised both fail it.
    if [ -z "$tmp" ] || [ ! -d "$tmp" ] \
       || [ "$( cd "$tmp" && git rev-parse --absolute-git-dir 2>/dev/null )" \
            != "$( cd "$tmp" && pwd )/.git" ]; then
        echo "hosted-read selftest: REFUSING to run -- the scratch repository" \
             "'${tmp:-<empty>}' is not this process's git dir, so every arm" \
             "would read and WRITE $(git rev-parse --absolute-git-dir \
             2>/dev/null || echo "some other repository")'s marker instead" >&2
        PATH="$saved_path"
        export PATH
        return 1
    fi

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

    # ⛔⛔⛔⛔⛔ REGISTER ITEM 781 — THE MARK STEPS OVER COMMITS AND SAYS NOTHING.
    #
    # Measured on this repository's own marker: `7b71077`'s macOS job was red, the mark advanced
    # past it to `69a46db`, the commits in between were green so the DISTANCE was zero, and the
    # report said `0 round(s) unread`. A person following a rule found the red; nothing on the
    # screen did. So the fixture stages exactly that: a mark two commits back, one `--seen` at the
    # tip, and the question is what the two in the middle look like afterwards.
    ( cd "$tmp" && printf '%s\n' "$base" > "$(hosted_read_marker)" )
    ( cd "$tmp" && : > d && git add d && git commit -qm fourth )
    local jumped
    jumped="$(git -C "$tmp" rev-parse HEAD~1)"
    ( cd "$tmp" && hosted_read_seen HEAD settled >/dev/null )
    said="$( cd "$tmp" && hosted_read_gap )"
    case "$said" in
        *"STEPPED OVER"*)
            echo "  ok    a mark that jumped names the commits it jumped"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  a jump over three commits was silent: $said"
            fail=$((fail + 1)) ;;
    esac
    case "$said" in
        *"${jumped:0:7}"*)
            echo "  ok    and it names WHICH commit was stepped over"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  the stepped-over commit ${jumped:0:7} is not named: $said"
            fail=$((fail + 1)) ;;
    esac
    case "$said" in
        *"$HOSTED_READ_SKIPPED_COST"*)
            echo "  ok    and it says what a stepped-over verdict cost here"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  a stepped-over commit does not name the cost: $said"
            fail=$((fail + 1)) ;;
    esac
    # ⛔ AND THE ZERO-GAP RECEIPT IS NOT A RECEIPT WHILE ONE IS OUTSTANDING — the same shape arm
    # (5) and item 779 each needed, and the reason this is a THIRD clause rather than a number.
    case "$said" in
        *"0 round(s) unread"*"STEPPED OVER"*)
            echo "  ok    a gap of zero still names what the mark went past"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  the settled gap swallowed the commits it stepped over: $said"
            fail=$((fail + 1)) ;;
    esac
    # ⛔⛔ AND IT REACHES ZERO BY BEING READ (this workspace's rule 5: a count with no road to zero
    # is not actionable). Reading each stepped-over run clears it, and the clause goes.
    ( cd "$tmp" && for s in $(hosted_read_skipped); do
          hosted_read_seen "$s" settled >/dev/null
      done )
    said="$( cd "$tmp" && hosted_read_gap )"
    case "$said" in
        *"STEPPED OVER"*)
            echo "  FAIL  a stepped-over commit that has now been read is still owed: $said"
            fail=$((fail + 1)) ;;
        *)  echo "  ok    reading a stepped-over run clears it"
            pass=$((pass + 1)) ;;
    esac
    # ⚠ A LOOK THAT FOUND NO VERDICT IS NOT A COMMIT NOBODY LOOKED AT. The two debts have
    # different remedies, so a commit must not be filed under both — `unsettled` owns it.
    ( cd "$tmp" && printf '%s\n' "$base" > "$(hosted_read_marker)" )
    ( cd "$tmp" && hosted_read_seen "$jumped" unsettled >/dev/null )
    ( cd "$tmp" && hosted_read_seen HEAD settled >/dev/null )
    said="$( cd "$tmp" && hosted_read_skipped )"
    case "$said" in
        *"$jumped"*)
            echo "  FAIL  a commit already looked at was also filed as stepped over"
            fail=$((fail + 1)) ;;
        *)  echo "  ok    a commit looked at before its run spoke is not also stepped over"
            pass=$((pass + 1)) ;;
    esac
    # ⚠ AND A CLONE THAT IS ONLY NOW STARTING TO TRACK HAS STEPPED OVER NOTHING. Without a mark
    # there are no two endpoints, and enumerating the whole history would file every commit ever
    # made as a debt — a count that starts at the size of the repository is one nobody can act on.
    ( cd "$tmp" && rm -f "$(hosted_read_marker)" )
    ( cd "$tmp" && hosted_read_seen HEAD settled >/dev/null )
    if [ -z "$( cd "$tmp" && hosted_read_skipped )" ]; then
        echo "  ok    a first read files no history as stepped over"
        pass=$((pass + 1))
    else
        echo "  FAIL  a clone's first read filed its own history as a debt"
        fail=$((fail + 1))
    fi

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
    # ⛔ AND THE SAME ROAD FOR THE THIRD DEBT — register item 781. A stepped-over
    # commit a rebase took away is not a verdict this branch will ever publish,
    # and without the prune this list is the one that can no longer reach zero.
    ( cd "$tmp" && printf '%s\nskipped %s\n' "$tip" \
        "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef" > "$(hosted_read_marker)" )
    said="$( cd "$tmp" && hosted_read_gap )"
    case "$said" in
        *"STEPPED OVER"*)
            echo "  FAIL  a stepped-over commit this tree dropped is a debt for ever: $said"
            fail=$((fail + 1)) ;;
        *)  echo "  ok    a stepped-over commit this branch dropped stops being one"
            pass=$((pass + 1)) ;;
    esac
    ( cd "$tmp" && hosted_read_seen "$base" settled >/dev/null )

    # ── AND A COMMIT THAT NEVER HAD A RUN IS NOT ONE NOBODY LOOKED AT ──────────
    #
    # ⛔⛔⛔⛔⛔ REGISTER ITEM 790, and the FOURTH face of the unit being wrong.
    # GitHub hangs a run on the TIP of a push, so a commit published underneath
    # one never gets a run at all — and the sentence above sent a reader to go
    # and read it. They find nothing, and the commit sits in the list for ever
    # because `settled` would read a verdict that does not exist and `unsettled`
    # waits for a run that will never speak.
    ( cd "$tmp" && printf '%s\nskipped %s\n' "$tip" "$base" > "$(hosted_read_marker)" )
    FAKE_GH_TOTAL=0
    said="$( cd "$tmp" && hosted_read_gap )"
    case "$said" in
        *"never had a hosted run of their own"*)
            echo "  ok    a stepped-over commit with no run of its own says so"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  a commit with no run reads as one nobody looked at: $said"
            fail=$((fail + 1)) ;;
    esac
    # ⚠⚠ AND IT IS NOT ALSO THE OTHER SENTENCE. Two clauses over one commit is
    # the covering again: the reader is told both to go and read a run and that
    # there is none, and acts on the first.
    case "$said" in
        *"STEPPED OVER"*)
            echo "  FAIL  a commit with no run is ALSO sent to be read: $said"
            fail=$((fail + 1)) ;;
        *)  echo "  ok    and it is not also called one nobody has looked at"
            pass=$((pass + 1)) ;;
    esac
    # ⚠⚠⚠ AND THE LIST CAN REACH ZERO, which is what makes this a repair rather
    # than a fourth thing to read. `--seen` drops it — OUT LOUD, because a mark
    # quietly declaring something handled is item 781's own defect.
    said="$( cd "$tmp" && hosted_read_seen "$tip" settled )"
    case "$said" in
        *"never had a hosted run of their own"*)
            echo "  ok    dropping a commit with no run is said out loud"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  a commit left the list in silence: $said"
            fail=$((fail + 1)) ;;
    esac
    said="$( cd "$tmp" && hosted_read_gap )"
    case "$said" in
        *"${base:0:7}"*)
            echo "  FAIL  a commit that never had a run is owed for ever: $said"
            fail=$((fail + 1)) ;;
        *)  echo "  ok    and it is gone, so this list can reach zero"
            pass=$((pass + 1)) ;;
    esac

    # ⚠⚠⚠⚠ AND *NOBODY COULD ASK* IS A THIRD STATE, not a quiet pass. An absent
    # `gh`, a refused call or a reply that is not a count answers nothing, and
    # this workspace's rule is that an unclassified case is RED. So the commit
    # keeps its place and the report says the question went unanswered.
    ( cd "$tmp" && printf '%s\nskipped %s\n' "$tip" "$base" > "$(hosted_read_marker)" )
    FAKE_GH_TOTAL=1
    FAKE_GH_FAIL=1
    said="$( cd "$tmp" && hosted_read_gap )"
    case "$said" in
        *"could not be asked"*)
            echo "  ok    a question that went unanswered keeps its own word"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  an unasked commit was folded into a measured answer: $said"
            fail=$((fail + 1)) ;;
    esac
    ( cd "$tmp" && hosted_read_seen "$tip" settled >/dev/null )
    said="$( cd "$tmp" && hosted_read_gap )"
    case "$said" in
        *"${base:0:7}"*)
            echo "  ok    and it is NOT dropped — not asked is not asked and none"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  a commit nobody could ask about was dropped anyway: $said"
            fail=$((fail + 1)) ;;
    esac
    FAKE_GH_FAIL=0
    ( cd "$tmp" && printf '%s\n' "$tip" > "$(hosted_read_marker)" )
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
    # ⚠ The double goes with it: the directory it lived in is gone, and PATH goes
    # back to what the caller had so nothing downstream is asking a fake.
    PATH="$saved_path"
    export PATH
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
