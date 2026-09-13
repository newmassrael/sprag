//! ⛔⛔⛔⛔⛔ **THE FOUR IMAGES A PROMOTION MOVES, STOOD IN FOR** — register item 1093, and the one
//! thing `a_promotion_outlives_the_executor_that_starts_it` cannot drive without.
//!
//! # ⚠⚠⚠⚠⚠ Why a compiled program and not a script
//!
//! The defect item 1093 is about is an identity test: the procedure that promotes asked `argv[0]`
//! what a process was, and `argv[0]` on this machine says `/proc/self/exe` for every driver. The
//! repair reads `/proc/<pid>/exe`, which is the kernel's own answer — so a stand-in daemon has to
//! be something `/proc/<pid>/exe` can name. **A `#!` script is not**: exec'ing one leaves
//! `/proc/<pid>/exe` pointing at the INTERPRETER, so a fixture built from shell scripts would make
//! the scan answer about `bash` and the gate would measure nothing.
//!
//! ⚠ It is reached by HARD LINK, which is what lets one program stand in at four names. Measured
//! 2026-09-13: a hard link's own path is what `/proc/<pid>/exe` reports and its own basename is
//! what `comm` reports, even though the inode is shared — so `bin/sprag-term` really does look like
//! this install's daemon to the scan under test. `crate::doubles`' rule is kept as well, because
//! linking never opens the target for writing and nothing here writes a program.
//!
//! # What it answers to
//!
//! `--version` (its build, from `SPRAG_STAND_IN_BUILD`), `--daemon` and `--drive` (stay up until
//! killed, which is all a promotion asks of them), `doctor` (is a daemon up) and `kill-server`.
//!
//! ⛔⛔ **AND `kill-server` KILLS THE DOOMED PROCESS GROUP, WHICH IS THE WHOLE POINT.** On
//! 2026-09-13 the real `kill-server` took down the daemon, the daemon took down its panes, and a
//! pane took down the shell running the promotion — mid-procedure, between the kill and the copy.
//! When `SPRAG_STAND_IN_DOOMED` names a process group this signals it, so a promoter that did not
//! detach dies exactly where run 344's did, and one that did detach is in another session and does
//! not. ⚠ It signals the group it may itself be in, and that is faithful rather than sloppy: run
//! 344's `promote-kill.out` is **0 bytes**, because the real one died the same way.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

/// Where this stand-in keeps what one invocation has to tell the next.
const STATE: &str = "SPRAG_STAND_IN_STATE";

/// What every image built from this program says its build is.
const BUILD: &str = "SPRAG_STAND_IN_BUILD";

/// The process group `kill-server` signals, or unset where a promotion is not being interrupted.
const DOOMED: &str = "SPRAG_STAND_IN_DOOMED";

/// How long `kill-server` waits for the daemon it signalled to actually go.
const GONE_BUDGET: Duration = Duration::from_secs(30);

fn state_dir() -> PathBuf {
    PathBuf::from(std::env::var(STATE).unwrap_or_else(|_| {
        panic!("{STATE} names the directory this stand-in keeps its daemon's pid in")
    }))
}

/// Whether `pid` is still a RUNNING process on this machine.
///
/// ⚠ `/proc` and not a signal: this program takes no dependencies, so it has no `kill(pid, 0)`, and
/// the gate that drives it is Linux-only for the same reason the thing under test is.
///
/// ⛔⛔⛔⛔⛔ **AND A ZOMBIE IS NOT ALIVE, WHICH COST A GATE RUN ON 2026-09-13.** `/proc/<pid>` goes
/// on existing for a killed child until its parent reaps it, and this stand-in's daemon is spawned
/// by a test harness that never waits. So the first version of this reported *daemon 3285572
/// outlived kill-server* about a process that had been dead for thirty seconds. The kernel's own
/// word for the difference is the state field of `/proc/<pid>/stat`, and `Z` is the whole of it.
fn alive(pid: &str) -> bool {
    let Ok(stat) = std::fs::read_to_string(Path::new("/proc").join(pid).join("stat")) else {
        return false;
    };
    // ⚠ Read AFTER the last `)`: field 2 is the executable name in parentheses and may itself
    // contain spaces and brackets, so splitting the whole line on whitespace is a parse that a
    // process called `sprag term)` would defeat.
    let Some((_, rest)) = stat.rsplit_once(')') else {
        return false;
    };
    rest.split_whitespace().next() != Some("Z")
}

