//! ⛔⛔⛔⛔⛔ **MAY THIS REPOSITORY PROMOTE RIGHT NOW, AND WHAT IS BEHIND THE DOOR** — register
//! item 868, done-when ⑴ ⑵ and ⑷ in one place.
//!
//! ## Why a tool, and why it needs no promotion of its own
//!
//! Item 868's three conditions were measured by hand, three commands at a time, and added up in
//! somebody's head — so the day a window opened this side was not ready. Everything the answer
//! needs is readable AT READ TIME: the tree from `git status`, the door from `git log`, and each
//! binary's build **from its own version line**. That is the narrowing item 872 recorded — a
//! reader-time instrument escapes the promotion ceiling item 868 is about — so this can say
//! something true about a daemon older than itself, which is the only case that matters.
//!
//! ## ⭐ Why the binary's own word and never its mtime
//!
//! Item 868's ⑶ prescribed *바이너리 mtime vs HEAD 커밋 시각*, and this repository's north star
//! forbids exactly that: check a binary by what is IN it, not by when it was touched. A copy
//! rewrites an mtime without changing a byte. Every one of these binaries prints its build, so
//! nothing is inferred here.
//!
//! ## What it will not do
//!
//! It cannot see another repository's live run, so it does not pretend to — condition ⑴ comes back
//! *ask a person*, and that answer HOLDS THE VERDICT BACK rather than passing. An instrument that
//! read its own blindness as consent would say *promote* at the one moment the window is shut,
//! which is the failure that opened the item. `sprag my-runs` is what item 865 built for asking.

//! ## ⭐ And what the run that just ended should do — item 868's ⑶
//!
//! The last line answers the one question two register entries both claim: a finished run is the
//! only quiet this tree gets, item 827 says fill it in minutes and item 868 says leave it for a
//! build. `Readiness::what_follows_an_ending` settles it from the rows rather than by preference,
//! and this prints the verdict with the reason it was composed from. It says; it fires nothing.

use sprag_host::promotion::{Answer, BehindTheDoor, Readiness, all_of, said_build};
use sprag_host::runs::RunLog;

/// The one thing a person can tell this process that it cannot see — condition ⑴.
///
/// ⛔⛔⛔⛔⛔ **WITHOUT IT THE VERDICT HAS NO PATH TO *YES*, AND THAT IS A DEFECT AND NOT A
/// SAFEGUARD.** Condition ⑴ is another repository's live run; this process answers
/// [`Answer::Unknowable`] and that answer holds the verdict back, correctly. But an instrument
/// whose good value is unreachable is one nobody can act on — this workspace's rule 5 — and item
/// 865 built `sprag my-runs` precisely so a PERSON could answer this. This flag is where their
/// answer arrives. ⚠ The default is unchanged: silence stays *ask a person*, never a pass.
const WINDOW_OPEN: &str = "--window-open";

/// What this repository's promoted binaries live under.
const PROMOTED: &str = ".local/share/sprag-loop/bin";

/// The binaries a promotion moves — [`sprag_host::promotion::IMAGES`], read rather than retyped.
///
/// ⚠ It was a list HERE until register item 897 gave it a second reader (the gate that holds all
/// four to saying their build). Two spellings of *which images a promotion moves* is how a fifth
/// one gets built, promoted, and never asked.
use sprag_host::promotion::IMAGES as BINARIES;

