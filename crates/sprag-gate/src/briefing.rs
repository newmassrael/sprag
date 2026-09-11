//! Whether the BRIEF's two sides name the same words — register item 1023.
//!
//! # The face item 1022 could not reach
//!
//! Item 1022 built a gate over the words the HOST publishes on its wire, and its population was
//! *a `pub` constant of the host crate that shipping code writes as a JSON member name*. That
//! predicate cannot be pointed at the loop's driver, and the register measured why on the day 1022
//! was paid: `outer.rs` spells the run's datamodel words as PRIVATE constants
//! (`UNANSWERED_RULE`, `UNREADABLE_RULE`, `UNWELL_RULE`) and **nothing in Rust reads them** — the
//! reader is `ai_loop.scxml`, which assigns each one out of `_event.data`. Widening 1022's gate
//! would have called all three dead publication, and passing them with an exemption list would have
//! made the exemption the gate.
//!
//! # ⚠⚠⚠⚠⚠ So the population is the EDGE, and neither side gets to be the authority
//!
//! The brief is the one payload this driver hands the document WHOLE at a single site — every other
//! event's data is built behind a type's own `wire()` and reaches the raise through a `&str`, where
//! a text scan would be guessing. So the subject is that payload's top-level keys against the
//! `_event.data` reads of the document's `brief` transition, and it is asked BOTH WAYS:
//!
//! * a key the driver writes and the document never reads is item 1022's defect on this edge — a
//!   value computed, published and dropped;
//! * a key the document reads and the driver never writes is worse, and the payload's own comment
//!   says so: *"a missing key is a Lua nil rather than an echoed empty string"*, so the document
//!   assigns nil over a decision its author wrote.
//!
//! ⚠⚠ **THE TWO DIRECTIONS GUARD EACH OTHER'S SIGHT.** A scan that stopped finding the payload
//! leaves every read word unwritten; a scan that stopped finding the transition leaves every
//! written word unread. Either blindness is LOUD, which is the property item 470 found a
//! single-direction ratchet cannot have — and the pinned vocabulary beside them is what catches the
//! one case both would miss, which is both going blind at once.
//!
//! # ⚠⚠⚠ Spelling is not the subject, so the words are RESOLVED
//!
//! Measured 2026-09-10 at `89f1b5dc`: the payload carries **28** top-level keys, **8** spelled as
//! string literals and **20** through constants, six of those associated (`Turn::WIRE_KEY`,
//! `ScreenRules::WIRE_KEY`, …). A gate that only understood constants would have walked past a
//! third of the edge — and the literals are the half MORE likely to drift, not less. So
//! [`crate::briefing::constants`] resolves a spelling to the word it stands for, and ⛔ a spelling
//! this cannot resolve is a RED rather than a skip: an unclassified key is not a passing one.
//!
//! ⚠ The link above is fully qualified, and every neighbouring module's is too, because a module's
//! `//!` doc resolves at the CRATE ROOT here — the `pub mod` line in `lib.rs` carries an outer doc
//! of its own, so the merged fragments take that scope. A bare `[`constants`]` compiles fine and
//! fails the rustdoc gate; this is the second module in two rounds to learn it that way.

use std::collections::{BTreeMap, BTreeSet};

use crate::sources::{Source, outside_strings};

/// The file that builds the brief this document is handed.
pub const DRIVER: &str = "crates/sprag-plugin/src/outer.rs";

/// Where the driver's own datamodel vocabulary is declared — a PREFIX over the whole crate.
///
/// ⚠ A prefix rather than the five files that hold it today, because a list with no glob decides
/// alone (item 470); the gate PINS which files actually contributed, so a sixth is a person's
/// decision rather than a fact a union absorbs.
pub const VOCABULARY: &str = "crates/sprag-plugin/src/";

/// The line that opens the brief payload, squeezed of whitespace.
///
/// ⚠ A NEEDLE, and the honest statement about it is that it can go blind. What makes that
/// survivable is the second direction: a payload this cannot find publishes nothing, and then every
/// word the document reads is one nobody wrote — which is the loudest answer this gate has.
pub const OPENS: &str = "letpayload=serde_json::json!({";

/// The document transition that lands the brief in the datamodel.
pub const EDGE: &str = "event=\"brief\"";

