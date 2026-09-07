//! **A JOB THAT RUNS THE SUITE MUST BUILD THE BINARIES THE SUITE DRIVES** — the claim `ci.yml`
//! could not make about itself.
//!
//! # ⚠⚠⚠ Why this file exists
//!
//! [`sprag_gate::sibling_bin`] answers *was this binary built from the source in this tree* by
//! reading cargo's own depfile beside it, and it refuses to pass when it cannot tell. That refusal
//! is right: the alternative is a suite that drives a stale daemon and reports green, which this
//! workspace has paid for three times.
//!
//! What nobody measured is where that depfile comes from. **`cargo test` UPLIFTS
//! `target/debug/sprag-term` and writes no `sprag-term.d` beside it; only `cargo build` writes the
//! uplifted depfile.** So a CI job whose only cargo step is `cargo test` leaves the guard with
//! nothing to read, and the guard correctly refuses the entire pty and MCP suite — which is what
//! the Linux job did in the commit that introduced the guard, while the macOS job (which has always
//! had a build step) failed for an unrelated reason and hid the pattern.
//!
//! # ⚠⚠ Why a LINE SCAN and not a YAML parse, said plainly
//!
//! This crate takes no dependencies by charter, and there is no YAML reader in std. A scan cannot
//! understand `ci.yml`; what it can do is answer one narrow question — *does the text of this job
//! invoke `cargo build` before it invokes `cargo test`* — and that is the whole claim. It is
//! stated here rather than implied so nobody reads a green run as *the workflow is correct*.
//!
//! ⚠ It is also why the job boundary is found by INDENTATION rather than by structure: a job's key
//! sits at exactly four spaces under `jobs:`, and anything deeper belongs to it.

use std::path::PathBuf;

/// The workflow this repository's gates run in.
fn workflow() -> String {
    let path: PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "..",
        "..",
        ".github",
        "workflows",
        "ci.yml",
    ]
    .iter()
    .collect();
    std::fs::read_to_string(&path)
        .unwrap_or_else(|why| panic!("{} is the workflow under test: {why}", path.display()))
}

