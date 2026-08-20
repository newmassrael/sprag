//! Which of the loop template's numbers a KIND is invited to author, and whether anything can
//! carry one — register item 494.
//!
//! # The defect this closes, twice measured
//!
//! `ai_loop.scxml` is a template other repositories copy, and beside it stands one repository's own
//! `debt_loop.scxml` — a KIND, holding the decisions that are not the template's to make. Several
//! of the template's `<data>` carry a comment saying so in as many words: *"it is the KIND's to
//! author, like `max_turns` and `reflect_every`"*.
//!
//! Item 492 found one of those sentences pointing at a road that did not exist. `context_ceiling`
//! had been authored in the kind's document since 2026-08-18, argued over three paragraphs and
//! dated — and there was no `LoopKind` reader, no `Brief` field, no wire key and no `<assign>`, so
//! **the number was 0 on every run anybody had ever driven**. Item 477 measured the far end: eight
//! of eight `reviewing` exits taking the fall-back, which is that state never once deciding.
//!
//! Item 494 is the same defect, found the next day, one `<data>` up. `reflect_after_refusals` said
//! the same sentence and had the same nothing. **A premise that produces one defect produces the
//! rest of its class**, and 492 had paid the instance.
//!
//! # ⚠⚠⚠⚠⚠ So this is a gate over the CLASS, and the ids are DERIVED
//!
//! Nothing here is spelled by hand:
//!
//! * the CLAIMED ids come from the template's own comments — a `<data>` is invited when the comment
//!   block immediately above it makes the claim, so a number that acquires the sentence tomorrow is
//!   watched from the moment it does;
//! * the READERS come from shipping code, as string literals in the last-argument position of a
//!   call, so which accessor a reader reaches through is not part of the needle;
//! * and WHICH FILES to read them from is a glob over the road those accessors travel
//!   ([`crate::authored::READS`]),
//!   so a kind in a file nobody has written yet is a subject the day it appears — register item
//!   498(a), which is what this module named ONE HARDCODED PATH for as long as there was one kind.
//!
//! ⚠ A ratchet whose needle is a constant can only ever see the spelling its author thought of —
//! register item 453, and [`crate::loop_shape`] one file over says it about a different needle.
//! ⚠⚠ Every derivation here is answered by a PIN in the gate that drives it, because *does it
//! pass?* and *does it still SEE?* are different questions (item 470) — and a glob has a second
//! one of its own: **how many subjects were there?** A union is green whether it read two kinds or
//! one.
//!
//! # ⚠⚠⚠⚠ What is required of a claimed id, and what deliberately is not
//!
//! Two things, because together they are exactly *a kind's decision can reach a run*:
//!
//! 1. a reader on `LoopKind`, or the value cannot leave the kind's document;
//! 2. an `<assign>` in the template's `brief` transition, or the value cannot land in the run's.
//!
//! **A wire key is NOT required**, and that is a decision rather than an omission. The sentence
//! says the number is the KIND's; `milestone_check` and the service needle are kind-only on purpose
//! (*"a caller who could name the needle could delete the wait by naming nothing"*), so a future
//! claimed number may be kind-only too. Requiring a key would make this gate demand a caller
//! override that its own premise argues against.
//!
//! # ⚠ What a text scan cannot claim
//!
//! This crate takes no dependencies by charter, so nothing here parses XML or Rust. A reader that
//! named its id through a `const` declared in another file walks past, and so would a claim written
//! in a comment block that is not the one above the `<data>`. Both are stated rather than implied,
//! and the pinned equality in this module's gate is what says the derivation has stopped seeing
//! what it used to see.
//!
//! ⚠⚠ **A CLAIM WRITTEN AS TWO SENTENCES IS ALSO UNSEEN** — *"this one is not the template's to
//! choose. It belongs to the kind, like `max_turns` and `reflect_every`"* makes the claim and no
//! sentence of it holds both halves. That is the price of bounding the reading by punctuation
//! instead of by a byte count measured on n = 2 (item 498(b)), and it is the SAFE direction: unseen
//! shows up as FEWER than the pin, which is a person's question, while the byte count's failure was
//! a claim that had simply moved too far to be read at all.

use std::collections::BTreeSet;

use crate::sources::Source;

