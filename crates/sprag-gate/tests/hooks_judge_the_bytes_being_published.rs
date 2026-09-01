//! **THE COMMIT GATE READ THE FILE ON DISK AND CALLED IT THE COMMIT** — register item 404, third
//! payment, and the first that drives `pre-commit` and `pre-push`.
//!
//! # ⚠⚠⚠ Why this file exists
//!
//! Item 404's earlier payments drove `commit-msg` (the one hermetic hook) and ratcheted three
//! SHAPES across every file in `.githooks/`. Both said out loud what they did not cover: **the
//! BEHAVIOUR of `pre-commit`'s gates and `pre-push`'s**, which need `mnemosyne-cli`, a cargo
//! toolchain and an X server and so are not hermetic. That is the debt this file pays, and the way
//! it pays it is the way item 403 was paid — a PATH of doubles, so the hook runs as the program git
//! runs while the tools it shells out to are ours.
//!
//! What running them found, on the first case that separated the index from the working tree:
//!
//!   * `pre-commit` took the staged NAME LIST and then handed rustfmt the WORKING-TREE BYTES, so
//!     the gate was wrong in **both** directions — a commit carrying unformatted Rust passed (stage
//!     it, then format the file), and a commit carrying perfectly formatted Rust was refused (stage
//!     it, then edit the file). Its own header claimed the opposite in as many words: *"checks
//!     exactly what is being committed, and nothing else"*. **That makes five for five** — 382,
//!     401, 314, 405, and now this: a doc in this tree claiming reach has never once survived being
//!     run.
//!   * `pre-push` has no format gate at all, while its header says it exists to catch the
//!     `--no-verify` bypass — and format is the gate a bypass most often steps over, since it is
//!     the one `pre-commit` owns alone.
//!   * both hooks show `validate-code-refs`'s report by filtering for a line beginning
//!     `violations:`, `|| true`. Item 213's fix therefore lives only as long as that word does: let
//!     the tool rename its summary and fifty-two findings go back to reaching nobody, silently.
//!
//! # ⚠⚠ What this file does NOT cover, said plainly so a green run is not misread
//!
//! `mnemosyne-cli`, `cargo` and `xvfb-run` are DOUBLES. Nothing here says the store is consistent,
//! that clippy is clean, that rustdoc resolves, or that the pixel smoke passes — only what the HOOK
//! does with what those tools answer. `rustfmt` is deliberately **not** doubled: the format gate is
//! the subject, and a doubled rustfmt would be asserting about the double.
//!
//! The pixel smoke is never actually run. Its cases assert which DECISION the hook reached, because
//! that decision — read the pushed range, answer *owed* on anything it cannot resolve — is the
//! logic, and it had never been executed by anything before this file.

use sprag_gate::doubles::Doubles;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

/// The tree these hooks belong to — through the one door, register item 809.
///
/// ⚠ A private `env!("CARGO_MANIFEST_DIR")` walk answers about the tree this test was COMPILED in,
/// which stopped being the tree it runs in. `workspace_root` refuses when the two differ.
fn repo_root() -> PathBuf {
    sprag_gate::sources::workspace_root()
}

/// **THE IDENTITY THIS REPOSITORY'S HOOKS ACCEPT**, read from the gate that enforces it —
/// register item 688.
///
/// # ⚠⚠⚠⚠⚠ Seventeen of this file's tests died on a gate none of them is about
///
/// This sandbox committed as `gate@example.invalid`, and `c893e39` put
/// `.githooks/ident-gate.sh` on `pre-commit` and `pre-push`. The hooks are LINKED in here
/// ([`Sandbox::link_hooks`], register item 467) precisely so what runs is the real one — so the
/// real one refused **every commit this suite makes**: `6 passed; 17 failed`, on both platforms,
/// with twenty-seven refusals naming that address. It rode out on **twenty-one consecutive
/// pushes** (2026-08-24 22:24 → 2026-08-26), because `pre-push` runs `validate-workspace`, clippy,
/// rustdoc and the pixel smoke — **and nothing in this crate.**
///
/// # ⚠⚠ The two obvious repairs were weighed and refused, and the reasons are the point
///
/// * **Add the sandbox's address to the allowlist.** That file's own doc says *"an edit here is a
///   statement about who may write history that the remote publishes"*, and the gate exists
///   because commits once reached a PUBLIC repository under a wrong address — the repository had
///   to be deleted and recreated. A real commit could then carry it.
/// * **Teach the gate to stand down outside its own repository.** It reads as the cleaner fix and
///   is the worse one: what runs here would be the real hook DISABLED, which is a weaker premise
///   than a copy, and the gate would then be absent exactly where nobody is looking.
///
/// Both fail in the direction the ident gate exists to prevent. The sandbox committing as an
/// accepted identity fails loudly instead — a red test, not a published commit.
///
/// # ⚠ Read rather than repeated
///
/// A literal copy of the address here is a second place to change, and this whole item is what a
/// second place costs. The allowlist is parsed out of the gate, so a round that changes who may
/// write history changes what this sandbox commits as in the same edit.
fn allowed_ident_email() -> String {
    let gate = repo_root().join(".githooks").join("ident-gate.sh");
    let text = std::fs::read_to_string(&gate)
        .unwrap_or_else(|why| panic!("read the ident gate at {}: {why}", gate.display()));
    // ⚠ THE ASSIGNMENT, not the `${SPRAG_ALLOWED_IDENT_EMAILS[0]}` the refusal message prints two
    // screens down — a plain search for the name would find the mention first.
    let (_, list) = text.split_once("SPRAG_ALLOWED_IDENT_EMAILS=(").unwrap_or_else(|| {
        panic!(
            "{} declares no `SPRAG_ALLOWED_IDENT_EMAILS=(` list, so this sandbox cannot tell which \
             identity the hooks it links will accept",
            gate.display(),
        )
    });
    let body = list.split_once(')').map_or(list, |(body, _)| body);
    body.split('"')
        .nth(1)
        .filter(|email| !email.is_empty())
        .unwrap_or_else(|| {
            panic!(
                "{}'s allowlist is empty, so there is no identity this sandbox may commit as: \
                 {body:?}",
                gate.display(),
            )
        })
        .to_owned()
}

/// Rust that `rustfmt --check` accepts unchanged.
const FORMATTED: &str = "fn paint() {}\n";

/// The same item, spaced so rustfmt rewrites it. `--check` answers 1 and names the file.
const UNFORMATTED: &str = "fn  paint (  )   { }\n";

/// ⚠⚠⚠⚠⚠ **THE DOUBLES ARE TRACKED FILES, NOT STRINGS THIS SUITE WRITES** — register item 467.
///
/// They used to be `const &str` bodies written into the sandbox's `bin/` and then executed, which
/// is `ETXTBSY` waiting to happen: the kernel refuses to execute a file any process holds open for
/// writing, and this harness runs its cases on THREADS of one process, so a case forking to spawn a
/// program inherits a sibling's open write handle and holds it until its own exec. Item 465
/// measured that shape on the neighbouring suite — **10 failures in 30 runs, 0 in 30 after** — and
/// this file carried five more of it.
///
/// They live in [`sprag_gate::doubles`]'s `hook-run` set: `mnemosyne-cli`, `cargo`, `actionlint`,
/// `git`, `xvfb-run`. What each one does is documented in the file itself, which is where a person
/// debugging a hook run will be looking.
///
/// ⚠ The one that had to change shape is `git`: it delegated to a path this file substituted into
/// its body at run time, and a tracked file cannot carry that. It now walks `PATH` skipping its own
/// directory, exactly as the `commit-msg` set's `grep` does.
fn doubles() -> Doubles {
    Doubles::of(env!("CARGO_MANIFEST_DIR")).set("hook-run")
}

/// A workflow this project's CI gate accepts.
const VALID_WORKFLOW: &str = "name: ci\non: push\njobs:\n  build:\n    runs-on: ubuntu-latest\n";

/// The same workflow carrying the marker the actionlint double refuses. It stands in for R343's
/// real defect — a `runner` context referenced where it does not exist — which `yaml.safe_load`
/// cannot see because the file PARSES.
const INVALID_WORKFLOW: &str =
    "name: ci # INVALID-WORKFLOW\non: push\njobs:\n  build:\n    runs-on: ubuntu-latest\n";

/// A repository of this test's own, holding the real `.githooks/` and a `bin/` of
/// doubles, so a hook runs exactly as git runs it without touching the developer's tree.
struct Sandbox {
    dir: PathBuf,
    bin: PathBuf,
    log: PathBuf,
    /// Tools taken off this sandbox's PATH entirely, by name.
    hidden: Vec<String>,
}

