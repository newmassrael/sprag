//! Which item a `///` block documents, and whether a change moved one onto another item — register
//! item 1088.
//!
//! # ⛔⛔⛔⛔⛔ What moves, and why every gate stayed green about it
//!
//! Rust attaches an outer doc comment to the NEXT item. So an item declared between a `///` block
//! and the item the block was written for takes the block, and the item it was written for is left
//! with none. The compiler accepts both attachments, clippy has no opinion, and rustdoc renders the
//! stolen prose on the wrong item. Measured on this repository's history, 2026-09-13:
//!
//! * `5a242872` (register item 996) declared `silent_by_kind_json` and `silent_by_kind_in` between
//!   `folds_by_reason_json` and its doc. It stood four days until register item 1073 put the doc
//!   back — and 1073's own first draft did the same thing again, with a constant.
//! * `706c4019` (register item 1052) declared `Placed` between `Item` and its doc, beside a comment
//!   warning against doing exactly that to the derive. It was still standing when this was written.
//! * `ab07598a` did it to `DetachOnDestroy` and was stopped before landing only because the stolen
//!   text happened to link a private item, which `-D warnings` refuses.
//!
//! # ⚠⚠⚠ Why the subject is a CHANGE and not the tree
//!
//! Nothing in one tree separates a doc that describes its item from one that describes a neighbour.
//! Three readings of the tree were measured before this was written, and all three were refused:
//!
//! * a bold headline glued to the doc line above it — **435** such lines in the workspace;
//! * a doc whose subject is another item's name — **0 of 2** on the two standing instances, whose
//!   stolen text names neither the item that took it nor the one it was taken from;
//! * an item with no doc at all — **598** in the workspace, of which the stranded `Item` is one.
//!
//! What IS decidable is the edit: a block that documented `K` before a change and documents `K′`
//! after it, while `K` is still there without it. So [`crate::doc_attachment::displaced`] reads two
//! versions of one file, and [`crate::doc_attachment::judge_commit`] reads them out of a commit and
//! its first parent. ⚠ Spelled through the crate, as `rust_source`'s header spells its own: this
//! header is read together with the one on `pub mod doc_attachment` and resolved from the crate root.
//!
//! # ⚠⚠⚠ What still stands, and putting it back — register item 1091
//!
//! A gate at the commit stops the next move and says nothing about the ones already made. Asked of
//! this repository's whole history on 2026-09-13, the census found **153** moves in **2175** commits,
//! **120** of them still standing at `97f7c9a6`, in **51** files. Two readers answer for those:
//!
//! * [`crate::doc_attachment::TreeAt`] asks whether a move stands in EVERY Rust file of a commit, not
//!   in the path it was made in — code leaves its file, and a question tied to the path cannot follow
//!   it (the census's one *could not ask* was exactly that).
//! * [`crate::doc_attachment::repaired`] takes a standing block out of the doc that carries it and puts
//!   it back above the item it was written for — or refuses, saying why, where the text does not say
//!   which item that is.
//!
//! # ⚠⚠ THE RESIDUE, STATED RATHER THAN HIDDEN
//!
//! * **An item is keyed by what its first line names** — `fn name`, `struct Name`, `field name` —
//!   and not by a nesting this does not parse. So a RENAME whose old name survives elsewhere in the
//!   file on an undocumented line can read as a move. How often that happens is a measurement, and
//!   it was taken over this repository's whole history before this became a gate. ⚠ Braces ARE
//!   followed as far as fields and variants need, since register item 1091: `id:` is a field only
//!   inside a struct's braces, so a struct literal no longer stands in for one, and a field is named
//!   through the type that declares it, `field PaneRef::id`, so another type's `id` does not either.
//! * **A block the same change also rewrote is not followed.** The moved block is looked for whole;
//!   a displacement that edits the text it moved is not seen.

use crate::ambient::git_in;
use crate::rust_source::{Shape, scan};
use std::collections::BTreeMap;
use std::path::Path;

/// One outer doc block, and the item it documents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    /// The block's lines with `///` and the whitespace around each removed, in order. A bare `///`
    /// is an empty line here, because a paragraph break is part of what was written.
    pub doc: Vec<String>,
    /// The one-indexed line each entry of [`Attachment::doc`] stands on, in the same order — so a
    /// run of the block can be found in the source and not only in the prose.
    pub doc_lines: Vec<usize>,
    /// What the block documents, as [`key_of`] names it — a field or a variant through the type that
    /// declares it, `field PaneRef::id`.
    pub key: String,
    /// The one-indexed line the documented item begins on.
    pub line: usize,
    /// Every outer attribute read while the item waited — above the block, inside it, or between it
    /// and the item — in source order.
    pub attributes: Vec<Attribute>,
}

/// One outer attribute, where it stands and what it says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attribute {
    /// The one-indexed lines it spans, first and last — more than one for `#[cfg_attr(` … `)]`.
    pub lines: (usize, usize),
    /// Its lines trimmed and joined with a newline, so one attribute indented two ways is one text.
    pub text: String,
}

/// An item line that carries no doc, and where a doc written for it would stand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bare {
    /// The one-indexed line the item begins on.
    pub line: usize,
    /// Its outer attributes, in source order.
    pub attributes: Vec<Attribute>,
}

impl Bare {
    /// The one-indexed line its outer attributes begin on, or [`Bare::line`] when it has none. A doc
    /// block goes directly above this line: put between an attribute and its item it would still
    /// attach, but `#[test]` above a doc is not how anybody here writes one.
    #[must_use]
    pub fn landing(&self) -> usize {
        self.attributes
            .first()
            .map_or(self.line, |attribute| attribute.lines.0)
    }
}

/// A block that documented one item before a change and documents another after it, while the
/// first is still in the file with no doc of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Displacement {
    /// The item the block was written for.
    pub was: String,
    /// The item that carries it now.
    pub now: String,
    /// The one-indexed line, in the changed file, of the item that carries it now.
    pub line: usize,
    /// The whole block that moved, as [`Attachment::doc`] holds it — never empty, since a block with
    /// no lines documents nothing and cannot move.
    ///
    /// ⚠ The WHOLE block and not its first line: where the moved prose ends inside the doc that took
    /// it is not in any later version of the file, and a repair has to know.
    pub doc: Vec<String>,
    /// The texts of the attributes that stood between the block and the item it was written for,
    /// as [`Attribute::text`] spells them, in source order.
    ///
    /// ⛔⛔ AN ITEM DECLARED BETWEEN AN ATTRIBUTE AND ITS ITEM TAKES THE ATTRIBUTE WITH THE DOC —
    /// register item 1091, measured three times: `dccca9c3` gave `backlogs`' `#[must_use]` to
    /// `discharged_owner`, `865ead09` gave `cells_text`'s to `find_in_line`, and `55bfb838` gave
    /// `oracle_at`'s `#[cfg(test)]` to a test. A repair that put back the prose alone would leave the
    /// code meaning something else.
    pub attributes: Vec<String>,
}

impl Displacement {
    /// The block's first line, so a reader can find the prose that moved.
    #[must_use]
    pub fn opening(&self) -> &str {
        self.doc.first().map_or("", String::as_str)
    }
}

/// What one line of a source is, for the question of which item a doc block reaches.
enum Line<'a> {
    /// A `///` line, with its text.
    Doc(&'a str),
    /// Blank, or a comment that is not an outer doc. Neither ends a block's wait for its item.
    Quiet,
    /// The first line of an outer attribute, trimmed. It stands between a doc and its item without
    /// being the item.
    Attribute(&'a str),
    /// Any other line, trimmed.
    Code(&'a str),
}

/// Every line of `text`, classified, with the punctuation on it that opens and closes bodies —
/// `{`, `}`, `;`, `(`, `)`, `[` and `]`, in order, outside every comment and literal.
///
/// ⚠⚠ THE COMMENTS ARE [`scan`]'s AND NOT A LINE PREFIX — register item 1051's rule. A fixture in a
/// raw string holds `///` lines that are not comments at all, and a reader that trusted the prefix
/// would find doc blocks inside a test's data.
///
/// ⚠⚠ AND SO ARE THE LITERALS — register item 1091. A line that begins inside a multi-line string is
/// the string's text: a fixture holding `fn new() {}` declares no `new`, and before this a repair
/// could choose that line to put a doc above. A `{` in a format string opens nothing either.
///
/// # Errors
///
/// When the scan ran out inside a literal or a comment: the rest of the file could not be told apart
/// into code and prose, and a reading of it would be a guess.
fn lines_of(text: &str) -> Result<Vec<(Line<'_>, Vec<u8>)>, String> {
    let scanned = scan(text);
    if let Some(unclosed) = scanned.unclosed {
        return Err(format!(
            "the source runs out inside an unclosed {unclosed:?}, so its doc blocks cannot be told \
             from its code"
        ));
    }
    let mut code = text.as_bytes().to_vec();
    for (at, end) in scanned
        .comments
        .iter()
        .map(|comment| (comment.at, comment.end))
        .chain(
            scanned
                .literals
                .iter()
                .map(|literal| (literal.at, literal.end)),
        )
    {
        code[at..end].fill(b' ');
    }
    let mut out = Vec::new();
    let mut offset = 0;
    for raw in text.split_inclusive('\n') {
        let structure: Vec<u8> = code[offset..offset + raw.len()]
            .iter()
            .copied()
            .filter(|byte| matches!(byte, b'{' | b'}' | b';' | b'(' | b')' | b'[' | b']'))
            .collect();
        let body = raw.trim_end_matches(['\n', '\r']);
        let trimmed = body.trim_start();
        let start = offset + (body.len() - trimmed.len());
        offset += raw.len();
        let trimmed = trimmed.trim_end();
        if trimmed.is_empty() {
            out.push((Line::Quiet, structure));
            continue;
        }
        // ⚠ Past a literal's opening, not at it: a line that begins WITH a quote is code.
        let inside_a_literal = scanned
            .literals
            .partition_point(|literal| literal.at < start)
            .checked_sub(1)
            .is_some_and(|at| start < scanned.literals[at].end);
        // The last comment opening at or before this line's first character, if the line begins
        // inside it. ⚠ A binary search over the scan's source order and not a walk from the top:
        // walked per line, a 20,000-line file with 15,000 comments costs their product, and the
        // history this was measured on holds every version of such files.
        let before = scanned
            .comments
            .partition_point(|comment| comment.at <= start);
        let covering = before
            .checked_sub(1)
            .map(|at| &scanned.comments[at])
            .filter(|comment| start < comment.end);
        let line = match covering {
            Some(comment) if comment.shape == Shape::WholeLine && comment.at == start => {
                match text[comment.at..comment.end].strip_prefix("///") {
                    // ⚠ `////` is an ordinary comment, which is Rust's own rule.
                    Some(rest) if !rest.starts_with('/') => Line::Doc(rest.trim()),
                    _ => Line::Quiet,
                }
            }
            Some(_) => Line::Quiet,
            None if inside_a_literal => Line::Quiet,
            None if trimmed.starts_with("#[") => Line::Attribute(trimmed),
            None => Line::Code(trimmed),
        };
        out.push((line, structure));
    }
    Ok(out)
}

/// What kind of braces a line stands inside, and whose they are, as far as *which field or variant is
/// this line* needs — register item 1091.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Body {
    /// The braces of the type named here — a struct, a union, or an enum's struct-like variant
    /// spelled `Enum::Variant` — where `name: Type` declares a field.
    Fields(String),
    /// The braces of the enum named here, where `Name,` declares a variant.
    Variants(String),
    /// Any other braces — a function, an impl, a module, a struct literal — and the file itself.
    Other,
}

/// The braces open at one point of a file, innermost last, and what the next `{` opens.
#[derive(Debug, Default)]
struct Nesting {
    /// Every body open here, outermost first.
    bodies: Vec<Body>,
    /// The body the last struct, union, enum or struct-like variant header announced, until a `{`
    /// opens it or a `;` outside brackets ends the header without one.
    announced: Option<Body>,
    /// How many `(` and `[` are open, so the `;` of `[u8; 4]` in a header's bounds ends nothing.
    brackets: usize,
}

impl Nesting {
    /// The innermost body open here.
    fn within(&self) -> &Body {
        self.bodies.last().unwrap_or(&Body::Other)
    }

    /// A line keyed `key` by [`key_of`], read here: a struct, union or enum header announces the
    /// body it opens, and so does a variant inside an enum, whose braces hold that variant's fields.
    fn header(&mut self, key: &str) {
        let (kind, name) = key.split_once(' ').unwrap_or((key, ""));
        self.announced = match (kind, self.within()) {
            ("struct" | "union", _) => Some(Body::Fields(name.to_owned())),
            ("enum", _) => Some(Body::Variants(name.to_owned())),
            ("variant", Body::Variants(owner)) => Some(Body::Fields(format!("{owner}::{name}"))),
            _ => return,
        };
    }

    /// The punctuation of one line, applied in order.
    fn step(&mut self, structure: &[u8]) {
        for byte in structure {
            match byte {
                b'{' => {
                    let opened = self.announced.take().unwrap_or(Body::Other);
                    self.bodies.push(opened);
                }
                b'}' => {
                    self.bodies.pop();
                }
                b'(' | b'[' => self.brackets += 1,
                b')' | b']' => self.brackets = self.brackets.saturating_sub(1),
                b';' if self.brackets == 0 => self.announced = None,
                _ => {}
            }
        }
    }
}

/// How many `[` are still open after `code`, starting from `depth`, with string contents skipped.
///
/// ⚠ An attribute can span lines — `#[cfg_attr(\n feature = "x",\n derive(Debug)\n)]` — and every
/// line of it stands between a doc and its item. A reader that took the second line for the item
/// would key the block on `feature = "x",`.
fn brackets_open_after(mut depth: usize, code: &str) -> usize {
    let mut in_string = false;
    let mut escaped = false;
    for ch in code.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    depth
}

/// One version of a file, read once for its doc blocks and its undocumented items.
///
/// ⚠ Read once and asked many times: the census asks every move it found of the files that still
/// hold that move's prose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Docs {
    /// Every doc block, with the item it documents, in source order.
    pub attached: Vec<Attachment>,
    /// Every item line that carries no doc, by its [`key_of`] key, in source order.
    pub bare: BTreeMap<String, Vec<Bare>>,
}