/// The ROAD a kind's number travels, as the Rust spells it — register item 498(a).
///
/// # ⚠⚠⚠⚠ Why the road and not the type, the file or the trait
///
/// This module used to name ONE PATH, `crates/sprag-plugin/src/kind.rs`, and *a list with no glob
/// decides alone* — item 470's own finding, violated one module over from where 470 wrote it down.
/// Its two failure directions are not symmetric: a reader that MOVES makes the gate call a working
/// channel missing (noisy, and it announces itself), while a SECOND kind reading its numbers
/// somewhere else makes the gate vouch for a channel that kind does not have (**silent**, which is
/// the exact defect item 494 exists to prevent).
///
/// `LoopKind` is a struct rather than a trait (measured 2026-08-20: one inherent `impl` in one
/// file), so a needle on the TYPE would find today's kind and nothing else. What a second kind
/// type would have in common with this one is the ACCESSOR FAMILY — every reader goes through
/// `OuterLoop::authored_…_in(script, session, "id")`, which is the road item 492 built and item 494
/// documented. So that is the needle, and a kind nobody has written yet is discovered by it.
///
/// ⚠ Measured over the whole workspace on 2026-08-20: two files hold it, and one of them is this
/// module's own test table — which [`JUDGE`] is about.
///
/// # ⚠⚠⚠ This road is now one of TWO, and only this one discovers a FILE
///
/// [`read_ids`] learned the generated-accessor road the same day (`policy().context_ceiling()`,
/// SCE PR-86 R-86.4), and four kind-side readers moved onto it. Discovery stayed here on purpose:
/// `policy().` is written all over this workspace's tests about things that are not `<data>` at
/// all, so globbing FILES by it would collect subjects that read nothing.
///
/// ⚠⚠ The residue, stated: the day the LAST `OuterLoop::authored_…` reader migrates, this glob
/// finds no file and the gate refuses — **loudly, not silently**, which is the direction that
/// keeps it a question for a person rather than a green about nothing (item 482). The repair then
/// is to discover by the claimed ids themselves, which the template already publishes.
pub const READS: &str = "OuterLoop::authored_";

/// The judge's own crate, which cannot hold a subject.
///
/// # ⚠⚠ It is derivable rather than merely convenient
///
/// `sprag-gate` declares NO dependencies by charter, so nothing in it can call `sprag_plugin`'s
/// accessors — while every needle this module hunts is quoted in its own text, both in prose and in
/// the tables that prove the derivation still sees. A judge that read itself would find a kind
/// reader in a crate that cannot compile one.
///
/// ⚠ The same idiom, and for the same reason, as `no_suite_runs_a_program_it_wrote`'s exemption of
/// [`crate::doubles`]: the file that DESCRIBES an offence is not committing it.
pub const JUDGE: &str = "crates/sprag-gate/";

/// The word every claim contains, lowercased for comparison.
///
/// ⚠ It is not the whole needle. See [`claims`], whose second reading is what keeps a rephrasing
/// from blinding this one.
pub const AUTHOR: &str = "author";

/// The two `<data>` every claim compares itself to, in the template's own words *"like `max_turns`
/// and `reflect_every`"*.
///
/// ⚠⚠ These are EXEMPLARS and not members: they carry the channel the sentence points at, which is
/// why the sentence points at them, and they make no claim of their own. Naming them is how
/// [`claims`] recognises a claim whose phrasing has changed.
pub const EXEMPLARS: [&str; 2] = ["max_turns", "reflect_every"];

/// Every Rust source that carries a reader of a kind's document, sorted, judge excluded.
///
/// ⚠⚠⚠ The caller must refuse an EMPTY answer and must PIN what this returns. A glob that finds
/// nothing reports no offences and reads exactly like a clean tree (item 482), and a glob whose
/// needle has gone blind does the same — *"does the gate pass?"* and *"does the gate still SEE?"*
/// are different questions, and only a pinned measurement answers the second (item 470).
#[must_use]
pub fn kind_sources(sources: &[Source]) -> Vec<String> {
    let needle: String = READS.chars().filter(|char| !char.is_whitespace()).collect();
    let mut found: Vec<String> = sources
        .iter()
        .filter(|source| !source.file.starts_with(JUDGE))
        .filter(|source| squeezed(&source.product).contains(&needle))
        .map(|source| source.file.clone())
        .collect();
    found.sort();
    found.dedup();
    found
}

