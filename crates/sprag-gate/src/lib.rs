//! The gates a test cannot be.
//!
//! # Why a crate outside the suite
//!
//! Some claims are about what the SUITE ITSELF did, and a test cannot make one. R341 measured
//! `cargo test` creating `~/.config/sprag/config.toml` on the developer's box — a test ran
//! `sprag set-option` through a helper that passed no environment, so the child resolved the
//! AMBIENT config home, and a SIBLING test then read that file and was green here and red on every
//! machine without it. The fix for any ONE call site is a seam. Nothing makes the NEXT call site
//! take one, and no test can be the guard: `XDG_CONFIG_HOME` is process-global, so a test can
//! neither observe nor isolate what the tests running beside it do to it.
//!
//! So the guard has to be a separate process, run after the suite. This crate is where those live.
//!
//! # ⚠ And the first one shipped broken, which is why the logic is here and not in the yaml
//!
//! R342 wrote that guard as three lines of shell in `ci.yml`: `find "$RUNNER_TEMP/ambient"
//! -mindepth 1`. `$RUNNER_TEMP/ambient` is the PARENT of the three homes, and the test step's own
//! `mkdir -p` creates them, so the find always returned three directories and the step **failed
//! unconditionally**. It never once passed. Both Linux runs that carried it were red for that
//! reason, and the round that added it recorded a green measured on the commits before it.
//!
//! The defect was not the depth argument. It was that **the guard looked somewhere other than
//! where the suite wrote** — so [`ambient_homes`] derives its paths from the same three environment
//! variables the suite ran under, and a variable that is unset or does not name a directory is an
//! ERROR rather than an empty walk. A probe pointed at nothing must never read as clean.

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

/// The three environment variables an XDG-respecting process writes under, in the order a report
/// reads best.
///
/// `XDG_DATA_HOME` is here even though no sprag reader resolves one today: the claim this guard
/// makes is *the suite wrote nothing outside what it was given*, and a variable nobody reads yet is
/// exactly where the next call site will write without anybody noticing.
pub const AMBIENT_HOMES: [&str; 3] = ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"];

/// A home this guard was asked to watch and could not.
///
/// Its own type because the alternative is the defect this crate exists for: a mis-pointed probe
/// that walks nothing, finds nothing, and reports the suite clean.
#[derive(Debug, PartialEq, Eq)]
pub enum Unwatchable {
    /// The variable is not set at all — nobody told this process where to look.
    Unset(&'static str),
    /// It is set, and what it names is not a directory that can be read.
    Unreadable {
        /// The variable that named it.
        var: &'static str,
        /// What it named.
        path: PathBuf,
        /// Why the walk could not start.
        why: String,
    },
}

impl fmt::Display for Unwatchable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unset(var) => write!(
                f,
                "{var} is not set, so this guard has no idea where the suite was pointed. \
                 Run the suite and this guard under the SAME three variables."
            ),
            Self::Unreadable { var, path, why } => write!(
                f,
                "{var} names {} and it cannot be walked ({why}). A guard that cannot read the \
                 directory it is judging must say so rather than report it empty.",
                path.display()
            ),
        }
    }
}

/// Every path under `home`, at any depth — empty exactly when nothing was written there.
///
/// RECURSIVE, and that is the substance rather than a detail: the write this guard exists to catch
/// is `<config home>/sprag/config.toml`, which is two levels down. A walk that only listed the
/// entries of `home` itself would see `sprag/` and could not tell a directory somebody made from a
/// file somebody wrote — and one that started a level too high, as the shell version did, sees the
/// homes themselves and can never be quiet at all.
///
/// # Errors
///
/// If `home` cannot be read as a directory. **Never treated as "nothing was written"**: a probe
/// that names the wrong thing answers about the wrong thing, and this project has now spent four
/// rounds on that one shape.
pub fn writes_under(home: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut walking = vec![home.to_path_buf()];
    while let Some(dir) = walking.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.is_dir() {
                walking.push(path.clone());
            }
            found.push(path);
        }
    }
    found.sort();
    Ok(found)
}

/// The three homes named by [`AMBIENT_HOMES`] in this process's environment, or the first one that
/// cannot be watched.
///
/// # Errors
///
/// If any of the three is unset, or names something that cannot be walked.
pub fn ambient_homes() -> Result<Vec<(&'static str, PathBuf)>, Unwatchable> {
    homes_from(std::env::var_os)
}

