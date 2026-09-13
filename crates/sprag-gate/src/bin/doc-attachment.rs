//! ⛔⛔⛔⛔⛔ **WHICH COMMITS MOVED A DOC ONTO ANOTHER ITEM** — register item 1088 — **WHICH OF THOSE
//! MOVES STILL STAND, AND PUTTING THEM BACK** — register item 1091.
//!
//! ```text
//! doc-attachment <commit>...
//! git rev-list --no-merges HEAD | doc-attachment
//! git rev-list --no-merges HEAD | doc-attachment --standing-at HEAD
//! git rev-list --no-merges HEAD | doc-attachment --standing-at HEAD --repair <path>
//! ```
//!
//! Judges each commit against its first parent with [`sprag_gate::doc_attachment::judge_commit`],
//! in the repository this is started in. One line per displacement, one summary.
//!
//! # ⚠⚠ The exit code answers the question that was asked
//!
//! * **Alone**: 1 when any commit moved a doc or could not be judged — what a range carries.
//! * **`--standing-at <rev>`**: 1 when any move found still stands ANYWHERE in `<rev>`, could not be
//!   asked, or came from a commit that could not be judged — the census. Fed the whole history, 0 is
//!   register item 1091's done-when, and the summary's second line says the same in words.
//! * **`--repair <path>`**, beside `--standing-at`: every move standing in `<path>` at `<rev>` is put
//!   back in the WORKING-TREE file by [`sprag_gate::doc_attachment::repaired`], and the exit is 1
//!   when any was refused. ⚠ The file as it is on disk: an edit already there is kept, and a move put
//!   back by hand already is reported as back rather than refused.
//!
//! # ⚠⚠ Why a binary beside the test
//!
//! `no_commit_moves_a_doc_onto_another_item` judges ONE commit — the one under test — because that
//! is the only commit a gate at the commit has any business refusing. Three questions it cannot put:
//!
//! * **was the predicate right about the history it will now judge?** Its false positives are a
//!   count over every commit this repository has, taken once before it became a gate and again
//!   whenever it changes;
//! * **what did a PUSH carry?** A range of several commits is judged here in one call;
//! * **what is still in the tree?** A move made before the gate existed was never refused.
//!
//! ⚠ Commits come from the arguments, or one per line on standard input when there are none — so
//! `git rev-list` feeds it without an argument list the shell has to hold.

use sprag_gate::doc_attachment::{
    Displacement, Docs, TreeAt, judge_between, judge_commit, repaired,
};
use std::io::BufRead;
use std::path::Path;
use std::process::ExitCode;

/// What the arguments asked for.
struct Asked {
    /// The commit every move found is asked about, when the census was asked for.
    standing_at: Option<String>,
    /// The working-tree file whose standing moves are to be put back.
    repair: Option<String>,
    /// The commits named on the command line; standard input is read when there are none.
    commits: Vec<String>,
}

/// The arguments, read: the flags first, in any order, then the commits.
fn asked(args: impl Iterator<Item = String>) -> Result<Asked, String> {
    let mut args = args.peekable();
    let mut standing_at = None;
    let mut repair = None;
    loop {
        match args.peek().map(String::as_str) {
            Some("--standing-at") => {
                args.next();
                standing_at = Some(
                    args.next()
                        .ok_or_else(|| "--standing-at needs the commit to ask about".to_owned())?,
                );
            }
            Some("--repair") => {
                args.next();
                repair =
                    Some(args.next().ok_or_else(|| {
                        "--repair needs the file to put moves back in".to_owned()
                    })?);
            }
            _ => break,
        }
    }
    // ⛔ A repair of *every move found* would put back moves that no longer stand — the question
    // *standing where* is what picks the ones to put back.
    if repair.is_some() && standing_at.is_none() {
        return Err(
            "--repair puts back the moves standing at a commit, so it needs --standing-at <rev>"
                .to_owned(),
        );
    }
    Ok(Asked {
        standing_at,
        repair,
        commits: args.collect(),
    })
}

