//! ⛔⛔⛔⛔ **EVERY WIRE WORD THE HOST PUBLISHES HAS SOMETHING THAT READS IT** — register item 1022.
//!
//! # What happened twice, and why the second time earned a gate
//!
//! Item 1018 found `PANE_DRIVEN_KEY` published from a field that was a constant `false`: a key that
//! could not become true. Item 1021 found its pair — `PANE_BORNE_BY_KEY` published CORRECTLY and
//! read by nobody. Since 2026-08-19 the host had joined the run registry on every pane listing to
//! compute it (a lock taken, a snapshot walked) and written the answer onto a wire where no reader
//! existed. Three weeks.
//!
//! ⚠⚠ **Both were found by a person counting `git grep` hits by hand**, which is item 1022. Not
//! one suite could have noticed: a fixture that publishes a key writes it and asserts on it in the
//! same file, so the round-trip is green whether or not the word means anything to anybody else.
//!
//! # ⚠⚠⚠⚠⚠ Why this cannot be a test inside `sprag-host`
//!
//! The claim is *nothing anywhere reads this*, and a test can only name symbols that exist. There
//! is no symbol for the reader nobody has written — [`sprag_gate::published`] says the same thing
//! about a wire word that [`sprag_gate::authored`] says about a kind's number, one document over.
//!
//! # ⚠⚠⚠ The population was decided by measurement, and it is not a prefix
//!
//! Item 1022 was measured on one slot's `PANE_*` keys and asked whether the population is only
//! those. It is not: the subject is every string constant the host crate EXPORTS and that shipping
//! code writes as a JSON member name — 152 of 294, measured 2026-09-10 at `e025e615`. Widening to
//! all of them costs **zero exemptions**, `RUN_*` keys included, and that is the whole reason the
//! wide population is the honest one. A prefix would have been a choice with nothing behind it, and
//! a wide population passed by an exemption list would have made the exemption the gate.

use std::collections::BTreeSet;

use sprag_gate::published::{VOCABULARY, Word, declared, published};
use sprag_gate::sources::rust_sources;

/// Where the host declares the words it publishes — measured 2026-09-10 at `e025e615`.
///
/// # ⚠⚠⚠⚠ Why a glob needed a pin of its own
///
/// [`VOCABULARY`] is a PREFIX over the whole crate rather than the two modules that hold the
/// vocabulary today, because a list with no glob decides alone (item 470). But a glob answers a
/// question this gate never asks: *how many places declare this wire?* Its union is green whether
/// it read four files or one, so the SET is pinned — a fifth module starting to publish is a
/// person's decision, not a fact for a union to absorb, and a declaration parse that has stopped
/// matching is announced rather than reported as a clean tree.
const DECLARED_IN: &[&str] = &[
    "crates/sprag-host/src/events.rs",
    "crates/sprag-host/src/hooks.rs",
    "crates/sprag-host/src/plugins.rs",
    "crates/sprag-host/src/wire.rs",
];

/// The pane-entry keys item 1022 was measured on, in the order the scan reports them.
///
/// ⚠ The register named five; its own command matches these SIX, because the pane entry's address
/// key is spelled the same way as the five that describe the pane. Re-deriving the item's
/// measurement is what this pin is for, so it holds what the command yields rather than what the
/// prose summarised.
///
/// ⚠⚠ It is an equality, and the churn is deliberate — [`sprag_gate::authored`]'s `CLAIMED` makes
/// the same trade for the same reason. Measured ABOVE the pin: a seventh pane key arrived, and the
/// question *who reads it* is due in that commit rather than three weeks later. Measured BELOW:
/// either a key went away on purpose or **the scan stopped seeing this family**, which the gate
/// beside this one cannot tell you, because a scan that finds nothing reports no offences.
const MEASURED_BY_1022: &[&str] = &[
    "PANE_BORNE_BY_KEY",
    "PANE_DRIVEN_KEY",
    "PANE_LEFT_BY_KEY",
    "PANE_REVIVED_KEY",
    "PANE_SESSION_KEY",
    "PANE_SUMMARY_ID_KEY",
];

/// ⛔⛔⛔⛔⛔ **A WORD THIS HOST PUBLISHES AND NOTHING READS IS A COMPUTATION NOBODY WANTED.**
#[test]
fn every_word_the_host_publishes_is_read_somewhere_that_does_not_write_it() {
    let words = published(&rust_sources());

    // ⚠ A probe pointed at nothing must never read as clean — `rust_sources`' own rule. The pins
    // below are what watch for a needle going blind; this is what watches for the walk being
    // pointed somewhere else entirely.
    assert!(
        words.len() > 100,
        "a scan that found only {} published word(s) under `{VOCABULARY}` is not looking at this \
         host's wire — 152 were measured at `e025e615`, and everything below would pass over an \
         almost empty set",
        words.len(),
    );

    let unread: Vec<String> = words
        .iter()
        .filter(|word| word.reads.is_empty())
        .map(unread_line)
        .collect();

    assert!(
        unread.is_empty(),
        "⛔⛔⛔⛔ REGISTER ITEM 1022: {} word(s) below are computed, published on this wire, and \
         READ BY NOTHING IN THIS WORKSPACE. That is what `PANE_BORNE_BY_KEY` did for three weeks \
         while every suite stayed green (item 1021), and what four `MOUSE_*_FIELD` words were \
         doing when this gate first ran — `mouse_args` wrote them through the constants and \
         `parse_mouse_args` read `\"button\"` and `\"kind\"` as literals, so a rename would have \
         reached the writer and left the parser behind.\n\
         Each line is one word, where it is published, and what to do with it: give it a reader, or \
         stop publishing it. A THIRD answer exists and is the one to check first — the reader may \
         be there already, spelling the word by hand, which is item 559 seen from the other side.\n\
         {}",
        unread.len(),
        unread.join("\n"),
    );
}

