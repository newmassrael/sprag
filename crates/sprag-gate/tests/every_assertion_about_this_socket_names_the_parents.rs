//! ⛔⛔⛔⛔⛔ **EVERY ASSERTION ABOUT WHAT IS ON THIS SOCKET NAMES WHO STARTED IT** — register item
//! 813, and the coverage its diagnosis is worth nothing without.
//!
//! # What item 813 is, in the number it arrived as
//!
//! `368c989`'s `headless (macos)` job failed one gate with `left: 2  right: 1` — two `sprag-term`
//! processes against a socket where the premise says one. **Two** was the whole of what that
//! failure could say, and it is true of at least three different worlds that want three different
//! repairs: a run that drifted onto the out-of-process driver, a `--daemon` whose fork intermediate
//! had not exited yet (register item 85's shape), and something else on that socket entirely.
//! Parentage separates them, the walk was already reading it, and every count in the suite was
//! throwing it away.
//!
//! The repair was a CENSUS: pid, parent, what that parentage makes it, and whether it was still
//! there when the sentence was written. The failing gate carries it now.
//!
//! # ⚠⚠⚠⚠⚠ Why a gate, and not the sentence the repair wrote about itself
//!
//! That round's commit also wrote, in the doc of the helper it fixed, *"Every `assert` on this in
//! this file says which processes it saw and who started them"*. **Measured one round later, that
//! was false**: twenty assertions in `crates/sprag-host/tests/cli.rs` read this socket and
//! **eleven printed a bare pid list** — every `driver_pids` site among them, including the four
//! that say `Found {:?}` about a premise nobody could then attribute. The sentence was written in
//! good faith about the sites its author had just edited, and nothing re-read it. This workspace's
//! rule 10: a reason written in prose is one nobody measures.
//!
//! ⚠⚠ AND THE HOLE THAT MATTERS IS THE NEXT SITE, not the eleven. Item 813's whole value is that
//! the NEXT occurrence — on a platform this suite cannot be run on locally, at a measured rate of
//! roughly one judged run in twenty — arrives carrying a fact instead of a number. A site added
//! without the census is the one that would spend that occurrence and leave nothing behind.

use sprag_gate::sources::rust_sources;

/// The ways this workspace asks the operating system WHAT IS ON A SOCKET.
///
/// ⚠ Spelled with the opening paren, because these are calls and not mentions: `daemon_pid`'s doc
/// names two of them in prose, and a gate that counted a doc's reference would be measuring how
/// often this file is explained rather than how often it is read.
const READERS: [&str; 4] = [
    "sprag_term_processes(",
    "sprag_term_census(",
    "sprag_term_pids(",
    "driver_pids(",
];

/// The two ways a site RENDERS what it read — pid, parent, role, and gone-or-still-there.
///
/// ⚠ Two and not one: a count taken right here can walk the table once (`term_census_here`), while
/// a count sampled as a MAXIMUM over a wait must keep the sample that produced it and be handed a
/// fresh walk beside it (`term_census_sentence`), so *it was there and it has gone* can be said
/// rather than guessed. Folding them into one spelling would force the second site to lie.
const RENDERERS: [&str; 2] = ["term_census_here", "term_census_sentence"];

/// How many assertions in this workspace read what is on a socket.
///
/// # ⚠⚠ AN EQUALITY, NOT A CEILING — register item 794's rule, load-bearing twice
///
/// A ceiling lets the population grow silently, which is how the coverage claim above went stale
/// while reading true. An equality also makes the arm below a POSITIVE CONTROL: rename a reader and
/// the needles match nothing, and *every site renders the census* is trivially true of no sites at
/// all — the vacuous green register item 799 measured.
///
/// **20, measured 2026-09-01** over `Source::assertions`, comment lines excluded.
const SITES_REGISTERED: usize = 20;

/// How many lines in this workspace CALL one of [`READERS`], definitions excluded.
///
/// # ⛔⛔⛔⛔⛔ This is the number that closes the hole the site rule cannot see
///
/// The site rule reads the text of an assertion. A caller can step outside it in one move —
/// `let after = sprag_term_pids(&sock);` and then `assert!(.., "{after:?}")` — and the assertion no
/// longer spells a reader, so it stops being a site and its bare pid list becomes invisible to the
/// gate. Nine such bindings already exist here for good reasons (a stable read across two
/// assertions, a pid to kill), so forbidding them would be a rule this suite cannot keep.
///
/// What can be held is that the POPULATION does not move unremarked. Refactoring a call out of an
/// assertion drops the site count; adding a new one raises this count; either way somebody reads
/// the message below before the number changes.
///
/// ⚠ Per LINE and not per call: two readers on one line are one place a person looks at.
///
/// **52, measured 2026-09-01, all of them in `crates/sprag-host/tests/cli.rs`.**
const CALLS_REGISTERED: usize = 52;

