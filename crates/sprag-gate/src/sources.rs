//! Every Rust source this workspace carries, for the gates that judge the TEXT of it.
//!
//! # Why a shared walker rather than one per gate
//!
//! Two gates in this crate ask a question about the whole workspace's source — *does anything
//! manufacture a program it then runs* (register item 467) and *does anything feed a child's stdin
//! and die when the child refuses first* (item 471). They need the same three things: the file
//! list, the file list to be BIG ENOUGH to have found the tree at all, and the comment lines gone
//! so a warning about a shape is not read as the shape. A walker copied into the second gate is the
//! duplication register item 213 is about, and the fiddly parts — skipping `target/`, refusing an
//! empty walk, deciding what counts as a comment — are exactly where two copies drift apart.
//!
//! # ⚠⚠ What a text scan can and cannot claim
//!
//! This crate takes no dependencies by charter and std has no Rust parser, so nothing here
//! UNDERSTANDS the code: a gate built on this answers a question about spelling. That is stated in
//! each gate rather than implied, and it is why every rule here is a ratchet on a shape that has
//! actually bitten this project rather than a general prohibition.

use std::path::{Path, PathBuf};

/// One source file, with its comment lines already dropped.
#[derive(Debug, Clone)]
pub struct Source {
    /// Relative to the workspace root, so a gate's message is a path a person can open.
    pub file: String,
    /// The lines that are not comments, as `(one-indexed line number, trimmed text)`.
    ///
    /// ⚠ Comments are dropped because every site these gates forbid now carries a comment SAYING
    /// what it must not do, naming the item — and a gate that read its own warning as the offence
    /// would go red on the fix.
    pub code: Vec<(usize, String)>,
    /// [`Source::code`] with every `#[cfg(test)]` ITEM gone — what SHIPS, rather than what proves it.
    ///
    /// # ⚠⚠⚠⚠⚠ A ratchet that counts test code punishes testing, which is backwards
    ///
    /// Register item 470 proposed counting `AiLoopState::` sites in the loop's driver and refusing
    /// an increase. Measured 2026-08-20, `outer.rs` carries **157** of them and **46 are inside its
    /// own test module** — so the ratchet as proposed would have gone red on the round that added a
    /// gate and green on the round that added an arm, which is the exact opposite of its purpose.
    ///
    /// The split is here rather than in that one gate for [`rust_sources`]'s own reason: a second
    /// copy of "where does the test code start" is where two copies drift apart, and the answer is
    /// fiddly (an item under the attribute may be a `mod`, an `impl`, a `thread_local!`, or a
    /// `const` that ends at a semicolon — this workspace carries all four).
    pub product: Vec<(usize, String)>,
}

impl Source {
    /// The file's code with every space gone, for a needle that spans lines in the real source.
    ///
    /// `child\n    .stdin\n    .take()` and `child.stdin.take()` are the same expression written
    /// two ways, and rustfmt chooses between them by line width — so a gate that looked for the
    /// second would pass the first by accident of formatting.
    #[must_use]
    pub fn squeezed(&self) -> String {
        self.code
            .iter()
            .flat_map(|(_, line)| line.chars().filter(|char| !char.is_whitespace()))
            .collect()
    }
}

/// The workspace root — `crates/sprag-gate/` is two levels down from it.
#[must_use]
pub fn workspace_root() -> PathBuf {
    [env!("CARGO_MANIFEST_DIR"), "..", ".."].iter().collect()
}

