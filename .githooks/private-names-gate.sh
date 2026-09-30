#!/usr/bin/env bash
# .githooks/private-names-gate.sh -- which names of this owner's own machines may reach a PUBLIC
# remote.
#
# WHY THIS EXISTS. `.claude/remote-build.toml` says in its first line that it holds REQUIREMENTS and
# "never a machine name", and says again beside the measurement rows that the machines are named in
# the owner's own notes and not there. Nothing enforced either sentence. A machine name went into
# that file in 2026-08 and another was about to in 2026-09, the second one caught by a person
# reading a diff. The remote is public, and a push does not un-publish: the host keeps unreachable
# objects and serves them by SHA (see ident-gate.sh for the incident that made this repository
# treat that as a fact rather than a risk). So the only cheap place to stop a name is the commit
# that would carry it, and the only reliable one is a hook, because a prose rule needs somebody to
# remember it and the debt loop commits unattended.
#
# WHERE THE NAMES COME FROM, and why this file lists none. A list kept here would itself publish
# the names it forbids, which is the exposure this gate exists to end. They are asked of the
# build wrapper, which already holds the fleet and already has a question for it:
#
#     bx --explain-registry     # one `alias <name>` record per host, answered in ~20 ms
#
# The wrapper's own reader answers, so this file carries no second parser of its registry and
# cannot disagree with it about what an alias is. (A `[[host]]` key the wrapper does not read is
# reported by the wrapper as `unread`, which is also why the registry cannot be given a key of
# this gate's own, such as "this name is already public".)
#
# WHAT IS JUDGED. Only what a commit ADDS, because a removed line publishes nothing:
#
#   pre-commit   the staged diff: added lines, and the path of every added or changed file
#   commit-msg   the message being written, minus the `#` lines git strips
#   pre-push     every commit in the range: its message and what its diff added. A line added in one
#                commit of the range and removed in a later one is still refused, because each
#                commit is a published object of its own.
#
# A NAME MATCHES AS A WHOLE IDENTIFIER, case-insensitively: the characters on both sides must not be
# letters or digits. `selftest-alphabet` and `xselftest-alpha` are not `selftest-alpha`;
# `selftest-alpha_x`, `selftest-alpha.example` and `"selftest-alpha"` are. Underscore counts as a
# boundary on purpose: `name_backup` is how a name reaches a file name.
#
# IPv4 ADDRESSES ARE REFUSED TOO, except loopback, the unspecified address, broadcast and the three
# documentation ranges (RFC 5737), and except in Cargo.lock, whose dotted versions are not
# addresses. Measured on the last 400 commits (134,271 added lines): zero hits. Two other patterns
# that were weighed were measured and DROPPED: `/home/<account>` hit 51 times (the owner's own
# account, cited as a path in ordinary notes) and `user@host` hit 8 times, every one an
# `example.invalid` fixture, `git@github.com` or a systemd unit. A pattern that refuses a routine
# commit gets bypassed, and a bypassed gate is worse than none.
#
# WHEN THE ANSWER CANNOT BE HAD. There are exactly three states and none of them is a quiet pass:
#
#   no wrapper on this machine        NOT CHECKED, said on stderr, and the commit proceeds. A clone
#                                     without the wrapper has no private names to protect.
#   wrapper here, no aliases answered REFUSED. A gate that could not read is not a gate that found
#                                     nothing, and this is the state a broken wrapper is in.
#   wrapper here, names answered      judged.
#
# The wrapper is looked for under $SPRAG_REMOTE_BUILD_HOME, or $HOME/.claude/remote-build when that
# is unset. It is deliberately NOT found through the BX environment variable: a test sandbox
# inherits that variable and replaces HOME, and keying on it made the sandbox look like a machine
# with a wrapper and no registry, which refuses every commit it makes (the failure ident-gate.sh's
# users paid for once already: seventeen tests, twenty-one pushes).
#
# WHAT THIS DOES NOT REACH, stated rather than hidden. A machine that is not in the wrapper's
# registry. A commit made where these hooks are not installed, or under `--no-verify`, or with
# HOME pointing somewhere else. A name spelled across a line break or encoded. The refusal for a
# name already in the published history does not un-publish it; it only stops the next mention.
#
# Self-tested: `bash .githooks/private-names-gate.sh --selftest` drives every arm against a
# throwaway repository and a stand-in wrapper, because a gate reachable only from a hook cannot
# otherwise be told apart from one that always passes.

