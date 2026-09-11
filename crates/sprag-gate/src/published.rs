//! Whether every wire word the host PUBLISHES has something that READS it — register item 1022.
//!
//! # The defect, and the two faces this repository has already paid for
//!
//! `sprag_host` mints the JSON member names it answers with as constants, and a slot builds an
//! object out of them. Nothing counted the far end. Item 1018 found `PANE_DRIVEN_KEY` published
//! from a field that was a constant `false` — a key that could not become true. Item 1021 found
//! `PANE_BORNE_BY_KEY` published correctly and read by NOBODY: the host had run a join over the run
//! registry on every pane listing since 2026-08-19 — a lock taken, a snapshot walked — and put the
//! answer somewhere no reader existed. **Both were found by a person counting by hand**, three
//! weeks and one day late, which is item 1022: there was no instrument.
//!
//! ⚠⚠ The cost of this face is not a wrong answer. It is a computation nobody wanted and a wire
//! that grows words no client knows, and neither shows up in a suite: a fixture that publishes a
//! key writes it and reads it back in the same file, so the round-trip is green whether or not the
//! key means anything to anybody else.
//!
//! # ⚠⚠⚠⚠⚠ The population is the ACT, not a name
//!
//! Item 1022 was measured on the `PANE_*` keys of one slot, and the register asked the question
//! that decides whether a gate is worth building: *is the population only those?* A prefix would
//! have been a choice with nothing behind it, and the register's warning is sharper than that —
//! widen the population and then pass it with an exemption list, and the exemption becomes the
//! gate. So the subject is derived from three facts the source states:
//!
//! 1. the constant is declared under [`crate::published::VOCABULARY`], the crate that owns this
//!    wire;
//! 2. it is `pub`, which is the crate saying the word is part of what it EXPORTS rather than one
//!    file's spelling of its own text;
//! 3. shipping code WRITES it as a JSON member name — [`crate::published::Written`] holds the three
//!    spellings this workspace uses for that act.
//!
//! Measured 2026-09-10 at `e025e615`: **294** exported string constants, **152** of them written,
//! and widening from the `PANE_*` keys to all 152 costs **ZERO** exemptions — every `RUN_*` key the
//! register worried might be persisted-and-never-read has a reader today. That measurement is what
//! chose the population. Nothing here is excused.
//!
//! # ⚠⚠⚠ What a text scan cannot claim
//!
//! [`crate::sources`]'s charter, one file over: this crate takes no dependencies, so nothing here
//! parses Rust. A reader that reached the word through `serde`'s derive, or spelled it by hand, is
//! invisible to this — and the second of those is not hypothetical. It is how the four
//! `MOUSE_*_FIELD` words this gate first went red on were orphaned: `mouse_args` writes them
//! through the constants while `parse_mouse_args` read `"button"` and `"kind"` as literals, which
//! is item 559's defect seen from the READER's side.

use std::collections::{BTreeMap, BTreeSet};

use crate::sources::{Source, outside_strings};

/// Where the host declares the vocabulary it publishes — a PREFIX over the whole crate.
///
/// ⚠ A prefix rather than the two modules that hold it today, because a list with no glob decides
/// alone — register item 470. The gate PINS which files the walk actually found declarations in,
/// so a third module joining is a person's decision rather than a fact a union absorbs.
pub const VOCABULARY: &str = "crates/sprag-host/src/";

/// How a member name reached a JSON object — the three spellings this workspace writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Written {
    /// `entry[KEY] = …` — an index-assign into a `serde_json::Value`.
    Assigned,
    /// `KEY: …` inside a `json!({ … })` literal.
    Member,
    /// `map.insert(KEY…, …)` — a `serde_json::Map` built by hand.
    Inserted,
}

/// One place a word was used, and what the use was.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Site {
    /// Workspace-relative, so a gate's message is a path a person can open.
    pub file: String,
    /// One-indexed.
    pub line: usize,
    /// `Some` when the site WRITES the word as a member name, `None` when it reads it.
    pub how: Option<Written>,
}

impl std::fmt::Display for Site {
    fn fmt(&self, into: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(into, "{}:{}", self.file, self.line)
    }
}

/// What [`declared`] knows about one constant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declaration {
    /// Workspace-relative path of the file declaring it.
    pub file: String,
    /// One-indexed line of the declaration.
    pub line: usize,
    /// The constant this one is declared to BE, when its right-hand side is another name rather
    /// than a literal.
    pub aliases: Option<String>,
}

