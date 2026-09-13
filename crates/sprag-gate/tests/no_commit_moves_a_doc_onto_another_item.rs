//! ⛔⛔⛔⛔⛔ **THE COMMIT UNDER TEST LEAVES EVERY DOC ON THE ITEM IT WAS WRITTEN FOR** — register
//! item 1088.
//!
//! An item declared between a `///` block and the item that block documents takes the block, and
//! nothing in a single tree can say so — see [`sprag_gate::doc_attachment`] for the three readings
//! of the tree that were measured and refused, and for the two displacements that landed here
//! regardless. So this judges the CHANGE: `HEAD` against its first parent.
//!
//! # ⚠⚠⚠ Why `HEAD` is the commit being made, wherever this runs
//!
//! * **In the commit hook**, the ratchet lane runs this crate inside a checkout of the INDEX, and
//!   `index_mirror` in `.githooks/content-gate.sh` writes that checkout's commit ON the real `HEAD`.
//!   So `HEAD` there is the commit about to land, and its parent is what it lands on.
//! * **In CI**, `HEAD` is the pushed tip, and both jobs that run this crate check out the WHOLE
//!   history behind it — see the next section for why one commit of it is no longer enough.
//! * **By hand**, `HEAD` is the last commit.
//!
//! # ⛔⛔⛔ A move that puts an earlier move back is not refused, and history says which it is
//!
//! Putting back a block that was the whole doc of the item that took it reads, to a reader of one
//! change, exactly like the move it undoes — register item 1091 found six such repairs refused by this
//! gate. So each move found is asked of history ([`sprag_gate::doc_attachment::undone_in_history`]):
//! only a move that carries a block back onto the item an earlier commit took it off is let through.
//! A shallow clone cannot answer that, and is refused for it rather than guessed at.
//!
//! # ⛔⛔⛔ A `HEAD` WITH NO PARENT IS REFUSED, NOT PASSED
//!
//! This repository has no root commit anywhere a suite runs. A parentless `HEAD` means the hook's
//! scaffolding commit was written without the commit it is built on, and judged as a root it would
//! compare nothing and pass every commit from then on — the gate switched off by its own plumbing.
//!
//! # ⚠ THE RESIDUE, STATED RATHER THAN HIDDEN
//!
//! `git commit --amend` is judged against the commit it replaces, not against that commit's parent,
//! so a displacement the amended commit already carried is not judged again. It was judged when it
//! was first committed.

use sprag_gate::doc_attachment::{
    Bare, ChangeReading, Displacement, Standing, TreeAt, census, judge_between, judge_commit,
    undone_in_history,
};
use sprag_gate::sources::workspace_root;
use std::path::{Path, PathBuf};

/// The commit `rev` names in `repo`, judged against its first parent — and REFUSED when it has none.
///
/// ⛔⛔⛔ A separate function so the refusal is an arm a case can drive: inlined into the gate below,
/// it could only go red inside a parentless mirror, and no mutation of it could ever be seen to.
fn the_change_under_test(repo: &Path, rev: &str) -> Result<ChangeReading, String> {
    let reading = judge_commit(repo, rev)?;
    if reading.base.is_none() {
        return Err(format!(
            "{} has no parent, so what it changed cannot be read. In the commit hook this is the \
             index mirror's scaffolding commit written without the HEAD it stands on — \
             `index_mirror` in `.githooks/content-gate.sh` gives it that parent. Judged as a root \
             commit it would compare nothing and pass",
            reading.tip,
        ));
    }
    Ok(reading)
}

