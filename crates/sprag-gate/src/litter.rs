//! **WHAT A SUITE LEFT BEHIND IN THE MACHINE'S SCRATCH DIRECTORY** — register item 927.
//!
//! # ⛔⛔⛔⛔⛔ Why this is TWO numbers and not one
//!
//! Measured 2026-09-06 13:40:27 UTC, on the workstation this repository is developed on:
//!
//! ```text
//! all                : 13870 dirs   19931.8 MB
//! the 22 largest     :    22 dirs   19586.6 MB   98.3% of the bytes, 0.16% of the count
//! everything else    : 13848 dirs     345.2 MB    1.7% of the bytes, 99.84% of the count
//! ```
//!
//! **A gate that counted either number alone would report that the other one did not move.** The
//! big directories are build and check targets — a handful, hundreds of megabytes each. The many
//! are per-case scratch directories a few kilobytes each, and there are fourteen thousand of them.
//! Between 12:43 and 13:40 on the day this was written the COUNT rose by seventy-four while the
//! BYTES did not change at all, because the only thing that ran in between was a test suite.
//!
//! That is register item 794's lesson — *when you write a number, write what it is the number OF*
//! — arriving in the form *one instrument cannot answer this with one number*.
//!
//! # ⚠⚠ It reports; it does not refuse a size
//!
//! [`sprag_gate::pty_demand`](crate::pty_demand)'s stance, for its reason: **a threshold picked
//! before the first honest measurement is a number somebody keeps.** What this DOES refuse is a
//! reading that was not made — a root that is not a directory, or one it cannot list. Neither of
//! those is a machine with no litter on it, and letting them read as zero is the shape that makes a
//! gate green about nothing (register items 914 and 924).
//!
//! # ⛔⛔⛔ There is NO exemption list, and that is a decision rather than an omission
//!
//! Item 927's own measuring command excludes `sprag-check-target` by name, because that one is a
//! build cache and counting it makes the total *"a number of the wrong thing"*. That is true of the
//! total and it is the reason this module reports **per series** as well: attribution does the work
//! an exclusion would, and it does it without anybody maintaining a list of what is allowed to be
//! large. A list like that is precisely what register item 926 measured going stale — it was
//! written when it was true, and nothing re-asked. Every directory is counted, every directory is
//! attributed, and the reader separates cache from litter by reading the series rather than by
//! trusting that somebody kept the list in step.

use std::collections::BTreeMap;

// ⛔⛔⛔⛔⛔ THERE IS NO `default_root()` HERE, AND ITS ABSENCE WAS DECIDED BY A GATE.
//
// The first draft of this module carried one: it read `std::env::temp_dir()` and re-applied
// register item 794's `is_absolute` refusal inline, on the argument that this crate's charter
// (*a gate binary must build when the product does not*) keeps `sprag-scratch` a **dev**-dependency
// and therefore out of a `[[bin]]`'s reach, so three duplicated lines were the lesser evil.
//
// `no_product_code_takes_a_scratch_root_unchecked` refused it, by name and line:
//
//   ⛔ ITEM 794: product code took a scratch root from the operating system and trusted it.
//   crates/sprag-gate/src/litter.rs:66: let raw = std::env::temp_dir();
//
// ⚠⚠ THE GATE WAS RIGHT AND THE ARGUMENT WAS WRONG. "Duplicate the policy" and "take the
// dependency" were presented as the only two moves, and there is a third: **the instrument does not
// need to guess a root at all.** It is told which one to read. That removes the duplicate, keeps
// the charter, and makes the reading say out loud what it was a reading OF — which is item 794's
// actual lesson rather than its mechanism.

/// One family of scratch directories, named by the shape of their names rather than by a list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Series {
    /// The stem every member shares, with the varying parts replaced — `sprag-gate-N-clean-T`.
    pub stem: String,
    /// How many directories carry it.
    pub dirs: usize,
    /// What they hold, in bytes.
    pub bytes: u64,
}

/// A reading of one scratch root: the two totals, and the series behind them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading {
    /// How many top-level scratch directories were found.
    pub dirs: usize,
    /// What they hold in total, in bytes.
    pub bytes: u64,
    /// The families, largest count first, then largest bytes, then by name so it is stable.
    pub series: Vec<Series>,
}

