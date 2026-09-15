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
//! # ⛔⛔⛔⛔⛔ And the list is priced from BOTH ends — register item 1106
//!
//! Item 1104 put a floor under the list: it must cover 80% of the paths the window changed. That
//! threshold is satisfied by naming MORE, its refusal ranks the directories to add, and **nothing
//! measured what adding one costs** — so the one instrument pointed at this clause pushed it in a
//! single direction for eleven days. Item 1106 priced where that direction ends: nine directories
//! holding 224 of 382 tracked files, and this product's own 600-second bound run out twice by real
//! milestone checks handed 58% of a tree to search.
//!
//! So the floor now has a ceiling beside it, and the two squeeze the list from opposite sides. A
//! list that cannot satisfy both is not a number to widen — it is the kind document saying its work
//! no longer has a stable shape, which is a finding rather than a threshold to move.
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
/// ⛔⛔⛔⛔⛔ **AND THIS GATE NOW RE-RUNS IT** — register item 1104, which is what the sentence
/// this doc used to carry cost.
///
/// It used to end: *"This gate does not re-run that command … a percentage measured on a moving
/// window is a number to re-take by hand."* **Nobody re-took it.** Measured 2026-09-14, the window
/// had moved from 702 changed paths to 612, the five named directories had fallen from the claimed
/// 80.1% to **73.4%**, and one of them no longer cleared the twentieth it was chosen by — while the
/// kind document went on stating the old figure with its own authority. A number a document asserts
/// and no gate re-takes is this workspace's rule 10 met in a comment: **prose nobody measures.**
///
/// ⚠⚠ The threshold asserted is the COVERAGE, not the per-directory heuristic that picked the
/// list. *A twentieth each* was a way of guessing at what matters and it guesses badly both ways —
/// it drops `crates/sprag-mcp/src` (one tracked file, 11 changed paths) and would admit
/// `crates/sprag-gui/src` (34 files, 10). What a checker needs is that the round it judges landed
/// somewhere it was told to open, and coverage says exactly that.
const COVERAGE_FLOOR_PERCENT: usize = 80;

/// ⛔⛔⛔⛔⛔ **HOW MUCH OF THE TREE THE LIST MAY BE** — register item 1106, and the force
/// [`COVERAGE_FLOOR_PERCENT`] has never had anything pushing back against.
///
/// # ⚠⚠⚠⚠⚠ The floor above is one-sided, and its own remedy walks into item 1106
///
/// That floor is satisfied by naming MORE, and its refusal ranks the biggest uncovered directories
/// so the next reader knows which to add. Nothing anywhere asks what adding one costs. **Measured
/// 2026-09-15**: coverage stands at **81%** against a floor of 80 — one point — and when the window
/// next drifts under it that refusal names `crates/sprag-terminal/src` first, which takes the list
/// from **58%** of the tree to **65%**; the one after it to **68%**.
///
/// Item 1106 measured what the far end of that walk costs: nine directories holding 224 of 382
/// tracked files, a judge told to open 58% of a tree, and **this product's own 600-second bound run
/// out twice** on the shape it was actually being asked in. So the gate that rewards a wider list
/// had a gate beside it that priced one, and the price was paid in a bound nobody was watching.
///
/// # ⚠⚠ A RATCHET AT WHERE WE STAND, NOT A BLESSING OF IT
///
/// 58% is not a figure this declares acceptable — item 1106 measured it failing. It is where the
/// list is, and the claim is *it did not grow*, which is the shape `TESTS_SWITCHED_OFF` states one
/// crate over for a population that likewise cannot reach zero by being asked to. The slack to 60
/// absorbs the tree's ordinary growth — twelve files inside the nine, several rounds of it — while
/// any DIRECTORY added jumps at least six points and meets this line.
///
/// ⚠ What relieves the pincer is not a wider list: it is that a round's own changed files are the
/// FIRST tier of the question since item 1106's layer ⑴ (`9b7dbe2d`), so this list is the fallback
/// for a round that has moved nothing rather than the sentence a judge usually meets. A list that
/// cannot cover the history without becoming the tree is that fact arriving as a red.
const BREADTH_CEILING_PERCENT: usize = 60;

