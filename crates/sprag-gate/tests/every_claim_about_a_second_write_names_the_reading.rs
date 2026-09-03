//! ⛔⛔⛔⛔⛔ **EVERY SENTENCE IN THIS WORKSPACE THAT SAYS WHAT A SECOND WRITE INTO A HELD COMPOSER
//! DOES NAMES THE READING THAT MEASURED IT** — register item 830.
//!
//! # ⛔⛔⛔⛔⛔ What went wrong: one answer given to two roads, for a fortnight
//!
//! `deliver` has two roads out of a composer that is already holding a prompt, and it gives them
//! different words. On `OnScreen::Shown` the text is PAINTED and the answer is
//! `Delivered::Unsubmitted`; on `OnScreen::MovedWithoutIt` the composer FOLDED the paste away and
//! the answer is `Delivered::Unreported`. **Both roads were described by one sentence** — *"a
//! second delivery would concatenate onto it"* — written 2026-08-19, and it was the whole argument
//! for `Unsubmitted` refusing to retry.
//!
//! It was never measured. Item 421 had measured the OTHER road the day before and got the opposite
//! answer (`Confirmed { attempts: 2, written: 2477 }`, the fold expanded, the agent working), and
//! the two readings sat one file apart for two weeks without either one being asked about the
//! other. Item 830 is the owner reading the transcript and saying *"이건 잘못된 조사 같은데"*.
//!
//! **Measured 2026-09-03 against `claude` 2.1.259, twice**, by
//! `sprag_host::live_agent::what_a_second_write_into_a_composer_already_holding_one_does`: a second
//! write of the identical bytes EXPANDS a folded composer (placeholder gone, the prompt's head
//! readable ONCE) and CONCATENATES onto a painted one (the text on the screen TWICE). So each
//! sentence was right about its own road and wrong about the other's, and nothing in the tree said
//! which road it was talking about.
//!
//! ⚠ This file is in its own population and cites the reading like everything else it judges — a
//! checker exempt from its own rule is the escape hatch this workspace's rule 6 is about.
//!
//! # ⚠⚠⚠ Why the rule is CITE THE READING rather than SAY THE RIGHT WORD
//!
//! A gate that required the word `expand` here and `concatenate` there would be a second authority
//! on the answer: it would have to be edited every time the peer changes, and until somebody edited
//! it the tree would be held to a reading nobody had retaken. What does not rot is the OBLIGATION —
//! a claim about what a live composer does must point at the live gate that read it, and that gate
//! carries the numbers and goes red on the day the peer stops behaving that way.
//!
//! This is the same shape as `an_economic_edge_carries_the_population_it_was_measured_in`: the
//! claim may say anything, and it must say where it was measured.
//!
//! # ⚠⚠ The population is the tree's own, and there is no exemption list
//!
//! A block joins by SAYING the thing — naming a second write and naming what it does — so a
//! sentence written tomorrow is judged for being written rather than for being registered. This
//! workspace's rule 6: an unclassified block is a RED and not a pass. ⚠ The one thing that would
//! disarm this quietly is an extraction that matches nothing, so the count is asserted from below.

use std::path::{Path, PathBuf};

/// The live gate that took the reading — named as TEXT because it is a `#[cfg(test)]` item in
/// another crate, which is not a thing an intra-doc link can reach (the neighbouring sweep gate
/// paid for that lesson).
const READING: &str = "what_a_second_write_into_a_composer_already_holding_one_does";

/// **NAMING THE ACT**: a write that follows one the same composer already took.
///
/// ⚠ Phrases and not the bare word `second`, which is a unit of time in half this workspace.
const ACT: [&str; 7] = [
    "second delivery",
    "second write",
    "second injection",
    "second paste",
    "next delivery",
    "writing again",
    "write again",
];

/// **NAMING WHAT IT DOES TO THE BOX** — the two answers item 830 measured, and the word for the
/// first one either way round.
const DOES: [&str; 4] = ["concatenat", "expand", "un-fold", "unfold"];

