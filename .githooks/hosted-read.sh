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

# ⛔ THE SCRATCH-SAFETY DECISION IS NOT WRITTEN TWICE. `scratch-guard.sh` holds
# it, drives the cases no harness here can produce, and is sourced so this file
# still runs standalone. See register item 792 for what it costs to get wrong and
# `scratch-guard.sh`'s own header for the macOS fold that cost a platform.
# shellcheck source-path=SCRIPTDIR
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/scratch-guard.sh"

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

# ⛔⛔⛔⛔⛔ WHAT A RE-RUN'S GREEN COSTS — register item 793, and the FIFTH way the
# unit has been wrong in this file.
#
# 776 counted READS against ROUNDS; 779 separated a COMMIT from its RUNS; 781
# separated a LINE from the SET beneath it; 790 found a commit that maps to NO
# run at all. Here the commit maps to a run and the run maps to SEVERAL VERDICTS:
# `gh run list` shows the LATEST attempt's, and a re-run's green renders exactly
# like a first attempt's.
#
# ⚠⚠⚠⚠⚠ IT IS NOT A HYPOTHETICAL, AND IT ALREADY ERASED A SAMPLE THIS
# REPOSITORY WAS LOOKING FOR. Measured 2026-08-31: `c2dc7df` reads
# `completed/success` in `gh run list`, and `run_attempt` is **2** — attempt 1
# was `failure`, one test, `plugins::tests::every_declared_guardrail_is_one_the_\
# parser_actually_reads`, killed by pty exhaustion. That is a FOURTH sample of
# register item 776 (3)(d), and a checker looking at the re-run's green wrote
# *"so no fourth 776 (3)(d) sample"* in as many words. The sample was in attempt
# 1 and nobody could see it.
#
# ⚠⚠ WHY THIS REPOSITORY SPECIFICALLY. Its standing rule is that *flake* is the
# name for having stopped diagnosing, and that not one is acceptable (items 700,
# 701). A re-run's green deletes exactly that one.
#
# ⛔ AND THE REMEDY IS NOT "DO NOT RE-RUN". A re-run can be entirely right — an
# infrastructure outage, a runner that never started. What cannot happen is that
# the two are INDISTINGUISHABLE at the place the verdict is read.
HOSTED_READ_RERUN_COST="a re-run's green renders exactly like a first attempt's \
-- c2dc7df read success at run_attempt 2 while attempt 1's red held the fourth \
776 (3)(d) sample, and a checker wrote that there was no such sample"

# ⛔⛔⛔ WHAT AN UNANSWERABLE ATTEMPT COSTS — register item 793, and the arm that
# keeps this repair from becoming its own escape hatch.
#
# ⚠ *It was attempt 1* and *nobody could ask which attempt it was* are different
# facts, and this workspace's rule is that an unclassified case is RED rather
# than a pass. Folding `unknown` into `1` would make every clone with no `gh`, no
# network or no token report first-attempt greens it never saw — which is the
# same covering in a new place.
HOSTED_READ_UNATTEMPTED_COST="a verdict nobody could ask the attempt of is not a \
first attempt -- assuming 1 would hand every clone without gh a green it never read"

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