/// One word of the host's exported vocabulary, with every shipping use of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word {
    /// The constant that declares it. When several names carry the word this is the one the others
    /// are declared to be — see [`Word::spelled`].
    pub name: String,
    /// The file that declaration is in, workspace-relative.
    pub declared_in: String,
    /// The other constants declared to BE this one, if any.
    ///
    /// ⚠⚠ An alias is not a second word, and treating it as one made this scan's first run report
    /// a defect that was not there. `WindowBirthAsk::OPENED_BY_KEY` is declared
    /// `= WINDOW_OPENED_BY_KEY` so the ask type can name its own key; the request is written
    /// through the alias and parsed through the free constant, and there is nothing to drift
    /// because the COMPILER carries that rename. What a rename cannot carry is a hand-spelled
    /// literal — which is why this merge follows the source's own `=` and never the string VALUE.
    /// Two constants that happen to spell the same English word are two words.
    pub spelled: BTreeSet<String>,
    /// Every shipping site that writes it as a member name. Non-empty by construction.
    pub writes: Vec<Site>,
    /// Every shipping site that uses it any other way.
    pub reads: Vec<Site>,
}

/// Every constant [`VOCABULARY`] exports as a string, by name.
///
/// ⚠ `pub` is required, and that is the point of it: a private constant is one file's spelling of
/// its own text, while the question here is about what the crate PUBLISHES. `config.rs` measures
/// the difference — its `ACTION_FIELD` is private, is written into a `toml_edit` table rather than
/// onto this wire, and a gate that counted it would be asking a JSON question about a TOML file.
#[must_use]
pub fn declared(sources: &[Source]) -> BTreeMap<String, Declaration> {
    let mut found = BTreeMap::new();
    for source in shipping(sources) {
        if !source.file.starts_with(VOCABULARY) {
            continue;
        }
        for (line, text) in &source.product {
            if let Some((name, aliases)) = declaration(text) {
                found.insert(
                    name,
                    Declaration {
                        file: source.file.clone(),
                        line: *line,
                        aliases,
                    },
                );
            }
        }
    }
    found
}

/// Every word of the host's exported vocabulary that shipping code WRITES as a member name, in
/// declaration-name order.
///
/// A word with no write site is absent: the question is about PUBLICATION, and a constant a client
/// sends or a grammar merely names has a different far end.
#[must_use]
pub fn published(sources: &[Source]) -> Vec<Word> {
    let declarations = declared(sources);
    let mut words: BTreeMap<String, Word> = BTreeMap::new();
    for (name, found) in &declarations {
        let head = canonical(name, &declarations);
        let entry = words.entry(head.clone()).or_insert_with(|| Word {
            declared_in: declarations.get(&head).unwrap_or(found).file.clone(),
            name: head.clone(),
            spelled: BTreeSet::new(),
            writes: Vec::new(),
            reads: Vec::new(),
        });
        if *name != head {
            entry.spelled.insert(name.clone());
        }
    }

    for source in shipping(sources) {
        for (line, name, how) in uses(source, &declarations) {
            let head = canonical(&name, &declarations);
            let Some(word) = words.get_mut(&head) else {
                continue;
            };
            let site = Site {
                file: source.file.clone(),
                line,
                how,
            };
            if how.is_some() {
                word.writes.push(site);
            } else {
                word.reads.push(site);
            }
        }
    }

    words
        .into_values()
        .filter(|word| !word.writes.is_empty())
        .collect()
}

/// The constant at the end of this one's `=` chain — the name that holds the literal.
///
/// Bounded by the number of declarations, so a cycle cannot spin. A cycle would mean no constant in
/// it holds a value, which the compiler refuses; the last name reached is as good an answer as
/// there is, and this walk must not be the thing that hangs a gate.
fn canonical(name: &str, declarations: &BTreeMap<String, Declaration>) -> String {
    let mut at = name.to_owned();
    for _ in 0..declarations.len() {
        match declarations
            .get(&at)
            .and_then(|found| found.aliases.clone())
        {
            Some(next) if next != at && declarations.contains_key(&next) => at = next,
            _ => break,
        }
    }
    at
}

