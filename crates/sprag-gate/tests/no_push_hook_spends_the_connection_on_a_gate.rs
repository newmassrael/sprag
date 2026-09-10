//! **A PUSH HOOK MAY NOT SPEND AN OPEN GITHUB CONNECTION ON A GATE THAT COMPILES** — register
//! item 480's remaining half.
//!
//! # ⛔⛔⛔⛔⛔ What it cost, measured rather than feared
//!
//! `git push` OPENS THE CONNECTION FIRST and runs `pre-push` after it, so every second the hook
//! spends is spent on an open ssh session. Measured 2026-08-20 while paying register item 456: the
//! rustdoc gate took **7m15s**, GitHub answered *"Connection to github.com closed by remote host"*,
//! and `git push` exited **141 (SIGPIPE) after every gate had PASSED** — `git ls-remote origin
//! main` confirmed the ref had not moved. A green gate lost the push.
//!
//! ⚠⚠ **AND THE RETRY SUCCEEDED IN SECONDS**, against a warm `target/doc-gate`, which is exactly
//! why this reads as a flake and is not one: it is a race between one command's runtime and a
//! remote's idle timeout, and the loser is decided by cache temperature. The item says in as many
//! words that **a retry loop is not the fix** — it hides which of the three things happened.
//!
//! # ⚠⚠⚠ What this gate holds, stated no wider than it is
//!
//! **`pre-push` does not itself invoke clippy or the rustdoc gate.** It reads a stamp
//! `.githooks/rust-gates.sh` writes and, where the stamp does not cover the tree being pushed,
//! REFUSES with that command named.
//!
//! # ⛔⛔⛔⛔⛔ It used to hold LESS THAN ITS TITLE, and this is the round that closed the gap
//!
//! Until register item 1005 this file said, in as many words: *it does NOT hold that nothing in
//! `pre-push` compiles — `run_hook_suite` and `run_pixel_smoke` still build, inside the same
//! connection*. That sentence was honest and it was a hole with a reason attached: item 480's own
//! done-when is **the hook cannot spend an open GitHub connection on a multi-minute gate**, stated
//! generally, and only the Rust gates had moved.
//!
//! ⚠⚠ The two that stayed were not small. `run_pixel_smoke` was `cargo build --release` over three
//! crates. `run_hook_suite` carried a comment measuring *"the suite it runs finished in under a
//! second"* — its SUITE, not its BUILD, and a cold tree pays minutes before a test runs.
//!
//! ⚠ They are rarer than the Rust gates: each is owed only when the push touches the paths it
//! reads. Rarer is not safer — the push that does touch them is the one that pays 7m15s on an open
//! session and loses the ref after passing.
//!
//! # ⚠⚠ Why the refusal is checked too, and not only the absence
//!
//! An absence alone is satisfied by a hook that simply stopped checking — which is the same
//! publication hole facing the other way, and the reason `pre-push` exists at all. So three things
//! are asked together: the heavy commands are GONE from the push hook, they are ALIVE in the
//! library, and the push hook REFUSES by naming that library. Drop any one and the other two are
//! satisfiable by a mistake.

use sprag_gate::sources::workspace_root;

/// The push hook, and the library the lane moved into.
const PUSH: &str = ".githooks/pre-push";
const LIBRARY: &str = ".githooks/rust-gates.sh";
/// The commit hook, which is where these gates are SUPPOSED to run: before any connection exists.
const COMMIT: &str = ".githooks/pre-commit";

/// A hook's text with its comment lines dropped — a hook that EXPLAINS the shape it must not have
/// is not the shape, and every one of these files carries exactly that explanation.
///
/// ⚠ Dropped by line, which is what these files are written in: no `.githooks` script puts code
/// after a `#` on the same line, and a reader that tried to would have to read shell quoting.
fn code_of(hook: &str) -> String {
    let path = workspace_root().join(hook);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|why| panic!("{hook} is a hook of this repository: {why}"));
    text.lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A hook's code with the lines that PRINT dropped as well — what it RUNS, and nothing it says.
