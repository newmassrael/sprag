//! A stand-in program a suite hands to the thing it is driving — and the rule that **nothing here
//! writes one**.
//!
//! # ⚠⚠⚠⚠⚠ Why this module exists: a suite that writes a program cannot reliably run it
//!
//! On **Linux**, `execve` refuses a file any process holds open for writing, with `ETXTBSY`
//! (`Os { code: 26 }`, *"Text file busy"*). Rust's test harness runs its cases on THREADS of one
//! process, so a case that forks to spawn a program inherits every write handle its siblings
//! happen to have open at that instant and holds each one until its own exec. A case that writes
//! its own double is therefore racing every other case in the binary, and `O_CLOEXEC` does not
//! close that window — it ends it one exec too late.
//!
//! Register item 465 measured the shape on one suite: **10 failures in 30 runs before, 0 in 30
//! after**, every failure `Text file busy` at the same line and every one green again under
//! `--test-threads=1`. That is how it survived for months as *a flake*. Item 467 is the class:
//! ten more sites in this workspace wrote an executable and then ran it, and only 465's happened
//! to run often enough beside a forking sibling to be caught.
//!
//! **A file nobody writes cannot be busy.** So a double is a TRACKED file in the crate that owns
//! it, and a fixture that needs one at a path it computes — a directory with a space in the name,
//! a `PATH` entry of its own, a program that has to be called `claude` — reaches it with a
//! SYMLINK. Linking never opens the target for writing, and the kernel's write-deny check follows
//! the link to the inode, so the tracked file is what is judged and nothing ever holds it.
//!
//! ⚠ **The remedy is not a retry on `ETXTBSY`.** That is the workaround item 465 refused: a retry
//! turns a race into a slower race and leaves the next site to find it again.
//!
//! # ⚠⚠⚠⚠⚠ The deny is LINUX'S ALONE, so a green macOS job proves nothing about this class
//!
//! Linux runs `deny_write_access` on the `open` that exec performs — for a `#!` script as much as
//! for a binary, because the shebang path opens the script for exec first and the deny is on that
//! open. **macOS makes no such check anywhere**: both shapes simply RUN, with the write handle
//! still held. That is measured on this workspace's own fleet rather than read out of a manual
//! page, and [`exec_of_a_held_writer`](crate::doubles::exec_of_a_held_writer) is the ONE place
//! that says which platform does which.
//!
//! ⚠⚠⚠⚠⚠ **This module used to state the deny as a universal, and item 467's own measurement
//! asserted the Linux answer on every platform** — so it was red on the macOS job of every push
//! from `28fb1a6` onward while the prose sites across this tree went on repeating the false
//! version, none of which any gate can go red for. A kernel
//! contract is a per-platform fact; spelling one without naming its platform is the same defect
//! `no_source_spells_a_bin_path_the_other_platform_lacks` exists for, in a different spelling.
//!
//! ⚠⚠ The consequence is operational, not academic: **the flake this module prevents cannot happen
//! on macOS at all**, so a macOS run being green says nothing about whether a suite is racing.
//! Only the Linux job — and a Linux developer's own sweep — can see it. The rule here stays
//! uniform across both platforms anyway, because the tree is shared and a double written for the
//! platform that tolerates it is a defect waiting for the platform that does not.
//!
//! # ⚠⚠⚠ And the fixture asserts its own staging
//!
//! A tracked file can arrive without its mode — a checkout that dropped the bit, an archive that
//! flattened it, a `git apply` of a patch that did not carry it. A double that cannot be executed
//! makes every case that uses it fail for the wrong reason, which is item 384's lesson. So
//! [`Doubles::program`](crate::doubles::Doubles::program) reads the mode and says so loudly, rather
//! than letting the case below it report the product refusing.
//!
//! ⚠ That link is spelled in full on purpose. This module's documentation is TWO fragments — the
//! `///` on `pub mod doubles;` over in `lib.rs` and this header — and rustdoc resolves the merged
//! comment in the outer scope, where `Doubles` is not a name. The rustdoc gate caught it
//! (`unresolved link to Doubles::program`, with no file or line to point at) and `cargo check`
//! could not have.

