//! Whether every date this workspace can print comes from the ONE place that owns a calendar —
//! register item 918.
//!
//! # ⛔⛔⛔⛔⛔ The absence this replaces, and what it cost
//!
//! Measured 2026-09-06T08:01:08Z, **nothing under `crates/` knew what a date was**: no time crate
//! is a dependency of any package here, and the only `%Y` in the tree was a test fixture asking a
//! live agent to run `date -u +%Y` — the product asked somebody else for the date because it could
//! not say one. So every number this workspace published over a LIVE store went out with no moment
//! on it, and a person was expected to type one beside it by hand.
//!
//! Register item 895 measured what that costs. Three readings of one ratio, same method, came out
//! `0.3130` · `0.3114` · `0.3036`, and because the first carried no moment a later round read the
//! third as a CONTRADICTION of it rather than as a second measurement.
//!
//! # ⛔⛔⛔ Why a ratchet and not a comment on the type
//!
//! `sprag_host::moment::Reading` now owns the conversion and is gated against days a person can
//! check with `date -u`. That gate proves ONE calendar right; it cannot notice a SECOND one. And a
//! second is the likely shape of the next mistake — a mouth in a hurry, a `days / 365`, a
//! `% 4`-only leap rule — which would print an instant that looks exactly like a true one while
//! being wrong on the days that matter.
//!
//! ⇒ So it is asked, every run: is the era arithmetic still in exactly one file. A comment saying
//! so is a claim nobody re-derives, which is this workspace's own rule.

use crate::sources::Source;

/// The one file allowed to turn a clock into words.
pub const THE_DOOR: &str = "crates/sprag-host/src/moment.rs";

/// The constants a civil-date conversion cannot be written without.
///
/// # ⚠⚠ Why constants and not the word *date*
///
/// The subject is not a mention, it is the ARITHMETIC. `146_097` (days in a 400-year Gregorian era)
/// and `719_468` (the Unix epoch's offset into an era beginning in March) appear in every closed
/// form of this conversion and in nothing else — and a hand-rolled calendar that avoided both would
/// be one that also avoided being right, which the second needle catches: `36_524` is the days in a
/// century, the correction a `% 4` leap rule leaves out.
///
/// ⚠ Underscores are removed before matching, so a second calendar cannot slip past by writing the
/// era length in the literal style this crate does not use.
///
/// ⛔⛔⛔⛔⛔ **AND THE NEEDLES THEMSELVES ARE SPLIT ACROSS `concat!`** — the gate went red on this
/// very line the first time it ran, which is the gate being RIGHT about this file rather than the
/// gate being dodged. Written whole, a needle table IS the arithmetic as far as a source scan is
/// concerned. The alternative was an exemption for this file, and an exemption list is the shape
/// that hollows a ratchet out: it grows, and the day a real conversion lands here it is already
/// excused. `refusals::DOORS` reached the same crossing and took the same turn.
const ERA_ARITHMETIC: [&str; 3] = [
    concat!("146", "097"),
    concat!("719", "468"),
    concat!("36", "524"),
];

/// Whether `line` does the arithmetic, as opposed to mentioning a date.
#[must_use]
pub fn converts_a_civil_date(line: &str) -> bool {
    let squeezed: String = line
        .chars()
        .filter(|char| !char.is_whitespace() && *char != '_')
        .collect();
    ERA_ARITHMETIC.iter().any(|era| squeezed.contains(era))
}

/// Every site outside [`THE_DOOR`] that converts one, as `(file, line, text)`.
#[must_use]
pub fn strays(sources: &[Source]) -> Vec<(String, usize, String)> {
    sources
        .iter()
        .filter(|source| source.file != THE_DOOR)
        .flat_map(|source| {
            source
                .code
                .iter()
                .filter(|(_, line)| converts_a_civil_date(line))
                .map(|(at, line)| (source.file.clone(), *at, line.clone()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{THE_DOOR, converts_a_civil_date, strays};
    use crate::sources::rust_sources;

    /// ⛔⛔⛔⛔⛔ **EVERY DATE THIS WORKSPACE PRINTS COMES FROM THE ONE PLACE THAT OWNS A
    /// CALENDAR** — register item 918.
    #[test]
    fn only_one_door_turns_a_clock_into_words() {
        let sources = rust_sources();
        // ⚠ THE PROBE IS AIMED FIRST. A needle that matched nothing anywhere would make the
        // assertion below pass by finding nothing — the shape this crate's own walker refuses for
        // the same reason, and the shape item 914 measured a whole gate driving zero rows in.
        assert!(
            sources.iter().any(|source| source.file == THE_DOOR
                && source.code.iter().any(|(_, l)| converts_a_civil_date(l))),
            "⛔ the probe found no civil-date arithmetic even at {THE_DOOR}, so it is pointed at \
             nothing and the sweep below proves nothing",
        );

        let strays = strays(&sources);
        assert!(
            strays.is_empty(),
            "⛔⛔⛔⛔⛔ A SECOND CALENDAR. {THE_DOOR} is checked against days a person can verify \
             with `date -u` — a leap year's own boundary, the same days in a common year, the \
             century that is NOT a leap year and the 400-year exception to it — and a conversion \
             written anywhere else carries none of that. It will print an instant that looks \
             exactly like a true one, which for a stamp whose whole job is making a quotation \
             checkable is the worst possible failure (register item 918). Route it through \
             `sprag_host::moment::Reading`. Found: {strays:?}",
        );
    }

    /// ⚠⚠ **AND THE PROBE TELLS ARITHMETIC FROM A MENTION**, or the sweep above is either noise or
    /// blind — driven on lines rather than on the tree, so both halves are asked.
    #[test]
    fn the_probe_reads_the_era_arithmetic_and_not_a_mention() {
        // ⛔⛔⛔⛔⛔ THE POSITIVE FIXTURES ARE SPLIT ACROSS `concat!` AND THAT IS NOT THE GATE
        // BEING DODGED — it is the gate being right about this file. Written whole they ARE the
        // arithmetic as far as a source scan is concerned, and the sweep above would find them
        // here. The alternative is an exemption for this file, which is the shape that hollows a
        // gate out: the list grows, and the day something real lands here it is already excused.
        for converting in [
            concat!("let shifted = days + 719", "_468;"),
            concat!("let era = shifted / 146", "097;"),
            concat!("+ day_of_era / 36", "_524"),
        ] {
            assert!(
                converts_a_civil_date(converting),
                "⚠⚠ the probe missed a civil-date conversion, so the sweep is blind: {converting}",
            );
        }
        // ⚠ AND THE OTHER HALF: prose, a run count and an ordinary duration are not calendars, and
        // a probe that fired on them would have to be narrowed until it fired on nothing.
        for innocent in [
            "/// the moment this store was read, in the words the register quotes",
            "let seconds = wait.seconds + 146;",
            "assert_eq!(log.runs.len(), 245);",
            "const STOP_DEADLINE: Duration = Duration::from_secs(36);",
        ] {
            assert!(
                !converts_a_civil_date(innocent),
                "⚠ the probe fired on a line that computes no date: {innocent}",
            );
        }
    }
}
