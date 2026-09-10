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
//! ⚠ It does NOT hold that nothing in `pre-push` compiles: `run_hook_suite` and `run_pixel_smoke`
//! still build, inside the same connection, and that is registered rather than papered over here.
//! A gate whose sentence is wider than its predicate is this register's own recurring defect.
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

/// ⛔ **THE GATE.**
#[test]
fn the_push_hook_names_the_rust_gates_and_does_not_run_them() {
    let push = code_of(PUSH);
    let library = code_of(LIBRARY);
    let commit = code_of(COMMIT);

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
    for spent in ["cargo clippy", "doc_gate", "RUSTDOCFLAGS"] {
        assert!(
            !push.contains(spent),
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
    for owed in ["cargo clippy", "RUSTDOCFLAGS", "cargo test -p sprag-gate"] {
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
}