/// Lines with every space gone — what a needle that spans a formatter's line break has to be read
/// against. ⚠ It takes the lines rather than a [`Source`] because the caller chooses between what
/// SHIPS and what proves it, and this module's subject is the shipping road.
fn squeezed(lines: &[(usize, String)]) -> String {
    lines
        .iter()
        .flat_map(|(_, line)| line.chars().filter(|char| !char.is_whitespace()))
        .collect()
}

/// One `<data>` of the template whose own comment says a KIND may author it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claimed {
    /// The document's own id, so a refusal talks in the words the document uses.
    pub id: String,
    /// The sentence that claimed it, trimmed to one line — a refusal shows the claim rather than
    /// only asserting there was one.
    pub said: String,
}

/// Every `<data>` the template invites a KIND to author, in document order.
///
/// # ⚠⚠⚠⚠⚠ Two readings of one claim, so a rephrasing cannot blind it
///
/// A comment block claims the `<data>` beneath it when ONE SENTENCE of it holds the word `author`
/// and either
///
/// * the phrase `kind's to author` — what both of today's claims say, in two different cases, which
///   is already evidence that one exact spelling is not safe; or
/// * BOTH of [`EXEMPLARS`] — *"like `max_turns` and `reflect_every`"*, the comparison every claim so
///   far has drawn, which survives any amount of rewriting around it.
///
/// Blinding this needs both to go at once.
///
/// # ⚠⚠⚠⚠⚠ Why a SENTENCE, and not the whole block or a count of bytes
///
/// Reading the two conditions over the whole block claimed **`reflect_prompt` and `max_turns`**, and
/// neither is a claim: those blocks run for dozens of lines, discuss both exemplars at length for
/// their own reasons, and say `author` somewhere about a PERSON. `max_turns` is worse than a false
/// positive — it is one of the exemplars, so a rule that reads the whole block has every claim
/// naming it and can never not claim it.
///
/// **A claim is one SENTENCE**, which this file has said since it was written — and it then bounded
/// the reading with 100 BYTES either side, a number measured on the only two claims in existence.
/// Item 498(b): a bound chosen on n = 2 is blind to the third claim by construction, and nothing
/// could report that — a claim whose exemplars land 120 bytes out is simply never seen, and a
/// derivation that sees nothing reports no offences.
///
/// So the bound is now the sentence itself, taken from the text's own punctuation. It is DERIVED
/// where the byte count was chosen: a rewording that stretches the claim keeps working, and a
/// paragraph that mentions both numbers in one breath and `author` in the next is still declined,
/// which is exactly the shape the byte count was defending against. ⚠ Measured 2026-08-20 against
/// the real template: the two rules claim the SAME two `<data>`, so this changed the argument
/// rather than the answer.
///
/// ⚠ The block is flattened to single-spaced text first because the template wraps at 80 columns —
/// a line-by-line reading would go blind the day a claim wraps between `author` and `max_turns`.
/// ⚠⚠ A sentence ends at `. `, so an abbreviation splits one in two. That is the SAFE direction: a
/// claim cut in half goes unseen, and unseen is what the pin in this module's gate reports.
///
/// ⚠ And the block must be the one IMMEDIATELY above the declaration: the same sentence is QUOTED
/// elsewhere in the template — inside the `brief` transition, next to the `<assign>` that made item
/// 492 true — and a claim is a statement about a declaration rather than a phrase that appears near
/// one.
#[must_use]
pub fn claims(scxml: &str) -> Vec<Claimed> {
    let mut found = Vec::new();
    for (open, _) in scxml.match_indices("<!--") {
        let rest = &scxml[open + 4..];
        let Some(close) = rest.find("-->") else {
            continue;
        };
        let block = &rest[..close];
        let Some(said) = claim_in(block) else {
            continue;
        };
        let after = rest[close + 3..].trim_start();
        if !after.starts_with("<data") {
            continue;
        }
        let Some(id) = attribute(&after[5..], "id") else {
            continue;
        };
        found.push(Claimed { id, said });
    }
    found
}

