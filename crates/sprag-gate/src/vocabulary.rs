//! Whether every party that WRITES an input event on this wire names its fields from one place —
//! register item 559.
//!
//! # ⚠⚠⚠⚠⚠ Why a half-single-sourced vocabulary is worse than none
//!
//! Item 544 stage 1c minted `KEY_FIELD` and its five siblings in `sprag_host::wire` because a THIRD
//! party — a Rust client that WRITES a keystroke — had arrived beside the parser that reads them and
//! the grammar that publishes them. Those two were routed through the constants. **The writers were
//! not.**
//!
//! Before the constants, a reader who wanted to know where a field name is spelled had to search,
//! and would have found every site. Afterwards `wire.rs` reads as though the vocabulary is
//! single-sourced — the constants are right there, used — so **the natural conclusion is that
//! renaming one is safe.** It is not: a rename reaches the daemon, the grammar and the driver, and
//! leaves the display client, the agent surface and the CLI sending a key the daemon refuses. Each
//! of those is a REFUSED KEYSTROKE at run time, on the surface a person types through, and **no
//! compiler sees it.**
//!
//! # ⚠⚠⚠ Why this cannot be a test against a running daemon
//!
//! A wire gate can only ask *does the spelling this writer uses TODAY work* — and it does, which is
//! why the split shipped green. The claim here is about tomorrow: that a rename cannot leave a
//! writer behind. That is a claim about the TEXT, so the instrument is a scan, and it lives beside
//! [`payload::indirect`](crate::payload::indirect) for that function's reason — the sites are
//! DISCOVERED rather than listed, so one that appears is announced by the gate that pins the rest.

use std::collections::BTreeSet;

use crate::sources::Source;

/// The two fields that IDENTIFY an input request, and the only ones this scan hunts.
///
/// # ⚠⚠⚠⚠ Why the anchors rather than all ten field names
///
/// `state`, `ctrl`, `col`, `row` and `kind` are ordinary English and this workspace spells them as
/// JSON keys for unrelated things — an agent's verdict has a `state`, a screen cell has a `row`, an
/// event has a `kind`. Measured 2026-08-22: hunting all ten found **65 sites and most were noise**,
/// which is a gate nobody can keep green, which is a gate that gets deleted.
///
/// **A request cannot be a keystroke without `key`, or a mouse report without `button`.** So the
/// anchors are sufficient: the modifiers cannot be hand-spelled into an input request without one of
/// these beside them, and the anchors are words this workspace uses for nothing else at a wire
/// field's position.
pub const ANCHORS: &[&str] = &["key", "button"];

/// One place an anchor is spelled as an object key in shipping code.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Spelling {
    /// Relative to the workspace root, so the gate's message is a path a person can open.
    pub file: String,
    /// One-indexed line, for the same reason.
    pub line: usize,
    /// The field name, exactly as the source spells it.
    pub field: String,
}

/// Every SHIPPING site that writes one of `fields` as a **literal object key** — `"key": …`.
///
/// # ⚠⚠⚠⚠ Why the shape is `"name":` and not the bare word
///
/// A colon immediately after the closing quote is what says the word is being used as a WIRE FIELD
/// NAME, because that is the only place JSON puts one. The bare word appears in prose, in match
/// arms and in identifiers.
///
/// # ⚠⚠⚠ What is not scanned, and why each exclusion is not a hole
///
/// * **`#[cfg(test)]` items** — [`Source::product`]'s job, and [`payload::indirect`]'s stated
///   reason: a fixture building a malformed request is proving something ABOUT the wire. A suite
///   that could not write one could not gate one.
/// * **files under a `tests/` directory** — the same argument, for the integration suites that
///   `product` cannot reach because the whole file is the test.
/// * **`exempt`** — files that write a DIFFERENT project's wire. See [`foreign_writers`], which
///   discovers them rather than listing them.
///
/// [`payload::indirect`]: crate::payload::indirect
#[must_use]
pub fn hand_spelled(sources: &[Source], fields: &[&str], exempt: &[String]) -> BTreeSet<Spelling> {
    let mut found = BTreeSet::new();
    for source in sources {
        if source.file.contains("/tests/") || exempt.contains(&source.file) {
            continue;
        }
        for (line, text) in &source.product {
            for field in fields {
                if text.contains(&format!("\"{field}\":")) {
                    found.insert(Spelling {
                        file: source.file.clone(),
                        line: *line,
                        field: (*field).to_owned(),
                    });
                }
            }
        }
    }
    found
}