impl Reading {
    /// Whether this reading found nothing — the state a clean machine is in.
    ///
    /// ⚠ This is a question about a reading that WAS made. [`read`]'s caller refuses the readings
    /// that could not be made, so an empty answer here never means *nobody looked*.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.dirs == 0 && self.bytes == 0
    }
}

/// The name a directory's family is known by: every run of digits becomes `N`, and a
/// `ThreadId(..)` becomes `T`.
///
/// # ⚠⚠ Why the stem is DERIVED and not matched against a list of known prefixes
///
/// The families here were not designed; they are whatever the harnesses happen to name their
/// scratch. A list would answer only about the ones somebody had already noticed, and the whole
/// value of this instrument is naming the one nobody has. Deriving the stem means a family that
/// appears tomorrow is attributed the day it appears, under a name that says what it is.
#[must_use]
pub fn stem_of(name: &str) -> String {
    let mut stem = String::with_capacity(name.len());
    let mut rest = name;
    // `ThreadId(123)` first, so its digits do not become `N` and leave `ThreadId(N)` behind.
    while let Some(at) = rest.find("ThreadId(") {
        stem.push_str(&rest[..at]);
        stem.push('T');
        rest = match rest[at..].find(')') {
            Some(close) => &rest[at + close + 1..],
            None => "",
        };
    }
    stem.push_str(rest);

    let mut out = String::with_capacity(stem.len());
    let mut in_digits = false;
    for ch in stem.chars() {
        if ch.is_ascii_digit() {
            if !in_digits {
                out.push('N');
                in_digits = true;
            }
        } else {
            in_digits = false;
            out.push(ch);
        }
    }
    out
}

