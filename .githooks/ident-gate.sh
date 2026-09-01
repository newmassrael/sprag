#!/usr/bin/env bash
# .githooks/ident-gate.sh — which addresses may author or commit here.
#
# WHY THIS EXISTS, and it is not hypothetical. In a sibling repository of this
# owner's, commits reached a PUBLIC remote authored with an address other than
# the one every other commit carries. A history rewrite removed them from the
# branch and did NOT un-publish them: the host keeps unreachable objects and
# went on serving them by SHA. The repository had to be deleted and recreated,
# and its CI history went with it. This gate exists so the next occurrence
# costs a refused commit instead.
#
# CHECKABLE FROM HERE, rather than taken on trust:
#
#   gh repo view newmassrael/watching-zenoh --json createdAt
#
# answers a creation date MONTHS NEWER than that repository's own first commit
# — the signature of exactly that repair, and the reason this file does not
# have to argue from an incident nobody here watched.
#
# WHY A CONFIG WOULD NOT HAVE CAUGHT IT. `git config user.email` was already
# correct there, in the clone AND in ~/.gitconfig. The commits came from a
# different environment. A config is a DEFAULT and it is per-clone; a rule that
# has to be present on the machine that gets it wrong has to travel with the
# tree, which is what a tracked hook does.
#
# WHY IT GRADES `git var` AND NEVER `git config`. The identity a commit will
# carry is not what the config says: GIT_AUTHOR_EMAIL and GIT_COMMITTER_EMAIL
# in the environment override it, `commit --author` overrides it again, and
# `git -c user.email=` overrides it for one invocation. `git var
# GIT_AUTHOR_IDENT` is git answering with what it will actually stamp, after
# all of that.
#
# WHY AN ALLOW-LIST. A deny-list passes every identity it has not been taught
# yet; an allow-list fails closed for the one nobody thought about. It also
# avoids writing the offending address into a tracked file of a public
# repository, which is the exposure the incident was about.
#
# ⚠ SHARPER HERE THAN ELSEWHERE, because this tree has TWO WRITERS: the AI debt
# loop commits into it unattended. An identity gate is the only thing that can
# tell the loop's commits from a stray environment's without a person reading
# every one.
#
# TWO CALLERS, ONE LIST. `pre-commit` grades the identity the next commit would
# carry; `pre-push` grades every commit in the range being published. Neither
# subsumes the other: pre-commit runs only for `git commit`, so cherry-pick,
# rebase, merge and `--no-verify` reach the remote without it, and a commit made
# where the hooks are not installed is caught only at the push -- which is the
# shape the incident actually had.
#
# Self-tested: `bash .githooks/ident-gate.sh --selftest` drives every arm
# against a throwaway repository, because a gate reachable only from a hook
# cannot otherwise be told apart from one that always passes.

# The identities this repository accepts. Add one DELIBERATELY, in its own
# commit: an edit here is a statement about who may write history that the
# remote publishes.
SPRAG_ALLOWED_IDENT_EMAILS=(
    "newmassrael@gmail.com"
)

# "Name <email> 1756100000 +0900" -> "email".
#
# Cut on the angle brackets, not on whitespace: a display name may contain
# spaces, and a field-counting parse returns the wrong token when it does --
# silently, which is the failure mode this file is about.
ident_email_of() {
    local ident="$1"
    ident="${ident#*<}"
    printf '%s' "${ident%%>*}"
}

ident_is_allowed() {
    local email="$1" allowed
    for allowed in "${SPRAG_ALLOWED_IDENT_EMAILS[@]}"; do
        [ "$email" = "$allowed" ] && return 0
    done
    return 1
}

# The shared refusal, so the two hooks cannot drift into explaining one rule
# two ways.
ident_refuse() {
    local hook="$1" what="$2" email="$3"
    {
        echo "${hook}: ${what} <${email}>,"
        echo "  which is not an identity this repository accepts."
        echo ""
        echo "  Why: commits reached a PUBLIC repo of this owner's under a"
        echo "  different address. Rewriting history did NOT un-publish them;"
        echo "  the repository had to be deleted and recreated, and its CI"
        echo "  history went with it."
        echo ""
        echo "  fix, in this clone:"
        echo "    git config user.email ${SPRAG_ALLOWED_IDENT_EMAILS[0]}"
        echo "    git config user.name  <your name>"
        echo "  and check the environment too -- these override the config:"
        echo "    env | grep -E '^GIT_(AUTHOR|COMMITTER)_EMAIL='"
        echo ""
        echo "  If a NEW identity is genuinely meant to write here, add it to"
        echo "  SPRAG_ALLOWED_IDENT_EMAILS in .githooks/ident-gate.sh --"
        echo "  deliberately, in its own commit."
    } >&2
}