# WHICH ATTEMPT the verdict on `$1` belongs to: a number, or `unknown` where the
# question could not be put — register item 793.
#
# ⛔⛔ IT ASKS RATHER THAN RESTING ON A HABIT. Reading `run_attempt` first is a
# rule this repository's own handoffs keep repeating, which is precisely the
# shape rule 10 rules out: a reason written in prose is a reason nobody re-runs.
# Which attempt a verdict came from is GitHub's fact, so this asks GitHub.
#
# ⚠⚠ THE MAXIMUM ACROSS THE COMMIT'S RUNS, because that is the attempt whose
# verdict `gh run list` renders — the thing a reader actually sees. A commit with
# several workflows is re-run material if ANY of them was re-run, and answering
# with the minimum would report the reassuring half of a split answer.
#
# ⚠ THREE SHAPES OF ANSWER AND NONE OF THEM A DEFAULT: a number is what was
# found; an empty reply (no run at all — item 790's case) answers `1`, because a
# commit with no run has no hidden attempt and item 790 already owns that debt;
# anything else is `unknown` and keeps its own word.
hosted_read_attempt_for() {
    local sha attempt asked
    sha="$1"
    command -v gh >/dev/null 2>&1 || { printf 'unknown\n'; return 0; }
    # ⛔⛔⛔⛔⛔ THE STATUS IS READ SEPARATELY FROM THE OUTPUT, and the first draft
    # of this function did not do that — measured by this file's own selftest on
    # the arm below. A refused call and a commit with no runs BOTH leave the
    # output empty, so folding them collapsed *nobody could ask* into *attempt 1*
    # — the exact covering `HOSTED_READ_UNATTEMPTED_COST` is written against, in
    # the function that constant was written for.
    attempt="$(gh api "repos/{owner}/{repo}/actions/runs?head_sha=${sha}" \
                   --jq '[.workflow_runs[].run_attempt] | max' 2>/dev/null)"
    asked=$?
    [ "$asked" -eq 0 ] || { printf 'unknown\n'; return 0; }
    case "$attempt" in
        null | '') printf '1\n' ;;
        *[!0-9]*)  printf 'unknown\n' ;;
        *)         printf '%s\n' "$attempt" ;;
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
    local runs advance
    local rerun attempt
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
    # ⛔⛔⛔⛔⛔ WHETHER THIS COMMIT HAS A RUN AT ALL, ASKED ONCE — register item 806,
    # and the half item 790 left. That item taught the SKIPPED list to drop a
    # commit with no run of its own, out loud, *"the only place the list can reach
    # zero"*. It never taught `--seen` itself: a commit handed straight to this
    # function was believed. Measured 2026-09-01 on `b7b9944`, which was published
    # underneath `0c94bda` and so got no run — `--seen b7b9944 settled` answered
    # *"the hosted result for b7b9944 was read"* when there had been nothing to
    # read, and `--seen b7b9944 unsettled` would have parked it in `owed` FOR EVER,
    # because `hosted_read_owed` prunes by ancestry and by nothing else.
    #
    # ⇒ Both words are wrong for that commit, which is exactly what
    # `HOSTED_READ_NORUN_COST` already says: *neither word can retire it*. So the
    # state is READ rather than assumed, and it decides three things below — the
    # mark advances, the commit is not owed, and the sentence says which case it is.
    #
    # ⚠ `unknown` is NOT this case and takes none of it: *not asked* is not *asked
    # and found none*, and an unclassified case is RED here rather than a pass.
    runs="$(hosted_read_runs_for "$sha")"
    # ⚠⚠ THE MARK MOVES FOR A COMMIT WITH NO RUN WHATEVER WORD WAS GIVEN. There is
    # nothing to come back for, so leaving the watermark behind it would make an
    # honest reader look again at a commit that can never answer — item 779's own
    # complaint from the other side.
    advance=no
    [ "$verdict" = settled ] && advance=yes
    [ "$runs" = 0 ] && advance=yes
    if [ "$advance" = yes ]; then
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
    # ⚠ AND A COMMIT WITH NO RUN IS NEVER OWED — item 806. `hosted_read_owed`
    # prunes by ancestry alone, so parking this sha there would be a debt with no
    # path to zero (rule 5): no verdict can ever arrive for it.
    if [ "$verdict" = unsettled ] && [ "$runs" != 0 ]; then
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
    # ⛔⛔⛔⛔⛔ WHICH ATTEMPT THIS READER SAW — register item 793. `settled` says a
    # verdict was there and was read; it has never said WHOSE. A re-run's green
    # and a first attempt's are the same word in this file and the same row in
    # `gh run list`, and the difference is a red nobody will ever look at again.
    #
    # ⚠⚠ THE MARK STILL ADVANCES. The reader did read a verdict, and refusing
    # `settled` here would make an honest read stop counting — item 779's own
    # mistake in a new place. What is recorded is a SEPARATE debt against this
    # commit: its first attempt's verdict is still unread.
    #
    # ⚠ Only for `settled`: an `unsettled` look read no verdict at all, so there
    # is no attempt behind it to have been the wrong one.
    rerun="$(hosted_read_rerun_raw | command grep -v " ${sha}\$" || true)"
    # ⚠ And not for a commit with no run: there is no attempt behind a verdict
    # that does not exist, so asking would file a debt about nothing (item 806).
    if [ "$verdict" = settled ] && [ "$runs" != 0 ]; then
        attempt="$(hosted_read_attempt_for "$sha")"
        case "$attempt" in
            1)       ;;
            unknown) rerun="$(printf '%s\nunattempted %s\n' "$rerun" "$sha" \
                              | command grep -v '^$' || true)" ;;
            *)       rerun="$(printf '%s\n%s %s\n' "$rerun" "$attempt" "$sha" \
                              | command grep -v '^$' || true)" ;;
        esac
    fi
    # ⛔⛔ THE RECORD IS MADE BEFORE IT IS ANNOUNCED — register item 804. This
    # redirection reported nothing when it failed, and the sentences below said the
    # reading had been recorded either way. A full disk or a read-only marker
    # arrives here as silence, and this file's whole subject is what was recorded.
    {
        printf '%s\n' "$mark"
        printf '%s\n' "$kept" | command sed '/^$/d;s/^/owed /'
        printf '%s\n' "$skipped" | command sed '/^$/d;s/^/skipped /'
        printf '%s\n' "$rerun" | command sed '/^$/d;s/^/rerun /'
    } | scratch_guard_write "$marker" "hosted-read" || return 1
    # ⛔⛔⛔ THREE SENTENCES, NOT TWO — register item 806. *A verdict was read*, *a
    # run has not spoken yet* and *there was no run to read* are three different
    # facts about a commit, and the third had been wearing whichever of the first
    # two the caller happened to type.
    if [ "$runs" = 0 ]; then
        echo "hosted-read: ${sha:0:7} HAS NO RUN OF ITS OWN, so there was no" \
             "verdict to read -- the mark moved past it and it is not owed," \
             "whichever word was given. ${HOSTED_READ_NORUN_COST}"
    elif [ "$verdict" = settled ]; then
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