fn main() -> std::process::ExitCode {
    let mut window_open = false;
    for arg in std::env::args().skip(1) {
        if arg == WINDOW_OPEN {
            window_open = true;
            continue;
        }
        eprintln!(
            "sprag-promotable: unknown argument {arg:?} — it takes {WINDOW_OPEN} and nothing else, \
             and reads the rest off this tree and the promoted binaries"
        );
        return std::process::ExitCode::FAILURE;
    }
    let head = match run(&["rev-parse", "--short", "HEAD"]) {
        Ok(head) => head,
        Err(why) => {
            eprintln!("sprag-promotable: cannot read HEAD: {why}");
            return std::process::ExitCode::FAILURE;
        }
    };

    // ── CONDITION ⑶: what each binary SAYS it is ────────────────────────────────────────────
    //
    // ⚠ All four are asked, and one that cannot be asked is a BLOCK rather than a skip: a
    // promotion that moves three of four leaves a daemon serving one image and a CLI another,
    // which is the skew `doctor` exists to name.
    let promoted = home().join(PROMOTED);
    let binaries = all_of(
        BINARIES
            .into_iter()
            .map(|name| match said_build(&promoted.join(name)) {
                Ok(build) if head_is(&build, &head) => Answer::Met,
                Ok(build) => Answer::Blocked(format!("{name} says {build}")),
                // ⛔⛔⛔ *CANNOT ASK* AND NOT *DOES NOT HOLD* — `all_of`'s own doc holds the
                // measurement: three of these four do not state their build at all, and calling
                // that a mismatch reads as *they are stale* and pins the verdict at NO for ever.
                Err(why) => Answer::Unknowable(format!("{name} cannot say ({why})")),
            })
            .collect(),
    );

    // ── CONDITION ⑵: whether anything has been edited ───────────────────────────────────────
    //
    // ⚠⚠ MEASURED AT THE MOMENT THE CONDITION BELONGS TO, which is item 868's own correction: the
    // question is whether the tree was quiet WHILE THE BUILD WAS MADE, and the only thing a single
    // reading can honestly say is whether it is quiet NOW. So this is reported as the tree's state
    // with its moment beside it, and a person building next reads it as *start from here*.
    let edited = run(&["status", "--porcelain"]).unwrap_or_default();
    let edited: Vec<&str> = edited.lines().filter(|line| !line.is_empty()).collect();

    let reading = Readiness::of([
        // ⚠ A PERSON'S ANSWER, OR NONE — see `WINDOW_OPEN`. Silence is `Unknowable` and holds the
        // verdict back; it is never read as consent, which is the failure that opened item 868.
        if window_open {
            Answer::Met
        } else {
            Answer::Unknowable(
                "not this process's to see — ask whoever holds the other run (`sprag my-runs`), \
                 then say so with --window-open"
                    .to_owned(),
            )
        },
        if edited.is_empty() {
            Answer::Met
        } else {
            Answer::Blocked(format!("{} path(s) edited in this tree", edited.len()))
        },
        binaries,
    ]);

    for (condition, moment, answer) in reading.rows() {
        let said = match answer {
            Answer::Met => "met".to_owned(),
            Answer::Blocked(why) => format!("BLOCKED — {why}"),
            Answer::Unknowable(why) => format!("ASK A PERSON — {why}"),
        };
        println!("  {:32} [{}] {said}", condition.word(), moment.word());
    }
    println!(
        "  {:32} {}",
        "may promote now",
        if reading.may_promote() { "YES" } else { "NO" }
    );
    // ── DONE-WHEN ⑶: which of the two register entries owns the instant a run ends ──────────
    //
    // ⛔ IT SAYS AND IT FIRES NOTHING — item 827 wrote *「자동으로 다시 걸어라」가 답이라고 미리
    // 정하지 마라*, and item 872's ⑵ made *`person` and `nothing` are never fired unattended* a
    // gate. Whether a re-fire is one a machine may make at all is the ENDING's disposition
    // (`sprag disposition`), a different question with a different answer.
    println!(
        "  {:32} {}",
        "a run just ended, so",
        reading.what_follows_an_ending()
    );

    // ── DONE-WHEN ⑷: and what is waiting behind the door ────────────────────────────────────
    //
    // ⛔⛔⛔⛔⛔ THE DAEMON'S OWN RECORDED WORD, not a binary's `--version` — measured 2026-09-05:
    // `sprag-term` reads `--version` as a command to spawn, so asking the daemon's own binary
    // answers nothing. Every run it drove stamped the build it was, so the newest row of its store
    // IS the daemon saying what it is, and `RunLog` is the product's own decode of that file.
    //
    // ⚠ `None` when nothing could say. See `BehindTheDoor::daemon` for what the first draft of
    // this printed instead.
    let daemon = daemon_build();
    let commits = daemon
        .as_ref()
        .and_then(|build| run(&["log", "--oneline", &format!("{build}..HEAD")]).ok())
        .unwrap_or_default();
    print!(
        "{}",
        BehindTheDoor {
            daemon,
            head,
            commits: commits
                .lines()
                .filter(|line| !line.is_empty())
                .map(ToOwned::to_owned)
                .collect(),
        }
    );

    // ⚠ EXIT STATUS IS THE VERDICT, so a script can read it without parsing prose — and a `no` is
    // not an error: it is an answer. `1` says *not now*, and stderr stays empty.
    if reading.may_promote() {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::from(1)
    }
}

/// **WHICH BUILD THE LOOP'S DAEMON SAYS IT IS**, read off the newest run it recorded — or [`None`]
/// when no store, no row, or no stamp can say.
///
/// ⚠ Decoded through [`RunLog`], the product's own shape for that file, rather than by walking a
/// `serde_json::Value`: a hand walk is a second reader of a format this crate owns.
fn daemon_build() -> Option<String> {
    let store = home().join(".local/share/sprag-loop/state/sprag/sprag-loop.runs.json");
    let read = std::fs::read_to_string(store).ok()?;
    let log: RunLog = serde_json::from_str(&read).ok()?;
    log.runs.iter().rev().find_map(|run| run.build.clone())
}

/// This user's home, or the current directory when the environment will not say.
fn home() -> std::path::PathBuf {
    std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_default()
}

/// Whether a stated build names this tree's `HEAD`. ⚠ Either may be the shorter prefix, so the
/// comparison is by prefix in both directions rather than by equality — `git` picks its own width.
fn head_is(build: &str, head: &str) -> bool {
    !build.is_empty() && (build.starts_with(head) || head.starts_with(build))
}

/// Run `git` in this tree and hand back its trimmed stdout.
fn run(args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .args(args)
        .output()
        .map_err(|error| error.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}