# Where the build wrapper lives. Overridable so a test can put a stand-in there.
#
# It is the first command in this file on purpose: the `shellcheck disable=` below excuses ONE
# assignment, and ShellCheck applies a directive that stands before the file's first command to the
# whole file (no_shell_script_escapes_the_static_checker refuses exactly that).
private_names_home() {
    printf '%s\n' "${SPRAG_REMOTE_BUILD_HOME:-${HOME:-}/.claude/remote-build}"
}

# The one awk program every arm runs. Input is either a git diff (`mode=diff`), a git log carrying
# commit markers (`mode=log`) or a plain message (`mode=message`); names arrive in the environment
# variable NAMES, one per line, because an awk `-v` value may not contain a newline on every awk.
# Output is one `where<TAB>what<TAB>text` record per finding.
#
# The state machine reads a hunk by its declared line counts rather than by the first character of a
# line, because an added line whose text begins with `++ ` is `+++ ` on the wire and would
# otherwise be taken for a file header and skipped (there is a selftest arm for exactly that).
#
# No character-class intervals ({1,3}) and no gawk extensions: BSD awk and mawk are both targets.
# shellcheck disable=SC2016  # awk source: the dollar signs are awk's own fields, not the shell's
PRIVATE_NAMES_AWK='
function isalnum(c) {
    return c ~ /^[a-z0-9]$/
}
function hit(where, what, text) {
    printf "%s\t%s\t%s\n", where, what, substr(text, 1, 140)
}
function exempt(a, b, c, d) {
    if (a == 127) return 1
    if (a == 0 && b == 0 && c == 0 && d == 0) return 1
    if (a == 255 && b == 255 && c == 255 && d == 255) return 1
    if (a == 192 && b == 0 && c == 2) return 1
    if (a == 198 && b == 51 && c == 100) return 1
    if (a == 203 && b == 0 && c == 113) return 1
    return 0
}
function scan(text, where,    low, i, nm, len, start, pos, abs, before, after, rest, run, n, parts, ok, j) {
    low = tolower(text)
    for (i = 1; i <= n_names; i++) {
        nm = folded[i]
        len = length(nm)
        start = 1
        while (start <= length(low)) {
            pos = index(substr(low, start), nm)
            if (pos == 0) break
            abs = start + pos - 1
            before = (abs > 1) ? substr(low, abs - 1, 1) : ""
            after = substr(low, abs + len, 1)
            if (!isalnum(before) && !isalnum(after)) {
                hit(where, shown[i], text)
                break
            }
            start = abs + 1
        }
    }
    if (curfile ~ /(^|\/)Cargo\.lock$/) return
    rest = text
    while (match(rest, /[0-9]+(\.[0-9]+)+/)) {
        run = substr(rest, RSTART, RLENGTH)
        before = (RSTART > 1) ? substr(rest, RSTART - 1, 1) : ""
        rest = substr(rest, RSTART + RLENGTH)
        if (before ~ /[A-Za-z0-9_]/) continue
        n = split(run, parts, ".")
        if (n != 4) continue
        ok = 1
        for (j = 1; j <= 4; j++) {
            if (length(parts[j]) > 3 || parts[j] + 0 > 255) ok = 0
        }
        if (!ok) continue
        if (exempt(parts[1] + 0, parts[2] + 0, parts[3] + 0, parts[4] + 0)) continue
        hit(where, run, text)
    }
}
BEGIN {
    n_raw = split(ENVIRON["NAMES"], raw, "\n")
    n_names = 0
    for (i = 1; i <= n_raw; i++) {
        if (raw[i] != "") {
            n_names++
            shown[n_names] = raw[i]
            folded[n_names] = tolower(raw[i])
        }
    }
    sha = ""
    curfile = ""
    minus = 0
    plus = 0
    lineno = 0
    state = (mode == "log") ? "none" : "diff"
}
mode == "message" {
    if ($0 !~ /^#/) scan($0, "message:" NR)
    next
}
mode == "log" && $0 ~ /^@@sprag-commit@@ / {
    sha = substr($2, 1, 12) ":"
    state = "msg"
    mline = 0
    minus = 0
    plus = 0
    curfile = ""
    next
}
state == "msg" {
    if ($0 == "@@sprag-end@@") { state = "diff"; next }
    mline++
    curfile = ""
    scan($0, sha "message:" mline)
    next
}
state == "diff" {
    c = substr($0, 1, 1)
    if (minus > 0 && c == "-") { minus--; next }
    if (plus > 0 && c == "+") {
        plus--
        lineno++
        if (curfile != "") scan(substr($0, 2), sha curfile ":" lineno)
        next
    }
    if (c == "\\") next
    if ($0 ~ /^@@ /) {
        split(substr($2, 2), oldr, ",")
        split(substr($3, 2), newr, ",")
        minus = ((oldr[2] == "") ? 1 : oldr[2]) + 0
        plus = ((newr[2] == "") ? 1 : newr[2]) + 0
        lineno = newr[1] + 0 - 1
        next
    }
    if (c == ":") {
        nf = split($0, tabs, "\t")
        split(tabs[1], meta, " ")
        if (substr(meta[5], 1, 1) != "D" && nf >= 2) {
            curfile = tabs[nf]
            scan(tabs[nf], sha tabs[nf] " (file name)")
        }
        next
    }
    if (substr($0, 1, 4) == "+++ ") {
        f = substr($0, 5)
        if (f == "/dev/null") curfile = ""
        else {
            if (substr(f, 1, 2) == "b/") f = substr(f, 3)
            curfile = f
        }
        next
    }
    next
}
'

# Print the private names, one per line.
#
#   0  names printed
#   2  no wrapper on this machine: nothing here to hold a commit against
#   1  the wrapper is here and named nobody: REFUSED by the caller
#
# ONE NAMES-BEARING LINE IS ALL IT TAKES. The wrapper's exit status is deliberately not the
# verdict: `--explain-registry` exits non-zero when a host block carries a key nothing reads, and
# that is a registry-hygiene finding, not a reason to stop guarding the names it did print.
private_names_list() {
    local bx records names
    bx="$(private_names_home)/bin/bx"
    [ -x "$bx" ] || return 2
    records="$("$bx" --explain-registry 2>/dev/null </dev/null)" || true
    names="$(printf '%s\n' "$records" | awk '$1 == "alias" && NF >= 2 { print $2 }')"
    [ -n "$names" ] || return 1
    printf '%s\n' "$names"
}

private_names_not_checked() {
    echo "$1: private machine names NOT CHECKED -- no build wrapper under $(private_names_home)," >&2
    echo "  so this machine has no list of names to hold the commit against." >&2
}

private_names_unanswered() {
    {
        echo "$1: the build wrapper under $(private_names_home) is here and named no host,"
        echo "  so this commit cannot be checked for private machine names. A gate that could not"
        echo "  read is not a gate that found nothing."
        echo "  look: $(private_names_home)/bin/bx --explain-registry"
    } >&2
}

# The shared refusal, so the three hooks cannot drift into explaining one rule three ways.
#   $1 hook   $2 what is being published   $3 findings, one `where<TAB>what<TAB>text` per line
private_names_refuse() {
    local hook="$1" what="$2" findings="$3" where name text shown=0 total=0
    total="$(printf '%s\n' "$findings" | wc -l | tr -d ' ')"
    {
        echo "${hook}: found a machine name or address the owner keeps private in ${what}:"
        while IFS="$(printf '\t')" read -r where name text; do
            [ -n "$where" ] || continue
            if [ "$shown" -lt 20 ]; then
                echo "    ${where}: ${name}   ${text}"
                shown=$((shown + 1))
            fi
        done <<EOF
$findings
EOF
        [ "$total" -gt "$shown" ] && echo "    ... and $((total - shown)) more"
        echo ""
        echo "  Why: the remote is public and a push does not un-publish; the host keeps unreachable"
        echo "  objects and serves them by SHA."
        echo "  fix: describe the machine by what it is (\"an 8-core, 30 GB build machine\"), not by"
        echo "  its name or address, and commit again. The names are asked of the build wrapper"
        echo "  (bx --explain-registry) and are not listed anywhere in this repository."
    } >&2
    return 0
}

# Run the awk program over a file of git output and print its findings.
#   $1 names   $2 mode (diff | log | message)   $3 the file to read
# Answers 0 with findings on stdout (possibly none), and nonzero only when awk itself failed.
private_names_scan() {
    NAMES="$1" LC_ALL=C awk -v mode="$2" "$PRIVATE_NAMES_AWK" <"$3"
}

# The three arms share one skeleton: ask for names, take the git output into a file the read of
# which is CHECKED, scan it, refuse on findings. The git command is the only part that differs.
#   $1 hook   $2 mode   $3 what   then the git command
private_names_gate() {
    local hook="$1" mode="$2" what="$3" names rc=0 dump findings
    shift 3
    names="$(private_names_list)" || rc=$?
    case "$rc" in
        0) ;;
        2) private_names_not_checked "$hook"; return 0 ;;
        *) private_names_unanswered "$hook"; return 1 ;;
    esac
    dump="$(mktemp)" || { echo "$hook: cannot make a scratch file to read git's answer into" >&2; return 1; }
    if ! "$@" >"$dump" 2>/dev/null; then
        rm -f "$dump"
        echo "${hook}: could not read ${what}; a gate cannot judge what it cannot read." >&2
        return 1
    fi
    if ! findings="$(private_names_scan "$names" "$mode" "$dump")"; then
        rm -f "$dump"
        echo "${hook}: the scan of ${what} failed; a gate that could not read is not a gate that found nothing." >&2
        return 1
    fi
    rm -f "$dump"
    if [ -n "$findings" ]; then
        private_names_refuse "$hook" "$what" "$findings"
        return 1
    fi
    return 0
}