# RETIRE the re-run debt on `$1` by saying what its FIRST attempt actually said:
# `$2` is `clean` (attempt 1 was green too, so the re-run hid nothing) or `red`
# (it was not, and that red is now this round's) — register item 793.
#
# ⛔⛔⛔⛔⛔ THIS IS THE ONLY WAY THE LIST REACHES ZERO, and it takes a WORD for the
# reason `--seen` does (item 779): the classification is a fact only the reader
# has, and neither answer may be the default. `clean` would let a reader retire a
# hidden red by typing; `red` would file a debt against a run that never had one.
# An unclassified call is REFUSED.
#
# ⚠⚠ `--seen <sha> settled` CANNOT retire it, and that is deliberate. Looking
# again finds the same re-run and re-files the same debt, because the thing that
# discharges this is reading a DIFFERENT verdict — attempt 1's — which no amount
# of looking at the latest one can do.
#
# ⚠ A `red` does not stay in this file. What it costs is a debt in the register,
# and this says so out loud rather than keeping a count nobody reads: the whole
# defect was a red that went unfiled because nothing named it.
hosted_read_first_attempt() {
    local sha said kept
    sha="$(git rev-parse --verify "${1:-}^{commit}" 2>/dev/null)" || {
        echo "hosted-read: '${1:-}' is not a commit in this tree" >&2
        return 1
    }
    said="${2:-}"
    case "$said" in
        clean|red) ;;
        *)  echo "hosted-read: say WHAT the first attempt said --" \
                 "'--first-attempt ${1:-} clean' if it was green too, so the" \
                 "re-run hid nothing, or '--first-attempt ${1:-} red' if it was" \
                 "not. A re-run's green is retired by reading the verdict it" \
                 "covered, never by looking at it again (register item 793)." >&2
            return 2 ;;
    esac
    kept="$(hosted_read_rerun_raw | command grep -v " ${sha}\$" || true)"
    {
        printf '%s\n' "$(hosted_read_watermark)"
        hosted_read_owed | command sed '/^$/d;s/^/owed /'
        hosted_read_skipped | command sed '/^$/d;s/^/skipped /'
        printf '%s\n' "$kept" | command sed '/^$/d;s/^/rerun /'
    } | scratch_guard_write "$(hosted_read_marker)" "hosted-read" || return 1
    if [ "$said" = red ]; then
        echo "hosted-read: recorded that ${sha:0:7}'s FIRST attempt was RED --" \
             "that red is this round's debt now, and it is the one a re-run's" \
             "green would have deleted. File it before the next push."
    else
        echo "hosted-read: recorded that ${sha:0:7}'s first attempt was green" \
             "too, so the re-run covered nothing"
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

