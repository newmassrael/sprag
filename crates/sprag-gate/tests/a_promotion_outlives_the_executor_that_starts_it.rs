//! ⛔⛔⛔⛔⛔ **A PROMOTION OUTLIVES THE EXECUTOR THAT STARTS IT** — register item 1093, and the
//! state this repository was actually left in on 2026-09-13 at 18:54:59.
//!
//! # ⚠⚠⚠⚠⚠ What was measured, and why prose could not hold it
//!
//! The promotion procedure lived in a skill document as a block of shell for an agent to copy. Run
//! 344's agent copied it and ran it in its own shell — a descendant of a pane the loop daemon
//! owned. `kill-server` took the daemon down, the daemon took its panes down, and a pane took the
//! promoter with it, **between the kill and the copy**. Its output's last line is `promote: loop
//! daemon is 3062314`; the next statement, `echo "promote: kill-server rc=$?"`, never ran, and
//! `promote-kill.out` is 0 bytes. `$L/bin` kept its 09-11 binaries, no daemon came back, three runs
//! (344, 342, 355) were left `Cancelled` and the GUI was down for 42 minutes.
//!
//! ⭐ **THE KNOWLEDGE EXISTED.** Register item 758 was this exact face and was cancelled on 8-29,
//! because that day the daemon came back in 28 seconds — and it came back because that day's agent
//! decided, on its own, to run the procedure detached. The improvisation was never written
//! anywhere, so the cancellation recorded *one agent thought of it* as *this cannot happen*. Prose
//! is what that knowledge was stored in, and prose is read by whoever happens to be looking.
//!
//! # What this gate stages, which is the thing no unit test can
//!
//! The two arms differ in ONE fact: whether the promoter's process group is destroyed mid-procedure.
//!
//! * `a_promotion_whose_executor_is_killed_mid_procedure_still_lands` — the stand-in
//!   `kill-server` signals the promoter's group, exactly as the daemon's dying panes did. The
//!   promotion must still finish, and the verdict must still be written.
//! * `a_promotion_nobody_interrupts_still_lands` — the control. Without it, a repair that simply
//!   never promoted would satisfy the arm above by doing nothing, and this workspace has paid for
//!   green-by-emptiness more than once.
//!
//! ⚠⚠ **THE CONTROL IS NOT DECORATION HERE.** The failure being guarded against is *the procedure
//! stopped early*, and the cheapest way to pass a test about surviving an interruption is to be a
//! procedure that never does anything. Both arms assert the same landing.
//!
//! # ⚠ Why the process arms are Linux-only, said rather than skipped
//!
//! The identity under test is `/proc/<pid>/exe` — the kernel's own answer to *what image is this*,
//! and the whole subject of item 1093's second half. macOS has no `/proc`, so there is nothing
//! there for these arms to be right or wrong about. `the_script_and_this_workspace_name_one_set_of_images`
//! and its neighbour are text and run everywhere, so this file is never a suite of zero tests
//! reporting `ok`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The procedure under test.
const SCRIPT: &str = "crates/sprag-host/tools/promote-loop-daemon";

/// Where this workspace says, once, which images a promotion moves.
const IMAGES_AUTHORITY: &str = "crates/sprag-host/src/promotion.rs";

/// Where this workspace says, once, what a loop daemon is called.
const NAME_AUTHORITY: &str = "crates/sprag-rpc/src/lib.rs";

fn repo() -> PathBuf {
    sprag_gate::sources::workspace_root()
}

fn text_of(relative: &str) -> String {
    let path = repo().join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|why| panic!("{} is under test: {why}", path.display()))
}

/// The one line of `text` carrying `needle`.
fn line_with<'a>(text: &'a str, needle: &str) -> &'a str {
    text.lines()
        .find(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("no line of this source carries {needle:?}"))
}

