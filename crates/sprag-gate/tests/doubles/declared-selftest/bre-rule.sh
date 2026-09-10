# THE ONE RULE ABOUT A BASIC REGULAR EXPRESSION, sourced by every stand-in that
# enforces it -- register item 1007.
#
# ⛔⛔⛔⛔⛔ WHY IT IS A FILE AND NOT A COPY IN EACH DOUBLE. `sed` and `grep` hand
# the SAME text to the SAME `regcomp`, so *what BSD reads differently* is one
# fact about basic regular expressions and not two facts about two programs. Two
# copies of it drift, and they drift in the direction that matters: the copy a
# hook actually meets and the copy a gate reads would stop agreeing, and nothing
# would say so. This workspace has paid for that shape more than once.
#
# ⚠⚠ MEASURED 2026-09-10 against a FreeBSD regex(3), and a FreeBSD sed(1) and
# grep(1) built on top of it. The measurement is what chose THESE THREE rather
# than a longer list:
#   silent: \| \+ \?          -- a literal, no error, a different answer
#   loud:   \b \B \w \W \s \S -- BSD refuses the expression outright
#   SAME:   \< \> \t \n \{ \( -- both read them the same way
# Only the silent three can be green on Linux and wrong on macOS, which is the
# whole shape these stand-ins exist for. Guessing would have put \< and \> on the
# list; the measurement says they belong on neither.
#
# ⚠ THE RESIDUE, STATED: the scan reads the whole pattern, so a `\|` on the
# REPLACEMENT side of sed's `s` command -- where it is not a regular expression
# at all -- would be refused for a reason that is not true. Nothing in this tree
# spells one. Separating the two sides needs a parser this cannot be, and the
# remedy if it ever bites is a bracket: `[|]` says the same thing.

# The caller's own name, for a refusal a reader can act on.
bre_program="${bre_program:-a POSIX double}"
# The one command the caller can offer, since the repair differs by program.
bre_remedy="${bre_remedy:-Write the POSIX spelling.}"
# 1 when the pattern in hand is NOT a basic regular expression -- an extended one
# (`-E`) or a fixed string (`-F`). The rule simply does not apply there: those
# characters are operators unescaped, and `\+` is a literal on BOTH platforms, so
# refusing would be refusing something portable. Each caller reads its own argv.
bre_extended="${bre_extended:-0}"

# Refuse the pattern in $1 for spelling $2, which BSD reads as the literal $3.
bre_refuse() {
    printf '%s\n' \
        "${bre_program}: REFUSING a pattern BSD reads differently:" \
        "  $1" \
        "  '$2' is a GNU extension to a BASIC regular expression. A BSD regex(3)" \
        "  reads it as the literal character '$3', so this matches nothing there," \
        "  exits 0, and the caller acts on an empty answer -- which is how a" \
        "  refusal ends up naming the wrong cause, and how a COUNT becomes 0" \
        "  without anybody being told. Register item 1007." \
        "  ${bre_remedy}" >&2
    exit 2
}

# Refuse $1 when it spells one of the three the other platform reads silently.
bre_scan() {
    [ "$bre_extended" -eq 0 ] || return 0
    case "$1" in
        *'\|'*) bre_refuse "$1" '\|' '|' ;;
    esac
    case "$1" in
        *'\+'*) bre_refuse "$1" '\+' '+' ;;
    esac
    case "$1" in
        *'\?'*) bre_refuse "$1" '\?' '?' ;;
    esac
}