/// The sources a gate about SHIPPING code may read.
///
/// ⚠ Both exclusions are [`crate::vocabulary`]'s, for its reasons: `#[cfg(test)]` items are
/// [`Source::product`]'s job, and a whole file under `tests/` is a suite that `product` cannot
/// reach. A fixture that writes a key and asserts on it is proving something ABOUT the wire — it is
/// exactly the round-trip that stays green while nothing else in the workspace has heard of the
/// word, so counting it as a reader would make this gate vouch for the defect it hunts.
fn shipping(sources: &[Source]) -> impl Iterator<Item = &Source> {
    sources
        .iter()
        .filter(|source| !source.file.contains("/tests/"))
}

/// `(name, what it is declared to be)` when this line declares an exported string constant.
///
/// The second member is `Some(other)` only when the right-hand side is another constant's NAME; a
/// string literal answers `None`, because [`outside_strings`] has already taken it away and because
/// the literal is not what this module is about.
fn declaration(text: &str) -> Option<(String, Option<String>)> {
    let rest = text.strip_prefix("pub const ")?;
    let (name, rest) = rest.split_once(':')?;
    let name = name.trim();
    if !is_constant(name) {
        return None;
    }
    let (kind, rest) = rest.split_once('=')?;
    // ⚠ EXACTLY a string, so `&[&str]` stays out. A slice of words is a vocabulary LIST, and its
    // members are named somewhere else or they are not named at all.
    if !matches!(kind.trim(), "&str" | "&'static str") {
        return None;
    }
    let value = outside_strings(rest);
    let value = value.trim().trim_end_matches(';').trim();
    Some((
        name.to_owned(),
        is_constant(value).then(|| value.to_owned()),
    ))
}

/// Whether `word` is spelled the way this workspace spells a wire constant.
fn is_constant(word: &str) -> bool {
    word.len() >= 3
        && word.starts_with(|first: char| first.is_ascii_uppercase())
        && word
            .chars()
            .all(|at| at.is_ascii_uppercase() || at.is_ascii_digit() || at == '_')
}

/// Every use of a declared constant in this source's shipping lines, as
/// `(line, name, how it was written)`.
///
/// # ⚠⚠⚠⚠ Why the file is SQUEEZED before it is read
///
/// [`Source::squeezed`]'s reason, and this scan needs it more than that one does: what says a
/// constant is being WRITTEN is the punctuation immediately around it, and rustfmt puts a line
/// break wherever the column runs out. `entry[KEY]` / `= json!(v)` and `map.insert(` / `KEY,` are
/// the same two acts written to a narrower width, and a line-at-a-time scan calls both of them
/// READS — the direction that makes a gate green about the defect it was built for.
fn uses(
    source: &Source,
    declarations: &BTreeMap<String, Declaration>,
) -> Vec<(usize, String, Option<Written>)> {
    let mut squeezed: Vec<char> = Vec::new();
    let mut lines: Vec<usize> = Vec::new();
    for (line, text) in &source.product {
        for at in outside_strings(text)
            .chars()
            .filter(|at| !at.is_whitespace())
        {
            squeezed.push(at);
            lines.push(*line);
        }
    }

    let mut found = Vec::new();
    let mut at = 0;
    while at < squeezed.len() {
        if !is_word_char(squeezed[at]) {
            at += 1;
            continue;
        }
        let start = at;
        while at < squeezed.len() && is_word_char(squeezed[at]) {
            at += 1;
        }
        let name: String = squeezed[start..at].iter().collect();
        if !declarations.contains_key(&name) {
            continue;
        }
        let before: String = squeezed[..start].iter().collect();
        let after: String = squeezed[at..].iter().collect();
        if let Some(how) = written(&before, &after) {
            found.push((lines[start], name, how));
        }
    }
    found
}

