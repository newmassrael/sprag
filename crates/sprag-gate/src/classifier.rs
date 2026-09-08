//! 🎯🎯🎯🎯🎯 **WHAT THE LOOP'S CLASSIFIER CAN BE BROKEN BY** — register item 841.
//!
//! # ⛔⛔⛔⛔⛔ The register's own sentence was wider than the truth, and the measurement is here
//!
//! Item 841 was opened reading *"whether the loop can re-aim now depends on whether this workspace
//! compiles — the classifier is a `cargo run`, so a round with a broken build has every proposal
//! quietly set aside."* Measured 2026-09-08 in a throwaway worktree of this repository, four
//! separate ways, and **the headline is false**:
//!
//! | broken | what the classifier did |
//! |---|---|
//! | `sprag-plugin`'s lib does not compile (`rc=101` for `-p sprag-plugin --lib`) | answered `YES STEP`, **exit 0** |
//! | cold build of the classifier's own bin, empty target directory | compiled **one** crate, `sprag-gate` |
//! | `crates/sprag-gui/Cargo.toml` malformed (`[package` unclosed) | **exit 101**, no answer |
//! | `Cargo.lock` stale by one path dependency | answered, **and rewrote the lockfile** |
//!
//! So product CODE cannot reach it — the classifier's package declares no dependencies, and
//! `cargo run -p <it> --bin <it>` compiles that package alone. What CAN reach it is the workspace's
//! MANIFEST GRAPH, which every member's `Cargo.toml` is part of, and the LOCKFILE, which it was
//! rewriting until `--locked` went into the argv.
//!
//! # ⚠⚠⚠ Why the COMPILE CLOSURE is the predicate, rather than *break it and see*
//!
//! The register's `Done when` names the mutation — *break the product, does the classifier still
//! answer* — and a gate shaped that way proves the crate it happened to break is outside the
//! closure. It says nothing about the next crate. **The closure itself is the general fact**: if
//! the only crate cargo compiles for the classifier is the classifier's own, then no product code
//! anywhere can fail it, for every breakage rather than the chosen one. It is also what cargo will
//! tell you for free, on its own `Compiling` lines, in about a second and a half.
//!
//! ⚠ And it is red under the real mutation, measured before this module was written: one
//! `sprag-detect = { workspace = true }` added to the classifier's `[dependencies]` took the
//! closure from **1 crate to 100** — `proc-macro2`, `syn`, `regex`, `termwiz` and the rest of the
//! product's tree, all of them now able to stop an answer.
//!
//! # ⚠⚠ What this module does NOT claim, stated because a gate's name is a claim
//!
//! Nothing here says the classifier survives a malformed manifest — it does not, and that is the
//! row above. The remedy for that one is to take the classifier's package out of this workspace,
//! which was measured and not taken: `-p sprag-gate` is spelled at **4** call sites in this
//! repository's hooks (`pre-push` 3, `pre-commit` 1) and **7** in its CI workflow, every one of
//! them calling it as a member of the root workspace — and `./target/debug/north-star`, which the
//! eviction would move, is the command every round of the loop reads its own population with. See
//! the register.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The kind document whose datamodel authors this repository's classifier, relative to the
/// workspace root.
///
/// ⚠ The sibling spelling is [`crate::loop_shape::DOCUMENT`], which is the TEMPLATE — the machine
/// every repository copies. This one holds the decisions that are this tree's own, and the argv
/// below is one of them.
pub const KIND_DOCUMENT: &str = "crates/sprag-plugin/src/debt_loop.scxml";

/// The datamodel id whose value is the classifier's argv.
pub const SUCCESSOR_CHECK: &str = "successor_check";

/// The flag that keeps the classifier from rewriting the lockfile of the tree it is judging.
///
/// ⚠⚠ Register item 196 is what makes this load-bearing rather than tidy: this tree has TWO
/// writers, so a `Cargo.lock` the referee rewrote lands in whatever the agent commits next, unread
/// by anybody. Measured 2026-09-08 — one path dependency added to a member was enough, and the
/// classifier answered and rewrote in the same breath.
pub const LOCKED: &str = "--locked";

