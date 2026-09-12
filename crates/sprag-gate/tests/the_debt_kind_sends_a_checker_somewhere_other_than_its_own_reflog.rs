//! ⛔⛔⛔⛔⛔ **THE FILES THIS REPOSITORY'S KIND TELLS A CHECKER TO OPEN ARE REAL, AND THEY ARE NOT
//! THE STALL CEILING'S MARKS** — register item 1074.
//!
//! # The defect, measured the day item 1072 landed
//!
//! Item 1072 put the one remedy anybody has TIMED for a checker going silent into the product — the
//! question naming the files to open rather than handing a judge a directory, **33.3 s under four
//! times the load that produced a 185.2 s worst case** — and fed it from `progress_marks`, because
//! a document had no other list. Run this repository's own kind through the driver's reduction and
//! the checker was handed exactly one path: the register is outside the tree and may never be named
//! to a judge standing in a copy, leaving `.git/logs/HEAD`. **A reflog says a commit happened and
//! nothing whatever about what was fixed.** The measured arm was switched on with nothing to feed.
//!
//! Item 1074 gave the checker its own clause (`check_opens`). This gate is what keeps the two lists
//! from collapsing back into one, and what refuses a name in it that has stopped existing.
//!
//! # ⚠⚠⚠⚠⚠ Why the tree, and why this crate
//!
//! Every claim here is about paths ON DISK — *does the thing a judge is sent to open exist?* — so
//! the input is the TREE and the test belongs in the lane register item 784 built for those: an
//! unrelated crate's rename is exactly what turns it red, and the commit hook runs this crate's
//! suite for that reason. The product's own reader would need the script engine and the compiled
//! machine; this crate declares no dependencies on purpose, so it reads the document as text
//! through [`sprag_gate::loop_shape::authored_list`].
//!
//! ⚠⚠ **A GATE THAT CANNOT SEE MUST NOT READ AS CLEAN.** Each claim below refuses an EMPTY or
//! ABSENT reading by name rather than passing over it, which is this crate's standing rule (item
//! 482) and the one failure a text scan is most likely to have.

use std::collections::BTreeSet;
use std::path::Path;

use sprag_gate::classifier::KIND_DOCUMENT;
use sprag_gate::loop_shape::authored_list;
use sprag_gate::sources::workspace_root;

/// The clause register item 1074 gave the checker.
const CHECK_OPENS: &str = "check_opens";

/// The stall ceiling's clause — register item 942, and the one `check_opens` was carved out of.
const PROGRESS_MARKS: &str = "progress_marks";

/// **HOW THE LIST BELOW WAS DERIVED, AS A COMMAND** — the thing an XML comment cannot hold, which
/// is why the kind document points here for it:
///
/// ```text
/// git log -200 --name-only --pretty=format: | grep -v '^$' \
///   | sed 's#\(^[^/]*/[^/]*/[^/]*\).*#\1#' | sort | uniq -c | sort -rn
/// ```
///
/// Over the 200 commits before 2026-09-12 that produced 702 changed paths, and the document names
/// every directory taking at least **a twentieth** of them and nothing else. The five held 80.1% of
/// the changed paths while being 196 of the tree's 367 tracked files.
///
/// ⚠ This gate does not re-run that command. What it holds is the part that ROTS — a named
/// directory that stopped existing, and the two lists collapsing back into one — because a
/// percentage measured on a moving window is a number to re-take by hand, while a dead path is a
/// checker being sent somewhere there is nothing to read.
const DERIVATION: &str = "at least a twentieth of the paths 200 commits changed";

fn document() -> String {
    let path = workspace_root().join(KIND_DOCUMENT);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|why| panic!("{} is this repository's own kind: {why}", path.display()))
}

