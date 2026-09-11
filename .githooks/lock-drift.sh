#!/usr/bin/env bash
# WHETHER A MEMBER'S MANIFEST HAS MOVED PAST `Cargo.lock` -- register item 1036.
#
# ⛔⛔⛔⛔⛔ WHAT HAPPENED, IN THIS WORKSPACE, ON 2026-09-11
#
# `4a9b893f` added one line to `crates/sprag-plugin/Cargo.toml` -- `sprag-gate`
# under `[dev-dependencies]` -- and did not update `Cargo.lock`, whose
# `sprag-plugin` entry went on listing nine dependencies without it. EVERY gate
# this workspace has passed that commit: rustfmt, `cargo clippy --workspace
# --all-targets -- -D warnings`, the rustdoc gate, the three ratchets,
# `validate-workspace` (T1 orphan 0) and `validate-code-refs` at the push, and
# the full `sprag-plugin` + `sprag-host` + `sprag-gate` suites, zero failures.
#
# The only thing that noticed was the debt loop's own `successor_check`, which
# runs `cargo run -q --locked ... --admits` and answered *"error: cannot update
# the lock file ... because --locked was passed"* with `rc=101` -- NOT A VERDICT.
# The round could not ask what to work on next, and the defect was found by a
# loop failing to speak rather than by anything measuring it.
#
# ⛔⛔⛔⛔⛔ WHY THIS IS A SHELL GATE AND NOT A TEST -- MEASURED, AFTER THE TEST
# WAS WRITTEN AND DID NOT RING
#
# The first repair was a `sprag-gate` test that read the same two files. With
# the drift staged it passed, on the build host AND locally, and the reader was
# correct. What was wrong was the PLACE: `cargo test` resolves the workspace
# before it runs anything, and resolving WRITES. Measured -- `git diff --stat
# Cargo.lock` after that run said `1 insertion(+)`, and the lock's `sprag-detect`
# entry had gained the very edge the test was about to look for.
#
# ⇒ **A TEST CANNOT MEASURE THIS, EVER: the tool that runs it repairs the thing
# it measures on the way in.** The gate is green, and green about nothing.
#
# ⚠⚠ It is the THIRD face of one mechanism, and the other two are already paid
# for: register item 841 (the classifier answered `YES STEP` and rewrote the
# lockfile in the same second, digest `82c008d5...` -> `dfa09b73...`, which is
# why `--locked` is in its argv) and register item 105 (`bx`'s remote build
# rewrites the lock and the local tree never learns). So `--locked` over there
# is not a wart to remove -- it is the defence, and this is its missing half:
# something that NOTICES the drift before the classifier has to go silent.
#
# ⚠ AND `cargo metadata --locked --offline` IS NOT THE PREDICATE EITHER. It was
# the second draft: green locally, red on the build host with *"failed to
# download `accesskit_ios v0.1.1` ... --offline was specified"*. Resolving reads
# every dependency's own manifest, so `--offline` does not remove the network
# dependence -- it converts it into a CACHE dependence, and a cache is a property
# of the machine rather than of the tree.
#
# ⚠⚠ WHAT THIS DELIBERATELY DOES NOT CLAIM: nothing about registry versions --
# their edges cannot be checked without resolving, which is the download this
# gate exists without. What it does claim is the edge this workspace actually
# breaks: one member depending on another. That is what `4a9b893f` did.

# Every workspace-member edge a member's manifest declares that its `Cargo.lock`
# entry does not carry, one per line, empty when the lock is whole.
#
# ⚠ Pure text, by awk, because the alternative is the tool that repairs the
# subject. The three dependency tables are read, target-specific ones included
# (`[target.'cfg(unix)'.dev-dependencies]` -- the table KIND is what matters and
# it is the part after the last dot), and both spellings cargo allows are taken:
# `name = { ... }` and `name.workspace = true`.
lock_drift() {
    local root
    root="$(git rev-parse --show-toplevel)" || return 1
    lock_drift_in "$root"
}

