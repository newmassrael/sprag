//! ⛔⛔⛔⛔⛔ **WHEN A LIVE STORE WAS READ** — register item 918, and the half of a quotation this
//! product left to a person's memory.
//!
//! # ⛔⛔⛔⛔⛔ A number taken over a live store cannot be re-derived, only re-taken
//!
//! Every analysis mouth here reads a file a daemon is still appending to. Running the command again
//! is **a new measurement**, not a check of the old one — so two readings that differ have not
//! found a discrepancy, and nothing can say which is which unless the first carried the moment it
//! was read.
//!
//! Register item 895 paid for that in full. Three readings of one ratio, same method, came out
//! `0.3130` · `0.3114` · `0.3036`, and because the first carried no moment a later round read the
//! third as a CONTRADICTION of it. That item's own conclusion is this module's charter: *write the
//! number beside the moment it was read; the command takes another measurement.*
//!
//! # ⚠⚠ Why the product stamps it rather than the person quoting it
//!
//! The person is exactly who forgets — item 895's three readings were all taken by people who knew
//! the rule. And a moment spelled at each mouth would be item 895's OTHER finding one level down
//! (four readers of one store, four filters, two counts of one population differing 8 against 10),
//! so the clock is read where the FILE is read and the words are composed here and nowhere else.
//!
//! # ⚠⚠⚠ There was no calendar in this workspace, which is why this is arithmetic and not a crate
//!
//! Measured 2026-09-06T08:01:08Z: no time crate is a dependency of any package here, and the only
//! `%Y` under `crates/` is a test fixture that asks a live agent to run `date -u +%Y` — the product
//! asked somebody else for the date because it could not say one. Adding a dependency to print
//! twenty characters would be the larger change; the civil-from-days conversion below is a closed
//! form, and [`Reading`]'s gate holds it against values a person can check with `date -u`.

/// ⛔⛔⛔⛔⛔ **THE MOMENT A LIVE STORE WAS READ**, and the one place this workspace turns a clock
/// into words — register item 918.
///
/// # ⛔⛔⛔ Why the unreadable clock is an ARM and not a zero
///
/// `SystemTime::now()` can fail to place itself after the epoch, and the obvious repair — call it
/// zero — would print `1970-01-01T00:00:00Z`, **a real-looking instant**. For a type whose entire
/// job is making a quotation checkable, a plausible wrong moment is the worst possible answer: it
/// cannot be told from a true one by the reader it exists to serve. So it is said out loud, this
/// workspace's rule 6, and the sentence a reader gets names the machine rather than a date.
///
/// # ⚠ UTC, no leap seconds — the same instant `date -u` prints
///
/// The conversion is Unix-epoch seconds to the proleptic Gregorian calendar, which is what
/// `date -u +%Y-%m-%dT%H:%M:%SZ` answers on this machine and what the register's own entries are
/// written in. A quoted line therefore pastes into the ledger unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Reading {
    /// Seconds since the Unix epoch, UTC.
    At(u64),
    /// ⛔ **THE CLOCK WOULD NOT SAY.** Never rendered as an instant; see the type.
    Unreadable,
}

impl Reading {
    /// **TAKE THE CLOCK NOW** — called where a file is READ, so the moment belongs to the read and
    /// not to whichever mouth got round to printing it.
    #[must_use]
    pub fn now() -> Self {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(Self::Unreadable, |since| Self::At(since.as_secs()))
    }

    /// A named moment, for a gate — the clock is the one input a test may not have.
    #[must_use]
    pub const fn at(unix_seconds: u64) -> Self {
        Self::At(unix_seconds)
    }
}

impl std::fmt::Display for Reading {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self::At(seconds) = *self else {
            return out.write_str("a moment this machine's clock could not state");
        };
        let (days, rest) = (seconds / 86_400, seconds % 86_400);
        let (year, month, day) = civil_from_days(days);
        write!(
            out,
            "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
            rest / 3_600,
            (rest % 3_600) / 60,
            rest % 60,
        )
    }
}