/// How much of an unreadable line a refusal quotes back.
///
/// A panic message that pastes a whole `<data>` element buries the sentence saying what to do.
const QUOTE_CAP: usize = 120;

/// **THE CLASSIFIER THIS REPOSITORY'S KIND DOCUMENT ACTUALLY DEPLOYS**, read into the parts a gate
/// can ask about.
///
/// ⚠ Every field is read off the document rather than spelled here. A gate holding its own copy of
/// the argv would agree with itself for ever, which is the shape this crate exists to refuse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deployed {
    /// The whole argv, whitespace-split, environment prefix and the classifier's own arguments
    /// included.
    pub argv: Vec<String>,
    /// Where the environment prefix sends cargo's output, or [`None`] where it names no directory
    /// at all — which is cargo's default, the tree's own `target/`, and the contention register
    /// item 196 is about.
    pub target_dir: Option<PathBuf>,
    /// The package cargo is told to run (`-p`).
    pub package: String,
    /// The binary of that package (`--bin`).
    pub bin: String,
    /// **CARGO'S OWN SWITCHES**, which is to say the ones between `run` and the `--` separator. The
    /// classifier's own arguments are past that separator and are not cargo's business.
    pub cargo_flags: BTreeSet<String>,
}

/// Why a kind document's classifier could not be read — never a default, on this crate's standing
/// rule that a probe which cannot see must not read as clean.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unreadable {
    /// The document declares no [`SUCCESSOR_CHECK`] at all. **The honest state for a tree that has
    /// written no classifier**, and the one a copied template is in.
    Unauthored,
    /// The id is there and its `expr` holds no quoted text, so there is no argv to read — carrying
    /// what was found instead.
    Unquoted(String),
    /// The argv is not `cargo run` — carrying it, because *some other program* is a decision an
    /// author took and this gate has nothing to say about one.
    NotACargoRun(Vec<String>),
    /// A switch this reader needs carries no value, or is absent — carrying which, and the argv.
    Unnamed {
        /// The switch that was looked for.
        flag: &'static str,
        /// What was read instead.
        argv: Vec<String>,
    },
}

impl Unreadable {
    /// **WHAT A READER SHOULD DO ABOUT IT** — the sentence a refusal carries.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Unauthored => format!(
                "this document authors no `{SUCCESSOR_CHECK}`, so nothing classifies a proposal \
                 and every reflection may aim this run wherever it likes"
            ),
            Self::Unquoted(found) => format!(
                "`{SUCCESSOR_CHECK}` is declared and holds no quoted text, so there is no argv to \
                 run: {found}"
            ),
            Self::NotACargoRun(argv) => format!(
                "`{SUCCESSOR_CHECK}` names a program that is not `cargo run`, which is an author's \
                 decision this gate has nothing to say about: {}",
                argv.join(" ")
            ),
            Self::Unnamed { flag, argv } => format!(
                "`{SUCCESSOR_CHECK}` is a cargo run that names no {flag}, so what it builds cannot \
                 be read off it: {}",
                argv.join(" ")
            ),
        }
    }
}

/// A cargo log that reported compiling nothing — see [`compiled_crates`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NothingCompiled {
    /// The first line the build printed, or the empty string where it printed none.
    pub said: String,
}

impl NothingCompiled {
    /// **WHAT A READER SHOULD DO ABOUT IT.**
    #[must_use]
    pub fn describe(&self) -> String {
        format!(
            "cargo printed no `Compiling` line, so this log says nothing about what the classifier \
             is built from — a cached target directory or a build that never started, and either \
             way the question was not asked. It printed: {:?}",
            self.said
        )
    }
}