use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// The tracked `tests/doubles/` directory of one crate.
///
/// Built from that crate's `CARGO_MANIFEST_DIR`, so the answer is the SOURCE tree rather than
/// wherever the test binary was uplifted to — the two differ under `cargo test`, and only the
/// first holds files git can track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Doubles {
    dir: PathBuf,
}

impl Doubles {
    /// The doubles owned by the crate whose manifest directory this is.
    ///
    /// Call it as `Doubles::of(env!("CARGO_MANIFEST_DIR"))` — the macro is the caller's, because
    /// it expands to the crate being compiled and a helper cannot ask that question for somebody
    /// else.
    #[must_use]
    pub fn of(manifest_dir: &str) -> Self {
        Self {
            dir: [manifest_dir, "tests", "doubles"].iter().collect(),
        }
    }

    /// One SUITE's doubles, in a directory of their own.
    ///
    /// ⚠⚠⚠⚠ **This is a separation rather than tidiness, and leaving it out is a live hazard.** A
    /// set goes on a `PATH` whole — that is what a double is for — so a `git` some other suite
    /// needed would silently answer the `git` calls of a suite that wanted the real one. The
    /// commit-msg suite puts its directory in front of the developer's `PATH` and the hooks it
    /// drives call `git` five times; a flat directory would have handed them a stand-in nobody in
    /// that suite asked for.
    #[must_use]
    pub fn set(self, suite: &str) -> Self {
        Self {
            dir: self.dir.join(suite),
        }
    }

    /// The directory itself — what goes on a `PATH` when the product resolves the double by name.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The double called `name`, having checked it is there and that it can be executed.
    ///
    /// # Panics
    ///
    /// When the file is missing, or when no execute bit survived the checkout. Both are staging
    /// failures rather than product failures, and saying so here is the difference between a
    /// person reading *the double is not executable* and a person reading *the hook refused*.
    #[must_use]
    pub fn program(&self, name: &str) -> PathBuf {
        let path = self.dir.join(name);
        let mode = std::fs::metadata(&path)
            .unwrap_or_else(|why| {
                panic!(
                    "⚠ THE TRACKED DOUBLE MUST BE THERE: {} — {why}. It is a file this repository \
                     carries, never one a test writes (register item 467).",
                    path.display(),
                )
            })
            .permissions()
            .mode();
        assert!(
            mode & 0o111 != 0,
            "⚠⚠⚠ THE DOUBLE MUST BE EXECUTABLE ({mode:o}): {}. Without the bit every case that \
             uses it reports the product refusing — for the wrong reason entirely.",
            path.display(),
        );
        path
    }

    /// Make the double called `name` reachable at `at`, by a link.
    ///
    /// This is the answer whenever the fixture needs the program at a path it computed rather than
    /// on a `PATH` it controls: a per-run directory, a directory whose name has a space in it, a
    /// name the product decides on. Any existing entry at `at` is replaced, so a fixture may
    /// re-stage without tearing its directory down.
    ///
    /// ⚠ A COPY here would be the defect this module exists for, one indirection further along.
    ///
    /// # Panics
    ///
    /// When the double is unusable (see [`Doubles::program`]) or the link cannot be made.
    pub fn link(&self, name: &str, at: &Path) -> PathBuf {
        linked_as(&self.program(name), at)
    }

    /// A `PATH` value with this directory in FRONT of `inherited`.
    ///
    /// The double has to win against whatever the developer has installed, and the inherited entries
    /// have to survive: a double that delegates — this workspace's `grep` and `git` both do — needs
    /// to find the real tool behind it.
    #[must_use]
    pub fn ahead_of(&self, inherited: &OsString) -> OsString {
        let mut dirs = vec![self.dir.clone()];
        dirs.extend(std::env::split_paths(inherited));
        std::env::join_paths(dirs).expect("a PATH with the doubles in front")
    }

    /// A `PATH` value with this directory in front of the one this process inherited.
    #[must_use]
    pub fn ahead_of_inherited(&self) -> OsString {
        self.ahead_of(&std::env::var_os("PATH").unwrap_or_default())
    }
}