impl Docs {
    /// Every doc block of `text` with the item it documents, and every item line that carries none.
    ///
    /// # Errors
    ///
    /// When the source cannot be read for its comments — see the reader's own refusal.
    pub fn read(text: &str) -> Result<Docs, String> {
        let mut attached = Vec::new();
        let mut bare: BTreeMap<String, Vec<Bare>> = BTreeMap::new();
        let mut pending: Vec<String> = Vec::new();
        let mut pending_lines: Vec<usize> = Vec::new();
        // The attributes read since the last code line. Blank lines, plain comments and doc lines
        // leave them standing, as they leave a doc's wait.
        let mut attributes: Vec<Attribute> = Vec::new();
        let mut open_attribute = 0;
        let mut nesting = Nesting::default();
        for (index, (line, structure)) in lines_of(text)?.into_iter().enumerate() {
            // ⚠ A line stands in the body open BEFORE its own braces, and every line's braces are
            // applied once, after it is read — an attribute's continuation lines included.
            if open_attribute > 0 {
                if let Line::Code(code) | Line::Attribute(code) = line {
                    open_attribute = brackets_open_after(open_attribute, code);
                    if let Some(open) = attributes.last_mut() {
                        open.lines.1 = index + 1;
                        open.text.push('\n');
                        open.text.push_str(code);
                    }
                }
            } else {
                match line {
                    Line::Doc(doc) => {
                        pending.push(doc.to_owned());
                        pending_lines.push(index + 1);
                    }
                    Line::Quiet => {}
                    Line::Attribute(code) => {
                        attributes.push(Attribute {
                            lines: (index + 1, index + 1),
                            text: code.to_owned(),
                        });
                        open_attribute = brackets_open_after(0, code);
                    }
                    Line::Code(code) => {
                        let key = key_of(code);
                        let named = item_named(&key, nesting.within());
                        nesting.header(&key);
                        let attributes = std::mem::take(&mut attributes);
                        if pending.is_empty() {
                            if let Some(named) = named {
                                bare.entry(named).or_default().push(Bare {
                                    line: index + 1,
                                    attributes,
                                });
                            }
                        } else {
                            attached.push(Attachment {
                                doc: std::mem::take(&mut pending),
                                doc_lines: std::mem::take(&mut pending_lines),
                                key: named.unwrap_or(key),
                                line: index + 1,
                                attributes,
                            });
                        }
                    }
                }
            }
            nesting.step(&structure);
        }
        Ok(Docs { attached, bare })
    }

    /// Where `moved` still stands in this version, or [`None`] where it does not: the block's
    /// opening still sits in a doc of an item OTHER than the one it was written for, and that item is
    /// still here with no doc — register item 1088's census question.
    ///
    /// ⚠⚠ THE SAME READER AS [`displaced`], so *this commit moved a doc* and *that move is still in
    /// the tree* are one author's two answers rather than a second parser over the first one's
    /// printout.
    ///
    /// # ⛔⛔⛔ ANY other item, and not the one that first took it — register item 1091
    ///
    /// A block that was moved can be moved again with the doc it was glued to. `786a1628` put
    /// `spawn_durability_saver`'s doc on `put_back_inherited_runs`, and `9346eb6e` then declared
    /// `leftover_driver` above that glued doc, taking both. Asked *is it on `put_back_inherited_runs`*,
    /// the first move answered *gone* while `spawn_durability_saver` still had no doc: a census that
    /// counts done by where the prose went first reads a second move as a repair.
    #[must_use]
    pub fn standing(&self, moved: &Displacement) -> Option<Standing> {
        let written_for = self.bare.get(&moved.was).cloned().unwrap_or_default();
        let carried_by: Vec<usize> = self
            .attached
            .iter()
            .filter(|held| held.key != moved.was && held.doc.iter().any(|l| l == moved.opening()))
            .map(|held| held.line)
            .collect();
        (!written_for.is_empty() && !carried_by.is_empty()).then_some(Standing {
            carried_by,
            written_for,
        })
    }
}

/// Every doc block of `text`, and the item each one documents.
///
/// # Errors
///
/// When the source cannot be read for its comments — see the reader's own refusal.
pub fn attachments(text: &str) -> Result<Vec<Attachment>, String> {
    Ok(Docs::read(text)?.attached)
}

/// The item keywords [`key_of`] names an item by, in the order it tries them.
const ITEM_KEYWORDS: [&str; 9] = [
    "fn", "struct", "enum", "union", "trait", "type", "const", "static", "mod",
];

/// `key` as it names an item standing `within` a body, or [`None`] for a line that names none there:
/// a field only inside a type's braces and a variant only inside an enum's, each named through the
/// type that declares it, and every other item kind anywhere, as [`key_of`] names it.
///
/// ⛔⛔⛔ THE BODY DECIDES FIELDS AND VARIANTS, AND NAMES THEIR OWNER — register item 1091, measured.
/// `8c2c6817` gave `PaneRef`'s field `id` to an accessor `fn id` along with its doc, and twenty-five
/// other lines of that file read as `field id` left undocumented: twenty-three struct literals and
/// parameters, which are no fields at all, and the `id` fields of `PaneInfo` and `ImageInfo`, which
/// are not `PaneRef`'s. A correct edit stood in the census as a move no repair could ever take down.
fn item_named(key: &str, within: &Body) -> Option<String> {
    let (kind, name) = key.split_once(' ').unwrap_or((key, ""));
    match (kind, within) {
        ("field", Body::Fields(owner)) => Some(format!("field {owner}::{name}")),
        ("variant", Body::Variants(owner)) => Some(format!("variant {owner}::{name}")),
        ("field" | "variant", _) => None,
        _ if ITEM_KEYWORDS.contains(&kind) || ["impl", "macro_rules"].contains(&kind) => {
            Some(key.to_owned())
        }
        _ => None,
    }
}

/// `code` with the qualifiers that come before an item keyword removed: visibility, `default`,
/// `async`, `unsafe`, `extern "abi"`, and `const` where it qualifies a `fn`.
fn after_qualifiers(code: &str) -> &str {
    let mut rest = code.trim_start();
    loop {
        if let Some(after) = rest.strip_prefix("pub(")
            && let Some(close) = after.find(')')
        {
            rest = after[close + 1..].trim_start();
            continue;
        }
        if let Some(after) = rest.strip_prefix("extern ") {
            let after = after.trim_start();
            rest = match after
                .strip_prefix('"')
                .and_then(|abi| abi.find('"').map(|at| (abi, at)))
            {
                Some((abi, close)) => abi[close + 1..].trim_start(),
                None => after,
            };
            continue;
        }
        if let Some(after) = ["pub ", "default ", "async ", "unsafe "]
            .iter()
            .find_map(|word| rest.strip_prefix(word))
        {
            rest = after.trim_start();
            continue;
        }
        if let Some(after) = rest.strip_prefix("const ") {
            let after = after.trim_start();
            if ["fn ", "unsafe ", "async ", "extern "]
                .iter()
                .any(|word| after.starts_with(word))
            {
                rest = after;
                continue;
            }
        }
        return rest;
    }
}

/// The identifier `text` begins with, `r#` included; empty when it begins with none.
fn identifier(text: &str) -> &str {
    let body = text.strip_prefix("r#").unwrap_or(text);
    let prefix = text.len() - body.len();
    let end = body
        .find(|ch: char| !(ch.is_alphanumeric() || ch == '_'))
        .unwrap_or(body.len());
    &text[..prefix + end]
}

/// What an item's first line names it: `fn name`, `struct Name`, `impl Trait for Type`, `field
/// name`, `variant Name` — or `line <text>` for anything that is none of those.
///
/// ⚠ A NAME AND NOT A SIGNATURE. A parameter added to a function is not a different function, and a
/// key that moved with every edit to its line would turn an ordinary change into a moved doc.
#[must_use]
pub fn key_of(code: &str) -> String {
    let rest = after_qualifiers(code);
    for keyword in ITEM_KEYWORDS {
        if let Some(after) = rest.strip_prefix(keyword)
            && after.starts_with(char::is_whitespace)
        {
            let after = after.trim_start();
            let after = match keyword {
                "static" => after.strip_prefix("mut ").map_or(after, str::trim_start),
                _ => after,
            };
            return format!("{keyword} {}", identifier(after));
        }
    }
    if let Some(after) = rest.strip_prefix("macro_rules!") {
        return format!("macro_rules {}", identifier(after.trim_start()));
    }
    if let Some(after) = rest.strip_prefix("impl")
        && (after.starts_with(char::is_whitespace) || after.starts_with('<'))
    {
        let head = after.split('{').next().unwrap_or(after);
        return format!(
            "impl {}",
            head.split_whitespace().collect::<Vec<_>>().join(" ")
        );
    }
    let name = identifier(rest);
    if !name.is_empty() {
        let after = rest[name.len()..].trim_start();
        if after.starts_with(':') && !after.starts_with("::") {
            return format!("field {name}");
        }
        if name.starts_with(|ch: char| ch.is_ascii_uppercase())
            && (after.is_empty() || after.starts_with([',', '(', '{', '=']))
        {
            return format!("variant {name}");
        }
    }
    format!(
        "line {}",
        rest.split_whitespace().collect::<Vec<_>>().join(" ")
    )
}