/// The sentence that makes the claim, or [`None`] where the block makes none.
///
/// The block is flattened to single-spaced text so an 80-column wrap cannot separate words the rule
/// wants together, and each SENTENCE of it is judged on its own — see [`claims`] for why the bound
/// is the punctuation rather than a count of bytes.
///
/// ⚠ The sentences are cut from the text AS WRITTEN and lowercased one at a time. Cutting them from
/// a lowercased copy would be a latent defect rather than a style: `str::to_lowercase` is allowed to
/// change a string's LENGTH, and this file's subject is a document full of `⚠` and Korean.
fn claim_in(block: &str) -> Option<String> {
    let flat = block.split_whitespace().collect::<Vec<_>>().join(" ");
    let exemplars: Vec<String> = EXEMPLARS
        .iter()
        .map(|exemplar| exemplar.to_lowercase())
        .collect();

    for (from, to) in sentences(&flat) {
        let said = &flat[from..to];
        let lowered = said.to_lowercase();
        if !lowered.contains(AUTHOR) {
            continue;
        }
        let phrased = lowered.contains("kind's to author");
        let compared = exemplars.iter().all(|exemplar| lowered.contains(exemplar));
        if phrased || compared {
            return Some(said.trim().to_owned());
        }
    }
    None
}

/// The half-open range of each sentence in `flat`, the ending `.` included.
///
/// ⚠ `. ` is ASCII, so every bound it produces is a char boundary — which is what lets a slice of
/// this text hold a `⚠` whole.
fn sentences(flat: &str) -> Vec<(usize, usize)> {
    let mut found = Vec::new();
    let mut start = 0;
    for (at, _) in flat.match_indices(". ") {
        found.push((start, at + 1));
        start = at + 2;
    }
    if start < flat.len() {
        found.push((start, flat.len()));
    }
    found
}

/// The value of `name="…"` in `tag`, up to the tag's own `>`.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let end = tag.find('>')?;
    let needle = format!("{name}=\"");
    let at = tag[..end].find(&needle)? + needle.len();
    let value = &tag[at..];
    let close = value.find('"')?;
    Some(value[..close].to_owned())
}

/// Every template id the kind-side Rust READS, taken from `code` — the file's lines with comments
/// already dropped, whitespace and all.
///
/// # ⚠⚠⚠⚠ The needle is the argument POSITION and not the accessor's name
///
/// A reader reaches the datamodel through one of several spellings today
/// (`OuterLoop::authored_number_in`, `authored_count_in`, `authored_text_in`, the script engine's
/// own `get_variable`) and the next one will have a name nobody here guessed. What they have in
/// common is the shape of the call: **the id is the last argument, as a literal**. Needling on that
/// means a new accessor teaches this rather than blinding it.
///
/// # ⚠⚠⚠⚠⚠ AND THERE IS A SECOND ROAD NOW, where the id is the METHOD and not an argument
///
/// SCE's codegen emits a read accessor per `<data>` — `policy().context_ceiling()` — and consuming
/// that (PR-86 R-86.4, 2026-08-20) is what a kind-side reader should do wherever the id has ONE
/// type: the document's own names become the compiler's, so a renamed `<data>` stops the build
/// instead of reading nothing. **Migrating four readers to it took this derivation to zero for two
/// claimed ids and the gate went red** — correctly, because from the old needle's side the readers
/// really had gone.
///
/// So both roads count, and a UNION is not a weakening here: the question is *can this kind's
/// decision leave its document*, and either spelling answers yes. ⚠ The accessor road cannot be
/// needled by the RECEIVER (`self.machine.policy()` today, something else tomorrow), so it is
/// needled by `policy().` — the one thing a generated read must go through.
///
/// ⚠⚠ An id read by BOTH roads appears once: this is a set.
///
/// ⚠ Whitespace is squeezed out first because rustfmt decides where these calls break, and
/// `session, "id")` and `session,\n    "id",\n)` are the same call written two ways.
///
/// # ⚠⚠⚠⚠⚠ A MACRO'S last argument is not a read, and this gate's own table is what found that
///
/// `panic!("the kind must declare {}", "reflect_after_refusals")` puts the id in exactly the
/// position a reader does. Every formatting macro in this workspace's refusals can do it, and one
/// of them naming a channel that does not exist would make this gate declare the channel PRESENT —
/// slack in the direction that hides the defect it was built for. So the innermost call's opening
/// parenthesis is walked back to and the name before it must not end in `!`.
#[must_use]
pub fn read_ids(code: &[(usize, String)]) -> BTreeSet<String> {
    let squeezed: String = code
        .iter()
        .flat_map(|(_, line)| line.chars().filter(|char| !char.is_whitespace()))
        .collect();
    let bytes = squeezed.as_bytes();
    let mut found = BTreeSet::new();
    for (at, _) in squeezed.match_indices(",\"") {
        let rest = &squeezed[at + 2..];
        let Some(end) = rest.find('"') else {
            continue;
        };
        // A trailing comma is what a formatter leaves behind when it broke the call over lines.
        if !rest[end + 1..].starts_with(')') && !rest[end + 1..].starts_with(",)") {
            continue;
        }
        if called_by_macro(bytes, at) {
            continue;
        }
        found.insert(rest[..end].to_owned());
    }
    // ── THE SECOND ROAD: a generated accessor, where the id IS the method name ──
    for (at, _) in squeezed.match_indices(ACCESSOR) {
        let rest = &squeezed[at + ACCESSOR.len()..];
        let end = rest
            .find(|char: char| !char.is_ascii_alphanumeric() && char != '_')
            .unwrap_or(rest.len());
        // ⚠ A CALL, not a field: `policy().session_id` is a struct member and names no `<data>`.
        // Requiring the parentheses is what keeps this from reading the generated policy's own
        // bookkeeping as a datamodel read.
        if end > 0 && rest[end..].starts_with("()") {
            found.insert(rest[..end].to_owned());
        }
    }
    found
}

