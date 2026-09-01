//! ⛔⛔⛔⛔⛔ **EVERY WAIT FOR A DAEMON SAYS WHY IT GAVE UP** — register item 812, and the coverage
//! register item 805 depends on.
//!
//! # What item 805 is waiting for, and why the instrument's reach is not prose
//!
//! Item 805 is a gate that failed **once**, on macOS, on a platform this suite cannot be run on
//! locally. The repair available was not a fix — the cause is unmeasured — but an INSTRUMENT:
//! `why_not_serving` was planted at every site that waits for a daemon, so that the next
//! occurrence arrives carrying a fact instead of a sentence. That item now waits for an event with
//! a measured rate of roughly one judged run in twenty; the whole value of the wait is that the
//! instrument is at **every** site when it comes.
//!
//! ⚠⚠ AND ITS REACH WAS WRITTEN DOWN RATHER THAN CHECKED. `why_not_serving`'s own doc said
//! *"Thirty-five gates in this file wait ten seconds"*. Measured 2026-09-01, one round later:
//! **37** sites, and their budgets are **not** all ten seconds — 10 s at forty call sites, 20 s at
//! five, 15 s at one and 5 s at two. The sentence was true when written and nothing re-read it,
//! which is this repository's rule 10 in its usual clothes.
//!
//! ⚠⚠⚠ THE HOLE THAT MATTERS IS THE NEXT SITE, not the count being stale. Nothing stopped a new
//! wait being added without the diagnosis, and such a site is exactly the one that would fail on
//! the platform nobody can reach and say nothing — the state item 805 spent a round removing.
//! *What is not classified is RED, not a pass.*

use sprag_gate::sources::rust_sources;

/// The sentence a gate prints when a daemon it spawned never answered.
///
/// ⚠ The needle is the MESSAGE rather than the helper, because the defect is a site that has the
/// message and lacks the helper. Hunting the helper would find only the sites that are already
/// right.
const GAVE_UP: &str = "never started serving";

/// The diagnosis every such site must carry.
const DIAGNOSIS: &str = "why_not_serving";

/// How many such waits this workspace has.
///
/// # ⚠⚠ AN EQUALITY, NOT A CEILING — register item 794's rule, and it is load-bearing twice here
///
/// A ceiling lets the population grow silently, which is the shape that let the count go stale in
/// the first place. An equality also makes the arm below a POSITIVE CONTROL: a reworded message
/// would make the needle match nothing, and "every site carries the diagnosis" is trivially true of
/// no sites at all — the vacuous green item 799 measured. The number is what refuses that.
///
/// **41, measured 2026-09-01** (`grep -c` over the tree, comment lines excluded — the helper's own
/// doc quotes the message and is not a site).
///
/// ⚠ GREW BY TWO IN EACH OF TWO ROUNDS, and by the same shape both times: register items 774 and
/// 815 each added a gate that reboots a daemon, so each waits once for the first daemon and again
/// for its replacement. That is the ratchet doing exactly what it was built for — the number moved
/// in the commit that moved the population, twice, and neither time by anybody remembering to.
const WAITS_REGISTERED: usize = 41;

/// This gate's own source, which must SPELL the needle in order to hunt for it.
///
/// ⚠⚠ IT IS A POSITIVE CONTROL AND NOT AN EXEMPTION. Splitting the string so it stops matching
/// itself is the trick that quietly stops matching anything; instead the walk is REQUIRED to find
/// it here, which is what proves the needle is the real spelling and the walk reached this tree at
/// all. Register item 799 measured the alternative: a scan pointed at nothing reads exactly like a
/// clean one.
const QUOTES_ITSELF: &str =
    "crates/sprag-gate/tests/every_wait_for_a_daemon_says_why_it_gave_up.rs";

/// How far below a site's message the diagnosis may appear.
///
/// ⚠ Three lines because `assert!` here spans `predicate`, `format string`, `argument` — and a
/// window wider than the statement would let one site's diagnosis vouch for its neighbour, which
/// is the accounting error this gate exists to prevent.
const WITHIN: usize = 3;

#[test]
fn every_wait_for_a_daemon_carries_the_diagnosis_and_the_count_is_the_registered_one() {
    let mut sites = Vec::new();
    let mut silent = Vec::new();
    let mut found_itself = false;

    for source in rust_sources() {
        for (at, line) in &source.code {
            if !line.contains(GAVE_UP) {
                continue;
            }
            if source.file == QUOTES_ITSELF {
                found_itself = true;
                continue;
            }
            sites.push(format!("{}:{at}", source.file));
            let carried = source
                .code
                .iter()
                .filter(|(other, _)| *other >= *at && *other <= at + WITHIN)
                .any(|(_, text)| text.contains(DIAGNOSIS));
            if !carried {
                silent.push(format!("{}:{at}", source.file));
            }
        }
    }

    // ⚠⚠ THE POSITIVE CONTROL COMES FIRST. Without it, a message somebody reworded makes the walk
    // find nothing and the claim below hold of an empty set — green, and about no gate at all.
    assert!(
        found_itself,
        "⚠⚠ THE SCAN IS BLIND: this gate spells `{GAVE_UP}` in its own source and the walk did not \
         find it there. Either the needle stopped being the real spelling, or the walk is reading a \
         tree that is not this one (register item 809) — and every verdict below is worthless \
         either way",
    );
    assert_eq!(
        sites.len(),
        WAITS_REGISTERED,
        "⛔ ITEM 812: this workspace has {} waits that say `{GAVE_UP}`, and {WAITS_REGISTERED} are \
         registered. GROWN: a new wait was added — give it `{DIAGNOSIS}` and raise the number in \
         the same commit. SHRUNK: sites went away or the message was reworded; if it was reworded, \
         this gate has been measuring nothing since and the needle has to move with it. Found: \
         {sites:?}",
        sites.len(),
    );

    assert!(
        silent.is_empty(),
        "⛔⛔⛔ ITEM 812: these wait for a daemon and, when it never comes, say only that it never \
         came. Register item 805 is waiting on ONE observation from a platform nobody can run this \
         suite on — measured at roughly one judged run in twenty — and a site without \
         `{DIAGNOSIS}` is the one that would spend that observation and leave nothing behind. Add \
         the diagnosis to: {silent:?}",
    );
}
