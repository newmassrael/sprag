//! ⛔⛔⛔⛔ **EVERY PARTY THAT WRITES A KEYSTROKE OR A MOUSE REPORT NAMES ITS FIELDS FROM ONE
//! PLACE** — register item 559.
//!
//! # ⚠⚠⚠⚠⚠ The defect is a rename that half-lands, and no compiler can see it
//!
//! `sprag_host::wire` mints `KEY_FIELD`, `KEY_STATE_FIELD` and the four modifier fields. The parser
//! that reads them and the grammar that publishes them go through the constants; until this round
//! the WRITERS did not — the display client's key path, its whole mouse report, the agent surface's
//! send-keys and its Enter-after-text, and the CLI's `send-keys`.
//!
//! So a rename reaches the daemon, the grammar and the driver, and leaves a writer sending a key the
//! daemon refuses. What a person sees is a **refused keystroke at run time on the surface they type
//! through**, and every suite stays green because each suite renames both of its own halves
//! together.
//!
//! ⚠⚠ **Half-single-sourced is worse than not single-sourced at all**, which is why this is a gate
//! rather than a tidy-up. Before the constants existed a reader had to search and would have found
//! every site; afterwards the module reads as though the vocabulary has one home, so the natural
//! conclusion — *renaming this is safe* — became false without anything announcing it.
//!
//! # ⚠⚠⚠ What the scan is pointed at, and what it deliberately is not
//!
//! Only the two ANCHOR fields, `key` and `button`: a request cannot be a keystroke without one and a
//! mouse report without the other, and the eight names around them are ordinary English this
//! workspace spells for unrelated things. Hunting all ten found **65 sites, most of them noise**
//! (measured 2026-08-22) — a gate nobody can keep green is a gate that gets deleted.

use std::collections::BTreeSet;

use sprag_gate::sources::rust_sources;
use sprag_gate::vocabulary::{ANCHORS, Spelling, foreign_writers, hand_spelled};

#[test]
fn no_shipping_writer_hand_spells_an_input_events_anchor_field() {
    let sources = rust_sources();
    let foreign: Vec<String> = foreign_writers(&sources).into_iter().collect();

    // ⚠⚠⚠⚠⚠ THE EXEMPTION IS ASSERTED BEFORE IT IS USED. A discovered exemption that discovered
    // nothing would silently become no exemption at all — which is harmless here and is exactly the
    // shape that is NOT harmless the day the property stops matching what it was written for.
    assert!(
        !foreign.is_empty(),
        "⚠⚠⚠ nothing in this workspace was found to call pinion's own input methods, so the \
         exemption below describes no file. Either the pixel smoke stopped driving pinion — in \
         which case this gate just widened without anyone deciding to — or the property stopped \
         matching how it is called",
    );

    let found: BTreeSet<Spelling> = hand_spelled(&sources, ANCHORS, &foreign);
    let named: Vec<String> = found
        .iter()
        .map(|at| format!("{}:{} — \"{}\"", at.file, at.line, at.field))
        .collect();
    assert!(
        found.is_empty(),
        "⛔⛔⛔⛔ REGISTER ITEM 559: {} shipping site(s) build an input request by hand instead of \
         calling `sprag_host::wire::keystroke_args` / `mouse_args`. A field rename then reaches the \
         daemon, the grammar and the driver and leaves these behind, and what a person sees is a \
         KEYSTROKE THE DAEMON REFUSES — with every suite green, because each suite renames both of \
         its own halves together.\n  {}",
        found.len(),
        named.join("\n  "),
    );
}

/// ⚠⚠ The exemption names PINION's writers, so it must not have swallowed one of sprag's.
///
/// A property-based exemption is only as narrow as the property. If a source both drove pinion's
/// input and wrote sprag's, exempting the file would hide a real site — so the gate says which files
/// it stood aside for, and holds that none of them is one of sprag's own wire crates.
#[test]
fn the_exemption_stands_aside_only_for_the_other_projects_wire() {
    let sources = rust_sources();
    let sprags_own: Vec<String> = foreign_writers(&sources)
        .into_iter()
        .filter(|file| {
            file.starts_with("crates/sprag-client/")
                || file.starts_with("crates/sprag-mcp/")
                || file.starts_with("crates/sprag-host/")
        })
        .collect();
    assert!(
        sprags_own.is_empty(),
        "⛔⛔⛔ the exemption covers {sprags_own:?}, which are sprag's OWN writers. A file that \
         drives both wires must not be excused for sprag's on account of pinion's — the exemption \
         would then be hiding the exact sites this gate exists to find",
    );
}
