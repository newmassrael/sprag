//! **A WORKING COPY A CHECK CAN BE WRONG IN** — register item 705, and the first place this
//! product knows what a version control system is.
//!
//! # ⛔⛔⛔⛔⛔ What this exists to stop, measured on a live run
//!
//! The independent milestone checker (register item 428) is another agent, spawned in the run's
//! repository since register item 710 and told *the work is in {dir} — OPEN THE FILES THERE*. To
//! decide whether a milestone holds it does what a careful reviewer does: it **mutates the tree**,
//! watches a gate go red, and puts the mutation back. Measured 2026-08-26 on run 0, in a walk that
//! says so in its own words — *"reverting the restore door … reddened both … mutation reverted,
//! tree clean"*.
//!
//! That tree is SHARED. While the checker held it there were three writers, not two: a person, the
//! agent, and the checker — and register item 196 had only ever counted two. Both costs landed the
//! same night:
//!
//! * a watcher read the checker's mutation as the AGENT's leftover and told the owner *"one
//!   sentence of the report is false right now"*. **The agent's report was true.**
//! * that watcher then ran `git checkout --` over it. The checker had already finished, so it was
//!   a no-op **by luck**; a minute earlier it would have destroyed the measurement that decides
//!   whether the milestone was reached.
//!
//! Nothing anywhere said a check was touching the tree — `sprag runs` said `Judging` and no more.
//! **A surface that says nothing is filled in by whoever is looking**, which is why two different
//! watchers reached two different wrong answers about one mutation.
//!
//! # ⚠⚠⚠ Why a `git worktree` rather than a notice
//!
//! A notice is the cheap half and it is not a substitute: it has to be READ, in time, by somebody
//! who is already looking — and the watcher that got this wrong *was* reading `runs`. The repair
//! that does not depend on a habit is for the check to have **nowhere to be wrong except its own
//! copy**, which is what this module hands it.
//!
//! # ⚠⚠ What was measured before any of it was written
//!
//! * **A cold `target/` is affordable.** A worktree gets a `target/` of its own — this
//!   repository's is an untracked symlink to a shared cache, and a fresh checkout carries no such
//!   link — and a cold whole-workspace compile measured **73 s** against
//!   [`CHECK_WITHIN`](crate::plugins)'s 600 s budget, one crate at 46 s. **No persistent
//!   checker-owned build directory is needed**, which is a whole mechanism this module does not
//!   have to carry.
//!
//!   ⚠⚠⚠ **THIS BULLET SAID "A WORKTREE INHERITS NO BUILD CACHE" AND THAT IS FALSE ON THIS
//!   MACHINE — register item 809.** `~/.cargo/config.toml` sets `rustc-wrapper = "sccache"`, and
//!   its own comment states the purpose: *"Share compiled crates across every `target/` directory
//!   on this box."* Measured 2026-09-01: **5341 Rust cache hits**. A separate `target/` is not a
//!   separate BUILD, and the claim above was about the directory while reading as a claim about
//!   isolation. What the timing measurement supports is only that the checkout is affordable; the
//!   isolation half is not this module's to promise, and `sprag_gate::sources::tree_under_test`
//!   now refuses the state where a build from one tree answers for another rather than trusting
//!   this sentence.
//! * **The uncommitted work has to travel**, and it can. A checkout of `HEAD` alone would hand the
//!   checker a DIFFERENT tree from the one the claim is about and it would never know — a silent
//!   wrong answer, which is worse than the defect being repaired. [`IsolatedCheckout::of`] carries
//!   the tracked working-tree changes across, and refuses rather than proceeding when it cannot.
//!
//! # ⚠ The residues, stated rather than discovered later
//!
//! * **Untracked files do not travel.** `git diff HEAD` does not describe them, so a claim resting
//!   on a file the agent has not added is judged against a tree without it. Measured, not assumed.
//! * **`git worktree add` writes under `.git/worktrees/`.** It does not touch the working tree —
//!   the shared tree's own `git status` is unchanged across a whole create/mutate/remove cycle,
//!   which this module's gate asserts — but the repository is not literally untouched.
//! * **A process that dies leaves its checkout behind**, for `git worktree prune` to collect.
//!   [`Drop`] is what removes it in every ordinary ending, including a panic.