/// Every `.rs` file under `crates/`, comment lines dropped.
///
/// # Panics
///
/// When the tree cannot be read, or when the walk found so few files that it is plainly pointed
/// somewhere else. A probe pointed at nothing must never read as clean.
#[must_use]
pub fn rust_sources() -> Vec<Source> {
    let root = workspace_root();
    let mut paths = Vec::new();
    walk(&root.join("crates"), &mut paths);
    paths.sort();
    assert!(
        paths.len() > 100,
        "a scan that found only {} sources is pointed at the wrong tree, and a probe pointed at \
         nothing must never read as clean",
        paths.len(),
    );

    paths
        .into_iter()
        .map(|path| {
            let file = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .display()
                .to_string();
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|why| panic!("{file} is a source of this workspace: {why}"));
            let code: Vec<_> = text
                .lines()
                .enumerate()
                .map(|(index, line)| (index + 1, line.trim().to_owned()))
                .filter(|(_, line)| !line.starts_with("//") && !line.starts_with('#'))
                .collect();
            let proving = proving_lines(&text)
                .unwrap_or_else(|why| panic!("{file} is a source of this workspace: {why}"));
            let product = code
                .iter()
                .filter(|(line, _)| !proving.contains(line))
                .cloned()
                .collect();
            Source {
                file,
                code,
                product,
            }
        })
        .collect()
}

/// Every one-indexed line of `text` that belongs to a `#[cfg(test)]` item, the attribute included.
///
/// # ⚠⚠⚠⚠ The rule is the ATTRIBUTE and the item under it, not `mod tests`
///
/// `mod tests {` is the convention and it is not the rule: this workspace also puts the attribute
/// on a `const` that ends at a semicolon (`outer.rs`'s `CONTEXT_CEILING`, which exists so a gate
/// can author a smaller ceiling than the shipped one), on `pub(crate) fn` helpers, on a `struct`,
/// on an `impl`, and on a `thread_local!`. A reader that keyed on `mod` would walk straight past
/// every one of them — [`crate::doubles`]'s own lesson, one file over: *a list with no glob decides
/// alone*, and a needle spelled as one shape is blind to the next.
///
/// So the item's extent is taken from the SOURCE rather than assumed: brace-delimited items end
/// when their braces balance, and item forms with no braces end at their first `;`.
///
/// # Errors
///
/// When an item opened under the attribute never closes before the end of the file. That is a
/// reader which has lost its place, and the whole rest of the file would be dropped as "test code"
/// — silently making every ratchet built on [`Source::product`] looser. A probe that cannot tell
/// must never read as clean.
pub fn proving_lines(text: &str) -> Result<std::collections::BTreeSet<usize>, String> {
    let mut proving = std::collections::BTreeSet::new();
    let mut strings = Strings::default();
    let mut item: Option<Extent> = None;
    let mut governed = false;

    for (index, raw) in text.lines().enumerate() {
        let line = index + 1;
        let code = strings.code_of(raw);
        let trimmed = code.trim();

        if let Some(extent) = &mut item {
            proving.insert(line);
            if extent.eat(trimmed) {
                item = None;
            }
            continue;
        }

        if governed {
            // Between the attribute and the item there may be more attributes, more doc comment,
            // or nothing at all. None of those is the item, so none of them ends the wait.
            if trimmed.is_empty() || trimmed.starts_with('#') {
                proving.insert(line);
                continue;
            }
            proving.insert(line);
            governed = false;
            let mut extent = Extent::default();
            if !extent.eat(trimmed) {
                item = Some(extent);
            }
            continue;
        }

        if let Some(rest) = attribute_is_cfg_test(trimmed) {
            proving.insert(line);
            if rest.is_empty() {
                governed = true;
            } else {
                let mut extent = Extent::default();
                if !extent.eat(rest) {
                    item = Some(extent);
                }
            }
        }
    }

    if item.is_some() || governed {
        return Err(
            "a `#[cfg(test)]` item is still open at the end of the file, so this reader has lost \
             its place — everything after it would be dropped as test code and every ratchet built \
             on it would go quietly slack"
                .to_owned(),
        );
    }
    Ok(proving)
}