/// The classifier's argv as its document authors it — the quoted segments of the `expr`, joined.
///
/// # Errors
///
/// [`Unreadable::Unauthored`] where the document declares no such id, [`Unreadable::Unquoted`]
/// where it declares one and holds no string.
pub fn authored_argv(scxml: &str) -> Result<String, Unreadable> {
    // ⚠ COMMENTS FIRST. This document explains itself at length and names this very id in its
    // prose three times over (measured 2026-09-08: four occurrences, of which one is the
    // declaration); a scan over the raw text would read whichever came first.
    let text = crate::loop_shape::uncommented(scxml);
    let marker = format!("id=\"{SUCCESSOR_CHECK}\"");
    let at = text.find(&marker).ok_or(Unreadable::Unauthored)?;
    let rest = &text[at..];
    let opening = "expr=\"";
    let expr_at = rest
        .find(opening)
        .ok_or_else(|| Unreadable::Unquoted(quoted(rest)))?;
    let body = &rest[expr_at + opening.len()..];
    // ⚠ The attribute is delimited by `"` and the argv inside it is written in SINGLE quotes, which
    // is what makes this end unambiguous — a document that spelled the argv in double quotes could
    // not be an XML attribute at all.
    let end = body
        .find('"')
        .ok_or_else(|| Unreadable::Unquoted(quoted(body)))?;
    let expr = &body[..end];
    let joined: String = expr
        .split('\'')
        .enumerate()
        .filter(|(nth, _)| nth % 2 == 1)
        .map(|(_, segment)| segment)
        .collect();
    if joined.split_whitespace().next().is_none() {
        return Err(Unreadable::Unquoted(quoted(expr)));
    }
    Ok(joined)
}

/// **THE DEPLOYED CLASSIFIER, READ INTO ITS PARTS.**
///
/// # Errors
///
/// Every arm of [`Unreadable`]. A document that names another kind of program is refused rather
/// than guessed at: what this reader is for is asking cargo about a cargo run.
pub fn deployed_classifier(scxml: &str) -> Result<Deployed, Unreadable> {
    let argv: Vec<String> = authored_argv(scxml)?
        .split_whitespace()
        .map(ToOwned::to_owned)
        .collect();
    // ⚠ BY FILE NAME, because the argv may spell cargo absolutely — and by POSITION, because the
    // word `cargo` also appears inside `CARGO_TARGET_DIR=…` one token earlier.
    let cargo_at = argv
        .iter()
        .position(|token| {
            Path::new(token)
                .file_name()
                .is_some_and(|name| name == "cargo")
        })
        .ok_or_else(|| Unreadable::NotACargoRun(argv.clone()))?;
    if argv.get(cargo_at + 1).map(String::as_str) != Some("run") {
        return Err(Unreadable::NotACargoRun(argv.clone()));
    }
    // Where cargo's own arguments end and the classifier's begin.
    let sep = argv
        .iter()
        .skip(cargo_at)
        .position(|token| token == "--")
        .map_or(argv.len(), |offset| cargo_at + offset);
    let cargos = &argv[cargo_at + 2..sep];
    let package = value_after(cargos, &["-p", "--package"])
        .ok_or_else(|| Unreadable::Unnamed {
            flag: "-p",
            argv: argv.clone(),
        })?
        .to_owned();
    let bin = value_after(cargos, &["--bin"])
        .ok_or_else(|| Unreadable::Unnamed {
            flag: "--bin",
            argv: argv.clone(),
        })?
        .to_owned();
    let target_dir = argv[..cargo_at]
        .iter()
        .find_map(|token| token.strip_prefix("CARGO_TARGET_DIR="))
        .map(PathBuf::from);
    let cargo_flags = cargos
        .iter()
        .filter(|token| token.starts_with('-'))
        .map(ToOwned::to_owned)
        .collect();
    Ok(Deployed {
        argv,
        target_dir,
        package,
        bin,
        cargo_flags,
    })
}