use std::path::{Path, PathBuf};
use std::process::Command;

/// **A WORKING COPY OF A REPOSITORY THAT NOBODY ELSE IS STANDING IN** — removed when this value is
/// dropped.
///
/// ⚠⚠⚠ The type is the guarantee. A caller cannot hold the path without holding the thing that
/// cleans it up, which is the arrangement that makes an early return or a panic tidy after itself
/// — and a checkout left behind is not merely litter here: it is a second tree carrying somebody's
/// half-applied mutation, which is the confusion this whole module exists to end.
#[derive(Debug)]
pub struct IsolatedCheckout {
    /// The repository it was cut from — needed at [`Drop`], because `git worktree remove` is a verb
    /// of the ORIGINAL repository and not of the copy.
    repo: PathBuf,
    /// Where the copy is.
    path: PathBuf,
}

impl IsolatedCheckout {
    /// **CUT A WORKING COPY OF `repo` THAT CARRIES WHAT IS UNCOMMITTED IN IT**, or [`None`] where
    /// this cannot be done at all.
    ///
    /// # ⚠⚠⚠⚠⚠ Every failure answers `None`, and that is a decision
    ///
    /// A directory that is not a repository, a `git` that is not installed, a worktree that cannot
    /// be created, a diff that will not apply — each of them means *this check cannot be isolated*,
    /// and the caller's honest response is the same in every case: fall back to the behaviour that
    /// existed before this module and SAY SO. Answering with a half-built copy would be the silent
    /// wrong answer this module was written to prevent, one layer in.
    ///
    /// ⚠⚠ **THE DIFF IS APPLIED OR THE WHOLE THING IS ABANDONED.** A checkout that quietly held
    /// `HEAD` when the claim was about uncommitted work is exactly the failure named in the module
    /// doc: the checker would open real files, judge them carefully, and answer about the wrong
    /// tree. So a refused `apply` tears the checkout down and reports nothing rather than
    /// something.
    ///
    /// ⚠ `HEAD` is DETACHED on purpose: the copy must not take a branch the shared tree is on, or
    /// the two would fight over the same ref the moment either one committed.
    #[must_use]
    pub fn of(repo: &Path, under: &Path) -> Option<Self> {
        // ⛔⛔⛔⛔⛔ **A TEMPORARY ROOT THAT IS NOT ABSOLUTE IS REFUSED** — register item 794. Unlike
        // the check discussed immediately below, this one is LOAD-BEARING, and the difference was
        // measured rather than argued.
        //
        // `RemotePaneAccess::cut` hands this `std::env::temp_dir()`, and its own doc one seam out
        // promises *the temporary root is this machine's, **not a directory of the repository's***
        // — because a copy inside the tree being copied is litter the checker can wander into and
        // shows up in the agent's own `git status`, which is register item 705's confusion
        // re-created by the repair. **That promise rests entirely on the path being absolute**,
        // and `temp_dir()` answers `""` when `TMPDIR` is set-and-empty, which makes the `join`
        // below relative.
        //
        // ⚠⚠ AND NOTHING DOWNSTREAM REFUSES IT, which is the whole reason a check earns its place
        // here. Measured 2026-08-31 in a throwaway repository: `git -C <repo> worktree add
        // --detach -q sprag-check-probe HEAD` **succeeds — rc=0** — and leaves
        // `?? sprag-check-probe/` INSIDE the repository, because git resolves a relative path
        // against its own `-C`. So the silent wrong answer this module exists to prevent would be
        // produced by git doing exactly what it was asked.
        //
        // ⚠ An empty path is not absolute, so the case that motivated this is caught by the same
        // question rather than by a separate emptiness test nobody would keep in step.
        if !under.is_absolute() {
            return None;
        }
        // ⚠⚠⚠⚠⚠ **THERE IS NO «IS THIS A REPOSITORY» CHECK HERE, AND ITS ABSENCE IS MEASURED.**
        // One stood here — a `git rev-parse --git-dir` ahead of everything — and a mutation proved
        // it decoration: with the guard deleted, `a_directory_that_is_no_repository_cannot_be_cut`
        // **stayed green**, because `git worktree add` refuses a directory git knows nothing about
        // by itself and this function already answers `None` on that. Two refusals for one fact is
        // the shape this crate keeps paying for; the one that is load-bearing stays.
        //
        // ⚠ It also asked a subtly different question than the one that matters. *Is there a git
        // dir* is not *can a worktree be cut here* — a bare repository answers yes to the first —
        // so the guard could have been right and useless at the same time.
        let path = under.join(format!(
            "sprag-check-{}-{}",
            std::process::id(),
            // ⚠ The nanosecond keeps two checks in one process from colliding. It is not a clock
            // anything READS — nothing here compares times — so its only property that matters is
            // that it differs, which is why a coarser stamp would be the wrong economy.
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |since| since.subsec_nanos()),
        ));
        if !ran(Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["worktree", "add", "--detach", "-q"])
            .arg(&path)
            .arg("HEAD"))
        {
            return None;
        }
        let cut = Self {
            repo: repo.to_path_buf(),
            path,
        };
        // ⚠⚠⚠ READ FROM THE SHARED TREE AND WRITTEN ONLY INTO THE COPY — which is why this is safe
        // to do while somebody else is working. [`carried_by`] opens files; it changes none.
        match carried_by(repo)? {
            // ⚠ AN EMPTY DIFF IS THE ORDINARY CASE, not a failure: the agent committed before
            // claiming its milestone, which is what the run this item was filed on had done. `git
            // apply` refuses an empty input, so the emptiness is answered here rather than read as
            // a refusal.
            Carried::Nothing => Some(cut),
            Carried::Uncommitted(diff) => applied(&cut.path, &diff).then_some(cut),
        }
    }

    /// Where the copy is — the directory a check should be told to stand in.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for IsolatedCheckout {
    /// ⚠⚠ `--force` because the copy is EXPECTED to be dirty: a check that mutated it and did not
    /// put everything back is the ordinary ending here, and refusing to clean up after exactly that
    /// case would leave the litter this type exists to prevent. Nothing in the copy is anybody's
    /// work — it was cut for one question and answered it.
    fn drop(&mut self) {
        let _ = ran(Command::new("git")
            .arg("-C")
            .arg(&self.repo)
            .args(["worktree", "remove", "--force"])
            .arg(&self.path));
    }
}

