//! ⛔⛔⛔⛔⛔ **A TEST THIS WORKSPACE SWITCHES OFF IS COUNTED, AND IT SAYS WHY** — register item 1044.
//!
//! # ⚠⚠⚠ Switching a gate off is the quietest edit in this repository
//!
//! Everything else that weakens a gate leaves a mark somebody reads. A deleted assertion shows in a
//! diff as a deletion; a widened population shows as a changed predicate; a lowered floor is a
//! number that moved. `#[ignore]` is one word on a line of its own, and the suite that used to fail
//! goes on reporting `ok` — the count moves from `0 ignored` to `1 ignored` in a line nobody diffs.
//!
//! **Measured 2026-09-11, and the silence was total:**
//!
//! | | reading |
//! |---|---|
//! | attributes in `crates/` | **33**, across **7** files |
//! | of those, in `sprag-host/src/live_agent.rs` | **27** |
//! | clauses under `.githooks/` that count them | **0** — every hit for the word is `.gitignore` or `target/` |
//! | that carry no reason at all | **0** |
//!
//! `test result:` prints `N ignored` on every run and nothing reads that number back, so a 34th
//! arrives looking exactly like the 33 that belong.
//!
//! # ⛔ Rule 5 first: this count has no path to zero, so the gate is a RATCHET
//!
//! Twenty-seven of the thirty-three drive a live `claude` CLI — credentials, network, minutes
//! apiece — and they are RIGHT to be off in CI. A gate demanding zero would be red forever and
//! therefore read by nobody, which is the shape item 1044's own text warned about before this file
//! existed. So the population admits roles that can never leave it, the target is "did not grow",
//! and a legitimate new one is admitted by raising the floor IN THE SAME EDIT — deliberately,
//! where a reviewer sees it.
//!
//! # ⛔⛔⛔⛔⛔ AND NO EXEMPTION LIST — rule 6
//!
//! The obvious shortcut is `count everything except live_agent.rs`, which would leave 6 to watch
//! instead of 33. That is exactly the escape hatch rule 6 forbids: the file with twenty-seven
//! switches in it becomes the one place a twenty-eighth is invisible. The count is the whole
//! population, and being legitimate is expressed as *the number did not move*, never as membership
//! of a list.
//!
//! # ⚠⚠ Why [`Source::attributes`] and not a grep — and not `Source::code` either
//!
//! An unanchored `grep` for the needle answered **41** before this round and answers **45** after
//! it — the four it gained are the prose this round wrote, two in this header and two in
//! [`Source::attributes`]'s. The anchored count did not move: **33** both times. A gate on the
//! loose figure would have gone red on the commit that explains it, which is not a stricter gate,
//! it is a gate that punishes writing the reason down.
//!
//! ⛔⛔⛔⛔⛔ **AND THE FIRST DRAFT OF THIS FILE READ `Source::code`, WHICH REPORTED 0.** That field
//! drops every line starting with `#`, so an attribute is exactly what it has already thrown away
//! — the gate compiled, ran, and measured nothing. [`rust_sources`] carries a warning about this
//! in its own body, left by the round whose first draft looked for `#[cfg(test)]` the same way; it
//! was read only after the same hole had been fallen into a second time. Hence
//! [`Source::attributes`], which is the complement of that filter and lives beside it, so the two
//! cannot drift into disagreeing about what a comment is (register item 213).
//!
//! [`outside_strings`] takes the string literals off what is left, and the needle is spelled in two
//! pieces below, because a literal here would be an attribute-looking line this gate counts itself.
//!
//! ⚠ [`Source::product`] is deliberately NOT used. It drops `#[cfg(test)]` items, and every
//! `#[ignore]` in the workspace is by definition test code, so that field would report zero
//! forever. It would also get `live_agent.rs` wrong in the other direction: that module is declared
//! `#[cfg(test)] mod live_agent;` from `lib.rs`, so nothing in the file itself marks it as harness
//! — a shape `no_product_code_takes_a_scratch_root_unchecked` records measuring the hard way.

use sprag_gate::sources::{Source, outside_strings, rust_sources};

/// How many tests this workspace switches off, measured 2026-09-11.
///
/// ⚠⚠ **A FLOOR ABOVE THE COUNT IS THAT MANY SWITCHES ADMITTED IN SILENCE**, which is what register
/// item 926 had to build into `north-star` and what `HARNESS_SITES_REGISTERED` states for the same
/// reason. So this is an equality and not a ceiling: a round that removes one lowers this number in
/// the same edit, and the refusal below names the figure to write.
///
/// The 33: twenty-seven in `sprag-host/src/live_agent.rs`, and one apiece in
/// `sprag-terminal/src/pty.rs`, `sprag-plugin/src/dialogue.rs`, `sprag-plugin/src/agent.rs`,
/// `sprag-host/src/hooks.rs`, `sprag-host/tests/wire_client.rs` and
/// `sprag-tui/tests/pty_round_trip.rs`.
const TESTS_SWITCHED_OFF: usize = 33;