///
/// # ⛔⛔⛔⛔⛔ Why an absence test needs this, measured on this very file
///
/// `pre-push` spells `cargo test -p sprag-gate && …` on purpose: item 784's reader walks this hook
/// for exactly those targets, and item 480's refusal must name the lane a person is to run. So the
/// hook MENTIONS commands it must never RUN, and a plain `contains` cannot tell an `echo` from a
/// call — the ambiguity this file's own comment names, and that
/// `a_fleet_ceiling_is_a_measurement_with_a_date` had to answer with a command-position rule.
///
/// ⚠⚠ The rule here is that one, at its narrowest: a line whose first word is `echo` runs nothing.
/// Every gate invocation in these hooks is a bare call at the head of a line — after `if !` at
/// most — so nothing that runs is lost, and the two lanes a refusal prints stop reading as
/// invocations of themselves.
fn commands_of(hook: &str) -> String {
    code_of(hook)
        .lines()
        .filter(|line| !line.trim_start().starts_with("echo "))
        .collect::<Vec<_>>()
        .join("\n")
}

/// ⛔ **THE GATE.**
#[test]
fn the_push_hook_names_the_rust_gates_and_does_not_run_them() {
    let push = code_of(PUSH);
    let commands = commands_of(PUSH);
    let library = code_of(LIBRARY);
    let commit = code_of(COMMIT);

    // ⚠⚠⚠ AND THE NARROWING IS ITSELF LOAD-BEARING, so it is asserted rather than trusted: if
    // dropping `echo` lines removed nothing, the projection is inert and clause 1 is back to a
    // plain `contains` that cannot tell a printed lane from a run one.
    assert!(
        commands.len() < push.len(),
        "⚠⚠⚠ `commands_of` dropped no line of `{PUSH}`. It exists because this hook PRINTS the \
         lane a person must run (item 784's reader walks it for those targets), and a projection \
         that removed nothing would make clause 1 unable to tell that echo from an invocation",
    );

    // ⚠⚠⚠ THE PREMISE, FIRST: a walk that found an empty hook would satisfy every absence below by
    // having read nothing. This is the shape register item 819's neighbours are all built on — a
    // probe pointed at nothing must never read as clean.
    assert!(
        push.len() > 2_000 && library.len() > 1_000,
        "⚠⚠⚠ THIS GATE'S OWN PREMISE FAILED: it read {} byte(s) of `{PUSH}` and {} of \
         `{LIBRARY}`. Every claim below is an ABSENCE, and an absence in a file nobody read is \
         free",
        push.len(),
        library.len(),
    );

    // ── ⛔⛔⛔ 1. THE HEAVY COMMANDS ARE GONE FROM THE PUSH HOOK ──
    //
    // ⚠⚠ THE LAST TWO ARE REGISTER ITEM 1005's, and they are what this list was missing while its
    // own header said so: `cargo build --release` is the pixel smoke's set, `cargo test` is the
    // suite that drives these hooks. Neither may be spelled in a file that runs inside a
    // connection — and `xvfb-run` with them, because the smoke's own body moved too.
    for spent in [
        "cargo clippy",
        "doc_gate",
        "RUSTDOCFLAGS",
        "cargo build --release",
        "cargo test",
        "xvfb-run",
    ] {
        assert!(
            !commands.contains(spent),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 480: `{PUSH}` invokes `{spent}`. `git push` opens its \
             connection BEFORE this hook runs, so that command is spent on an open ssh session — \
             measured at 7m15s, after which GitHub closed the connection and `git push` exited 141 \
             with every gate PASSED and `origin/main` unmoved. Move it to `{LIBRARY}`, which runs \
             outside any connection, and refuse here instead. ⚠ A retry loop is not the fix: it \
             hides which of the three things happened.",
        );
    }

    // ── ⛔⛔⛔ 2. AND THEY ARE ALIVE IN THE LIBRARY ──
    //
    // ⚠⚠ Without this the clause above is satisfied by DELETING the gates, which is the same
    // publication hole facing the other way — a push hook that checks nothing passes it perfectly.
    for owed in [
        "cargo clippy",
        "RUSTDOCFLAGS",
        "cargo test -p sprag-gate",
        "cargo build --release",
        "xvfb-run",
    ] {
        assert!(
            library.contains(owed),
            "⛔⛔⛔⛔ REGISTER ITEM 480: `{LIBRARY}` does not carry `{owed}`. The push hook is \
             allowed to stop running these only because this file runs them — a tree nothing \
             compiled must not be publishable, and the absence above would then be a hole rather \
             than a repair.",
        );
    }

    // ── ⛔⛔⛔ 3. AND THE COMMIT HOOK REACHES THEM, which is where they belong ──
    //
    // ⚠ *Before the connection is opened* is item 480's own first done-when, and `pre-commit` is
    // the only hook that runs before one exists.
    assert!(
        commit.contains("rust_gates_run"),
        "⛔⛔⛔⛔ REGISTER ITEM 480: `{COMMIT}` does not call `rust_gates_run`. These gates have to \
         run SOMEWHERE before a connection is open, and the commit hook is the only place there is \
         — a repair that only removed them from the push hook would publish trees nothing compiled.",
    );

    // ── ⛔⛔⛔ 4. AND THE PUSH HOOK REFUSES BY NAMING THE COMMAND ──
    //
    // ⚠⚠ A refusal whose remedy is not a command is a WALL. The stamp is written by `pre-commit`,
    // and after a commit there is no `.rs` staged, so nothing a person could type would clear it.
    // The verb is required because a bare invocation of that library must stay silent-free and
    // cheap — see its own dispatch, and `no_hook_library_run_with_no_arguments_is_silently_successful`.
    assert!(
        push.contains("rust-gates.sh --clear"),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 480: `{PUSH}` does not name `rust-gates.sh --clear`. Refusing a \
         push without naming the command that clears it is a wall: the stamp is written by the \
         COMMIT hook, and after a commit there is no `.rs` staged, so a person meeting this \
         refusal has nothing they can type. A refusal names the command that builds it.",
    );

    // ── ⛔⛔⛔ 5. AND SO DOES EACH GATE ITEM 1005 MOVED ──
    //
    // ⚠⚠ ONE VERB PER GATE, spelled in both files: the push hook prints it and the library answers
    // to it. Held from both sides because either alone is satisfiable by a typo — a refusal naming
    // a verb nothing implements is the wall this clause exists to prevent, and a verb nobody names
    // is a gate that can only be cleared by reading the library.
    //
    // ⚠ THE VERB ALONE, not `rust-gates.sh <verb>` joined: the hook prints the script once and
    // passes the verb in, so the joined string is never written down. Demanding it would force the
    // path to be repeated at every call site — a worse file, to satisfy a needle. Clause 4 above
    // already holds that this hook names the script.
    for verb in ["--clear-pixel", "--clear-hooks"] {
        assert!(
            push.contains(verb),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 1005: `{PUSH}` refuses a push that owes this gate and does \
             not name `{verb}`. Nothing writes these stamps as a side effect — unlike the Rust \
             gates, which `pre-commit` clears while committing — so a person who edited a hook or \
             a painted crate has NOTHING they can type unless this file says it.",
        );
        assert!(
            library.contains(verb),
            "⛔⛔⛔⛔ REGISTER ITEM 1005: `{LIBRARY}` does not answer to `{verb}`, which `{PUSH}` \
             tells people to run. A refusal whose remedy does not exist is worse than the gate it \
             replaced: the push is stopped and the command fails.",
        );
    }
}