/// The sources that write **pinion's** input wire rather than sprag's, discovered by the method
/// name they call.
///
/// # ⚠⚠⚠⚠⚠ Why this is discovered and not a list of file names
///
/// `sprag-gui`'s pixel smoke drives the GUI through pinion's `scene/key`, whose argument is also
/// called `key` — a different project's vocabulary that happens to share a word. Exempting it by
/// NAME would be a list with no glob, deciding alone: the day that file stops driving pinion, or the
/// day a second one starts, the list is wrong and nothing says so.
///
/// So the exemption is a PROPERTY: a source that calls pinion's own input method is writing
/// pinion's vocabulary at that site. ⚠ The gate asserts this set is non-empty, so an exemption that
/// stops describing anything is announced rather than silently widening the scan's blind spot.
#[must_use]
pub fn foreign_writers(sources: &[Source]) -> BTreeSet<String> {
    const PINION_INPUT: &[&str] = &["\"scene/key\"", "\"scene/modifiers\""];
    sources
        .iter()
        .filter(|source| {
            source
                .product
                .iter()
                .any(|(_, text)| PINION_INPUT.iter().any(|method| text.contains(method)))
        })
        .map(|source| source.file.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠⚠⚠⚠⚠ THE SCAN'S OWN CONTROL, over text this test owns.
    ///
    /// The gate beside this asserts an EMPTY answer, and `is_empty()` is equally true of a scan that
    /// works and of a scan that has stopped matching anything at all — item 453's blind ratchet, in
    /// the shape a set-emptiness assertion invites. A control taken from the workspace cannot close
    /// that, because once the item is paid there is nothing left there to find.
    #[test]
    fn the_scan_finds_a_hand_spelled_anchor_and_leaves_the_neighbouring_words_alone() {
        let source = Source {
            file: "crates/made-up/src/lib.rs".to_owned(),
            code: Vec::new(),
            product: vec![
                (7, "json!({ \"key\": key, \"ctrl\": mods.ctrl })".to_owned()),
                (9, "let key = \"Enter\";".to_owned()),
                (11, "match kind { Button::Left => \"button\", }".to_owned()),
            ],
        };
        let found = hand_spelled(std::slice::from_ref(&source), ANCHORS, &[]);
        assert_eq!(
            found,
            BTreeSet::from([Spelling {
                file: "crates/made-up/src/lib.rs".to_owned(),
                line: 7,
                field: "key".to_owned(),
            }]),
            "the object KEY at line 7 is the site; the binding at 9 and the match arm at 11 spell \
             the same words somewhere a wire field never appears, and a scan that took them would \
             be a gate nobody could keep green",
        );
    }

    /// A file under `tests/` is a fixture, and a fixture that could not build a request by hand
    /// could not gate one.
    #[test]
    fn a_fixture_may_spell_what_a_writer_may_not() {
        let source = Source {
            file: "crates/made-up/tests/wire.rs".to_owned(),
            code: Vec::new(),
            product: vec![(3, "json!({ \"key\": \"Enter\" })".to_owned())],
        };
        assert!(
            hand_spelled(std::slice::from_ref(&source), ANCHORS, &[]).is_empty(),
            "an integration test is where this wire is PROVEN, so its hand-built requests are the \
             instrument rather than the defect",
        );
    }
}