impl Sandbox {
    fn new(tag: &str) -> Sandbox {
        let dir =
            std::env::temp_dir().join(format!("sprag-gate-hookrun-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create a sandbox repository");
        let sandbox = Sandbox {
            bin: dir.join("bin"),
            log: dir.join("invocations"),
            dir,
            hidden: Vec::new(),
        };
        std::fs::create_dir_all(&sandbox.bin).expect("create the sandbox's PATH directory");

        sandbox.link_hooks();
        // `[ -f Cargo.toml ]` is what both hooks gate their cargo work on.
        sandbox.write("Cargo.toml", "[workspace]\nmembers = []\n");
        sandbox.double("mnemosyne-cli");
        sandbox.double("cargo");
        sandbox.double("actionlint");
        sandbox.double("xvfb-run");
        // ⛔⛔⛔ WITHOUT THIS THE HOOK REACHES GITHUB — register item 790. `hosted-read.sh` asks
        // whether a stepped-over commit ever had a run, and every sha in this sandbox exists
        // nowhere, so an undoubled suite would be measuring the network and the developer's `gh`
        // auth. `1` is the world every case written before that item assumes — a stepped-over
        // commit whose run is there and unread — and the cases for the other two answers stage
        // their own count over it.
        sandbox.double("gh");
        sandbox.write("gh-total-count", "1\n");

        sandbox.git(&["init", "-q", "."]);
        // ⚠⚠⚠⚠⚠ AN IDENTITY THE LINKED HOOKS ACCEPT — register item 688, and see
        // `allowed_ident_email` for why this is read from the gate rather than spelled here, and
        // why the two easier repairs were refused. The name stays obviously this suite's; only the
        // ADDRESS is load-bearing, because `ident_email_of` cuts on the angle brackets.
        sandbox.git(&["config", "user.email", &allowed_ident_email()]);
        sandbox.git(&["config", "user.name", "sprag-gate"]);
        // ⚠ This project sets `core.hooksPath` — without pinning it back the sandbox's own commits
        // would run the REAL hooks against the REAL store, which is the developer's tree.
        sandbox.git(&["config", "core.hooksPath", ".git/hooks"]);
        sandbox.git(&["config", "commit.gpgsign", "false"]);
        sandbox
    }

    /// The hooks as they are on disk, MODE AND ALL — git invokes only an executable file, and
    /// `hooks_cannot_pass_in_silence` is the gate on that bit being recorded.
    ///
    /// ⚠⚠⚠⚠ **LINKED RATHER THAN COPIED** — register item 467. A copy is a file this process wrote,
    /// and the hooks are then EXECUTED, so every sandbox carried the `ETXTBSY` window that item 465
    /// measured at 10 failures in 30 runs on the neighbouring suite. A link opens nothing for
    /// writing, and `metadata` follows it, so the mode this claims to carry across is still the
    /// real hook's own — read from the same inode git will refuse to run without the bit.
    ///
    /// ⚠ Every entry travels, not only the executable ones: both hooks `source` their siblings
    /// (`.githooks/doc-gate.sh`, `.githooks/content-gate.sh`) through the sandbox's own
    /// `git rev-parse --show-toplevel`, so a hook directory missing them is a hook that cannot run.
    fn link_hooks(&self) {
        let from = repo_root().join(".githooks");
        let to = self.dir.join(".githooks");
        std::fs::create_dir_all(&to).expect("create the sandbox's hook directory");
        for entry in std::fs::read_dir(&from)
            .unwrap_or_else(|why| panic!("{} is this repo's hooks: {why}", from.display()))
        {
            let path = entry.expect("read a hook directory entry").path();
            if !path.is_file() {
                continue;
            }
            let name = path.file_name().expect("a hook has a name");
            sprag_gate::doubles::linked_as(&path, &to.join(name));
        }
    }

    fn write(&self, rel: &str, text: &str) {
        let path = self.dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create a directory in the sandbox");
        }
        std::fs::write(&path, text).unwrap_or_else(|why| panic!("write {rel}: {why}"));
    }

    /// Put the tracked double called `name` on this sandbox's `PATH`.
    ///
    /// ⚠ A LINK, and the double itself is never written — see [`doubles`]. It links into `bin/`
    /// rather than putting the tracked directory on `PATH` because [`Sandbox::without`] has to be
    /// able to take a single tool away, and because the `git` double finds the real git by skipping
    /// *its own* directory, which must be this sandbox's and not one shared with another suite.
    fn double(&self, name: &str) {
        doubles().link(name, &self.bin.join(name));
    }

    /// Take a tool off the table entirely — this sandbox's double AND any real one the developer
    /// happens to have installed.
    ///
    /// ⚠ Both halves are needed. Deleting only the double would leave the real binary further down
    /// PATH and the case would be measuring the developer's machine instead of the absence it
    /// claims to stage.
    fn without(&mut self, tool: &str) {
        let _ = std::fs::remove_file(self.bin.join(tool));
        self.hidden.push(tool.to_owned());
    }

    /// Make `git diff --cached` fail for the HOOK the way an unreadable index would, leaving every
    /// other git call intact. Installed on demand, after staging is done with the real git.
    ///
    /// ⚠ The double finds the real git for itself, by walking `PATH` and skipping the directory it
    /// was linked into. This used to be a path substituted into the double's body as it was
    /// written — which is exactly the write item 467 is about, and a tracked file cannot carry a
    /// path chosen at run time anyway.
    fn break_index_reads(&self) {
        self.double("git");
    }

    /// A PATH with the doubles in front of whatever the developer has, minus any hidden tool.
    fn path(&self) -> std::ffi::OsString {
        let inherited = std::env::var_os("PATH").unwrap_or_default();
        let mut dirs = vec![self.bin.clone()];
        for dir in std::env::split_paths(&inherited) {
            if self.hidden.iter().any(|tool| dir.join(tool).is_file()) {
                continue;
            }
            dirs.push(dir);
        }
        std::env::join_paths(dirs).expect("a PATH with the doubles in front")
    }