fn main() -> ExitCode {
    let mut asked = match asked(std::env::args().skip(1)) {
        Ok(asked) => asked,
        Err(why) => {
            eprintln!("doc-attachment: {why}");
            return ExitCode::FAILURE;
        }
    };
    if asked.commits.is_empty() {
        for line in std::io::stdin().lock().lines() {
            match line {
                Ok(line) if !line.trim().is_empty() => asked.commits.push(line.trim().to_owned()),
                Ok(_) => {}
                Err(why) => {
                    eprintln!("doc-attachment: standard input could not be read: {why}");
                    return ExitCode::FAILURE;
                }
            }
        }
    }
    // ⛔ NOTHING TO JUDGE IS NOT A CLEAN RESULT — an empty `rev-list` and a mistyped range both
    // arrive here as no commits, and neither has said anything about any doc.
    if asked.commits.is_empty() {
        eprintln!(
            "doc-attachment: no commits were named — pass them as arguments or one per line on \
             standard input"
        );
        return ExitCode::FAILURE;
    }
    let repo = match std::env::current_dir() {
        Ok(repo) => repo,
        Err(why) => {
            eprintln!("doc-attachment: this process cannot say where it is standing: {why}");
            return ExitCode::FAILURE;
        }
    };
    let tree = match asked
        .standing_at
        .as_deref()
        .map(|rev| TreeAt::read(&repo, rev))
    {
        None => None,
        Some(Ok(tree)) => Some(tree),
        Some(Err(why)) => {
            eprintln!("doc-attachment: the tree to ask could not be read: {why}");
            return ExitCode::FAILURE;
        }
    };
    let mut moved_in = 0;
    let mut displacements = 0;
    let mut unreadable = 0;
    let mut files = 0;
    let mut stands = 0;
    let mut unasked = 0;
    let mut to_put_back: Vec<Displacement> = Vec::new();
    for commit in &asked.commits {
        // ⚠ `A..B` is the NET change between two commits — see `judge_between` for the two
        // questions only a range can put. Anything else names one commit.
        let judged = match commit.split_once("..") {
            Some((base, tip)) => judge_between(&repo, base, tip),
            None => judge_commit(&repo, commit),
        };
        let reading = match judged {
            Ok(reading) => reading,
            Err(why) => {
                unreadable += 1;
                eprintln!("doc-attachment: {commit} could not be judged: {why}");
                continue;
            }
        };
        files += reading.compared.len();
        if !reading.found.is_empty() {
            moved_in += 1;
        }
        for (path, moved) in &reading.found {
            displacements += 1;
            let standing = match &tree {
                None => String::new(),
                Some(tree) => match tree.standing(moved) {
                    Ok(places) if places.is_empty() => format!(" [gone at {}]", short(&tree.rev)),
                    Ok(places) => {
                        stands += 1;
                        if places
                            .iter()
                            .any(|(at, _)| asked.repair.as_deref() == Some(*at))
                        {
                            to_put_back.push(moved.clone());
                        }
                        let told: Vec<String> = places
                            .iter()
                            .map(|(at, place)| {
                                format!(
                                    "{at}: carried by line(s) {:?}, written for line(s) {:?}",
                                    place.carried_by,
                                    place
                                        .written_for
                                        .iter()
                                        .map(|bare| bare.line)
                                        .collect::<Vec<_>>(),
                                )
                            })
                            .collect();
                        format!(" [STANDS at {} in {}]", short(&tree.rev), told.join("; "))
                    }
                    // ⛔ *COULD NOT ASK* IS COUNTED, never folded into *gone*: a file holding the
                    // moved prose that cannot be read is exactly the one that could say *stands*.
                    Err(why) => {
                        unasked += 1;
                        format!(" [could not ask at {}: {why}]", short(&tree.rev))
                    }
                },
            };
            println!(
                "{} {path}:{} — the doc written for `{}` now documents `{}`: \"{}\"{standing}",
                short(&reading.tip),
                moved.line,
                moved.was,
                moved.now,
                moved.opening(),
            );
        }
    }
    println!(
        "doc-attachment: {} commit(s) judged, {files} Rust file version pair(s) compared, \
         {moved_in} commit(s) moved a doc ({displacements} displacement(s)), {unreadable} could \
         not be judged",
        asked.commits.len(),
    );
    let Some(rev) = &asked.standing_at else {
        return exit(moved_in == 0 && unreadable == 0);
    };
    println!(
        "doc-attachment: at {rev}, {stands} of those {displacements} displacement(s) still \
         stand and {unasked} could not be asked"
    );
    let Some(path) = &asked.repair else {
        return exit(stands == 0 && unasked == 0 && unreadable == 0);
    };
    let refused = match put_back(&repo, path, &to_put_back) {
        Ok(refused) => refused,
        Err(why) => {
            eprintln!("doc-attachment: {path}: {why}");
            return ExitCode::FAILURE;
        }
    };
    exit(refused == 0 && unasked == 0 && unreadable == 0)
}

/// Every move in `moves` put back in the working-tree `path`, one after another over the text the
/// last one left, answering how many were refused.
///
/// ⚠ The file is written only when something was put back, and once.
///
/// # Errors
///
/// When the file cannot be read or written.
fn put_back(repo: &Path, path: &str, moves: &[Displacement]) -> Result<usize, String> {
    let file = repo.join(path);
    let mut text = std::fs::read_to_string(&file)
        .map_err(|why| format!("the working-tree file could not be read: {why}"))?;
    let (mut back, mut refused, mut already) = (0, 0, 0);
    for moved in moves {
        let stands = match Docs::read(&text) {
            Ok(docs) => docs.standing(moved).is_some(),
            Err(why) => {
                return Err(format!(
                    "the working-tree file cannot be read for its docs: {why}"
                ));
            }
        };
        // ⚠ The same move found by two commits, or put back by hand already: nothing to do, and
        // said so rather than counted as either outcome.
        if !stands {
            already += 1;
            println!(
                "doc-attachment: {path}: no longer stands in the working tree — `{}` from `{}`: \"{}\"",
                moved.was,
                moved.now,
                moved.opening(),
            );
            continue;
        }
        match repaired(&text, moved) {
            Ok(done) => {
                text = done;
                back += 1;
                println!(
                    "doc-attachment: {path}: put back on `{}`, off `{}`: \"{}\"",
                    moved.was,
                    moved.now,
                    moved.opening(),
                );
            }
            Err(why) => {
                refused += 1;
                println!("doc-attachment: {path}: REFUSED — {why}");
            }
        }
    }
    if back > 0 {
        std::fs::write(&file, &text)
            .map_err(|why| format!("the working-tree file could not be written: {why}"))?;
    }
    println!(
        "doc-attachment: {path}: {back} put back, {refused} refused, {already} no longer standing"
    );
    Ok(refused)
}

/// A commit id cut to the eight characters a reader matches against `git log --oneline`.
fn short(sha: &str) -> &str {
    &sha[..sha.len().min(8)]
}

/// The exit code for `clean`.
fn exit(clean: bool) -> ExitCode {
    if clean {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