/// One top-level key of the brief payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key {
    /// One-indexed line in [`DRIVER`].
    pub line: usize,
    /// How the source spells it — a quoted literal, or a constant's path.
    pub spelling: String,
    /// The word it stands for, or [`None`] when nothing in [`VOCABULARY`] declares that spelling.
    pub word: Option<String>,
}

/// Every string constant [`VOCABULARY`] declares, under its bare name and, for an associated one,
/// under `Type::NAME` as well.
///
/// # ⚠⚠⚠⚠ A bare name that means two things resolves to NEITHER
///
/// `WIRE_KEY` is declared six times in this crate with six different values — `may_answer`,
/// `screen_rules`, `turn_within_ms` and three more. Resolving the bare name to whichever
/// declaration the walk saw last would put a plausible wrong word into the gate's answer, and a
/// gate that is confidently wrong is worse than one that says it cannot tell. So an ambiguous bare
/// name is DROPPED, and the qualified `Type::WIRE_KEY` — which is how the payload actually spells
/// every one of them — is what resolves.
#[must_use]
pub fn constants(sources: &[Source]) -> BTreeMap<String, String> {
    let mut found: BTreeMap<String, String> = BTreeMap::new();
    let mut ambiguous: BTreeSet<String> = BTreeSet::new();
    for source in sources {
        if !source.file.starts_with(VOCABULARY) || source.file.contains("/tests/") {
            continue;
        }
        let mut within: Option<(String, i32)> = None;
        for (_, text) in &source.product {
            if let Some(rest) = text.strip_prefix("impl")
                && let Some(name) = implemented(rest)
            {
                within = Some((name, 0));
            }
            if let Some((name, depth)) = within.take() {
                let depth = depth + braces(text);
                if depth > 0 {
                    within = Some((name.clone(), depth));
                }
                if let Some((constant, value)) = declaration(text) {
                    found.insert(format!("{name}::{constant}"), value);
                }
            }
            if let Some((constant, value)) = declaration(text) {
                match found.get(&constant) {
                    Some(already) if *already != value => {
                        ambiguous.insert(constant);
                    }
                    Some(_) => {}
                    None => {
                        found.insert(constant, value);
                    }
                }
            }
        }
    }
    for name in ambiguous {
        found.remove(&name);
    }
    found
}

/// Every top-level key of the brief payload, in document order.
///
/// Empty when the payload cannot be found, which is a state the gate must announce rather than read
/// as clean — see [`OPENS`].
#[must_use]
pub fn published(sources: &[Source]) -> Vec<Key> {
    let words = constants(sources);
    let Some(driver) = sources.iter().find(|source| source.file == DRIVER) else {
        return Vec::new();
    };

    let mut keys = Vec::new();
    let mut depth = 0i32;
    let mut open = false;
    for (line, text) in &driver.product {
        if !open {
            if squeezed(text) == OPENS {
                open = true;
                depth = braces(text);
            }
            continue;
        }
        // ⚠ THE KEY IS READ OFF THE RAW LINE AND THE DEPTH OFF THE BLANKED ONE. A literal key IS a
        // string, so a scan that blanked strings first would see `:` and no name at all.
        if depth == 2
            && let Some(spelling) = member(text)
        {
            let word = resolve(&words, &spelling);
            keys.push(Key {
                line: *line,
                spelling,
                word,
            });
        }
        depth += braces(text);
        if depth <= 0 {
            break;
        }
    }
    keys
}

/// Every word the document's `brief` transition reads out of `_event.data`, with the line it is
/// read on.
///
/// ⚠ Comments are taken away first, through [`crate::loop_shape::uncommented_lines`] rather than a
/// second copy of what a comment is. Measured while this gate was being built: the transition's
/// prose mentions `_event.data.silence`, and a scan that read its own document's commentary
/// reported a key nobody writes — a defect that was not there.
///
/// ⛔⛔ **AND THE LINE IS THE DOCUMENT'S, WHICH THE FIRST DRAFT GOT WRONG.** It counted lines in the
/// comment-STRIPPED text, so the message named `ai_loop.scxml:508` for an `<assign>` on line 3601 —
/// a number a person would have opened, read something unrelated, and disbelieved the gate over.
#[must_use]
pub fn read_on_the_edge(scxml: &str) -> BTreeMap<String, usize> {
    let mut found = BTreeMap::new();
    let mut open = false;
    for (at, line) in crate::loop_shape::uncommented_lines(scxml) {
        if !open {
            open = line.contains(EDGE);
            if !open {
                continue;
            }
        } else if line.trim_start().starts_with("</transition>") {
            break;
        }
        let mut rest = line.as_str();
        while let Some(reads) = rest.find("_event.data.") {
            rest = &rest[reads + "_event.data.".len()..];
            let name: String = rest
                .chars()
                .take_while(|letter| {
                    letter.is_ascii_lowercase() || letter.is_ascii_digit() || *letter == '_'
                })
                .collect();
            if !name.is_empty() {
                found.entry(name).or_insert(at);
            }
        }
    }
    found
}