# The git flags every arm reads with. Spelled once, and spelled OUT: a user's diff.noprefix,
# diff.mnemonicPrefix, an external diff driver or a textconv would each change what the awk sees.
private_names_git() {
    git -c core.quotepath=off "$@"
}

# pre-commit's arm: what the commit about to be made would carry.
private_names_gate_pending() {
    private_names_gate "$1" diff "the staged changes" \
        private_names_git diff --cached --raw -p -U0 --no-color --no-ext-diff --no-textconv \
        --src-prefix=a/ --dst-prefix=b/ --diff-filter=ACMRT
}

# commit-msg's arm: the message being written. Lines beginning `#` are what git strips.
private_names_gate_message() {
    private_names_gate "$1" message "the commit message" cat -- "$2"
}

# pre-push's arm: every commit in the range being published.
#
# `range` is `<base>..<tip>` or a bare `<tip>` for a new ref, which grades the whole history --
# deliberately, as ident-gate.sh's range arm does: a new remote is when a stray name would be
# published wholesale.
private_names_gate_range() {
    private_names_gate "$1" log "the commits in ${2}" \
        private_names_git log -p --raw -U0 --no-color --no-ext-diff --no-textconv \
        --src-prefix=a/ --dst-prefix=b/ --diff-filter=ACMRT \
        --format='@@sprag-commit@@ %H%n%B%n@@sprag-end@@' "$2" --
}