    fn git(&self, args: &[&str]) -> String {
        let run = Command::new("git")
            .args(args)
            .current_dir(&self.dir)
            // The developer's own git configuration must not reach in: it is not part of the
            // subject, and `core.hooksPath` in particular would change what these runs mean.
            .env("HOME", &self.dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("git on PATH — the sandbox is a git repository");
        assert!(
            run.status.success(),
            "git {args:?} refused in the sandbox: {}",
            String::from_utf8_lossy(&run.stderr),
        );
        String::from_utf8_lossy(&run.stdout).trim().to_owned()
    }

    fn commit(&self, message: &str) -> String {
        self.git(&["commit", "-q", "-m", message]);
        self.git(&["rev-parse", "HEAD"])
    }

    /// Run a hook the way git runs it: from the work tree, with the refs (if any) on stdin.
    fn run(&self, hook: &str, refs_on_stdin: Option<&str>, report: Option<&str>) -> Output {
        let mut command = Command::new(self.dir.join(".githooks").join(hook));
        command
            .current_dir(&self.dir)
            .env("HOME", &self.dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("DOUBLE_LOG", &self.log)
            .env("PATH", self.path())
            .env_remove("REFS_REPORT")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(report) = report {
            command.env("REFS_REPORT", report);
        }
        if hook == "pre-push" {
            command.args(["origin", "git@example.invalid:sprag.git"]);
        }
        command.stdin(match refs_on_stdin {
            Some(_) => Stdio::piped(),
            None => Stdio::null(),
        });

        let mut child = command
            .spawn()
            .unwrap_or_else(|why| panic!(".githooks/{hook} must be executable: {why}"));
        if let Some(refs) = refs_on_stdin {
            // ⚠⚠⚠⚠ Through [`sprag_gate::feeding`], because a hook may REFUSE before it reads a
            // byte — the guard cases below are exactly that — and this used to treat the resulting
            // `EPIPE` as fatal. Register item 471; git tolerates the same thing and judges the
            // hook by its status.
            sprag_gate::feeding::feed(&mut child, refs.as_bytes());
        }
        child.wait_with_output().expect("wait for the hook")
    }

    /// Every command the doubles were asked to run, in order.
    fn invocations(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    fn done(self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn said(run: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    )
}

/// One ref line in the form git writes: `<local ref> <local sha> <remote ref> <remote sha>`.
fn ref_line(local_sha: &str, remote_sha: &str) -> String {
    format!("refs/heads/main {local_sha} refs/heads/main {remote_sha}\n")
}

/// The all-zero sha git sends for a ref that does not exist on the remote yet.
const ABSENT: &str = "0000000000000000000000000000000000000000";

/// Did the hook reach the pixel smoke?
///
/// ⚠ Three markers rather than one, because `run_pixel_smoke` refuses BEFORE it announces itself on
/// a machine without xvfb or the lavapipe ICD — and the ICD is looked for on the real filesystem,
/// which this test has no business faking. Reached is reached; which branch it took afterwards
/// depends on the box and is not the decision under test.
fn reached_the_pixel_smoke(told: &str) -> bool {
    told.contains("this push paints")
        || told.contains("needs xvfb-run")
        || told.contains("needs the lavapipe")
}

/// Whether the push gate reached the suite that DRIVES these hooks — register item 688.
fn reached_the_hook_suite(told: &str) -> bool {
    told.contains("this push changes a hook")
}

// ─── pre-commit ────────────────────────────────────────────────────────────────────────────────

/// ⚠⚠⚠ **THE CONTROL, AND IT ASSERTS ITS OWN STAGING.** Every case below asserts a REFUSAL or an
/// ACCEPTANCE that turns on one gate; a hook whose doubles had quietly neutered everything would
/// satisfy several of them. So this one says the ordinary commit passes AND names each gate that
/// ran, which is the half that proves the doubles are load-bearing rather than inert.
#[test]
fn an_ordinary_commit_passes_and_every_gate_the_hook_names_actually_ran() {
    let sandbox = Sandbox::new("commit-control");
    sandbox.write("crates/sprag-gui/paint.rs", FORMATTED);
    sandbox.git(&["add", "crates/sprag-gui/paint.rs"]);

    let run = sandbox.run("pre-commit", None, None);
    let told = said(&run);
    assert!(
        run.status.success(),
        "a commit of formatted Rust must pass, or the refusals below prove nothing: {told}",
    );

    let invoked = sandbox.invocations();
    for expected in [
        "mnemosyne-cli validate-workspace",
        "mnemosyne-cli validate-code-refs",
        "cargo clippy --workspace --all-targets -- -D warnings",
        "cargo doc --workspace --no-deps --document-private-items",
    ] {
        assert!(
            invoked.contains(expected),
            "the hook's header promises `{expected}` and the run did not make it:\n{invoked}",
        );
    }
    assert!(
        told.contains("rustfmt"),
        "and the format gate must have run on a staged *.rs file: {told}",
    );
    sandbox.done();
}

/// ⚠⚠⚠⚠ **THE DEFECT.** `git diff --cached --name-only` answers with NAMES; the bytes behind those
/// names in the working tree are a different thing, and handing them to rustfmt judges a file
/// nobody is committing.
///
/// The way in is ordinary, not contrived: stage a change, then keep working — an editor's
/// format-on-save, a `cargo fmt`, one more edit. This project stages by PATH and reads
/// `git diff --cached` precisely because the two diverge (register item 196), so the divergence is
/// the normal state here rather than an exotic one.
#[test]
fn the_commit_gate_judges_the_staged_bytes_and_not_the_file_on_disk() {
    let sandbox = Sandbox::new("commit-staged-bytes");
    sandbox.write("crates/sprag-gui/paint.rs", UNFORMATTED);
    sandbox.git(&["add", "crates/sprag-gui/paint.rs"]);
    // …and then the file on disk is tidied, which changes nothing about what is staged.
    sandbox.write("crates/sprag-gui/paint.rs", FORMATTED);

    let run = sandbox.run("pre-commit", None, None);
    let told = said(&run);
    assert!(
        !run.status.success(),
        "the INDEX holds unformatted Rust and that is what the commit will carry — a gate that \
         reads the tidy copy on disk instead has judged a file nobody is committing: {told}",
    );
    assert!(
        told.contains("paint.rs"),
        "and the refusal must name the file, which is the only part a person can act on: {told}",
    );
    sandbox.done();
}

/// ⚠⚠⚠⚠ **THE SAME DEFECT FROM THE OTHER SIDE, AND IT IS NOT A FORMALITY.** A gate that simply
/// refused more often would satisfy the case above while making the repository harder to commit to
/// for reasons that have nothing to do with the commit. The hook's own header promises this half in
/// as many words — *"no untouched file can fail a commit that did not touch it"* — and the working
/// tree is exactly such an untouched thing.
#[test]
fn an_unstaged_edit_cannot_fail_a_commit_that_does_not_carry_it() {
    let sandbox = Sandbox::new("commit-unstaged-edit");
    sandbox.write("crates/sprag-gui/paint.rs", FORMATTED);
    sandbox.git(&["add", "crates/sprag-gui/paint.rs"]);
    // Work continues in the editor. None of it is staged.
    sandbox.write("crates/sprag-gui/paint.rs", UNFORMATTED);

    let run = sandbox.run("pre-commit", None, None);
    assert!(
        run.status.success(),
        "the staged content is formatted and it is the only thing being committed; a half-written \
         edit on disk is not the commit's business: {}",
        said(&run),
    );
    sandbox.done();
}

/// ⚠⚠⚠⚠ **THE CASE THAT CAUGHT THE FIX'S OWN FIRST DRAFT**, and it was found by running the gate
/// against real tracked sources rather than by any fixture here — which is why it is now a fixture.
///
/// rustfmt FOLLOWS `mod` declarations. A mirror holding only the paths under judgement answers
/// `failed to resolve mod ...` and exits nonzero, so a gate built that way refuses almost every
/// commit this repository makes while having judged nothing. No format fixture catches it, because
/// a fixture written for the format rule has no `mod` in it: the defect lives in the SHAPE of the
/// mirror, not in the rule.
#[test]
fn a_module_root_is_not_refused_for_children_the_commit_does_not_carry() {
    let sandbox = Sandbox::new("commit-module-root");
    sandbox.write("crates/sprag-vt/src/child.rs", FORMATTED);
    sandbox.write("crates/sprag-vt/src/lib.rs", "mod child;\n");
    sandbox.git(&[
        "add",
        "crates/sprag-vt/src/child.rs",
        "crates/sprag-vt/src/lib.rs",
    ]);
    sandbox.commit("a module root and the child it declares");

    // A later commit touches only the root. The child is in the tree, not in this change.
    sandbox.write(
        "crates/sprag-vt/src/lib.rs",
        "mod child;\n\npub fn root() {}\n",
    );
    sandbox.git(&["add", "crates/sprag-vt/src/lib.rs"]);

    let run = sandbox.run("pre-commit", None, None);
    assert!(
        run.status.success(),
        "the staged root is formatted and its child is right there in the tree — a gate that \
         cannot see the child has judged nothing and refused anyway: {}",
        said(&run),
    );
    sandbox.done();
}

/// ⚠⚠ **THE CONTROL FOR THE WORKFLOW GATE**, and the first thing that ever ran it: nothing in this
/// repository had executed the actionlint branch, so *the checker is reached at all* was itself
/// unmeasured.
#[test]
fn a_commit_of_a_valid_workflow_passes_and_the_checker_was_given_it() {
    let sandbox = Sandbox::new("workflow-control");
    sandbox.write(".github/workflows/ci.yml", VALID_WORKFLOW);
    sandbox.git(&["add", ".github/workflows/ci.yml"]);

    let run = sandbox.run("pre-commit", None, None);
    assert!(
        run.status.success(),
        "a valid staged workflow must pass: {}",
        said(&run),
    );
    assert!(
        sandbox.invocations().contains("actionlint-read"),
        "and the checker must actually have been handed the workflow, or the case below is \
         asserting about a branch that never runs:\n{}",
        sandbox.invocations(),
    );
    sandbox.done();
}

/// ⚠⚠⚠⚠ **THE RUSTFMT DEFECT, ONE GATE OVER — found by sweeping the hook after fixing the first.**
/// This gate also took the staged NAMES and handed its checker the WORKING-TREE bytes, so a broken
/// workflow went out whenever the file was tidied after staging.
///
/// ⚠⚠⚠ It matters more here than anywhere else in the hook: a workflow is the ONE thing CI cannot
/// catch afterwards. An invalid expression does not fail a step — the run never starts, so there is
/// no job and no log to read.
#[test]
fn the_workflow_gate_judges_the_staged_bytes_and_not_the_file_on_disk() {
    let sandbox = Sandbox::new("workflow-staged-bytes");
    sandbox.write(".github/workflows/ci.yml", INVALID_WORKFLOW);
    sandbox.git(&["add", ".github/workflows/ci.yml"]);
    // …and then the file on disk is repaired, which changes nothing about what is staged.
    sandbox.write(".github/workflows/ci.yml", VALID_WORKFLOW);

    let run = sandbox.run("pre-commit", None, None);
    let told = said(&run);
    assert!(
        !run.status.success(),
        "the INDEX holds a workflow that will never start a run, and CI cannot catch it \
         afterwards because there is no run to look at: {told}",
    );
    let invoked = sandbox.invocations();
    assert!(
        invoked.contains("INVALID-WORKFLOW"),
        "and the checker must have been given the STAGED bytes — reading the repaired copy on \
         disk is judging a file nobody is committing:\n{invoked}",
    );
    sandbox.done();
}

/// ⚠⚠⚠ **THE ABSENT TOOL IS ANNOUNCED IN A WORD, NOT STEPPED OVER IN SILENCE** — the hook promises
/// exactly this and nothing measured it.
///
/// ⚠⚠ And the stance here is deliberately WEAKER than the rustfmt gate's, which refuses outright
/// (item 403). actionlint is not part of any toolchain this project pins, so demanding it would
/// block every commit on a fresh clone. The difference between the two answers is a judgement, so
/// it is worth a case that pins which one this gate gives.
///
/// ⚠⚠⚠⚠ **THE FIXTURE ASSERTS ITS OWN STAGING**, and here that comes free: the staged workflow is
/// the INVALID one. If actionlint were still reachable the hook would refuse, so a passing run is
/// itself the proof that the tool is genuinely gone rather than merely renamed out of the way.
#[test]
fn a_missing_actionlint_is_announced_in_a_word_rather_than_skipped_in_silence() {
    let mut sandbox = Sandbox::new("workflow-no-tool");
    sandbox.without("actionlint");
    sandbox.write(".github/workflows/ci.yml", INVALID_WORKFLOW);
    sandbox.git(&["add", ".github/workflows/ci.yml"]);

    let run = sandbox.run("pre-commit", None, None);
    let told = said(&run);
    assert!(
        run.status.success(),
        "actionlint is not a toolchain component this project pins, so its absence must not \
         block every commit on a fresh clone: {told}",
    );
    assert!(
        told.contains("NOT INSTALLED") && told.contains("actionlint"),
        "but a gate that vanishes quietly is one people stop expecting, so its absence must be \
         said out loud: {told}",
    );
    assert!(
        !sandbox.invocations().contains("actionlint-read"),
        "and nothing may have read the workflow — if the double still ran, this case is \
         measuring the wrong absence:\n{}",
        sandbox.invocations(),
    );
    sandbox.done();
}

/// ⚠⚠ **A COMMIT CARRYING NO RUST DOES NOT PAY FOR CLIPPY** — the hook promises exactly this
/// ("docs/atomic-only commits stay cheap") and nothing measured it. It is also the control for the
/// case below: without it, a hook that ran clippy unconditionally would satisfy that one.
#[test]
fn a_commit_carrying_no_rust_does_not_pay_for_the_expensive_gates() {
    let sandbox = Sandbox::new("commit-no-rust");
    sandbox.write("notes.md", "prose only\n");
    sandbox.git(&["add", "notes.md"]);

    let run = sandbox.run("pre-commit", None, None);
    assert!(
        run.status.success(),
        "a prose commit must pass: {}",
        said(&run)
    );
    let invoked = sandbox.invocations();
    assert!(
        !invoked.contains("cargo"),
        "no *.rs is staged, so clippy and the rustdoc gate are minutes spent on nothing:\n{invoked}",
    );
    sandbox.done();
}

/// ⚠⚠⚠⚠ **THE BIG COMMIT IS THE ONE THAT MUST NOT SKIP THE LINT, AND THE PIPELINE DECIDING IT CAN
/// FAIL FROM SUCCESS.**
///
/// `pre-commit` chooses whether to run clippy with
/// `git diff --cached --name-only … | grep -qE '\.rs$'` under `set -o pipefail`. `grep -q` exits
/// the instant it matches — on the FIRST line — so on a staged list long enough to fill the pipe
/// buffer, `git` is still writing when the reader goes away, takes EPIPE, and dies. `pipefail` then
/// makes the whole pipeline nonzero, the `if` reads FALSE, and **clippy and the rustdoc gate are
/// skipped in silence** — on exactly the commits that are largest and most worth linting.
///
/// ⚠⚠⚠ A small list does not reproduce it: git finishes writing before grep exits, which is why
/// this stages enough names to exceed a 64K pipe buffer rather than a handful.
#[test]
fn a_commit_too_large_for_a_pipe_buffer_still_pays_for_the_lint() {
    let sandbox = Sandbox::new("commit-large");
    // ~3000 paths of ~30 bytes: comfortably past the 64K a pipe holds.
    for index in 0..3000 {
        sandbox.write(&format!("crates/sprag-vt/src/g{index:05}.rs"), FORMATTED);
    }
    sandbox.git(&["add", "crates/sprag-vt"]);

    let run = sandbox.run("pre-commit", None, None);
    let told = said(&run);
    assert!(run.status.success(), "the staged Rust is formatted: {told}");
    let invoked = sandbox.invocations();
    assert!(
        invoked.contains("cargo clippy"),
        "three thousand staged *.rs files and the lint did not run — the gate answered \
         \"no Rust here\" because its own query died of a broken pipe:\n{invoked}",
    );
    sandbox.done();
}

/// ⚠⚠⚠⚠ **`|| true` ABSORBS THE TOOL FAILING, NOT JUST THE PATTERN MISSING** — the round's central
/// defect, fourth instance, and this one is in shipped code.
///
/// `staged_rs="$(git diff --cached … | grep -E '\.rs$' || true)"` needs the `|| true` because grep
/// answers 1 when nothing matches. But it swallows `git` failing just as happily, and an empty
/// `staged_rs` is indistinguishable from *this commit carries no Rust* — so an index the hook
/// CANNOT READ passes as a commit with nothing to check. Every gate downstream is keyed off that
/// same variable, so one unreadable index waives rustfmt, clippy and the rustdoc gate at once.
///
/// ⚠⚠⚠ The fixture stages FORMATTED Rust on purpose. If the hook can read the index there is
/// nothing to complain about, so a refusal here can only mean it noticed it could not read — and a
/// pass can only mean it did not.
#[test]
fn a_commit_gate_that_cannot_read_the_index_refuses_rather_than_finding_nothing() {
    let sandbox = Sandbox::new("commit-blind-index");
    sandbox.write("crates/sprag-gui/paint.rs", FORMATTED);
    // Staged with the REAL git, before the double goes in.
    sandbox.git(&["add", "crates/sprag-gui/paint.rs"]);
    sandbox.break_index_reads();

    let run = sandbox.run("pre-commit", None, None);
    let told = said(&run);
    assert!(
        !run.status.success(),
        "the hook could not read the index and answered \"nothing staged\" — that is a gate \
         reporting no work because its QUESTION failed: {told}",
    );
    sandbox.done();
}

// ─── pre-push ──────────────────────────────────────────────────────────────────────────────────

/// ⚠⚠⚠⚠ **THE HOLE THE PUSH GATE EXISTS TO CLOSE, AND DID NOT.** `pre-push`'s header says it
/// re-runs the integrity gates to catch what pre-commit missed and names `--no-verify` among the
/// ways that happens — and then its list has no format gate at all. So the one check a bypass most
/// reliably steps over, being the check `pre-commit` alone owns, was the one the second chance did
/// not offer.
#[test]
fn a_push_carrying_unformatted_rust_is_refused() {
    let sandbox = Sandbox::new("push-unformatted");
    sandbox.write("crates/sprag-host/base.rs", FORMATTED);
    sandbox.git(&["add", "crates/sprag-host/base.rs"]);
    let base = sandbox.commit("base");

    // A commit that never met the commit gate — `git commit --no-verify`, an amend, a rebase.
    sandbox.write("crates/sprag-host/slipped.rs", UNFORMATTED);
    sandbox.git(&["add", "crates/sprag-host/slipped.rs"]);
    let head = sandbox.commit("slipped past the commit gate");

    let run = sandbox.run("pre-push", Some(&ref_line(&head, &base)), None);
    let told = said(&run);
    assert!(
        !run.status.success(),
        "this push publishes unformatted Rust and the push gate is the last place to see it: {told}",
    );
    assert!(
        told.contains("slipped.rs"),
        "and the refusal must name the file the push carries: {told}",
    );
    sandbox.done();
}

/// ⚠⚠⚠ **THE UNKNOWN RANGE IS JUDGED, NOT WAIVED** — the stance `pixel_smoke_is_owed` already takes
/// in as many words, applied to the gate beside it. A brand-new branch has no remote commit to diff
/// against, and answering *nothing changed* there would make the first push of any branch the one
/// push that is never checked.
#[test]
fn a_push_of_a_branch_the_remote_has_never_seen_is_judged_rather_than_waived() {
    let sandbox = Sandbox::new("push-new-branch");
    sandbox.write("crates/sprag-host/slipped.rs", UNFORMATTED);
    sandbox.git(&["add", "crates/sprag-host/slipped.rs"]);
    let head = sandbox.commit("the first commit of a new branch");

    let run = sandbox.run("pre-push", Some(&ref_line(&head, ABSENT)), None);
    let told = said(&run);
    assert!(
        !run.status.success(),
        "there is no remote commit to compare against, so the whole tree is what this push \
         publishes — waiving it would exempt every branch's first push: {told}",
    );
    assert!(
        told.contains("slipped.rs"),
        "and the refusal must name the file: {told}",
    );
    sandbox.done();
}

/// ⚠⚠ **THE CONTROL FOR THE TWO ABOVE**, and the first thing that ever ran `pixel_smoke_is_owed`:
/// a push whose range touches neither `crates/sprag-gui` nor `crates/sprag-grid` must pass without
/// reaching for an X server.
#[test]
fn a_push_whose_range_does_not_paint_passes_without_owing_the_pixel_smoke() {
    let sandbox = Sandbox::new("push-no-paint");
    sandbox.write("crates/sprag-host/base.rs", FORMATTED);
    sandbox.git(&["add", "crates/sprag-host/base.rs"]);
    let base = sandbox.commit("base");

    sandbox.write("crates/sprag-host/more.rs", FORMATTED);
    sandbox.git(&["add", "crates/sprag-host/more.rs"]);
    let head = sandbox.commit("nothing that paints");

    let run = sandbox.run("pre-push", Some(&ref_line(&head, &base)), None);
    let told = said(&run);
    assert!(
        run.status.success(),
        "a push of formatted, non-painting Rust must pass: {told}",
    );
    assert!(
        !reached_the_pixel_smoke(&told),
        "and it must not reach for the pixel smoke, which is minutes and an X server: {told}",
    );
    sandbox.done();
}

/// ⚠⚠⚠ **AND THE DECISION GOES THE OTHER WAY WHEN THE RANGE PAINTS.** Without this the case above
/// would be satisfied by a hook that never ran the smoke at all — which is the state R349 shipped
/// and eleven rounds did not notice.
#[test]
fn a_push_whose_range_paints_owes_the_pixel_smoke() {
    let sandbox = Sandbox::new("push-paint");
    sandbox.write("crates/sprag-host/base.rs", FORMATTED);
    sandbox.git(&["add", "crates/sprag-host/base.rs"]);
    let base = sandbox.commit("base");

    sandbox.write("crates/sprag-gui/paint.rs", FORMATTED);
    sandbox.git(&["add", "crates/sprag-gui/paint.rs"]);
    let head = sandbox.commit("a change under crates/sprag-gui");

    let run = sandbox.run("pre-push", Some(&ref_line(&head, &base)), None);
    let told = said(&run);
    assert!(
        reached_the_pixel_smoke(&told),
        "this push changes what the GUI paints and the smoke is the only gate that looks at \
         pixels: {told}",
    );
    sandbox.done();
}

/// ⛔⛔⛔⛔⛔ **A PUSH THAT CHANGES A HOOK OWES THE SUITE THAT DRIVES HOOKS** — register item 688,
/// and the gate that was missing when that item was written.
///
/// # What it cost to not have this
///
/// `c893e39` put `.githooks/ident-gate.sh` on `pre-commit` and `pre-push`. This crate links the
/// real hooks into a throwaway repository and drives them, so the new gate refused **every commit
/// this suite makes**: `6 passed; 17 failed`, on both CI platforms. **Twenty-one consecutive
/// pushes carried that red** (2026-08-24 22:24 → 2026-08-26) because `pre-push` ran
/// `validate-workspace`, clippy, rustdoc and the pixel smoke — and nothing that reads a hook.
///
/// ⚠⚠⚠ **THE GATE ITSELF WAS NOT THE PROBLEM AND ITS OWN SELFTEST SAID SO.** `ident-gate.sh` has
/// thirteen `--selftest` arms and its commit message recorded *"a mutation reds five of them"*. It
/// measured itself correctly and could not measure what it did ONE CRATE OVER. **A gate that
/// passes its own selftest is evidence it is right, not evidence the tree is green.**
#[test]
fn a_push_that_changes_a_hook_owes_the_suite_that_drives_hooks() {
    let sandbox = Sandbox::new("push-hook");
    sandbox.write("crates/sprag-host/base.rs", FORMATTED);
    sandbox.git(&["add", "crates/sprag-host/base.rs"]);
    let base = sandbox.commit("base");

    sandbox.write(".githooks/some-gate.sh", "#!/bin/sh\nexit 0\n");
    sandbox.git(&["add", ".githooks/some-gate.sh"]);
    let head = sandbox.commit("a change under .githooks");

    let run = sandbox.run("pre-push", Some(&ref_line(&head, &base)), None);
    let told = said(&run);
    assert!(
        reached_the_hook_suite(&told),
        "⛔⛔⛔ THIS PUSH EDITS A HOOK, and the only crate that can tell whether a hook still works \
         is the one that drives it. Publishing without running it is what put a red on twenty-one \
         pushes: {told}",
    );
    sandbox.done();
}

/// ⚠⚠⚠ **THE CONTROL, AND WITHOUT IT THE ARM ABOVE IS SATISFIED BY A GATE THAT ALWAYS RUNS.** A
/// push that touches no hook must NOT owe the suite — otherwise every push in this repository pays
/// for it, which is the cost that makes a gate get waived, and a waived gate is the state item 688
/// is about wearing different clothes.
#[test]
fn a_push_that_changes_no_hook_passes_without_owing_the_hook_suite() {
    let sandbox = Sandbox::new("push-no-hook");
    sandbox.write("crates/sprag-host/base.rs", FORMATTED);
    sandbox.git(&["add", "crates/sprag-host/base.rs"]);
    let base = sandbox.commit("base");

    sandbox.write("crates/sprag-host/more.rs", FORMATTED);
    sandbox.git(&["add", "crates/sprag-host/more.rs"]);
    let head = sandbox.commit("a change that is not a hook");

    let run = sandbox.run("pre-push", Some(&ref_line(&head, &base)), None);
    let told = said(&run);
    assert!(
        !reached_the_hook_suite(&told),
        "⚠⚠⚠ THE CONTROL: nothing under `.githooks` changed here, so the hook suite is not owed. A \
         gate that ran on every push would make the arm above pass while measuring nothing: {told}",
    );
    sandbox.done();
}

/// ⚠⚠⚠ **NO REFS AT ALL STILL OWES IT.** A hook run by hand, a git that fed nothing, a stream some
/// tool upstream had already drained: the failure this guards is a paint change going unlooked-at,
/// so not knowing has to mean *run it*.
#[test]
fn a_push_with_no_refs_on_stdin_owes_the_pixel_smoke() {
    let sandbox = Sandbox::new("push-no-refs");
    sandbox.write("crates/sprag-host/base.rs", FORMATTED);
    sandbox.git(&["add", "crates/sprag-host/base.rs"]);
    sandbox.commit("base");

    let run = sandbox.run("pre-push", None, None);
    let told = said(&run);
    assert!(
        reached_the_pixel_smoke(&told),
        "an unresolvable push must run the gate rather than waive it: {told}",
    );
    sandbox.done();
}

/// ⚠⚠⚠ **AND THE PUSH SIDE LAYS THE TREE OUT BY A DIFFERENT ROUTE**, so it needs its own case: the
/// commit path uses the ambient index, this one reads the pushed commit into a scratch index. A
/// mirror missing the child breaks here for the same reason and would not be caught by the case
/// above.
#[test]
fn a_pushed_module_root_is_not_refused_for_children_the_range_does_not_carry() {
    let sandbox = Sandbox::new("push-module-root");
    sandbox.write("crates/sprag-vt/src/child.rs", FORMATTED);
    sandbox.write("crates/sprag-vt/src/lib.rs", "mod child;\n");
    sandbox.git(&[
        "add",
        "crates/sprag-vt/src/child.rs",
        "crates/sprag-vt/src/lib.rs",
    ]);
    let base = sandbox.commit("a module root and the child it declares");

    sandbox.write(
        "crates/sprag-vt/src/lib.rs",
        "mod child;\n\npub fn root() {}\n",
    );
    sandbox.git(&["add", "crates/sprag-vt/src/lib.rs"]);
    let head = sandbox.commit("touch only the root");

    let run = sandbox.run("pre-push", Some(&ref_line(&head, &base)), None);
    assert!(
        run.status.success(),
        "the range carries a formatted root whose child is in the pushed commit — refusing that \
         is a gate that judged nothing: {}",
        said(&run),
    );
    sandbox.done();
}

/// ⚠⚠⚠ **DELETING A REF CARRIES NO TREE, AND BOTH WALKERS HAVE TO KNOW THAT.** Git sends an
/// all-zero LOCAL sha for a deletion. There is no commit to lay out and nothing to judge, so both
/// the smoke decision and the format gate must step over it — and a gate that instead tried to diff
/// against `0000…` fails hard under `set -e`, turning *delete a stale branch* into a push that
/// cannot happen.
#[test]
fn deleting_a_ref_carries_no_content_and_is_not_judged() {
    let sandbox = Sandbox::new("push-deletion");
    sandbox.write("crates/sprag-gui/paint.rs", FORMATTED);
    sandbox.git(&["add", "crates/sprag-gui/paint.rs"]);
    let remote = sandbox.commit("what the remote already has");

    // A deletion: the local side is all zeros, the remote side is a real commit.
    let run = sandbox.run("pre-push", Some(&ref_line(ABSENT, &remote)), None);
    let told = said(&run);
    assert!(
        run.status.success(),
        "a deletion publishes no content — refusing it means a stale branch can never be \
         removed: {told}",
    );
    assert!(
        !reached_the_pixel_smoke(&told),
        "and there is no tree to paint from, so the smoke is not owed either: {told}",
    );
    sandbox.done();
}

/// A well-formed sha that names no object in any clone.
const UNRESOLVABLE: &str = "1234567890abcdef1234567890abcdef12345678";

/// ⚠⚠⚠⚠ **A QUERY THAT ERRORED MUST NOT READ AS "NOTHING TO JUDGE"** — and it did, because of a
/// bash rule that is easy to be wrong about.
///
/// `set -euo pipefail` is at the top of the hook, so every `git` in it looks guarded. It is not: a
/// function invoked as the condition of an `if` runs with **errexit suppressed throughout its whole
/// body**. Measured: `f() { false; echo REACHED; }; if ! f; then …` prints REACHED. So a failing
/// `git diff` inside either ref walker leaves its variable EMPTY, the `[ -n … ]` beside it reads
/// false, and the range is skipped — the gate reporting *nothing here* because its question
/// errored.
///
/// ⚠⚠⚠ **THIS CASE EXISTS BECAUSE A MUTATION FAILED TO GO RED.** Deleting the deletion-guard from
/// the format walker left every case green, which should have reded one — and chasing that is what
/// turned up the suppressed `set -e` underneath. The mutation that catches nothing is the finding.
#[test]
fn a_push_whose_local_sha_cannot_be_resolved_is_refused_not_waved_through() {
    let sandbox = Sandbox::new("push-unresolvable");
    sandbox.write("crates/sprag-host/base.rs", FORMATTED);
    sandbox.git(&["add", "crates/sprag-host/base.rs"]);
    let base = sandbox.commit("base");

    let run = sandbox.run("pre-push", Some(&ref_line(UNRESOLVABLE, &base)), None);
    let told = said(&run);
    assert!(
        !run.status.success(),
        "the hook cannot read what this push claims to carry, and answering \"then there is \
         nothing to check\" is how a gate passes without judging: {told}",
    );
    assert!(
        told.contains(UNRESOLVABLE),
        "and it must name the sha it could not resolve: {told}",
    );
    sandbox.done();
}

/// ⚠⚠⚠⚠ **THE PUSH GATE OWES THE WORKFLOW CHECK MORE THAN IT OWES THE FORMAT ONE.**
///
/// `pre-push` exists to catch what `pre-commit` missed — an amend, a rebase, a `--no-verify`. That
/// argument was used to give it a format gate; it applies harder here, because **a workflow is the
/// one thing CI cannot catch afterwards.** Every other check has a second chance on the runner. An
/// invalid workflow expression does not fail a step — the run never STARTS, so there is no job and
/// no log, and the push that carried it looks exactly like a push that was fine.
///
/// ⚠⚠ This case exists because the round that added the format gate here left this one out and
/// wrote down that it had. Reading that note back, the reason given did not survive: the header's
/// promise covers *anything* pre-commit may have missed, workflows included.
#[test]
fn a_push_carrying_an_invalid_workflow_is_refused() {
    let sandbox = Sandbox::new("push-bad-workflow");
    sandbox.write("crates/sprag-host/base.rs", FORMATTED);
    sandbox.git(&["add", "crates/sprag-host/base.rs"]);
    let base = sandbox.commit("base");

    // A commit that never met the commit gate.
    sandbox.write(".github/workflows/ci.yml", INVALID_WORKFLOW);
    sandbox.git(&["add", ".github/workflows/ci.yml"]);
    let head = sandbox.commit("a workflow that will never start a run");

    let run = sandbox.run("pre-push", Some(&ref_line(&head, &base)), None);
    let told = said(&run);
    assert!(
        !run.status.success(),
        "this push publishes a workflow whose run will never start, and the runner cannot report \
         a run that does not begin — the push gate is the last place anyone can see it: {told}",
    );
    assert!(
        sandbox.invocations().contains("INVALID-WORKFLOW"),
        "and the checker must have been handed the bytes the COMMIT carries:\n{}",
        sandbox.invocations(),
    );
    sandbox.done();
}

/// ⚠⚠ **THE CONTROL**: a valid workflow in the range passes, and the checker was actually given it.
/// Without this, the case above is satisfied by a push gate that refuses every workflow.
#[test]
fn a_push_carrying_a_valid_workflow_passes_and_the_checker_was_given_it() {
    let sandbox = Sandbox::new("push-good-workflow");
    sandbox.write("crates/sprag-host/base.rs", FORMATTED);
    sandbox.git(&["add", "crates/sprag-host/base.rs"]);
    let base = sandbox.commit("base");

    sandbox.write(".github/workflows/ci.yml", VALID_WORKFLOW);
    sandbox.git(&["add", ".github/workflows/ci.yml"]);
    let head = sandbox.commit("a workflow that parses and starts");

    let run = sandbox.run("pre-push", Some(&ref_line(&head, &base)), None);
    let told = said(&run);
    assert!(run.status.success(), "a valid workflow must pass: {told}");
    assert!(
        sandbox.invocations().contains("actionlint-read"),
        "and the checker must have been reached, or the case above proves nothing:\n{}",
        sandbox.invocations(),
    );
    sandbox.done();
}

// ─── both hooks ────────────────────────────────────────────────────────────────────────────────

/// ⚠⚠⚠⚠ **ITEM 213's FIX IS PINNED TO A WORD, AND NOTHING WAS HOLDING THE WORD.** Both hooks show
/// the citation report as `printf … | grep -E '^violations:' || true`. Rename that summary line in
/// `mnemosyne-cli` — a tool this repository pins no version of — and the filter matches nothing,
/// `|| true` swallows the miss, and the fifty-two findings are back in `/dev/null` with no one the
/// wiser. A gate that can regress in silence is the shape 213 and 403 both were.
///
/// ⚠⚠ Driven for BOTH hooks in one case on purpose: the two copies of this line are exactly what
/// went wrong last time, when `pre-commit` was fixed and `pre-push` was left behind for twelve
/// commits because nothing compared them.
/// ⚠⚠⚠⚠ **THE TOOL BOTH HOOKS ARE BUILT ON, AND NOTHING EVER CHECKED THAT ITS ABSENCE REFUSES.**
/// Each hook opens with a `command -v mnemosyne-cli` guard and exits 1 — the same rule item 403 was
/// about, one file over, and never driven. A guard that exits 0 by accident would let every
/// integrity check in this repository be skipped on a machine that simply lacks the binary, which
/// is exactly the state 403 found `commit-msg` in.
///
/// ⚠⚠ Both hooks in one case, deliberately: the two copies of this guard are the shape that drifts.
#[test]
fn neither_hook_proceeds_when_the_tool_all_its_checks_need_is_absent() {
    for hook in ["pre-commit", "pre-push"] {
        let mut sandbox = Sandbox::new(&format!("no-mnemosyne-{hook}"));
        sandbox.without("mnemosyne-cli");
        sandbox.write("crates/sprag-gui/paint.rs", FORMATTED);
        sandbox.git(&["add", "crates/sprag-gui/paint.rs"]);
        let base = sandbox.commit("base");

        let refs = ref_line(&base, ABSENT);
        let run = sandbox.run(hook, Some(&refs), None);
        let told = said(&run);
        assert!(
            !run.status.success(),
            "{hook} cannot run a single one of its integrity checks without mnemosyne-cli, and a \
             gate that cannot run must refuse rather than wave the change through: {told}",
        );
        assert!(
            told.contains("install mnemosyne-cli"),
            "{hook} must name the tool a person has to install: {told}",
        );
        // ⚠⚠⚠⚠ THE STATUS, NOT MERELY "NONZERO" — and that distinction was MEASURED, not guessed.
        // Deleting the guard's `exit 1` left this case GREEN: the hook ran on, `set -e` killed it at
        // the first `mnemosyne-cli` call, and 127 is as nonzero as 1. So the assertion could not
        // tell a DELIBERATE refusal from a hook falling over the missing binary a line later — the
        // same rule, arrived at by accident, with bash's "command not found" in place of the
        // sentence telling a person what to install. `1` is the hook deciding.
        assert_eq!(
            run.status.code(),
            Some(1),
            "{hook} must REFUSE at its own guard (1), not crash into the absent tool (127): {told}",
        );
        assert!(
            !sandbox.invocations().contains("cargo"),
            "{hook} must stop at the guard rather than carry on to the expensive checks:\n{}",
            sandbox.invocations(),
        );
        sandbox.done();
    }
}

/// ⚠⚠⚠⚠⚠ **A HOOK THAT REFUSES BEFORE READING ITS REFS IS STILL JUDGED BY ITS STATUS** — register
/// item 471, and the case above is the one that kept meeting it.
///
/// `pre-push` reads git's ref list at its top, but AFTER the `command -v mnemosyne-cli` guard, so a
/// machine without that tool refuses with the list undrained. Git tolerates the `EPIPE` that follows
/// and reads the hook's exit status; a harness standing in for git has to do the same, or the case
/// above cannot be expressed at all.
///
/// ⚠⚠⚠⚠ **THE LENGTH IS THE WHOLE GATE.** A short list fits in the pipe's buffer, so the write
/// SUCCEEDS unless the child happens to have gone first — a race that failed once in the first
/// seven runs of a 30-run loop on a loaded build machine and passed every run on its own, which is
/// how it survived as *a flake*. Longer than any pipe buffer, the write must block and then meet
/// the closed pipe, every time and on every machine.
#[test]
fn a_hook_that_refuses_before_reading_a_long_ref_list_is_still_judged_by_its_status() {
    let mut sandbox = Sandbox::new("refs-left-unread");
    sandbox.without("mnemosyne-cli");
    sandbox.write("crates/sprag-gui/paint.rs", FORMATTED);
    sandbox.git(&["add", "crates/sprag-gui/paint.rs"]);
    let base = sandbox.commit("base");

    // ~90 bytes a line: comfortably past Linux's 64 KiB pipe, and a plausible list — a push of
    // every branch of a busy repository is not a strange thing to feed a hook.
    let refs = ref_line(&base, ABSENT).repeat(2048);
    assert!(
        refs.len() > 128 * 1024,
        "the list has to exceed any pipe buffer, or this case is the race it replaces: {} bytes",
        refs.len(),
    );

    let run = sandbox.run("pre-push", Some(&refs), None);
    let told = said(&run);
    assert_eq!(
        run.status.code(),
        Some(1),
        "the hook refused at its guard and the harness must have READ that, rather than dying on \
         the write of a list nobody drained: {told}",
    );
    assert!(
        told.contains("install mnemosyne-cli"),
        "and the refusal is still the one it names: {told}",
    );
    sandbox.done();
}

#[test]
fn neither_hook_swallows_a_report_whose_summary_line_changed_shape() {
    let renamed = "findings: total=7 citation_unbound=7 impl_missing=0";

    let commit = Sandbox::new("report-commit");
    commit.write(
        "notes.md",
        "no rust here, so this run is only about the report\n",
    );
    commit.git(&["add", "notes.md"]);
    let run = commit.run("pre-commit", None, Some(renamed));
    let told = said(&run);
    assert!(
        run.status.success(),
        "the checker agreed, so the commit stands: {told}",
    );
    assert!(
        told.contains("findings: total=7"),
        "but what it SAID must reach the person — a summary line the hook does not recognise is \
         still the report, and dropping it is how fifty-two violations stayed invisible: {told}",
    );
    commit.done();

    let push = Sandbox::new("report-push");
    push.write(
        "notes.md",
        "no rust here, so this run is only about the report\n",
    );
    push.git(&["add", "notes.md"]);
    let base = push.commit("base");
    push.write("notes.md", "a second commit, still no rust\n");
    push.git(&["add", "notes.md"]);
    let head = push.commit("more notes");

    let run = push.run("pre-push", Some(&ref_line(&head, &base)), Some(renamed));
    let told = said(&run);
    assert!(
        run.status.success(),
        "the checker agreed, so the push stands: {told}",
    );
    assert!(
        told.contains("findings: total=7"),
        "and the push gate must not lose it either — it is the copy that was left behind last \
         time: {told}",
    );
    push.done();
}

/// ⛔⛔⛔⛔⛔ **A PUSH SAYS HOW LONG THIS CLONE HAS GONE WITHOUT READING A HOSTED RESULT** —
/// register item 776, arms (1), (2) and (4).
///
/// # ⛔⛔⛔⛔⛔ The rule was in force for all 33 of the red runs
///
/// `CLAUDE.md` pre-authorises the push here and says to read the previous run at the START of the
/// next round. It was not followed while CI carried **33 consecutive failures over two days** —
/// and the reason is not a missing rule. **A round that never looked and a round that looked and
/// saw green render identically**, so nothing about the second-to-last round could tell anybody
/// which one it had been.
///
/// # ⚠⚠⚠⚠ Why the GAP and not the reds
///
/// Item 776's own done-when (4) settles the axis on a sibling repository's measurement: it was
/// GREEN while nobody had read a hosted result for five rounds. Same structure, and the green was
/// luck. Counting reds scores those five rounds as zero — so what is counted here is the distance
/// from *the commit whose hosted result was read* to HEAD.
///
/// # ⚠⚠⚠ It is asserted to REPORT and asserted not to REFUSE, and both halves matter
///
/// A hook that refused would overturn *push and continue*, which this repository chose
/// deliberately — item 776 says the ceiling is not zero in as many words. So the claim is that the
/// push SAYS the number and still stands. Without the second half the obvious "improvement" is to
/// make it a gate, and the round that made it would be answering a question nobody asked.
#[test]
fn a_push_says_how_long_this_clone_has_gone_without_reading_a_hosted_result() {
    let push = Sandbox::new("push-hosted-read");
    push.write("notes.md", "a tree with no rust in it\n");
    push.git(&["add", "notes.md"]);
    let base = push.commit("base");
    push.write("notes.md", "a second commit\n");
    push.git(&["add", "notes.md"]);
    let head = push.commit("more notes");

    // ── A CLONE NOBODY HAS RECORDED A READING IN — the state every fresh worker starts in ────
    let run = push.run("pre-push", Some(&ref_line(&head, &base)), None);
    let told = said(&run);
    assert!(
        told.contains("NOBODY HAS RECORDED READING"),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 776: this push publishes without saying that nothing in this clone \
         has ever read a hosted result. That is the screen which is indistinguishable from *the \
         last run was green*, and being indistinguishable is what let 33 reds through: {told}",
    );
    assert!(
        !told.contains("0 round(s)"),
        "⛔⛔⛔ REGISTER ITEM 776: *nobody has read one* is being rendered as a count, and a count \
         of zero is what *I read it just now* looks like. An absence that renders like a \
         measurement gets acted on like one: {told}",
    );
    assert!(
        run.status.success(),
        "⚠⚠⚠ AND IT MUST NOT REFUSE. `CLAUDE.md` chose *push and continue* for this repository and \
         item 776's own done-when says the ceiling is not zero — a hook that stopped the push here \
         would be overturning that decision rather than measuring it: {told}",
    );

    // ── AND ONCE A READING IS RECORDED, IT COUNTS THE ROUNDS SINCE ───────────────────────────
    //
    // ⚠⚠ Recorded at BASE and not at HEAD, so the answer has to be a NUMBER rather than the
    // zero-shaped sentence — a report that only ever said *0 unread* would pass an assertion that
    // merely looked for a reading having happened.
    let marker = push.git(&["rev-parse", "--absolute-git-dir"]);
    std::fs::write(
        std::path::Path::new(marker.trim()).join("sprag-hosted-read"),
        format!("{base}\n"),
    )
    .expect("record a hosted read in the sandbox");
    let run = push.run("pre-push", Some(&ref_line(&head, &base)), None);
    let told = said(&run);
    assert!(
        told.contains("1 round(s) published since"),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 776: a reading recorded one commit back must be reported as the \
         DISTANCE to HEAD. That distance is the axis this item settled on — a sibling repository \
         was green while five rounds went unread, so *how many reds* scores exactly the wrong \
         thing: {told}",
    );
    assert!(
        told.contains(&base[..7]),
        "⛔⛔⛔ REGISTER ITEM 776: the report names a count and not the commit it counted from, so \
         nobody can check it or record the next one against it: {told}",
    );

    // ── AND AN OPEN GAP READS AS A DEBT, WHILE A SETTLED ONE READS AS A RECEIPT ──────────────
    //
    // ⛔⛔⛔⛔⛔ REGISTER ITEM 776, arm (5) — the half arm (2) does not reach. A sibling
    // repository's watcher named why an audible line is not yet enough: *"the more often
    // transition notifications come, the less they get looked at — there never seems to be a
    // reason."* A report that reads the same whether or not anything is owed becomes one more of
    // those, and the prescription item 776 settled on is NOT *look more often* — it is that an
    // opening gap has to arrive as something other than the routine line beside it.
    //
    // ⚠⚠⚠ THE CONTROL HERE IS ALIVE, unlike the two this workspace spent three rounds repairing
    // (register items 771 and 775): a gap of zero is a state this fixture can actually stage, it
    // renders through the same code as the open one, and a writer that stamped the cost clause
    // unconditionally is caught by the second assertion rather than by nothing.
    const COST: &str = "33 rounds";
    assert!(
        told.contains(COST),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 776 arm (5): the gap is open and the report says only a number. \
         What makes a rare signal survive beside a frequent one is that it stops looking like the \
         frequent one the moment it means something — and the number this repository owes here is \
         its own: {told}",
    );

    std::fs::write(
        std::path::Path::new(marker.trim()).join("sprag-hosted-read"),
        format!("{head}\n"),
    )
    .expect("record a hosted read at HEAD in the sandbox");
    let run = push.run("pre-push", Some(&ref_line(&head, &base)), None);
    let told = said(&run);
    assert!(
        told.contains("0 round(s) unread"),
        "⚠⚠⚠ THE CONTROL'S OWN PREMISE FAILED: a read recorded at HEAD is not reported as a \
         settled gap, so what follows would be a control over some other state: {told}",
    );
    assert!(
        !told.contains(COST),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 776 arm (5): a clone that owes NOTHING is being told what an \
         unread gap cost. A warning that arrives whether or not it applies is exactly the frequent \
         signal this arm exists to stop the rare one turning into — and after a few rounds of it, \
         the round that does owe something reads the same as the ones that did not: {told}",
    );

    // ── AND A GAP OF ZERO IS NOT A RECEIPT WHEN A RUN WAS LOOKED AT BEFORE IT SPOKE ──────────
    //
    // ⛔⛔⛔⛔⛔ REGISTER ITEM 779, and it is arm (5)'s own shape one unit over. The mark counts
    // COMMITS; what has to be read is RUNS. **Measured 2026-08-30**: three pushes were outstanding
    // at once — the oldest run still `in_progress` an hour and three quarters after it was created,
    // the two behind it `queued`, and nothing in `.github/workflows` serialising them. A reader at
    // the top of a round therefore meets `queued` on an ORDINARY round, looks honestly, and stamps
    // it — and the next `--seen` buries that run for good, because the mark could not tell *I read
    // a verdict* from *I looked and there was none*.
    //
    // ⚠⚠ The state is staged in the marker FILE rather than through `--seen`, deliberately: this
    // gate is about what a PUSH says, and driving the recorder here would make it a test of two
    // things at once. The recorder's own arms are `hosted-read.sh --selftest`, which this suite
    // runs elsewhere.
    std::fs::write(
        std::path::Path::new(marker.trim()).join("sprag-hosted-read"),
        format!("{head}\nowed {base}\n"),
    )
    .expect("record a look that found no verdict");
    let run = push.run("pre-push", Some(&ref_line(&head, &base)), None);
    let told = said(&run);
    assert!(
        told.contains("0 round(s) unread"),
        "⚠⚠⚠ THE PREMISE OF THE ARM BELOW: the gap has to be SETTLED for this to be about the \
         other debt at all — otherwise the sentence is carried by the gap and this proves nothing \
         about a run that never spoke: {told}",
    );
    assert!(
        told.contains(&base[..7]),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 779: a commit whose run had not spoken when somebody looked at it \
         is buried by the next read, and the push says nothing. That is register item 776's own \
         finding one unit over — *a round that never looked and a round that looked and saw green \
         render identically* — except here the round DID look, at a run that had not answered: \
         {told}",
    );
    assert!(
        run.status.success(),
        "⚠⚠ AND IT STILL MUST NOT REFUSE. Item 776 settled that the ceiling is not zero here, and \
         a second kind of debt does not reopen that decision: {told}",
    );

    // ── AND A GAP OF ZERO IS NOT A RECEIPT WHEN THE MARK STEPPED OVER A COMMIT ───────────────
    //
    // ⛔⛔⛔⛔⛔ REGISTER ITEM 781, and it is this item's own finding a THIRD time. Arm (5) held
    // that *nobody looked* and *somebody looked and saw green* must not render alike; item 779
    // held that *a verdict was read* and *a run had not spoken* must not either. What was left is
    // that **a commit the mark stepped over renders like one that was read** — because
    // `--seen <sha> settled` covers everything beneath it by construction, and the act that moved
    // the mark looked at exactly one run.
    //
    // ⚠⚠⚠ MEASURED ON THIS REPOSITORY'S OWN MARKER, 2026-08-30~31: `7b71077`'s macOS job was RED
    // — a pty-exhaustion refusal and a readiness assertion — the mark advanced past it to
    // `69a46db`, the commits in between were green so the DISTANCE was zero, and the push said
    // `0 round(s) unread`. A person following a written rule found that red; nothing on the screen
    // did, which is the same sentence this whole item was opened over.
    //
    // ⚠⚠ Staged in the marker FILE for the same reason the arm above is: this gate is about what a
    // PUSH says. The recorder's own arms — that a jump is enumerated at all, that reading each one
    // clears it, that a first read files no history — are `hosted-read.sh --selftest`.
    std::fs::write(
        std::path::Path::new(marker.trim()).join("sprag-hosted-read"),
        format!("{head}\nskipped {base}\n"),
    )
    .expect("record a commit the mark stepped over");
    let run = push.run("pre-push", Some(&ref_line(&head, &base)), None);
    let told = said(&run);
    assert!(
        told.contains("0 round(s) unread"),
        "⚠⚠⚠ THE PREMISE OF THE ARM BELOW: the gap has to be SETTLED for this to be about the \
         stepped-over commit at all — otherwise the sentence is carried by the distance and this \
         proves nothing about what the mark went past: {told}",
    );
    assert!(
        told.contains("STEPPED OVER"),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 781: the mark advanced past a commit whose run nobody has ever \
         looked at, and the push says the clone is up to date. That is exactly the screen that let \
         `7b71077`'s red through — the commits around it were green, so the distance was zero and \
         the debt had nowhere to appear: {told}",
    );
    assert!(
        told.contains(&base[..7]),
        "⛔⛔⛔ REGISTER ITEM 781: the report says something was stepped over and does not say \
         WHICH commit, so nobody can go and read that run — and a debt with no address is one that \
         cannot be paid off: {told}",
    );
    assert!(
        run.status.success(),
        "⚠⚠ AND IT STILL MUST NOT REFUSE. Item 776 settled that the ceiling is not zero here; a \
         third kind of debt does not reopen that decision either: {told}",
    );

    // ── AND A COMMIT THAT NEVER HAD A RUN IS NOT ONE NOBODY LOOKED AT ────────────────────────
    //
    // ⛔⛔⛔⛔⛔ REGISTER ITEM 790, and this item's finding a FOURTH time. GitHub hangs a run on the
    // TIP of a push, so a commit published underneath one never gets a run at all — and the
    // sentence the arm above asserts then sends a reader to go and read it. They find nothing.
    //
    // ⚠⚠⚠ MEASURED ON THIS REPOSITORY, 2026-08-31: `0642aa7` went out with `c772057`,
    // `actions/runs?head_sha=` answered `total_count` 0 for it and 1 for the tip, and the push
    // said *nobody has looked at their runs at all* — true about an absence, and pointing at
    // nothing. Worse, NEITHER word retires such a commit: `settled` would read a verdict that does
    // not exist and `unsettled` waits for a run that will never speak, so it sits in the list for
    // ever and the count stops being one anybody acts on.
    push.write("gh-total-count", "0\n");
    let run = push.run("pre-push", Some(&ref_line(&head, &base)), None);
    let told = said(&run);
    assert!(
        told.contains("never had a hosted run of their own"),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 790: a commit the mark stepped over that never had a run of its \
         own reads exactly like one whose run is sitting there unread. The reader goes to look, \
         finds nothing, and the commit cannot be cleared by either word: {told}",
    );
    assert!(
        !told.contains("STEPPED OVER"),
        "⛔⛔⛔⛔ REGISTER ITEM 790: the same commit is ALSO called one nobody has looked at, so \
         the report says both *go and read this run* and *there is no run* — and a reader acts on \
         the first. Two clauses over one commit is the covering, not the repair: {told}",
    );
    assert!(
        run.status.success(),
        "⚠⚠ AND IT STILL MUST NOT REFUSE — item 776's ceiling, a fourth time: {told}",
    );

    // ── AND *NOBODY COULD ASK* IS A THIRD STATE, NOT A QUIET PASS ────────────────────────────
    //
    // ⚠⚠⚠⚠ An absent `gh`, a refused call or a reply that is not a count answers NOTHING, and
    // this workspace's rule is that an unclassified case is RED rather than a pass. Folding it
    // into either measured answer is what would make the asking an escape hatch: *had no run* is
    // the one that DROPS a commit, so a silent fallback there would bury a real red the day the
    // network was down.
    std::fs::remove_file(push.dir.join("gh-total-count")).expect("take the staged count away");
    let run = push.run("pre-push", Some(&ref_line(&head, &base)), None);
    let told = said(&run);
    assert!(
        told.contains("could not be asked"),
        "⛔⛔⛔⛔ REGISTER ITEM 790: the question about this commit went unanswered and the report \
         gave one of the two answers it never got. *Not asked* and *asked and found none* have \
         different remedies, and only the second may retire a commit: {told}",
    );
    assert!(
        told.contains(&base[..7]),
        "⛔⛔⛔ REGISTER ITEM 790: a commit whose question went unanswered is not named, so nobody \
         can go and settle it by hand — a debt with no address cannot be paid: {told}",
    );
    assert!(
        run.status.success(),
        "⚠⚠ AND IT STILL MUST NOT REFUSE — item 776's ceiling once more: {told}",
    );
    push.write("gh-total-count", "1\n");
    push.done();
}