/// What is between the first `open` and the next `close` of `line`.
fn between(line: &str, open: char, close: char) -> String {
    let from = line
        .find(open)
        .unwrap_or_else(|| panic!("{line:?} has no {open:?}"));
    let rest = &line[from + open.len_utf8()..];
    let to = rest
        .find(close)
        .unwrap_or_else(|| panic!("{line:?} has no {close:?}"));
    rest[..to].to_owned()
}

/// ⚠ Read off the RIGHT-HAND SIDE and not off the line: `pub const IMAGES: [&str; 4] = [ … ]`
/// opens a bracket in its TYPE, and a parse that took the first one answered `["&str; 4"]` — which
/// this gate's own four-name arm caught on 2026-09-13 rather than comparing two parse bugs.
fn rust_images() -> Vec<String> {
    let text = text_of(IMAGES_AUTHORITY);
    let line = line_with(&text, "pub const IMAGES");
    let (_, values) = line
        .split_once('=')
        .unwrap_or_else(|| panic!("{line:?} declares no value"));
    between(values, '[', ']')
        .split(',')
        .map(|one| one.trim().trim_matches('"').to_owned())
        .filter(|one| !one.is_empty())
        .collect()
}

fn shell_images() -> Vec<String> {
    let text = text_of(SCRIPT);
    between(line_with(&text, "IMAGES=("), '(', ')')
        .split_whitespace()
        .map(ToOwned::to_owned)
        .collect()
}

/// ⛔ **THE RATCHET, AND IT RUNS ON EVERY PLATFORM.** A promotion that moved three of four images
/// leaves a daemon serving one build and a CLI another — the skew `doctor` exists to name, and the
/// reason `promotion.rs` keeps this list in one place. The shell has to be a READER of that list,
/// never a second authority, or a fifth image gets built, promoted and never asked about.
#[test]
fn the_script_and_this_workspace_name_one_set_of_images() {
    let rust = rust_images();
    assert_eq!(
        rust.len(),
        4,
        "⛔ {IMAGES_AUTHORITY} stopped naming four images ({rust:?}), so either this parse has \
         broken or a promotion moves a different set now — and the shell below is judged against \
         whatever this reads",
    );
    assert_eq!(
        shell_images(),
        rust,
        "⛔ ITEM 1093: {SCRIPT} and {IMAGES_AUTHORITY} disagree about which images a promotion \
         moves. The script is a reader of that list and not a second authority: a name here that \
         is not there is an image nothing else knows to check, and a name there that is not here \
         is one the promotion silently leaves stale.",
    );
}

/// ⛔ **AND THE SAME FOR THE DAEMON'S NAME.** `runs_the_daemon`, `pids_named` and `kill-server` all
/// come through `sprag_rpc::DAEMON_BIN_NAME`, whose own doc says *the processes that LAUNCH a
/// daemon and the ones that RECOGNISE one must name it once*. The script recognises one, so it is
/// one of those processes.
#[test]
fn the_script_and_this_workspace_name_one_daemon() {
    let rpc = text_of(NAME_AUTHORITY);
    let shell = text_of(SCRIPT);
    let declared = between(line_with(&rpc, "pub const DAEMON_BIN_NAME"), '"', '"');
    let spelled = between(line_with(&shell, "DAEMON_BIN_NAME="), '"', '"');
    assert_eq!(
        spelled, declared,
        "⛔ ITEM 1093: {SCRIPT} calls a daemon {spelled:?} and {NAME_AUTHORITY} calls it \
         {declared:?}. A promoter that recognises the wrong name finds no daemon to stop and \
         copies binaries out from under a live one.",
    );
}