/// How this occurrence uses the constant, or `None` when it IS the declaration.
///
/// `Some(None)` is a READ: every use that is neither one of the three writes nor the declaration.
fn written(before: &str, after: &str) -> Option<Option<Written>> {
    // ⚠⚠⚠⚠⚠ THE RIGHT-HAND SIDE OF AN ALIAS IS NOT A READER, and reading it as one would blind
    // this gate for exactly the words that have two names: `pub const A: &str = B;` would hand `B`
    // a reader in the same line that says `A` means it, so no aliased word could ever be found
    // unread. It is a declaration of sameness, and nothing has been read.
    if is_alias_value(before) {
        return None;
    }
    // ⚠ A DECLARATION'S OWN NAME never reaches here: squeezing glues `pub const` to it, so
    // `pubconstPANE_BORNE_BY_KEY` is one word and no constant matches it. That is why the only
    // declaration shape this has to know about is the alias's value half, above.
    if after.starts_with(':') && !after.starts_with("::") {
        return Some(Some(Written::Member));
    }
    if after.starts_with("]=") && !after.starts_with("]==") {
        return Some(Some(Written::Assigned));
    }
    if strip_path(before).ends_with(".insert(") {
        return Some(Some(Written::Inserted));
    }
    Some(None)
}

/// Whether this occurrence stands where `pub const NAME: &str = ` has just ended — the value half
/// of an alias declaration.
///
/// ⚠ The whole `const NAME: &str =` run is required rather than a trailing `str=`, because a local
/// binding (`let word: &str = SOME_KEY;`) ends the same way and IS a read. Spelling out the
/// declaration is what keeps the two apart.
fn is_alias_value(before: &str) -> bool {
    let Some(head) = before.strip_suffix('=') else {
        return false;
    };
    let head = head
        .strip_suffix("&str")
        .or_else(|| head.strip_suffix("&'staticstr"));
    let Some(head) = head.and_then(|head| head.strip_suffix(':')) else {
        return false;
    };
    // The declared name, with `pub const` squeezed onto the front of it — see [`uses`].
    let declared: String = head
        .chars()
        .rev()
        .take_while(|at| is_word_char(*at))
        .collect::<Vec<char>>()
        .into_iter()
        .rev()
        .collect();
    declared.starts_with("pubconst") || declared.starts_with("const")
}

/// `before` with any trailing `a::b::` qualification removed, so `.insert(crate::wire::` and
/// `.insert(` are the same call.
fn strip_path(before: &str) -> &str {
    let mut cut = before;
    while let Some(rest) = cut.strip_suffix("::") {
        let head = rest.trim_end_matches(is_word_char);
        if head.len() == rest.len() {
            return cut;
        }
        cut = head;
    }
    cut
}