# pre-commit's arm: the identity the commit ABOUT TO BE MADE would carry.
ident_gate_pending() {
    local hook="$1" pair role verb ident email
    for pair in "AUTHOR authored" "COMMITTER committed"; do
        role="${pair%% *}"
        verb="${pair##* }"
        if ! ident="$(git var "GIT_${role}_IDENT")"; then
            echo "${hook}: \`git var GIT_${role}_IDENT\` failed; cannot" >&2
            echo "  determine the identity this commit would carry. A gate" >&2
            echo "  that could not read is not a gate that found nothing." >&2
            return 1
        fi
        email="$(ident_email_of "$ident")"
        if [ -z "$email" ]; then
            echo "${hook}: no email in GIT_${role}_IDENT: ${ident}" >&2
            return 1
        fi
        if ! ident_is_allowed "$email"; then
            ident_refuse "$hook" "this commit would be ${verb} as" "$email"
            return 1
        fi
    done
    return 0
}

# pre-push's arm: every commit in the range being published.
#
# `range` is `<base>..<tip>` when the remote already has the ref and a bare
# `<tip>` when it does not. That second case grades the WHOLE history and is
# deliberate: a brand-new remote is exactly when a stray identity would
# otherwise be republished wholesale.
#
# Reports EVERY offender rather than the first, because the fix is a rebase
# whose scope the author needs to know before starting it.
ident_gate_range() {
    local hook="$1" range="$2" sha email bad=0 shown=0 log
    if ! log="$(git log --format='%H %ae%n%H %ce' "$range" --)"; then
        echo "${hook}: \`git log ${range}\` failed; cannot determine which" >&2
        echo "  identities this push would publish. A gate that could not" >&2
        echo "  read is not a gate that found nothing." >&2
        return 1
    fi
    while IFS=' ' read -r sha email; do
        [ -n "$sha" ] || continue
        if ! ident_is_allowed "$email"; then
            if [ "$bad" -eq 0 ]; then
                ident_refuse "$hook" "this push would publish commits by" "$email"
                echo "" >&2
                echo "  offending commits in ${range}:" >&2
            fi
            bad=$((bad + 1))
            if [ "$shown" -lt 20 ]; then
                echo "    ${sha} <${email}>" >&2
                shown=$((shown + 1))
            fi
        fi
    done <<EOF
$log
EOF
    if [ "$bad" -gt 0 ]; then
        [ "$bad" -gt "$shown" ] && echo "    ... and $((bad - shown)) more" >&2
        return 1
    fi
    return 0
}