/// How far back the coverage is measured — the window the kind document's own derivation names.
const WINDOW: usize = 200;

/// How the list is derived, as a command — the thing an XML comment cannot hold, which is why the
/// kind document points here for it:
///
/// ```text
/// git log -200 --name-only --pretty=format: | grep -v '^$' \
///   | sed 's#\(^[^/]*/[^/]*/[^/]*\).*#\1#' | sort | uniq -c | sort -rn
/// ```
const DERIVATION: &str = "at least 80% of the paths the last 200 commits changed";

/// Every path the last [`WINDOW`] commits changed, as this repository's own VCS answers.
///
/// ⛔ **A READING THAT DID NOT HAPPEN IS A PANIC, NEVER AN EMPTY LIST** — rule 6, and the one
/// failure mode a gate that shells out is most likely to have. An empty answer would make the
/// coverage claim below vacuously satisfiable at 0 named directories.
fn paths_the_window_changed() -> Vec<String> {
    // ⛔⛔ THROUGH `ambient::git_in`, NEVER `Command::new("git")` — register item 965. `pre-commit`
    // runs this suite, so a bare child inherits an ABSOLUTE `GIT_INDEX_FILE` naming the index git
    // is about to commit, and that outranks `current_dir`. This constructor cuts it.
    let out = sprag_gate::ambient::git_in(&workspace_root())
        .args([
            "log",
            &format!("-{WINDOW}"),
            "--name-only",
            "--pretty=format:",
        ])
        .output()
        .unwrap_or_else(|why| {
            panic!(
                "⛔ REGISTER ITEM 1104: this gate re-derives `{CHECK_OPENS}` from the history and \
                 could not run git: {why}. It must not pass here — an unreadable instrument \
                 answering *clean* is the one conclusion rule 6 forbids"
            )
        });
    assert!(
        out.status.success(),
        "⛔ REGISTER ITEM 1104: `git log -{WINDOW}` failed in {}: {}",
        workspace_root().display(),
        String::from_utf8_lossy(&out.stderr),
    );
    let paths: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect();
    assert!(
        !paths.is_empty(),
        "⛔ REGISTER ITEM 1104: the last {WINDOW} commits changed no paths, which cannot be true of \
         a tree this gate is running inside — a shallow clone answers this way, and the coverage \
         claim would then be satisfied by naming nothing",
    );
    paths
}

/// Every file this repository tracks, as its own VCS answers.
///
/// ⛔ **A READING THAT DID NOT HAPPEN IS A PANIC, NEVER AN EMPTY LIST** —
/// [`paths_the_window_changed`]'s rule, for the same reason one step over: an empty tree would make
/// the breadth claim below divide by zero, and any guard against that would read as clean.
fn files_this_tree_tracks() -> Vec<String> {
    // ⛔⛔ THROUGH `ambient::git_in`, NEVER `Command::new("git")` — register item 965, and the
    // hazard is sharper for THIS reading than for the window's: `pre-commit` runs this suite with
    // an absolute `GIT_INDEX_FILE` naming the index being committed, and `ls-files` reads the
    // index. A bare child would answer about that one and call it the tree.
    let out = sprag_gate::ambient::git_in(&workspace_root())
        .args(["ls-files"])
        .output()
        .unwrap_or_else(|why| {
            panic!(
                "⛔ REGISTER ITEM 1106: this gate prices `{CHECK_OPENS}` against the tree and could \
                 not run git: {why}. It must not pass here — an unreadable instrument answering \
                 *clean* is the one conclusion rule 6 forbids"
            )
        });
    assert!(
        out.status.success(),
        "⛔ REGISTER ITEM 1106: `git ls-files` failed in {}: {}",
        workspace_root().display(),
        String::from_utf8_lossy(&out.stderr),
    );
    let tracked: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect();
    assert!(
        !tracked.is_empty(),
        "⛔ REGISTER ITEM 1106: this tree tracks no files, which cannot be true of one this gate is \
         running inside — and a zero population makes *the list is not most of the tree* true of \
         every list there is",
    );
    tracked
}