# Every commit whose verdict was read FROM A RE-RUN, as `<word> <sha>` per line
# where `<word>` is the attempt number or `unattempted` — register item 793, and
# the raw form, without the ancestry prune the reported one gets.
#
# ⚠ Raw because `hosted_read_seen` rewrites the marker and must carry forward
# what it did not just decide, including entries for commits a rebase dropped —
# the prune is a REPORTING rule, and applying it while writing would silently
# discard a debt the moment somebody looked from a detached HEAD.
hosted_read_rerun_raw() {
    local marker
    marker="$(hosted_read_marker)"
    [ -r "$marker" ] || return 0
    command sed -n 's/^rerun //p' "$marker"
}

# The same list, pruned to what HEAD still contains — register item 793, with the
# prune `hosted_read_owed` and `hosted_read_skipped` get and for the same reason:
# a commit this branch dropped is not a debt, and a count that cannot reach zero
# stops being acted on.
hosted_read_rerun() {
    local word sha
    hosted_read_rerun_raw | while read -r word sha; do
        [ -n "$sha" ] || continue
        git merge-base --is-ancestor "$sha" HEAD 2>/dev/null \
            && printf '%s %s\n' "$word" "$sha"
    done
}

# The clause naming what was read FROM A RE-RUN — empty when nothing was, which
# is what keeps a receipt a receipt (register item 776, arm 5).
#
# ⚠⚠ A FIFTH CLAUSE AND NOT A NUMBER ADDED TO ONE OF THE FOUR. The states are
# *nobody looked*, *the run had not spoken*, *the mark went past it*, *there was
# no run to read*, and now *what was read was a re-run's verdict*. Each has its
# own remedy, and a reader handed one sentence for two of them comes away
# thinking one act discharges both — which is the covering item 779 was about.
#
# ⚠ TWO ARMS, because *it was attempt N* and *nobody could ask* are different
# facts and an unclassified case is RED here rather than a pass.
hosted_read_rerun_clause() {
    local listed count seen list
    seen="$(hosted_read_rerun)"
    [ -n "$seen" ] || return 0
    list="$(printf '%s\n' "$seen" | command sed -n 's/^unattempted //p')"
    if [ -n "$list" ]; then
        count="$(printf '%s\n' "$list" | command grep -c .)"
        listed="$(printf '%s\n' "$list" | command cut -c1-7 | command tr '\n' ' ')"
        printf '%s' "; and ${count} commit(s) were read without anyone being able \
to ask which attempt spoke (${listed% }) -- ${HOSTED_READ_UNATTEMPTED_COST}"
    fi
    list="$(printf '%s\n' "$seen" | command grep -v '^unattempted ' \
            | command sed 's/^[0-9]* //')"
    if [ -n "$list" ]; then
        count="$(printf '%s\n' "$list" | command grep -c .)"
        listed="$(printf '%s\n' "$list" | command cut -c1-7 | command tr '\n' ' ')"
        printf '%s' "; and ${count} commit(s) were read from a RE-RUN's verdict, \
so their first attempt's is still unread (${listed% }) -- ${HOSTED_READ_RERUN_COST}"
    fi
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
             "--$(hosted_read_owed_clause)$(hosted_read_skipped_clause)$(hosted_read_rerun_clause)"
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
             "0 round(s) unread$(hosted_read_owed_clause)$(hosted_read_skipped_clause)$(hosted_read_rerun_clause)"
        return 0
    fi
    count="$(git rev-list --count "${recorded}..HEAD" 2>/dev/null || echo "")"
    if [ -z "$count" ]; then
        echo "hosted-read: ${recorded:0:7} is not an ancestor of HEAD, so the" \
             "gap cannot be read here"
        return 0
    fi
    echo "hosted-read: ${count} round(s) published since a hosted result was" \
         "read (last read at ${recorded:0:7}) -- ${HOSTED_READ_COST}$(hosted_read_owed_clause)$(hosted_read_skipped_clause)$(hosted_read_rerun_clause)"
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
    local scratch_refusal wiring
    local FAKE_GH_ATTEMPT rerun_sha
    saved_path="$PATH"
    here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    # ⚠⚠ CHECKED HERE **AND** BELOW, and the pair is not redundant — register item
    # 792. This catches the status (`mktemp` off PATH exits 127), which is exactly
    # what happened; the block below additionally asks whether the path is a
    # DIRECTORY, which a status alone cannot answer. The statement that assigns is
    # the one place no later edit can drift away from, so the narrow check lives
    # there and `hooks_cannot_pass_in_silence` reads for it.
    tmp="$(mktemp -d)" || return 1
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
#
# ⚠ TWO QUESTIONS NOW, told apart by the `--jq` the caller wrote (item 793):
# `hosted_read_runs_for` asks for a count, `hosted_read_attempt_for` asks for the
# attempt. Branching on the ARGUMENTS rather than on an environment variable of
# the double's own keeps the two answers independent, so an arm can drive a
# re-run on a commit whose run count is anything.
[ "${FAKE_GH_FAIL:-0}" = 1 ] && exit 1
case "$*" in
    *run_attempt*) printf '%s\n' "${FAKE_GH_ATTEMPT:-1}" ;;
    *)             printf '%s\n' "${FAKE_GH_TOTAL:-1}" ;;
