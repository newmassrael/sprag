//! ⛔⛔⛔⛔⛔ **WHAT A COMMENT IS, COUNTED RATHER THAN ASSERTED** — register items 1051 and 1055.
//!
//! # The number this exists because a doc claimed
//!
//! Item 1051 was paid by moving one gate off `line.trim_start().starts_with("//")` and onto
//! [`sprag_gate::rust_source`], which knows the other two comment shapes and the five literal
//! shapes that defeat a scanner keyed on `//` alone. The doc written in the same commit said *the
//! same approximation is written at ten call sites in six files*.
//!
//! **Six was wrong; the walk answers seven.** The count was taken by eye off a grep whose output
//! also held a `starts_with("///")` and the site being replaced, and nobody could re-take it — which
//! is the register's own rule 10 arriving on the round that had just spent itself on rule 10. So
//! the number stops being prose here.
//!
//! # ⚠⚠ An EQUALITY, because this one has to fall
//!
//! A ceiling catches the eleventh copy and says nothing when the tenth is paid off — and a ratchet
//! that only notices growth loses its grip one repayment at a time. An equality is one edit in
//! either direction and refuses with the number to write, so a copy added is a stop and a copy
//! removed is a recorded step (register item 926).
//!
//! ⚠ The remaining sites are NOT interchangeable, which is why item 1055 exists rather than one
//! more commit here: [`sprag_gate::sources::code_lines`] drops `#` lines beside `//` ones and the
//! shape of what it feeds is a second question, `mcp_stdio.rs` reads a config format that is not
//! Rust at all, and moving them by rote is the mechanical edit this register keeps refusing.

use sprag_gate::sources::rust_sources;

/// How many code lines under `crates/` still spell *a comment* for themselves.
///
/// MEASURED 2026-09-12 by the walk below, at the commit that paid item 1051. It was **eleven**
/// before that commit; the site it moved is the difference.
const HAND_ROLLED_COMMENT_FILTERS: usize = 10;

/// How many FILES those sites are spread over — the half of the doc's claim that was wrong.
const FILES_THEY_LIVE_IN: usize = 7;

/// Every code line that decides for itself what a comment is, as `path:line`.
///
/// # ⛔⛔⛔⛔⛔ The needle was ASSEMBLED out of two halves, and the mutation said it bought nothing
///
/// The reasoning was the one this workspace keeps meeting — spelled out, a gate is answered by its
/// own text, and `the_pane_clause_spends_the_verdict_it_was_given` is where that was learned. It
/// does not reach here, and running it is how that was found rather than argued: spelling the
/// needle out left the count at **ten**, unmoved. A Rust string literal holding this needle carries
/// ESCAPED quotes — `starts_with(\"//\")` in the file's bytes — so it cannot match itself. Only a
/// RAW string could, and the gate would be right to count one.
///
/// ⚠ [`rust_sources`] hands back the lines that are CODE, which is what makes the paragraphs above
/// — and the module doc of [`sprag_gate::rust_source`], which quotes the needle to explain it —
/// invisible here. A scan that read its own reasoning as the offence would go red on the fix.
fn hand_rolled_sites() -> Vec<String> {
    let needle = "starts_with(\"//\")".to_owned();
    let mut found: Vec<String> = Vec::new();
    for source in rust_sources() {
        for (at, line) in &source.code {
            if line.contains(&needle) {
                found.push(format!("{}:{at}", source.file));
            }
        }
    }
    found
}

/// ⚠⚠ **Both numbers, because they fail differently.** A copy moved from one file into another
/// leaves the site count alone and the file count is what sees it; a second copy added to a file
/// that already has one leaves the file count alone. Neither alone can say the population did not
/// move.
#[test]
fn every_hand_rolled_comment_filter_is_counted_and_the_count_is_one_edit_to_change() {
    let sites = hand_rolled_sites();
    assert!(
        !sites.is_empty(),
        "⛔ ITEM 1055: the walk found NO site spelling a comment for itself, which on this tree \
         means the scan is pointed somewhere other than where the code is — a probe pointed at \
         nothing must never read as clean",
    );

    let mut files: Vec<&str> = sites
        .iter()
        .map(|site| site.split(':').next().expect("a site carries a path"))
        .collect();
    files.sort_unstable();
    files.dedup();

    assert_eq!(
        sites.len(),
        HAND_ROLLED_COMMENT_FILTERS,
        "⛔ ITEM 1055: {} code line(s) decide for themselves what a comment is, and this gate is \
         written against {HAND_ROLLED_COMMENT_FILTERS}. UP is an eleventh copy of the \
         approximation item 1051 paid off — use `sprag_gate::rust_source` instead. DOWN is one of \
         them paid, so write `HAND_ROLLED_COMMENT_FILTERS = {}` and say in the register which \
         site moved.\nsites:\n  {}",
        sites.len(),
        sites.len(),
        sites.join("\n  "),
    );

    assert_eq!(
        files.len(),
        FILES_THEY_LIVE_IN,
        "⛔ ITEM 1055: those sites are spread over {} file(s) against the {FILES_THEY_LIVE_IN} this \
         gate is written for. Write `FILES_THEY_LIVE_IN = {}`.\nfiles:\n  {}",
        files.len(),
        files.len(),
        files.join("\n  "),
    );
}

/// ⛔⛔⛔⛔⛔ **AND THE SITES ARE NAMED, not merely counted** — a count that happens to be ten is
/// passed by ten sites anywhere, including ten this gate has never been about.
///
/// The widest-reach one is named here because it is the one item 1055 is FOR:
/// [`sprag_gate::sources::code_lines`] is what `Source::code` is built from, so every gate in this
/// crate that reads code rather than prose reads through that spelling. A round that moved the
/// other nine and left this one would satisfy a bare count at nine and have paid the least of it.
#[test]
fn the_widest_reach_site_is_named_so_the_count_cannot_stand_in_for_it() {
    let sites = hand_rolled_sites();
    let widest: Vec<&String> = sites
        .iter()
        .filter(|site| site.starts_with("crates/sprag-gate/src/sources.rs:"))
        .collect();
    assert_eq!(
        widest.len(),
        4,
        "⛔ ITEM 1055: `sources.rs` carries {} of the sites against the 4 measured 2026-09-12. It \
         is the one `Source::code` is built from, so it is the site the others are waiting on — if \
         it moved, write the new number here and in the register.\nthere:\n  {}",
        widest.len(),
        sites.join("\n  "),
    );
}