/// [`ambient_homes`] against a stated environment rather than the process's — the seam its own
/// tests need, since the real one is process-global and this crate's tests run as threads of one
/// binary (the very property that makes a test unable to be the guard).
fn homes_from(
    lookup: impl Fn(&'static str) -> Option<OsString>,
) -> Result<Vec<(&'static str, PathBuf)>, Unwatchable> {
    let mut homes = Vec::with_capacity(AMBIENT_HOMES.len());
    for var in AMBIENT_HOMES {
        let value = lookup(var).ok_or(Unwatchable::Unset(var))?;
        let path = PathBuf::from(value);
        // Read once HERE rather than left to the walk, so "you pointed me at a file" and "you
        // pointed me at nothing" are one answer with one shape.
        std::fs::read_dir(&path).map_err(|why| Unwatchable::Unreadable {
            var,
            path: path.clone(),
            why: why.to_string(),
        })?;
        homes.push((var, path));
    }
    Ok(homes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory per test, since these run as threads of one binary.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sprag-gate-{}-{tag}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch home");
        dir
    }

    #[test]
    fn a_home_nobody_wrote_to_reports_nothing() {
        let home = scratch("clean");
        assert_eq!(
            writes_under(&home).expect("walk a clean home"),
            Vec::<PathBuf>::new()
        );
    }

    /// ⚠ THE ONE THE SHIPPED SHELL COULD NOT MAKE. The write this guard exists to catch is
    /// `<config home>/sprag/config.toml`, so a walk that stops at the first level is blind to it.
    #[test]
    fn a_file_the_suite_left_two_levels_down_is_found() {
        let home = scratch("nested");
        std::fs::create_dir_all(home.join("sprag")).expect("the product's own directory");
        std::fs::write(
            home.join("sprag").join("config.toml"),
            "window-size = \"manual\"\n",
        )
        .expect("the file R341 measured a test writing");

        let found = writes_under(&home).expect("walk a written home");
        assert!(
            found.contains(&home.join("sprag").join("config.toml")),
            "the file two levels down must be named: {found:?}",
        );
        // And the directory holding it, so a report says the whole shape of what appeared.
        assert!(found.contains(&home.join("sprag")), "{found:?}");
    }

    /// A home that cannot be read is an ERROR, never an empty walk.
    ///
    /// This is the class the shipped guard belonged to from the other side: it looked one level too
    /// high and could never be quiet. The opposite mistake — looking somewhere that does not exist —
    /// is quiet FOREVER, which is worse, because a gate that always passes reads exactly like a
    /// product that is behaving.
    #[test]
    fn a_home_that_is_not_there_is_a_failure_and_not_a_pass() {
        let home = scratch("absent").join("never-created");
        let error = writes_under(&home).expect_err("a walk that cannot start must say so");
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn an_unset_variable_is_named_rather_than_skipped() {
        let error = homes_from(|_| None).expect_err("nothing was set");
        assert_eq!(error, Unwatchable::Unset("XDG_CONFIG_HOME"));
        assert!(
            error.to_string().contains("the SAME three variables"),
            "and it says how to fix it: {error}",
        );
    }

    #[test]
    fn a_variable_pointing_at_a_file_is_named_with_what_it_pointed_at() {
        let dir = scratch("not-a-dir");
        let file = dir.join("this-is-a-file");
        std::fs::write(&file, "x").expect("write the decoy");
        let owned = file.clone();

        let error = homes_from(move |var| {
            (var == "XDG_CONFIG_HOME").then(|| OsString::from(owned.clone()))
        })
        .expect_err("a file is not a home");
        match error {
            Unwatchable::Unreadable { var, path, .. } => {
                assert_eq!(var, "XDG_CONFIG_HOME");
                assert_eq!(path, file);
            }
            other => panic!("the file must be named, not {other:?}"),
        }
    }

    /// All three are required, not just the first — the guard's claim covers config, data and state.
    #[test]
    fn every_home_the_list_names_must_be_watchable() {
        let dir = scratch("partial");
        let owned = dir.clone();
        let error =
            homes_from(move |var| (var != "XDG_STATE_HOME").then(|| OsString::from(owned.clone())))
                .expect_err("one missing home is a missing guard");
        assert_eq!(error, Unwatchable::Unset("XDG_STATE_HOME"));
    }
}