fn named(scxml: &str, id: &str) -> Vec<String> {
    authored_list(scxml, id).unwrap_or_else(|| {
        panic!(
            "⚠⚠⚠⚠⚠ {KIND_DOCUMENT} DECLARES NO `{id}`, so every claim below would be about an \
             empty list — and a probe pointed at nothing reads exactly like a clean tree. Either \
             the id was removed, in which case say so here, or this reader stopped seeing the \
             document's `<data>` shape"
        )
    })
}

/// ⛔⛔⛔⛔⛔ **A JUDGE IS SENT TO SOMETHING THAT EXISTS.**
///
/// ⚠⚠ The whole point of naming files is that a judge opens them instead of searching. A name that
/// has stopped existing is worse than naming nothing: the sentence still says *these are where to
/// look*, so a judge that finds nothing there has been told, with the document's own authority,
/// that there is nothing to look at.
#[test]
fn every_path_the_kind_sends_a_checker_to_is_one_this_tree_has() {
    let root = workspace_root();
    let opens = named(&document(), CHECK_OPENS);
    assert!(
        !opens.is_empty(),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 1074: this kind declares `{CHECK_OPENS}` and names nothing in it, \
         so its checker falls back to the stall ceiling's marks — which for this document reduce \
         to `.git/logs/HEAD` alone. The measured arm is then switched on with a reflog to feed it. \
         The list is derived as {DERIVATION}",
    );
    let gone: Vec<&String> = opens
        .iter()
        .filter(|path| !root.join(Path::new(path)).exists())
        .collect();
    assert!(
        gone.is_empty(),
        "⛔⛔⛔⛔⛔ A CHECKER IS BEING SENT SOMEWHERE THIS TREE DOES NOT HAVE: {gone:?}. The clause \
         says *its work lands in these* with the document's own authority, so a dead name tells a \
         judge there is nothing to look at rather than that the list is stale. Re-derive it — \
         {DERIVATION} — and write the new list into `{KIND_DOCUMENT}`",
    );
}

/// ⛔⛔⛔⛔⛔ **THE TWO CLAUSES HAVE NOT COLLAPSED BACK INTO ONE** — the whole of register item 1074.
///
/// # ⚠⚠⚠⚠⚠ Why this is a gate and not a note
///
/// The state item 1074 was filed on is not *the lists were equal*; it is that there was only ONE
/// list and two readers of it. The way that state comes back is for somebody to make the clauses
/// agree — copying the marks into `check_opens`, or adding a source directory to `progress_marks`
/// so the checker can see it — and both readings are then wrong at once: the judge gets a reflog,
/// and the only ceiling in `Ceiling::ALL` that measures PROGRESS gets a term per source directory,
/// which makes its joined reading move more often and its bound bite later.
///
/// ⚠⚠ **DISJOINT, NOT MERELY UNEQUAL.** One shared entry is enough to be the defect: it is a path
/// that has to serve both questions, and the axes the item measured say no path does both well.
#[test]
fn what_the_checker_opens_and_what_the_ceiling_watches_are_different_lists() {
    let scxml = document();
    let opens: BTreeSet<String> = named(&scxml, CHECK_OPENS).into_iter().collect();
    let marks: BTreeSet<String> = named(&scxml, PROGRESS_MARKS).into_iter().collect();
    assert!(
        !marks.is_empty(),
        "⚠⚠⚠ THE PREMISE OF THE CLAIM BELOW: this kind must still name marks, or *the two lists \
         differ* is true of a document that simply has one list again — which is the state item \
         1074 was filed on, wearing the other face",
    );
    let both: Vec<&String> = opens.intersection(&marks).collect();
    assert!(
        both.is_empty(),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 1074: {both:?} is written in BOTH clauses, so one path is \
         answering two questions that disagree. A stall mark wants to be few, cheap to `stat` and \
         reliably moving — a reflog is ideal and tells a judge nothing; a checker's path wants the \
         work in it. And the direction of the damage is quiet: every entry shared with \
         `{PROGRESS_MARKS}` is one more term in the ceiling's joined reading, so it moves more \
         often and the bound fires LATER than its own derivation says",
    );
}