# ─── selftest ───────────────────────────────────────────────────────────────
#
# The range arm is reachable only from `pre-push`, and a gate nothing can
# execute cannot be told apart from one that always passes. This drives every
# arm against a throwaway repository so the rules stay gradable without a push.
#
# THE WIRING IS TESTED TOO, and that is not padding: in a sibling repository
# the call was added to `pre-push` without the `source` beside it, every
# library-level case stayed green because the suite sources the library itself,
# and the defect surfaced only as a broken push.
ident_gate_selftest() {
    local tmp pass=0 fail=0 good bad hook here
    here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    good="${SPRAG_ALLOWED_IDENT_EMAILS[0]}"
    bad="nobody@example.invalid"
    tmp="$(mktemp -d)" || return 1

    _t() {
        if [ "$2" -eq "$3" ]; then
            echo "  ok    $1  (rc=$3)"
            pass=$((pass + 1))
        else
            echo "  FAIL  $1  want rc=$2, got rc=$3"
            fail=$((fail + 1))
        fi
    }

    (
        cd "$tmp" || exit 1
        git init -q .
        git config user.name probe
        git config user.email "$good"
        : >a && git add a && git commit -q -m good
        : >b && git add b &&
            GIT_AUTHOR_EMAIL="$bad" GIT_AUTHOR_NAME=probe git commit -q -m 'bad author'
        : >c && git add c &&
            GIT_COMMITTER_EMAIL="$bad" GIT_COMMITTER_NAME=probe git commit -q -m 'bad committer'
    ) >/dev/null 2>&1 || { rm -rf "$tmp"; return 1; }

    local base tip
    base="$(git -C "$tmp" rev-list --max-parents=0 HEAD)"
    tip="$(git -C "$tmp" rev-parse HEAD)"

    ( cd "$tmp" && ident_gate_range probe "$base" ) >/dev/null 2>&1
    _t "a range of allowed identities passes" 0 $?
    ( cd "$tmp" && ident_gate_range probe "$tip" ) >/dev/null 2>&1
    _t "a bad author or committer in the range is refused" 1 $?
    ( cd "$tmp" && ident_gate_range probe "${base}..${tip}" ) >/dev/null 2>&1
    _t "the A..B range form is refused too" 1 $?
    ( cd "$tmp" && ident_gate_range probe no-such-ref-anywhere ) >/dev/null 2>&1
    _t "a range git cannot read fails rather than passing" 1 $?
    ( cd "$tmp" && ident_gate_pending probe ) >/dev/null 2>&1
    _t "pending: an allowed identity passes" 0 $?
    ( cd "$tmp" && GIT_AUTHOR_EMAIL="$bad" ident_gate_pending probe ) >/dev/null 2>&1
    _t "pending: a bad author is refused" 1 $?
    ( cd "$tmp" && GIT_COMMITTER_EMAIL="$bad" ident_gate_pending probe ) >/dev/null 2>&1
    _t "pending: a bad committer is refused" 1 $?

    local named
    named="$( ( cd "$tmp" && ident_gate_range probe "$tip" ) 2>&1 >/dev/null \
              | command grep -c "$bad" )"
    if [ "$named" -ge 2 ]; then
        echo "  ok    every offending commit is named  (${named} lines)"
        pass=$((pass + 1))
    else
        echo "  FAIL  only ${named} offender line(s); the rebase scope needs all"
        fail=$((fail + 1))
    fi

    if [ "$(ident_email_of 'Some One <who@example.com> 1756100000 +0900')" = "who@example.com" ]; then
        echo "  ok    a spaced display name still yields the address"
        pass=$((pass + 1))
    else
        echo "  FAIL  a spaced display name did not yield the address"
        fail=$((fail + 1))
    fi

    for hook in pre-commit pre-push; do
        if command grep -q 'ident-gate\.sh"' "$here/$hook"; then
            echo "  ok    the ${hook} hook sources ident-gate.sh"
            pass=$((pass + 1))
        else
            echo "  FAIL  the ${hook} hook does not source ident-gate.sh"
            fail=$((fail + 1))
        fi
    done
    if command grep -q 'ident_gate_pending' "$here/pre-commit"; then
        echo "  ok    pre-commit calls the pending arm"
        pass=$((pass + 1))
    else
        echo "  FAIL  pre-commit never calls the pending arm"
        fail=$((fail + 1))
    fi
    if command grep -q 'ident_gate_range' "$here/pre-push"; then
        echo "  ok    pre-push calls the range arm"
        pass=$((pass + 1))
    else
        echo "  FAIL  pre-push never calls the range arm"
        fail=$((fail + 1))
    fi

    rm -rf "$tmp"
    echo "ident-gate selftest: ${pass}/$((pass + fail)) arm(s) pass"
    [ "$fail" -eq 0 ]
}

# ⛔⛔⛔⛔⛔ AND EVERY OTHER DIRECT RUN IS REFUSED — register item 819, and the file the register
# named as NOT having this defect. It has it: `--selftest` was the only arm, so `bash
# .githooks/ident-gate.sh` exited 0 with no output, exactly like the two the item was filed for.
# Re-measuring the register's own sentence is what found it (this workspace's rule 4).
#
# ⚠ `hosted-read.sh` and `loop-read.sh` really are different, and the difference is what this arm
# restores: they ANSWER a bare invocation (a gap report, an owed list). A file with nothing to say
# says so with a status, which is `scratch-guard.sh`'s shape.
if [ "${BASH_SOURCE[0]}" = "${0}" ]; then
    case "${1:-}" in
        --selftest) ident_gate_selftest; exit $? ;;
        *) echo "ident-gate.sh is a LIBRARY, not a command: it is sourced by pre-commit." >&2
           echo "usage: ident-gate.sh --selftest   (the gate itself needs a hook's arguments)" >&2
           exit 2 ;;
    esac
fi