# [`lock_drift`] against a tree the CALLER names — extracted so the selftest can hand it a case the
# real workspace cannot hold, which is register items 792 and 799's shared rule: when the
# population cannot produce the case a guard is for, do not assert around the guard.
lock_drift_in() {
    local root="$1"
    [ -n "$root" ] || return 1

    awk -v root="$root" '
        function table_kind(header,   n, parts) {
            n = split(header, parts, ".")
            return parts[n]
        }
        BEGIN {
            # ── ① which package each member manifest declares, and what it needs ──
            cmd = "ls -d " root "/crates/*/ 2>/dev/null"
            while ((cmd | getline dir) > 0) {
                manifest = dir "Cargo.toml"
                name = ""; section = ""
                while ((getline line < manifest) > 0) {
                    gsub(/^[ \t]+|[ \t]+$/, "", line)
                    if (line ~ /^\[.*\]$/) {
                        section = substr(line, 2, length(line) - 2)
                        continue
                    }
                    if (line == "" || line ~ /^#/) continue
                    if (section == "package" && name == "" && line ~ /^name[ \t]*=/) {
                        split(line, kv, "=")
                        gsub(/^[ \t]*"|"[ \t]*$/, "", kv[2])
                        name = kv[2]
                        continue
                    }
                    kind = table_kind(section)
                    if (kind != "dependencies" && kind != "dev-dependencies" && kind != "build-dependencies") continue
                    if (line !~ /=/) continue
                    key = line
                    sub(/=.*$/, "", key)
                    gsub(/^[ \t]+|[ \t]+$/, "", key)
                    sub(/\..*$/, "", key)
                    if (key != "" && name != "") needs[name "\t" key] = 1
                    else if (key != "") pending[dir "\t" key] = 1
                }
                close(manifest)
                if (name != "") { member[name] = 1; home[name] = dir }
                for (p in pending) {
                    split(p, at, "\t")
                    if (at[1] == dir && name != "") needs[name "\t" at[2]] = 1
                    delete pending[p]
                }
            }
            close(cmd)

            # ── ② what the lock says each of them already carries ──
            lockfile = root "/Cargo.lock"
            package = ""; collecting = 0
            while ((getline line < lockfile) > 0) {
                gsub(/^[ \t]+|[ \t]+$/, "", line)
                if (line == "[[package]]") { package = ""; collecting = 0; continue }
                if (line ~ /^name[ \t]*=/) {
                    split(line, kv, "=")
                    gsub(/^[ \t]*"|"[ \t]*$/, "", kv[2])
                    package = kv[2]
                    continue
                }
                if (line ~ /^dependencies[ \t]*=[ \t]*\[/) { collecting = 1; continue }
                if (collecting) {
                    if (line == "]") { collecting = 0; continue }
                    entry = line
                    sub(/,$/, "", entry)
                    gsub(/"/, "", entry)
                    sub(/ .*$/, "", entry)
                    if (package != "" && entry != "") carried[package "\t" entry] = 1
                }
            }
            close(lockfile)

            # ── ③ the difference, and only for edges between MEMBERS ──
            for (edge in needs) {
                split(edge, at, "\t")
                if (!(at[2] in member)) continue
                if (!(edge in carried))
                    printf "  %s declares %s and the lock entry for %s does not carry it\n", at[1], at[2], at[1]
            }
        }
    ' < /dev/null
}

# ⛔⛔⛔⛔⛔ THE SELFTEST, AND IT STAGES ITS DRIFT IN A SCRATCH TREE FOR THE REASON THIS WHOLE
# GATE MOVED HERE -- register item 1036. Drift cannot be staged in the REAL workspace and then
# measured, because anything that builds or tests repairs the lock on its way in; a scratch tree
# nothing ever resolves is the only place the two states can both exist to be compared.
#
# ⚠ It takes `--root` rather than reading the repository, so the arms drive the same `lock_drift`
# the hook calls against a tree the caller owns -- the extraction item 799 and item 792 both
# argue for: when the real population cannot produce the case, hand the decision the case.
lock_drift_selftest() {
    local pass=0 fail=0 tmp root out scratch_refusal

    tmp="$(mktemp -d "${TMPDIR:-/tmp}/lock-drift-selftest.XXXXXX")" || tmp=""
    if [ -z "$tmp" ] || [ ! -d "$tmp" ]; then
        echo "lock-drift selftest: no scratch directory could be made" >&2
        return 1
    fi
    root="$tmp/repo"
    mkdir -p "$root/crates/alpha" "$root/crates/beta"
    (
        cd "$root" || exit 1
        git init -q .
        git config user.email selftest@example.invalid
        git config user.name "lock-drift selftest"
    ) >/dev/null 2>&1

    # ⛔ THE SUBJECT IS THE SCRATCH REPOSITORY OR THIS DOES NOT RUN -- register item 792, whose
    # selftest re-initialised the REAL repository. Every arm below writes into `$root`.
    scratch_refusal="$(scratch_guard_check "$root" 2>/dev/null || true)"
    if [ -n "$scratch_refusal" ]; then
        echo "lock-drift selftest: REFUSING to run -- ${scratch_refusal}" >&2
        rm -rf "$tmp"
        return 1
    fi

    printf '[package]\nname = "alpha"\n\n[dependencies]\nbeta = { workspace = true }\n' \
        >"$root/crates/alpha/Cargo.toml"
    printf '[package]\nname = "beta"\n' >"$root/crates/beta/Cargo.toml"

    # ⑴ A LOCK THAT CARRIES THE EDGE IS SILENT.
    printf '[[package]]\nname = "alpha"\ndependencies = [\n "beta",\n]\n\n[[package]]\nname = "beta"\n' \
        >"$root/Cargo.lock"
    out="$(lock_drift_in "$root")"
    if [ -z "$out" ]; then
        pass=$((pass + 1))
        echo "  ok    a lock that carries every member edge says nothing"
    else
        fail=$((fail + 1))
        echo "  FAIL  a whole lock was reported as drifted: ${out}" >&2
    fi

    # ⑵ THE MUTATION -- the manifest gains an edge and the lock does not. This is the exact shape
    # `4a9b893f` committed, and the arm this gate exists for.
    printf '[package]\nname = "alpha"\n\n[dependencies]\nbeta = { workspace = true }\n\n[dev-dependencies]\ngamma = { workspace = true }\n' \
        >"$root/crates/alpha/Cargo.toml"
    mkdir -p "$root/crates/gamma"
    printf '[package]\nname = "gamma"\n' >"$root/crates/gamma/Cargo.toml"
    out="$(lock_drift_in "$root")"
    case "$out" in
        *"alpha declares gamma"*)
            pass=$((pass + 1))
            echo "  ok    a member edge the lock does not carry is named"
            ;;
        *)
            fail=$((fail + 1))
            echo "  FAIL  the drift this gate exists for was not reported: ${out:-<nothing>}" >&2
            ;;
    esac

    # ⑶ AND A NON-MEMBER EDGE IS NOT ONE OF ITS CLAIMS -- the narrowing stated in this file's
    # header, driven rather than promised. A registry dependency cannot be checked without
    # resolving, so reporting one would be a claim this gate cannot support.
    printf '[package]\nname = "beta"\n\n[dependencies]\nserde = { workspace = true }\n' \
        >"$root/crates/beta/Cargo.toml"
    out="$(lock_drift_in "$root")"
    case "$out" in
        *"beta declares serde"*)
            fail=$((fail + 1))
            echo "  FAIL  a registry dependency was reported, which this gate cannot judge" >&2
            ;;
        *)
            pass=$((pass + 1))
            echo "  ok    a non-member dependency is outside what this gate claims"
            ;;
    esac

    rm -rf "$tmp"
    echo "lock-drift selftest: ${pass}/$((pass + fail)) arm(s) pass"
    [ "$fail" -eq 0 ]
}