/// The one thing a generated `<data>` read goes through, squeezed — see [`read_ids`]'s second road.
///
/// ⚠ The RECEIVER is deliberately not part of it: today it is `self.machine.policy()`, tomorrow a
/// borrow held somewhere else, and a needle that spelled the receiver would go blind on a
/// refactor that changed nothing about whether the id is read.
pub const ACCESSOR: &str = "policy().";

/// Whether the call whose argument list contains `at` is a MACRO invocation.
///
/// Walks left to the innermost unmatched `(`, stepping over quoted spans so a literal holding a
/// parenthesis cannot move the reader's place, and answers on the byte before it.
///
/// ⚠ An unbalanced walk answers `true` — a reader that has lost its place must not vouch for a
/// channel. That is the direction that keeps this gate strict rather than slack.
fn called_by_macro(bytes: &[u8], at: usize) -> bool {
    let mut depth = 0usize;
    let mut index = at;
    while index > 0 {
        index -= 1;
        match bytes[index] {
            b'"' => {
                // Step over the literal this quote closes.
                let Some(open) = bytes[..index].iter().rposition(|byte| *byte == b'"') else {
                    return true;
                };
                index = open;
            }
            b')' => depth += 1,
            b'(' => {
                if depth == 0 {
                    return bytes[index.saturating_sub(1)] == b'!' && index > 0;
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    true
}

/// Whether the template's `brief` transition assigns `id`, so a carried value can land.
///
/// ⚠ The `<assign>` is what makes a channel real at the far end: item 492's own road ends here, and
/// a reader plus a field with no assignment would carry a number to a document that drops it.
#[must_use]
pub fn assigned(scxml: &str, id: &str) -> bool {
    scxml.contains(&format!("<assign location=\"{id}\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(text: &str) -> Vec<(usize, String)> {
        text.lines()
            .enumerate()
            .map(|(index, line)| (index + 1, line.trim().to_owned()))
            .filter(|(_, line)| !line.starts_with("//"))
            .collect()
    }

    /// ⚠⚠⚠⚠⚠ **BOTH DIRECTIONS.** Each row is a comment block as the template could really carry
    /// one, plus whether the `<data>` under it is claimed. The two spellings at the top are the two
    /// this template ACTUALLY holds — differing in case, which is already proof that one exact
    /// phrase is not a safe needle — and the rows under them are the rewordings a person reaches
    /// for.
    ///
    /// Item 453's lesson is that a needle written for one spelling is green through the others
    /// without ever saying so, and item 470's is that **the ratchet cannot answer whether it still
    /// SEES the shape.** This table is the only thing that answers the second question.
    #[test]
    fn every_way_of_claiming_a_number_for_a_kind_is_seen_and_the_rest_declined() {
        // (the comment block, whether the `<data>` under it is claimed)
        let table: &[(&str, bool)] = &[
            // Verbatim from `reflect_after_refusals`, upper case.
            (
                "⚠⚠ IT IS THE KIND'S TO AUTHOR, like `max_turns` and `reflect_every`: how\n\
                 patient to be with a checker is a judgement about the work.",
                true,
            ),
            // Verbatim from `context_ceiling`, mixed case — the same claim, a different spelling.
            (
                "⚠ It is the KIND's to author, like `max_turns` and `reflect_every` — what\n\
                 a session may spend depends on the work.",
                true,
            ),
            // ⚠ REPHRASED past the exact phrase, and still seen because the comparison stands.
            (
                "⚠ A repository authors this one, the way it does `max_turns` and `reflect_every`.",
                true,
            ),
            // ⚠ REPHRASED past the comparison, and still seen because the phrase stands.
            ("⚠ This number is the kind's to author.", true),
            // ⚠ DECLINED — the exemplars with nothing about authoring is the ordinary case: half
            // this document's comments discuss those two numbers.
            (
                "⚠ `judging` tests `max_turns` before `reflect_every`, so an equal pair exhausts \
                 first.",
                false,
            ),
            // ⚠ DECLINED — authoring discussed about something that is not a kind's decision.
            (
                "⚠ Written by the driver on every `turn.done` and by nothing else; no author \
                 touches it.",
                false,
            ),
            // ⚠ DECLINED — a claim about ONE exemplar is not the comparison the class draws.
            ("⚠ It is the caller's to author, like `max_turns`.", false),
            // ⚠⚠⚠⚠⚠ DECLINED — AND THIS ROW IS WHAT HOLDS THE SENTENCE BOUND. The two exemplars
            // and the word are all here, in DIFFERENT sentences, which is the shape of
            // `reflect_prompt`'s and `max_turns`'s own blocks: pages about both numbers, with
            // `author` somewhere in them meaning a PERSON. Widening the bound to the whole block
            // re-claims both of those in the real template, and without this row the table cannot
            // see that at all — measured, by widening it and watching these assertions stay green.
            (
                "⚠ `judging` tests `max_turns` before `reflect_every`, so an equal pair exhausts \
                 first — and that ordering is why the reflection cadence borrows the budget rather \
                 than standing on a number of its own, which the paragraph below spends a while \
                 on. What the AUTHOR of a copy of this template writes here is their business.",
                false,
            ),
            // ⚠⚠⚠⚠⚠ SEEN — AND THIS ROW IS ITEM 498(b), THE CASE THE OLD BOUND WAS BLIND TO. One
            // sentence, the claim made plainly, and the exemplars 212 and 228 BYTES past the word
            // because the author explained themselves on the way. The old rule read 100 bytes
            // either side, measured on the only two claims that existed (29 and 30 bytes), so a
            // third claim written like this was never seen — and a derivation that sees nothing
            // reports no offences.
            //
            // ⚠⚠⚠⚠ THE DISTANCES ARE MEASURED, not eyeballed. The first draft of this row put the
            // exemplars 39 and 55 bytes out, which the OLD rule sees perfectly well: it would have
            // passed under both rules and proved nothing about either. That is item 494's own
            // finding — a table of short synthetic rows cannot see a width bound at all.
            (
                "⚠ Whoever authors the kind document decides this one, because how patient to be \
                 with a checker depends on the machine it runs on and on the work in front of it \
                 rather than on anything this template could know, exactly as `max_turns` and \
                 `reflect_every` do.",
                true,
            ),
        ];

        let mut wrong = Vec::new();
        for (block, claimed) in table {
            let scxml = format!("<!--\n{block}\n-->\n<data id=\"subject\" expr=\"0\"/>");
            let read = claims(&scxml);
            if read.iter().any(|found| found.id == "subject") != *claimed {
                wrong.push(format!("owed {claimed}, read {read:?} for {block:?}"));
            }
        }
        assert!(
            wrong.is_empty(),
            "⚠⚠⚠⚠⚠ a ratchet that cannot see the ordinary way of writing the claim is green \
             forever in the voice of a working one — and this gate exists because the SAME sentence \
             is already written two ways in the real template: {wrong:#?}",
        );
    }

    /// ⚠⚠⚠ A claim is a statement about a DECLARATION, not a phrase that appears near one — and the
    /// template really does quote its own sentence somewhere else, beside the `<assign>` that item
    /// 492 added. A reader that attached the quotation to the next `<data>` it could find would
    /// invent a claim about whatever declaration came later in the file.
    #[test]
    fn a_claim_quoted_away_from_a_declaration_claims_nothing() {
        let quoted = "<transition event=\"brief\">\n<!--\n\
             `context_ceiling` says *\"it is the KIND's to author, like `max_turns` and \
             `reflect_every`\"*, and until this assignment existed there was no road.\n\
             -->\n<assign location=\"context_ceiling\" expr=\"1\"/>\n</transition>\n\
             <data id=\"unrelated\" expr=\"0\"/>";
        assert!(
            claims(quoted).is_empty(),
            "the block above an `<assign>` claims no `<data>`, however exactly it quotes the claim",
        );

        // ⚠ And a `<data>` with no comment above it at all is not claimed by the block before the
        // one that precedes it.
        let bare = "<!--\n⚠ It is the kind's to author.\n-->\n<data id=\"claimed\" expr=\"0\"/>\n\
             <data id=\"bookkeeping\" expr=\"0\"/>";
        let found = claims(bare);
        assert_eq!(found.len(), 1, "one claim, not two: {found:?}");
        assert_eq!(found[0].id, "claimed");
    }

    /// ⚠⚠⚠⚠⚠ **THE GLOB FINDS A KIND NOBODY HAS WRITTEN YET, AND REFUSES TO FIND THE JUDGE** —
    /// register item 498(a).
    ///
    /// Each row is a source as the walk hands it over. The point of the second one is that a file
    /// this module has never heard of is discovered by the ROAD its readers travel — which is what
    /// a hardcoded path could not do, and the direction it failed in was the silent one.
    #[test]
    fn a_kind_is_found_by_the_road_its_readers_travel() {
        let source = |file: &str, body: &str| Source {
            file: file.to_owned(),
            code: code(body),
            product: code(body),
        };
        let reads = "pub fn ceiling(&self) -> Option<i64> {\n    \
                     OuterLoop::authored_number_in(&self.script, &self.session, \"context_ceiling\")\n}";

        let found = kind_sources(&[
            source("crates/sprag-plugin/src/kind.rs", reads),
            // ⚠ A SECOND KIND, in a file no constant here names. Before item 498 this one was
            // invisible and the gate vouched for its channels anyway.
            source("crates/sprag-plugin/src/review_kind.rs", reads),
            // ⚠ NOT a kind reader: the wire's own merge, which reads a caller's map and falls back
            // to the kind. `sprag-host` really does hold this shape (`plugins.rs`), and a rule
            // that took it for a kind would demand every claimed number of a file whose job is to
            // let a caller OVERRIDE one.
            source(
                "crates/sprag-host/src/plugins.rs",
                "context_ceiling: opt_count(map, \"context_ceiling\")?.or_else(|| kind.context_ceiling()),",
            ),
            // ⚠⚠ THE JUDGE, quoting the needle it hunts — see [`JUDGE`].
            source(
                "crates/sprag-gate/src/authored.rs",
                "pub const READS: &str = \"OuterLoop::authored_\";",
            ),
        ]);

        assert_eq!(
            found,
            vec![
                "crates/sprag-plugin/src/kind.rs".to_owned(),
                "crates/sprag-plugin/src/review_kind.rs".to_owned(),
            ],
            "⚠⚠⚠⚠⚠ the glob must find every file a kind's numbers are read in and nothing else. A \
             judge that read its own text would report a kind reader inside a crate that declares \
             no dependencies and cannot compile one.",
        );

        assert!(
            kind_sources(&[source("crates/sprag-plugin/src/kind.rs", "let x = 1;")]).is_empty(),
            "⚠⚠⚠ and a tree with no reader at all must come back EMPTY rather than defaulting to \
             the file this module used to name — the caller refuses that, and an empty answer it \
             could not see would be item 482's vacuous gate exactly",
        );
    }

    /// ⚠⚠⚠⚠ **THE READER SIDE, BOTH DIRECTIONS.** Every accessor this workspace reaches the
    /// datamodel through is here as its real call, plus the shapes a formatter produces, plus the
    /// near-misses that must NOT count as a reader.
    #[test]
    fn every_shape_of_reading_an_authored_id_is_seen_and_the_rest_declined() {
        // (the Rust, the ids owed)
        let table: &[(&str, &[&str])] = &[
            (
                "OuterLoop::authored_number_in(&self.script, &self.session, \"context_ceiling\")",
                &["context_ceiling"],
            ),
            (
                "OuterLoop::authored_count_in(&self.script, &self.session, \"max_turns\")",
                &["max_turns"],
            ),
            (
                "self.script.get_variable(&self.session, \"milestone_check\")",
                &["milestone_check"],
            ),
            // ⚠⚠⚠⚠⚠ THE SECOND ROAD — a generated accessor, where the id is the METHOD. Consuming
            // SCE PR-86 R-86.4 moved four kind-side readers onto this shape, and the derivation
            // that only knew the first road reported those ids as unread.
            (
                "self.machine.policy().context_ceiling()",
                &["context_ceiling"],
            ),
            // ⚠ THE RECEIVER IS NOT PART OF THE NEEDLE: a reader that held the policy some other
            // way reads the same `<data>`, and a needle spelling `self.machine` would go blind on a
            // refactor that changed nothing.
            (
                "kind.policy().reflect_after_refusals()",
                &["reflect_after_refusals"],
            ),
            // ⚠⚠ DECLINED — a FIELD of the generated policy is not a `<data>` read. `session_id` is
            // the policy's own bookkeeping and appears in this workspace's tests constantly; a rule
            // that took it for a datamodel id would invent a claim nobody made.
            ("engine.policy().session_id", &[]),
            // ⚠ DECLINED — and this is the same trap one letter over: a method call on something
            // that is not a policy names no id.
            ("run.context().context_ceiling()", &[]),
            // ⚠ Broken over lines by the formatter, trailing comma and all — the same call.
            (
                "OuterLoop::authored_text_in(\n&self.script,\n&self.session,\n\"closing_rules\",\n)",
                &["closing_rules"],
            ),
            // ⚠ An accessor whose name nobody here guessed, which is the whole point of needling
            // the argument position instead of the name.
            (
                "read_however_the_next_one_is_spelled(&session, \"a_future_number\")",
                &["a_future_number"],
            ),
            // ⚠ DECLINED — a literal that is not the last argument reads nothing.
            (
                "OuterLoop::authored_number_in(&self.script, \"not_a_session\", name)",
                &[],
            ),
            // ⚠ DECLINED — prose in a refusal is not a read, however exactly it names the id. This
            // row is the one that FOUND the hole: a formatting macro puts its arguments in the same
            // position a reader does, and every refusal in this workspace could have vouched for a
            // channel that did not exist.
            (
                "panic!(\"the kind must declare {}\", \"reflect_after_refusals\");",
                &[],
            ),
            // ⚠ DECLINED — and a literal holding a parenthesis must not move the reader's place,
            // or the walk finds the wrong call and answers about the wrong name.
            (
                "assert_eq!(held, \"a number (or nothing)\", \"max_turns\");",
                &[],
            ),
            // ⚠ SEEN — a real read whose earlier argument is itself a call, which is what the walk
            // has to step over rather than stop at.
            (
                "authored_number_in(script.as_ref(), session_of(&run), \"context_ceiling\")",
                &["context_ceiling"],
            ),
        ];

        let mut wrong = Vec::new();
        for (rust, owed) in table {
            let read = read_ids(&code(rust));
            let want: BTreeSet<String> = owed.iter().map(|id| (*id).to_owned()).collect();
            if read != want {
                wrong.push(format!("owed {want:?}, read {read:?} for {rust:?}"));
            }
        }
        assert!(
            wrong.is_empty(),
            "a reader this cannot see is a channel this gate would call missing, and a refusal that \
             names a working channel is worse than none: {wrong:#?}",
        );
    }

    /// ⚠⚠ A comment is not code, so a claim spelled inside a doc comment in `kind.rs` — and every
    /// reader here has one, quoting the template's sentence — must not be mistaken for a read.
    #[test]
    fn a_doc_comment_quoting_the_id_is_not_a_reader() {
        let commented = "/// The template says *\"it is the KIND's to author\"* about\n\
             /// `get_variable(&self.session, \"reflect_after_refusals\")`.\n\
             pub fn nothing(&self) {}";
        assert!(
            read_ids(&code(commented)).is_empty(),
            "the walk drops comment lines, and this is what depends on it",
        );
    }

    #[test]
    fn an_assignment_is_the_documents_own_landing_place() {
        let scxml = "<assign location=\"context_ceiling\" expr=\"_event.data.context_ceiling\"/>";
        assert!(assigned(scxml, "context_ceiling"));
        assert!(!assigned(scxml, "context"));
        assert!(!assigned(
            "<data id=\"context_ceiling\" expr=\"0\"/>",
            "context_ceiling"
        ));
    }
}
