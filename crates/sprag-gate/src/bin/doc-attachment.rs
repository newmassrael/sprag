//! ⛔⛔⛔⛔⛔ **WHICH COMMITS MOVED A DOC ONTO ANOTHER ITEM** — register item 1088.
//!
//! ```text
//! doc-attachment <commit>...
//! git rev-list --no-merges HEAD | doc-attachment
//! ```
//!
//! Judges each commit against its first parent with [`sprag_gate::doc_attachment::judge_commit`],
//! in the repository this is started in. One line per displacement, one summary; exit 1 when any
//! commit moved a doc or could not be judged.
//!
//! # ⚠⚠ Why a binary beside the test
//!
//! `no_commit_moves_a_doc_onto_another_item` judges ONE commit — the one under test — because that
//! is the only commit a gate at the commit has any business refusing. Two questions it cannot put:
//!
//! * **was the predicate right about the history it will now judge?** Its false positives are a
//!   count over every commit this repository has, taken once before it became a gate and again
//!   whenever it changes;
//! * **what did a PUSH carry?** A range of several commits is judged here in one call.
//!
//! ⚠ Commits come from the arguments, or one per line on standard input when there are none — so
//! `git rev-list` feeds it without an argument list the shell has to hold.

use sprag_gate::doc_attachment::{judge_between, judge_commit, stands_at};
use std::io::BufRead;

fn main() -> std::process::ExitCode {
    let mut commits: Vec<String> = std::env::args().skip(1).collect();
    // ⚠ `--standing-at <rev>` FIRST, when given: every displacement found is also asked whether it
    // still stands at that commit — the census's second question, put by the same reader.
    let standing_at = match commits.first().map(String::as_str) {
        Some("--standing-at") => {
            if commits.len() < 2 {
                eprintln!("doc-attachment: --standing-at needs the commit to ask about");
                return std::process::ExitCode::FAILURE;
            }
            let rev = commits.remove(1);
            commits.remove(0);
            Some(rev)
        }
        _ => None,
    };
    if commits.is_empty() {
        for line in std::io::stdin().lock().lines() {
            match line {
                Ok(line) if !line.trim().is_empty() => commits.push(line.trim().to_owned()),
                Ok(_) => {}
                Err(why) => {
                    eprintln!("doc-attachment: standard input could not be read: {why}");
                    return std::process::ExitCode::FAILURE;
                }
            }
        }
    }
    // ⛔ NOTHING TO JUDGE IS NOT A CLEAN RESULT — an empty `rev-list` and a mistyped range both
    // arrive here as no commits, and neither has said anything about any doc.
    if commits.is_empty() {
        eprintln!(
            "doc-attachment: no commits were named — pass them as arguments or one per line on \
             standard input"
        );
        return std::process::ExitCode::FAILURE;
    }
    let repo = match std::env::current_dir() {
        Ok(repo) => repo,
        Err(why) => {
            eprintln!("doc-attachment: this process cannot say where it is standing: {why}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let mut moved_in = 0;
    let mut displacements = 0;
    let mut unreadable = 0;
    let mut files = 0;
    let mut stands = 0;
    let mut unasked = 0;
    for commit in &commits {
        // ⚠ `A..B` is the NET change between two commits — see `judge_between` for the two
        // questions only a range can put. Anything else names one commit.
        let judged = match commit.split_once("..") {
            Some((base, tip)) => judge_between(&repo, base, tip),
            None => judge_commit(&repo, commit),
        };
        match judged {
            Ok(reading) => {
                files += reading.compared.len();
                if !reading.found.is_empty() {
                    moved_in += 1;
                }
                for (path, moved) in &reading.found {
                    displacements += 1;
                    let standing = match &standing_at {
                        Some(rev) => match stands_at(&repo, rev, path, moved) {
                            Ok(true) => {
                                stands += 1;
                                format!(" [STANDS at {rev}]")
                            }
                            Ok(false) => format!(" [gone at {rev}]"),
                            // ⛔ *COULD NOT ASK* IS COUNTED, never folded into *gone*: a file
                            // renamed since cannot say whether the move in it stands.
                            Err(why) => {
                                unasked += 1;
                                format!(" [could not ask at {rev}: {why}]")
                            }
                        },
                        None => String::new(),
                    };
                    println!(
                        "{} {path}:{} — the doc written for `{}` now documents `{}`: \"{}\"{standing}",
                        &reading.tip[..reading.tip.len().min(8)],
                        moved.line,
                        moved.was,
                        moved.now,
                        moved.opening,
                    );
                }
            }
            Err(why) => {
                unreadable += 1;
                eprintln!("doc-attachment: {commit} could not be judged: {why}");
            }
        }
    }
    println!(
        "doc-attachment: {} commit(s) judged, {files} Rust file version pair(s) compared, \
         {moved_in} commit(s) moved a doc ({displacements} displacement(s)), {unreadable} could \
         not be judged",
        commits.len(),
    );
    if let Some(rev) = &standing_at {
        println!(
            "doc-attachment: at {rev}, {stands} of those {displacements} displacement(s) still \
             stand and {unasked} could not be asked"
        );
    }
    if moved_in == 0 && unreadable == 0 {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}
