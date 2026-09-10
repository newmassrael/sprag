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
    /// Whether [`Sandbox::finish`] stamps `HEAD^{tree}` as Rust-gate-cleared before every push.
    ///
    /// ⚠⚠⚠ ON for every case that is about something else, which is nearly all of them — see the
    /// note in [`Sandbox::finish`]. A case that is about the STAMPS themselves turns it off with
    /// [`Sandbox::stamps_are_mine`], because a fixture that rewrites the stamp before each run
    /// erases what the previous run's remedy just wrote — and *did the remedy take* is exactly the
    /// question `every_refusal_this_push_hook_gives_names_a_remedy_that_listens` asks.
    auto_stamp: std::cell::Cell<bool>,
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
            auto_stamp: std::cell::Cell::new(true),
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
        // ⛔⛔⛔⛔⛔ THE INHERITED GIT ENVIRONMENT IS CUT FIRST — register item 965, and this
        // helper was one variable short of the reasoning already written below it. `pre-commit`
        // runs this suite, and `git commit -- <pathspec>` exports an ABSOLUTE `GIT_INDEX_FILE`
        // naming the index it is about to commit; that outranks `current_dir`, so every `add` in
        // every sandbox here wrote into the caller's index. Measured 2026-09-08: twenty-one
        // sandboxes running in parallel collided on the caller's `index.lock` and git said
        // "another git process seems to be running" — the sandboxes were fighting over the
        // OPERATOR's repository.
        //
        // ⚠ The two `env` calls below still stand and still say what they are for. They are set
        // AFTER the cut, so they survive it.
        let run = sprag_gate::ambient::git_in(&self.dir)
            .args(args)
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

    /// Stamp a path-scoped push gate as already cleared — register item 1005.
    ///
    /// # ⚠⚠⚠ It writes what the REPOSITORY writes, by calling the repository's own function
    ///
    /// The stamp `pre-push` compares against is `paths_tree_of`'s exact output, trailing space and
    /// all. A fixture that spelled that format itself would be a second copy of it (item 213), and
    /// the copy that drifts is the one that makes a gate pass while the real stamp never matches —
    /// which is this whole file's subject, one level up.
    fn stamp_push_gate(&self, stamp: &str, paths: &[&str]) {
        self.stamp_push_gate_at("HEAD", stamp, paths);
    }

    /// The same stamp, for a rev that is not `HEAD` — register item 1030.
    ///
    /// ⚠⚠⚠ THE SWEEP NEEDS IT BECAUSE THE DEFECT IS ABOUT WHICH REV A STAMP IS ABOUT. Every
    /// `rust-gates.sh` verb stamps what the runner has checked out, so a push of some other tip is
    /// cleared only by standing there first — and a fixture that could stamp only `HEAD` could not
    /// carry out that remedy, which is the thing being measured.
    fn stamp_push_gate_at(&self, rev: &str, stamp: &str, paths: &[&str]) {
        let script = format!(
            ". \"$PWD/.githooks/content-gate.sh\"; paths_tree_of {rev} {}",
            paths.join(" "),
        );
        let mut command = Command::new("bash");
        command.arg("-c").arg(script).current_dir(&self.dir);
        for (name, value) in Sandbox::git_environment() {
            command.env(name, value);
        }
        let out = command
            .output()
            .unwrap_or_else(|why| panic!("read {paths:?} through content-gate.sh: {why}"));
        assert!(
            out.status.success() && !out.stdout.is_empty(),
            "⚠⚠⚠ THE FIXTURE COULD NOT READ WHAT IT MEANS TO STAMP: `paths_tree_of` answered \
             nothing for {paths:?}. Every arm using this would then assert a refusal it caused \
             itself.\n{}",
            String::from_utf8_lossy(&out.stderr),
        );
        std::fs::write(self.dir.join(".git").join(stamp), out.stdout)
            .unwrap_or_else(|why| panic!("write {stamp}: {why}"));
    }

    /// Stop [`Sandbox::finish`] stamping the Rust gates before each push, and clear what it wrote.
    ///
    /// For a case whose subject IS the stamps: from here on this sandbox holds exactly the stamps
    /// the case put there.
    fn stamps_are_mine(&self) {
        self.auto_stamp.set(false);
        for stamp in [
            "sprag-rust-gates-passed",
            "sprag-hook-suite-passed",
            "sprag-pixel-smoke-passed",
        ] {
            let _ = std::fs::remove_file(self.dir.join(".git").join(stamp));
        }
    }

    /// Stamp the Rust gates as having cleared `rev`'s whole tree — what `--clear` writes when it is
    /// run standing on `rev`.
    fn stamp_rust_gates_at(&self, rev: &str) {
        let tree = self.git(&["rev-parse", &format!("{rev}^{{tree}}")]);
        std::fs::write(
            self.dir.join(".git").join("sprag-rust-gates-passed"),
            format!("{tree}\n"),
        )
        .expect("write the Rust-gate stamp");
    }

    /// Run a hook the way git runs it: from the work tree, with the refs (if any) on stdin.
    fn run(&self, hook: &str, refs_on_stdin: Option<&str>, report: Option<&str>) -> Output {
        let mut command = self.hook_command(hook);
        if let Some(report) = report {
            command.env("REFS_REPORT", report);
        }
        self.finish(command, hook, refs_on_stdin)
    }

    /// The `Command` that runs a hook as git runs it — one spelling, shared by every arm.
    fn hook_command(&self, hook: &str) -> Command {
        let mut command = Command::new(self.dir.join(".githooks").join(hook));
        // ⛔⛔⛔⛔⛔ THE HOOK UNDER TEST IS A CHILD TOO — register item 965, and this is the half a
        // fix to `Self::git` alone would have missed. A hook's whole subject is *what is being
        // committed*, and it reads that from `GIT_INDEX_FILE`. Run from a real `git commit --
        // <pathspec>`, this suite inherits the OUTER repository's index, so every hook here judged
        // sprag's staged bytes while standing in the sandbox: measured 2026-09-08, nine cases
        // failed with `unable to read sha1 file of .githooks/pre-commit` — the sandbox had the
        // paths and not the blobs. The verdicts were about the wrong repository.
        sprag_gate::ambient::cut(&mut command, &sprag_gate::ambient::inherited_git_names());
        command
            .current_dir(&self.dir)
            .env("HOME", &self.dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("DOUBLE_LOG", &self.log)
            .env("PATH", self.path())
            .env_remove("REFS_REPORT")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // ⛔⛔⛔⛔⛔ **AND THEN THIS SANDBOX'S OWN, WHICH IS THE OTHER HALF** — register item 1017.
        //
        // The cut above is right and it was not enough: it left every hook here running with NO git
        // environment, and that is a state `git commit` never produces. A hook that mishandled one
        // of these variables was therefore green across all 29 cases while refusing every real
        // commit — measured 2026-09-10, twice in two rounds, and both times the thing that caught
        // it was a person driving `git commit` by hand.
        //
        // ⚠⚠ THE VALUES ARE THE SANDBOX'S, NEVER THE OPERATOR'S, and that is what makes supplying
        // them safe where inheriting them is the item-965 disaster: `.git/index` resolves under
        // `current_dir` above, which is this sandbox. A hook that walks off with it damages this
        // throwaway repository and nothing else.
        for (name, value) in Sandbox::git_environment() {
            command.env(name, value);
        }
        command
    }

    /// Arm the push fixture if this is a push, feed stdin, and read the hook's verdict.
    fn finish(&self, mut command: Command, hook: &str, refs_on_stdin: Option<&str>) -> Output {
        if hook == "pre-push" {
            command.args(["origin", "git@example.invalid:sprag.git"]);
            // ⛔⛔⛔⛔⛔ **THE TREE IS STAMPED AS ALREADY CLEARED, because since register item 480
            // the push hook REFUSES a tree the Rust gates have not been through.** It no longer
            // runs them itself: `git push` opens its connection before this hook, so a gate that
            // outlasts the remote's idle timeout loses the push after passing (7m15s, SIGPIPE 141,
            // the ref unmoved). Without this line every push driven here meets that refusal, and
            // the cases below — which are about the FORMAT gate, the pixel smoke and the ref walk
            // — would all be measuring one sentence they are not about.
            //
            // ⚠⚠ IT IS THE HONEST FIXTURE AND NOT A WAIVER: what it stages is a commit that went
            // through `pre-commit`, which is what the stamp means and the only way a real push
            // gets one. A case that wants the refusal writes no stamp and asserts it —
            // `no_push_hook_spends_the_connection_on_a_gate` holds the hook's side of the same
            // fact from the text.
            // ⚠ THROUGH `ambient::git_in`, not a bare `git` child — register item 965. `pre-commit`
            // runs this suite, so under `git commit -- <pathspec>` a child of its own would inherit
            // an ABSOLUTE `GIT_INDEX_FILE` naming the index git is about to commit, which outranks
            // `current_dir` and would land a sandbox's read in the operator's repository.
            if self.auto_stamp.get()
                && let Ok(tree) = sprag_gate::ambient::git_in(&self.dir)
                    .args(["rev-parse", "HEAD^{tree}"])
                    .output()
                && tree.status.success()
            {
                let stamp = self.dir.join(".git").join("sprag-rust-gates-passed");
                let _ = std::fs::write(stamp, tree.stdout);
            }
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

    /// The git environment `git commit` hands a hook, with this sandbox's own values.
    ///
    /// # ⛔⛔⛔⛔⛔ MEASURED FROM GIT, NOT REMEMBERED — register item 1017
    ///
    /// A scratch repository was committed to with a hook that printed its own `GIT_*`, 2026-09-10.
    /// Seven variables, and the list is not the one this repository had been guessing at:
    ///
    /// ```text
    /// GIT_AUTHOR_DATE  GIT_AUTHOR_EMAIL  GIT_AUTHOR_NAME
    /// GIT_EDITOR  GIT_EXEC_PATH  GIT_INDEX_FILE  GIT_PREFIX
    /// ```
    ///
    /// ⛔ **NO `GIT_DIR` AND NO `GIT_WORK_TREE`.** `content-gate.sh` cuts both, on a guess written
    /// while paying item 1011; the guess is harmless and it is still a guess, and this list is what
    /// replaces it. ⚠ And `GIT_INDEX_FILE` is **relative** (`.git/index`) for a plain `git commit`
    /// but an **absolute** `…/next-index-<pid>.lock` under `git commit -- <pathspec>` — a temporary
    /// index, not the repository's. Both shapes are staged below, because a hook that resolves the
    /// variable against the wrong directory fails on the first and not the second.
    ///
    /// # ⚠⚠ What is deliberately NOT supplied, and why that is a line rather than an omission
    ///
    /// `GIT_EDITOR` and `GIT_EXEC_PATH` describe the CALLER'S INSTALLATION — where git's helper
    /// binaries live, what to open for a message — and not the commit. A sandbox inventing an
    /// `EXEC_PATH` would break every git call the hook makes, which is a failure about this fixture
    /// rather than about any hook.
    /// ⚠ AND NOTHING THAT GIT DOES NOT SET. A sandbox-only marker here would be a variable no hook
    /// can ever meet, in the one fixture whose entire claim is *this is the environment git gives*.
    fn git_environment() -> Vec<(&'static str, String)> {
        vec![
            // ⚠ RELATIVE, exactly as git writes it, because that is the whole defect class: a
            // relative path is resolved against whatever directory reads it, and a hook that hands
            // it to a command running somewhere else gets a different index or none.
            ("GIT_INDEX_FILE", ".git/index".to_owned()),
            ("GIT_PREFIX", String::new()),
            // ⚠ The identity is the one this sandbox's own config carries, so `ident-gate.sh` —
            // which grades `git var GIT_AUTHOR_IDENT`, and that reads these first — sees exactly
            // what it saw before this environment existed. Supplying them is more faithful, not a
            // change of subject.
            ("GIT_AUTHOR_NAME", "sprag-gate".to_owned()),
            ("GIT_AUTHOR_EMAIL", allowed_ident_email()),
            ("GIT_AUTHOR_DATE", "@1756100000 +0900".to_owned()),
        ]
    }

    /// Run a hook the way `git commit -- <pathspec>` runs it: with an ABSOLUTE index somewhere
    /// other than `.git/index`.
    ///
    /// `git` in this sandbox, staging into `index` rather than into `.git/index`.
    ///
    /// ⚠ Through `ambient::git_in` like every other git call here (item 965), and the variable is
    /// set AFTER the cut — the cut removes what this process inherited, and a value named
    /// deliberately is not one it took away.
    fn git_into_index(&self, index: &std::path::Path, args: &[&str]) {
        let run = sprag_gate::ambient::git_in(&self.dir)
            .args(args)
            .env("HOME", &self.dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_INDEX_FILE", index)
            .output()
            .expect("git on PATH — the sandbox is a git repository");
        assert!(
            run.status.success(),
            "git {args:?} refused against {}: {}",
            index.display(),
            String::from_utf8_lossy(&run.stderr),
        );
    }

    /// ⚠ THE SAME `Command` [`Sandbox::run`] BUILDS, with one variable replaced — through
    /// [`Sandbox::hook_command`], because a second spelling of the setup is a second thing to keep
    /// current and the only difference this case is about is that one variable.
    fn run_with_index(&self, hook: &str, index: &std::path::Path) -> Output {
        let mut command = self.hook_command(hook);
        command
            .env("GIT_INDEX_FILE", index)
            .stdin(Stdio::null())
            .output()
            .unwrap_or_else(|why| panic!(".githooks/{hook} must be executable: {why}"))
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

/// Two bodies rustfmt accepts unchanged, told apart by the name they declare.
///
/// ⚠ BOTH FORMATTED ON PURPOSE. The gate under test below is the compiler's, not the format rule's,
/// and a fixture that was also unformatted would let the rustfmt refusal satisfy the case while
/// clippy went on reading whatever it liked.
const STAGED_BODY: &str = "fn staged() {}\n";

/// The same file, still being worked on. Never staged, and so never part of any commit.
const ON_DISK_BODY: &str = "fn on_disk() {}\n";

/// ⛔⛔⛔⛔⛔ **THE BIG GATES READ THE FILE ON DISK AND CALLED IT THE COMMIT** — register item 1011,
/// which is this file's opening defect one gate over and four months later.
///
/// Item 404 moved rustfmt onto the staged content and said so in `pre-commit`'s header. Clippy, the
/// rustdoc gate and the ratchet lane were left compiling the working tree — the gates that cost the
/// most, and the ones that decide whether the thing being committed BUILDS. So a commit could pass
/// every gate green and carry Rust that nothing had ever compiled. That is register item 213's
/// shape exactly: one rule, two spellings, and only the cheap one fixed.
///
/// ⚠⚠ **ALL THREE ARE COVERED SINCE 2026-09-10** — item 1011 moved clippy and the rustdoc gate,
/// item 1014 the ratchet lane, and the case below asserts each of them BY NAME rather than asserting
/// that "cargo" saw the right bytes. Three legs with one claim is one leg's worth of evidence: the
/// round that moved the first two left the third reading the disk, and a joint assertion would have
/// been green throughout.
///
/// ⚠⚠⚠ **DRIVEN END TO END BEFORE IT WAS WRITTEN**, outside this harness and with a real cargo,
/// because a defect measured only through a double is a claim about the double. 2026-09-10: stage
/// Rust that fails `-D warnings`, leave clean Rust on disk, `git commit` → **rc=0**; then
/// `cargo clippy --workspace --all-targets -- -D warnings` on the committed tree → **rc=101,
/// `error: unused variable`**. The commit that landed could not be built by the person who pulled it.
///
/// ⚠⚠ **THE ASSERTION IS ON THE BYTES, NOT ON WHERE THE GATE STOOD.** *Clippy ran in a mirror* is a
/// fact about the mechanism and would stay true if the mirror held the wrong tree; *clippy was
/// handed `fn staged`* is the answer, and it is the only form of the question that goes red when
/// the mirror is laid out from the wrong place. The `cargo` double reports it.
#[test]
fn the_rust_gates_compile_the_staged_bytes_and_not_the_file_on_disk() {
    let sandbox = Sandbox::new("commit-rust-gates-index");
    sandbox.write("subject.rs", STAGED_BODY);
    sandbox.git(&["add", "subject.rs"]);
    // …and then work carries on in the editor. This repository stages by path and reads
    // `git diff --cached` precisely because the two diverge (register item 196), so what follows is
    // the ordinary state here rather than a contrived one.
    sandbox.write("subject.rs", ON_DISK_BODY);

    let run = sandbox.run("pre-commit", None, None);
    let told = said(&run);
    assert!(
        run.status.success(),
        "both bodies are formatted and the doubled toolchain agrees with everything, so this \
         commit must pass — a refusal here would be about something this case is not: {told}",
    );

    let invoked = sandbox.invocations();
    assert!(
        invoked.contains("cargo clippy"),
        "the expensive gates must have run at all, or the assertion below is vacuously true:\n\
         {invoked}",
    );
    assert!(
        invoked.contains("cargo-saw clippy fn staged() {}"),
        "the INDEX holds `fn staged` and that is what this commit will carry, so that is what \
         clippy has to compile. It was given something else:\n{invoked}",
    );
    assert!(
        !invoked.contains("cargo-saw clippy fn on_disk() {}"),
        "and clippy must not have been given the working tree's copy, which nobody is \
         committing:\n{invoked}",
    );
    assert!(
        invoked.contains("cargo-saw doc fn staged() {}"),
        "and the rustdoc gate is the other half of item 1011 — it moved in the same edit and is \
         asserted separately, because one of the two could be left behind in silence:\n{invoked}",
    );

    // ⛔⛔ AND THE RATCHET LANE, WHICH THIS ASSERTION HELD AS A DEBT FOR ONE ROUND — register item
    // 1014. It used to require `cargo-saw test fn on_disk() {}`: the lane really did compile the
    // disk, and pinning that as a RED rather than describing it in prose is what made the round
    // that moved it come here and say so. It has moved; the assertion is inverted rather than
    // deleted, because *the tests run on the committed bytes* is now the claim worth defending.
    assert!(
        invoked.contains("cargo-saw test fn staged() {}"),
        "the ratchet lane compiles the index too since item 1014, so it must have been given \
         `fn staged` — the same bytes clippy and the rustdoc gate were given, and the ones this \
         commit carries:\n{invoked}",
    );
    assert!(
        !invoked.contains("cargo-saw test fn on_disk() {}"),
        "and no leg may still be reading the working tree's copy, which nobody is committing:\n\
         {invoked}",
    );
    sandbox.done();
}

/// ⚠⚠ **THE SAME FIX FROM THE OTHER SIDE** — a gate that compiled a mirror of some FIXED tree would
/// satisfy the case above for as long as the fixture never changed. This one commits, then stages a
/// second body, and requires the gates to have moved with the index.
///
/// ⚠ It is the arm that would catch a mirror laid out from `HEAD` rather than from the index — a
/// plausible reading of "the committed bytes", and the wrong one: `HEAD` is the commit BEFORE this
/// one.
#[test]
fn the_rust_gates_follow_the_index_rather_than_the_commit_already_made() {
    let sandbox = Sandbox::new("commit-rust-gates-moves");
    sandbox.write("subject.rs", ON_DISK_BODY);
    sandbox.git(&["add", "subject.rs"]);
    sandbox.commit("build(fixture): a first body, already committed");

    sandbox.write("subject.rs", STAGED_BODY);
    sandbox.git(&["add", "subject.rs"]);

    let run = sandbox.run("pre-commit", None, None);
    let invoked = sandbox.invocations();
    assert!(
        run.status.success(),
        "the staged body is formatted and every doubled tool agrees: {}",
        said(&run),
    );
    assert!(
        invoked.contains("cargo-saw clippy fn staged() {}"),
        "the index has moved on from `HEAD`, and the gates must judge where it moved TO:\n{invoked}",
    );
    sandbox.done();
}

/// ⛔⛔⛔⛔⛔ **A MIRROR THAT IS KEPT IS A MIRROR THAT CAN GO WRONG** — register item 1011's second
/// half, and the arm without which the fix would have shipped a new silent way to compile the wrong
/// bytes.
///
/// The layout persists between commits so that cargo's cache survives (a throwaway directory means a
/// cold build every time: 1m17s against 11.9s, measured). The price is that `git read-tree -m -u`
/// only touches the paths whose content moved — it does not look at the files it is leaving alone.
/// Measured 2026-09-10: corrupt one by hand, ask for the SAME tree again, and it exits **0** with
/// the wrong bytes still there. `content-gate.sh` therefore VERIFIES the layout and lays the whole
/// thing out again on any dirt at all.
///
/// ⚠⚠ THE CASE STAGES THE CORRUPTION THE REAL ONE WOULD BE — an interrupted sync leaves a file that
/// is not what the scratch index recorded — and requires the next run to compile the index anyway.
/// Delete the verification and this goes red while every other arm here stays green.
#[test]
fn a_mirror_that_was_corrupted_is_laid_out_again_rather_than_compiled() {
    let sandbox = Sandbox::new("commit-rust-gates-repair");
    sandbox.write("subject.rs", STAGED_BODY);
    sandbox.git(&["add", "subject.rs"]);

    // The first run lays the mirror out and leaves it there.
    let first = sandbox.run("pre-commit", None, None);
    assert!(
        first.status.success(),
        "the fixture's own first commit must pass: {}",
        said(&first),
    );
    let mirrored = sandbox
        .dir
        .join("target")
        .join("index-gates")
        .join("subject.rs");
    assert!(
        mirrored.is_file(),
        "the Rust gates are supposed to have checked the index out at {} — without that this case \
         is asserting about a path nothing writes",
        mirrored.display(),
    );

    // …and then it is not what it was. `read-tree` will not notice: the tree it is asked for has
    // not changed, so there is nothing it considers itself to owe.
    std::fs::write(&mirrored, ON_DISK_BODY).expect("corrupt the mirror");

    // ⚠⚠ THE LOG IS EMPTIED FIRST, or the assertion below is satisfied by the FIRST run's entry and
    // says nothing whatever about the second — a green about a line written before the corruption
    // existed. Measured by writing it the other way round: the case passed with the repair removed.
    std::fs::write(&sandbox.log, "").expect("empty the invocation log");

    let run = sandbox.run("pre-commit", None, None);
    let invoked = sandbox.invocations();
    assert!(
        run.status.success(),
        "nothing about the commit changed, so it must still pass: {}",
        said(&run),
    );
    assert!(
        invoked.contains("cargo-saw clippy fn staged() {}"),
        "the index still holds `fn staged`, so a checkout found in any other state must be laid \
         out again before anything compiles it:\n{invoked}",
    );
    assert!(
        !invoked.contains("cargo-saw clippy fn on_disk() {}"),
        "and the corrupted copy must not have been what clippy was given — that is neither the \
         index nor the working tree nor anything a person could name:\n{invoked}",
    );
    sandbox.done();
}

/// ⛔⛔⛔⛔⛔ **THE HOOK IS RUN WITH THE ENVIRONMENT GIT HANDS IT** — register item 1017, and this
/// is the face of this suite that did not exist until now.
///
/// # What was invisible, and what it cost twice
///
/// Every case here ran its hook with the git environment CUT — rightly, because inheriting the
/// operator's `GIT_INDEX_FILE` made nine cases judge sprag's staged bytes while standing in a
/// sandbox (item 965). But cut is not what `git commit` produces either, and the gap between them
/// held the entire real running environment of a hook. Measured 2026-09-10, twice in two rounds:
/// a hook that mishandled `GIT_INDEX_FILE` was **green across all 29 cases here** while refusing
/// **every** real commit with `fatal: .git/index: index file open failed: Not a directory`. Both
/// times what caught it was a person typing `git commit`.
///
/// # ⚠⚠ Why supplying is safe where inheriting was the disaster
///
/// The values are THIS SANDBOX'S. `.git/index` resolves under the sandbox's own working directory,
/// so a hook that walks off with the variable damages a throwaway repository. Item 965's defect was
/// the operator's index arriving here; this is the sandbox's index arriving at the hook, which is
/// the direction that was always missing.
///
/// # ⚠ The assertion is on what the CHILD received
///
/// `mnemosyne-cli` is the first thing `pre-commit` runs and it dumps its own `GIT_*` through
/// `/usr/bin/env`. That is the environment a child actually got, not what this fixture believes it
/// set — the two are the same claim only when nothing between them drops a variable.
#[test]
fn a_hook_is_driven_with_the_git_environment_a_commit_would_hand_it() {
    let sandbox = Sandbox::new("commit-git-environment");
    sandbox.write("crates/sprag-gui/paint.rs", FORMATTED);
    sandbox.git(&["add", "crates/sprag-gui/paint.rs"]);

    let run = sandbox.run("pre-commit", None, None);
    let told = said(&run);
    assert!(
        run.status.success(),
        "an ordinary commit must still pass once the hook is given git's environment — if this \
         refuses, the environment is what broke it and that is the finding: {told}",
    );

    let invoked = sandbox.invocations();
    for expected in [
        "git-env GIT_INDEX_FILE=.git/index",
        "git-env GIT_PREFIX=",
        "git-env GIT_AUTHOR_NAME=sprag-gate",
    ] {
        assert!(
            invoked.contains(expected),
            "the hook must have been handed `{expected}`, because `git commit` hands it — a run \
             without it is a run of something git never produces:\n{invoked}",
        );
    }
    assert!(
        invoked.contains(&format!(
            "git-env GIT_AUTHOR_EMAIL={}",
            allowed_ident_email()
        )),
        "and the identity must be the one this sandbox commits as, or `ident-gate.sh` is being \
         asked a different question than it was before:\n{invoked}",
    );
    sandbox.done();
}

/// ⛔⛔⛔ **AND THE OTHER SHAPE OF THE SAME VARIABLE, WHICH IS THE ONE THAT HIDES** — register item
/// 1017.
///
/// `git commit` writes `GIT_INDEX_FILE=.git/index`, RELATIVE. `git commit -- <pathspec>` writes an
/// ABSOLUTE `…/.git/next-index-<pid>.lock` — a temporary index that is not the repository's at all.
/// Measured on both, 2026-09-10. A hook that resolves the variable against the wrong directory
/// fails on the relative shape and sails through the absolute one, so a suite that staged only one
/// of them would be green for exactly half the ways a person commits.
///
/// ⚠ This repository commits BOTH ways: `git commit -- <path>` is what item 965 was measured on.
/// ⚠⚠ **AND THE TEMPORARY INDEX HOLDS DIFFERENT BYTES FROM `.git/index`, WHICH IS THE ONLY WAY
/// THIS CASE CAN GO RED.** A copy of the real index would make *judged the temporary one* and
/// *judged the real one* the same observation, and the case would pass over a hook that ignored the
/// variable entirely. Three states, all different: the temporary index carries `fn staged`,
/// `.git/index` carries `fn on_disk`, and so does the file on disk.
#[test]
fn a_hook_is_driven_with_the_absolute_index_a_pathspec_commit_hands_it() {
    let sandbox = Sandbox::new("commit-git-environment-pathspec");
    sandbox.write("subject.rs", ON_DISK_BODY);
    sandbox.git(&["add", "subject.rs"]);

    // The shape git writes for `git commit -- <pathspec>`: an ABSOLUTE path to a lock file beside
    // the real index, holding the content that commit is about — not the repository's own index.
    let temporary = sandbox.dir.join(".git").join("next-index-probe.lock");
    std::fs::copy(sandbox.dir.join(".git").join("index"), &temporary)
        .expect("a temporary index beside the real one, as a pathspec commit makes");
    sandbox.write("subject.rs", STAGED_BODY);
    sandbox.git_into_index(&temporary, &["add", "subject.rs"]);
    sandbox.write("subject.rs", ON_DISK_BODY);

    let run = sandbox.run_with_index("pre-commit", &temporary);
    let told = said(&run);
    assert!(
        run.status.success(),
        "a pathspec commit hands an ABSOLUTE temporary index and the gates must judge it exactly \
         as they judge the ordinary one: {told}",
    );

    let invoked = sandbox.invocations();
    assert!(
        invoked.contains(&format!("git-env GIT_INDEX_FILE={}", temporary.display())),
        "the hook must actually have been handed it:\n{invoked}",
    );
    assert!(
        invoked.contains("cargo-saw clippy fn staged() {}"),
        "and the gates must have compiled what the TEMPORARY index carries — `.git/index` and the \
         file on disk both say `fn on_disk`, so reading either of them is the defect this case is \
         for:\n{invoked}",
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
///
/// # ⛔⛔⛔⛔⛔ It asserted the SMOKE ITSELF until register item 1005, and the answer got stronger
///
/// The smoke is `cargo build --release` over three crates, and `git push` opens its connection
/// before this hook runs — so it was minutes spent on an open ssh session, which is the shape item
/// 480 measured at 7m15s and SIGPIPE 141 with every gate PASSED. It is now REFUSED here and run by
/// `rust-gates.sh --clear-pixel`, outside any connection.
///
/// ⚠⚠ That is the same stance one notch further, not a retreat: what is asserted is still that
/// this push does not go through unlooked-at. The third thing — a quiet pass — is what must never
/// happen, and it is what this arm holds.
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
        !run.status.success(),
        "⛔⛔⛔ this push changes what the GUI paints and nothing has looked at the pixels, so it \
         must not pass: {told}",
    );
    assert!(
        !reached_the_pixel_smoke(&told),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 1005: it REACHED the smoke instead of refusing. That is \
         `cargo build --release` over three crates inside an open GitHub connection, which is the \
         7m15s that lost a push after every gate passed: {told}",
    );
    assert!(
        told.contains("--clear-pixel"),
        "⚠⚠⚠ AND THE REFUSAL MUST NAME THE COMMAND THAT CLEARS IT. Nothing writes this stamp as a \
         side effect of committing, so a person meeting this refusal has nothing else to type: \
         {told}",
    );
    sandbox.done();
}

/// ⛔⛔⛔⛔⛔ **AND A TREE THE SMOKE HAS CLEARED GOES THROUGH** — register item 1005, and without
/// this arm the one above is satisfied by a hook that refuses every push that paints.
///
/// ⚠⚠ THE STAMP IS SCOPED TO WHAT THE GATE READS, which is where this differs from item 480's:
/// that one stamps the tip tree, because gates compiling the workspace owe another look at any
/// change. This one is owed only where `PIXEL_PATHS` moves, so a tip-tree stamp would go stale on
/// every unrelated commit and charge minutes to publish a change the smoke cannot see — the cost
/// that makes a gate get waived, which is item 688 wearing different clothes.
#[test]
fn a_push_whose_paint_the_smoke_already_cleared_is_not_refused() {
    let sandbox = Sandbox::new("push-paint-cleared");
    sandbox.write("crates/sprag-host/base.rs", FORMATTED);
    sandbox.git(&["add", "crates/sprag-host/base.rs"]);
    let base = sandbox.commit("base");

    sandbox.write("crates/sprag-gui/paint.rs", FORMATTED);
    sandbox.git(&["add", "crates/sprag-gui/paint.rs"]);
    let head = sandbox.commit("a change under crates/sprag-gui");
    sandbox.stamp_push_gate(
        "sprag-pixel-smoke-passed",
        &["crates/sprag-gui", "crates/sprag-grid"],
    );

    let run = sandbox.run("pre-push", Some(&ref_line(&head, &base)), None);
    let told = said(&run);
    assert!(
        run.status.success(),
        "⛔⛔⛔ THE SMOKE CLEARED EXACTLY THESE BYTES, and refusing anyway makes the refusal a wall \
         rather than a gate — the stamp is the whole mechanism and this is the arm that reads it: \
         {told}",
    );
    assert!(
        !reached_the_pixel_smoke(&told),
        "and it must not run the smoke either — a stamp that is read and then ignored costs the \
         connection exactly what item 1005 is about: {told}",
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
        !run.status.success(),
        "⛔⛔⛔ THIS PUSH EDITS A HOOK, and the only crate that can tell whether a hook still works \
         is the one that drives it. Publishing without it having run is what put a red on \
         twenty-one pushes: {told}",
    );
    assert!(
        !reached_the_hook_suite(&told),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 1005: it RAN the suite instead of refusing. The comment that \
         used to justify running it here measured the SUITE at under a second — not the BUILD \
         underneath it, which is minutes on a cold tree, spent on an open GitHub connection: \
         {told}",
    );
    assert!(
        told.contains("--clear-hooks"),
        "⚠⚠⚠ AND THE REFUSAL MUST NAME THE COMMAND THAT CLEARS IT — a person who has just edited \
         a hook has never run this suite outside a connection, so this is the only way through: \
         {told}",
    );
    sandbox.done();
}

/// ⛔⛔⛔⛔⛔ **AND A HOOK THE SUITE HAS CLEARED GOES THROUGH** — register item 1005's other half,
/// and without it the arm above is satisfied by a hook that refuses every push touching
/// `.githooks` — including, note, every push this round itself makes.
#[test]
fn a_push_whose_hooks_the_suite_already_cleared_is_not_refused() {
    let sandbox = Sandbox::new("push-hook-cleared");
    sandbox.write("crates/sprag-host/base.rs", FORMATTED);
    sandbox.git(&["add", "crates/sprag-host/base.rs"]);
    let base = sandbox.commit("base");

    sandbox.write(".githooks/some-gate.sh", "#!/bin/sh\nexit 0\n");
    sandbox.git(&["add", ".githooks/some-gate.sh"]);
    let head = sandbox.commit("a change under .githooks");
    sandbox.stamp_push_gate("sprag-hook-suite-passed", &[".githooks"]);

    let run = sandbox.run("pre-push", Some(&ref_line(&head, &base)), None);
    let told = said(&run);
    assert!(
        run.status.success(),
        "⛔⛔⛔ THE SUITE CLEARED EXACTLY THESE HOOKS and the push is still refused, which makes \
         the stamp decorative and the refusal a wall: {told}",
    );
    assert!(
        !reached_the_hook_suite(&told),
        "and it must not run the suite either: {told}",
    );
    sandbox.done();
}

/// ⛔⛔⛔⛔⛔ **A DELETION IS NOT CHARGED FOR EITHER GATE** — register item 1005's fourth done-when,
/// and item 480 paid to learn the rule it applies.
///
/// # What it cost there, and why each site owes the question again
///
/// While `pre-push` RAN its gates, a push nothing could be said about was answered conservatively:
/// run them anyway. That was free — a gate over a tree that is not being published wastes time and
/// refuses nothing. **A refusal is not free.** `git push --delete` publishes no content, its local
/// sha is all zeros and there is no tree to stamp, so refusing it means a stale branch can never be
/// removed.
///
/// # ⚠⚠⚠⚠⚠ WHERE THE PROPERTY IS DECIDED, measured rather than assumed
///
/// A waiver was written at both call sites first, on item 480's model. **Removing it changed
/// nothing** — this arm stayed green — because `pushed_range_touches` `continue`s past an all-zero
/// local sha, so a deletion-only push is NOT OWED and never reaches either refusal. The waiver was
/// deleted: a branch that cannot be entered is a rule that reads exactly like a live one.
///
/// ⚠⚠ SO THIS ARM IS THE ONLY THING HOLDING IT, which is why it asserts the hook's exit rather than
/// its wording. The day that walk starts calling a deletion owed, the refusal becomes reachable and
/// this goes red — which is the notice a person needs, and the reason the property is not left to
/// a comment in the walk.
///
/// ⚠ The stamp reader answers *no* for a deletion too, by construction: no tree, so no stamp can be
/// about it. That is not a second decision — it is never consulted for one.
#[test]
fn a_deletion_is_not_refused_by_either_push_gate() {
    let sandbox = Sandbox::new("push-delete-gates");
    sandbox.write(".githooks/some-gate.sh", "#!/bin/sh\nexit 0\n");
    sandbox.git(&["add", ".githooks/some-gate.sh"]);
    sandbox.write("crates/sprag-gui/paint.rs", FORMATTED);
    sandbox.git(&["add", "crates/sprag-gui/paint.rs"]);
    let head = sandbox.commit("a tree that both gates would be owed for");

    // The shape git sends for `git push --delete`: an all-zero LOCAL sha, and a remote that has
    // the branch. Both gates would be owed if this carried a tree — and it carries none.
    let run = sandbox.run("pre-push", Some(&ref_line(ABSENT, &head)), None);
    let told = said(&run);
    assert!(
        run.status.success(),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 1005: a deletion was REFUSED. It publishes no content, so there \
         is nothing for either gate to be about — and a branch that can never be removed is what \
         item 480 measured this exact mistake costing: {told}",
    );
    assert!(
        !reached_the_pixel_smoke(&told) && !reached_the_hook_suite(&told),
        "and nothing may be RUN for it either: {told}",
    );
    assert!(
        told.contains("carries no tree to compile"),
        "⚠⚠⚠ AND IT MUST STILL BE DECIDED WHERE THE CODE SAYS IT IS DECIDED — register item 1030. \
         The stamp walks skip a deletion rather than answer NO to it, and if `saw_ref` ever started \
         counting one, this push would be waved through by a STAMP READER claiming a tree it does \
         not carry was cleared. Same exit, different reason, and the reason is the rule: {told}",
    );
    sandbox.done();
}

/// ⛔⛔⛔⛔⛔ **A DELETION PUSHED ALONGSIDE A REAL REF NO LONGER REFUSES THE REAL ONE** — register
/// item 1030, and this is the shape that item was opened for.
///
/// # What it was, measured 2026-09-11 by driving this repository's own hook
///
/// The stamp was CORRECT for the tree being published — `.git/sprag-rust-gates-passed` equalled
/// `git rev-parse HEAD^{tree}` — and `main` alone passed. Adding one deletion line to the same
/// stdin turned it into a refusal naming `bash .githooks/rust-gates.sh --clear`, **which could not
/// clear it**: the walk answered NO because of the deletion, whatever the stamp said, so the person
/// pays a multi-minute command and meets the same sentence. Item 480 wrote *"a refusal whose remedy
/// is not a command is a wall"*; this was the wall wearing the remedy's clothes.
///
/// ⚠ The sweep below holds the general property. This arm holds the measured shape, so a reader
/// meeting the register entry finds the case it names.
#[test]
fn a_deletion_pushed_alongside_a_real_ref_does_not_refuse_the_real_one() {
    let sandbox = Sandbox::new("push-mixed-deletion");
    sandbox.write("crates/sprag-host/base.rs", FORMATTED);
    sandbox.git(&["add", "crates/sprag-host/base.rs"]);
    let base = sandbox.commit("base");

    sandbox.write("crates/sprag-host/more.rs", FORMATTED);
    sandbox.git(&["add", "crates/sprag-host/more.rs"]);
    let head = sandbox.commit("a change the Rust gates are owed for");

    let mixed = format!(
        "{}refs/heads/gone {ABSENT} refs/heads/gone {base}\n",
        ref_line(&head, &base),
    );
    let run = sandbox.run("pre-push", Some(&mixed), None);
    let told = said(&run);
    assert!(
        run.status.success(),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 1030: the tree being published is exactly the one the stamp \
         cleared, and one deletion line in the same push refused it — naming a command that writes \
         that same stamp and so cannot change this answer: {told}",
    );
    sandbox.done();
}

/// ⚠⚠⚠ **THE CONTROL, AND WITHOUT IT THE ARM ABOVE IS SATISFIED BY A HOOK THAT WAVED THROUGH ANY
/// PUSH CARRYING A DELETION.** What item 1030 asked to be measured FIRST is what a deletion leaving
/// the stamp judgment loosens — and the answer has to be *nothing*: every ref that carries a tree
/// is still compared, so a real ref the gates have not cleared is refused whether or not a deletion
/// rides along with it.
#[test]
fn a_deletion_does_not_carry_an_uncleared_ref_through_with_it() {
    let sandbox = Sandbox::new("push-mixed-uncleared");
    sandbox.write("crates/sprag-host/base.rs", FORMATTED);
    sandbox.git(&["add", "crates/sprag-host/base.rs"]);
    let base = sandbox.commit("base");

    sandbox.write("crates/sprag-host/more.rs", FORMATTED);
    sandbox.git(&["add", "crates/sprag-host/more.rs"]);
    let head = sandbox.commit("a change the Rust gates are owed for");
    sandbox.stamps_are_mine();

    let mixed = format!(
        "{}refs/heads/gone {ABSENT} refs/heads/gone {base}\n",
        ref_line(&head, &base),
    );
    let run = sandbox.run("pre-push", Some(&mixed), None);
    let told = said(&run);
    assert!(
        !run.status.success(),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 1030: nothing has cleared this tree and the push carried it \
         anyway, because a deletion was in the same list. That is the loosening the item asked to \
         be measured before the repair, and the repair must not be it: {told}",
    );
    sandbox.done();
}

/// ⛔⛔⛔⛔⛔ **A PUSH GIT NAMED NO REFS IN IS NOT REFUSED** — register item 1030's second face, and
/// the one that fires on the most ordinary command in this repository.
///
/// # ⚠⚠⚠ It replaces an arm that asserted the opposite, and the premise is what changed
///
/// `a_push_with_no_refs_on_stdin_is_refused_rather_than_waived` read an empty list as *"something
/// IS being published and nobody knows what"* and required the refusal. That premise was a guess,
/// and `git_hands_a_pre_push_hook_no_refs_when_it_has_nothing_to_publish` below is the measurement
/// that replaces it: git writes one line per ref it is about to update, and none only when there is
/// nothing to update. So an empty list is git saying this push publishes nothing.
///
/// ⚠⚠ WHAT THE GUESS COST, measured 2026-09-11: `git push origin main` on an up-to-date branch was
/// REFUSED, naming `bash .githooks/rust-gates.sh --clear` — a command that cannot clear it, because
/// the walk was counting refs and not reading the stamp. A no-op push exited 1.
///
/// ⚠ WHAT IS NOT WAIVED is the case item 480's stance was actually about — a ref git DID name and
/// this clone cannot read. `a_push_carrying_a_ref_this_clone_cannot_read_is_refused` holds that.
#[test]
fn a_push_git_named_no_refs_in_is_not_refused() {
    let sandbox = Sandbox::new("push-no-refs-passes");
    sandbox.write(".githooks/some-gate.sh", "#!/bin/sh\nexit 0\n");
    sandbox.write("crates/sprag-gui/paint.rs", FORMATTED);
    sandbox.git(&["add", ".githooks/some-gate.sh", "crates/sprag-gui/paint.rs"]);
    sandbox.commit("a tree every gate here would be owed for, had anything been pushed");
    sandbox.stamps_are_mine();

    let run = sandbox.run("pre-push", None, None);
    let told = said(&run);
    assert!(
        run.status.success(),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 1030: git names one ref per ref it will update and named none, \
         which it does only when there is nothing to update. Refusing that makes `git push` fail on \
         an up-to-date branch, and the command the refusal named could never clear it: {told}",
    );
    assert!(
        !reached_the_pixel_smoke(&told) && !reached_the_hook_suite(&told),
        "and nothing may be RUN for a push that publishes nothing either: {told}",
    );
    sandbox.done();
}

/// ⚠⚠⚠ **AND A REF GIT DID NAME THAT THIS CLONE CANNOT READ IS STILL REFUSED** — item 480's stance,
/// kept where it is real. This is the case that one above is NOT: something is being published and
/// nobody here can say what. The two were one branch until register item 1030 measured them apart.
#[test]
fn a_push_carrying_a_ref_this_clone_cannot_read_is_refused() {
    let sandbox = Sandbox::new("push-unreadable-ref");
    sandbox.write("crates/sprag-host/base.rs", FORMATTED);
    sandbox.git(&["add", "crates/sprag-host/base.rs"]);
    let base = sandbox.commit("base");

    // A local sha this repository has never held: git named a ref, and the content behind it is
    // unreachable from here.
    let stranger = "1111111111111111111111111111111111111111";
    let run = sandbox.run("pre-push", Some(&ref_line(stranger, &base)), None);
    let told = said(&run);
    assert!(
        !run.status.success(),
        "⛔⛔⛔ a push naming content this clone cannot resolve must not pass: that is the case \
         item 480's *every unknown answers no* was written for, and it is the one register item \
         1030 kept while waiving the empty list: {told}",
    );
    sandbox.done();
}

/// ⛔⛔⛔⛔⛔ **WHAT GIT ACTUALLY HANDS THIS HOOK WHEN THERE IS NOTHING TO PUSH** — register item
/// 1030, and this arm exists because the sentence it checks used to live in a comment.
///
/// The repair above rests on one fact about git, not about this repository: **git writes one ref
/// line per ref it is about to update, and writes none when there is none.** A fact in prose is a
/// fact nobody re-measures, and this repository has been wrong about exactly this comment once
/// already — so the fact is asked of git, here, every run, with a real remote and a real push.
///
/// ⚠ The hook under test is not involved: a recording hook stands in for it, because the subject is
/// git's side of the contract. What the real hook does with an empty list is the arm above.
#[test]
fn git_hands_a_pre_push_hook_no_refs_when_it_has_nothing_to_publish() {
    let sandbox = Sandbox::new("push-git-contract");
    sandbox.write("crates/sprag-host/base.rs", FORMATTED);
    sandbox.git(&["add", "crates/sprag-host/base.rs"]);
    sandbox.commit("base");

    // A remote of this sandbox's own, so the push is real and reaches nothing outside it.
    let remote = sandbox.dir.join("remote.git");
    sandbox.git(&[
        "init",
        "--quiet",
        "--bare",
        remote.to_str().expect("a utf-8 path"),
    ]);
    sandbox.git(&[
        "remote",
        "add",
        "origin",
        remote.to_str().expect("a utf-8 path"),
    ]);

    // A hook that records how many ref lines it was handed, and refuses so nothing is published
    // by the measurement itself.
    let counted = sandbox.dir.join("ref-lines");
    sandbox.write(
        "recording-hooks/pre-push",
        "#!/usr/bin/env bash\nn=0\nwhile IFS= read -r line; do\n  [ -n \"$line\" ] && \
         n=$((n+1))\ndone\nprintf '%s\\n' \"$n\" >>\"$REF_LINE_LOG\"\nexit 1\n",
    );
    let hooks = sandbox.dir.join("recording-hooks");
    let hook = hooks.join("pre-push");
    let mut mode = std::fs::metadata(&hook)
        .expect("the recording hook")
        .permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut mode, 0o755);
    std::fs::set_permissions(&hook, mode).expect("make the recording hook executable");

    let push = |sandbox: &Sandbox, refspec: &str| {
        let mut command = sprag_gate::ambient::git_in(&sandbox.dir);
        command
            .args([
                "-c",
                &format!("core.hooksPath={}", hooks.display()),
                "push",
                "origin",
                refspec,
            ])
            .env("HOME", &sandbox.dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("REF_LINE_LOG", &counted);
        command.output().expect("git on PATH")
    };

    // First push: git has a ref to update, so it names one — and the recording hook refuses, which
    // leaves the remote empty and the branch still ahead.
    push(&sandbox, "HEAD:refs/heads/main");
    let named = std::fs::read_to_string(&counted).unwrap_or_default();
    assert_eq!(
        named.trim(),
        "1",
        "⚠⚠⚠ THE CONTROL: git must name the ref it is about to update, or the arm below is \
         measuring a hook that is never handed anything. It said: {named:?}",
    );

    // Now publish it for real, with no hook in the way, and push the same thing again.
    sandbox.git(&["push", "--quiet", "origin", "HEAD:refs/heads/main"]);
    let _ = std::fs::remove_file(&counted);
    let run = push(&sandbox, "HEAD:refs/heads/main");
    let named = std::fs::read_to_string(&counted).unwrap_or_default();
    assert_eq!(
        named.trim(),
        "0",
        "⛔⛔⛔⛔⛔ REGISTER ITEM 1030: this is the fact the empty-list branch in `pre-push` rests \
         on — git hands a hook NO ref lines when there is nothing to update. If git ever names a \
         ref here, that branch is waiving a push that publishes something, and it must be taken \
         out. git said {named:?}, and the push said: {}",
        said(&run),
    );
    sandbox.done();
}

/// A ref line in the shapes `git push` actually produces — register item 1030.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Pushed {
    /// The tip the pusher has checked out, which is what every clearing verb stamps.
    Head,
    /// A tip that is NOT what is checked out, and carries a different tree.
    Older,
    /// `git push --delete`: an all-zero local sha, and no tree at all.
    Deletion,
}

impl Pushed {
    /// The rev this ref publishes; `None` for a deletion, which publishes nothing.
    fn tip<'a>(self, head: &'a str, older: &'a str) -> Option<&'a str> {
        match self {
            Pushed::Head => Some(head),
            Pushed::Older => Some(older),
            Pushed::Deletion => None,
        }
    }

    /// The line git would write for it.
    fn line(self, slot: usize, base: &str, older: &str, head: &str) -> String {
        let name = if slot == 0 {
            "refs/heads/main"
        } else {
            "refs/heads/side"
        };
        match self {
            Pushed::Head => format!("{name} {head} {name} {older}\n"),
            Pushed::Older => format!("{name} {older} {name} {base}\n"),
            Pushed::Deletion => format!("{name} {ABSENT} {name} {base}\n"),
        }
    }
}

/// The remedy this hook offers when one stamp cannot cover a push — register item 1030.
const SPLIT_REMEDY: &str = "push the refs one at a time";

/// The remedy it offers when the clearing verb would stamp something other than what is pushed.
const STAND_REMEDY: &str = "stand where you are pushing from";

/// Carry out whatever the hook's refusal tells a person to do, until the push goes through.
///
/// ⛔⛔⛔⛔⛔ **THE REMEDY IS EXECUTED, NOT MATCHED.** A case asserting that a refusal *contains*
/// the word `--clear` is satisfied by a refusal that names a command which cannot change its own
/// answer, and that is precisely what register item 1030 is: three shapes where the named command
/// wrote a value about something else and the same sentence came back. So this does what the
/// message says — and only what the message says — and the assertion is that the push then passes.
///
/// ⚠⚠ WHICH REV THE COMMAND IS RUN FROM IS TAKEN FROM THE MESSAGE TOO. Every `rust-gates.sh` verb
/// stamps what the runner has checked out, so a bare *run this command* means running it where you
/// are, and only the sentence naming [`STAND_REMEDY`] licenses standing somewhere else. A hook that
/// stops printing that line therefore has this walk stamp `HEAD`, meet the same refusal, and go red
/// — which is the notice a person needs.
fn carry_out_until_it_passes(
    sandbox: &Sandbox,
    shape: &[Pushed],
    base: &str,
    older: &str,
    head: &str,
) -> Result<(), String> {
    let stdin: String = shape
        .iter()
        .enumerate()
        .map(|(slot, kind)| kind.line(slot, base, older, head))
        .collect();
    let refs = if shape.is_empty() {
        None
    } else {
        Some(stdin.as_str())
    };

    let mut last = String::new();
    for _ in 0..8 {
        let run = sandbox.run("pre-push", refs, None);
        last = said(&run);
        if run.status.success() {
            return Ok(());
        }
        if last.contains(SPLIT_REMEDY) {
            // ⚠⚠ AND THE RECURSION IS BOUNDED BY THIS LINE RATHER THAN BY A COUNTER: each level
            // is handed strictly fewer refs, and a push of ONE cannot be told to split — that is
            // an error, not a deeper level. A `depth` parameter was carried here until clippy
            // pointed out it was read only to be passed on, which is what a redundant guard looks
            // like from the outside.
            if shape.len() < 2 {
                return Err(format!(
                    "the hook told a push of {} ref(s) to be split, which is not something anybody \
                     can do: {last}",
                    shape.len(),
                ));
            }
            for one in shape {
                carry_out_until_it_passes(sandbox, std::slice::from_ref(one), base, older, head)?;
            }
            return Ok(());
        }
        let named = ["--clear-hooks", "--clear-pixel", "--clear"]
            .into_iter()
            .find(|verb| last.contains(&format!("rust-gates.sh {verb}")));
        let Some(verb) = named else {
            return Err(format!(
                "the refusal named neither a command nor a split, which is the wall item 480 \
                 forbade: {last}",
            ));
        };
        // ⚠ DISTINCT tips, because two refs at the same commit are one place to stand — which is
        // the same counting the hook does when it decides whether one stamp can cover the push.
        let mut tips: Vec<&str> = shape
            .iter()
            .filter_map(|kind| kind.tip(head, older))
            .collect();
        tips.sort_unstable();
        tips.dedup();
        let from = if last.contains(STAND_REMEDY) {
            match tips.as_slice() {
                [only] => *only,
                _ => {
                    return Err(format!(
                        "the hook said to stand where this push comes from, and it comes from {} \
                         places: {last}",
                        tips.len(),
                    ));
                }
            }
        } else {
            "HEAD"
        };
        match verb {
            "--clear" => sandbox.stamp_rust_gates_at(from),
            "--clear-hooks" => {
                sandbox.stamp_push_gate_at(from, "sprag-hook-suite-passed", &[".githooks"]);
            }
            "--clear-pixel" => sandbox.stamp_push_gate_at(
                from,
                "sprag-pixel-smoke-passed",
                &["crates/sprag-gui", "crates/sprag-grid"],
            ),
            _ => unreachable!("the list above is the list matched here"),
        }
    }
    Err(format!(
        "the remedy was carried out eight times over and the refusal did not move: {last}",
    ))
}

/// ⛔⛔⛔⛔⛔ **EVERY REFUSAL THIS HOOK CAN GIVE MUST NAME A REMEDY THAT LISTENS** — register item
/// 1030, and this is the general property the three arms above are instances of.
///
/// # Where the population comes from, rather than a hand-picked list
///
/// A ref line git can write is one of three things: the tip you have checked out, some other tip,
/// or a deletion. The shapes are then every push of nought, one or two of those — thirteen — and a
/// case is not chosen, it is enumerated. Item 1030 was found by asking *item 1005's fourth
/// done-when* at one more site; a list of shapes somebody thought of would have missed it the same
/// way.
///
/// ⚠⚠ **AND TWO IS ENOUGH, WHICH IS AN ARGUMENT AND NOT A BUDGET.** Every walk in `pre-push` is a
/// fold over the ref list, and each branch any of them takes is decided by two things: how many
/// refs carry a tree — none, one, or more than one — and whether those trees are all the same. Two
/// ref lines realise every combination of both; a third adds a longer list and no new branch. The
/// day a walk starts caring about ref ORDER or about a count above two, this reasoning is what
/// stops being true, and the sentence is here so that is noticed rather than assumed.
///
/// # What is measured, and why it is not `contains`
///
/// Each shape starts from a tree nothing has cleared. The hook is run; if it refuses, whatever it
/// told the person to do is DONE — the stamp that command would write, from the rev the message
/// says to run it in, or the split it asks for — and it is run again. A shape passes when the push
/// goes through. A shape FAILS when the remedy is carried out and the same refusal comes back,
/// which is the exact defect item 1030 names: a command that is there and does not listen.
#[test]
fn every_refusal_this_push_hook_gives_names_a_remedy_that_listens() {
    let sandbox = Sandbox::new("push-remedy-sweep");
    sandbox.write(".githooks/some-gate.sh", "#!/bin/sh\nexit 0\n");
    sandbox.write("crates/sprag-gui/paint.rs", FORMATTED);
    sandbox.git(&["add", ".githooks/some-gate.sh", "crates/sprag-gui/paint.rs"]);
    let base = sandbox.commit("base");

    sandbox.write(".githooks/some-gate.sh", "#!/bin/sh\nexit 0 # older\n");
    sandbox.write(
        "crates/sprag-gui/paint.rs",
        "fn paint() {}\n\nfn older() {}\n",
    );
    sandbox.git(&["add", ".githooks/some-gate.sh", "crates/sprag-gui/paint.rs"]);
    let older = sandbox.commit("a tip that is not what is checked out");

    sandbox.write(".githooks/some-gate.sh", "#!/bin/sh\nexit 0 # head\n");
    sandbox.write(
        "crates/sprag-gui/paint.rs",
        "fn paint() {}\n\nfn head() {}\n",
    );
    sandbox.git(&["add", ".githooks/some-gate.sh", "crates/sprag-gui/paint.rs"]);
    let head = sandbox.commit("the tip that is checked out");

    let alphabet = [Pushed::Head, Pushed::Older, Pushed::Deletion];
    let mut shapes: Vec<Vec<Pushed>> = vec![Vec::new()];
    shapes.extend(alphabet.iter().map(|one| vec![*one]));
    for first in alphabet {
        shapes.extend(alphabet.iter().map(|second| vec![first, *second]));
    }
    assert_eq!(
        shapes.len(),
        13,
        "⚠ the population is every push of nought, one or two ref lines over three kinds — a count \
         that changes means the enumeration below stopped being the enumeration it claims",
    );

    let mut walls = Vec::new();
    for shape in &shapes {
        sandbox.stamps_are_mine();
        if let Err(why) = carry_out_until_it_passes(&sandbox, shape, &base, &older, &head) {
            walls.push(format!("{shape:?} — {why}"));
        }
    }
    assert!(
        walls.is_empty(),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 1030: these pushes are refused by a message whose remedy does not \
         reach them. Item 480 wrote *a refusal whose remedy is not a command is a wall*; a command \
         that cannot change the answer is that wall with the remedy's clothes on, and a person \
         meeting one pays minutes to be told the same thing again.\n\n{}",
        walls.join("\n\n"),
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

/// ⚠⚠⚠ **A PUSH THIS HOOK CANNOT DESCRIBE IS NOT WAVED THROUGH.** The failure this guards is a
/// change going unlooked-at, so not knowing has to mean *do not pass*, and the refusal has to name
/// the command that clears it — register item 480.
///
/// # ⛔⛔⛔ It asserted the PIXEL SMOKE until register item 480, and the answer got stronger
///
/// While `pre-push` still ran the Rust gates itself, *not knowing* meant running them and then the
/// smoke, and this case read the smoke's own line to prove nothing had been waived. Since 480 the
/// push hook refuses a tree those gates have not cleared — and a push whose refs it cannot read is
/// a push it cannot say that about, so it is REFUSED before anything expensive begins.
///
/// # ⛔⛔⛔⛔⛔ IT WAS AN EMPTY REF LIST UNTIL REGISTER ITEM 1030, AND THAT WAS THE WRONG SUBJECT
///
/// This case used to stage *no refs at all* and call it *a push nobody can describe*. Git was then
/// asked what it actually sends, and the answer was that an empty list is git DESCRIBING the push:
/// nothing is being updated. Two different situations had been sharing one branch, and the one this
/// case is about — content being published that nobody here can read — is staged by naming a sha
/// this clone has never held. `a_push_git_named_no_refs_in_is_not_refused` holds the other, and
/// `git_hands_a_pre_push_hook_no_refs_when_it_has_nothing_to_publish` is what separates them.
#[test]
fn a_push_this_hook_cannot_describe_is_refused_rather_than_waived() {
    let sandbox = Sandbox::new("push-no-refs");
    sandbox.write("crates/sprag-host/base.rs", FORMATTED);
    sandbox.git(&["add", "crates/sprag-host/base.rs"]);
    let base = sandbox.commit("base");

    let stranger = "2222222222222222222222222222222222222222";
    let run = sandbox.run("pre-push", Some(&ref_line(stranger, &base)), None);
    let told = said(&run);
    assert!(
        !run.status.success(),
        "⛔⛔⛔ an unresolvable push must not pass: this hook cannot read the ref git named, so it \
         cannot say the tree being published has been through any gate — and a push nobody can \
         describe is the one most likely to carry work nothing has seen: {told}",
    );
    assert!(
        told.contains(stranger),
        "⚠⚠⚠ AND THE REFUSAL MUST NAME THE REF IT COULD NOT READ — register item 480's *a refusal \
         whose remedy is not a command is a wall*, applied to the one refusal here that no stamp \
         can lift. Nothing a person types clears an unreadable sha, so what they need instead is \
         WHICH sha it was; a refusal that withholds it leaves them re-running a push to find out: \
         {told}",
    );
    // ⛔⛔⛔⛔⛔ **AND IT IS NOT TOLD TO GO AND STAND ON THE SHA IT CANNOT READ** — register item
    // 1030, and this arm is the ONLY thing holding that.
    //
    // `STAND_REMEDY` is the remedy for a tip this clone HAS and has not checked out. Offered for a
    // sha that is not here at all it would be the very defect item 1030 is about, written by the
    // repair for it. A guard against that was written into `name_a_remedy_that_listens` first and
    // DELETED: the mutation that breaks it left every case green, because this push is refused by
    // the identity walk long before that line is reached. A branch nothing can enter reads exactly
    // like a live rule, so it went — and the property is asserted here instead, where it will go
    // red the day that ordering changes and the advice becomes reachable.
    assert!(
        !told.contains(STAND_REMEDY),
        "the push was told to stand somewhere it cannot stand: {told}",
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