/// Every doc block that a change from `before` to `after` took off the item it was written for and
/// put on another, while the first item is still there without a doc.
///
/// # ⚠⚠ The three conditions, and what each one keeps out
///
/// 1. **The whole block survives, as a run inside one block of `after`.** A block that was edited
///    or deleted is not a block that moved.
/// 2. **No block of `after` holding it documents the same item.** A block that moved WITH its item,
///    or that another item also carries word for word, is still where it was written. And of the
///    items holding it, only one that did not already carry it in `before` is where it went: the
///    same prose written twice leaves a copy that was there all along, and a block only such copies
///    still hold was deleted.
/// 3. **The item it documented is still in `after`, with no doc.** An item that was renamed or
///    removed took its name with it, and its doc going to the new name is the edit, not a theft.
///
/// # Errors
///
/// When either version cannot be read for its comments.
pub fn displaced(before: &str, after: &str) -> Result<Vec<Displacement>, String> {
    let was = Docs::read(before)?.attached;
    let Docs {
        attached: now,
        bare,
    } = Docs::read(after)?;
    let mut opens: BTreeMap<&str, Vec<(usize, usize)>> = BTreeMap::new();
    for (block, attachment) in now.iter().enumerate() {
        for (at, line) in attachment.doc.iter().enumerate() {
            opens.entry(line.as_str()).or_default().push((block, at));
        }
    }
    let mut found = Vec::new();
    for old in &was {
        let Some(first) = old.doc.first() else {
            continue;
        };
        let holders: Vec<&Attachment> = opens
            .get(first.as_str())
            .into_iter()
            .flatten()
            .filter(|(block, at)| now[*block].doc[*at..].starts_with(old.doc.as_slice()))
            .map(|(block, _)| &now[*block])
            .collect();
        if holders.iter().any(|held| held.key == old.key) || !bare.contains_key(&old.key) {
            continue;
        }
        let last_doc = old.doc_lines.last().copied().unwrap_or_default();
        // ⛔⛔⛔ Only a holder that did NOT already carry the block went anywhere — register item
        // 1091, off `crates/sprag-gui/src/view.rs`: `with_prompt` and `with_confirm` were written
        // word for word the same, `6d7c406e` put `with_keyhelp` between the second copy and its
        // item, and the FIRST holder is `with_prompt`, which had its copy all along. Named as where
        // the block went, the repair took `with_prompt`'s own doc off it and left the copy on
        // `with_keyhelp`. A block only the old carriers still hold was deleted, not moved.
        let landed = holders.iter().filter(|held| {
            !was.iter()
                .any(|kept| kept.key == held.key && carries(&kept.doc, &old.doc))
        });
        for holder in landed {
            let moved = Displacement {
                was: old.key.clone(),
                now: holder.key.clone(),
                line: holder.line,
                doc: old.doc.clone(),
                // ⚠ Only those AFTER the block: an attribute above a doc is not between the doc and
                // its item, and no declaration put there could take it.
                attributes: old
                    .attributes
                    .iter()
                    .filter(|attribute| attribute.lines.0 > last_doc)
                    .map(|attribute| attribute.text.clone())
                    .collect(),
            };
            if !found.contains(&moved) {
                found.push(moved);
            }
        }
    }
    Ok(found)
}

/// Whether `doc` holds `run` as consecutive lines — the same reading [`displaced`] gives a block
/// that survives inside a larger one.
fn carries(doc: &[String], run: &[String]) -> bool {
    !run.is_empty() && doc.windows(run.len()).any(|window| window == run)
}

/// Where a move still stands in one version of a file — [`Docs::standing`]'s answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Standing {
    /// The line of every item keyed as the one that took the block, whose doc still carries the
    /// block's opening.
    pub carried_by: Vec<usize>,
    /// Every undocumented item keyed as the one the block was written for.
    pub written_for: Vec<Bare>,
}

/// Every Rust file of one commit, to be asked where a move stands — register item 1091.
///
/// # ⛔⛔⛔ THE WHOLE TREE, AND NOT THE PATH THE MOVE WAS MADE IN
///
/// Measured before this was written: `809ae70c` moved a doc in `crates/sprag-gui/src/wire.rs`, and
/// `068f5774` deleted that file and carried its code into `crates/sprag-client/src/wire.rs` — a
/// delete and an add, which git does not call a rename. Asked by its path, that move could only ever
/// answer *could not ask*; and a file that kept its path while the code left it would answer *gone*
/// for a move standing one file over. A move is a fact about prose and two items, and the prose is
/// what is looked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeAt {
    /// The commit, as git spells it in full.
    pub rev: String,
    /// Every Rust file of the commit, as `(path, text)`, in git's order.
    files: Vec<(String, String)>,
}

impl TreeAt {
    /// Every Rust file of `rev` in `repo`.
    ///
    /// # Errors
    ///
    /// When `rev` names no commit, or its tree or a Rust file in it cannot be read as UTF-8 text.
    pub fn read(repo: &Path, rev: &str) -> Result<TreeAt, String> {
        let rev = resolved(repo, rev)?;
        let listing = git(repo, &["ls-tree", "-r", "-z", "--name-only", &rev])?
            .ok_or_else(|| format!("git could not list the files of {rev}"))?;
        let mut files = Vec::new();
        for path in listing.split('\0').filter(|path| path.ends_with(".rs")) {
            let text = git(repo, &["cat-file", "blob", &format!("{rev}:{path}")])?
                .ok_or_else(|| format!("git could not read {path} at {rev}"))?;
            files.push((path.to_owned(), text));
        }
        Ok(TreeAt { rev, files })
    }

    /// Every file in which `moved` still stands, with where — empty when it stands nowhere.
    ///
    /// ⚠ Only a file holding the block's opening is read for its docs: no other can carry it.
    ///
    /// # Errors
    ///
    /// When a file that holds the opening cannot be read for its comments. ⛔ Never skipped: that
    /// file is exactly the one that could answer *stands*.
    pub fn standing(&self, moved: &Displacement) -> Result<Vec<(&str, Standing)>, String> {
        let mut out = Vec::new();
        for (path, text) in &self.files {
            if !text.contains(moved.opening()) {
                continue;
            }
            let docs = Docs::read(text).map_err(|why| format!("{path} at {}: {why}", self.rev))?;
            if let Some(standing) = docs.standing(moved) {
                out.push((path.as_str(), standing));
            }
        }
        Ok(out)
    }
}

/// `text` with `moved` undone: the block taken out of the doc that carries it and put back directly
/// above the item it was written for — above that item's attributes, at that item's indentation —
/// register item 1091.
///
/// # ⛔⛔⛔ Refused rather than guessed
///
/// * **The block is not carried WHOLE, by exactly one doc of the item that took it.** Prose edited
///   since the move is somebody's to reread, and a block carried twice has no one place to be taken
///   from.
///
/// ⚠⚠ **ONLY FROM [`Displacement::now`], though [`Docs::standing`] asks of any other item.** A block
/// moved twice sits on its SECOND taker, and the move to put back first is that later one: newest
/// first, each link returns the block to the taker before it, and the older move then finds it on its
/// own `now`. Taking the older link first — straight off the second taker — would split the block
/// the newer move recorded, which then stands in the census with nothing whole left to put back. So
/// that order is refused here rather than half done.
/// * **The item it was written for is undocumented in more than one place, or in none.** Two `fn new`
///   in two impls, or a struct literal's `id:` line keyed like the field it fills: which of them the
///   prose describes is not in the text, so it is not this function's to pick.
/// * **An attribute that moved with the block is not right after it any more.** The attributes that
///   stood between the block and its item ([`Displacement::attributes`]) and that the item does not
///   carry now went with the block, and they go back with it — from directly after the moved lines,
///   in order. Found anywhere else, or not at all, they were edited since, and are somebody's to read.
///
/// ⚠ No landing is inside a string literal: [`Docs::read`] reads no item there — see `lines_of`.
///
/// # Errors
///
/// A sentence naming which of those it was, or that `text` could not be read.
pub fn repaired(text: &str, moved: &Displacement) -> Result<String, String> {
    let docs = Docs::read(text)?;
    let carriers: Vec<(&Attachment, usize)> = docs
        .attached
        .iter()
        .filter(|held| held.key == moved.now)
        .flat_map(|held| {
            (0..held.doc.len())
                .filter(|at| held.doc[*at..].starts_with(&moved.doc))
                .map(move |at| (held, at))
        })
        .collect();
    let [(carrier, at)] = carriers.as_slice() else {
        return Err(format!(
            "the block written for `{}` is carried whole by {} doc(s) of `{}` rather than by exactly \
             one — edited since, or moved again and owed back by that later move first: \"{}\"",
            moved.was,
            carriers.len(),
            moved.now,
            moved.opening(),
        ));
    };
    let landings = docs.bare.get(&moved.was).map_or(&[][..], Vec::as_slice);
    let [target] = landings else {
        return Err(format!(
            "`{}` is undocumented on {} line(s) {:?} rather than on exactly one, so which of them the \
             block was written for is not in the text: \"{}\"",
            moved.was,
            landings.len(),
            landings.iter().map(|bare| bare.line).collect::<Vec<_>>(),
            moved.opening(),
        ));
    };
    let first = carrier.doc_lines[*at];
    let last_doc = carrier.doc_lines[*at + moved.doc.len() - 1];
    // The attributes that went with the block: those it stood above that its item no longer carries.
    let mut went: Vec<&str> = moved.attributes.iter().map(String::as_str).collect();
    for kept in &target.attributes {
        if let Some(found) = went.iter().position(|text| *text == kept.text) {
            went.remove(found);
        }
    }
    let rest_of_carrier = carrier
        .doc_lines
        .get(*at + moved.doc.len())
        .copied()
        .unwrap_or(carrier.line);
    let after_block: Vec<&Attribute> = carrier
        .attributes
        .iter()
        .filter(|attribute| last_doc < attribute.lines.0 && attribute.lines.0 < rest_of_carrier)
        .collect();
    let carried = after_block.len() >= went.len()
        && after_block
            .iter()
            .zip(&went)
            .all(|(attribute, text)| attribute.text == *text);
    if !carried {
        return Err(format!(
            "`{}` stood under {went:?} when the block was written for it and does not carry it now, \
             but that is not what follows the block in the doc of `{}` — edited since, so where it \
             went is not in the text: \"{}\"",
            moved.was,
            moved.now,
            moved.opening(),
        ));
    }
    let last = went
        .len()
        .checked_sub(1)
        .map_or(last_doc, |at| after_block[at].lines.1);
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let from = indentation(lines[first - 1]);
    let to = indentation(lines[target.landing() - 1]);
    // ⚠ Each line trades the block's indentation for the item's, so a `#[cfg_attr(` that spans lines
    // keeps the shape inside it; a blank line stays blank rather than gaining trailing spaces.
    let block: Vec<String> = lines[first - 1..last]
        .iter()
        .map(|line| match (line.trim_start(), line.strip_prefix(from)) {
            ("", _) => "\n".to_owned(),
            (_, Some(rest)) => format!("{to}{rest}"),
            (rest, None) => format!("{to}{rest}"),
        })
        .collect();
    let mut out = String::with_capacity(text.len() + block.len() * to.len());
    for (index, line) in lines.iter().enumerate() {
        let number = index + 1;
        if number == target.landing() {
            out.extend(block.iter().map(String::as_str));
        }
        if !(first..=last).contains(&number) {
            out.push_str(line);
        }
    }
    Ok(out)
}

/// The whitespace `line` begins with.
fn indentation(line: &str) -> &str {
    &line[..line.len() - line.trim_start().len()]
}

/// What a change did to the doc blocks of the Rust files it touched — one commit against its first
/// parent ([`judge_commit`]), or the net change between two commits ([`judge_between`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeReading {
    /// The later side, as git spells it in full.
    pub tip: String,
    /// The earlier side, or [`None`] for a commit judged alone that has no parent.
    pub base: Option<String>,
    /// Every Rust file compared, by its path at the tip. A file ADDED or DELETED between the two
    /// sides has no second version to compare and is not here.
    pub compared: Vec<String>,
    /// Every displacement found, with the path it was found in.
    pub found: Vec<(String, Displacement)>,
}

/// The Rust files a `git diff-tree -r -M -z --name-status` output names as changed in place or
/// renamed, as `(path before, path after)`.
///
/// ⚠ A pure function over git's own bytes, so the two shapes that carry two paths are driven by a
/// case rather than by whatever the last commit happened to contain.
#[must_use]
pub fn changed_rust_files(name_status: &str) -> Vec<(String, String)> {
    let mut fields = name_status.split('\0').filter(|field| !field.is_empty());
    let mut out = Vec::new();
    while let Some(status) = fields.next() {
        let pair = match status.as_bytes().first() {
            Some(b'R' | b'C') => fields.next().zip(fields.next()),
            _ => fields.next().map(|path| (path, path)),
        };
        let Some((before, after)) = pair else {
            break;
        };
        if matches!(status.as_bytes().first(), Some(b'M' | b'R'))
            && before.ends_with(".rs")
            && after.ends_with(".rs")
        {
            out.push((before.to_owned(), after.to_owned()));
        }
    }
    out
}