/// Signal `what` — a pid, or a process group when it is written `-<pgid>`.
fn signal(what: &str) {
    let _ = Command::new("kill").arg("-9").arg("--").arg(what).status();
}

/// Stay up until something kills this, but never for ever.
///
/// ⚠ A BACKSTOP, not a behaviour under test: a gate arm that panics before its teardown leaves this
/// running, and a stand-in daemon that outlived the machine's uptime would be exactly the litter
/// register item 927 counts. Long enough that no arm can reach it, short enough that a crashed run
/// costs nothing by morning.
fn stay_up() -> ExitCode {
    std::thread::sleep(Duration::from_secs(900));
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let me = args
        .first()
        .map(PathBuf::from)
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "sprag".to_owned());
    let build = std::env::var(BUILD).unwrap_or_else(|_| "unknown".to_owned());
    let rest: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();

    // ⚠ The SHAPE `sprag_host::promotion::said_build` reads, parentheses and all: a stand-in that
    // said its build some other way would pass a gate the real images could not.
    if rest.contains(&"--version") {
        println!("{me} 0.0.1 ({build})");
        return ExitCode::SUCCESS;
    }

    // A daemon and a driver differ in what they are ASKED, never in what they do here: both stay up
    // until something kills them, and what the scan under test has to tell apart is their argv.
    if rest.contains(&"--daemon") {
        let dir = state_dir();
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("daemon.pid"), std::process::id().to_string());
        let _ = std::fs::write(dir.join("serving"), b"");
        return stay_up();
    }
    if rest.contains(&"--drive") {
        return stay_up();
    }

    match rest.first().copied() {
        Some("doctor") => {
            let dir = state_dir();
            let pid = std::fs::read_to_string(dir.join("daemon.pid")).unwrap_or_default();
            if !dir.join("serving").exists() || pid.is_empty() || !alive(pid.trim()) {
                eprintln!("stand-in: nothing is serving");
                return ExitCode::FAILURE;
            }
            println!("build {build}");
            ExitCode::SUCCESS
        }
        Some("kill-server") => {
            let dir = state_dir();
            let pid = std::fs::read_to_string(dir.join("daemon.pid")).unwrap_or_default();
            let pid = pid.trim().to_owned();
            let _ = std::fs::remove_file(dir.join("serving"));
            if !pid.is_empty() {
                signal(&pid);
                // ⚠ The real `kill-server` returns only once the socket has stopped serving, and
                // its own doc says why: returning early lets the next line of a promotion race a
                // daemon that is still cancelling runs. A stand-in that returned sooner would make
                // the procedure look safe in a way the product is not.
                let until = Instant::now() + GONE_BUDGET;
                while alive(&pid) && Instant::now() < until {
                    std::thread::sleep(Duration::from_millis(20));
                }
                if alive(&pid) {
                    eprintln!("stand-in: daemon {pid} outlived kill-server");
                    return ExitCode::FAILURE;
                }
            }
            // ⛔ AND NOW THE PANE THE PROMOTER WAS RUNNING IN, if this case is staging that.
            if let Ok(doomed) = std::env::var(DOOMED)
                && !doomed.is_empty()
            {
                signal(&format!("-{doomed}"));
            }
            ExitCode::SUCCESS
        }
        other => {
            eprintln!(
                "stand-in {me}: nothing here answers {:?} — it stands in for the four images a \
                 promotion moves, and answers --version, --daemon, --drive, doctor and kill-server",
                other.unwrap_or("no argument"),
            );
            ExitCode::from(2)
        }
    }
}