/// `Some(what follows it on the line)` when `line` starts a `#[cfg(test)]` attribute.
///
/// ⚠ `#[cfg(all(test, unix))]` and `#[cfg(test)]` both govern test-only code, and a reader that
/// only knew the bare spelling would count the first one's item as shipping. The needle is
/// therefore `cfg(` … `test` on an attribute rather than one exact string — *the rule, not the
/// spelling*, which is register item 453's whole finding.
///
/// ⚠⚠⚠⚠⚠ **AND THE WORD SCAN ALONE IS WRONG IN THE OTHER DIRECTION.** `#[cfg(not(test))]` names
/// `test` and governs code that ships in every build EXCEPT the test one — dropping its item would
/// make a ratchet under-count the driver, which is a gate going quietly slack rather than red.
/// Measured 2026-08-20: this workspace carries exactly one of them, and one is enough. Negated
/// groups are therefore removed before the word is looked for.
fn attribute_is_cfg_test(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("#[")?;
    let close = matching_bracket(rest)?;
    let (attribute, after) = rest.split_at(close);
    if !attribute.starts_with("cfg(") {
        return None;
    }
    let names_test = without_negations(attribute)
        .split(|char: char| !char.is_ascii_alphanumeric() && char != '_')
        .any(|word| word == "test");
    names_test.then(|| after.strip_prefix(']').unwrap_or(after).trim())
}