# The gate: refuse a commit whose manifests the lock has not caught up with.
lock_drift_gate() {
    local drift
    drift="$(lock_drift)" || {
        echo "lock-drift: could not read this workspace's manifests" >&2
        return 1
    }
    [ -z "$drift" ] && return 0

    echo "pre-commit: REGISTER ITEM 1036 -- a manifest has moved past Cargo.lock:" >&2
    printf '%s\n' "$drift" >&2
    echo "pre-commit: run \`cargo metadata --offline\` to write the lock, and commit it WITH" >&2
    echo "pre-commit: the manifest that needs it -- ONE commit, because a lock that lands" >&2
    echo "pre-commit: later is a commit nobody could build from." >&2
    echo "pre-commit: why this is a gate: measured on 4a9b893f, a stale lock passed rustfmt," >&2
    echo "pre-commit: clippy, rustdoc, all three ratchets, both mnemosyne gates and every" >&2
    echo "pre-commit: crate suite. The only thing that noticed was the debt loop's own" >&2
    echo "pre-commit: successor_check going SILENT -- it runs --locked on purpose (item 841)," >&2
    echo "pre-commit: so a stale lock costs a round its ability to ask what comes next." >&2
    return 1
}

# ⚠ Sourced by `pre-commit` as a LIBRARY; run directly it drives its own arms. The guard is
# `BASH_SOURCE` rather than `$1`, on `tree-drift.sh`'s measured reason: a sourced file sees the
# CALLER's positional parameters, so keying on `$1` would fire inside a hook that happened to have
# one.
if [ "${BASH_SOURCE[0]}" = "${0}" ]; then
    # shellcheck source-path=SCRIPTDIR
    . "$(dirname "${BASH_SOURCE[0]}")/scratch-guard.sh"
    scratch_guard_cut_ambient
    case "${1:-}" in
        --selftest)
            lock_drift_selftest
            exit $?
            ;;
        *)
            echo "lock-drift.sh is a LIBRARY, not a command: it is sourced by pre-commit." >&2
            echo "usage: lock-drift.sh --selftest" >&2
            exit 2
            ;;
    esac
fi
