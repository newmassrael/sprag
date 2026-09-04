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

use sprag_host::promotion::{Answer, BehindTheDoor, Readiness, all_of};
use sprag_host::runs::RunLog;

/// What this repository's promoted binaries live under.
const PROMOTED: &str = ".local/share/sprag-loop/bin";

/// The binaries a promotion moves — item 868's ⑶ counts FOUR, so a missing one is not a pass.
const BINARIES: [&str; 4] = ["sprag", "sprag-term", "sprag-gui", "sprag-mcp"];

fn main() -> std::process::ExitCode {
    if std::env::args_os().nth(1).is_some() {
        eprintln!(
            "sprag-promotable: takes no argument — it reads this tree and the promoted binaries"
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
        Answer::Unknowable(
            "not this process's to see — ask whoever holds the other run (`sprag my-runs`)"
                .to_owned(),
        ),
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

/// What `binary --version` says its build is — the parenthesised hash of `sprag 0.0.1 (<build>)`.
fn said_build(binary: &std::path::Path) -> Result<String, String> {
    let out = std::process::Command::new(binary)
        .arg("--version")
        .output()
        .map_err(|error| error.to_string())?;
    let said = String::from_utf8_lossy(&out.stdout);
    // ⚠ READ OFF THE PARENTHESES rather than by splitting on spaces: the version line's leading
    // words are the crate's name and number, and either may gain a word.
    let (_, after) = said.split_once('(').ok_or("no build in its version line")?;
    let (build, _) = after.split_once(')').ok_or("its build is not closed")?;
    Ok(build.trim().to_owned())
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
