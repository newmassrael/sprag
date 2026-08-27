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
//! * **A cold `target/` is affordable.** A worktree inherits no build cache — this repository's
//!   `target` is an untracked symlink to a shared cache, so a fresh checkout genuinely starts cold
//!   — and a cold whole-workspace compile measured **73 s** against
//!   [`CHECK_WITHIN`](crate::plugins)'s 600 s budget, one crate at 46 s. **No persistent
//!   checker-owned build directory is needed**, which is a whole mechanism this module does not
//!   have to carry.
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
        // to do while somebody else is working. `git diff HEAD` opens files; it changes none.
        let carried = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["diff", "HEAD", "--binary"])
            .output()
            .ok()?;
        if !carried.status.success() {
            return None;
        }
        // ⚠ AN EMPTY DIFF IS THE ORDINARY CASE, not a failure: the agent committed before claiming
        // its milestone, which is what the run this item was filed on had done. `git apply` refuses
        // an empty input, so the emptiness is answered here instead of read as a refusal.
        if carried.stdout.is_empty() {
            return Some(cut);
        }
        applied(&cut.path, &carried.stdout).then_some(cut)
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
    fn a_repository_mid_claim(under: &Path) -> PathBuf {
        let repo = under.join(format!("sprag-705-{}", std::process::id()));
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
        let repo = a_repository_mid_claim(&under);
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
}