/// The word a spelling stands for: the literal itself, or the constant it names.
///
/// # ⚠⚠⚠⚠⚠ Why the lookup walks the path DOWN instead of matching it whole
///
/// The payload spells the same kind of constant four different ways —
/// `crate::consent::Consents::WIRE_KEY`, `Turn::WIRE_KEY`, `HOLD_WITHIN_KEY` — and which one a
/// call site uses is a fact about that file's `use` block, not about the wire. The first draft of
/// this looked the whole spelling up and **the gate went red on four real keys** because it could
/// not see past the module path in front of them. So the longest suffix wins: `Consents::WIRE_KEY`
/// answers the qualified spelling, and the ambiguous bare `WIRE_KEY` — which [`constants`] refuses
/// to hold — is never reached.
fn resolve(words: &BTreeMap<String, String>, spelling: &str) -> Option<String> {
    if let Some(word) = spelling
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    {
        return Some(word.to_owned());
    }
    // ⛔⛔⛔ A QUALIFIED SPELLING NEVER FALLS THROUGH TO THE BARE NAME. `Handback::WIRE_KEY`
    // answered by whatever the crate's last plain `WIRE_KEY` happened to be is the confidently
    // wrong answer this module refuses to give: the qualification is the caller SAYING which type's
    // key it means, and dropping it would resolve six different words to one.
    let segments: Vec<&str> = spelling.split("::").collect();
    let shortest = if segments.len() > 1 { 2 } else { 1 };
    (0..=segments.len() - shortest)
        .find_map(|from| words.get(&segments[from..].join("::")).cloned())
}

/// The type an `impl` line implements FOR, given everything after the `impl` keyword.
///
/// `impl Consents` and `impl Display for Consents` both answer `Consents`, which is the type whose
/// associated constants the block declares.
fn implemented(rest: &str) -> Option<String> {
    let rest = rest.trim_start_matches(|at: char| at == '<' || at == '>' || at.is_whitespace());
    let subject = rest.rsplit(" for ").next().unwrap_or(rest);
    let name: String = subject
        .chars()
        .take_while(|at| at.is_ascii_alphanumeric() || *at == '_')
        .collect();
    (!name.is_empty() && name.starts_with(|first: char| first.is_ascii_uppercase())).then_some(name)
}

/// `(name, value)` when this line declares a string constant with a literal value.
fn declaration(text: &str) -> Option<(String, String)> {
    let rest = text
        .strip_prefix("pub const ")
        .or_else(|| text.strip_prefix("const "))
        .or_else(|| {
            text.split_once("const ")
                .filter(|(head, _)| head.starts_with("pub("))
                .map(|(_, tail)| tail)
        })?;
    let (name, rest) = rest.split_once(':')?;
    let name = name.trim();
    if name.is_empty()
        || !name
            .chars()
            .all(|at| at.is_ascii_uppercase() || at.is_ascii_digit() || at == '_')
    {
        return None;
    }
    let (kind, rest) = rest.split_once('=')?;
    if !matches!(kind.trim(), "&str" | "&'static str") {
        return None;
    }
    let rest = rest.trim().strip_prefix('"')?;
    let (value, _) = rest.split_once('"')?;
    Some((name.to_owned(), value.to_owned()))
}