# --- selftest ------------------------------------------------------------------------------
#
# The range arm is reachable only from `pre-push`, and a gate nothing can execute cannot be told
# apart from one that always passes. This drives every arm against throwaway repositories and a
# stand-in wrapper, and checks that the three hooks actually source and call the library: a
# library nothing sources is a gate that runs nowhere.
private_names_selftest() {
    local tmp pass=0 fail=0 here repo hist rb rb_silent rb_broken rb_unread empty notrepo
    here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
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
    _ok() {
        echo "  ok    $1"
        pass=$((pass + 1))
    }
    _no() {
        echo "  FAIL  $1"
        fail=$((fail + 1))
    }

    repo="$tmp/repo"
    hist="$tmp/hist"
    rb="$tmp/rb"
    rb_silent="$tmp/rb-silent"
    rb_broken="$tmp/rb-broken"
    rb_unread="$tmp/rb-unread"
    empty="$tmp/empty"
    notrepo="$tmp/notrepo"
    mkdir -p "$repo" "$hist" "$rb/bin" "$rb_silent/bin" "$rb_broken/bin" "$rb_unread/bin" \
        "$empty" "$notrepo"

    # Stand-in wrappers: what `bx --explain-registry` answers is the only thing this gate reads.
    cat >"$rb/bin/bx" <<'EOF'
#!/bin/sh
if [ "$1" = "--explain-registry" ]; then
    printf 'alias selftest-alpha\nalias Selftest-Beta\nskip selftest-alpha some-repository\n'
    exit 0
fi
exit 9
EOF
    cat >"$rb_silent/bin/bx" <<'EOF'
#!/bin/sh
exit 0
EOF
    cat >"$rb_broken/bin/bx" <<'EOF'
#!/bin/sh
exit 9
EOF
    cat >"$rb_unread/bin/bx" <<'EOF'
#!/bin/sh
printf 'alias selftest-alpha\nunread selftest-alpha oops\n'
exit 1
EOF
    chmod +x "$rb/bin/bx" "$rb_silent/bin/bx" "$rb_broken/bin/bx" "$rb_unread/bin/bx"

    (
        cd "$repo" || exit 1
        git init -q .
        git config user.name probe
        git config user.email probe@example.invalid
        echo base >base.txt
        git add base.txt
        git commit -q -m base
        cd "$hist" || exit 1
        git init -q .
        git config user.name probe
        git config user.email probe@example.invalid
        echo one >one.txt
        git add one.txt
        git commit -q -m c0
        echo "note about selftest-alpha here" >two.txt
        git add two.txt
        git commit -q -m c1
        git rm -q two.txt
        git commit -q -m c2
        echo three >three.txt
        git add three.txt
        git commit -q -m "mention Selftest-Beta in the message"
        echo four >four.txt
        git add four.txt
        git commit -q -m c4
    ) >/dev/null 2>&1 || { rm -rf "$tmp"; return 1; }

    # THE SUBJECT IS THE SCRATCH REPOSITORY OR THIS DOES NOT RUN AT ALL, for the reason
    # ident-gate.sh's selftest gives: a run that is not standing in its own subject has no verdict.
    local scratch_refusal scratch_dir
    for scratch_dir in "$repo" "$hist"; do
        scratch_refusal="$(scratch_guard_check "$scratch_dir" 2>/dev/null || true)"
        if [ -n "$scratch_refusal" ]; then
            echo "private-names selftest: REFUSING to run -- ${scratch_refusal}" >&2
            rm -rf "$tmp"
            return 1
        fi
    done

    local c0 c1 c2 c3 c4
    c0="$(git -C "$hist" rev-parse HEAD~4)"
    c1="$(git -C "$hist" rev-parse HEAD~3)"
    c2="$(git -C "$hist" rev-parse HEAD~2)"
    c3="$(git -C "$hist" rev-parse HEAD~1)"
    c4="$(git -C "$hist" rev-parse HEAD)"

    # Put exactly one file in the index of the scratch repository, whatever an earlier arm left.
    _stage() {
        (
            cd "$repo" && git reset -q --hard HEAD && git clean -fdq &&
                mkdir -p "$(dirname "$1")" && printf '%s\n' "$2" >"$1" && git add "$1"
        ) >/dev/null 2>&1
    }
    _pending() {
        ( cd "$repo" && SPRAG_REMOTE_BUILD_HOME="${1:-$rb}" private_names_gate_pending probe ) 2>&1
    }
    _pending_rc() {
        ( cd "$repo" && SPRAG_REMOTE_BUILD_HOME="${1:-$rb}" private_names_gate_pending probe ) >/dev/null 2>&1
        echo $?
    }

    # -- where the names come from ------------------------------------------------------------
    local listed
    listed="$(SPRAG_REMOTE_BUILD_HOME="$rb" private_names_list | tr '\n' ' ')"
    if [ "$listed" = "selftest-alpha Selftest-Beta " ]; then
        _ok "names are the wrapper's alias records, and its other records are not names"
    else
        _no "names read as [$listed]"
    fi
    SPRAG_REMOTE_BUILD_HOME="$empty" private_names_list >/dev/null 2>&1
    _t "no wrapper on this machine answers 2 (not checked)" 2 $?
    SPRAG_REMOTE_BUILD_HOME="$rb_silent" private_names_list >/dev/null 2>&1
    _t "a wrapper that names nobody answers 1 (refused)" 1 $?
    SPRAG_REMOTE_BUILD_HOME="$rb_broken" private_names_list >/dev/null 2>&1
    _t "a wrapper that fails with no records answers 1 (refused)" 1 $?
    SPRAG_REMOTE_BUILD_HOME="$rb_unread" private_names_list >/dev/null 2>&1
    _t "a wrapper that exits non-zero but printed an alias still guards it" 0 $?

    # -- the staged diff -----------------------------------------------------------------------
    _stage plain.txt "nothing private here"
    _t "pending: a clean change passes" 0 "$(_pending_rc)"
    _stage notes.txt "measured on selftest-alpha, 8 cores"
    _t "pending: an added line naming a host is refused" 1 "$(_pending_rc)"
    # THE OUTPUT IS TAKEN INTO A VARIABLE AND MATCHED WITH `case`, NOT PIPED INTO `grep -q`: `grep -q`
    # leaves at its first match, the writer takes SIGPIPE, and under `pipefail` the pipeline reads as
    # a failure (measured here: all three "names the line" arms went red on a correct refusal).
    local told
    told="$(_pending)" || true
    case "$told" in
        *"notes.txt:1: selftest-alpha"*) _ok "pending: the refusal names the file, the line and the name" ;;
        *) _no "pending: the refusal does not say where" ;;
    esac
    _stage notes.txt "measured on SELFTEST-ALPHA"
    _t "pending: the match ignores case" 1 "$(_pending_rc)"
    _stage notes.txt "the second one, Selftest-Beta"
    _t "pending: every alias is guarded, not just the first" 1 "$(_pending_rc)"
    _stage notes.txt "selftest-alphabet and xselftest-alpha are other words"
    _t "pending: a longer identifier containing a name is not the name" 0 "$(_pending_rc)"
    _stage notes.txt "see selftest-alpha_backup"
    _t "pending: an underscore is a boundary, so name_suffix is refused" 1 "$(_pending_rc)"
    _stage notes.txt "++ selftest-alpha"
    _t "pending: an added line that begins with two pluses is content, not a header" 1 "$(_pending_rc)"
    _stage selftest-alpha.txt "innocent content"
    _t "pending: a name in a file NAME is refused" 1 "$(_pending_rc)"
    (
        cd "$repo" && git reset -q --hard HEAD && git clean -fdq &&
            echo "line with selftest-alpha" >held.txt && git add held.txt && git commit -q -m held &&
            git rm -q held.txt
    ) >/dev/null 2>&1
    _t "pending: removing a line that names a host publishes nothing" 0 "$(_pending_rc)"
    (
        cd "$repo" && git reset -q --hard HEAD && git clean -fdq &&
            git mv held.txt moved.txt 2>/dev/null
    ) >/dev/null 2>&1
    _t "pending: moving a file whose old content names a host adds no line" 0 "$(_pending_rc)"
    # THE TWO ADDRESSES THAT MUST BE REFUSED ARE BUILT HERE, NOT SPELLED: this file is itself added by
    # a commit held against this very gate, and a private address written out in it is refused (the
    # first commit of this gate was, measured). No exemption was added for that on purpose: a gate
    # that can excuse its own file is a gate with a door in it.
    local private_ip dotted_version
    private_ip="$(printf '%s.%s.%s.%s' 10 1 2 3)"
    dotted_version="$(printf '%s.%s.%s.%s' 1 2 3 4)"
    _stage addr.txt "listening on ${private_ip}"
    _t "pending: a private IPv4 address is refused" 1 "$(_pending_rc)"
    _stage addr.txt "bind 127.0.0.1 or 0.0.0.0, docs use 192.0.2.7"
    _t "pending: loopback, unspecified and documentation addresses pass" 0 "$(_pending_rc)"
    _stage addr.txt "version 1.2.3.4.5 and 1.2.3 are not addresses"
    _t "pending: a five-part or three-part dotted number is not an address" 0 "$(_pending_rc)"
    _stage Cargo.lock "version = \"${dotted_version}\""
    _t "pending: a dotted version in Cargo.lock is not an address" 0 "$(_pending_rc)"
    _stage plain.txt "selftest-alpha"
    _t "pending: no wrapper on this machine steps aside" 0 "$(_pending_rc "$empty")"
    told="$(_pending "$empty")" || true
    case "$told" in
        *"NOT CHECKED"*) _ok "pending: stepping aside is said out loud, never silent" ;;
        *) _no "pending: stepping aside printed nothing" ;;
    esac
    _t "pending: a wrapper that names nobody refuses rather than passes" 1 "$(_pending_rc "$rb_silent")"
    # A `.git` FILE that points nowhere, rather than a directory that merely is not a repository:
    # `mktemp` may put this scratch tree under ANOTHER repository (a symlinked TMPDIR onto a build
    # cache inside a work tree does), and then `git diff --cached` here would succeed against the
    # parent and the arm would measure the parent. A dangling pointer fails wherever it stands.
    printf 'gitdir: %s\n' "$tmp/no-such-git-dir" >"$notrepo/.git"
    ( cd "$notrepo" && SPRAG_REMOTE_BUILD_HOME="$rb" private_names_gate_pending probe ) >/dev/null 2>&1
    _t "pending: an index git cannot read fails rather than passing" 1 $?

    # -- the commit message --------------------------------------------------------------------
    local msg="$tmp/msg"
    printf 'fix(x): a clean subject\n\n- a clean bullet\n' >"$msg"
    ( SPRAG_REMOTE_BUILD_HOME="$rb" private_names_gate_message probe "$msg" ) >/dev/null 2>&1
    _t "message: a clean message passes" 0 $?
    printf 'fix(x): measured on selftest-alpha\n' >"$msg"
    ( SPRAG_REMOTE_BUILD_HOME="$rb" private_names_gate_message probe "$msg" ) >/dev/null 2>&1
    _t "message: a name in the subject is refused" 1 $?
    printf 'fix(x): a subject\n\n# Changes to be committed: selftest-alpha.txt\n' >"$msg"
    ( SPRAG_REMOTE_BUILD_HOME="$rb" private_names_gate_message probe "$msg" ) >/dev/null 2>&1
    _t "message: a name only in a # line git strips is not published" 0 $?
    printf 'fix(x): a subject\n\n- one\n- run on Selftest-Beta\n' >"$msg"
    told="$( ( SPRAG_REMOTE_BUILD_HOME="$rb" private_names_gate_message probe "$msg" ) 2>&1 )" || true
    case "$told" in
        *"message:4: Selftest-Beta"*) _ok "message: the refusal names the line" ;;
        *) _no "message: the refusal does not name the line" ;;
    esac

    # -- a range of commits --------------------------------------------------------------------
    _range_rc() {
        ( cd "$hist" && SPRAG_REMOTE_BUILD_HOME="$rb" private_names_gate_range probe "$1" ) >/dev/null 2>&1
        echo $?
    }
    _t "range: a range with no host name in it passes" 0 "$(_range_rc "${c3}..${c4}")"
    _t "range: a commit that ADDS a host name is refused" 1 "$(_range_rc "${c0}..${c1}")"
    _t "range: added in one commit and removed in the next is still refused" 1 "$(_range_rc "${c0}..${c2}")"
    _t "range: a commit that only REMOVES it passes" 0 "$(_range_rc "${c1}..${c2}")"
    _t "range: a name only in a commit MESSAGE is refused" 1 "$(_range_rc "${c2}..${c3}")"
    told="$( ( cd "$hist" && SPRAG_REMOTE_BUILD_HOME="$rb" private_names_gate_range probe "${c0}..${c1}" ) 2>&1 )" || true
    case "$told" in
        *"${c1:0:12}:two.txt:1: selftest-alpha"*) _ok "range: the refusal names the commit, the file and the line" ;;
        *) _no "range: the refusal does not name the commit" ;;
    esac
    _t "range: a bare tip grades the whole history" 1 "$(_range_rc "$c4")"
    _t "range: a range git cannot read fails rather than passing" 1 "$(_range_rc no-such-ref-anywhere)"

    # -- the wiring: a library nothing sources is a gate that runs nowhere -----------------------
    local hook
    for hook in pre-commit commit-msg pre-push; do
        if command grep -q 'private-names-gate\.sh"' "$here/$hook"; then
            _ok "the ${hook} hook sources private-names-gate.sh"
        else
            _no "the ${hook} hook does not source private-names-gate.sh"
        fi
    done
    if command grep -q 'private_names_gate_pending' "$here/pre-commit"; then
        _ok "pre-commit calls the pending arm"
    else
        _no "pre-commit never calls the pending arm"
    fi
    if command grep -q 'private_names_gate_message' "$here/commit-msg"; then
        _ok "commit-msg calls the message arm"
    else
        _no "commit-msg never calls the message arm"
    fi
    if command grep -q 'private_names_gate_range' "$here/pre-push"; then
        _ok "pre-push calls the range arm"
    else
        _no "pre-push never calls the range arm"
    fi

    rm -rf "$tmp"
    echo "private-names selftest: ${pass}/$((pass + fail)) arm(s) pass"
    [ "$fail" -eq 0 ]
}

# A bare run is answered, never passed in silence: the tooling must not answer "success" to a
# question it never heard (see a_hook_library_run_directly_says_so).
if [ "${BASH_SOURCE[0]}" = "${0}" ]; then
    . "$(dirname "${BASH_SOURCE[0]}")/scratch-guard.sh"
    # The caller's git environment is cut before any arm runs: the fixtures below run git in
    # throwaway repositories, and an absolute GIT_INDEX_FILE (which `git commit -- <path>` exports
    # to its hooks) outranks the `cd` and would land in the caller's index instead.
    scratch_guard_cut_ambient
    case "${1:-}" in
        --selftest) private_names_selftest; exit $? ;;
        *) echo "private-names-gate.sh is a LIBRARY, not a command: it is sourced by pre-commit," >&2
           echo "commit-msg and pre-push." >&2
           echo "usage: private-names-gate.sh --selftest   (the gate itself needs a hook's arguments)" >&2
           exit 2 ;;
    esac
fi
