//! **A HOOK LIBRARY RUN DIRECTLY SAYS SO** — register item 819, and the FIFTH shape in which a
//! hook of this repository passes in silence.
//!
//! # ⛔⛔⛔⛔⛔ What it costs, measured on the round that wrote this gate's siblings
//!
//! `.githooks/doc-gate.sh` defines one function and is SOURCED by `pre-commit` and `pre-push`. Run
//! it — `bash .githooks/doc-gate.sh` — and it exits **0 having printed nothing**, because defining
//! a function is all it does. On 2026-09-02 a round did exactly that to check its work before
//! committing, read the zero as a pass, and was refused by the real gate one minute later with two
//! rustdoc `private_intra_doc_links`.
//!
//! ⚠⚠ That is worse than a hook that fails to check something, which is what
//! [`hooks_cannot_pass_in_silence`](../hooks_cannot_pass_in_silence.rs) is about: **it defeats the
//! attempt to check by hand.** The person is doing the right thing and the tooling answers
//! *success* to a question it never heard.
//!
//! # ⚠⚠⚠ The predicate, and why it is not *must refuse*
//!
//! Two of these files ANSWER a bare invocation and should keep doing so: `hosted-read.sh` prints
//! the gap report, `loop-read.sh` prints what is owed. Demanding a refusal from every file would
//! make those two worse. So what is forbidden is the combination that carries no information at
//! all: **exit 0 with nothing on either stream.** Answer, or refuse — silence-and-success is
//! neither, and it is the only shape a reader cannot tell from a pass.
//!
//! ⚠ RE-MEASURED rather than taken from the register, which is this workspace's rule 4 — and it
//! paid: item 819 named `doc-gate.sh` and `content-gate.sh` and explicitly cleared `ident-gate.sh`
//! ("`$1` 분기를 갖고 직접 실행에 답한다"). Running all six found `ident-gate.sh` silent too: its
//! only arm was `--selftest`, so every other invocation fell off the end at exit 0.
//!
//! # ⚠⚠ What this gate DOES that a scan could not, and what it costs
//!
//! It RUNS each library, so it is the one gate here whose subject is behaviour rather than text.
//! That is only safe while a bare invocation has no side effects — today it prints usage or a
//! report and touches nothing. **A library that grows a default action which writes something is a
//! library this gate would then perform on every test run**, and whoever adds one has to give it
//! an argument-taking dispatch instead. Stated here rather than discovered later.
//!
//! ⚠ `.sh` only. `commit-msg`, `pre-commit` and `pre-push` are the hooks themselves; running those
//! would run the real gates.

use std::path::PathBuf;
use std::process::Command;

/// Every library in `.githooks/`, as `(name, path)` — found by WALKING the directory, because a
/// hardcoded list decides alone which files are looked at and the one it leaves out is the one
/// nobody is watching. That is [`hooks_cannot_pass_in_silence`]'s own rule, one file over.
fn hook_libraries() -> Vec<(String, PathBuf)> {
    let dir = sprag_gate::sources::workspace_root().join(".githooks");
    let mut found: Vec<(String, PathBuf)> = std::fs::read_dir(&dir)
        .unwrap_or_else(|why| panic!("{} is this repo's hooks: {why}", dir.display()))
        .map(|entry| entry.expect("read a hook directory entry").path())
        .filter(|path| path.extension().is_some_and(|kind| kind == "sh"))
        .map(|path| {
            let name = path
                .file_name()
                .expect("a library has a name")
                .to_string_lossy()
                .into_owned();
            (name, path)
        })
        .collect();
    found.sort();
    found
}

/// ⛔ **THE GATE.**
#[test]
fn no_hook_library_run_with_no_arguments_is_silently_successful() {
    let root = sprag_gate::sources::workspace_root();
    let libraries = hook_libraries();
    assert!(
        libraries.len() >= 6,
        "this scan found {} librar(ies) in .githooks and this repository has at least six — it is \
         pointed at the wrong tree, and a probe pointed at nothing must never read as clean: {:?}",
        libraries.len(),
        libraries.iter().map(|(name, _)| name).collect::<Vec<_>>(),
    );

    let mut silent = Vec::new();
    let mut answered = Vec::new();
    for (name, path) in &libraries {
        let ran = Command::new("bash")
            .arg(path)
            .current_dir(&root)
            .output()
            .unwrap_or_else(|why| panic!("{name} must be runnable by bash: {why}"));
        let said = ran.stdout.len() + ran.stderr.len();
        if ran.status.success() && said == 0 {
            silent.push(name.clone());
        } else {
            answered.push(format!(
                "{name}: exit {} with {said} byte(s)",
                ran.status.code().unwrap_or(-1),
            ));
        }
    }

    assert!(
        silent.is_empty(),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 819: {silent:?} exited 0 having said NOTHING. A sourced library \
         that is run does exactly nothing, successfully — and a person checking their work by hand \
         reads that zero as a pass, which is how a round shipped two rustdoc errors it had just \
         'verified'. Give it a `if [ \"${{BASH_SOURCE[0]}}\" = \"${{0}}\" ]` dispatch that either \
         does the thing or says `usage` with a non-zero status, as `scratch-guard.sh` always has.\n\
         What the others said: {answered:?}",
    );
}
