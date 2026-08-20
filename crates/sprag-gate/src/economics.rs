//! Whether the loop's economic edge is priced in the POPULATION it will run in — register item 493.
//!
//! # The defect this closes
//!
//! `reviewing` replaces a session when `context - floor >= 20 * cold`: the reading a replacement
//! discards has to be worth twenty times the cache it re-writes. Every gate that drove that edge
//! carried one borrowed trio — cold 7,000, floor 38,500, a last reading of 466,013 — which puts the
//! break-even at 178,500 and the fixture three times past it. The gates proved the arm REACHABLE
//! and left every reader believing it was ORDINARY.
//!
//! Measured 2026-08-20 over all 250 of that repository's transcripts carrying two billed requests,
//! the break-even is 600,970 and 49 sessions ever read that far. **At 466,013 this population does
//! not reach it at all** — the fixture had inverted the decision for the only reading anybody had
//! written down. The instance was repaired by re-measuring; this is the part that keeps it repaired.
//!
//! # ⚠⚠⚠⚠⚠ Two artefacts, and the arithmetic is RECOMPUTED rather than believed
//!
//! A number that is a measurement rots in two directions at once, and each has its own silence:
//!
//! * the FIXTURE can be edited to numbers no paragraph describes, and every gate stays green
//!   because they all read the fixture;
//! * the PARAGRAPH can be edited — or re-measured and only half-updated — and nothing at all reads
//!   it, because prose is not a test.
//!
//! So this reads BOTH and pins them to each other (register item 470: a ratchet that cannot rot
//! reads both artefacts), and it takes the multiplier from the DOCUMENT'S OWN GUARDS rather than
//! from either side's prose — so `20` becoming `15` in the transitions makes the stated sum wrong
//! by arithmetic instead of by opinion.
//!
//! ⚠⚠⚠⚠ **NOTHING HERE IS SPELLED.** The claim is found by its SHAPE (`N * X + Y` = `Z`, a rate of
//! `n of the m sessions`, a date), so re-wording the paragraph around it changes nothing and
//! re-measuring it is caught by the pin. A needle that is a constant only ever sees the spelling
//! its author thought of — register item 453, and item 494 measured the same thing one file over.
//!
//! ⚠ What a text scan cannot claim: this crate parses no XML and no Rust by charter, so a
//! measurement moved into a `const` in another file, or a claim written in a second comment block,
//! walks past. Both are stated rather than implied, and the module's own table is what says the
//! derivation has stopped seeing what it used to see.

/// The template that states the measurement, relative to the workspace root.
pub const TEMPLATE: &str = "crates/sprag-plugin/src/ai_loop.scxml";

/// The fixture every economics gate drives, and the one place its three costs live.
pub const FIXTURE: &str = "crates/sprag-plugin/src/testing.rs";

/// What that trio is declared as in [`FIXTURE`].
pub const SAMPLE: &str = "MEASURED_HERE";

/// The quantities `reviewing`'s guards compare, as the document escapes them.
///
/// ⚠ It is the LEFT-HAND SIDE only. The multiplier is what follows, and reading it out of the guard
/// is the whole point: prose that disagreed with the transitions would otherwise be arithmetic
/// nobody could check.
pub const GUARD: &str = "context - floor";

/// How the fixture's own helper spells the toll, so the one number it DOES hard-code is pinned too.
///
/// ⚠⚠ `Billed::toll` is `20 * self.cold`, and that 20 is the same folklore this module exists to
/// stop: a document re-priced to fifteen would leave the fixture computing a trade the loop does
/// not make, and the premise assertions written on it would be quietly about nothing.
pub const FIXTURE_TOLL: &str = "* self.cold";

/// What the template's own paragraph states about the trade.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stated {
    /// How many cache reads one cache write costs, as the paragraph does the sum.
    pub multiplier: u64,
    /// The toll a replacement re-pays.
    pub cold: u64,
    /// The standing cost no replacement escapes.
    pub floor: u64,
    /// The reading past which replacing pays for itself, as the paragraph states it.
    pub sum: u64,
    /// How many sessions of the measured population ever read that far.
    pub reached: u64,
    /// How many were measured.
    pub population: u64,
    /// The day it was measured — a measurement without one is folklore (register item 456).
    pub dated: String,
    /// The sentence the sum was found in, so a refusal shows the claim rather than asserting there
    /// was one.
    pub sentence: String,
}

