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
//! * **In CI**, `HEAD` is the pushed tip, and both jobs that run this crate check out one commit of
//!   history behind it.
//! * **By hand**, `HEAD` is the last commit.
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
    Bare, ChangeReading, Displacement, Standing, TreeAt, judge_between, judge_commit,
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

#[test]
fn the_commit_under_test_leaves_every_doc_on_the_item_it_was_written_for() {
    let reading = the_change_under_test(&workspace_root(), "HEAD").unwrap_or_else(|why| {
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
    let told: Vec<String> = reading
        .found
        .iter()
        .map(|(path, moved)| {
            format!(
                "{path}:{} — the doc written for `{}` now documents `{}`: \"{}\"",
                moved.line,
                moved.was,
                moved.now,
                moved.opening(),
            )
        })
        .collect();
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