/// One unread word, as a line a person can act on without re-running anything.
fn unread_line(word: &Word) -> String {
    let names = if word.spelled.is_empty() {
        word.name.clone()
    } else {
        format!(
            "{} (also spelled {})",
            word.name,
            word.spelled
                .iter()
                .cloned()
                .collect::<Vec<String>>()
                .join(", ")
        )
    };
    let sites: Vec<String> = word
        .writes
        .iter()
        .map(std::string::ToString::to_string)
        .collect();
    format!(
        "  `{names}` — declared {}, published at {} and read nowhere",
        word.declared_in,
        sites.join(", "),
    )
}

/// ⚠⚠⚠⚠⚠ **AND WHICH FILES DECLARE THIS WIRE IS PINNED**, so a glob that widened says so.
#[test]
fn which_files_the_host_declares_a_published_word_in_is_what_this_gate_can_still_see() {
    let words = published(&rust_sources());
    let found: Vec<String> = words
        .iter()
        .map(|word| word.declared_in.clone())
        .collect::<BTreeSet<String>>()
        .into_iter()
        .collect();
    let pinned: Vec<String> = DECLARED_IN.iter().map(|file| (*file).to_owned()).collect();

    assert_eq!(
        found, pinned,
        "⚠⚠⚠⚠⚠ EITHER THIS WIRE'S VOCABULARY MOVED OR THE DECLARATION SCAN WENT BLIND, and it \
         cannot tell which.\n\
         MORE than the pin: a module of the host crate started publishing words of its own. Check \
         that each has a reader — the gate beside this one does — and add the file here.\n\
         FEWER than the pin: a module stopped declaring published words, OR `pub const NAME: \
         &str = …` stopped being how one is written and the parse walked past every line of it. \
         The first is a change somebody meant; the second is this scan quietly ceasing to work, \
         which is register item 453's finding.\n\
         ⚠ The judge's own crate is never here: it declares no dependencies and holds no wire, \
         while its text quotes every needle this hunts.",
    );
}

/// ⚠⚠⚠⚠⚠ **AND THE FAMILY ITEM 1022 WAS MEASURED ON IS RE-DERIVED EVERY RUN.**
///
/// # Why the gate above cannot answer this
///
/// Item 470's finding, the sharpest that round produced: blinding the needle left the ratchet
/// GREEN, because a needle that sees nothing reports no offences. *Does the gate pass?* and *does
/// the gate still SEE?* are different questions, and only a pinned measurement answers the second.
/// This one is the item's own: the pane keys it counted by hand must still be words this scan finds
/// published, and each must still have the reader that closed item 1021.
#[test]
fn the_pane_keys_item_1022_counted_are_still_words_this_gate_finds_published() {
    let words = published(&rust_sources());
    // The register's own needle, `^pub const PANE_[A-Z_]*_KEY`, which wants a MIDDLE: the bare
    // `PANE_KEY` several ask types carry is an event subject's address rather than one of this
    // entry's descriptions, and the command that produced the item's measurement never matched it.
    let family: Vec<&Word> = words
        .iter()
        .filter(|word| {
            word.name
                .strip_prefix("PANE_")
                .and_then(|rest| rest.strip_suffix("_KEY"))
                .is_some_and(|middle| !middle.is_empty())
        })
        .collect();
    let found: Vec<String> = family.iter().map(|word| word.name.clone()).collect();
    let pinned: Vec<String> = MEASURED_BY_1022
        .iter()
        .map(|name| (*name).to_owned())
        .collect();

    assert_eq!(
        found, pinned,
        "⚠⚠⚠⚠⚠ EITHER THE PANE ENTRY CHANGED OR THIS SCAN STOPPED SEEING IT.\n\
         MORE than the pin: a seventh pane key is published. Ask who reads it in the commit that \
         adds it — three weeks was the delay last time — then pin it here.\n\
         FEWER than the pin: a key was withdrawn, or the write forms this scan knows \
         (`entry[KEY] = …`, `KEY: …`, `.insert(KEY…)`) stopped covering how the slot builds its \
         answer. The second is the gate beside this one going green about nothing.",
    );

    let unread: Vec<String> = family
        .iter()
        .filter(|word| word.reads.is_empty())
        .map(|word| word.name.clone())
        .collect();
    assert!(
        unread.is_empty(),
        "⛔⛔⛔ {unread:?} are the item's own keys, and one of them has lost its reader. \
         `PANE_BORNE_BY_KEY` was in exactly this state from 2026-08-19 until item 1021 closed it, \
         and what noticed was a person, not a suite",
    );

    let declarations = declared(&rust_sources());
    assert!(
        family
            .iter()
            .all(|word| declarations.contains_key(&word.name)),
        "every word this family names is one the vocabulary declares, or the scan has invented a \
         subject",
    );
}