/// The three costs the fixture holds, as it declares them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    /// The first billed request's cache write.
    pub cold: u64,
    /// The second billed request's cache read.
    pub floor: u64,
    /// The last billed request's cache read.
    pub context: u64,
}

impl Sample {
    /// The reading past which replacing this session pays for itself, at `multiplier` to one.
    #[must_use]
    pub const fn break_even(&self, multiplier: u64) -> u64 {
        self.floor + multiplier * self.cold
    }
}

/// Every multiplier the document's own guards compare the discardable reading against.
///
/// ⚠⚠ Plural on purpose: `review.done` and `review.none` each carry one, and two guards that
/// disagree are a document in which *did the reviewer find a habit* decides the price of a
/// handover. The caller asserts they agree; this only reports what is written.
#[must_use]
pub fn guard_multipliers(scxml: &str) -> Vec<u64> {
    let mut found = Vec::new();
    for (at, _) in scxml.match_indices(GUARD) {
        let rest = scxml[at + GUARD.len()..].trim_start();
        let rest = rest
            .strip_prefix("&gt;=")
            .or_else(|| rest.strip_prefix(">="))
            .unwrap_or(rest)
            .trim_start();
        let Some((multiplier, end)) = number_at(rest, 0) else {
            continue;
        };
        if rest[end..].trim_start().starts_with("* cold") {
            found.push(multiplier);
        }
    }
    found
}

/// Every multiplier the FIXTURE spells in its own arithmetic — see [`FIXTURE_TOLL`].
///
/// ⚠ Empty is a legitimate answer and the caller decides what it means: a fixture that stopped
/// computing a toll at all has nothing to disagree with the document about.
#[must_use]
pub fn fixture_multipliers(rust: &str) -> Vec<u64> {
    rust.match_indices(FIXTURE_TOLL)
        .filter_map(|(at, _)| {
            let before = rust[..at].trim_end();
            number_ending_at(rust, before.len())
        })
        .collect()
}

/// What the template says it measured, or why this could not be read as a claim.
///
/// # ⚠⚠⚠⚠ The boundary is the COMMENT BLOCK, and inside it the SENTENCE
///
/// Item 494 measured what happens without one: a rule that read a whole block claimed the example
/// the block was discussing. The break-even is found by its shape anywhere in the template; the
/// RATE has to share a sentence with it, and the DATE has to share the block. That is what keeps
/// this from reading *"it read 25 sessions of the 255 and reported 4 of 25"* — the paragraph's own
/// account of the superseded filing, two sentences later — as the claim.
///
/// # ⚠ Exactly one block may state it
///
/// A second block stating a second sum is an ambiguity, not a claim: the pin below would silently
/// pick whichever came first, and the other could then say anything. It refuses instead.
pub fn stated(scxml: &str) -> Result<Stated, String> {
    let mut claims: Vec<Stated> = Vec::new();
    for (open, _) in scxml.match_indices("<!--") {
        let rest = &scxml[open + 4..];
        let Some(close) = rest.find("-->") else {
            continue;
        };
        let flat = flatten(&rest[..close]);
        let Some((multiplier, cold, floor, sum, end)) = arithmetic(&flat) else {
            continue;
        };
        let sentence = sentence_from(&flat, end);
        let Some((reached, population)) = rate_in(&sentence) else {
            return Err(format!(
                "⚠⚠⚠ the block stating the break-even says nothing about HOW MANY sessions reach \
                 it, which is the whole finding: an edge a fifth of sessions take is not the same \
                 fact as one every session takes. Sentence: {sentence}"
            ));
        };
        let Some(dated) = date_in(&flat) else {
            return Err(format!(
                "⚠⚠⚠ the break-even is stated with no DATE in its block. A measurement without one \
                 cannot be told from folklore and nothing can say it has gone stale (register item \
                 456). Sentence: {sentence}"
            ));
        };
        claims.push(Stated {
            multiplier,
            cold,
            floor,
            sum,
            reached,
            population,
            dated,
            sentence,
        });
    }
    match claims.len() {
        1 => Ok(claims.remove(0)),
        0 => Err(format!(
            "⚠⚠⚠⚠⚠ no comment block in `{TEMPLATE}` states the break-even as arithmetic. The \
             paragraph that prices a restart must show its sum — `N * cold + floor` = the reading \
             — or the numbers beside it are a claim nothing can check."
        )),
        many => Err(format!(
            "⚠⚠⚠ {many} blocks state a break-even. A pin that picked one of them would leave the \
             others free to say anything: {:?}",
            claims.iter().map(|c| &c.sentence).collect::<Vec<_>>()
        )),
    }
}