/// Make `program` reachable at `at`, by a symlink rather than a copy.
///
/// The free function is for the stand-ins whose program is a REAL one the system already has —
/// `/bin/cat` standing in for an agent, `/bin/sleep` under a name that breaks a `/proc` parse. A
/// copy of those is a file this process wrote, and carries item 467's window exactly like a script
/// it composed; a link carries none of it, because the inode being executed is the system's and
/// nothing here can open it for writing.
///
/// ⚠ The link is what is exec'd, so its own basename is what `argv[0]`, `comm` and any
/// basename-reading rule see — which is the point at every call site here.
///
/// # Panics
///
/// When `program` is not there, or the link cannot be made.
pub fn linked_as(program: &Path, at: &Path) -> PathBuf {
    assert!(
        program.exists(),
        "⚠ the program a stand-in links to must be there: {}",
        program.display(),
    );
    if let Some(parent) = at.parent() {
        std::fs::create_dir_all(parent)
            .unwrap_or_else(|why| panic!("a directory for the stand-in {}: {why}", at.display()));
    }
    let _ = std::fs::remove_file(at);
    std::os::unix::fs::symlink(program, at).unwrap_or_else(|why| {
        panic!(
            "link {} to the stand-in {}: {why}",
            program.display(),
            at.display(),
        )
    });
    at.to_path_buf()
}

/// Where THIS machine keeps the system program called `name`, found the way a shell finds it.
///
/// # ⚠⚠⚠⚠⚠ Why nothing here may spell `/bin/<name>`
///
/// The two platforms this workspace's suites run on do not agree about what `/bin` holds. On Linux
/// `/bin` is a symlink to `/usr/bin` and therefore holds EVERYTHING, so any `/bin/<name>` a source
/// spells resolves. macOS's `/bin` is a real directory of about thirty programs, and `true` and
/// `false` are not among them — they live in `/usr/bin` and there is no `/bin/true` at all.
///
/// So `/bin/<name>` is precisely the shape that is green on every Linux run and `NotFound` on the
/// macOS job: a red that arrives hours later, in a crate whose author was editing something else,
/// wearing the wrong cause.
///
/// ⚠⚠ **This workspace has now learned that three times and applied it none.** It is written on a
/// `pty` doctest, where it was first paid for; it is written in register item 467's own entry,
/// which recorded copying `/bin/true`; and it came back as the macOS red on `28fb1a6`, which item
/// 467's `doubles` tests and item 471's `feeding` test both carried in on the same push. A lesson
/// recorded beside one call site does not reach the next one — so this seam exists, and
/// `no_source_spells_a_bin_path_the_other_platform_lacks` is what makes the tree take it.
///
/// ⚠ `PATH` rather than a list of directories to try, because that is the question actually being
/// asked — *where does this machine keep it* — and it is the answer the product's own children get.
///
/// # Panics
///
/// When no executable called `name` is on `PATH`. A fixture that quietly fell back to a program
/// that is not there would fail later and somewhere else, which is the diagnosis this project keeps
/// paying for.
#[must_use]
pub fn system(name: &str) -> PathBuf {
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let searched: Vec<PathBuf> = std::env::split_paths(&inherited).collect();
    for dir in &searched {
        let candidate = dir.join(name);
        let Ok(found) = candidate.metadata() else {
            continue;
        };
        if found.is_file() && found.permissions().mode() & 0o111 != 0 {
            return candidate;
        }
    }
    panic!(
        "⚠ this machine has no executable called `{name}` on PATH, so a fixture cannot reach the \
         system program it wanted to stand in for. Searched: {}",
        searched
            .iter()
            .map(|dir| dir.display().to_string())
            .collect::<Vec<_>>()
            .join(", "),
    )
}

/// `ETXTBSY` — the errno a refused exec carries, *"Text file busy"*.
///
/// Spelled once, here, because a bare `26` in an assertion is the kind of literal that gets read as
/// a magic number and copied to a site where it means something else.
pub const TEXT_FILE_BUSY: i32 = 26;