/// Days since the Unix epoch to a proleptic Gregorian `(year, month, day)`.
///
/// # ⚠⚠ The shifted era, and why the constants look arbitrary
///
/// The closed form counts from an era beginning on 0000-03-01, which puts the leap day at the END
/// of a year and removes every special case for February: `719_468` is the epoch's offset into that
/// scheme, `146_097` is the days in a 400-year era, and the `1460 / 36524 / 146_096` corrections are
/// the three Gregorian leap rules in that order. It is exact for every day this program can hold —
/// there is no accumulated drift and no table to age.
///
/// ⚠ It is NOT a re-derivation of a rule stated elsewhere in this tree: nothing else here knows
/// what a date is, which the module doc measures. The gate holds it against days a person can check
/// with `date -u`, including the two a leap rule gets wrong when it is written by hand.
const fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let shifted = days + 719_468;
    let era = shifted / 146_097;
    let day_of_era = shifted % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    // The era's own month numbering: 0 is March, so 10 is January of the FOLLOWING calendar year.
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + if month <= 2 { 1 } else { 0 };
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::Reading;

    /// ⛔⛔⛔⛔⛔ **THE CALENDAR IS ARITHMETIC NOBODY IN THIS TREE HAD WRITTEN BEFORE, SO IT IS
    /// CHECKED AGAINST DAYS A PERSON CAN VERIFY** — register item 918.
    ///
    /// # ⚠⚠ Which days, and why these rather than a handful of round numbers
    ///
    /// Every pair below is a day an off-by-one in a leap rule gets WRONG while the ordinary days
    /// around it stay right — that is the whole failure mode of a hand-written civil conversion,
    /// and a gate made of arbitrary timestamps would pass a build that had it:
    ///
    /// | pair | what it pins |
    /// |---|---|
    /// | the epoch | the `719_468` offset itself |
    /// | 2024-02-28 / 29 / 03-01 | a leap year's own boundary, both sides of the extra day |
    /// | 2023-02-28 / 03-01 | the SAME days in a common year, so the rule is not always-on |
    /// | 2100-02-28 / 03-01 | the century that is NOT a leap year — the `36_524` correction |
    /// | 2000-02-29 | the 400-year exception to that exception — the `146_096` correction |
    /// | 2026-12-31T23:59:59 | the last second of a year, which is where a day rolls early |
    ///
    /// ⚠ The expected strings were taken from `date -u -d @<seconds>` on this machine on
    /// 2026-09-06, which is the same authority the register's own entries are written against.
    #[test]
    fn the_moment_a_store_was_read_is_the_instant_date_u_would_print() {
        for (seconds, expected) in [
            (0u64, "1970-01-01T00:00:00Z"),
            // ⛔ A LEAP YEAR'S OWN BOUNDARY, all three days, because a conversion that drops the
            // extra day is right on the 28th and wrong for the rest of the year after it.
            (1_709_078_400, "2024-02-28T00:00:00Z"),
            (1_709_164_800, "2024-02-29T00:00:00Z"),
            (1_709_251_200, "2024-03-01T00:00:00Z"),
            // ⚠ THE CONTROL: the same two dates in a COMMON year. Without it a build that always
            // inserted the day would pass every leap-year row above.
            (1_677_542_400, "2023-02-28T00:00:00Z"),
            (1_677_628_800, "2023-03-01T00:00:00Z"),
            // ⛔⛔ THE CENTURY THAT IS NOT A LEAP YEAR — divisible by 4 and by 100. A `% 4` rule
            // alone puts a 29th here.
            (4_107_456_000, "2100-02-28T00:00:00Z"),
            (4_107_542_400, "2100-03-01T00:00:00Z"),
            // ⛔⛔⛔ AND THE EXCEPTION TO THAT EXCEPTION: divisible by 400, so the day IS there.
            // A rule stopped one correction short reports 2000-03-01 for this.
            (951_782_400, "2000-02-29T00:00:00Z"),
            // ⚠ THE LAST SECOND OF A YEAR, where a day computed by rounding rolls early.
            (1_798_761_599, "2026-12-31T23:59:59Z"),
            // ⚠ AND A MOMENT WITH EVERY FIELD NON-ZERO, so a field printed in the wrong slot
            // cannot hide behind a row of zeros.
            (1_788_681_668, "2026-09-06T08:01:08Z"),
        ] {
            assert_eq!(
                Reading::at(seconds).to_string(),
                expected,
                "⛔⛔⛔⛔⛔ REGISTER ITEM 918: a quoted number carries this string, and the whole \
                 point of carrying it is that a later reading can be told from a discrepancy. A \
                 calendar that is wrong here makes every quotation confidently unverifiable. \
                 Seconds: {seconds}",
            );
        }
    }

    /// ⛔⛔⛔ **A CLOCK THAT WOULD NOT SAY MUST NOT PRINT A DATE** — register item 918, and this
    /// workspace's rule 6 applied to the one input this type does not control.
    ///
    /// The repair everyone reaches for is a zero, and a zero here renders `1970-01-01T00:00:00Z` —
    /// an instant a reader cannot tell from a true one. So the two are asserted APART, which is the
    /// same shape register item 894 ⑥ used for a ceiling nobody set.
    #[test]
    fn a_clock_that_would_not_say_is_not_rendered_as_an_instant() {
        let silent = Reading::Unreadable.to_string();
        assert!(
            !silent.contains('Z') && silent.contains("could not state"),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 918: an unreadable clock must SAY so. Got: {silent}",
        );
        assert_ne!(
            silent,
            Reading::at(0).to_string(),
            "⛔⛔⛔⛔⛔ AND IT MUST NOT BE THE EPOCH: *the clock would not answer* and *this was \
             read at the epoch* are different facts, and folding them gives a reader a date that \
             is wrong by fifty-six years while looking exactly like every other line.",
        );
    }
}