/// `git` in `repo`, answered as its standard output, or [`None`] when it exited non-zero.
fn git(repo: &Path, args: &[&str]) -> Result<Option<String>, String> {
    let ran = git_in(repo).args(args).output().map_err(|why| {
        format!(
            "git could not be run in {} for {args:?}: {why}",
            repo.display()
        )
    })?;
    if !ran.status.success() {
        return Ok(None);
    }
    String::from_utf8(ran.stdout).map(Some).map_err(|_| {
        format!(
            "git answered {args:?} in {} with bytes that are not UTF-8",
            repo.display()
        )
    })
}

/// What `commit` in `repo` did to the doc blocks of every Rust file it changed, against its first
/// parent.
///
/// # ⛔⛔ A PARENT THIS CLONE DOES NOT HAVE IS *COULD NOT ASK*, NEVER *NOTHING CHANGED*
///
/// A shallow clone grafts its deepest commit, so that commit reads as having no parent — which is
/// also what a real root commit says. The two are told apart by asking git whether the clone is
/// shallow, and the shallow one is refused: answered as a root, every commit CI checks out at depth
/// one would be judged against nothing and pass.
///
/// # Errors
///
/// A sentence naming what could not be read: the commit, its parent in a shallow clone, a version
/// of a file, or a version that is not Rust this can scan.
pub fn judge_commit(repo: &Path, commit: &str) -> Result<ChangeReading, String> {
    let tip = resolved(repo, commit)?;
    let parent = git(
        repo,
        &["rev-parse", "--verify", "--quiet", &format!("{tip}^1")],
    )?
    .map(|sha| sha.trim().to_owned());
    let Some(parent) = parent else {
        let shallow = git(repo, &["rev-parse", "--is-shallow-repository"])?.unwrap_or_default();
        if shallow.trim() == "true" {
            return Err(format!(
                "{tip} has no parent in this clone because the clone is SHALLOW, so what it \
                 changed cannot be read — check it out with at least one commit of history behind it"
            ));
        }
        return Ok(ChangeReading {
            tip,
            base: None,
            compared: Vec::new(),
            found: Vec::new(),
        });
    };
    compared(repo, parent, tip)
}

/// The NET change from `base` to `tip`, judged exactly as [`judge_commit`] judges one commit —
/// register item 1088.
///
/// # ⚠⚠ Net, and that is the question it is for
///
/// A doc moved and put back inside the range is not in the answer; one moved inside it that still
/// stands at `tip` is. So two questions a single commit cannot put are answered here:
///
/// * `C^..HEAD`, for a commit `C` that moved a doc — **does that move still STAND?** The census this
///   module was measured on found displacements across the whole history, and which of them are
///   still in the tree is a question for the same predicate rather than for a person's eye.
/// * `origin/main..HEAD` — **what does a push publish?** Several commits, judged as the one change a
///   reader of the remote will see.
///
/// # Errors
///
/// When either side does not name a commit, or anything [`judge_commit`] refuses on.
pub fn judge_between(repo: &Path, base: &str, tip: &str) -> Result<ChangeReading, String> {
    let base = resolved(repo, base)?;
    let tip = resolved(repo, tip)?;
    compared(repo, base, tip)
}

/// `name` as the full id of the commit it names in `repo`.
fn resolved(repo: &Path, name: &str) -> Result<String, String> {
    git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{name}^{{commit}}"),
        ],
    )?
    .map(|sha| sha.trim().to_owned())
    .ok_or_else(|| format!("{name} does not name a commit in {}", repo.display()))
}

/// Every Rust file changed from `base` to `tip`, both versions read and compared.
fn compared(repo: &Path, base: String, tip: String) -> Result<ChangeReading, String> {
    let listing = git(
        repo,
        &[
            "diff-tree",
            "-r",
            "-M",
            "-z",
            "--name-status",
            "--no-commit-id",
            &base,
            &tip,
        ],
    )?
    .ok_or_else(|| format!("git could not list what changed from {base} to {tip}"))?;
    let mut compared = Vec::new();
    let mut found = Vec::new();
    for (was_at, now_at) in changed_rust_files(&listing) {
        let read = |rev: &str, path: &str| -> Result<String, String> {
            git(repo, &["cat-file", "blob", &format!("{rev}:{path}")])?
                .ok_or_else(|| format!("git could not read {path} at {rev}"))
        };
        let before = read(&base, &was_at)?;
        let after = read(&tip, &now_at)?;
        for moved in displaced(&before, &after).map_err(|why| format!("{now_at}: {why}"))? {
            found.push((now_at.clone(), moved));
        }
        compared.push(now_at);
    }
    Ok(ChangeReading {
        tip,
        base: Some(base),
        compared,
        found,
    })
}

impl Displacement {
    /// Whether this move puts `earlier` back: it carries the block `earlier` took, known by its
    /// opening, onto the item that block was written for — register item 1091.
    ///
    /// # ⛔⛔⛔ Why a move has to be asked this at all
    ///
    /// A block that was the WHOLE doc of the item that took it — a test declared under the doc of the
    /// test below it, with no doc of its own — goes back whole, and the change that puts it back reads
    /// exactly like a move: the block leaves an item, lands on another, and the item it left has no doc.
    /// [`displaced`] cannot tell the two apart, and no reading of one change can: measured on this
    /// repository's own repairs, six of 112 were refused by the gate that exists to stop the move they
    /// undo. What tells them apart is history — the block was written for the item it lands on, and an
    /// earlier commit took it off. So a move is asked against the moves before it.
    ///
    /// ⚠ The block is known by its OPENING and the item by its KEY, which is [`Docs::standing`]'s own
    /// identity for a move — not by the whole block, which may have been edited since, and not by the
    /// item that took it, which may have been moved on again.
    #[must_use]
    pub fn undoes(&self, earlier: &Displacement) -> bool {
        self.now == earlier.was && self.opening() == earlier.opening()
    }
}

/// Whether `older` is an ancestor of `newer` in `repo` — a commit counts as its own.
///
/// # Errors
///
/// When git cannot say, which is neither answer.
fn is_ancestor(repo: &Path, older: &str, newer: &str) -> Result<bool, String> {
    let ran = git_in(repo)
        .args(["merge-base", "--is-ancestor", older, newer])
        .output()
        .map_err(|why| {
            format!(
                "git could not be run in {} for merge-base: {why}",
                repo.display()
            )
        })?;
    match ran.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(format!(
            "git could not say whether {older} is an ancestor of {newer}: {}",
            String::from_utf8_lossy(&ran.stderr).trim()
        )),
    }
}

/// Every move `commit` made in `path`, against its first parent — none for a root, for a commit whose
/// parent this clone does not have, or for a commit where `path` did not exist on both sides.
///
/// # Errors
///
/// When either version cannot be read for its comments.
fn moves_in(repo: &Path, commit: &str, path: &str) -> Result<Vec<Displacement>, String> {
    let Some(before) = git(repo, &["cat-file", "blob", &format!("{commit}^1:{path}")])? else {
        return Ok(Vec::new());
    };
    let Some(after) = git(repo, &["cat-file", "blob", &format!("{commit}:{path}")])? else {
        return Ok(Vec::new());
    };
    displaced(&before, &after).map_err(|why| format!("{path} at {commit}: {why}"))
}

/// The commit in the history behind `before` whose move in `path` the change `repair` puts back —
/// [`Displacement::undoes`], asked of history — or [`None`] when it puts none back: the gate's
/// question for a move it found, register item 1091.
///
/// # ⚠⚠ Which commits are asked, in what order
///
/// The commits behind `before` that touched `path`, newest first. First those that changed how often
/// the NAME of the item the block leaves is spelled — the commit that declared it, which for a block
/// moved by a declaration is the commit that moved it — and then, when none of those is the answer,
/// every commit of the file. The first pass is an index into the second, never a replacement for it.
///
/// # ⛔⛔ A SHALLOW CLONE THAT FOUND NOTHING HAS NOT ANSWERED
///
/// The history that would say *put back* can be exactly the history the clone was not given. Answered
/// as *not a repair*, the gate would refuse a repair with the wrong sentence; answered as *a repair*, it
/// would pass a move nobody checked. So it is refused as unknown, and says why.
///
/// # Errors
///
/// When the history cannot be listed or a version in it read, or the clone is shallow and nothing was
/// found.
pub fn undone_in_history(
    repo: &Path,
    before: &str,
    path: &str,
    repair: &Displacement,
) -> Result<Option<String>, String> {
    let history = |pickaxe: Option<&str>| -> Result<Vec<String>, String> {
        let pickaxe = pickaxe.map(|name| format!("-S{name}"));
        let mut args = vec!["log", "--format=%H", "--no-merges"];
        args.extend(pickaxe.as_deref());
        args.extend([before, "--", path]);
        Ok(git(repo, &args)?
            .ok_or_else(|| format!("git could not list the history of {path} behind {before}"))?
            .lines()
            .map(str::to_owned)
            .collect())
    };
    let name = repair.was.rsplit([' ', ':']).next().unwrap_or_default();
    let mut asked = std::collections::BTreeSet::new();
    let first = if name.is_empty() {
        Vec::new()
    } else {
        history(Some(name))?
    };
    for commit in first {
        if asked.insert(commit.clone())
            && moves_in(repo, &commit, path)?
                .iter()
                .any(|earlier| repair.undoes(earlier))
        {
            return Ok(Some(commit));
        }
    }
    for commit in history(None)? {
        if asked.insert(commit.clone())
            && moves_in(repo, &commit, path)?
                .iter()
                .any(|earlier| repair.undoes(earlier))
        {
            return Ok(Some(commit));
        }
    }
    let shallow = git(repo, &["rev-parse", "--is-shallow-repository"])?.unwrap_or_default();
    if shallow.trim() == "true" {
        return Err(format!(
            "this clone is SHALLOW, so the history that would say whether the doc landing on `{}` \
             is being put back where it was written is not here — check it out with its whole history",
            repair.now,
        ));
    }
    Ok(None)
}

/// One move a [`Census`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// The commit that made it — for a range, its tip — as git spells it in full.
    pub commit: String,
    /// The file it was made in, by its path at that commit.
    pub path: String,
    /// The move.
    pub moved: Displacement,
    /// The earlier commit among those named whose move this one puts back
    /// ([`Displacement::undoes`]), or [`None`] for a move.
    pub puts_back: Option<String>,
    /// Where it stands in the tree the census was asked about — the files, empty for *gone* — or why
    /// that could not be asked. [`None`] when no tree was asked about, and for a move that puts one
    /// back, which is not a defect to look for.
    pub standing: Option<Result<Vec<(String, Standing)>, String>>,
}

/// What some commits did to docs, and — asked about a tree — what of it still stands there: the
/// census register items 1088 and 1091 are measured by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Census {
    /// How many commits or ranges were named.
    pub judged: usize,
    /// How many Rust file version pairs were compared.
    pub compared: usize,
    /// Every commit that could not be judged, with why.
    pub unreadable: Vec<(String, String)>,
    /// Every move found, in the order the commits were named.
    pub found: Vec<Found>,
    /// The tree asked about, as git spells it in full, when one was.
    pub at: Option<String>,
}