/// The member name this line opens with, quoted literal or constant path, when it is one.
fn member(text: &str) -> Option<String> {
    if let Some(rest) = text.strip_prefix('"') {
        let (name, after) = rest.split_once('"')?;
        return after
            .trim_start()
            .starts_with(':')
            .then(|| format!("\"{name}\""));
    }
    let mut spelling = String::new();
    let mut chars = text.chars().peekable();
    while let Some(at) = chars.peek().copied() {
        if at.is_ascii_alphanumeric() || at == '_' {
            spelling.push(at);
            chars.next();
        } else if at == ':' {
            chars.next();
            if chars.peek() == Some(&':') {
                chars.next();
                spelling.push_str("::");
            } else {
                // A single colon ends the name, which is what makes this a member and not a call.
                let last = spelling.rsplit("::").next().unwrap_or_default();
                return is_constant(last).then_some(spelling);
            }
        } else {
            return None;
        }
    }
    None
}

/// Whether `word` is spelled the way this workspace spells a wire constant.
fn is_constant(word: &str) -> bool {
    word.len() >= 3
        && word.starts_with(|first: char| first.is_ascii_uppercase())
        && word
            .chars()
            .all(|at| at.is_ascii_uppercase() || at.is_ascii_digit() || at == '_')
}

/// How far this line opens or closes, with string literals taken away first.
fn braces(text: &str) -> i32 {
    outside_strings(text)
        .chars()
        .map(|at| match at {
            '{' | '(' | '[' => 1,
            '}' | ')' | ']' => -1,
            _ => 0,
        })
        .sum()
}