/// **WHAT CARGO SAID IT COMPILED**, off its own `Compiling` lines.
///
/// # ⚠⚠ Why the answer is cargo's report and not a dependency walk
///
/// A walk over `[dependencies]` would be this gate's reading of the graph, and the graph has three
/// more sections that reach a `--bin` build (`build-dependencies` and two shapes of
/// `target.'cfg(…)'.dependencies`). Cargo prints what it actually compiled, so the closure is read
/// off the run rather than re-derived — and a section nobody here thought of is still in it.
///
/// ⚠ The caller must build into an EMPTY target directory. Cargo prints nothing for a unit it did
/// not have to build, so a warm directory answers *nothing was compiled* about a closure that is
/// whatever it is.
///
/// # Errors
///
/// [`NothingCompiled`] for a log with no such line — never an empty set, which would read as
/// *nothing product-side is in the closure* about a question that was never put.
pub fn compiled_crates(log: &str) -> Result<BTreeSet<String>, NothingCompiled> {
    let compiled: BTreeSet<String> = log
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix("Compiling "))
        .filter_map(|rest| rest.split_whitespace().next())
        .map(ToOwned::to_owned)
        .collect();
    if compiled.is_empty() {
        return Err(NothingCompiled {
            said: log.lines().next().unwrap_or_default().to_owned(),
        });
    }
    Ok(compiled)
}

/// **THE CRATES IN THAT CLOSURE THAT ARE NOT THE CLASSIFIER'S OWN** — empty for a classifier
/// nothing product-side can break.
///
/// ⚠ A set rather than a yes-or-no, so a refusal can NAME them. Register item 461's measured
/// complaint about a bare count applies to a gate's message as much as to a run's row.
#[must_use]
pub fn foreign_to(compiled: &BTreeSet<String>, package: &str) -> BTreeSet<String> {
    compiled
        .iter()
        .filter(|name| name.as_str() != package)
        .cloned()
        .collect()
}

/// How many crates a refusal NAMES before summarising the rest.
///
/// ⚠ This crate's own doctrine, one gate over: `STALE_REPORT_CAP` caps a stale-binary report for
/// the measured reason that *a panic message pasting six hundred paths buries the sentence saying
/// what to do*. The mutation this module's gate is red under drags in **96** foreign crates, so the
/// cap is not hypothetical here.
pub const NAME_CAP: usize = 6;

/// A set of crate names written for a refusal to READ — the first [`NAME_CAP`] of them, then how
/// many more there are.
#[must_use]
pub fn named(crates: &BTreeSet<String>) -> String {
    let head: Vec<&str> = crates.iter().take(NAME_CAP).map(String::as_str).collect();
    let rest = crates.len().saturating_sub(head.len());
    if rest == 0 {
        head.join(", ")
    } else {
        format!("{}, …and {rest} more", head.join(", "))
    }
}

/// The value of the first of `flags` that appears in `argv` and is followed by something.
fn value_after<'a>(argv: &'a [String], flags: &[&str]) -> Option<&'a str> {
    argv.iter()
        .position(|token| flags.contains(&token.as_str()))
        .and_then(|at| argv.get(at + 1))
        .map(String::as_str)
}