esac
ASKED
    chmod +x "$tmp/bin/gh"
    PATH="$tmp/bin:$PATH"
    FAKE_GH_TOTAL=1
    FAKE_GH_FAIL=0
    FAKE_GH_ATTEMPT=1
    export PATH FAKE_GH_TOTAL FAKE_GH_FAIL FAKE_GH_ATTEMPT

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
    # ⛔⛔⛔⛔⛔ TWO CLAIMS, AND THE FIRST DRAFT MADE THEM ONE — measured on macOS
    # 2026-09-01, by the gate register item 799 had just built. This selftest had
    # never run anywhere but a person's Linux box, and on macOS it REFUSED every
    # time: `mktemp -d` there always answers under `/var`, which is a symlink to
    # `/private/var`, so `pwd` (logical) and git's answer (physical) disagree and
    # a check written as one comparison read that as *somebody else's repository*.
    #
    # ⚠⚠ The two claims are **I am standing inside my own scratch** and **it is
    # not the caller's**, and folding them cost a platform. They are separated
    # here: the first compares PHYSICAL paths on both sides so a symlinked TMPDIR
    # is not a finding; the second says outright that the scratch git dir differs
    # from the caller's, which is the sentence that actually keeps the operator's
    # marker safe and is true whatever the path went through to get here.
    #
    # ⚠ Reproduced on Linux before it was touched, by injection rather than by
    # belief: `TMPDIR=<a symlink> hosted-read.sh --selftest` refused identically.
    scratch_refusal="$(scratch_guard_check "$tmp")"
    if [ -z "$tmp" ] || [ ! -d "$tmp" ] || [ -n "$scratch_refusal" ]; then
        echo "hosted-read selftest: REFUSING to run -- ${scratch_refusal:-the" \
             "scratch path is not a directory}, so every arm would read and" \
             "WRITE $(git rev-parse --absolute-git-dir \
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

    # ⛔⛔⛔⛔⛔ THE SAME COMMIT HANDED STRAIGHT TO `--seen` — register item 806, the
    # half item 790 left. Above, the commit reached the list by being STEPPED OVER;
    # here a person names it, and until this arm existed both words were believed.
    ( cd "$tmp" && printf '%s\n' "$base" > "$(hosted_read_marker)" )
    FAKE_GH_TOTAL=0
    said="$( cd "$tmp" && hosted_read_seen "$tip" settled )"
    case "$said" in
        *"HAS NO RUN OF ITS OWN"*)
            echo "  ok    --seen settled on a runless commit says there was nothing to read"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  --seen settled on a runless commit said: $said"
            fail=$((fail + 1)) ;;
    esac
    # ⚠ And it is NOT the sentence for a verdict that was read.
    case "$said" in
        *"was read"*)
            echo "  FAIL  a commit with no run was reported as one whose result was read: $said"
            fail=$((fail + 1)) ;;
        *)  echo "  ok    and it is not called a verdict that was read"
            pass=$((pass + 1)) ;;
    esac
    # ⛔⛔ AND `unsettled` MUST NOT PARK IT IN `owed`, which prunes by ancestry
    # alone: no verdict can ever arrive, so that debt could not reach zero.
    #
    # ⚠⚠ THE LIST IS ASKED DIRECTLY, not looked for in the sentence. The first
    # draft of this arm searched the whole `--gap` line for the sha and failed on
    # its own subject: the mark had (correctly) advanced onto that commit, so
    # `last read at <sha>` matched and the arm called it *owed*. A sha appearing
    # SOMEWHERE is not the same claim as a sha appearing in a particular clause.
    ( cd "$tmp" && printf '%s\n' "$base" > "$(hosted_read_marker)" )
    ( cd "$tmp" && hosted_read_seen "$tip" unsettled >/dev/null )
    said="$( cd "$tmp" && hosted_read_owed )"
    case "$said" in
        *"$tip"*)
            echo "  FAIL  a runless commit called unsettled is owed for ever: $said"
            fail=$((fail + 1)) ;;
        *)  echo "  ok    a runless commit is not owed whichever word it got"
            pass=$((pass + 1)) ;;
    esac
    # ⚠ And the mark DID move past it — there is nothing to come back for.
    said="$( cd "$tmp" && hosted_read_watermark )"
    if [ "$said" = "$tip" ]; then
        echo "  ok    and the mark moved past it rather than waiting on nothing"
        pass=$((pass + 1))
    else
        echo "  FAIL  the mark stayed behind a commit that can never answer: $said"
        fail=$((fail + 1))
    fi
    FAKE_GH_TOTAL=1

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
    #
    # ⛔⛔ *UNREADABLE* AND *NOT WIRED* ARE SEPARATE FINDINGS, and folding them
    # cost a diagnosis: on 2026-09-01 this arm failed once inside the parallel
    # `sprag-gate` suite and passed on the next run, with the one sentence it had
    # to offer naming the wrong cause -- `pre-push never calls the gap arm` while
    # the hook plainly did. Which of the two it was could not be recovered
    # afterwards, so the next occurrence says it (register item 803).
    #
    # ⛔⛔⛔ THE READ HAPPENS ONCE AND THE FAILURE REPORTS WHAT IT READ — register
    # item 803. This arm has now failed twice inside the parallel `sprag-gate`
    # suite while `pre-push` plainly called the arm, and each time the sentence
    # named a cause that was not true. A verdict about a file that does not carry
    # what the file said leaves nothing to diagnose from, so the bytes go into a
    # variable, the decision reads THAT variable, and a failure prints its size
    # and what `sed` wrote — including on stderr, which is where a double that was
    # handed no subject would complain.
    wiring="$(command sed 's/#.*//' "$here/pre-push" 2>&1)"
    if [ ! -r "$here/pre-push" ]; then
        echo "  FAIL  pre-push is not readable at $here -- the wiring cannot be" \
             "judged, and this is NOT the same finding as it not calling the arm"
        fail=$((fail + 1))
    elif printf '%s\n' "$wiring" | command grep -q 'hosted_read_gap'; then
        echo "  ok    the pre-push hook calls the gap arm"
        pass=$((pass + 1))
    else
        echo "  FAIL  pre-push is readable and the ${#wiring} byte(s) read from it" \
             "never call the gap arm -- what the reader got began:" \
             "$(printf '%s' "$wiring" | command head -c 200)"
        fail=$((fail + 1))
    fi

    # ⛔⛔⛔⛔⛔ AND IT IS CALLED THE WAY THE HOOK CALLS IT — under `set -euo
    # pipefail`. This file's whole standing is that it REPORTS and never refuses,
    # and a non-zero status from it is a REFUSED PUSH rather than a sentence.
    # Measured 2026-09-01 in `loop-read.sh`: a `grep` that matched nothing made its
    # report exit 1, `pre-push` died on the assignment, and `git push` failed with
    # no diagnosis at all. The arm exists in both files now because the property is
    # the hook's, not one file's, and neither selftest could see it while it called
    # the function the way a test finds convenient.
    # ⛔⛔ IN A CHILD PROCESS, NOT `if ( set -e; … )`. `set -e` is SUPPRESSED for any
    # command in a condition, and the suppression reaches inside a subshell written
    # there -- so that shape switches off the very guard it means to observe. It was
    # written that way first and measured as a dead control the same hour.
    ( cd "$tmp" && bash -c 'set -euo pipefail; . "$1"; hosted_read_gap >/dev/null' \
        _ "$here/hosted-read.sh" )
    said=$?
    if [ "$said" -eq 0 ]; then
        echo "  ok    the report exits 0 under the hook's own set -euo pipefail"
        pass=$((pass + 1))
    else
        echo "  FAIL  the report exits ${said} under set -euo pipefail, which is a" \
             "REFUSED PUSH rather than a sentence"
        fail=$((fail + 1))
    fi

    # ⛔⛔⛔⛔⛔ THE RE-RUN ARMS — register item 793. Five of them, because the
    # states are *attempt 1* (silent), *attempt N* (a debt), *unaskable* (a
    # different debt), *retired clean* and *retired red*, and this file's own rule
    # is that a state which cannot be told from another one is the defect.
    ( cd "$tmp" && : > r && git add r && git commit -qm rerun )
    rerun_sha="$(git -C "$tmp" rev-parse HEAD)"

    # A first attempt says nothing, so the receipt stays a receipt.
    FAKE_GH_ATTEMPT=1
    ( cd "$tmp" && hosted_read_seen HEAD settled >/dev/null )
    said="$( cd "$tmp" && hosted_read_gap )"
    case "$said" in
        *"RE-RUN"*)
            echo "  FAIL  a first-attempt read claimed a re-run: $said"
            fail=$((fail + 1)) ;;
        *)  echo "  ok    a verdict read at attempt 1 adds no clause"
            pass=$((pass + 1)) ;;
    esac

    # ⛔ The one this item exists for: the same word, the same green, a different
    # attempt — and the sentence must not read the same.
    FAKE_GH_ATTEMPT=2
    ( cd "$tmp" && hosted_read_seen HEAD settled >/dev/null )
    said="$( cd "$tmp" && hosted_read_gap )"
    case "$said" in
        *"read from a RE-RUN's verdict"*"${rerun_sha:0:7}"*)
            echo "  ok    a verdict read at attempt 2 is named as a re-run's"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  a re-run read said: $said"
            fail=$((fail + 1)) ;;
    esac

    # ⚠ Looking AGAIN cannot retire it: the same look finds the same re-run.
    ( cd "$tmp" && hosted_read_seen HEAD settled >/dev/null )
    said="$( cd "$tmp" && hosted_read_gap )"
    case "$said" in
        *"read from a RE-RUN's verdict"*)
            echo "  ok    looking again does not retire a re-run's verdict"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  a second look retired the re-run: $said"
            fail=$((fail + 1)) ;;
    esac

    # ⛔ And an unclassified retirement is REFUSED rather than defaulted.
    if ( cd "$tmp" && hosted_read_first_attempt "$rerun_sha" >/dev/null 2>&1 ); then
        echo "  FAIL  --first-attempt accepted no word"
        fail=$((fail + 1))
    else
        echo "  ok    --first-attempt refuses a look that says nothing"
        pass=$((pass + 1))
    fi

    # Reading the covered verdict is what reaches zero.
    said="$( cd "$tmp" && hosted_read_first_attempt "$rerun_sha" red )"
    case "$said" in
        *"FIRST attempt was RED"*)
            echo "  ok    a red first attempt is reported as this round's debt"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  retiring a red said: $said"
            fail=$((fail + 1)) ;;
    esac
    said="$( cd "$tmp" && hosted_read_gap )"
    case "$said" in
        *"RE-RUN"*)
            echo "  FAIL  the re-run clause survived being retired: $said"
            fail=$((fail + 1)) ;;
        *)  echo "  ok    a retired re-run leaves the sentence"
            pass=$((pass + 1)) ;;
    esac

    # ⚠ `unknown` is NOT folded into attempt 1 — its own clause, its own words.
    FAKE_GH_FAIL=1
    ( cd "$tmp" && hosted_read_seen HEAD settled >/dev/null )
    FAKE_GH_FAIL=0
    said="$( cd "$tmp" && hosted_read_gap )"
    case "$said" in
        *"able to ask which attempt spoke"*)
            echo "  ok    an unaskable attempt keeps its own clause"
            pass=$((pass + 1)) ;;
        *)  echo "  FAIL  an unaskable attempt said: $said"
            fail=$((fail + 1)) ;;
    esac
    ( cd "$tmp" && hosted_read_first_attempt "$rerun_sha" clean >/dev/null )

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
        --first-attempt) shift; hosted_read_first_attempt "${1:-}" "${2:-}" ;;
        --gap|"") hosted_read_gap ;;
        *) echo "usage: hosted-read.sh [--gap|--seen SHA <settled|unsettled>" \
                "|--first-attempt SHA <clean|red>|--selftest]" >&2
           exit 2 ;;
    esac
fi