/// Turn what a scratch root holds into a [`Reading`].
///
/// The impure half — listing a directory and measuring what each entry holds — happens at the
/// caller's seam, so every case below can be handed to this function directly. That is the shape
/// `sprag_scratch`'s own `root_from` uses, and `sprag_rpc`'s socket policy before it: the read is
/// at one edge and the decision is a pure function of what it read.
///
/// ⚠ `sprag_scratch` is named WITHOUT an intra-doc link on purpose: it is a **dev**-dependency of
/// this crate — the charter is that a gate binary must build when the product does not — so the
/// link resolves nowhere and this repository's doc gate runs with `-D warnings`. The crate being
/// named already carries the same note about its own private `root_from`, for the same reason: a
/// doc link is only as good as what can reach it.
///
/// ⚠ It takes what was found, not where to look. A caller that found nothing because it looked in
/// the wrong place must say so itself — this function cannot tell that from a clean machine, and
/// pretending otherwise is how *nobody looked* comes to read as *nothing there*.
#[must_use]
pub fn read(found: impl IntoIterator<Item = (String, u64)>) -> Reading {
    let mut dirs = 0usize;
    let mut bytes = 0u64;
    let mut families: BTreeMap<String, (usize, u64)> = BTreeMap::new();

    for (name, held) in found {
        dirs += 1;
        bytes += held;
        let entry = families.entry(stem_of(&name)).or_insert((0, 0));
        entry.0 += 1;
        entry.1 += held;
    }

    let mut series: Vec<Series> = families
        .into_iter()
        .map(|(stem, (dirs, bytes))| Series { stem, dirs, bytes })
        .collect();
    // Count first: this instrument exists because the count and the bytes disagree about which
    // family matters, and the count is the half no other gate in this workspace looks at.
    series.sort_by(|a, b| {
        b.dirs
            .cmp(&a.dirs)
            .then(b.bytes.cmp(&a.bytes))
            .then(a.stem.cmp(&b.stem))
    });

    Reading {
        dirs,
        bytes,
        series,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_family_is_named_by_what_varies_in_it() {
        assert_eq!(
            stem_of("sprag-promoted-1016056-ThreadId(331)"),
            "sprag-promoted-N-T"
        );
        assert_eq!(
            stem_of("sprag-check-target-997185405"),
            "sprag-check-target-N"
        );
        assert_eq!(
            stem_of("sprag-check-2197492-326602367-target"),
            "sprag-check-N-N-target"
        );
        assert_eq!(stem_of("sprag-verify-target"), "sprag-verify-target");
    }

    /// ⚠ `ThreadId(..)` is collapsed BEFORE the digits are, or its own digits would leave
    /// `ThreadId(N)` and two families that are one.
    #[test]
    fn a_thread_id_does_not_leave_its_digits_behind() {
        assert_eq!(
            stem_of("sprag-gate-7-clean-ThreadId(12)"),
            "sprag-gate-N-clean-T"
        );
        assert_eq!(
            stem_of("sprag-gate-7-clean-ThreadId(12)"),
            stem_of("sprag-gate-99999-clean-ThreadId(4)"),
            "two runs of the same case are one family, whatever their pid and thread",
        );
    }

    /// ⛔⛔⛔⛔⛔ **THE FINDING, AS A TEST**: the two numbers rank the families differently, and a
    /// gate reporting either one alone would say the other did not move.
    #[test]
    fn the_count_and_the_bytes_name_different_families() {
        let reading = read([
            ("sprag-check-target-1".to_string(), 4_000_000_000),
            ("sprag-check-target-2".to_string(), 2_000_000_000),
            ("sprag-gate-1-clean-ThreadId(1)".to_string(), 4096),
            ("sprag-gate-2-clean-ThreadId(2)".to_string(), 4096),
            ("sprag-gate-3-clean-ThreadId(3)".to_string(), 4096),
        ]);
        assert_eq!(reading.dirs, 5);
        assert_eq!(reading.bytes, 6_000_012_288);

        let by_count = &reading.series[0];
        assert_eq!(
            by_count.stem, "sprag-gate-N-clean-T",
            "{:?}",
            reading.series
        );
        assert_eq!(by_count.dirs, 3);

        let biggest = reading
            .series
            .iter()
            .max_by_key(|series| series.bytes)
            .expect("a reading with families has a largest one");
        assert_eq!(
            biggest.stem, "sprag-check-target-N",
            "the family holding the bytes is NOT the family holding the count — that is the whole \
             reason this reading carries two numbers: {:?}",
            reading.series,
        );
        assert_ne!(
            by_count.stem, biggest.stem,
            "⛔ ITEM 927: if these ever agree on this fixture the fixture has stopped modelling \
             the machine that produced the item",
        );
    }

    /// ⚠ A reading of nothing is CLEAN, and it is the caller's job to know it looked. See [`read`].
    #[test]
    fn a_root_with_nothing_in_it_reads_as_clean() {
        let reading = read([]);
        assert!(reading.is_clean());
        assert_eq!(reading.dirs, 0);
        assert_eq!(reading.bytes, 0);
        assert!(reading.series.is_empty());
    }

    /// ⛔ **THE MUTATION ITEM 927 ASKS FOR BY NAME**: *plant one directory and if the count does
    /// not rise, the gate is not there.* Driven on both numbers, because the item's finding is that
    /// one of them can move while the other does not.
    #[test]
    fn planting_one_directory_moves_the_count_it_belongs_to() {
        let before = read([("sprag-gate-1-clean-ThreadId(1)".to_string(), 4096)]);
        let after = read([
            ("sprag-gate-1-clean-ThreadId(1)".to_string(), 4096),
            ("sprag-gate-2-clean-ThreadId(2)".to_string(), 4096),
        ]);
        assert_eq!(
            before.dirs + 1,
            after.dirs,
            "the total count rose by the one planted"
        );
        assert_eq!(
            before.bytes + 4096,
            after.bytes,
            "and so did the bytes it holds"
        );
        assert_eq!(
            after.series[0].dirs,
            before.series[0].dirs + 1,
            "and the family it belongs to is the one that grew: {:?}",
            after.series,
        );

        // A directory of a family nobody has seen makes its own line rather than joining another.
        let novel = read([
            ("sprag-gate-1-clean-ThreadId(1)".to_string(), 4096),
            ("sprag-brand-new-9".to_string(), 1),
        ]);
        assert_eq!(novel.series.len(), 2, "{:?}", novel.series);
        assert!(
            novel.series.iter().any(|s| s.stem == "sprag-brand-new-N"),
            "a family that appears today is attributed today, under a name that says what it is: \
             {:?}",
            novel.series,
        );
    }
}