/// The head of `text`, one line of it at most, for a refusal to quote.
fn quoted(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default().trim();
    match line.char_indices().nth(QUOTE_CAP) {
        Some((cut, _)) => format!("{}…", &line[..cut]),
        None => line.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Deployed, LOCKED, NAME_CAP, NothingCompiled, Unreadable, authored_argv, compiled_crates,
        deployed_classifier, foreign_to, named,
    };
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    /// A document shaped like the one this repository ships, small enough to read.
    fn document(expr: &str) -> String {
        format!(
            r#"<scxml><datamodel>
    <!-- A comment that names id="successor_check" and must not be read as one. -->
    <data id="reference" expr="'somewhere else'"/>
    <data id="successor_check" expr="{expr}"/>
</datamodel></scxml>"#
        )
    }

    #[test]
    fn a_document_that_authors_no_classifier_is_refused_by_name() {
        let why = authored_argv("<scxml><datamodel/></scxml>")
            .expect_err("a document with no such id must not read as a classifier");
        assert_eq!(why, Unreadable::Unauthored);
        assert!(
            why.describe().contains("authors no"),
            "⚠ a refusal that does not say what is missing sends a reader to this source: {}",
            why.describe(),
        );
    }

    /// ⚠⚠ THE CONTROL FOR THE ONE ABOVE, and the reason the fixture carries a comment: this
    /// document explains itself at length and names the id in prose, so a reader that skipped
    /// [`crate::loop_shape::uncommented`] would answer about a sentence.
    #[test]
    fn the_prose_that_names_the_id_is_not_read_as_the_id() {
        let argv = authored_argv(&document("'cargo run -p a --bin b'"))
            .expect("the declared id is the one read");
        assert_eq!(argv, "cargo run -p a --bin b");
    }

    #[test]
    fn a_classifier_that_is_not_a_cargo_run_is_refused_and_carried() {
        let why = deployed_classifier(&document("'/bin/echo YES'"))
            .expect_err("a program that is not cargo must not be read as one");
        assert!(
            matches!(why, Unreadable::NotACargoRun(_)),
            "got {why:?} for an argv that names /bin/echo",
        );
        assert!(
            why.describe().contains("/bin/echo"),
            "⚠ the refusal must carry what it read: {}",
            why.describe(),
        );
    }

    /// ⛔⛔⛔ `cargo` WITHOUT `run` IS NOT A CLASSIFIER THIS GATE CAN DRIVE, and the arm exists
    /// because `CARGO_TARGET_DIR=…` puts the word `cargo` in the argv one token earlier than the
    /// program does.
    #[test]
    fn the_word_cargo_in_the_environment_prefix_is_not_the_program() {
        let why = deployed_classifier(&document("'env CARGO_TARGET_DIR=/tmp/c /bin/echo YES'"))
            .expect_err("an environment variable holding the word must not pass for the program");
        assert!(matches!(why, Unreadable::NotACargoRun(_)), "got {why:?}");
    }

    #[test]
    fn a_cargo_run_that_names_no_package_is_refused_and_says_which_switch() {
        let why = deployed_classifier(&document("'cargo run -q --bin north-star'"))
            .expect_err("a run with no -p names no package");
        assert_eq!(
            why,
            Unreadable::Unnamed {
                flag: "-p",
                argv: ["cargo", "run", "-q", "--bin", "north-star"]
                    .iter()
                    .map(|t| (*t).to_owned())
                    .collect(),
            },
        );
    }

    #[test]
    fn a_cargo_run_that_names_no_binary_is_refused_and_says_which_switch() {
        let why = deployed_classifier(&document("'cargo run -q -p sprag-gate'"))
            .expect_err("a run with no --bin names no binary");
        assert!(
            matches!(why, Unreadable::Unnamed { flag: "--bin", .. }),
            "got {why:?}",
        );
    }

    /// The shape this repository actually ships, read whole — including that the classifier's own
    /// switches past `--` are NOT read as cargo's.
    #[test]
    fn the_shipped_shape_reads_into_its_parts() {
        let read = deployed_classifier(&document(
            "'env CARGO_TARGET_DIR=/home/c/.cache/a ' +\n\
             'cargo run -q --locked --manifest-path /w/Cargo.toml ' +\n\
             '-p sprag-gate --bin north-star -- --admits /w/ledger.md'",
        ))
        .expect("the shipped shape is readable");
        assert_eq!(
            read,
            Deployed {
                argv: [
                    "env",
                    "CARGO_TARGET_DIR=/home/c/.cache/a",
                    "cargo",
                    "run",
                    "-q",
                    "--locked",
                    "--manifest-path",
                    "/w/Cargo.toml",
                    "-p",
                    "sprag-gate",
                    "--bin",
                    "north-star",
                    "--",
                    "--admits",
                    "/w/ledger.md",
                ]
                .iter()
                .map(|t| (*t).to_owned())
                .collect(),
                target_dir: Some(PathBuf::from("/home/c/.cache/a")),
                package: "sprag-gate".to_owned(),
                bin: "north-star".to_owned(),
                cargo_flags: ["-q", "--locked", "--manifest-path", "-p", "--bin"]
                    .iter()
                    .map(|t| (*t).to_owned())
                    .collect(),
            },
        );
        assert!(
            read.cargo_flags.contains(LOCKED),
            "the fixture carries {LOCKED} so the assertion above is about a set that holds it",
        );
        assert!(
            !read.cargo_flags.contains("--admits"),
            "⛔ the classifier's OWN switch is past `--` and must not be read as cargo's, or a \
             document could satisfy a cargo-flag assertion with an argument to its program",
        );
    }

    /// ⛔⛔⛔ A LOG THAT COMPILED NOTHING IS A REFUSAL, never an empty closure — the vacuous green
    /// this workspace keeps meeting (register item 799).
    #[test]
    fn a_build_log_with_no_compiling_line_is_refused_rather_than_read_as_empty() {
        let why = compiled_crates("    Finished `dev` profile in 0.04s\n")
            .expect_err("a warm build says nothing about a closure");
        assert_eq!(
            why,
            NothingCompiled {
                said: "    Finished `dev` profile in 0.04s".to_owned(),
            },
        );
        assert!(
            why.describe().contains("Compiling"),
            "⚠ the refusal must name what it looked for: {}",
            why.describe(),
        );
    }

    /// The real shape cargo prints, both arms — one crate, and the hundred a dependency drags in.
    #[test]
    fn the_closure_is_read_off_cargos_own_lines() {
        let alone = compiled_crates(
            "   Compiling sprag-gate v0.0.1 (/home/coin/sprag/crates/sprag-gate)\n    Finished\n",
        )
        .expect("a log with one Compiling line is readable");
        assert_eq!(alone, ["sprag-gate".to_owned()].into_iter().collect());
        assert_eq!(foreign_to(&alone, "sprag-gate"), BTreeSet::new());

        let dragged = compiled_crates(
            "   Compiling proc-macro2 v1.0.106\n   Compiling regex v1.12.4\n   Compiling \
             sprag-vt v0.0.1 (/w/crates/sprag-vt)\n   Compiling sprag-gate v0.0.1 \
             (/w/crates/sprag-gate)\n",
        )
        .expect("a log with several is readable");
        assert_eq!(
            foreign_to(&dragged, "sprag-gate"),
            ["proc-macro2", "regex", "sprag-vt"]
                .iter()
                .map(|t| (*t).to_owned())
                .collect(),
            "⚠⚠ THE CONTRAST ARM: a reader that answered *nothing foreign* for this log would \
             answer it for every log, and the arm above would be green by construction",
        );
    }

    /// ⚠⚠ **A REFUSAL NAMES A FEW AND COUNTS THE REST**, which is this crate's `STALE_REPORT_CAP`
    /// doctrine: the mutation the gate is red under drags in 96 crates, and a message that pastes
    /// all of them buries the sentence saying what to put back.
    #[test]
    fn a_refusal_names_a_few_of_the_closure_and_counts_the_rest() {
        let few: BTreeSet<String> = ["b", "a"].iter().map(|t| (*t).to_owned()).collect();
        assert_eq!(named(&few), "a, b", "under the cap, every name is written");

        let many: BTreeSet<String> = (0..NAME_CAP + 4).map(|n| format!("c{n:02}")).collect();
        let said = named(&many);
        assert!(
            said.ends_with("…and 4 more"),
            "⚠ over the cap, the remainder is COUNTED rather than dropped: {said}",
        );
        assert_eq!(
            said.split(", ").count(),
            NAME_CAP + 1,
            "⚠⚠ and exactly {NAME_CAP} of them are named, or the cap is not a cap: {said}",
        );
    }
}