/// How many of `tracked` sit under one of the `opens` directories.
///
/// ⚠ The same `starts_with(format!("{open}/"))` the coverage claim uses, and deliberately not a
/// looser one: two readers of *is this path inside that directory* free to disagree is this
/// workspace's oldest class of defect, and the pincer only means anything if both jaws measure
/// membership the same way.
fn how_many_are_under(tracked: &[String], opens: &[String]) -> usize {
    tracked
        .iter()
        .filter(|path| {
            opens
                .iter()
                .any(|open| path.starts_with(&format!("{open}/")))
        })
        .count()
}

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

/// ⛔⛔⛔⛔⛔ **A ROUND OF THIS KIND LANDS SOMEWHERE THE CHECKER WAS TOLD TO OPEN** — register item
/// 1104, and the claim the kind document used to make in prose that nothing re-took.
///
/// # ⚠⚠⚠⚠⚠ Why coverage, and why it is measured here rather than asserted there
///
/// Naming files is the one remedy anybody has TIMED for a checker going silent (item 1072: 33.3 s
/// against a 185.2 s worst case). It buys that only for work that lands INSIDE the names — a round
/// whose whole commit fell outside them leaves the judge searching exactly as if the clause were
/// empty, while the document goes on saying *these are where the work lands*. That is worse than
/// silence, for the reason the existence claim above states.
///
/// So the number is re-taken from the history on every run. The window moves under it by
/// construction — that is not noise, it is the fact the old comment could not survive: it asserted
/// **80.1%** and was at **73.4%** two days later, having named the same five directories the whole
/// time.
///
/// ⚠⚠ The floor is a DECLARED number and the reading is a MEASURED one, which is the shape this
/// workspace keeps arriving at (register item 1056, one crate over: compare against something the
/// alternative cannot cross rather than against a figure an afternoon can move). A list that drifts
/// under the floor is a list to re-derive, and the refusal below carries the table to re-derive it
/// from — so the remedy is in the sentence rather than in somebody's memory of a command.
#[test]
fn what_the_checker_is_told_to_open_covers_where_this_kind_actually_works() {
    let opens = named(&document(), CHECK_OPENS);
    let changed = paths_the_window_changed();
    // ⚠ Through the shared reader since register item 1106 gave this claim a counterpart: the two
    // jaws of the pincer have to agree about what *inside a named directory* means, or a list can
    // satisfy one by a definition the other does not use.
    let covered = how_many_are_under(&changed, &opens);
    // ⚠ Integer arithmetic, and the multiplication first: `covered / total * 100` is zero for every
    // input a gate can have.
    let percent = covered * 100 / changed.len();

    // ⚠⚠ THE TABLE IS BUILT WHETHER OR NOT IT IS PRINTED, because a refusal that said only *you are
    // at 73%* would send the next reader back to a shell to find out which directories to add —
    // which is the step the old comment's "re-take it by hand" asked for and nobody took.
    let mut missed: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for path in &changed {
        if opens
            .iter()
            .any(|open| path.starts_with(&format!("{open}/")))
        {
            continue;
        }
        let head: Vec<&str> = path.split('/').take(3).collect();
        *missed.entry(head.join("/")).or_default() += 1;
    }
    let mut ranked: Vec<(&String, &usize)> = missed.iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));

    assert!(
        percent >= COVERAGE_FLOOR_PERCENT,
        "⛔⛔⛔⛔⛔ REGISTER ITEM 1104: this kind tells its checker to open {} \
         director(ies), and they hold only {percent}% of the {} paths the last {WINDOW} commits \
         changed — the floor is {COVERAGE_FLOOR_PERCENT}%. A round landing outside them leaves the \
         judge searching exactly as if the clause were empty, while the document says *this is \
         where the work lands*. Derivation: {DERIVATION}. Biggest uncovered, most changed first: \
         {:?}",
        opens.len(),
        changed.len(),
        ranked.iter().take(8).collect::<Vec<_>>(),
    );
}