/// Every site where this workspace switches a test off, as `(file, one-indexed line, whole line)`.
///
/// ⚠ The needle is built rather than written, so this file does not match itself. The pieces join
/// to the attribute's opening; a reader looking for it in this source will find it only here.
fn switched_off(sources: &[Source]) -> Vec<(String, usize, String)> {
    let opener = format!("#{}", "[ignore");
    let mut sites = Vec::new();
    for source in sources {
        for (line, text) in &source.attributes {
            let code = outside_strings(text);
            if code.trim_start().starts_with(opener.as_str()) {
                sites.push((source.file.clone(), *line, code.trim().to_owned()));
            }
        }
    }
    sites
}

/// ⛔⛔⛔ **A SWITCH WITH NO REASON ON IT IS THE ONE NOBODY CAN EVER ASSESS** — register item 1044,
/// and rule 6 applied to the attribute rather than to the count.
///
/// The count above cannot tell a legitimate switch from a silent one; only the text can. All 33
/// measured today carry `= "..."` saying what they need — credentials, a live CLI, a machine to
/// themselves — which is what makes them auditable at all. A bare one says nothing, and is
/// indistinguishable from a gate somebody turned off to get a commit through.
///
/// ⚠ This is the half of the item with a real path to zero (rule 5): it is AT zero, and the claim
/// is that it stays there. The ratchet below is the half that cannot be.
#[test]
fn every_test_this_workspace_switches_off_says_why() {
    let sources = rust_sources();
    let bare = format!("#{}{}", "[ignore", "]");
    let sites = switched_off(&sources);
    let mute: Vec<&(String, usize, String)> =
        sites.iter().filter(|(_, _, text)| *text == bare).collect();
    assert!(
        mute.is_empty(),
        "⛔ ITEM 1044: {} test(s) are switched off with no reason written on them:\n{}\n\
         Every one of the 33 measured on 2026-09-11 says what it needs — a live CLI, credentials, \
         minutes. A bare one cannot be told apart from a gate somebody turned off to get a commit \
         through, which is the whole subject of this item. Write the reason into the attribute.",
        mute.len(),
        mute.iter()
            .map(|(file, line, _)| format!("  {file}:{line}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
}

/// ⛔⛔⛔⛔⛔ **AND THE COUNT ITSELF DOES NOT DRIFT** — register item 1044.
///
/// A reason can be written on a switch that should not exist. This arm asks the other question: is
/// the number the same one somebody looked at? It is an EQUALITY in both directions, and the two
/// refusals say different things because the repairs are different — one is "argue for it here",
/// the other is "write the smaller number down".
#[test]
fn the_tests_this_workspace_switches_off_are_counted() {
    let sources = rust_sources();
    let sites = switched_off(&sources);
    let found = sites.len();
    let where_they_are = |sites: &[(String, usize, String)]| {
        let mut per: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
        for (file, _, _) in sites {
            *per.entry(file.as_str()).or_default() += 1;
        }
        per.iter()
            .map(|(file, count)| format!("  {count:>3}  {file}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert!(
        found <= TESTS_SWITCHED_OFF,
        "⛔ ITEM 1044: this workspace now switches off {found} test(s), against the \
         {TESTS_SWITCHED_OFF} that were measured and argued for. Switching a gate off is the \
         quietest edit here — the suite still reports `ok` and only `N ignored` moves — so a new \
         one is admitted by raising this constant in the same edit, with the reason in its doc, \
         never by the number drifting.\n{}",
        where_they_are(&sites),
    );
    // ⚠⚠ AND THE OTHER DIRECTION, which is not symmetry for its own sake: a floor left above the
    // count is exactly that many new switches this gate would accept without a word — item 926's
    // rule, and the reason `HARNESS_SITES_REGISTERED` carries three dated paragraphs of lowering.
    assert!(
        found >= TESTS_SWITCHED_OFF,
        "⚠⚠⚠ ITEM 1044: only {found} test(s) are switched off and this gate still admits \
         {TESTS_SWITCHED_OFF}. The slack is {} switch(es) that could be added in silence. Write \
         `TESTS_SWITCHED_OFF = {found}` and say in its doc what was re-enabled.\n{}",
        TESTS_SWITCHED_OFF - found,
        where_they_are(&sites),
    );
}