/// Every job in `ci.yml`, as `(name, its own lines)`.
///
/// A job's key is indented exactly four spaces under `jobs:`; every deeper line is that job's until
/// the next such key. Comments and blanks travel with whichever job they sit in, which is harmless
/// here — a commented-out `cargo build` would be a false positive, so they are stripped.
fn jobs(text: &str) -> Vec<(String, Vec<String>)> {
    let mut found: Vec<(String, Vec<String>)> = Vec::new();
    let mut in_jobs = false;
    for line in text.lines() {
        if line.starts_with("jobs:") {
            in_jobs = true;
            continue;
        }
        if !in_jobs {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        // A top-level key ends the `jobs:` block entirely.
        if indent == 0 {
            in_jobs = false;
            continue;
        }
        if indent == 2 && trimmed.ends_with(':') && !trimmed.starts_with('#') {
            found.push((trimmed.trim_end_matches(':').to_owned(), Vec::new()));
            continue;
        }
        // ⚠ COMMENTS ARE DROPPED. This file explains itself at length, and several of those
        // paragraphs quote the very commands being searched for — so a scan that read them would
        // find `cargo build` in prose and pass a job that runs none.
        if trimmed.starts_with('#') {
            continue;
        }
        if let Some((_, lines)) = found.last_mut() {
            lines.push(trimmed.to_owned());
        }
    }
    found
}

/// ⚠⚠⚠ **EVERY JOB THAT RUNS `cargo test` RUNS `cargo build` FIRST.**
///
/// Not a style rule. The suite spawns `sprag-term` and `sprag-tui` as PROCESSES, and
/// [`sprag_gate::sibling_bin`] refuses a binary whose build record is missing — which `cargo test`
/// alone does not write. A job without the build step does not run a weaker suite; it runs NO
/// suite, and says so in a hundred identical panics.
///
/// ⚠ THE ORDER is part of the claim: a build AFTER the test proves nothing about the test.
#[test]
fn every_job_that_runs_the_suite_builds_the_binaries_it_drives() {
    let text = workflow();
    let mut checked = 0_usize;
    for (name, lines) in jobs(&text) {
        let Some(tests_at) = lines.iter().position(|line| line.contains("cargo test")) else {
            continue;
        };
        checked += 1;
        let builds_at = lines
            .iter()
            .position(|line| line.contains("cargo build"))
            .unwrap_or_else(|| {
                panic!(
                    "⚠⚠⚠ the {name:?} job runs `cargo test` and never runs `cargo build`. The \
                     suite drives sibling binaries as processes and `sprag_gate::sibling_bin` \
                     reads cargo's depfile to answer whether they are current — a depfile only \
                     `cargo build` writes beside an uplifted binary. Without that step the guard \
                     cannot tell, refuses correctly, and the whole pty and MCP suite fails on this \
                     runner alone."
                )
            });
        assert!(
            builds_at < tests_at,
            "⚠⚠ the {name:?} job builds AFTER it tests, which is the same as not building: the \
             depfile the guard reads has to exist when the suite starts, not when it is over",
        );
    }
    // THE CONTROL. Every assertion above is satisfied by a scan that found no jobs at all — which
    // is exactly what a changed indentation or a renamed `jobs:` key would produce.
    assert!(
        checked >= 2,
        "this scan found only {checked} job(s) running the suite, and this workflow has more than \
         one. The parse has stopped seeing the file rather than the file having changed.",
    );
}

/// The binaries a promotion moves, read out of the product's own list.
///
/// ⚠ The LITERAL in `promotion.rs` and not a copy: this crate takes no dependencies by charter, so
/// it cannot import `sprag_host::promotion::IMAGES` — but it can read the line that declares it,
/// which is what every reader in this crate does with the product's Rust. A fifth image arrives
/// here with no edit; a renamed constant empties this and the control below says so.
fn images() -> Vec<String> {
    // ⚠ THROUGH THE DOOR, for `provider`'s reason one function down — register item 809.
    let path = sprag_gate::sources::workspace_root().join("crates/sprag-host/src/promotion.rs");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|why| panic!("{} declares the image set: {why}", path.display()));
    text.lines()
        .find(|line| line.contains("pub const IMAGES"))
        .map(|line| {
            line.split('"')
                .skip(1)
                .step_by(2)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Which workspace package provides the binary `image`, discovered from the tree.
///
/// ⚠⚠ DISCOVERED AND NOT MAPPED. A hand-written table of *binary to crate* is the place that leaks
/// (register items 80, 762 and 945, which is the item that measured it), and this relation is
/// already written down twice in the tree: as `[[bin]] name = "…"` in a manifest, and as
/// `src/bin/<name>.rs` beside it. Both spellings are read, because this workspace uses both — the
/// `sprag` and `sprag-term` binaries are files under `sprag-host`, while `sprag-gui` and
/// `sprag-mcp` are declared sections.
fn provider(image: &str) -> Option<String> {
    // ⚠⚠ THROUGH THE DOOR — register item 809, whose gate caught this function's first draft
    // walking up with `CARGO_MANIFEST_DIR` and a `..`. That root is baked in at COMPILE time, so a
    // gate run in one tree could judge another; `workspace_root` compares the two and refuses when
    // they differ. ⚠ `workflow()` at the top of this file still bakes one in and 809's gate does
    // not flag it — that is this file's remaining work and not this round's, but nothing new here
    // adds to it.
    let crates = sprag_gate::sources::workspace_root().join("crates");
    for entry in std::fs::read_dir(&crates).expect("the workspace's crates directory") {
        let dir = entry.expect("a readable entry").path();
        let manifest = dir.join("Cargo.toml");
        let Ok(text) = std::fs::read_to_string(&manifest) else {
            continue;
        };
        let declared = text
            .lines()
            .any(|line| line.trim() == format!("name = \"{image}\""))
            && text.contains("[[bin]]");
        let filed = dir.join("src").join("bin").join(format!("{image}.rs"));
        if declared || filed.is_file() {
            let name = text
                .lines()
                .find_map(|line| line.trim().strip_prefix("name = \""))
                .and_then(|rest| rest.strip_suffix('"'))?;
            return Some(name.to_owned());
        }
    }
    None
}

/// ⛔⛔⛔⛔⛔ **AND IT BUILDS *WHICH* BINARIES, WHICH IS WHAT THIS FILE'S OWN TITLE CLAIMED AND ITS
/// PREDICATE DID NOT ASK** — register item 948.
///
/// # ⛔⛔⛔⛔ The gate above was true and the claim above it was wider
///
/// `every_job_that_runs_the_suite_builds_the_binaries_it_drives` asks whether a job runs
/// `cargo build` **at all**, before `cargo test`. Both headless jobs did — `cargo build --workspace
/// --exclude sprag-gui` — so both were green here while neither built `target/debug/sprag-gui`, and
/// two suites in `sprag-host`'s `cli.rs` walk `promotion::IMAGES` (four of them) through
/// `sibling_bin`, which panics on a binary that is not there. **Measured 2026-09-07** on runs
/// 34124187331 and 34128530278: `every_image_a_promotion_moves_says_which_build_it_is` and
/// `the_door_can_ask_every_image_once_a_promotion_has_moved_them` FAILED in the Linux job and in
/// the macOS job, on every push, for as long as anybody had not read the hosted result.
///
/// ⇒ This is the module title made into a question the workflow can be asked: for each image, does
/// this job's build reach the package that provides it?
///
/// ⚠⚠ **AND IT IS LOCALLY INVISIBLE, WHICH IS WHY IT HAS TO BE A GATE OVER THE WORKFLOW.** A
/// developer's tree has all four binaries, so the two tests pass here and would pass in any
/// sandbox that ran `cargo build --workspace`. The only place the defect exists is a job
/// description, and this crate's charter is *the gates a test cannot be*.
///
/// ⚠ A `--workspace` build covers a package unless it is `--exclude`d; a `-p` build covers exactly
/// what it names. Those two spellings are the whole of the reading, and a third would be a new
/// arm somebody writes rather than a scan that guessed.
#[test]
fn every_job_that_runs_the_suite_builds_each_image_a_promotion_moves() {
    let text = workflow();
    let images = images();
    assert_eq!(
        images.len(),
        4,
        "⚠⚠⚠ THE PRODUCT'S OWN LIST could not be read — found {images:?}. Every assertion below is \
         vacuous without it, which is item 924's shape: a scan of nothing is green for the wrong \
         reason. `sprag_host::promotion::IMAGES` is where this population lives",
    );

    let mut missing = Vec::new();
    let mut checked = 0_usize;
    for (job, lines) in jobs(&text) {
        // ⛔⛔⛔⛔⛔ THE POPULATION IS THE JOBS THAT CAN REACH THOSE SUITES, AND NARROWING IT WAS
        // FORCED BY THIS GATE'S OWN FIRST DRAFT — which asked it of every job running any
        // `cargo test` and duly convicted `pixel-linux` for not building `sprag-mcp`. That job
        // runs `cargo test -p sprag-gui`, whose tests drive no sibling binary at all, so the
        // finding was the gate over-claiming: **exactly the defect this file was opened to repair,
        // one level up.** A gate whose claim is wider than its subject reds on work that is right.
        //
        // ⚠⚠ The two suites that walk `promotion::IMAGES` live in `sprag-host`, so a job reaches
        // them by sweeping the workspace or by naming that package. Both spellings are read; a
        // third selection would be an arm somebody writes rather than a scan that guessed.
        let drives = |line: &&String| {
            line.contains("cargo test")
                && (line.contains("--workspace") || line.contains("-p sprag-host"))
        };
        if !lines.iter().any(|line| drives(&line)) {
            continue;
        }
        checked += 1;
        let builds: Vec<&String> = lines
            .iter()
            .filter(|line| line.contains("cargo build"))
            .collect();
        for image in &images {
            let package = provider(image).unwrap_or_else(|| {
                panic!(
                    "⛔ REGISTER ITEM 948: nothing in this workspace declares a binary named \
                     {image:?}, and `promotion::IMAGES` says a promotion moves one. Either the \
                     product's list or the tree has moved without the other",
                )
            });
            let built = builds.iter().any(|line| {
                (line.contains("--workspace") && !line.contains(&format!("--exclude {package}")))
                    || line.contains(&format!("-p {package}"))
            });
            if !built {
                missing.push(format!(
                    "  the {job:?} job runs the suite and never builds `{image}` (package \
                     `{package}`). Its build line(s): {builds:?}"
                ));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 948: a job runs the sweep without building a binary the sweep \
         DRIVES. `sibling_bin` panics on a binary that is not there — correctly — so those suites \
         do not run a weaker check, they run NONE, and they say so on every push in a log nobody \
         has to read. This is invisible locally, because a developer's tree has all four:\n{}",
        missing.join("\n"),
    );
    // THE CONTROL, and it is the same one the gate above needs: every assertion here is satisfied
    // by a scan that found no jobs running the suite at all.
    assert!(
        checked >= 2,
        "this scan found only {checked} job(s) running the suite, and this workflow has more than \
         one — the parse has stopped seeing the file",
    );
}