/// **WHAT A WORKING TREE IS HOLDING THAT NO COMMIT DOES** — `git diff HEAD`, as a value.
///
/// ⚠⚠ The two readers of this fact want different halves of it and must not become two authors of
/// the question: [`IsolatedCheckout::of`] wants the DIFF, to carry it into the copy, and a run's
/// ending wants only *is there any* — register item 682's commit-contamination clause, where a run
/// that died mid-edit leaves its mutation for the next person to commit by accident.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Carried {
    /// The tree holds nothing a commit does not.
    Nothing,
    /// The tree holds this much, as `git diff HEAD --binary` describes it.
    ///
    /// ⚠ Never empty — an empty diff is [`Nothing`](Self::Nothing), so a caller matching on this
    /// arm cannot be looking at *no changes* spelled a second way.
    Uncommitted(Vec<u8>),
}

impl Carried {
    /// How many bytes the tree is holding — `0` for [`Nothing`](Self::Nothing).
    ///
    /// ⚠ A SIZE and not a file count: `git diff` describes changes, and counting the files in it
    /// would be a second parser of a format this module deliberately does not read.
    #[must_use]
    pub fn bytes(&self) -> usize {
        match self {
            Self::Nothing => 0,
            Self::Uncommitted(diff) => diff.len(),
        }
    }
}