/// The three costs the fixture declares, or why they could not be read.
pub fn sample(rust: &str) -> Result<Sample, String> {
    let opened = format!("{SAMPLE}: Billed = Billed {{");
    let Some(at) = rust.find(&opened) else {
        return Err(format!(
            "⚠⚠⚠⚠ `{FIXTURE}` declares no `{SAMPLE}`. The economics gates drive ONE trio and this \
             is where it lives; a fixture that spelled its numbers at each site again is the defect \
             register item 493 closed."
        ));
    };
    let rest = &rust[at + opened.len()..];
    let Some(close) = rest.find('}') else {
        return Err(format!("⚠ `{SAMPLE}` is never closed"));
    };
    let body = &rest[..close];
    let field = |name: &str| -> Result<u64, String> {
        let needle = format!("{name}:");
        let at = body.find(&needle).ok_or_else(|| {
            format!("⚠⚠ `{SAMPLE}` declares no `{name}`, so nothing can price the trade: {body}")
        })?;
        number_at(body[at + needle.len()..].trim_start(), 0)
            .map(|(value, _)| value)
            .ok_or_else(|| format!("⚠⚠ `{SAMPLE}`'s `{name}` is not a number: {body}"))
    };
    Ok(Sample {
        cold: field("cold")?,
        floor: field("floor")?,
        context: field("context")?,
    })
}

/// `N * X + Y` = `Z`, wherever it stands, with the byte the sum ends at.
///
/// ⚠ The separators of both artefacts are accepted (`28,981` in prose, `28_981` in Rust) so the
/// same reader can be pointed at either. ⚠⚠ The guards are written `... &gt;= 20 * cold`, where
/// what follows the multiplier is a WORD — so a transition can never be mistaken for the sum.
fn arithmetic(flat: &str) -> Option<(u64, u64, u64, u64, usize)> {
    for (at, _) in flat.match_indices(" * ") {
        let Some(multiplier) = number_ending_at(flat, at) else {
            continue;
        };
        let Some((cold, after_cold)) = number_at(flat, at + 3) else {
            continue;
        };
        let Some(plus) = flat[after_cold..].strip_prefix(" + ") else {
            continue;
        };
        let Some((floor, after_floor)) = number_at(plus, 0) else {
            continue;
        };
        let tail = &plus[after_floor..];
        let Some(equals) = tail
            .strip_prefix("` = ")
            .or_else(|| tail.strip_prefix(" = "))
            .or_else(|| tail.strip_prefix("` is "))
        else {
            continue;
        };
        let Some((sum, after_sum)) = number_at(equals, 0) else {
            continue;
        };
        let end = flat.len() - equals.len() + after_sum;
        return Some((multiplier, cold, floor, sum, end));
    }
    None
}

/// From `at` to the end of the sentence it sits in — the bound the rate must fall inside.
fn sentence_from(flat: &str, at: usize) -> String {
    let rest = &flat[at.min(flat.len())..];
    let end = rest.find(". ").map_or(rest.len(), |stop| stop + 1);
    rest[..end].trim().to_owned()
}