fn is_word_char(at: char) -> bool {
    at.is_ascii_alphanumeric() || at == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a source the way the walk builds one, so a case cannot pass against a shape the real
    /// walk would never hand it — [`crate::sources`]'s own rule, one crate in.
    fn source(file: &str, lines: &[&str]) -> Source {
        let product: Vec<(usize, String)> = lines
            .iter()
            .enumerate()
            .map(|(index, text)| (index + 1, (*text).trim().to_owned()))
            .collect();
        Source {
            file: file.to_owned(),
            code: product.clone(),
            product,
            // ⚠ This fixture's cases carry no attribute, and an empty vector here says exactly
            // that rather than standing in for one — register item 1044, whose gate spent a draft
            // reading a field that had already thrown its subject away.
            attributes: Vec::new(),
        }
    }

    /// ⚠⚠⚠⚠⚠ THE SCAN'S OWN CONTROL, over text this test owns.
    ///
    /// The gate beside this asserts that nothing was found, and *found nothing* is equally true of
    /// a scan that works and of one that has stopped matching anything — item 453's blind ratchet,
    /// in the shape an emptiness assertion invites. Once the workspace is clean there is nothing
    /// left in it to serve as a control, so the control is written here: all three ways this
    /// workspace writes a member name, against a read that must not be mistaken for one.
    #[test]
    fn the_scan_knows_the_three_ways_a_member_name_is_written_from_a_read_of_one() {
        let host = source(
            "crates/sprag-host/src/wire.rs",
            &[
                "pub const ASSIGNED_KEY: &str = \"assigned\";",
                "pub const MEMBER_KEY: &str = \"member\";",
                "pub const INSERTED_KEY: &str = \"inserted\";",
                "pub const READ_KEY: &str = \"read\";",
                "entry[ASSIGNED_KEY] = json!(true);",
                "let out = json!({ MEMBER_KEY: 1, });",
                "map.insert(crate::wire::INSERTED_KEY.to_owned(), Value::from(2));",
                "let seen = value.get(READ_KEY);",
                "entry[READ_KEY] = json!(3);",
            ],
        );
        let found = published(std::slice::from_ref(&host));
        let named: Vec<(String, Vec<Option<Written>>, usize)> = found
            .iter()
            .map(|word| {
                (
                    word.name.clone(),
                    word.writes.iter().map(|at| at.how).collect(),
                    word.reads.len(),
                )
            })
            .collect();

        assert_eq!(
            named,
            vec![
                ("ASSIGNED_KEY".to_owned(), vec![Some(Written::Assigned)], 0),
                ("INSERTED_KEY".to_owned(), vec![Some(Written::Inserted)], 0),
                ("MEMBER_KEY".to_owned(), vec![Some(Written::Member)], 0),
                ("READ_KEY".to_owned(), vec![Some(Written::Assigned)], 1),
            ],
            "each of the three writes must be seen as a write and the `get` must not be — a scan \
             that lost one of the writes reports a published word as unpublished and never looks \
             for its reader, and a scan that counted the `get` as a write would report a word \
             nothing reads",
        );
    }

    /// ⚠⚠ A constant declared to BE another is a second SPELLING, not a second word.
    #[test]
    fn a_constant_declared_to_be_another_shares_that_words_reader() {
        let host = source(
            "crates/sprag-host/src/wire.rs",
            &[
                "pub const WINDOW_OPENED_BY_KEY: &str = \"opened_by\";",
                "pub const OPENED_BY_KEY: &'static str = WINDOW_OPENED_BY_KEY;",
                "map.insert(Self::OPENED_BY_KEY.to_owned(), Value::from(opener.0));",
                "let opener = args.get(WINDOW_OPENED_BY_KEY);",
            ],
        );
        let found = published(std::slice::from_ref(&host));

        assert_eq!(found.len(), 1, "one word, two names: {found:?}");
        assert_eq!(found[0].name, "WINDOW_OPENED_BY_KEY");
        assert_eq!(
            found[0].spelled,
            BTreeSet::from(["OPENED_BY_KEY".to_owned()]),
            "the alias is recorded rather than dropped, so a message can say which name was written",
        );
        assert_eq!(
            found[0].reads.len(),
            1,
            "the free constant's reader is this word's reader — the compiler carries a rename \
             across the `=`, so there is no drift for a gate to find",
        );
    }

    /// A fixture is where this wire is PROVEN, so its round-trip is the instrument and not a reader.
    #[test]
    fn a_suites_round_trip_is_not_the_reader_this_gate_is_asking_for() {
        let host = source(
            "crates/sprag-host/src/wire.rs",
            &[
                "pub const LONELY_KEY: &str = \"lonely\";",
                "entry[LONELY_KEY] = json!(true);",
            ],
        );
        let suite = source(
            "crates/sprag-host/tests/cli.rs",
            &["assert_eq!(listed[0][LONELY_KEY], json!(true));"],
        );
        let found = published(&[host, suite]);

        assert_eq!(found.len(), 1);
        assert!(
            found[0].reads.is_empty(),
            "a suite asserting on the key it just published is the shape item 1021 stayed green \
             under for three weeks: {:?}",
            found[0].reads,
        );
    }

    /// ⚠ What the crate does not EXPORT is not what it publishes.
    #[test]
    fn a_private_constant_is_one_files_spelling_rather_than_this_crates_vocabulary() {
        let host = source(
            "crates/sprag-host/src/config.rs",
            &[
                "const ACTION_FIELD: &str = \"action\";",
                "entry[ACTION_FIELD] = value(action.to_string());",
            ],
        );
        assert!(
            published(std::slice::from_ref(&host)).is_empty(),
            "`config.rs` writes that field into a `toml_edit` table, and a gate that counted it \
             would be asking a JSON question about a TOML file",
        );
    }

    /// ⚠⚠⚠ A write rustfmt broke across lines is still a write.
    #[test]
    fn a_write_the_formatter_split_is_not_read_as_a_reader() {
        let host = source(
            "crates/sprag-host/src/workspace.rs",
            &[
                "pub const SPLIT_KEY: &str = \"split\";",
                "entry[SPLIT_KEY]",
                "= serde_json::json!(borne);",
            ],
        );
        let found = published(std::slice::from_ref(&host));

        assert_eq!(found.len(), 1);
        assert!(
            found[0].reads.is_empty(),
            "the column ran out between the index and its `=`, which is a fact about rustfmt and \
             not about the wire: {:?}",
            found[0].reads,
        );
    }
}