/// Every `.rs` file under `crates/`, as `(path relative to the root, text)`.
fn rust_sources() -> Vec<(String, String)> {
    let root = sprag_gate::sources::workspace_root();
    let mut paths = Vec::new();
    walk(&root.join("crates"), &mut paths);
    paths.sort();
    // ⚠ A probe pointed at nothing must never read as clean — `sources::rust_sources`' own rule,
    // restated here because this walk is this file's and not that one's.
    assert!(
        paths.len() > 100,
        "a scan that found only {} sources is pointed at the wrong tree",
        paths.len(),
    );
    paths
        .into_iter()
        .map(|path| {
            let file = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .display()
                .to_string();
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|why| panic!("{file} is a source of this workspace: {why}"));
            (file, text)
        })
        .collect()
}

fn walk(dir: &Path, into: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name == "target") {
                continue;
            }
            walk(&path, into);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            into.push(path);
        }
    }
}

/// One run of consecutive comment lines, flattened to a single line.
///
/// ⚠⚠ **A BLOCK AND NOT A LINE**, which is the neighbouring document gates' lesson in this crate's
/// own terms: an author wraps a sentence wherever rustfmt puts the margin, so *"the text was never
/// written again"* is three lines here and two lines one commit later. A line-wise filter reports a
/// claim as uncited while its citation sits on the line below it.
struct Block {
    /// One-indexed line where the run starts, so a message is a place a person can open.
    at: usize,
    /// Every comment line in the run, whitespace flattened and joined with single spaces.
    text: String,
}

fn comment_blocks(source: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut run: Vec<&str> = Vec::new();
    let mut at = 0;
    for (index, line) in source.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("//") {
            if run.is_empty() {
                at = index + 1;
            }
            run.push(trimmed);
        } else if !run.is_empty() {
            blocks.push(Block {
                at,
                text: run
                    .join(" ")
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" "),
            });
            run.clear();
        }
    }
    if !run.is_empty() {
        blocks.push(Block {
            at,
            text: run
                .join(" ")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
        });
    }
    blocks
}

#[test]
fn every_claim_about_what_a_second_write_does_names_the_live_reading() {
    let mut population = Vec::new();
    let mut uncited = Vec::new();
    for (file, text) in rust_sources() {
        for block in comment_blocks(&text) {
            let said = block.text.to_lowercase();
            if !ACT.iter().any(|act| said.contains(act)) {
                continue;
            }
            if !DOES.iter().any(|does| said.contains(does)) {
                continue;
            }
            let where_it_is = format!("{file}:{}", block.at);
            if !block.text.contains(READING) {
                uncited.push(format!("{where_it_is} — {}", head(&block.text)));
            }
            population.push(where_it_is);
        }
    }

    // ⚠⚠⚠⚠ **THE CONTROL, FIRST.** Every assertion below is vacuous over an empty population, and
    // the way this gate dies quietly is a rename that stops the extraction matching — not an author
    // deleting a citation. Five is the reading taken 2026-09-03; the floor is deliberately under it
    // so that removing a claim is allowed and removing the INSTRUMENT is not.
    assert!(
        population.len() >= 5,
        "⛔⛔⛔⛔⛔ THE EXTRACTION HAS STOPPED FINDING THE CLAIMS. Seven blocks in this workspace \
         said what a second write into a held composer does when item 830 was paid (2026-09-03), \
         and this run found {}. Either the phrases moved — {ACT:?} and {DOES:?} — or this gate is \
         pointed at a tree that does not carry them. A gate that matches nothing passes forever. \
         Found: {population:?}",
        population.len(),
    );

    // ══ ⛔⛔⛔⛔⛔ AND THE RULE — register item 830 ═══════════════════════════════════════════════
    assert!(
        uncited.is_empty(),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 830: {} block(s) say what a second write into a composer that is \
         already holding one does, and do not name the reading that measured it. What that costs \
         is not hypothetical — one such sentence described BOTH of `deliver`'s roads for a \
         fortnight and was the whole argument for `Delivered::Unsubmitted` never retrying, while a \
         live reading one file away said the opposite about the other road. A live composer's \
         behaviour is not derivable from this source; say where it was read.\n\
         \n  The reading: `sprag_host::live_agent::{READING}`\n  \
         Run it: cargo test -p sprag-host --lib {READING} -- --ignored --nocapture\n\
         \n  Uncited:\n    {}",
        uncited.len(),
        uncited.join("\n    "),
    );
}

/// The first 120 characters of a block, so a message names the sentence without reprinting an essay.
fn head(text: &str) -> String {
    text.chars().take(120).collect()
}