/// Every move `reading` found that puts no earlier move back, as the line a refusal prints — the gate's
/// decision, register items 1088 and 1091.
///
/// ⛔⛔ A separate function for [`the_change_under_test`]'s reason: inlined into the gate, the history
/// arm could only be driven by a real repair landing, and no mutation of it could be seen to go red.
/// A move that history cannot be asked about is told, never let through.
fn unexplained(repo: &Path, reading: &ChangeReading) -> Vec<String> {
    let before = reading.base.as_deref().unwrap_or_default();
    reading
        .found
        .iter()
        .filter_map(|(path, moved)| {
            match undone_in_history(repo, before, path, moved) {
                Ok(Some(earlier)) => {
                    // ⚠ Printed on a green run: a move let through is a claim, and says which commit
                    // it answers to.
                    eprintln!(
                        "doc attachment: {path}: `{}` gets back the doc {earlier} took from it: \"{}\"",
                        moved.now,
                        moved.opening(),
                    );
                    None
                }
                Ok(None) => Some(format!(
                    "{path}:{} — the doc written for `{}` now documents `{}`: \"{}\"",
                    moved.line,
                    moved.was,
                    moved.now,
                    moved.opening(),
                )),
                Err(why) => Some(format!(
                    "{path}:{} — the doc on `{}` moved to `{}`, and history could not be asked \
                     whether that puts it back: {why}",
                    moved.line, moved.was, moved.now,
                )),
            }
        })
        .collect()
}

#[test]
fn the_commit_under_test_leaves_every_doc_on_the_item_it_was_written_for() {
    let repo = workspace_root();
    let reading = the_change_under_test(&repo, "HEAD").unwrap_or_else(|why| {
        panic!("⛔⛔⛔ REGISTER ITEM 1088: the commit under test could not be judged: {why}")
    });
    let parent = reading.base.as_deref().unwrap_or_default();
    // ⚠ THE POPULATION, printed on a green run too: *compared nothing* and *compared and clean* are
    // different sentences, and a commit that touched no Rust is the first of them.
    eprintln!(
        "doc attachment: {} judged against {parent}: {} Rust file(s) compared",
        reading.tip,
        reading.compared.len(),
    );
    let told = unexplained(&repo, &reading);
    assert!(
        told.is_empty(),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 1088: this commit declared an item between a `///` block and the \
         item that block documents, so the block now documents the new item and its own item has \
         none:\n  {}\nDeclare the new item ABOVE the doc block, never between a block and its item.",
        told.join("\n  "),
    );
}

/// A throwaway repository of this suite's own, under the machine's scratch root and removed again
/// when the case ends.
///
/// ⚠ Its git is cut off from this process's git environment (`ambient::git_in`, register item 965)
/// and from the developer's configuration, so the commits it makes are the fixture's and nothing else.
struct Repository {
    dir: PathBuf,
}

impl Repository {
    fn empty(tail: &str) -> Repository {
        let dir = sprag_scratch::scratch_for("sprag-gate-doc-attachment", tail);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create a scratch repository");
        let repository = Repository { dir };
        repository.git(&["init", "-q", "."]);
        repository.git(&["config", "user.email", "doc-attachment@invalid"]);
        repository.git(&["config", "user.name", "sprag-gate"]);
        repository.git(&["config", "commit.gpgsign", "false"]);
        repository
    }

    /// A clone of `from` at `depth`, through a `file://` URL — the only road on which `--depth` is
    /// honoured for a local repository.
    fn cloned(from: &Repository, depth: &str, tail: &str) -> Repository {
        let dir = sprag_scratch::scratch_for("sprag-gate-doc-attachment", tail);
        let _ = std::fs::remove_dir_all(&dir);
        let url = format!("file://{}", from.dir.display());
        let dest = dir.display().to_string();
        from.git(&["clone", "-q", "--depth", depth, &url, &dest]);
        Repository { dir }
    }