/// `n of the m sessions`, which is what makes a door RARE rather than merely reachable.
fn rate_in(sentence: &str) -> Option<(u64, u64)> {
    for (at, _) in sentence.match_indices(" of ") {
        let Some(reached) = number_ending_at(sentence, at) else {
            continue;
        };
        let rest = &sentence[at + 4..];
        let rest = rest.strip_prefix("the ").unwrap_or(rest);
        let Some((population, end)) = number_at(rest, 0) else {
            continue;
        };
        if rest[end..].trim_start().starts_with("session") {
            return Some((reached, population));
        }
    }
    None
}

/// The first `YYYY-MM-DD` in the block, read as a shape rather than matched against a list.
fn date_in(flat: &str) -> Option<String> {
    let bytes = flat.as_bytes();
    let digits = |from: usize, how_many: usize| {
        from + how_many <= bytes.len()
            && bytes[from..from + how_many].iter().all(u8::is_ascii_digit)
    };
    (0..bytes.len()).find_map(|at| {
        (digits(at, 4)
            && bytes.get(at + 4) == Some(&b'-')
            && digits(at + 5, 2)
            && bytes.get(at + 7) == Some(&b'-')
            && digits(at + 8, 2))
        .then(|| flat[at..at + 10].to_owned())
    })
}

/// A run of digits at `from`, ignoring the separators either artefact spells them with.
fn number_at(text: &str, from: usize) -> Option<(u64, usize)> {
    let mut value: u64 = 0;
    let mut at = from;
    let mut any = false;
    for byte in text.as_bytes()[from..].iter() {
        match byte {
            b'0'..=b'9' => {
                value = value.checked_mul(10)?.checked_add(u64::from(byte - b'0'))?;
                any = true;
            }
            b',' | b'_' if any => {}
            _ => break,
        }
        at += 1;
    }
    // ⚠ A trailing separator is not part of the number: `49 of the 250, and …` must read 250.
    while at > from && matches!(text.as_bytes()[at - 1], b',' | b'_') {
        at -= 1;
    }
    any.then_some((value, at))
}

/// The run of digits ENDING at `at`, for the left-hand side of `N * X`.
fn number_ending_at(text: &str, at: usize) -> Option<u64> {
    let bytes = text.as_bytes();
    let mut from = at;
    while from > 0 && matches!(bytes[from - 1], b'0'..=b'9' | b',' | b'_') {
        from -= 1;
    }
    (from < at)
        .then(|| number_at(text, from))
        .flatten()
        .and_then(|(value, end)| (end == at).then_some(value))
}