/// ⛔⛔⛔⛔⛔ **THE DAEMON THIS SCRIPT STARTS MUST NOT INHERIT THE PROMOTION LOCK** — register
/// item 1116, and the arm that keeps a promotion able to run a SECOND time.
///
/// # ⚠⚠ What the absence of `9>&-` cost, measured
///
/// `flock` holds on the OPEN FILE DESCRIPTION, and a fd inherited across `fork` SHARES it. A
/// daemon started with fd 9 still open therefore holds the promotion's own lock, and hands a copy
/// to every pane, shell, agent and driver it spawns — fifteen of them on 2026-09-15, with
/// `/proc/<daemon>/fd/9 -> .promote.lock` on a daemon this script had started that morning.
///
/// ⛔ **The cost is a CYCLE, not a warning.** The next promotion needs the lock; the lock releases
/// when that daemon dies; killing it is this script's own `kill-server`, which sits BELOW the
/// `flock` that already refused. Three promotions were refused in one day with *"another promotion
/// holds the lock"* while none was running, and nothing inside the script can break out of it.
///
/// ⚠ THE ASSERTION IS ON THE SPAWN LINE AND NOT ON THE FILE, because `9>&-` anywhere else would
/// close the promoter's own copy — which releases the lock and ends one-promotion-at-a-time. What
/// this holds is that the redirection is on the line that starts the daemon.
#[test]
fn the_daemon_this_script_starts_does_not_inherit_the_promotion_lock() {
    let shell = text_of(SCRIPT);
    let lock = line_with(&shell, "exec 9>");
    assert!(
        lock.contains(".promote.lock"),
        "⚠ THE PREMISE: this gate is about fd 9 being the promotion lock, and the line that opens \
         it no longer names that file — so every assertion below is about nothing. Got {lock:?}",
    );
    let spawn = line_with(&shell, "--daemon </dev/null");
    assert!(
        spawn.contains("9>&-"),
        "⛔⛔⛔⛔⛔ ITEM 1116: {SCRIPT} starts the daemon WITHOUT closing fd 9 for it, so that \
         daemon inherits this promotion's own lock and every process it spawns gets a copy. The \
         next promotion then refuses with *another promotion holds the lock* while none is \
         running, and the only way out is a person killing the daemon by hand — because the \
         `kill-server` that would release it is below the `flock` that refused. The line was: \
         {spawn:?}",
    );
}

// ⚠⚠ **WHERE THE SCRIPT'S OWN `--selftest` IS RUN, AND WHY NOT HERE.** It drives the identity
// predicate against the process strings measured on 2026-09-13, and it is run by
// `a_declared_selftest_is_one_this_suite_runs` — which walked `.githooks/` alone until this item
// widened it to the tree. An arm here would be a SECOND runner of one question, which is the shape
// that made that population narrow in the first place; the gate beside it holds the two populations
// equal, so a narrowing reds there rather than silently dropping this script.