/// **WHAT `repo`'S WORKING TREE IS HOLDING**, or [`None`] where this build cannot say — a directory
/// that is no repository, a `git` that is not installed, a command that failed.
///
/// # ⚠⚠⚠ `None` is *cannot say*, and no caller may fill it in
///
/// Register item 709's discipline. Answering [`Carried::Nothing`] for a tree nobody could read
/// would put the sentence *this run left the tree clean* in a reader's mouth on no evidence — which
/// is the exact shape of the accident item 682's clause is about, arriving from the other side.
#[must_use]
pub fn carried_by(repo: &Path) -> Option<Carried> {
    let read = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["diff", "HEAD", "--binary"])
        .output()
        .ok()?;
    if !read.status.success() {
        return None;
    }
    Some(if read.stdout.is_empty() {
        Carried::Nothing
    } else {
        Carried::Uncommitted(read.stdout)
    })
}

/// Run `command` silently and say whether it succeeded — a spawn that fails and a non-zero exit are
/// one answer here, because both mean *this did not happen*.
fn ran(command: &mut Command) -> bool {
    command
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Feed `patch` to `git apply` inside `at`, and say whether it landed whole.
///
/// ⚠ `--whitespace=nowarn` because a patch cut from a tree and applied to the same tree's `HEAD`
/// carries whatever whitespace that tree has; warning about it would be this function judging the
/// agent's code, which is not its job and not its competence.
///
/// # ⚠⚠⚠⚠⚠ The child may refuse BEFORE it reads — register item 471
///
/// `git apply` can reject a patch and exit while this side is still writing, and a large patch
/// does not fit in a pipe buffer, so the write BLOCKS and then fails with `BrokenPipe`. Read as a
/// failure to feed, that reports the wrong thing entirely: the answer this function came for is
/// the child's EXIT STATUS, which is sitting there waiting to be collected.
///
/// ⚠⚠ **`sprag_gate::feeding::feed` IS THE WRONG DOOR HERE**, for the reason `crate`'s own
/// exemption states one file over: that one PANICS on a write error, and a daemon must not. So the
/// tolerance is carried by name instead — the `BrokenPipe` arm is matched and forgiven, every
/// other error is a failure, and the handle is dropped before the wait so a child that DOES read
/// sees end-of-file rather than a pipe nobody closes.
fn applied(at: &Path, patch: &[u8]) -> bool {
    use std::io::Write as _;

    let Ok(mut child) = Command::new("git")
        .arg("-C")
        .arg(at)
        .args(["apply", "--whitespace=nowarn"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return false;
    };
    let fed = match child.stdin.take() {
        Some(mut pipe) => match pipe.write_all(patch) {
            Ok(()) => true,
            // ⚠ THE CHILD WENT FIRST, which is not this side failing to feed it: `git apply` had
            // already decided, and the decision is the status read below.
            Err(why) if why.kind() == std::io::ErrorKind::BrokenPipe => true,
            Err(_) => false,
        },
        // ⚠ Unreachable with `Stdio::piped()` above, and answered rather than unwrapped: a panic
        // here would take a daemon down over a checkout it could simply decline to make.
        None => false,
    };
    // ⚠⚠ THE HANDLE IS GONE BY NOW — it was moved into the arm above and dropped at its end, which
    // is what closes the pipe. Waiting with it still open is a wait that never returns.
    child.wait().is_ok_and(|status| fed && status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a throwaway repository holding one committed file, plus the two kinds of uncommitted
    /// work an agent can have when its milestone is checked.
    ///
    /// Answers where it is and what its `git diff HEAD` reads, so a caller can watch that stay
    /// still.
    /// ⛔⛔⛔⛔ **`tag` SEPARATES TWO CALLERS IN ONE PROCESS, AND ITS ABSENCE WAS MEASURED.** The
    /// name was `sprag-705-{pid}` alone, which is unique against OTHER processes and identical
    /// between two tests of the SAME one — and these run in parallel, each tearing its repository
    /// down at the end. Adding item 794's arm turned that into a red the first time it ran:
    /// *the fixture needs a working `git`: ["init", "-q", "."]*, because the sibling had removed
    /// the directory out from under it. A fixture two tests share is a fixture neither owns.
    fn a_repository_mid_claim(under: &Path, tag: &str) -> PathBuf {
        let repo = under.join(format!("sprag-705-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&repo).expect("a directory to make a repository in");
        for args in [
            &["init", "-q", "."][..],
            &["config", "user.email", "gate@example"][..],
            &["config", "user.name", "gate"][..],
        ] {
            assert!(
                ran(Command::new("git").arg("-C").arg(&repo).args(args)),
                "the fixture needs a working `git`: {args:?}",
            );
        }
        std::fs::write(repo.join("door.txt"), "door returns 1\n").expect("the committed file");
        assert!(ran(Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["add", "door.txt"])));
        assert!(ran(Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["commit", "-qm", "base"])));
        // ⚠⚠ THE UNCOMMITTED WORK IS THE POINT OF THE FIXTURE. A repository that was CLEAN here
        // would be satisfied by a checkout of `HEAD` alone, and the arm that says the agent's
        // in-flight work travels would pass over a mechanism that carries nothing.
        std::fs::write(repo.join("door.txt"), "door returns 2\n").expect("the uncommitted edit");
        std::fs::write(repo.join("untracked.txt"), "not added\n").expect("the untracked file");
        repo
    }

    /// What the shared tree's own `git diff HEAD` reads — the exact thing register item 705's
    /// «done when» asks to hold still.
    fn diff_of(repo: &Path) -> Vec<u8> {
        Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["diff", "HEAD"])
            .output()
            .expect("the shared tree answers its own diff")
            .stdout
    }

    /// ⛔⛔⛔⛔⛔ **A CHECK THAT MUTATES AND PUTS IT BACK LEAVES THE SHARED TREE EXACTLY AS THE
    /// AGENT LEFT IT** — register item 705, and the property its «done when» names.
    ///
    /// # ⚠⚠⚠⚠⚠ The premise, asserted inside: the check must REALLY mutate
    ///
    /// Register item 280's fifth lesson, and it is the whole risk of this gate. *The shared tree
    /// did not change* is satisfied trivially by a check that never wrote anything, so the arm
    /// below writes into the copy and asserts the copy CHANGED before asking whether the original
    /// did. Without that this gate would go green against a mechanism that isolates nothing.
    ///
    /// # ⚠⚠⚠ And «unchanged» rather than «empty», which is a correction to the register
    ///
    /// The item says the shared tree's `git diff HEAD` must stay EMPTY, because the run it was
    /// filed on had committed before claiming. That is the special case. What has to hold in
    /// general is that the tree is **what the agent left** — so this fixture deliberately carries
    /// uncommitted work, and the assertion is equality with the reading taken before the check.
    #[test]
    fn a_check_that_mutates_its_own_copy_leaves_the_shared_tree_alone() {
        let under = std::env::temp_dir();
        let repo = a_repository_mid_claim(&under, "mutates");
        let before = diff_of(&repo);
        assert!(
            !before.is_empty(),
            "⚠⚠⚠⚠ THE FIXTURE: the agent must be mid-claim with work not yet committed, or the \
             arm about carrying it across is about nothing",
        );

        let cut = IsolatedCheckout::of(&repo, &under).expect("a repository can be cut");

        // ── (a) THE COPY CARRIES THE AGENT'S UNCOMMITTED WORK ─────────────────────────────────
        assert_eq!(
            std::fs::read_to_string(cut.path().join("door.txt")).ok(),
            Some("door returns 2\n".to_owned()),
            "⛔⛔⛔⛔ THE CHECKER WOULD HAVE JUDGED THE WRONG TREE. The claim under test is about \
             work that is in the working tree and not yet in `HEAD`; a copy that holds `HEAD` \
             alone lets a careful checker open real files, reason correctly, and answer about \
             something else — silently, which is worse than the defect this replaces",
        );
        // ⚠ AND THE RESIDUE IS ASSERTED RATHER THAN LEFT TO BE DISCOVERED: `git diff HEAD` cannot
        // describe a file git has never been told about, so an untracked one does not travel.
        // Named here so the day it matters somebody finds this line instead of the symptom.
        assert!(
            !cut.path().join("untracked.txt").exists(),
            "⚠⚠ the stated residue changed: untracked files now travel, and the module doc says \
             they do not",
        );

        // ── (b) ⚠⚠ THE PREMISE: THE CHECK REALLY MUTATES ──────────────────────────────────────
        std::fs::write(cut.path().join("door.txt"), "MUTATED BY THE CHECK\n")
            .expect("the check writes into its own copy");
        let during = diff_of(&repo);
        assert_ne!(
            std::fs::read_to_string(cut.path().join("door.txt")).ok(),
            Some("door returns 2\n".to_owned()),
            "⚠⚠⚠⚠⚠ THE PREMISE FAILED: the check wrote nothing, so «the shared tree did not \
             change» below is satisfied by a mechanism that isolates nothing at all",
        );

        // ── (c) THE CLAIM: THE SHARED TREE DID NOT MOVE ───────────────────────────────────────
        assert_eq!(
            during, before,
            "⛔⛔⛔⛔⛔ REGISTER ITEM 705: a check mutated the tree the AGENT is working in. \
             Measured 2026-08-26: a watcher read exactly this as the agent's leftover and told the \
             owner the agent's report was false — it was true — and then ran `git checkout --` \
             over it, which was a no-op only because the check had already finished",
        );

        // ── (d) AND IT IS STILL SO AFTER THE CHECK PUTS ITS MUTATION BACK ─────────────────────
        std::fs::write(cut.path().join("door.txt"), "door returns 2\n").expect("the check reverts");
        assert_eq!(
            diff_of(&repo),
            before,
            "⚠⚠⚠ and the whole cycle — cut, mutate, revert — leaves the shared tree where it was",
        );

        // ── (e) CLEANUP LEAVES NOTHING, which is the other half of not being in anybody's way ──
        let was_at = cut.path().to_path_buf();
        drop(cut);
        assert!(
            !was_at.exists(),
            "⚠⚠⚠⚠ A COPY OUTLIVED ITS CHECK. Nothing removes it afterwards — it is not litter but \
             a second tree holding a half-applied mutation, which is the confusion this module was \
             written to end, re-created by the repair",
        );
        assert_eq!(
            diff_of(&repo),
            before,
            "⚠⚠ and removing the copy is not a write to the original either",
        );
        let _ = std::fs::remove_dir_all(&repo);
    }

    /// ⛔⛔⛔⛔⛔ **A COPY DOES NOT CARRY THE SHARED TREE'S BUILD DIRECTORY** — register item 811.
    ///
    /// # Why the arm above is not already this one
    ///
    /// [`a_check_that_mutates_its_own_copy_leaves_the_shared_tree_alone`] proves that an untracked
    /// FILE does not travel. A build directory is a THIRD SHAPE: in this repository `target` is an
    /// untracked **symlink** into a cache every checkout on this machine shares. Register item 794
    /// measured that shapes are exactly what a check about litter gets wrong — `git status` cannot
    /// see three of them — so a gate that proved it about a regular file has not proved it here.
    ///
    /// # ⚠⚠ What it would cost, which is why the arm exists
    ///
    /// If the symlink travelled, a build inside the copy would write THROUGH it into the shared
    /// cache, and the objects it left would carry the COPY's paths. That is precisely the state
    /// register item 809 measured and now detects: a gate compiled against one tree answering for
    /// another. Item 811 tried to attribute a real occurrence of it and could not — the copy was
    /// gone and nothing records which tree a build wrote from — so this gate is the half that can
    /// be kept: the mechanism is pinned even though the event was not.
    ///
    /// ⚠⚠⚠ MEASURED 2026-09-01 before this was written, and the gate is what holds the answer: a
    /// `git worktree` cut from this repository and built with plain `cargo`, and again through the
    /// build wrapper, wrote only into its OWN `target/` — this workspace's cache did not move
    /// either time. Four candidate paths were refuted that way (the compiler cache, plain cargo,
    /// the wrapper, an environment override) and none of them is what happened.
    ///
    /// ⚠ The scratch root is ASKED FOR rather than taken — register items 794 and 795. The two
    /// tests beside this one predate that rule and still take it; this one does not add a site.
    #[cfg(unix)]
    #[test]
    fn a_copy_does_not_carry_the_shared_trees_build_directory() {
        let under = sprag_scratch::scratch_root();
        let repo = a_repository_mid_claim(&under, "no-build-dir");
        let cache = under.join("sprag-811-shared-cache");
        std::fs::create_dir_all(&cache).expect("a cache standing in for the shared one");
        std::os::unix::fs::symlink(&cache, repo.join("target"))
            .expect("the shape this repository's own build directory has");

        // ── ⚠⚠ THE PREMISE, ASSERTED: the fixture really carries the shape under test ─────────
        let planted = std::fs::symlink_metadata(repo.join("target"))
            .expect("the shared tree has a build directory");
        assert!(
            planted.file_type().is_symlink(),
            "⚠⚠ THE FIXTURE MUST BE A SYMLINK, not a directory. A plain directory is the shape the \
             arm above already covers, and a gate that planted one would be asserting about the \
             wrong kind of litter (register item 794)",
        );
        assert!(
            repo.join("target").is_dir(),
            "⚠ and it must RESOLVE, or `git` would be refusing a broken link rather than \
             declining to carry a live one",
        );

        let cut = IsolatedCheckout::of(&repo, &under).expect("a repository can be cut");

        assert!(
            std::fs::symlink_metadata(cut.path().join("target")).is_err(),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 811: the copy carries the shared tree's build directory. It \
             is a symlink into a cache every checkout on this machine shares, so a build inside \
             the copy writes THROUGH it — and the objects it leaves carry the COPY's paths, which \
             is the skew register item 809 detects. The copy is cut with `git worktree add` plus \
             `git apply` precisely so that only TRACKED work travels; whatever now carries \
             untracked entries has to stop.",
        );

        drop(cut);
        let _ = std::fs::remove_dir_all(&repo);
        let _ = std::fs::remove_dir_all(&cache);
    }

    /// ⚠⚠⚠⚠ **A DIRECTORY THAT IS NOT A REPOSITORY IS ANSWERED, NOT GUESSED AT** — the absence
    /// arm, and the one that keeps the caller's fallback honest.
    ///
    /// Every failure in [`IsolatedCheckout::of`] means *this check cannot be isolated*, and a
    /// caller that received a half-built copy instead would tell a checker to stand somewhere that
    /// holds nothing. `None` is what lets the caller degrade to what it did before item 705 **and
    /// say so** — register item 709's discipline, which this crate keeps finding new places for.
    #[test]
    fn a_directory_that_is_no_repository_cannot_be_cut() {
        let under = std::env::temp_dir();
        let plain = under.join(format!("sprag-705-plain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&plain);
        std::fs::create_dir_all(&plain).expect("an ordinary directory");
        assert!(
            IsolatedCheckout::of(&plain, &under).is_none(),
            "⚠⚠⚠ a directory git knows nothing about must answer «cannot», so the caller falls \
             back to the shared tree deliberately rather than pointing a checker at an empty copy",
        );
        let _ = std::fs::remove_dir_all(&plain);
    }

    /// ⛔⛔⛔⛔⛔ **A TEMPORARY ROOT THAT IS NOT ABSOLUTE IS REFUSED, AND NOTHING IS LEFT BEHIND** —
    /// register item 794, at the seam register item 705's own promise was resting on.
    ///
    /// # What was assumed and never measured
    ///
    /// [`crate::remote_access`]'s `cut` hands [`IsolatedCheckout::of`] `std::env::temp_dir()`, and
    /// its doc there promises *the temporary root is this machine's, **not a directory of the
    /// repository's*** — because a copy inside the tree being copied is litter the checker can
    /// wander into and it shows up in the agent's own `git status`. **That promise holds only if
    /// the path is absolute**, and `temp_dir()` answers `""` when `TMPDIR` is set-and-empty.
    ///
    /// ⚠⚠⚠ **AND GIT DOES NOT REFUSE THE RESULT**, which is what makes this a check worth having
    /// where the repository test above turned out to be decoration. Measured 2026-08-31 in a
    /// throwaway repository: `git -C <repo> worktree add --detach -q sprag-check-probe HEAD` exits
    /// **0** and leaves `?? sprag-check-probe/` inside the repository, because git resolves a
    /// relative path against its own `-C`. Item 705's confusion, re-created by the repair for it.
    ///
    /// # ⛔⛔⛔⛔⛔ THE FIXTURE IS MADE CLEAN ON PURPOSE, AND THE FIRST DRAFT WAS A DEAD CONTROL
    ///
    /// This test was first written against `a_repository_mid_claim` as it stands — **mid-claim, so
    /// carrying uncommitted work** — and asserted two things. A mutation that deleted the guard
    /// left it **GREEN**, which is the only signal a dead control ever gives. Both assertions were
    /// measuring something else:
    ///
    ///   * `is_none()` — with work to carry, [`of`](IsolatedCheckout::of) reaches [`applied`],
    ///     which runs `git -C <path> apply`. A RELATIVE `path` is resolved against the TEST
    ///     PROCESS's directory, not the repository, so it fails and `of` answers `None` for a
    ///     reason that has nothing to do with the root being relative.
    ///   * a `status` comparison — [`Drop`] removes the worktree, so the shared tree looks
    ///     identical afterwards whether or not one was ever cut. The litter is real WHILE the check
    ///     runs and gone by the time a test can look, which is exactly why it needed a guard rather
    ///     than an assertion.
    ///
    /// ⇒ The repository is emptied of uncommitted work first, so `carried_by` answers
    /// [`Carried::Nothing`] and `of` returns `Some` the moment the worktree is cut. **Then `None`
    /// has one cause left, and it is the guard.** The `status` arm is gone rather than reworded:
    /// what it claimed to hold is `Drop`'s to keep, and its neighbour above drives that.
    ///
    /// ⚠ Both shapes are driven: the EMPTY path that motivated this, and an ordinary relative one,
    /// because the guard asks a single question of both rather than testing for emptiness.
    #[test]
    fn a_temporary_root_that_is_not_absolute_is_refused() {
        let under = std::env::temp_dir();
        let repo = a_repository_mid_claim(&under, "not-absolute");
        // ⚠⚠ THE FIXTURE'S OWN POINT IS UNDONE HERE, DELIBERATELY. `a_repository_mid_claim` leaves
        // work uncommitted because its other caller is about carrying that across; this arm needs
        // the opposite, so that the only way back from `of` is the guard.
        assert!(
            ran(Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(["checkout", "--", "."])),
            "⚠⚠⚠ THE FIXTURE: the repository must be clean for `carried_by` to answer Nothing, or \
             `of` can refuse for a second reason and this arm stops measuring the guard",
        );

        for relative in ["", "sprag-check-relative"] {
            assert!(
                IsolatedCheckout::of(&repo, Path::new(relative)).is_none(),
                "⛔⛔⛔⛔⛔ ITEM 794: a temporary root of {relative:?} is not absolute, so the copy \
                 would be cut INSIDE the repository being copied — git resolves it against `-C` \
                 and exits 0, measured. `RemotePaneAccess::cut`'s promise that the root is not a \
                 directory of the repository's rests on this refusal, and nothing downstream makes \
                 it: the copy is torn down by `Drop` afterwards, so no later look can find it",
            );
        }
        let _ = std::fs::remove_dir_all(&repo);
    }
}