/// A comment block as one line, because the template wraps at 70 columns and a claim is a sentence.
fn flatten(block: &str) -> String {
    block.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::{arithmetic, guard_multipliers, rate_in, sample, stated};

    /// ⚠⚠⚠⚠⚠ **THE DERIVATION IS EXERCISED ON TEXT THIS FILE WROTE, so a red in the gate beside it
    /// is a claim about the product rather than about this reader.**
    ///
    /// Register item 470's lesson in one table: a ratchet cannot see that its own needle has gone
    /// blind, and the only thing that can is a table of cases where each check is the ONLY thing
    /// wrong. Every row here is the claim with one fact removed.
    #[test]
    fn each_missing_half_of_the_claim_is_refused_by_itself() {
        let whole = "<!-- Measured 2026-08-20: a restart pays for itself only past `20 * 28,981 + \
                     21,350` = 600,970 tokens of reading, and 49 of the 250 sessions ever got \
                     there. An earlier filing read 25 sessions of the 255 and reported 4 of 25. -->";
        let read = stated(whole).expect("the whole claim reads");
        assert_eq!(
            (read.multiplier, read.cold, read.floor, read.sum),
            (20, 28_981, 21_350, 600_970),
            "the arithmetic is taken from the sentence, separators and all",
        );
        assert_eq!(
            (read.reached, read.population),
            (49, 250),
            "⚠⚠⚠⚠⚠ AND NOT `4 of 25`, which the same block states two sentences later about the \
             filing this one superseded. A rule that read the whole block would take the example \
             for the claim — register item 494 measured exactly that.",
        );
        assert_eq!(read.dated, "2026-08-20", "and the date is the block's own");

        let rateless = whole.replace(", and 49 of the 250 sessions ever got there", "");
        assert!(
            stated(&rateless).is_err(),
            "⚠⚠⚠ a break-even with no rate must be refused: *reachable* and *ordinary* are \
             different claims and the fixture used to make the second one silently",
        );

        let undated = whole.replace("2026-08-20", "recently");
        assert!(
            stated(&undated).is_err(),
            "⚠⚠ and an undated measurement is folklore — register item 456",
        );

        let twice = format!("{whole}{whole}");
        assert!(
            stated(&twice).is_err(),
            "⚠⚠⚠ two blocks stating a sum is an ambiguity: a pin that took the first would leave \
             the second free to say anything",
        );

        assert!(
            stated("<!-- no sum here, 250 sessions, 2026-08-20 -->").is_err(),
            "⚠⚠⚠⚠ and prose with the numbers but no ARITHMETIC states nothing this can recompute",
        );
    }

    /// ⚠⚠⚠⚠ **THE GUARD IS WHERE THE MULTIPLIER COMES FROM**, and a document whose two guards
    /// disagree must be visible as two readings rather than averaged into one.
    #[test]
    fn the_multiplier_is_read_off_every_guard_that_prices_a_handover() {
        let both = r#"<transition cond="context - floor &gt;= 20 * cold" target="restarting"/>
                      <transition cond="context - floor &gt;= 20 * cold" target="restarting"/>"#;
        assert_eq!(guard_multipliers(both), vec![20, 20]);

        let split = both.replacen("20 * cold", "15 * cold", 1);
        assert_eq!(
            guard_multipliers(&split),
            vec![15, 20],
            "⚠⚠⚠ two guards, two prices — the caller is what refuses this, and it cannot refuse \
             what it cannot see",
        );

        assert!(
            guard_multipliers(r#"cond="context - floor &gt;= cold""#).is_empty(),
            "⚠ a guard with no multiplier at all is not a price this can recompute",
        );
        assert_eq!(
            super::fixture_multipliers("const fn toll(&self) -> u64 {\n        20 * self.cold\n}"),
            vec![20],
            "⚠⚠⚠ and the FIXTURE's own arithmetic spells one too — the single number item 493's \
             repair could not derive, so it is pinned instead of trusted",
        );
        assert!(
            arithmetic("the loop restarts 20 * cold times").is_none(),
            "⚠⚠ and a guard can never be mistaken for the paragraph's sum: what follows the \
             multiplier there is a WORD",
        );
    }

    /// ⚠⚠ The fixture side, which is the half a gate reading only prose would leave free to drift.
    #[test]
    fn the_fixtures_three_costs_are_read_where_they_are_declared() {
        let declared = "pub(crate) const MEASURED_HERE: Billed = Billed {\n    cold: 28_981,\n    \
                        floor: 21_350,\n    context: 696_747,\n};";
        assert_eq!(
            sample(declared).map(|read| (read.cold, read.floor, read.context)),
            Ok((28_981, 21_350, 696_747)),
        );
        assert!(
            sample("pub(crate) const OTHER: Billed = Billed { cold: 1 };").is_err(),
            "⚠⚠⚠ a fixture that renamed or deleted the sample must be a refusal rather than a \
             gate that quietly measures nothing",
        );
        assert!(
            sample("pub(crate) const MEASURED_HERE: Billed = Billed {\n    cold: 1,\n};").is_err(),
            "⚠⚠ and one that dropped a cost cannot price the trade",
        );
    }

    /// ⚠ The rate reader alone, because *49 of the 250 sessions* and *49 of the 250 requests* are
    /// different claims and only one of them is this door's.
    #[test]
    fn a_rate_is_of_sessions_and_of_nothing_else() {
        assert_eq!(
            rate_in("and 49 of the 250 sessions got there"),
            Some((49, 250))
        );
        assert_eq!(rate_in("and 49 of 250 sessions got there"), Some((49, 250)));
        assert_eq!(
            rate_in("and 49 of the 250 requests got there"),
            None,
            "⚠⚠ a rate over requests is not a rate over sessions, and the door is a session's",
        );
        assert_eq!(rate_in("a fifth of the sessions got there"), None);
    }
}