// ─────────────────────────────────────────────────────────────────────────────────────────────
// The process arms. See this module's header for why they are Linux-only and said rather than
// silently skipped.
// ─────────────────────────────────────────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
mod staged {
    use super::{OsString, Path, PathBuf, SCRIPT, repo};
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command};
    use std::time::{Duration, Instant};

    /// The program standing in for all four images. Cargo builds it before this test runs, so
    /// nothing here writes a program — `sprag_gate::doubles` explains at length why that matters.
    const STAND_IN: &str = env!("CARGO_BIN_EXE_loop-promotion-stand-in");

    /// Long enough for a staging copy, a kill, four renames and a relaunch on a loaded machine.
    const BUDGET: Duration = Duration::from_secs(90);

    struct World {
        dir: PathBuf,
        root: PathBuf,
        bin: PathBuf,
        tree: PathBuf,
        state: PathBuf,
        verdict: PathBuf,
        head: String,
    }

    fn cut_git(run: &mut Command) -> &mut Command {
        sprag_gate::ambient::cut(run, &sprag_gate::ambient::inherited_git_names())
    }

    fn must(run: &mut Command, what: &str) -> String {
        let done = run
            .output()
            .unwrap_or_else(|why| panic!("{what} must be runnable: {why}"));
        assert!(
            done.status.success(),
            "{what} failed: {}{}",
            String::from_utf8_lossy(&done.stdout),
            String::from_utf8_lossy(&done.stderr),
        );
        String::from_utf8_lossy(&done.stdout).trim().to_owned()
    }

    /// A fixture install and a fixture checkout, with one program hard-linked in at four names.
    ///
    /// ⚠ `sprag-gui` and `sprag-mcp` start as TEXT rather than as the stand-in. Neither is executed
    /// before the promotion, and starting them as something a promotion could never produce is what
    /// makes *the new bytes landed* an unambiguous reading afterwards rather than an inode compared
    /// with itself.
    fn world(arm: &str) -> World {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("promotion-{arm}"));
        // ⚠ BOTH SPELLINGS of this path. `/proc/<pid>/exe` answers canonically and this workspace's
        // `target/` is a symlink, so a stray is found under the resolved name while everything
        // handed to the script below is deliberately left unresolved — which is what keeps the
        // script's own `cd -P` load-bearing rather than accidentally satisfied.
        if let Ok(resolved) = std::fs::canonicalize(env!("CARGO_TARGET_TMPDIR")) {
            clear_strays_at(&resolved.join(format!("promotion-{arm}")));
        }
        clear_strays_at(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        let root = dir.join("root");
        let bin = root.join("bin");
        let tree = dir.join("tree");
        let debug = tree.join("target").join("debug");
        let state = dir.join("state");
        let verdicts = dir.join("verdict");
        for each in [&bin, &debug, &state, &verdicts] {
            std::fs::create_dir_all(each).expect("the fixture must be creatable");
        }

        // ⚠ Through `git_in`, which cuts the git environment this process inherited: under
        // `pre-commit` an absolute `GIT_INDEX_FILE` outranks `current_dir`, and register item 965
        // is the rule that a suite's git child must never carry it.
        // ⚠ `core.hooksPath` is emptied deliberately: a global one would run the operator's hooks
        // inside a fixture that is not their repository.
        let quiet = [
            "-c",
            "user.email=gate@example.invalid",
            "-c",
            "user.name=promotion gate",
            "-c",
            "core.hooksPath=",
            "-c",
            "commit.gpgsign=false",
        ];
        must(
            sprag_gate::ambient::git_in(&tree).args(["init", "-q"]),
            "git init in the fixture checkout",
        );
        std::fs::write(tree.join("a"), b"a\n").expect("the fixture commit must have a file");
        must(
            sprag_gate::ambient::git_in(&tree)
                .args(quiet)
                .args(["add", "a"]),
            "git add in the fixture checkout",
        );
        must(
            sprag_gate::ambient::git_in(&tree).args(quiet).args([
                "commit",
                "-q",
                "-m",
                "the build under promotion",
            ]),
            "git commit in the fixture checkout",
        );
        let head = must(
            sprag_gate::ambient::git_in(&tree).args(["rev-parse", "--short", "HEAD"]),
            "git rev-parse in the fixture checkout",
        );

        for image in ["sprag", "sprag-term", "sprag-gui", "sprag-mcp"] {
            std::fs::hard_link(STAND_IN, debug.join(image))
                .unwrap_or_else(|why| panic!("the checkout's {image} must be linkable: {why}"));
        }
        for image in ["sprag", "sprag-term"] {
            std::fs::hard_link(STAND_IN, bin.join(image))
                .unwrap_or_else(|why| panic!("the install's {image} must be linkable: {why}"));
        }
        for image in ["sprag-gui", "sprag-mcp"] {
            std::fs::write(bin.join(image), format!("the old {image}\n"))
                .expect("the install's stale images must be writable");
        }

        World {
            verdict: verdicts.join("promote.rc"),
            dir,
            root,
            bin,
            tree,
            state,
            head,
        }
    }

    fn env_of(world: &World, build: &str) -> Vec<(String, OsString)> {
        vec![
            ("SPRAG_STAND_IN_STATE".into(), world.state.clone().into()),
            ("SPRAG_STAND_IN_BUILD".into(), build.into()),
            ("PROMOTE_ROOT".into(), world.root.clone().into()),
            ("PROMOTE_TREE".into(), world.tree.clone().into()),
            ("PROMOTE_SOCK".into(), world.dir.join("sock").into()),
            // ⚠ Empty on purpose: the script reads an empty unit name as *this install has no GUI
            // unit*, and a fixture must never restart the operator's.
            ("PROMOTE_GUI_UNIT".into(), String::new().into()),
            ("PROMOTE_WAIT".into(), "30".into()),
        ]
    }

    fn with_env<'a>(run: &'a mut Command, env: &[(String, OsString)]) -> &'a mut Command {
        for (name, value) in env {
            run.env(name, value);
        }
        cut_git(run)
    }

    fn wait_for(path: &Path, what: &str) -> String {
        let until = Instant::now() + BUDGET;
        while Instant::now() < until {
            if let Ok(said) = std::fs::read_to_string(path)
                && !said.trim().is_empty()
            {
                return said;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!(
            "⛔ ITEM 1093: {what} never appeared at {} within {BUDGET:?}. A promotion that leaves \
             no record is indistinguishable from one that died where run 344's did.",
            path.display(),
        )
    }

    /// ⚠⚠ A ZOMBIE IS NOT ALIVE. `/proc/<pid>` outlives a killed process until its parent reaps it
    /// — measured 2026-09-13, when this read a thirty-second-dead process as running and a wait sat
    /// out its whole budget on it.
    ///
    /// ⚠ This stays even though [`tear_down`] now reaps every child this fixture spawns, because
    /// the two answer different questions: reaping ends the leak, and this is asked of processes
    /// **between** their death and that reaping — and of the relaunched daemon, which is nobody's
    /// child here at all. The same reading is in the script's `still_running` and the stand-in's
    /// `alive`, neither of which can reap what they are asked about.
    fn alive(pid: &str) -> bool {
        if pid.is_empty() {
            return false;
        }
        let Ok(stat) = std::fs::read_to_string(Path::new("/proc").join(pid).join("stat")) else {
            return false;
        };
        stat.rsplit_once(')')
            .is_some_and(|(_, rest)| rest.split_whitespace().next() != Some("Z"))
    }

    /// End anything still running an image under `prefix`.
    ///
    /// ⛔ **BECAUSE A FAILED ARM LEAVES ITS DAEMON BEHIND, AND THE NEXT ARM FINDS IT.** Measured
    /// 2026-09-13: a panicking arm left a daemon whose `/proc/<pid>/exe` still named this fixture's
    /// path with ` (deleted)` after the directory was recreated, the scan under test correctly
    /// matched it — stripping that suffix is deliberate — and the next run stopped waiting for a
    /// daemon that was never going to answer. The fixture is what has to be clean, not the scan.
    fn clear_strays_at(prefix: &Path) {
        let prefix = prefix.to_string_lossy().into_owned();
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return;
        };
        for entry in entries.flatten() {
            let pid = entry.file_name().to_string_lossy().into_owned();
            if !pid.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let Ok(exe) = std::fs::read_link(entry.path().join("exe")) else {
                continue;
            };
            let exe = exe.to_string_lossy().into_owned();
            let exe = exe.strip_suffix(" (deleted)").unwrap_or(&exe);
            if exe.starts_with(&prefix) {
                signal(&pid);
            }
        }
    }

    fn exe_of(pid: &str) -> String {
        std::fs::read_link(Path::new("/proc").join(pid).join("exe"))
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    fn signal(what: &str) {
        let _ = Command::new("kill").arg("-9").arg("--").arg(what).status();
    }

    /// Bring a stand-in daemon up and answer with its pid, keeping the handle in `kids`.
    ///
    /// ⛔⛔⛔⛔⛔ **THE HANDLE IS KEPT BECAUSE DROPPING IT IS WHAT MAKES THE ZOMBIE.** A dropped
    /// [`Child`] is never reaped, so `/proc/<pid>` outlives the process and every liveness question
    /// asked about it answers *running* for ever. That is not a hypothetical here: it is the defect
    /// this fixture actually produced on 2026-09-13, where a stand-in reported *daemon 3285572
    /// outlived kill-server* about a process that had been dead for thirty seconds, and a gate run
    /// spent its whole budget waiting on it. `clippy::zombie_processes` names the same thing.
    fn start_daemon(world: &World, env: &[(String, OsString)], kids: &mut Vec<Child>) -> String {
        let mut run = Command::new(world.bin.join("sprag-term"));
        run.arg("--daemon");
        kids.push(
            with_env(&mut run, env)
                .spawn()
                .expect("the stand-in daemon must start"),
        );
        wait_for(&world.state.join("daemon.pid"), "the stand-in daemon's pid")
            .trim()
            .to_owned()
    }

    /// Start a stand-in DRIVER, spelling `argv[0]` the way this machine's real ones do.
    ///
    /// ⛔ `/proc/self/exe` is not decoration: item 763 has the daemon start its drivers by the
    /// kernel's handle to its own image, so that a driver cannot be a different build from its
    /// daemon — and that is exactly why the procedure item 1093 replaces never matched one.
    fn start_driver(world: &World, env: &[(String, OsString)], kids: &mut Vec<Child>) -> String {
        let mut run = Command::new(world.bin.join("sprag-term"));
        run.arg0("/proc/self/exe")
            .args(["--drive", "1", "-t", "loop", "-w", "sprag"]);
        let child = with_env(&mut run, env)
            .spawn()
            .expect("the stand-in driver must start");
        let pid = child.id().to_string();
        kids.push(child);
        pid
    }

    /// Run the promotion from a shell in a session of its own, and answer with that shell's pid.
    ///
    /// ⛔⛔ THE SESSION IS THE WHOLE FIXTURE. The promoter on 2026-09-13 was a descendant of a pane
    /// the daemon owned, so the daemon's death reached it. Here the wrapper is a session leader of
    /// its own, `doomed` decides whether the stand-in `kill-server` signals that session's process
    /// group, and the test runner is never in it.
    ///
    /// ⚠ The wrapper SITS THERE afterwards rather than exiting. The repair makes the caller's
    /// status mean *launched*, so a wrapper that returned immediately would have nothing left to
    /// kill and the arm would stage nothing.
    fn promote_from_a_doomed_shell(
        world: &World,
        env: &[(String, OsString)],
        doomed: bool,
        kids: &mut Vec<Child>,
    ) -> String {
        let script = repo().join(SCRIPT);
        let state = world.state.display();
        let export = if doomed {
            "export SPRAG_STAND_IN_DOOMED=$pgid; "
        } else {
            ""
        };
        let wrapper = format!(
            "pgid=$(ps -o pgid= -p $$ | tr -d ' '); \
             printf '%s' \"$$\" > {state}/wrapper.pid; \
             {export}\
             bash {} --verdict {} > {state}/caller.out 2>&1; \
             printf '%s' \"$?\" > {state}/caller.rc; \
             sleep 600",
            script.display(),
            world.verdict.display(),
        );
        let mut run = Command::new("setsid");
        run.arg("sh").arg("-c").arg(wrapper);
        kids.push(
            with_env(&mut run, env)
                .spawn()
                .expect("the doomed shell must start"),
        );
        wait_for(&world.state.join("wrapper.pid"), "the doomed shell's pid")
            .trim()
            .to_owned()
    }

    /// Everything this fixture started, ended — so a green run leaves no stray process behind.
    ///
    /// ⚠⚠ **THE WRAPPER GOES BY ITS PROCESS GROUP, NOT ITS PID.** It is a session leader (see
    /// [`promote_from_a_doomed_shell`]), so its pid IS its group — and signalling the pid alone
    /// leaves the `sleep` it is sitting in orphaned for its full term. That is register item 927's
    /// litter, made by the teardown that was supposed to prevent it.
    fn tear_down(world: &World, kids: &mut Vec<Child>) {
        if let Ok(pid) = std::fs::read_to_string(world.state.join("wrapper.pid")) {
            signal(&format!("-{}", pid.trim()));
        }
        if let Ok(pid) = std::fs::read_to_string(world.state.join("daemon.pid")) {
            signal(pid.trim());
        }
        // ⚠ And anything still running an image out of this fixture, whatever started it — the
        // relaunched daemon is not a child of this process and is named by no file the arms kept.
        clear_strays_at(&world.bin);
        if let Ok(resolved) = std::fs::canonicalize(&world.bin) {
            clear_strays_at(&resolved);
        }
        // ⛔ AND THE HANDLES ARE REAPED. Signalling ends the process; only `wait` ends the entry
        // in the process table, and an unreaped entry is what makes every later liveness question
        // about that pid answer *running*.
        for kid in kids.iter_mut() {
            let _ = kid.kill();
            let _ = kid.wait();
        }
        kids.clear();
    }

    fn assert_landed(world: &World, said: &str) {
        assert_eq!(
            said.trim(),
            "rc=0",
            "⛔ ITEM 1093: the promotion's own verdict is {said:?}. Its running account is at {}",
            world.verdict.with_extension("log").display(),
        );
        let stood_in = std::fs::read(STAND_IN).expect("the stand-in must be readable");
        for image in ["sprag-gui", "sprag-mcp"] {
            let landed = std::fs::read(world.bin.join(image)).unwrap_or_default();
            assert_eq!(
                landed.len(),
                stood_in.len(),
                "⛔ ITEM 1093: {image} in the install is still the stale file this fixture put \
                 there, so the promotion reported a verdict without moving the images. This is \
                 precisely the 2026-09-13 state: a promotion that stopped at its copy.",
            );
        }
        // ⛔⛔⛔⛔⛔ **THE IDENTITY SCAN MATCHED, AT PROCESS LEVEL.** This fixture runs exactly one
        // driver, and it spells argv[0] `/proc/self/exe` the way every real one does. The procedure
        // item 1093 replaces reported `promote: killing driver` ZERO times on 2026-09-13 and had
        // never matched a driver at all — so *found 1 driver(s)* is the sentence that could not
        // have been printed before, and the pure arms in the script's own selftest cannot say it.
        let account = std::fs::read_to_string(world.verdict.with_extension("log"))
            .expect("the promotion's running account must be readable");
        assert!(
            account.contains("found 1 driver(s)"),
            "⛔ ITEM 1093: the promoter did not identify the fixture's single driver. Its argv[0] \
             is `/proc/self/exe`, which is what defeated the procedure this replaces — identity \
             has to come off `/proc/<pid>/exe`. Its account said:\n{account}",
        );
        let daemon = std::fs::read_to_string(world.state.join("daemon.pid")).unwrap_or_default();
        let daemon = daemon.trim();
        assert!(
            alive(daemon),
            "⛔ ITEM 1093: no daemon is running after the promotion. On 2026-09-13 this machine \
             sat with no daemon for 42 minutes and three runs cancelled.",
        );
        // ⚠ CANONICAL, because `/proc/<pid>/exe` is. This fixture sits under a `target/` that is a
        // symlink on this machine, and comparing the uncanonicalised path is exactly the silent
        // miss the script's own `cd -P` now avoids — found here on 2026-09-13.
        let image = std::fs::canonicalize(world.bin.join("sprag-term"))
            .expect("the promoted daemon image must exist");
        assert_eq!(
            exe_of(daemon),
            image.display().to_string(),
            "the relaunched daemon must be the install's image, which is what the scan under test \
             identifies it by",
        );
    }

    /// ⛔⛔⛔⛔⛔ **THE ARM ITEM 1093 EXISTS FOR.**
    #[test]
    fn a_promotion_whose_executor_is_killed_mid_procedure_still_lands() {
        let world = world("interrupted");
        let env = env_of(&world, &world.head);
        let mut kids: Vec<Child> = Vec::new();
        let was = start_daemon(&world, &env, &mut kids);
        let _driver = start_driver(&world, &env, &mut kids);
        let wrapper = promote_from_a_doomed_shell(&world, &env, true, &mut kids);

        let said = wait_for(&world.verdict, "the promotion's verdict");
        assert_landed(&world, &said);

        // ⚠ THE STAGING IS ITSELF ASSERTED. If the promoter's group were never destroyed this arm
        // would be the control wearing another name, and would go on passing after the repair was
        // removed.
        assert!(
            !alive(&wrapper),
            "⛔ THE STAGING FAILED: the shell that started the promotion is still alive, so this \
             arm never reproduced 2026-09-13 and proves nothing about surviving it.",
        );
        let now = std::fs::read_to_string(world.state.join("daemon.pid")).unwrap_or_default();
        assert_ne!(
            now.trim(),
            was,
            "the daemon after a promotion must be the relaunched one, not the one that was running \
             before it",
        );
        tear_down(&world, &mut kids);
    }

    /// ⚠⚠ **THE CONTROL.** A procedure that never promoted would pass the arm above by doing
    /// nothing at all.
    #[test]
    fn a_promotion_nobody_interrupts_still_lands() {
        let world = world("plain");
        let env = env_of(&world, &world.head);
        let mut kids: Vec<Child> = Vec::new();
        let was = start_daemon(&world, &env, &mut kids);
        let _driver = start_driver(&world, &env, &mut kids);
        let wrapper = promote_from_a_doomed_shell(&world, &env, false, &mut kids);

        let said = wait_for(&world.verdict, "the promotion's verdict");
        assert_landed(&world, &said);
        assert!(
            alive(&wrapper),
            "the control's shell must NOT have been killed — without that this arm is the \
             interrupted one twice and the pair measures one thing",
        );
        let _ = was;
        tear_down(&world, &mut kids);
    }

    /// ⛔ **A VERDICT NOBODY COULD FIND IS NOT A VERDICT.** Both halves of the shape are refused
    /// where the caller can still read the refusal — before anything is taken down.
    #[test]
    fn a_verdict_path_that_could_not_be_waited_on_is_refused_before_anything_is_touched() {
        let world = world("refusals");
        let env = env_of(&world, &world.head);
        // ⚠ THE ABSOLUTE ONE IS BUILT FROM THE FIXTURE, NOT SPELLED AS `/tmp/…` — register item
        // 931. It is never created, since the script refuses it before touching anything, but a
        // literal scratch root in a source is a site no seam knows the prefix of, and the ratchet
        // that says so cannot tell a path that is only ever refused from one that gets made.
        let relative = "promote.rc".to_owned();
        let no_suffix = world.dir.join("promote-no-suffix").display().to_string();
        for bad in [&relative, &no_suffix] {
            let mut run = Command::new("bash");
            run.arg(repo().join(SCRIPT)).arg("--verdict").arg(bad);
            let done = with_env(&mut run, &env)
                .output()
                .expect("the script must be runnable");
            assert!(
                !done.status.success(),
                "⛔ ITEM 1093: {bad:?} was accepted as a verdict path. A detached promotion reports \
                 through that file and nothing else, so a path a waiter cannot name is a promotion \
                 nobody can read the result of.",
            );
        }
        // ⚠ And the install is untouched: a refusal that had already staged or killed would be a
        // worse failure than the one it refused.
        assert!(
            std::fs::read_to_string(world.bin.join("sprag-gui"))
                .unwrap_or_default()
                .contains("old"),
            "a refused promotion must not have touched the install",
        );
    }
}