/// What a platform's `execve` does with a file some process is holding open for writing.
///
/// This is a KERNEL CONTRACT and the two platforms this workspace's suites run on do not agree
/// about it, which is the whole reason it is named rather than assumed — see the [module
/// header](crate::doubles).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeldWriter {
    /// The exec is refused with [`TEXT_FILE_BUSY`]. Linux, via `deny_write_access`.
    Refused,
    /// The exec is permitted; the platform makes no such check. macOS.
    Permitted,
    /// This platform has never been measured here, so nothing is claimed about it.
    ///
    /// ⚠ A third variant rather than a guess or a `compile_error!`: a build on a platform nobody
    /// has measured should still BUILD, and the measurement below is what refuses — loudly, and
    /// with the instruction to go measure — instead of a `cfg` chain silently handing back
    /// whichever answer happened to be last in the list.
    Unmeasured,
}

/// What THIS platform is expected to do with a file held open for writing, as measured on this
/// workspace's own fleet.
///
/// ⚠⚠⚠⚠⚠ **This is the single source of truth for that fact.** Prose elsewhere in the tree points
/// here rather than restating it — every site that restated it had to be corrected by hand when
/// the premise turned out to be per-platform, which is how the macOS red survived as long as it
/// did. `cfg!` is what makes it a claim the compiler picks per target, and
/// `this_platform_matches_the_held_writer_contract_it_declares` is what makes the claim FALSIFIABLE
/// — it stages the window for real and compares, so this going stale in either direction is a red
/// rather than a comment nobody re-reads.
#[must_use]
pub const fn exec_of_a_held_writer() -> HeldWriter {
    if cfg!(target_os = "linux") {
        HeldWriter::Refused
    } else if cfg!(target_os = "macos") {
        HeldWriter::Permitted
    } else {
        HeldWriter::Unmeasured
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠⚠⚠⚠⚠ **THE MECHANISM, MEASURED ON THIS MACHINE RATHER THAN QUOTED FROM A MANUAL PAGE.**
    ///
    /// Item 467's whole premise is that a file held open for writing cannot be executed, and the
    /// register has been wrong about a premise before. This stages the window deliberately — a
    /// write handle this test holds, on a file this test wrote — and compares what the platform
    /// actually does against what [`exec_of_a_held_writer`] declares it does.
    ///
    /// ⚠⚠⚠⚠⚠ **This is the assertion that used to be wrong, and it is worth saying how.** It read
    /// `.status().unwrap_err()` and required `ETXTBSY` — the LINUX answer, asserted on every
    /// target. macOS permits the exec, so from `28fb1a6` onward the macOS job was red on a fact
    /// nobody had measured there. Two defects, not one: the premise was universal when the
    /// mechanism is per-platform, AND `unwrap_err()` panicked *before* the message could speak, so
    /// the interesting outcome — permitted — reported `unwrap_err() on an Ok value` and named
    /// neither the platform nor the shape. Both are fixed here: the outcome is classified first
    /// and every branch says which shape it was.
    ///
    /// ⚠ **Both shapes**, because eight of the ten sites the item names are `#!/bin/sh` scripts
    /// rather than binaries, and *"the kernel only denies writes to an IMAGE"* would have excused
    /// them. On Linux it does not: the shebang path opens the script for exec first, and the deny
    /// is on that open. Measured here so the excuse cannot be made from a reading — and kept as
    /// two INDEPENDENT measurements so that a platform which ever split the two is reported as
    /// splitting them, rather than reported as whichever shape ran first.
    ///
    /// ⚠ It cannot flake in the direction that matters: a sibling's inherited write handle can
    /// only make a refusal more likely, never less, so the `Refused` platforms cannot be flaked
    /// green. On a `Permitted` platform the staging holds regardless — there is no race to lose.
    #[test]
    fn the_held_writer_contract_this_platform_declares_is_what_it_actually_does() {
        use std::io::Write as _;

        let declared = exec_of_a_held_writer();
        assert_ne!(
            declared,
            HeldWriter::Unmeasured,
            "⚠ this platform's exec-of-a-held-writer contract has never been measured, so \
             `exec_of_a_held_writer` claims nothing about it. Run this case here, read what it \
             reports, and add the arm — do NOT guess from a manual page, which is the mistake \
             that put the universal claim in this module in the first place.",
        );

        let dir = std::env::temp_dir().join(format!("sprag-gate-etxtbsy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a directory for the staged window");

        for (shape, body) in [
            ("script", b"#!/bin/sh\nexit 0\n".to_vec()),
            (
                "image",
                std::fs::read(system("true")).expect("a real binary to copy"),
            ),
        ] {
            let path = dir.join(shape);
            let mut held = std::fs::File::create(&path).expect("write the file under test");
            held.write_all(&body).expect("its bytes");
            held.flush().expect("its bytes reach the file");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("make it executable");

            // The handle is STILL OPEN here, which is the whole staging.
            let measured = match std::process::Command::new(&path).status() {
                Err(why) if why.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                    // ⚠ BOTH facts, because they can drift apart: std's classification is what a
                    // caller would match on, and the number is what item 467's entry and this
                    // tree's prose name. A platform where they disagree is a thing to know.
                    assert_eq!(
                        why.raw_os_error(),
                        Some(TEXT_FILE_BUSY),
                        "⚠ this platform refused the {shape} with `ExecutableFileBusy` but errno \
                         {:?} rather than {TEXT_FILE_BUSY} — std's classification and the number \
                         this tree spells have come apart, so fix `TEXT_FILE_BUSY` and the prose \
                         that quotes it together.",
                        why.raw_os_error(),
                    );
                    HeldWriter::Refused
                }
                Ok(_) => HeldWriter::Permitted,
                Err(why) => panic!(
                    "⚠ executing the {shape} while this process holds it open for writing \
                     answered {why:?}, which is neither a refusal ({TEXT_FILE_BUSY}, ETXTBSY) nor \
                     a run. That is a THIRD behaviour this module does not know about — measure \
                     it and give `HeldWriter` an arm rather than widening this one.",
                ),
            };
            assert_eq!(
                measured, declared,
                "⚠ this platform executed the {shape} shape as {measured:?} while \
                 `exec_of_a_held_writer` declares {declared:?}. The declaration is the tree's \
                 single source of truth for register item 467's premise and the prose across the \
                 workspace rests on it, so fix the declaration and that prose together — never \
                 this assertion alone.",
            );
            drop(held);
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A tracked double is found, is executable, and RUNS — this crate's own `grep`, which item 465
    /// put there.
    #[test]
    fn a_tracked_double_is_found_executable_and_runnable() {
        let doubles = Doubles::of(env!("CARGO_MANIFEST_DIR")).set("commit-msg");
        let grep = doubles.program("grep");
        assert!(
            grep.starts_with(env!("CARGO_MANIFEST_DIR")),
            "in the SOURCE tree, which is the only place git can carry a file: {}",
            grep.display(),
        );

        let refused = std::process::Command::new(&grep)
            .args(["-q", "-P", "x"])
            .stdin(std::process::Stdio::null())
            .output()
            .expect("the tracked double runs");
        assert_eq!(
            refused.status.code(),
            Some(2),
            "and it is the grep item 403 needs"
        );
    }

    /// A link reaches the same program under a name the fixture chose, and re-linking replaces.
    #[test]
    fn a_link_reaches_the_program_under_a_chosen_name_and_can_be_restaged() {
        let dir = std::env::temp_dir().join(format!("sprag-gate-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        // A directory with a SPACE in it, because that is the shape `sprag-host`'s hook fixture
        // needs and the one a copy was reached for.
        let at = dir.join("a dir").join("claude");
        linked_as(&system("true"), &at);
        assert!(
            std::process::Command::new(&at)
                .status()
                .expect("the linked program runs")
                .success(),
        );

        // Re-staging is not an error: a fixture may point the same name somewhere else.
        linked_as(&system("false"), &at);
        assert!(
            !std::process::Command::new(&at)
                .status()
                .expect("the relinked program runs")
                .success(),
            "a second link must REPLACE the first, or a fixture cannot restage",
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