impl Census {
    /// How many of the commits named moved at least one doc. ⚠ A commit that only put one back
    /// counts: it is still a change in which prose left one item for another.
    #[must_use]
    pub fn moved_in(&self) -> usize {
        self.found
            .iter()
            .map(|found| found.commit.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .len()
    }

    /// How many of the moves found put an earlier one back.
    #[must_use]
    pub fn repairs(&self) -> usize {
        self.found
            .iter()
            .filter(|found| found.puts_back.is_some())
            .count()
    }

    /// How many moves still stand in the tree asked about.
    #[must_use]
    pub fn stands(&self) -> usize {
        self.found
            .iter()
            .filter(|found| matches!(&found.standing, Some(Ok(places)) if !places.is_empty()))
            .count()
    }

    /// How many moves could not be asked about.
    #[must_use]
    pub fn unasked(&self) -> usize {
        self.found
            .iter()
            .filter(|found| matches!(found.standing, Some(Err(_))))
            .count()
    }
}

/// The census of `commits` in `repo` — each judged against its first parent, or `A..B` as the net
/// change — and, when `standing_at` names a commit, where each move that puts no earlier one back
/// still stands in it.
///
/// # ⛔⛔⛔ A move that puts an earlier one back is not asked whether it stands — register item 1091
///
/// Otherwise the census could never reach zero: the repair of a block that was a whole doc is itself a
/// move to a reader of one change ([`Displacement::undoes`] says why), and it would stand in the very
/// tree it repaired. It is recognised among the commits NAMED, and only when the earlier commit is an
/// ANCESTOR of the later — so a whole history is judged alike in whatever order it is named, and a
/// commit is never taken for the repair of one made after it.
///
/// # Errors
///
/// When `standing_at` names no readable tree, or git cannot say whether one commit descends from
/// another.
pub fn census(
    repo: &Path,
    commits: &[String],
    standing_at: Option<&str>,
) -> Result<Census, String> {
    let tree = standing_at.map(|rev| TreeAt::read(repo, rev)).transpose()?;
    let mut pairs = 0;
    let mut unreadable = Vec::new();
    let mut found = Vec::new();
    for commit in commits {
        // ⚠ `A..B` is the NET change between two commits — see `judge_between` for the two questions
        // only a range can put. Anything else names one commit.
        let judged = match commit.split_once("..") {
            Some((base, tip)) => judge_between(repo, base, tip),
            None => judge_commit(repo, commit),
        };
        match judged {
            Ok(reading) => {
                pairs += reading.compared.len();
                for (path, moved) in reading.found {
                    found.push(Found {
                        commit: reading.tip.clone(),
                        path,
                        moved,
                        puts_back: None,
                        standing: None,
                    });
                }
            }
            Err(why) => unreadable.push((commit.clone(), why)),
        }
    }
    let mut puts_back = vec![None; found.len()];
    for (at, repair) in found.iter().enumerate() {
        for earlier in &found {
            if earlier.commit != repair.commit
                && earlier.path == repair.path
                && repair.moved.undoes(&earlier.moved)
                && is_ancestor(repo, &earlier.commit, &repair.commit)?
            {
                puts_back[at] = Some(earlier.commit.clone());
                break;
            }
        }
    }
    for (found, earlier) in found.iter_mut().zip(puts_back) {
        found.puts_back = earlier;
        if let (Some(tree), None) = (&tree, &found.puts_back) {
            found.standing = Some(tree.standing(&found.moved).map(|places| {
                places
                    .into_iter()
                    .map(|(path, place)| (path.to_owned(), place))
                    .collect()
            }));
        }
    }
    Ok(Census {
        judged: commits.len(),
        compared: pairs,
        unreadable,
        found,
        at: tree.map(|tree| tree.rev),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `crates/sprag-gate/src/north_star.rs` at `706c4019^`, lines 928–943: `Item` and its doc.
    const ITEM_BEFORE: &str = r##"            Says::Made(word) => write!(f, "{number} from {named} [{mark}] (made, `{word}`)"),
            Says::Met(word) => write!(f, "{number} from {named} [{mark}] (MET, `{word}`)"),
            Says::Neither => write!(f, "{number} from {named} [{mark}] (UNREAD)"),
        }
    }
}

/// One numbered item of section A, after its blocks have been grouped.
///
/// ⚠ A number can own several blocks: this ledger closes an item by laying a new block ON TOP of
/// the original rather than editing it. So the blocks are grouped by number and the TOPMOST mark
/// wins, which is the same rule a reader uses — the newest block is the current one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// The number the ledger files it under.
    pub number: u32,
"##;

    /// The same file at `706c4019`, lines 928–964: `Placed` declared between `Item`'s doc and `Item`.
    const ITEM_AFTER: &str = r##"            Says::Made(word) => write!(f, "{number} from {named} [{mark}] (made, `{word}`)"),
            Says::Met(word) => write!(f, "{number} from {named} [{mark}] (MET, `{word}`)"),
            Says::Neither => write!(f, "{number} from {named} [{mark}] (UNREAD)"),
        }
    }
}

/// One numbered item of section A, after its blocks have been grouped.
///
/// ⚠ A number can own several blocks: this ledger closes an item by laying a new block ON TOP of
/// the original rather than editing it. So the blocks are grouped by number and the TOPMOST mark
/// wins, which is the same rule a reader uses — the newest block is the current one.
/// One item in the derived work order, and the TERM that placed it there — register item 1052.
///
/// ⚠⚠ The `why` travels with the number rather than being recomputed by whoever prints it. An order
/// a reader cannot interrogate is the eye-choice it replaces with extra steps: rule 11's complaint
/// is not that the wrong item gets picked, it is that *그 판단이 어디에도 안 남는다*.
///
/// ⚠ Declared BELOW [`Item`]'s doc comment and above its derive would have silently taken that
/// derive — the compiler caught it as five conflicting impls on this type, which is the one shape
/// where inserting a struct between a doc comment and its attributes is not a formatting matter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placed {
    /// The register item.
    pub number: u32,
    /// Whether a declared override (a `critical` severity, or a standing red) placed it.
    pub critical: bool,
    /// Its chain depth, `None` when nobody wrote the chain down — see [`Reading::depth`].
    pub depth: Option<u32>,
    /// The term that put it here, in words a reader can check against the ledger.
    pub why: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// The number the ledger files it under.
    pub number: u32,
"##;

    /// `crates/sprag-host/src/plugins.rs` at `5a242872^`, lines 6673–6689: `folds_by_reason_json`.
    const FOLDS_BEFORE: &str = r##"    /// ⚠⚠ Everything else is typed because a durable log has named columns and needs values. A
    /// journal has exactly one reader — the row, which renders it with `step_to_json` — and
    /// `crate::runs::RunRegistry::persistable` deliberately keeps no journal at all. Decoding a
    /// rendered walk back into `StepRecord`s so the same function could render it again would be a
    /// second spelling of the walk with nothing asking for one.
    pub journal: Option<Vec<Value>>,
}