    fn git(&self, args: &[&str]) -> String {
        let run = sprag_gate::ambient::git_in(&self.dir)
            .args(args)
            .env("HOME", &self.dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("git on PATH");
        assert!(
            run.status.success(),
            "git {args:?} refused in {}: {}",
            self.dir.display(),
            String::from_utf8_lossy(&run.stderr),
        );
        String::from_utf8_lossy(&run.stdout).trim().to_owned()
    }

    /// Write `lib.rs` and commit it, answering the commit's id.
    fn commit(&self, text: &str, message: &str) -> String {
        self.commit_files(&[("lib.rs", Some(text))], message)
    }

    /// Write each named file — or remove it, for [`None`] — and commit them together, answering
    /// the commit's id.
    fn commit_files(&self, files: &[(&str, Option<&str>)], message: &str) -> String {
        for (name, text) in files {
            match text {
                Some(text) => {
                    std::fs::write(self.dir.join(name), text).expect("write a fixture file");
                    self.git(&["add", name]);
                }
                None => {
                    self.git(&["rm", "-q", name]);
                }
            }
        }
        self.git(&["commit", "-q", "-m", message]);
        self.git(&["rev-parse", "HEAD"])
    }

    fn path(&self) -> &Path {
        &self.dir
    }
}

impl Drop for Repository {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// ⛔⛔⛔⛔⛔ **A REAL COMMIT THAT DECLARES BETWEEN A DOC AND ITS ITEM IS NAMED, AND ONE THAT DECLARES
/// ABOVE IS NOT** — register item 1088, driven through git rather than handed two strings.
///
/// ⚠⚠ The control is on the SAME parent and differs only in where the new item went, and it states
/// its population: a green answer that compared no file would be the gate saying nothing.
#[test]
fn a_real_commit_that_declares_between_a_doc_and_its_item_is_named() {
    let repository = Repository::empty("commits");
    let base = repository.commit("/// Adds.\npub fn add() {}\n", "a documented function");
    let between = repository.commit(
        "/// Adds.\npub fn mul() {}\npub fn add() {}\n",
        "a function declared between the doc and its function",
    );
    let judged = judge_commit(repository.path(), "HEAD").expect("the commit is readable");
    let moved = Displacement {
        was: "fn add".to_owned(),
        now: "fn mul".to_owned(),
        line: 2,
        doc: vec!["Adds.".to_owned()],
        attributes: Vec::new(),
    };
    assert_eq!(
        (judged.base.as_deref(), judged.found.clone()),
        (
            Some(base.as_str()),
            vec![("lib.rs".to_owned(), moved.clone())]
        ),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 1088: `mul` took `add`'s doc and the commit that did it must be named",
    );
    assert_eq!(
        judge_between(repository.path(), &base, &between)
            .expect("both commits are readable")
            .found,
        vec![("lib.rs".to_owned(), moved)],
        "⚠⚠ and the range over the same two commits answers the same thing — one reader, two doors",
    );

    repository.git(&["reset", "-q", "--hard", &base]);
    repository.commit(
        "pub fn mul() {}\n\n/// Adds.\npub fn add() {}\n",
        "a function declared above the doc",
    );
    let above = judge_commit(repository.path(), "HEAD").expect("the commit is readable");
    assert_eq!(
        (above.compared.clone(), above.found.clone()),
        (vec!["lib.rs".to_owned()], Vec::new()),
        "⚠⚠⚠ THE CONTROL: the same parent, the new function declared ABOVE the doc — compared, and \
         nothing moved",
    );
}

/// ⛔⛔⛔⛔ **A MOVE IS ASKED OF THE WHOLE TREE, SO CODE THAT LEFT ITS FILE STILL ANSWERS FOR IT** —
/// register item 1091, through git.
///
/// ⚠⚠ The shape was measured here before this was written: `809ae70c` moved a doc in
/// `crates/sprag-gui/src/wire.rs`, and `068f5774` deleted that file and carried its code into
/// `crates/sprag-client/src/wire.rs` — a delete and an add, which git does not call a rename. Asked
/// by its path, that move could only ever answer *could not ask*. The control is the same move put
/// back in the file it went to.
#[test]
fn a_move_is_asked_of_the_whole_tree_so_code_that_left_its_file_still_answers_for_it() {
    let repository = Repository::empty("tree");
    repository.commit_files(
        &[("a.rs", Some("/// Adds.\npub fn add() {}\n"))],
        "a documented function",
    );
    let moving = repository.commit_files(
        &[(
            "a.rs",
            Some("/// Adds.\npub fn mul() {}\npub fn add() {}\n"),
        )],
        "a displacement",
    );
    let (path, moved) = judge_commit(repository.path(), &moving)
        .expect("the commit is readable")
        .found
        .remove(0);
    assert_eq!(path, "a.rs", "⚠ the move is made in `a.rs`");
    // ⚠ Enough beside the moved code that git would not call `b.rs` a rename of `a.rs`.
    let beside = "pub fn unrelated() {}\n".repeat(40);
    repository.commit_files(
        &[
            ("a.rs", None),
            (
                "b.rs",
                Some(&format!(
                    "{beside}/// Adds.\npub fn mul() {{}}\npub fn add() {{}}\n"
                )),
            ),
        ],
        "the code leaves its file",
    );
    assert_eq!(
        TreeAt::read(repository.path(), "HEAD")
            .expect("the tree is readable")
            .standing(&moved)
            .expect("the move is asked"),
        vec![(
            "b.rs",
            Standing {
                carried_by: vec![42],
                written_for: vec![Bare {
                    line: 43,
                    attributes: Vec::new(),
                }],
            }
        )],
        "⛔⛔⛔⛔ REGISTER ITEM 1091: `a.rs` is gone, and the move stands in the file its code went to",
    );
    repository.commit_files(
        &[(
            "b.rs",
            Some(&format!(
                "{beside}pub fn mul() {{}}\n/// Adds.\npub fn add() {{}}\n"
            )),
        )],
        "the move put back",
    );
    assert_eq!(
        TreeAt::read(repository.path(), "HEAD")
            .expect("the tree is readable")
            .standing(&moved)
            .expect("the move is asked"),
        Vec::new(),
        "⚠⚠ THE CONTROL: put back in `b.rs`, the move stands nowhere",
    );
}

/// ⛔⛔⛔⛔⛔ **A SHALLOW CLONE CANNOT SAY WHAT ITS TIP CHANGED, AND IS REFUSED RATHER THAN PASSED** —
/// register item 1088, and the reason both CI jobs running this crate check out at depth 2.
///
/// ⚠⚠ The two neighbours are the controls: a clone with one commit behind the tip is judged, and a
/// REAL root commit reads as having no base rather than being refused — this file's gate refuses
/// that one itself, in its own words, because no suite here runs at a root.
#[test]
fn a_shallow_clone_is_refused_and_a_clone_with_history_is_judged() {
    let origin = Repository::empty("origin");
    let root = origin.commit("/// Adds.\npub fn add() {}\n", "the root");
    origin.commit(
        "/// Adds.\npub fn mul() {}\npub fn add() {}\n",
        "a displacement",
    );

    let at_root = judge_commit(origin.path(), &root).expect("a root commit is readable");
    assert_eq!(
        (at_root.base, at_root.compared.len()),
        (None, 0),
        "⚠ a real root has no base and compares nothing — and is not refused, since it is not SHALLOW",
    );
    let refused_root = the_change_under_test(origin.path(), &root).expect_err(
        "⛔⛔⛔ REGISTER ITEM 1088: the gate must refuse a commit with no parent rather than pass it",
    );
    assert!(
        refused_root.contains("no parent"),
        "⛔⛔⛔ and say which of the two it is, so a mirror written without its HEAD reads as that: \
         {refused_root}",
    );

    let shallow = Repository::cloned(&origin, "1", "shallow");
    let refused = judge_commit(shallow.path(), "HEAD").expect_err(
        "⛔ a depth-1 clone's tip reads as having no parent, and must not be judged as a root",
    );
    assert!(
        refused.contains("SHALLOW"),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 1088: the refusal must say why, or a CI job at the default depth would \
         be told something it cannot act on: {refused}",
    );

    let deep = Repository::cloned(&origin, "2", "deep");
    let judged = judge_commit(deep.path(), "HEAD").expect("a depth-2 clone has the parent");
    assert_eq!(
        judged.found.len(),
        1,
        "⚠⚠ THE CONTROL: one commit of history is enough, and the displacement is found through it: \
         {judged:?}",
    );
}

/// ⛔⛔⛔⛔⛔ **A COMMIT THAT PUTS A MOVED DOC BACK IS LET THROUGH, AND A FRESH MOVE IS NOT** — register
/// item 1091, through git.
///
/// ⚠⚠ The repair reads as a move to a reader of one change, which is asserted first: without that, the
/// arm would pass whether or not history was asked. The two controls differ from it in one fact each —
/// no earlier move at all, and an earlier move of a DIFFERENT block onto the same item.
#[test]
fn a_commit_that_puts_a_moved_doc_back_is_let_through_and_a_fresh_move_is_not() {
    let repository = Repository::empty("undo");
    repository.commit("/// Adds.\npub fn add() {}\n", "a documented function");
    let moving = repository.commit(
        "/// Adds.\npub fn mul() {}\n\npub fn add() {}\n",
        "mul declared under add's doc",
    );
    let back = repository.commit(
        "pub fn mul() {}\n\n/// Adds.\npub fn add() {}\n",
        "the doc put back",
    );
    let reading = judge_commit(repository.path(), &back).expect("the repair is readable");
    assert_eq!(
        reading.found.len(),
        1,
        "⛔ to a reader of one change the repair IS a move — the reason history is asked: {reading:?}",
    );
    assert_eq!(
        undone_in_history(repository.path(), &moving, "lib.rs", &reading.found[0].1)
            .expect("the history is whole"),
        Some(moving.clone()),
        "⛔⛔⛔ REGISTER ITEM 1091: the move this puts back is named",
    );
    assert_eq!(
        unexplained(repository.path(), &reading),
        Vec::<String>::new(),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 1091: a repair must not be refused by the gate that exists to stop \
         the move it undoes",
    );

    // ⚠⚠ A move made by MOVING an item that already existed, not by declaring one: the name of the
    // item that took the block is spelled as often before that commit as after it, so the pickaxe
    // pass cannot find it and only the walk over the whole file can.
    let reordered = Repository::empty("reordered");
    reordered.commit(
        "/// Adds.\npub fn add() {}\n\npub fn mul() {}\n",
        "mul below add",
    );
    let moved_up = reordered.commit(
        "/// Adds.\npub fn mul() {}\n\npub fn add() {}\n",
        "mul moved up under add's doc",
    );
    let put_back = reordered.commit(
        "pub fn mul() {}\n\n/// Adds.\npub fn add() {}\n",
        "the doc put back",
    );
    let reading = judge_commit(reordered.path(), &put_back).expect("the repair is readable");
    assert_eq!(
        undone_in_history(reordered.path(), &moved_up, "lib.rs", &reading.found[0].1)
            .expect("the history is whole"),
        Some(moved_up),
        "⚠⚠ a move no name-count can find is still found, by the walk over every commit of the file",
    );

    let fresh = Repository::empty("fresh");
    fresh.commit(
        "pub fn mul() {}\n\n/// Adds.\npub fn add() {}\n",
        "add documented below mul",
    );
    let moved = fresh.commit(
        "/// Adds.\npub fn mul() {}\n\npub fn add() {}\n",
        "the doc moved onto mul",
    );
    let told = unexplained(
        fresh.path(),
        &judge_commit(fresh.path(), &moved).expect("readable"),
    );
    assert_eq!(
        told.len(),
        1,
        "⚠⚠ THE CONTROL: the same move with no earlier one to undo is refused: {told:?}",
    );

    let other = Repository::empty("other-block");
    other.commit(
        "/// Adds.\npub fn add() {}\n\n/// Other.\npub fn other() {}\n",
        "two documented functions",
    );
    other.commit(
        "/// Adds.\npub fn mul() {}\n\npub fn add() {}\n\n/// Other.\npub fn other() {}\n",
        "mul declared under add's doc",
    );
    let onto = other.commit(
        "/// Adds.\npub fn mul() {}\n\n/// Other.\npub fn add() {}\n\npub fn other() {}\n",
        "other's doc moved onto add",
    );
    let told = unexplained(
        other.path(),
        &judge_commit(other.path(), &onto).expect("readable"),
    );
    assert!(
        told.len() == 1 && told[0].contains("Other."),
        "⚠⚠ THE CONTROL: history took a block off `add`, but not THIS block — landing on `add` is \
         not putting it back: {told:?}",
    );
}

/// ⛔⛔⛔⛔ **A SHALLOW CLONE CANNOT SAY A MOVE PUTS ONE BACK, AND IS REFUSED SAYING SO** — register item
/// 1091, and the reason both CI jobs now check out the whole history.
#[test]
fn a_shallow_clone_cannot_let_a_repair_through() {
    let origin = Repository::empty("undo-origin");
    origin.commit("/// Adds.\npub fn add() {}\n", "a documented function");
    origin.commit(
        "/// Adds.\npub fn mul() {}\n\npub fn add() {}\n",
        "mul declared under add's doc",
    );
    origin.commit(
        "pub fn mul() {}\n\n/// Adds.\npub fn add() {}\n",
        "the doc put back",
    );
    let shallow = Repository::cloned(&origin, "2", "undo-shallow");
    let reading = judge_commit(shallow.path(), "HEAD").expect("depth 2 has the parent");
    let told = unexplained(shallow.path(), &reading);
    assert!(
        told.len() == 1 && told[0].contains("SHALLOW"),
        "⛔⛔⛔⛔ REGISTER ITEM 1091: the commit that would say *put back* is not in the clone, and the \
         refusal must say that rather than call the repair a move: {told:?}",
    );
}

/// ⛔⛔⛔⛔ **THE CENSUS DOES NOT COUNT A REPAIR AS STANDING, AND TAKES NO COMMIT FOR THE REPAIR OF A
/// LATER ONE** — register item 1091: without the first the census could never reach zero, and without
/// the second the move itself would be read as the repair of its own repair and never asked about.
#[test]
fn the_census_counts_no_repair_as_standing_and_no_commit_as_repairing_a_later_one() {
    let repository = Repository::empty("census");
    let written = repository.commit("/// Adds.\npub fn add() {}\n", "a documented function");
    let moving = repository.commit(
        "/// Adds.\npub fn mul() {}\n\npub fn add() {}\n",
        "mul declared under add's doc",
    );
    let back = repository.commit(
        "pub fn mul() {}\n\n/// Adds.\npub fn add() {}\n",
        "the doc put back",
    );
    let named = [back.clone(), moving.clone(), written];
    let now = census(repository.path(), &named, Some(&back)).expect("the census is taken");
    assert_eq!(
        (now.found.len(), now.repairs(), now.stands(), now.unasked()),
        (2, 1, 0, 0),
        "⛔⛔⛔⛔ REGISTER ITEM 1091: the move and its repair are both found, and at the repaired tree \
         nothing stands: {now:?}",
    );
    let then = census(repository.path(), &named, Some(&moving)).expect("the census is taken");
    assert_eq!(
        (then.repairs(), then.stands()),
        (1, 1),
        "⚠⚠ THE CONTROL: asked about the tree the move made, the move stands — it is not the repair \
         of the commit that later put it back: {then:?}",
    );
}