/// `attribute` with every `not(…)` group removed, parentheses balanced rather than counted to the
/// first `)` — `not(any(target_os = "linux", target_os = "macos"))` is one group, not two.
fn without_negations(attribute: &str) -> String {
    let mut kept = String::with_capacity(attribute.len());
    let mut rest = attribute;
    while let Some(at) = rest.find("not(") {
        kept.push_str(&rest[..at]);
        let inside = &rest[at + "not(".len()..];
        let mut depth = 1usize;
        let mut end = inside.len();
        for (index, char) in inside.char_indices() {
            match char {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = index + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        rest = &inside[end.min(inside.len())..];
    }
    kept.push_str(rest);
    kept
}

/// The index of the `]` that closes an attribute opened by the caller's `#[`.
fn matching_bracket(rest: &str) -> Option<usize> {
    let mut depth = 1usize;
    for (index, char) in rest.char_indices() {
        match char {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

/// How far an item under `#[cfg(test)]` reaches.
#[derive(Default)]
struct Extent {
    depth: usize,
    braced: bool,
}

impl Extent {
    /// Feeds one line of already-de-stringed code; `true` once the item has ended.
    fn eat(&mut self, code: &str) -> bool {
        for char in code.chars() {
            match char {
                '{' => {
                    self.braced = true;
                    self.depth += 1;
                }
                '}' => self.depth = self.depth.saturating_sub(1),
                // `const C: &str = "…";` and `use …;` and `struct S;` all end here — but only
                // before any brace has opened, so a `;` INSIDE a body is not mistaken for the end.
                ';' if !self.braced && self.depth == 0 => return true,
                _ => {}
            }
        }
        self.braced && self.depth == 0
    }
}

/// A line reader that knows a brace inside a string literal is not a brace.
///
/// # ⚠⚠⚠ It has to carry state across lines, because this workspace's strings do
///
/// Nearly every refusal message here is one string literal continued with a trailing `\` over four
/// or five lines. A per-line stripper would see the opening `"` with no close, then read the next
/// line's ordinary code as string contents — or worse, read a `{` in the prose as an item opening.
#[derive(Default)]
struct Strings {
    /// `Some(hashes)` while inside a literal — `0` for an ordinary `"…"`, `n` for `r#…"…"#…`.
    open: Option<usize>,
}

impl Strings {
    /// `line` with comment text and literal contents removed, so only structure is left.
    fn code_of(&mut self, line: &str) -> String {
        let chars: Vec<char> = line.chars().collect();
        let mut out = String::with_capacity(chars.len());
        let mut at = 0;
        while at < chars.len() {
            if let Some(hashes) = self.open {
                at = self.close_at(&chars, at, hashes);
                continue;
            }
            match chars[at] {
                '/' if chars.get(at + 1) == Some(&'/') => break,
                '"' => {
                    self.open = Some(0);
                    at += 1;
                }
                'r' if raw_hashes(&chars, at).is_some() => {
                    let hashes = raw_hashes(&chars, at).unwrap_or_default();
                    self.open = Some(hashes);
                    at += 2 + hashes;
                }
                // A char literal holds one char or one escape and then closes; a lifetime never
                // closes. Only the first is a literal, and only the first can hide a brace.
                '\'' => {
                    at += char_literal(&chars, at).unwrap_or(1);
                }
                char => {
                    out.push(char);
                    at += 1;
                }
            }
        }
        // An ordinary string does not survive the end of a line unless the line asked it to.
        if self.open == Some(0) && !line.trim_end().ends_with('\\') {
            self.open = None;
        }
        out
    }

    /// Consumes literal contents from `at`, returning where the caller should look next.
    fn close_at(&mut self, chars: &[char], at: usize, hashes: usize) -> usize {
        if hashes == 0 {
            match chars[at] {
                '\\' => return at + 2,
                '"' => {
                    self.open = None;
                    return at + 1;
                }
                _ => return at + 1,
            }
        }
        if chars[at] == '"' && chars[at + 1..].iter().take(hashes).all(|char| *char == '#') {
            self.open = None;
            return at + 1 + hashes;
        }
        at + 1
    }
}

/// `Some(hashes)` when `r`, `r"`, or `r#…"` starts a raw string at `at`.
fn raw_hashes(chars: &[char], at: usize) -> Option<usize> {
    let mut hashes = 0;
    while chars.get(at + 1 + hashes) == Some(&'#') {
        hashes += 1;
    }
    (chars.get(at + 1 + hashes) == Some(&'"')).then_some(hashes)
}

/// The width of a char literal starting at `at`, or `None` when it is a lifetime.
fn char_literal(chars: &[char], at: usize) -> Option<usize> {
    let width = if chars.get(at + 1) == Some(&'\\') {
        4
    } else {
        3
    };
    (chars.get(at + width - 1) == Some(&'\'')).then_some(width)
}

fn walk(dir: &Path, into: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir)
        .unwrap_or_else(|why| panic!("{} is this workspace's source: {why}", dir.display()));
    for entry in entries {
        let path = entry.expect("read a directory entry").path();
        if path.is_dir() {
            // Build output is not source, and it holds copies of everything.
            if path.file_name().is_some_and(|name| name == "target") {
                continue;
            }
            walk(&path, into);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            into.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠⚠⚠ The walk finds this crate's own files, and a comment naming a shape is NOT the shape.
    ///
    /// ⚠⚠⚠⚠⚠ **THE FIRST VERSION OF THIS CASE MATCHED ITSELF AND WENT RED FOR IT.** It asserted
    /// that a phrase appearing only in this file's HEADER was absent from the code — and spelled
    /// that phrase as a string literal, which put it in the code. The claim is structural, so it is
    /// asserted structurally here: no line a gate reads may be a comment, and the file must have
    /// comments for that to mean anything.
    #[test]
    fn the_walk_reaches_this_crate_and_drops_what_a_comment_says() {
        let sources = rust_sources();
        let mine = sources
            .iter()
            .find(|source| source.file == "crates/sprag-gate/src/sources.rs")
            .expect("the walker finds its own file");

        assert!(
            mine.squeezed().contains("fnrust_sources()"),
            "the code of the file is what a gate reads",
        );

        let raw = std::fs::read_to_string(workspace_root().join(&mine.file))
            .expect("the file the walker just read");
        let commented = raw
            .lines()
            .filter(|line| line.trim_start().starts_with("//"))
            .count();
        assert!(
            commented > 20,
            "this file is mostly reasoning, and a claim about dropping comments is vacuous \
             without them: {commented}",
        );
        assert!(
            mine.code.iter().all(|(_, line)| !line.starts_with("//")),
            "not one comment line may reach a gate — every site the gates forbid now carries a \
             comment SAYING what it must not do, and a gate that read its own warning as the \
             offence would go red on the fix",
        );
    }

    /// ⚠⚠⚠⚠⚠ **BOTH DIRECTIONS, because a reader that drops everything is as blind as one that
    /// drops nothing** — and only one of the two announces itself.
    ///
    /// Each row is an item as a source of this workspace could really carry it, plus the count of
    /// its lines that belong to the PROVING rather than to the product. Every governing spelling
    /// in the tree is represented (measured 2026-08-20: `#[cfg(test)]`, `#[cfg(all(test, unix))]`
    /// and `#[cfg(not(test))]` are the three that mention the word), and so is every item form the
    /// attribute is put on here — `mod`, `const`, `fn`, `struct`, `impl`, `thread_local!`, `use`.
    #[test]
    fn the_reader_sees_every_shape_of_a_test_only_item_and_declines_the_rest() {
        let table: &[(&str, usize)] = &[
            ("#[cfg(test)]\nmod tests {\n    fn one() {}\n}\n", 4),
            // The item form with no braces at all, and what this workspace really uses it for: a
            // name the DOCUMENT reads, which no shipping line of the driver ever mentions.
            (
                "#[cfg(test)]\nconst CEILING: &str = \"context_ceiling\";\n",
                2,
            ),
            // A brace inside the literal, which is what a reader with no string state trips over.
            (
                "#[cfg(test)]\nconst SHAPE: &str = \"a {\";\nfn ships() {}\n",
                2,
            ),
            ("#[cfg(all(test, unix))]\nfn probe() {\n    ()\n}\n", 4),
            // Attributes stack, and the governing one need not be last.
            (
                "#[cfg(test)]\n#[derive(Debug)]\nstruct Seen {\n    at: u8,\n}\n",
                5,
            ),
            (
                "#[cfg(test)]\nthread_local! {\n    static N: u8 = 0;\n}\n",
                4,
            ),
            (
                "#[cfg(test)]\nimpl Seen {\n    fn at(&self) -> u8 {\n        0\n    }\n}\n",
                6,
            ),
            ("#[cfg(test)]\nuse std::fmt;\n", 2),
            // ⚠⚠⚠ DECLINED — every one of these SHIPS, and dropping it makes a ratchet slack.
            ("#[cfg(not(test))]\nfn ships() {}\n", 0),
            ("#[cfg(unix)]\nfn ships() {}\n", 0),
            ("#[cfg(target_os = \"linux\")]\nfn ships() {}\n", 0),
            ("fn ships() {\n    let brace = \"{\";\n}\n", 0),
            // A comment that merely SAYS the words is not the attribute.
            ("/// see `#[cfg(test)]` for why\nfn ships() {}\n", 0),
        ];

        let mut wrong = Vec::new();
        for (source, owed) in table {
            let read = proving_lines(source).map(|lines| lines.len());
            if read.as_ref().ok() != Some(owed) {
                wrong.push(format!("owed {owed}, read {read:?} for {source:?}"));
            }
        }
        assert!(
            wrong.is_empty(),
            "the reader that decides what SHIPS must see every shape this workspace writes, and \
             decline every shape it does not: {wrong:#?}",
        );
    }

    /// ⚠⚠⚠⚠ An item that never closes is a reader that has LOST ITS PLACE, and the whole rest of
    /// the file would go quietly into the proving pile. It has to say so instead.
    #[test]
    fn an_unclosed_test_item_is_refused_rather_than_swallowing_the_file() {
        assert!(
            proving_lines("#[cfg(test)]\nmod tests {\n    fn one() {}\n").is_err(),
            "a reader that cannot find the end of the item it is dropping must refuse, because \
             the alternative is a ratchet that gets looser without anybody being told",
        );
    }

    /// ⚠⚠⚠ The split is worth nothing unless the file it is measured on really HAS both halves.
    #[test]
    fn the_split_is_measured_on_a_file_that_really_has_both_halves() {
        let sources = rust_sources();
        let driver = sources
            .iter()
            .find(|source| source.file == "crates/sprag-plugin/src/outer.rs")
            .expect("the loop's driver is a source of this workspace");

        assert!(
            driver.product.len() < driver.code.len(),
            "the driver carries its own test module and the split must see it",
        );
        assert!(
            driver.product.len() > driver.code.len() / 4,
            "and it must not have swallowed the file: {} product lines of {}",
            driver.product.len(),
            driver.code.len(),
        );
    }
}