/// **THE SPLIT AS IT CROSSES THE WIRE** — one entry per reflect reason word, `{delivered, folded}`.
///
/// ⚠ Composed from `rows()` rather than from a list here, so [`sprag_plugin::ReflectReason::ALL`]
/// stays the only authority on which reasons there are — register item 856(1) and this workspace's
/// rule 6: a reason nobody classified must not quietly leave the table.
fn folds_by_reason_json(folds: sprag_plugin::FoldsByReason) -> Value {
    let mut out = serde_json::Map::new();
    for (reason, row) in folds.rows() {
        out.insert(
"##;

    /// The same file at `5a242872`, lines 6680–6731: two functions declared between that doc and
    /// its function.
    const FOLDS_AFTER: &str = r##"    /// ⚠⚠ Everything else is typed because a durable log has named columns and needs values. A
    /// journal has exactly one reader — the row, which renders it with `step_to_json` — and
    /// `crate::runs::RunRegistry::persistable` deliberately keeps no journal at all. Decoding a
    /// rendered walk back into `StepRecord`s so the same function could render it again would be a
    /// second spelling of the walk with nothing asking for one.
    pub journal: Option<Vec<Value>>,
}

/// **THE SPLIT AS IT CROSSES THE WIRE** — one entry per reflect reason word, `{delivered, folded}`.
///
/// ⚠ Composed from `rows()` rather than from a list here, so [`sprag_plugin::ReflectReason::ALL`]
/// stays the only authority on which reasons there are — register item 856(1) and this workspace's
/// rule 6: a reason nobody classified must not quietly leave the table.
/// **EVERY KIND OF CHECKER SILENCE WITH ITS COUNT**, keyed by the arm's own word — register item
/// 996, on [`folds_by_reason_json`]'s rule one function down.
///
/// ⚠ Every row travels, including the zeroes, for that function's reason: a kind that never
/// happened and a kind nobody counted are different facts, and a table that omitted its zeroes
/// would make them the same on the far side.
fn silent_by_kind_json(silent: sprag_plugin::judge::SilentByKind) -> Value {
    let mut out = serde_json::Map::new();
    for (kind, count) in silent.rows() {
        out.insert(kind.wire_str().to_owned(), json!(count));
    }
    Value::Object(out)
}

/// **THE SILENCES A REPORT COUNTED, BY KIND** — [`silent_by_kind_json`]'s reader, whole or nothing.
///
/// ⚠⚠ [`None`] when the key is absent or any row is unreadable, which is this block's rule and not
/// a local choice: a daemon too old to publish this cannot say what its silences WERE, and filling
/// in zeros would answer *none of them was the prompt's fault* on its behalf — the reassuring
/// reading of a number nobody measured. The caller refuses the whole tally, exactly as it does for
/// register item 499's and 674's keys.
fn silent_by_kind_in(tally: &Value) -> Option<sprag_plugin::judge::SilentByKind> {
    let table = tally.get("silent_by")?.as_object()?;
    let mut silent = sprag_plugin::judge::SilentByKind::NONE;
    for (word, count) in table {
        // ⚠ An unknown word refuses the table rather than being skipped: a newer daemon reporting a
        // fourth kind is one this build cannot total honestly, and a silent skip would publish a
        // sum smaller than the one that was counted.
        silent.restore(
            sprag_plugin::judge::Silence::named(word)?,
            small(Some(count))?,
        );
    }
    Some(silent)
}

fn folds_by_reason_json(folds: sprag_plugin::FoldsByReason) -> Value {
    let mut out = serde_json::Map::new();
    for (reason, row) in folds.rows() {
"##;

    /// `text` with the lines from the one starting `from` up to (not including) the one starting
    /// `to` moved to just before the line starting `before` — a repair built out of the real bytes.
    fn moved(text: &str, from: &str, to: &str, before: &str) -> String {
        let start = text
            .find(from)
            .expect("the block to move is in the fixture");
        let end = start + text[start..].find(to).expect("its end is in the fixture");
        let block = &text[start..end];
        let rest = format!("{}{}", &text[..start], &text[end..]);
        let at = rest
            .find(before)
            .expect("the landing line is in the fixture");
        format!("{}{block}{}", &rest[..at], &rest[at..])
    }

    /// ⛔⛔⛔⛔⛔ **THE TWO DISPLACEMENTS THAT LANDED ARE NAMED, EACH BY BOTH ITEMS** — register item
    /// 1088, off the bytes git holds for them.
    #[test]
    fn the_two_displacements_that_landed_on_this_repository_are_named() {
        assert_eq!(
            displaced(ITEM_BEFORE, ITEM_AFTER).expect("both versions read"),
            vec![Displacement {
                was: "struct Item".to_owned(),
                now: "struct Placed".to_owned(),
                line: 23,
                doc: vec![
                    "One numbered item of section A, after its blocks have been grouped.".to_owned(),
                    String::new(),
                    "⚠ A number can own several blocks: this ledger closes an item by laying a new \
                     block ON TOP of"
                        .to_owned(),
                    "the original rather than editing it. So the blocks are grouped by number and \
                     the TOPMOST mark"
                        .to_owned(),
                    "wins, which is the same rule a reader uses — the newest block is the current \
                     one."
                        .to_owned(),
                ],
                attributes: vec!["#[derive(Debug, Clone, PartialEq, Eq)]".to_owned()],
            }],
            "⛔⛔⛔⛔⛔ REGISTER ITEM 1088: `706c4019` put `Placed` between `Item` and its doc, and \
             every gate this repository has stayed green about it",
        );
        assert_eq!(
            displaced(FOLDS_BEFORE, FOLDS_AFTER).expect("both versions read"),
            vec![Displacement {
                was: "fn folds_by_reason_json".to_owned(),
                now: "fn silent_by_kind_json".to_owned(),
                line: 20,
                doc: vec![
                    "**THE SPLIT AS IT CROSSES THE WIRE** — one entry per reflect reason word, \
                     `{delivered, folded}`."
                        .to_owned(),
                    String::new(),
                    "⚠ Composed from `rows()` rather than from a list here, so \
                     [`sprag_plugin::ReflectReason::ALL`]"
                        .to_owned(),
                    "stays the only authority on which reasons there are — register item 856(1) \
                     and this workspace's"
                        .to_owned(),
                    "rule 6: a reason nobody classified must not quietly leave the table."
                        .to_owned(),
                ],
                attributes: vec![
                    // ⚠ none: `folds_by_reason_json` has no attribute to take
                ],
            }],
            "⛔⛔⛔⛔⛔ REGISTER ITEM 1088: `5a242872` put two functions between \
             `folds_by_reason_json` and its doc, and the doc stood on the wrong one for four days",
        );
    }

    /// ⚠⚠ **A DISPLACEMENT STANDS UNTIL ITS REPAIR, AND THE SAME READER SAYS WHICH** — the census's
    /// second question, register item 1088, off the real `Item` bytes.
    #[test]
    fn a_displacement_stands_until_its_block_is_back_on_its_item() {
        let displacement = displaced(ITEM_BEFORE, ITEM_AFTER)
            .expect("both versions read")
            .remove(0);
        let stands = |text: &str| {
            Docs::read(text)
                .expect("reads")
                .standing(&displacement)
                .is_some()
        };
        assert!(
            stands(ITEM_AFTER),
            "⛔⛔ at the commit that made it, the move stands: {displacement:?}",
        );
        let repaired = moved(
            ITEM_AFTER,
            "/// One item in the derived work order",
            "\n#[derive(Debug, Clone, PartialEq, Eq)]\npub struct Item {",
            "/// One numbered item of section A",
        );
        assert!(
            !stands(&repaired),
            "⚠⚠ THE CONTROL: once `Placed` is declared above the block, the same move no longer \
             stands — or every finding would read as standing for ever",
        );
        // ⚠⚠ AND A MOVE WHOSE TEXT WAS LATER DELETED DOES NOT STAND EITHER. `Item` is still bare here,
        // so this is the one arm that reaches the check on the moved TEXT rather than stopping at
        // the bare item — measured: with that check deleted, the control above stayed green. What
        // is left is a missing doc, which is not a moved one and not this question.
        let deleted = ITEM_AFTER.replace(
            "/// One numbered item of section A, after its blocks have been grouped.\n///\n/// ⚠ A \
             number can own several blocks: this ledger closes an item by laying a new block ON TOP \
             of\n/// the original rather than editing it. So the blocks are grouped by number and \
             the TOPMOST mark\n/// wins, which is the same rule a reader uses — the newest block is \
             the current one.\n",
            "",
        );
        assert_ne!(
            deleted, ITEM_AFTER,
            "⚠ the fixture must really have lost the moved text, or the arm below is about nothing",
        );
        assert!(
            !stands(&deleted),
            "⚠⚠ nothing carries the prose that was taken from `Item` any more, so no move stands — \
             `Item` having no doc is a different finding",
        );
    }

    /// ⚠⚠ **A STANDING MOVE NAMES WHERE IT STANDS** — register item 1091, off the real `Item` and
    /// `folds_by_reason_json` bytes: the item carrying the block, and the undocumented item it was
    /// written for with the line a doc for it goes above.
    #[test]
    fn a_standing_move_names_the_item_carrying_it_and_where_its_own_items_doc_goes() {
        let item = displaced(ITEM_BEFORE, ITEM_AFTER)
            .expect("both versions read")
            .remove(0);
        assert_eq!(
            Docs::read(ITEM_AFTER).expect("reads").standing(&item),
            Some(Standing {
                carried_by: vec![23],
                written_for: vec![Bare {
                    line: 35,
                    attributes: vec![Attribute {
                        lines: (34, 34),
                        text: "#[derive(Debug, Clone, PartialEq, Eq)]".to_owned(),
                    }],
                }],
            }),
            "⚠⚠ `Placed` on line 23 carries `Item`'s block, and `Item` begins on 35 under its derive \
             on 34 — the line its doc goes above",
        );
        // ⚠⚠ THE CONTROL FOR THE OTHER HALF: the block still sits on `Placed`, and `Item` has a doc of
        // its own again. What the block was written for is not bare, so no move stands — and without
        // this case, no case here decides the undocumented-item half of the question.
        let documented_again = ITEM_AFTER.replacen(
            "#[derive(Debug, Clone, PartialEq, Eq)]\npub struct Item {",
            "/// A doc of its own.\n#[derive(Debug, Clone, PartialEq, Eq)]\npub struct Item {",
            1,
        );
        assert_ne!(
            documented_again, ITEM_AFTER,
            "⚠ the fixture must really have gained the doc, or the arm below is about nothing",
        );
        assert_eq!(
            Docs::read(&documented_again)
                .expect("reads")
                .standing(&item),
            None,
            "⚠⚠ `Item` is documented again, so the block left on `Placed` is not a move that stands",
        );
        let folds = displaced(FOLDS_BEFORE, FOLDS_AFTER)
            .expect("both versions read")
            .remove(0);
        assert_eq!(
            Docs::read(FOLDS_AFTER).expect("reads").standing(&folds),
            Some(Standing {
                carried_by: vec![20],
                written_for: vec![Bare {
                    line: 50,
                    attributes: Vec::new(),
                }],
            }),
            "⚠ with no attribute above it, an item's doc goes above the item's own line",
        );
    }

    /// ⛔⛔⛔⛔ **EACH STANDING BLOCK GOES BACK ABOVE ITS OWN ITEM, AND NOTHING ELSE MOVES** — register
    /// item 1091, off the two displacements that landed here.
    ///
    /// ⚠⚠ The expected bytes are the fixture with the block cut out and pasted above its item, spelled
    /// by string surgery rather than by the function under test. The answer is then put to
    /// [`displaced`] against the version from BEFORE the move, which must find nothing.
    #[test]
    fn each_standing_block_goes_back_above_its_own_item_and_nothing_else_moves() {
        for (before, after, from, to, above) in [
            (
                ITEM_BEFORE,
                ITEM_AFTER,
                "/// One numbered item",
                "/// One item in the derived",
                "#[derive(Debug, Clone, PartialEq, Eq)]\npub struct Item {",
            ),
            (
                FOLDS_BEFORE,
                FOLDS_AFTER,
                "/// **THE SPLIT",
                "/// **EVERY KIND",
                "fn folds_by_reason_json",
            ),
        ] {
            let moved = displaced(before, after)
                .expect("both versions read")
                .remove(0);
            let block =
                &after[after.find(from).expect("the block")..after.find(to).expect("its end")];
            let cut = after.replacen(block, "", 1);
            let at = cut.find(above).expect("the item");
            let expected = format!("{}{block}{}", &cut[..at], &cut[at..]);
            let put_back = repaired(after, &moved)
                .expect("⛔⛔⛔ a block with one carrier and one undocumented item is put back");
            assert_eq!(
                put_back, expected,
                "⛔⛔⛔⛔ REGISTER ITEM 1091: the block goes back above `{}` and nothing else changes",
                moved.was,
            );
            assert_eq!(
                displaced(before, &put_back).expect("both read"),
                Vec::new(),
                "⚠⚠ against the version before the move, nothing has moved",
            );
            assert_eq!(
                Docs::read(&put_back).expect("reads").standing(&moved),
                None,
                "⚠ and the move stands no more",
            );
        }
    }

    /// ⚠⚠⚠ **THE BLOCK LANDS ABOVE ITS ITEM'S ATTRIBUTES, HOWEVER MANY LINES THEY TAKE, AT ITS ITEM'S
    /// INDENTATION** — register item 1091, and the shape most standing moves have: a test declared
    /// under the doc of the test below it.
    #[test]
    fn a_block_lands_above_its_items_attributes_at_its_items_indentation() {
        let before = "mod tests {\n    /// Checks the sum.\n    ///\n    /// Twice.\n    #[test]\n    \
                      #[cfg_attr(\n        miri,\n        ignore\n    )]\n    fn adds() {}\n}\n";
        let after = "mod tests {\n    /// Checks the sum.\n    ///\n    /// Twice.\n    #[test]\n    fn \
                     subtracts() {}\n\n    #[test]\n    #[cfg_attr(\n        miri,\n        ignore\n    \
                     )]\n    fn adds() {}\n}\n";
        let moved = displaced(before, after).expect("both read").remove(0);
        assert_eq!(
            repaired(after, &moved).expect("put back"),
            "mod tests {\n    #[test]\n    fn subtracts() {}\n\n    /// Checks the sum.\n    ///\n    \
             /// Twice.\n    #[test]\n    #[cfg_attr(\n        miri,\n        ignore\n    )]\n    fn \
             adds() {}\n}\n",
            "⚠⚠⚠ above `#[test]` and the four-line `cfg_attr`, never between them and `adds`",
        );
        let nested = "impl Alpha {\n    /// Makes one.\n    fn other() {}\n}\n\nfn make() {}\n";
        let carried = Displacement {
            was: "fn make".to_owned(),
            now: "fn other".to_owned(),
            line: 3,
            doc: vec!["Makes one.".to_owned()],
            attributes: Vec::new(),
        };
        assert_eq!(
            repaired(nested, &carried).expect("put back"),
            "impl Alpha {\n    fn other() {}\n}\n\n/// Makes one.\nfn make() {}\n",
            "⚠⚠ at `make`'s indentation, not at the one the block was carried at",
        );
    }

    /// ⛔⛔⛔ **A BLOCK MOVED TWICE STILL STANDS, AND NEWEST FIRST PUTS BOTH MOVES BACK** — register
    /// item 1091, the shape `786a1628` and `9346eb6e` left in `sprag-term.rs`: `put_back` declared
    /// under `spawn`'s doc, then `leftover` declared under the doc the two had become.
    #[test]
    fn a_block_moved_twice_still_stands_and_newest_first_puts_both_back() {
        let original = "/// Saves.\nfn spawn() {}\n";
        let first = "/// Saves.\n/// Puts back.\nfn put_back() {}\n\nfn spawn() {}\n";
        let second = "/// Saves.\n/// Puts back.\n/// Finds a leftover.\nfn leftover() {}\n\nfn \
                      put_back() {}\n\nfn spawn() {}\n";
        let older = displaced(original, first).expect("both read").remove(0);
        let newer = displaced(first, second).expect("both read").remove(0);
        assert_eq!(
            (older.was.as_str(), newer.was.as_str()),
            ("fn spawn", "fn put_back"),
            "⚠ the two moves of the chain, each named by its own commit",
        );
        let docs = Docs::read(second).expect("reads");
        assert!(
            docs.standing(&older).is_some(),
            "⛔⛔⛔ REGISTER ITEM 1091: `spawn` still has no doc, and its prose sits on `leftover` — the \
             move stands although `put_back`, the item that first took it, no longer carries it",
        );
        assert!(
            docs.standing(&newer).is_some(),
            "⚠ and the second move stands too"
        );
        let refused = repaired(second, &older).expect_err(
            "⛔⛔ the older move first would take `Saves.` out of the block the newer move recorded, \
             leaving that one standing with nothing whole to put back",
        );
        assert!(
            refused.contains("owed back by that later move first"),
            "⛔ and the refusal says which move goes first: {refused}",
        );
        let unwound = repaired(second, &newer).expect("the newer move is put back first");
        let unwound = repaired(&unwound, &older)
            .expect("⛔⛔ and the older one then finds its block on `put_back`, its own taker");
        assert_eq!(
            unwound,
            "/// Finds a leftover.\nfn leftover() {}\n\n/// Puts back.\nfn put_back() {}\n\n/// \
             Saves.\nfn spawn() {}\n",
            "⛔⛔⛔ each doc back on the item it was written for",
        );
        let read = Docs::read(&unwound).expect("reads");
        assert_eq!(
            (read.standing(&older), read.standing(&newer)),
            (None, None),
            "⚠ THE CONTROL: unwound, neither move stands",
        );
    }

    /// ⛔⛔⛔ **AN ATTRIBUTE THAT MOVED WITH THE DOC GOES BACK WITH IT, AND ONE THE ITEM KEPT DOES NOT** —
    /// register item 1091, the shape `865ead09` has: `find_in_line` declared between `cells_text`'s
    /// `#[must_use]` and `cells_text`, taking both. Here the attribute spans lines, so its inside
    /// shape is on trial too, and an attribute above the doc is the one that must stay where it is.
    #[test]
    fn an_attribute_that_moved_with_its_doc_goes_back_with_it() {
        let before = "#[allow(dead_code)]\n/// A cell row's text.\n#[cfg_attr(\n    unix,\n    \
                      must_use\n)]\nfn cells_text() {}\n";
        let after = "#[allow(dead_code)]\n/// A cell row's text.\n#[cfg_attr(\n    unix,\n    \
                     must_use\n)]\n/// Collect matches in one line.\nfn find_in_line() {}\n\nfn \
                     cells_text() {}\n";
        let moved = displaced(before, after).expect("both read").remove(0);
        assert_eq!(
            moved.attributes,
            vec!["#[cfg_attr(\nunix,\nmust_use\n)]".to_owned()],
            "⛔⛔ the attribute between the doc and `cells_text` is recorded, and the one above the \
             doc is not",
        );
        assert_eq!(
            repaired(after, &moved).expect("put back"),
            "#[allow(dead_code)]\n/// Collect matches in one line.\nfn find_in_line() {}\n\n/// A \
             cell row's text.\n#[cfg_attr(\n    unix,\n    must_use\n)]\nfn cells_text() {}\n",
            "⛔⛔⛔ REGISTER ITEM 1091: the doc AND the attribute it stood above go back to `cells_text`, \
             the attribute keeping its inside shape",
        );

        let kept = "#[allow(dead_code)]\n/// A cell row's text.\n#[cfg_attr(\n    unix,\n    \
                    must_use\n)]\n/// Collect matches in one line.\nfn find_in_line() {}\n\n\
                    #[cfg_attr(\n    unix,\n    must_use\n)]\nfn cells_text() {}\n";
        assert_eq!(
            repaired(kept, &moved).expect("put back"),
            "#[allow(dead_code)]\n#[cfg_attr(\n    unix,\n    must_use\n)]\n/// Collect matches in \
             one line.\nfn find_in_line() {}\n\n/// A cell row's text.\n#[cfg_attr(\n    unix,\n    \
             must_use\n)]\nfn cells_text() {}\n",
            "⚠⚠ THE CONTROL: `cells_text` still carries its attribute, so the one after the block is \
             `find_in_line`'s own and stays",
        );

        let replaced = "#[allow(dead_code)]\n/// A cell row's text.\n#[inline]\n/// Collect matches \
                        in one line.\nfn find_in_line() {}\n\nfn cells_text() {}\n";
        let refused = repaired(replaced, &moved).expect_err(
            "⛔⛔ the attribute that went with the block is not what follows it — edited since",
        );
        assert!(
            refused.contains("must_use"),
            "⛔ and the refusal names the attribute it could not find: {refused}",
        );
    }

    /// ⛔⛔⛔ **WHAT THE TEXT DOES NOT SAY IS REFUSED, NEVER PICKED** — register item 1091, each refusal
    /// beside the input it differs from, which is a repair.
    #[test]
    fn a_repair_the_text_cannot_decide_is_refused_and_says_why() {
        let makes = Displacement {
            was: "fn new".to_owned(),
            now: "fn other".to_owned(),
            line: 3,
            doc: vec!["Makes.".to_owned()],
            attributes: Vec::new(),
        };
        let one = "impl Alpha {\n    /// Makes.\n    fn other() {}\n    fn new() {}\n}\n";
        assert_eq!(
            repaired(one, &makes).expect("⚠ THE CONTROL: one carrier and one undocumented `new`"),
            "impl Alpha {\n    fn other() {}\n    /// Makes.\n    fn new() {}\n}\n",
        );

        let two = format!("{one}\nimpl Beta {{\n    fn new() {{}}\n}}\n");
        let refused = repaired(&two, &makes).expect_err(
            "⛔⛔⛔ two undocumented `new`, and which one the block is for is not in the text",
        );
        assert!(
            refused.contains("[4, 8]"),
            "⛔ and the refusal names both lines: {refused}",
        );

        // ⚠ The carrier holds TWO lines, so a reader that matched the opening alone would go on to
        // move them rather than index past a one-line doc — and be caught by this arm's own sentence.
        let longer = "impl Alpha {\n    /// Makes.\n    /// Differently.\n    fn other() {}\n    fn new() {}\n}\n";
        let edited = Displacement {
            doc: vec!["Makes.".to_owned(), "And more.".to_owned()],
            ..makes.clone()
        };
        let refused = repaired(longer, &edited)
            .expect_err("⛔⛔ the carrier holds the opening, but not the whole block that moved");
        assert!(
            refused.contains("by 0 doc(s)"),
            "⛔⛔ and the refusal says the block is not carried whole: {refused}",
        );

        // ⚠ A line of a string literal declares nothing, so the only `fn new` here is no item and
        // there is nowhere for the block to go — rather than a landing inside the fixture.
        let in_a_string =
            "const FIXTURE: &str = r#\"\nfn new() {}\n\"#;\n\n/// Makes.\nfn other() {}\n";
        let refused = repaired(in_a_string, &makes).expect_err(
            "⛔⛔⛔ the only `fn new` is a line of a string literal, and a block put there would be data",
        );
        assert!(
            refused.contains("on 0 line(s)"),
            "⛔ and the refusal says there is no such item to put it above: {refused}",
        );
    }

    /// ⛔⛔⛔⛔ **AND THE REPAIR OF EACH, MADE FROM THE SAME BYTES, IS GREEN** — the control that keeps
    /// the arm above from being a reader that reports every change.
    #[test]
    fn the_repair_of_each_displacement_is_not_one() {
        let item_repaired = moved(
            ITEM_AFTER,
            "/// One item in the derived work order",
            "\n#[derive(Debug, Clone, PartialEq, Eq)]\npub struct Item {",
            "/// One numbered item of section A",
        );
        assert_eq!(
            displaced(ITEM_BEFORE, &item_repaired).expect("both versions read"),
            Vec::new(),
            "⚠⚠ `Placed` declared ABOVE `Item`'s doc takes nothing, and a new documented item is the \
             ordinary change: {item_repaired}",
        );
        let folds_repaired = moved(
            FOLDS_AFTER,
            "/// **EVERY KIND OF CHECKER SILENCE",
            "fn folds_by_reason_json",
            "/// **THE SPLIT AS IT CROSSES THE WIRE**",
        );
        assert_eq!(
            displaced(FOLDS_BEFORE, &folds_repaired).expect("both versions read"),
            Vec::new(),
            "⚠⚠ the two functions declared above the doc they had taken: {folds_repaired}",
        );
        // ⚠ AND UNDOING A DISPLACEMENT IS NOT ONE EITHER: the glued block that stood on `Placed`
        // splits into two, and neither half is the whole block that `before` held.
        assert_eq!(
            displaced(ITEM_AFTER, &item_repaired).expect("both versions read"),
            Vec::new(),
            "⚠ the repair commit itself must not be refused",
        );
    }

    /// ⛔⛔⛔ **A NAME DOCUMENTED IN ONE PLACE AND BARE IN ANOTHER, LEFT WHERE IT WAS, DID NOT MOVE** —
    /// condition ⑵ of [`displaced`], and the one shape that reaches it.
    ///
    /// ⚠⚠ Everywhere else condition ⑶ answers first, because an item that kept its doc is never
    /// bare — so a mutation deleting ⑵ stayed green against every other case in this module. Two impls
    /// each with a `fn new`, one documented and one not, are where it is: without ⑵ an edit anywhere in
    /// the file would report the documented `new` as having moved onto itself.
    #[test]
    fn a_name_documented_in_one_place_and_bare_in_another_is_not_moved_by_an_unrelated_edit() {
        let before = "impl Alpha {\n    /// Makes an alpha.\n    fn new() {}\n}\n\nimpl Beta {\n    fn new() {}\n}\n";
        let after = "impl Alpha {\n    /// Makes an alpha.\n    fn new() {}\n}\n\nimpl Beta {\n    fn \
                     new() {}\n\n    fn other() {}\n}\n";
        assert_eq!(
            displaced(before, after).expect("both read"),
            Vec::new(),
            "⛔⛔⛔ the block still documents `fn new` — the same item it was written for — and a \
             second, undocumented `fn new` elsewhere in the file does not make it a move",
        );
    }

    /// ⚠⚠⚠ **A DOC THAT TRAVELS WITH ITS ITEM, AND A DOC WHOSE ITEM WAS RENAMED, DID NOT MOVE** —
    /// condition ⑶ of [`displaced`] driven by the rename, and the reorder as the ordinary change it
    /// has to leave alone. ⚠ Condition ⑵'s own arm is the case above: here ⑶ answers first.
    #[test]
    fn a_doc_that_moves_with_its_item_or_follows_a_rename_is_not_displaced() {
        let before =
            "/// Adds.\n///\n/// Second paragraph.\nfn add() {}\n\n/// Takes away.\nfn sub() {}\n";
        let reordered =
            "/// Takes away.\nfn sub() {}\n\n/// Adds.\n///\n/// Second paragraph.\nfn add() {}\n";
        assert_eq!(
            displaced(before, reordered).expect("both read"),
            Vec::new(),
            "⚠⚠ two items swapped, each carrying its own doc — condition ⑵",
        );
        let renamed =
            "/// Adds.\n///\n/// Second paragraph.\nfn plus() {}\n\n/// Takes away.\nfn sub() {}\n";
        assert_eq!(
            displaced(before, renamed).expect("both read"),
            Vec::new(),
            "⚠⚠ `add` renamed `plus`: its doc went with the name, and `add` is gone — condition ⑶",
        );
        let inserted_between = "/// Adds.\n///\n/// Second paragraph.\nfn mul() {}\nfn add() {}\n\n/// Takes away.\nfn sub() {}\n";
        assert_eq!(
            displaced(before, inserted_between).expect("both read"),
            vec![Displacement {
                was: "fn add".to_owned(),
                now: "fn mul".to_owned(),
                line: 4,
                doc: vec![
                    "Adds.".to_owned(),
                    String::new(),
                    "Second paragraph.".to_owned()
                ],
                attributes: Vec::new(),
            }],
            "⛔ THE CONTROL FOR BOTH: the same `before`, and an undocumented item put between the \
             doc and `add` — which is the displacement with no doc of its own to glue on",
        );
    }

    /// ⛔⛔⛔⛔ **PROSE WRITTEN TWICE WENT TO THE ITEM THAT GAINED IT, NOT TO THE ONE THAT HAD IT** —
    /// register item 1091, the shape of `crates/sprag-gui/src/view.rs` from `876576f2` (the same
    /// doc on `with_prompt` and `with_confirm`) through `6d7c406e` (`with_keyhelp` between the
    /// second copy and its item) to the repair that took the wrong copy.
    #[test]
    fn a_doc_written_twice_moved_to_the_item_that_gained_it_and_goes_back_from_there() {
        let twice = "/// Overlay the prompt.\nfn with_prompt() {}\n\n/// Overlay the prompt.\nfn \
                     with_confirm() {}\n";
        let keyhelp_between = "/// Overlay the prompt.\nfn with_prompt() {}\n\n/// Overlay the \
                               prompt.\n/// Overlay the key table.\nfn with_keyhelp() {}\n\nfn \
                               with_confirm() {}\n";
        let moved = Displacement {
            was: "fn with_confirm".to_owned(),
            now: "fn with_keyhelp".to_owned(),
            line: 6,
            doc: vec!["Overlay the prompt.".to_owned()],
            attributes: Vec::new(),
        };
        assert_eq!(
            displaced(twice, keyhelp_between).expect("both read"),
            vec![moved.clone()],
            "⛔⛔⛔ `with_prompt` carried its copy before the change — the block went to \
             `with_keyhelp`, the one holder that did not",
        );
        assert_eq!(
            repaired(keyhelp_between, &moved).expect("the repair is decidable"),
            "/// Overlay the prompt.\nfn with_prompt() {}\n\n/// Overlay the key table.\nfn \
             with_keyhelp() {}\n\n/// Overlay the prompt.\nfn with_confirm() {}\n",
            "⛔⛔ the copy comes off `with_keyhelp` and `with_prompt` keeps its own",
        );
        let wrong_copy_taken = "fn with_prompt() {}\n\n/// Overlay the prompt.\n/// Overlay the key \
                                table.\nfn with_keyhelp() {}\n\n/// Overlay the prompt.\nfn \
                                with_confirm() {}\n";
        assert_eq!(
            displaced(keyhelp_between, wrong_copy_taken).expect("both read"),
            vec![Displacement {
                was: "fn with_prompt".to_owned(),
                now: "fn with_confirm".to_owned(),
                line: 8,
                doc: vec!["Overlay the prompt.".to_owned()],
                attributes: Vec::new(),
            }],
            "⛔⛔ the repair that took `with_prompt`'s own copy is a move onto `with_confirm` — \
             `with_keyhelp` held that prose before it and still does",
        );
        let inside = "/// Overlay a name.\n/// Overlay the prompt.\nfn with_prompt() {}\n\n/// Overlay \
                      the prompt.\nfn with_confirm() {}\n";
        let inside_then_keyhelp = "/// Overlay a name.\n/// Overlay the prompt.\nfn with_prompt() \
                                   {}\n\n/// Overlay the prompt.\n/// Overlay the key table.\nfn \
                                   with_keyhelp() {}\n\nfn with_confirm() {}\n";
        assert_eq!(
            displaced(inside, inside_then_keyhelp).expect("both read"),
            vec![Displacement {
                line: 7,
                ..moved.clone()
            }],
            "⛔⛔ a copy held INSIDE a larger block was there all along too — carrying is a run \
             anywhere in the block, as a holder's is, not its opening",
        );
        let copy_deleted = "/// Overlay the prompt.\nfn with_prompt() {}\n\nfn with_confirm() {}\n";
        assert_eq!(
            displaced(twice, copy_deleted).expect("both read"),
            Vec::new(),
            "⚠⚠ THE CONTROL: one copy deleted and the other where it always was — nothing moved",
        );
    }

    /// ⚠⚠⚠ **A MULTI-PARAGRAPH DOC IS ONE BLOCK, AND A NEW ITEM ABOVE IT TAKES NOTHING** — register
    /// item 1088's done-when ⑵.
    #[test]
    fn a_new_documented_item_above_a_multi_paragraph_doc_takes_nothing() {
        let before = "/// **HEADLINE** — the first paragraph.\n///\n/// # A section\n///\n/// ⚠⚠ \
                      **ANOTHER BOLD PARAGRAPH**, as this repository writes them.\npub struct Kept;\n";
        let after = "/// **A NEW ITEM**, documented.\npub const NEW: u32 = 1;\n\n/// **HEADLINE** — \
                     the first paragraph.\n///\n/// # A section\n///\n/// ⚠⚠ **ANOTHER BOLD \
                     PARAGRAPH**, as this repository writes them.\npub struct Kept;\n";
        assert_eq!(
            displaced(before, after).expect("both read"),
            Vec::new(),
            "⚠⚠⚠ a doc with three paragraphs and two bold headlines is ordinary here — the shape a \
             reading of the tree would have had to call suspicious 435 times",
        );
    }

    /// ⛔⛔⛔ **`///` INSIDE A STRING IS DATA** — the reason this reads [`scan`]'s comments and not a
    /// line prefix. A test fixture holds exactly such lines, this module's own among them.
    #[test]
    fn doc_lines_inside_a_string_literal_are_not_doc_blocks() {
        let before = "const FIXTURE: &str = r#\"\n/// Written for one.\nfn one() {}\n\"#;\n";
        let after =
            "const FIXTURE: &str = r#\"\n/// Written for one.\nfn two() {}\nfn one() {}\n\"#;\n";
        assert_eq!(
            attachments(after).expect("reads"),
            Vec::new(),
            "⛔⛔⛔ a raw string's `///` lines are not a doc block, so there is nothing to attach",
        );
        assert_eq!(
            displaced(before, after).expect("both read"),
            Vec::new(),
            "⛔⛔ and a change inside that string moves no doc",
        );
    }

    /// ⚠⚠ **AN ATTRIBUTE BETWEEN A DOC AND ITS ITEM IS NOT THE ITEM, HOWEVER MANY LINES IT TAKES.**
    #[test]
    fn an_attribute_spanning_lines_does_not_end_a_docs_wait_for_its_item() {
        let text = "/// Doc.\n#[cfg_attr(\n    feature = \"x\",\n    derive(Debug)\n)]\n// a note\n\npub \
                    struct Held {\n    /// Its field.\n    pub(crate) inner: u8,\n}\n";
        assert_eq!(
            attachments(text).expect("reads"),
            vec![
                Attachment {
                    doc: vec!["Doc.".to_owned()],
                    doc_lines: vec![1],
                    key: "struct Held".to_owned(),
                    line: 8,
                    attributes: vec![Attribute {
                        lines: (2, 5),
                        text: "#[cfg_attr(\nfeature = \"x\",\nderive(Debug)\n)]".to_owned(),
                    }],
                },
                Attachment {
                    doc: vec!["Its field.".to_owned()],
                    doc_lines: vec![9],
                    key: "field Held::inner".to_owned(),
                    line: 10,
                    attributes: Vec::new(),
                },
            ],
            "⚠⚠ the doc reaches past a five-line attribute, a comment and a blank to the struct",
        );
    }

    /// ⛔⛔⛔ **A FIELD IS DECLARED INSIDE A STRUCT'S BRACES, AND A VARIANT INSIDE AN ENUM'S** —
    /// register item 1091. `id:` in a struct literal, `Row {` opening one and `Some(…)` in an
    /// expression are code, not undocumented items; a `}` in an attribute's string closes nothing,
    /// and the `;` of a bound's `[T; 4]` does not end the header it is in.
    #[test]
    fn a_field_or_a_variant_is_an_item_only_inside_the_braces_that_declare_it() {
        let text = "struct PaneRef {\n    info: PaneInfo,\n}\n\nimpl PaneRef {\n    /// The pane's host \
                    id.\n    fn id(&self) -> u64 {\n        self.info.id\n    }\n}\n\nfn row() -> Row {\n    \
                    Row {\n        id: 3,\n        name: String::new(),\n    }\n}\n\nstruct Wrapped<T>\n\
                    where\n    [T; 4]: Clone,\n{\n    #[serde(rename = \"}\")]\n    inner: T,\n    buf: \
                    [u8; 4],\n}\n\nenum Heard {\n    Nothing,\n    Said { words: String },\n}\n\nfn heard() \
                    -> Option<Heard> {\n    Some(Heard::Nothing)\n}\n";
        assert_eq!(
            Docs::read(text)
                .expect("reads")
                .bare
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec![
                "enum Heard",
                "field PaneRef::info",
                "field Wrapped::buf",
                "field Wrapped::inner",
                "fn heard",
                "fn row",
                "impl PaneRef",
                "struct PaneRef",
                "struct Wrapped",
                "variant Heard::Nothing",
                "variant Heard::Said",
            ],
            "⛔⛔⛔ REGISTER ITEM 1091: no `field id`, `field name`, `variant Row` or `variant Some` — \
             those lines are expressions, and every field and variant declared here is named",
        );
    }

    /// ⛔⛔⛔ **A FIELD GIVEN TO AN ACCESSOR WITH ITS DOC IS NOT A MOVE, THOUGH ANOTHER TYPE KEEPS A
    /// FIELD OF THAT NAME** — register item 1091, the shape `8c2c6817` has: `PaneRef`'s `id` became
    /// `fn id`, and `PaneInfo` still declares an undocumented `id` of its own.
    #[test]
    fn a_field_given_to_an_accessor_is_not_a_move_while_another_type_keeps_one_of_that_name() {
        let before = "struct PaneInfo {\n    id: u64,\n}\n\nstruct PaneRef {\n    /// The pane's host \
                      id.\n    id: u64,\n    info: PaneInfo,\n}\n";
        let accessor = "struct PaneInfo {\n    id: u64,\n}\n\nstruct PaneRef {\n    info: PaneInfo,\n}\n\n\
                        impl PaneRef {\n    /// The pane's host id.\n    fn id(&self) -> u64 {\n        \
                        self.info.id\n    }\n}\n";
        assert_eq!(
            displaced(before, accessor).expect("both read"),
            Vec::new(),
            "⛔⛔⛔ REGISTER ITEM 1091: `PaneRef::id` is gone, and `PaneInfo::id` was never what the \
             doc described",
        );
        let between = "struct PaneInfo {\n    id: u64,\n}\n\nstruct PaneRef {\n    /// The pane's host \
                       id.\n    info: PaneInfo,\n    id: u64,\n}\n";
        assert_eq!(
            displaced(before, between).expect("both read"),
            vec![Displacement {
                was: "field PaneRef::id".to_owned(),
                now: "field PaneRef::info".to_owned(),
                line: 7,
                doc: vec!["The pane's host id.".to_owned()],
                attributes: Vec::new(),
            }],
            "⚠⚠ THE CONTROL: `info` declared between the doc and `PaneRef`'s own `id`, which stays — \
             that is the move, and it is named through its owner",
        );
    }

    /// ⚠ **WHAT AN ITEM'S FIRST LINE NAMES IT** — every shape [`key_of`] tells apart.
    #[test]
    fn an_items_key_is_its_kind_and_its_name() {
        for (line, key) in [
            ("pub(crate) const fn spells(x: u8) -> u8 {", "fn spells"),
            ("pub unsafe extern \"C\" fn raw() {", "fn raw"),
            ("pub const RED: &str = \"@red:\";", "const RED"),
            ("static mut COUNT: u32 = 0;", "static COUNT"),
            ("pub enum Recurrence {", "enum Recurrence"),
            (
                "impl<T: Clone> fmt::Display for Wrapper<T> {",
                "impl <T: Clone> fmt::Display for Wrapper<T>",
            ),
            ("macro_rules! twice {", "macro_rules twice"),
            ("pub recurrence: Recurrence,", "field recurrence"),
            ("r#type: String,", "field r#type"),
            ("Intermittent,", "variant Intermittent"),
            ("UnrunnableRed {", "variant UnrunnableRed"),
            ("let value = 1;", "line let value = 1;"),
        ] {
            assert_eq!(key_of(line), key, "{line}");
        }
    }

    /// ⛔ **A SOURCE THE SCAN LOST ITS PLACE IN IS REFUSED**, never read as a file with no docs.
    #[test]
    fn a_source_that_runs_out_inside_a_literal_is_refused() {
        assert!(
            attachments("/// Doc.\nconst OPEN: &str = \"never closed;\n").is_err(),
            "⛔ read as prose, the rest of the file would carry no doc blocks and nothing could move",
        );
    }

    /// ⚠⚠ **THE TWO NAME-STATUS SHAPES THAT CARRY TWO PATHS** — a rename is compared across its
    /// paths, and a copy, an addition and a deletion have no before-and-after of one file.
    #[test]
    fn only_rust_files_changed_in_place_or_renamed_are_compared() {
        let listing = "M\0crates/a/src/lib.rs\0R087\0crates/a/src/old.rs\0crates/a/src/new.rs\0\
                       A\0crates/a/src/added.rs\0D\0crates/a/src/gone.rs\0C075\0crates/a/src/x.rs\0\
                       crates/a/src/copy.rs\0M\0README.md\0";
        assert_eq!(
            changed_rust_files(listing),
            vec![
                (
                    "crates/a/src/lib.rs".to_owned(),
                    "crates/a/src/lib.rs".to_owned()
                ),
                (
                    "crates/a/src/old.rs".to_owned(),
                    "crates/a/src/new.rs".to_owned()
                ),
            ],
            "⚠⚠ a copy's two paths must be consumed as two, or every later entry shifts by one",
        );
    }
}