/// ⛔⛔⛔⛔⛔ **AND IT IS NOT MOST OF THE TREE** — register item 1106, and the jaw the claim above
/// spent eleven days without.
///
/// # ⚠⚠⚠⚠⚠ What a one-sided threshold does, measured rather than supposed
///
/// `what_the_checker_is_told_to_open_covers_where_this_kind_actually_works` is satisfied by naming
/// MORE, and it hands the next reader a ranked list of directories to add. There was no reading
/// anywhere of what adding one costs — so the only instrument pointed at this clause pushed it in
/// exactly one direction, and the direction ends at *open the whole repository*.
///
/// **That end was reached and priced.** Item 1106: nine directories, 224 of 382 tracked files, and
/// the product's own 600-second bound run out twice by real milestone checks asked that way. The
/// remedy item 1072 switched on — name the files, 33.3 s against a 185.2 s worst case — was being
/// fed a search, and every gate in this repository was green throughout, this file's included.
///
/// # ⛔⛔⛔ Why a ceiling and not an equality
///
/// An exact count would go red on ordinary growth: this tree gains files most rounds and most of
/// them land inside the nine, so the gate would fire on work that never touched the list and the
/// reader would learn to retype the number without reading it. The ceiling is about what the LIST
/// chooses. A directory added jumps at least six points ([`BREADTH_CEILING_PERCENT`] carries the
/// two candidates and their arithmetic); ordinary growth needs a dozen files to move it one.
///
/// # ⚠⚠ And rule 5: both jaws are satisfiable today, at 81% and 58%
///
/// A pincer whose two numbers could not hold at once would be a gate demanding a list that cannot
/// exist. It can: this is what the reading is as this is written. What the pincer refuses is the
/// silent walk between them — and if a window ever moves so far that no list satisfies both, that
/// is the honest finding item 1106 would want on the table rather than another six points spent
/// quietly.
#[test]
fn what_the_checker_is_told_to_open_is_not_most_of_the_tree() {
    let opens = named(&document(), CHECK_OPENS);
    let tracked = files_this_tree_tracks();
    let under = how_many_are_under(&tracked, &opens);
    // ⚠ Integer arithmetic, multiplication first — the coverage claim's own note, for the same
    // reason: `under / tracked * 100` is zero for every input a gate can have.
    let percent = under * 100 / tracked.len();

    // ⚠⚠ THE TABLE IS BUILT WHETHER OR NOT IT IS PRINTED — the coverage claim's rule. A refusal
    // saying only *you are at 66%* sends the reader to a shell to find out which entry is the
    // expensive one, which is the step nobody takes.
    let mut costs: Vec<(usize, &String)> = opens
        .iter()
        .map(|open| {
            (
                tracked
                    .iter()
                    .filter(|path| path.starts_with(&format!("{open}/")))
                    .count(),
                open,
            )
        })
        .collect();
    costs.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));

    assert!(
        percent <= BREADTH_CEILING_PERCENT,
        "⛔⛔⛔⛔⛔ REGISTER ITEM 1106: this kind tells its checker to open {} director(ies), and \
         they hold {under} of the {} files this tree tracks — {percent}%, over the \
         {BREADTH_CEILING_PERCENT}% ceiling. A judge told to open most of a tree is SEARCHING, \
         which is the state that ran this product's 600-second bound out twice while the coverage \
         floor beside this one stayed green. ⛔ The remedy is NOT to raise this number: the list \
         is a fallback for a round that has moved nothing, and a round's own files are the first \
         tier of the question. If the coverage floor cannot be met without crossing this, say so \
         in `{KIND_DOCUMENT}` rather than buying six more points. Most files first: {costs:?}",
        opens.len(),
        tracked.len(),
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