/// `text` with every space gone — [`Source::squeezed`]'s reason on one line.
fn squeezed(text: &str) -> String {
    text.chars().filter(|at| !at.is_whitespace()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(file: &str, lines: &[&str]) -> Source {
        let product: Vec<(usize, String)> = lines
            .iter()
            .enumerate()
            .map(|(index, text)| (index + 1, (*text).trim().to_owned()))
            .filter(|(_, text)| !text.starts_with("//"))
            .collect();
        Source {
            file: file.to_owned(),
            code: product.clone(),
            product,
            // ⚠ No attribute in this fixture's cases; empty says so — register item 1044.
            attributes: Vec::new(),
        }
    }

    /// ⚠⚠⚠⚠⚠ THE SCAN'S OWN CONTROL, over a payload this test owns.
    ///
    /// The gate beside it asserts that two sets agree, and *they agree* is equally true of a scan
    /// that read them and of one that read nothing — item 453's blind ratchet in the shape a set
    /// comparison invites. So the three spellings the real payload uses are exercised here: a
    /// quoted literal, a bare constant and an associated one, against a nested object whose keys
    /// must NOT be taken for the edge's.
    #[test]
    fn the_scan_reads_a_literal_a_constant_and_an_associated_one_and_no_nested_key() {
        let driver = source(
            DRIVER,
            &[
                "const WORKING_RULES: &str = \"working_rules\";",
                "let payload = serde_json::json!({",
                "\"north_star\": brief.north_star,",
                "WORKING_RULES: rules.clone(),",
                "Turn::WIRE_KEY: within,",
                "Consents::WIRE_KEY: clauses.iter().map(|held| {",
                "serde_json::json!({",
                "Consent::ASKED_KEY: held.asked(),",
                "})",
                "}).collect::<Vec<_>>(),",
                "});",
                "self.machine.raise_external(AiLoopEvent::Brief, &payload.to_string(), \"\");",
            ],
        );
        let vocabulary = source(
            "crates/sprag-plugin/src/completion.rs",
            &[
                "impl Turn {",
                "pub const WIRE_KEY: &'static str = \"turn_within_ms\";",
                "}",
                "impl Consents {",
                "pub const WIRE_KEY: &'static str = \"may_answer\";",
                "}",
                "impl Consent {",
                "pub const ASKED_KEY: &'static str = \"asked\";",
                "}",
            ],
        );
        let found: Vec<(String, Option<String>)> = published(&[driver, vocabulary])
            .into_iter()
            .map(|key| (key.spelling, key.word))
            .collect();

        assert_eq!(
            found,
            vec![
                ("\"north_star\"".to_owned(), Some("north_star".to_owned())),
                ("WORKING_RULES".to_owned(), Some("working_rules".to_owned())),
                (
                    "Turn::WIRE_KEY".to_owned(),
                    Some("turn_within_ms".to_owned())
                ),
                (
                    "Consents::WIRE_KEY".to_owned(),
                    Some("may_answer".to_owned())
                ),
            ],
            "the three spellings are this edge's keys and `Consent::ASKED_KEY` is a key of the \
             object one of them CARRIES — a scan that took it would be gating a nested shape \
             against a datamodel that has never heard of it, and one that lost any of the other \
             three would report a word the document reads as one nobody writes",
        );
    }

    /// ⛔⛔⛔ **A MODULE PATH IN FRONT OF A CONSTANT IS NOT A DIFFERENT CONSTANT** — the bug this
    /// gate's own first run found, on four real keys of the real payload.
    #[test]
    fn a_key_spelled_through_its_module_path_resolves_to_the_same_word() {
        let vocabulary = source(
            "crates/sprag-plugin/src/consent.rs",
            &[
                "impl Consents {",
                "pub const WIRE_KEY: &'static str = \"may_answer\";",
                "}",
            ],
        );
        let words = constants(std::slice::from_ref(&vocabulary));

        for spelling in [
            "crate::consent::Consents::WIRE_KEY",
            "consent::Consents::WIRE_KEY",
            "Consents::WIRE_KEY",
        ] {
            assert_eq!(
                resolve(&words, spelling).as_deref(),
                Some("may_answer"),
                "`{spelling}` is one file's `use` block talking, not a second word — and reading \
                 it as unresolvable makes this gate report a live key as one nobody can check",
            );
        }
        assert_eq!(
            resolve(&words, "Handback::WIRE_KEY").as_deref(),
            None,
            "but a DIFFERENT type's key must not be answered by this one just because the last \
             segment matches",
        );
    }

    /// ⚠⚠⚠⚠ A bare name that means two things must resolve to NEITHER.
    #[test]
    fn an_ambiguous_bare_constant_is_dropped_rather_than_guessed() {
        let vocabulary = source(
            "crates/sprag-plugin/src/readiness.rs",
            &[
                "impl Attended {",
                "pub const WIRE_KEY: &'static str = \"await_person_ms\";",
                "}",
                "impl Handback {",
                "pub const WIRE_KEY: &'static str = \"handback_still_ms\";",
                "}",
            ],
        );
        let words = constants(std::slice::from_ref(&vocabulary));

        assert_eq!(words.get("WIRE_KEY"), None, "{words:?}");
        assert_eq!(
            words.get("Attended::WIRE_KEY").map(String::as_str),
            Some("await_person_ms"),
        );
        assert_eq!(
            words.get("Handback::WIRE_KEY").map(String::as_str),
            Some("handback_still_ms"),
        );
    }

    /// ⚠⚠⚠ The document's own commentary is not a read — measured while this gate was written.
    #[test]
    fn a_word_the_transitions_prose_names_is_not_a_word_it_reads() {
        let scxml = "<transition event=\"brief\" target=\"working\">\n\
             <!-- Which of those a run is in is `_event.data.silence`. -->\n\
             <assign location=\"north_star\" expr=\"_event.data.north_star\"/>\n\
             </transition>\n";
        let read = read_on_the_edge(scxml);

        assert_eq!(
            read.keys().cloned().collect::<Vec<String>>(),
            vec!["north_star".to_owned()],
            "the prose mention must not become a key nobody writes, and the assignment must: \
             {read:?}",
        );
        // ⛔⛔⛔⛔⛔ AND THE LINE IS THE ONE A PERSON CAN OPEN — the defect this gate shipped in its
        // own first draft. Stripping the comment away renumbers everything after it, so the
        // message named a line 3093 rows off the assignment it was about. One comment line stands
        // above the assignment here precisely so a scan that removed it would answer 2, not 3.
        assert_eq!(
            read.get("north_star"),
            Some(&3),
            "the assignment is the document's third line, and a number measured in some other \
             text is a number nobody can check: {read:?}",
        );
    }

    /// ⚠⚠ And a read on ANOTHER edge is not this edge's.
    #[test]
    fn only_the_brief_transition_is_this_edges_reader() {
        let scxml = "<transition event=\"brief\">\n\
             <assign location=\"north_star\" expr=\"_event.data.north_star\"/>\n\
             </transition>\n\
             <transition event=\"judge\" cond=\"_event.data.stop_short\">\n\
             <assign location=\"stop_reason\" expr=\"_event.data.stop_short\"/>\n\
             </transition>\n";
        let read = read_on_the_edge(scxml);

        assert!(
            !read.contains_key("stop_short"),
            "`judge` carries a fact about a turn, not a decision this run holds — counting it \
             would make every other event's payload a word this edge owes: {read:?}",
        );
    }
}