/// This gate's own source, which must SPELL the needles in order to hunt for them.
///
/// ⚠⚠ IT IS A POSITIVE CONTROL AND NOT AN EXEMPTION. Splitting a needle so it stops matching this
/// file is the trick that quietly stops matching anything; instead the walk is REQUIRED to find the
/// needles here, which is what proves they are the real spelling and that the walk reached this
/// tree at all (register item 809's skew, and 799's scan pointed at nothing).
const QUOTES_ITSELF: &str =
    "crates/sprag-gate/tests/every_assertion_about_this_socket_names_the_parents.rs";

/// Whether `line` is a definition of one of [`READERS`] rather than a use of it.
///
/// ⚠ The four helpers are free functions at the top level of a test file, so `fn ` and the name is
/// the whole rule. A method would need more, and the day one appears this returns the wrong answer
/// LOUDLY — through the equality above — rather than quietly excusing a site.
fn defines_a_reader(line: &str) -> bool {
    line.starts_with("fn ") && READERS.iter().any(|reader| line.contains(reader))
}

#[test]
fn every_assertion_that_reads_this_socket_renders_the_census_and_the_count_is_the_registered_one() {
    let mut sites = Vec::new();
    let mut silent = Vec::new();
    let mut found_itself = false;

    for source in rust_sources() {
        if source.file == QUOTES_ITSELF {
            found_itself = source
                .code
                .iter()
                .any(|(_, line)| READERS.iter().any(|reader| line.contains(reader)));
            continue;
        }
        for site in source.assertions() {
            if !READERS.iter().any(|reader| site.text.contains(reader)) {
                continue;
            }
            let at = format!("{}:{}", source.file, site.at);
            if !RENDERERS
                .iter()
                .any(|renderer| site.text.contains(renderer))
            {
                silent.push(at.clone());
            }
            sites.push(at);
        }
    }

    // ⚠⚠ THE POSITIVE CONTROL COMES FIRST. Without it, a renamed helper makes the walk find nothing
    // and every claim below holds of an empty set — green, and about no assertion at all.
    assert!(
        found_itself,
        "⚠⚠ THE SCAN IS BLIND: this gate spells every needle in its own source and the walk did \
         not find one there. Either a reader was renamed and the needles no longer match the tree, \
         or the walk is reading a tree that is not this one (register item 809) — and every \
         verdict below is worthless either way",
    );
    assert_eq!(
        sites.len(),
        SITES_REGISTERED,
        "⛔ ITEM 813: this workspace has {} assertions that read what is on a socket, and \
         {SITES_REGISTERED} are registered. GROWN: a new one was added — give it a census and \
         raise the number in the same commit. SHRUNK: sites went away, or a call was moved out of \
         its assertion into a binding, which is exactly the move that makes a bare pid list \
         invisible to this gate. Found: {sites:?}",
        sites.len(),
    );

    assert!(
        silent.is_empty(),
        "⛔⛔⛔ ITEM 813: these assert about the processes on a socket and, when they fail, print a \
         number or a bare list of pids. `368c989`'s macOS job printed `2` where `1` was owed and \
         nothing could be attributed from it: a driver the daemon spawned, a `--daemon` fork \
         intermediate still in the sample (register item 85) and a stranger on the socket all read \
         the same. Hand each of these a census — `term_census_here(&sock)` for a count taken here, \
         `term_census_sentence` for one sampled over a wait: {silent:?}",
    );
}

#[test]
fn no_reader_of_this_socket_is_called_anywhere_this_gate_has_not_counted() {
    let mut calls = Vec::new();
    for source in rust_sources() {
        if source.file == QUOTES_ITSELF {
            continue;
        }
        for (at, line) in &source.code {
            if READERS.iter().any(|reader| line.contains(reader)) && !defines_a_reader(line) {
                calls.push(format!("{}:{at}", source.file));
            }
        }
    }

    assert_eq!(
        calls.len(),
        CALLS_REGISTERED,
        "⛔⛔ ITEM 813: this workspace reads what is on a socket at {} places and \
         {CALLS_REGISTERED} are registered. This number exists because the rendering rule reads \
         the TEXT of an assertion, and one refactor steps outside it: bind the pids to a name \
         first and the assertion that prints them stops spelling a reader. So the population is \
         pinned. GROWN: a new read — if an assertion prints it, that assertion owes a census \
         (`term_census_here`); raise this number in the same commit. SHRUNK: reads went away, or a \
         helper was renamed and this gate has been measuring nothing since. Found: {calls:?}",
        calls.len(),
    );
}
