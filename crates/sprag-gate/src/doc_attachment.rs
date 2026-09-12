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
//! # ⚠⚠ THE RESIDUE, STATED RATHER THAN HIDDEN
//!
//! * **An item is keyed by what its first line names** — `fn name`, `struct Name`, `field name` —
//!   and not by a nesting this does not parse. So a RENAME whose old name survives elsewhere in the
//!   file on an undocumented line can read as a move. How often that happens is a measurement, and
//!   it was taken over this repository's whole history before this became a gate.
//! * **A block the same change also rewrote is not followed.** The moved block is looked for whole;
//!   a displacement that edits the text it moved is not seen.
//! * **Lines inside a multi-line string literal are read as code.** They can name an item that is
//!   not there, which only matters to the rename case above.

use crate::ambient::git_in;
use crate::rust_source::{Shape, scan};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// One outer doc block, and the item it documents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    /// The block's lines with `///` and the whitespace around each removed, in order. A bare `///`
    /// is an empty line here, because a paragraph break is part of what was written.
    pub doc: Vec<String>,
    /// What the block documents, as [`key_of`] names it.
    pub key: String,
    /// The one-indexed line the documented item begins on.
    pub line: usize,
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
    /// The block's first line, so a reader can find the prose that moved.
    pub opening: String,
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

/// Every line of `text`, classified.
///
/// ⚠⚠ THE COMMENTS ARE [`scan`]'s AND NOT A LINE PREFIX — register item 1051's rule. A fixture in a
/// raw string holds `///` lines that are not comments at all, and a reader that trusted the prefix
/// would find doc blocks inside a test's data.
///
/// # Errors
///
/// When the scan ran out inside a literal or a comment: the rest of the file could not be told apart
/// into code and prose, and a reading of it would be a guess.
fn lines_of(text: &str) -> Result<Vec<Line<'_>>, String> {
    let scanned = scan(text);
    if let Some(unclosed) = scanned.unclosed {
        return Err(format!(
            "the source runs out inside an unclosed {unclosed:?}, so its doc blocks cannot be told \
             from its code"
        ));
    }
    let mut out = Vec::new();
    let mut offset = 0;
    for raw in text.split_inclusive('\n') {
        let body = raw.trim_end_matches(['\n', '\r']);
        let trimmed = body.trim_start();
        let start = offset + (body.len() - trimmed.len());
        offset += raw.len();
        let trimmed = trimmed.trim_end();
        if trimmed.is_empty() {
            out.push(Line::Quiet);
            continue;
        }
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
        match covering {
            Some(comment) if comment.shape == Shape::WholeLine && comment.at == start => {
                match text[comment.at..comment.end].strip_prefix("///") {
                    // ⚠ `////` is an ordinary comment, which is Rust's own rule.
                    Some(rest) if !rest.starts_with('/') => out.push(Line::Doc(rest.trim())),
                    _ => out.push(Line::Quiet),
                }
            }
            Some(_) => out.push(Line::Quiet),
            None if trimmed.starts_with("#[") => out.push(Line::Attribute(trimmed)),
            None => out.push(Line::Code(trimmed)),
        }
    }
    Ok(out)
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

/// Every doc block of `text` with the item it documents, and the key of every item line that
/// carries no doc.
fn walk(text: &str) -> Result<(Vec<Attachment>, BTreeSet<String>), String> {
    let mut attached = Vec::new();
    let mut bare = BTreeSet::new();
    let mut pending: Vec<String> = Vec::new();
    let mut open_attribute = 0;
    for (index, line) in lines_of(text)?.into_iter().enumerate() {
        if open_attribute > 0 {
            if let Line::Code(code) | Line::Attribute(code) = line {
                open_attribute = brackets_open_after(open_attribute, code);
            }
            continue;
        }
        match line {
            Line::Doc(doc) => pending.push(doc.to_owned()),
            Line::Quiet => {}
            Line::Attribute(code) => open_attribute = brackets_open_after(0, code),
            Line::Code(code) => {
                let key = key_of(code);
                if pending.is_empty() {
                    if names_an_item(&key) {
                        bare.insert(key);
                    }
                } else {
                    attached.push(Attachment {
                        doc: std::mem::take(&mut pending),
                        key,
                        line: index + 1,
                    });
                }
            }
        }
    }
    Ok((attached, bare))
}

/// Every doc block of `text`, and the item each one documents.
///
/// # Errors
///
/// When the source cannot be read for its comments — see the reader's own refusal.
pub fn attachments(text: &str) -> Result<Vec<Attachment>, String> {
    Ok(walk(text)?.0)
}

/// The item keywords [`key_of`] names an item by, in the order it tries them.
const ITEM_KEYWORDS: [&str; 9] = [
    "fn", "struct", "enum", "union", "trait", "type", "const", "static", "mod",
];

/// Whether `key` names an item, a field or a variant rather than an arbitrary line.
fn names_an_item(key: &str) -> bool {
    ITEM_KEYWORDS
        .iter()
        .chain(["impl", "macro_rules", "field", "variant"].iter())
        .any(|kind| key.starts_with(kind) && key[kind.len()..].starts_with(' '))
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
///    or that another item also carries word for word, is still where it was written.
/// 3. **The item it documented is still in `after`, with no doc.** An item that was renamed or
///    removed took its name with it, and its doc going to the new name is the edit, not a theft.
///
/// # Errors
///
/// When either version cannot be read for its comments.
pub fn displaced(before: &str, after: &str) -> Result<Vec<Displacement>, String> {
    let (was, _) = walk(before)?;
    let (now, bare) = walk(after)?;
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
        let Some(holder) = holders.first() else {
            continue;
        };
        if holders.iter().any(|held| held.key == old.key) || !bare.contains(&old.key) {
            continue;
        }
        let moved = Displacement {
            was: old.key.clone(),
            now: holder.key.clone(),
            line: holder.line,
            opening: first.clone(),
        };
        if !found.contains(&moved) {
            found.push(moved);
        }
    }
    Ok(found)
}

/// Whether a displacement found in some change still stands in `text`, a later version of the same
/// file: the moved text still sits in a block documenting the item that took it, and the item it was
/// written for is still there with no doc — register item 1088's census question.
///
/// ⚠⚠ THE SAME READER AS [`displaced`], so *this commit moved a doc* and *that move is still in the
/// tree* are one author's two answers rather than a second parser over the first one's printout.
///
/// # Errors
///
/// When `text` cannot be read for its comments.
pub fn still_stands(text: &str, moved: &Displacement) -> Result<bool, String> {
    let (attached, bare) = walk(text)?;
    Ok(bare.contains(&moved.was)
        && attached
            .iter()
            .any(|held| held.key == moved.now && held.doc.contains(&moved.opening)))
}

/// [`still_stands`], asked of `path` as it is at `rev` in `repo`.
///
/// # Errors
///
/// When `path` is not in `rev` — a file renamed or deleted since cannot say whether a move in it
/// stands, and *gone* would be a guess — or when that version cannot be read.
pub fn stands_at(repo: &Path, rev: &str, path: &str, moved: &Displacement) -> Result<bool, String> {
    let text = git(repo, &["cat-file", "blob", &format!("{rev}:{path}")])?
        .ok_or_else(|| format!("{path} is not in {rev}"))?;
    still_stands(&text, moved).map_err(|why| format!("{path} at {rev}: {why}"))
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
                opening: "One numbered item of section A, after its blocks have been grouped."
                    .to_owned(),
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
                opening: "**THE SPLIT AS IT CROSSES THE WIRE** — one entry per reflect reason \
                          word, `{delivered, folded}`."
                    .to_owned(),
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
        assert!(
            still_stands(ITEM_AFTER, &displacement).expect("reads"),
            "⛔⛔ at the commit that made it, the move stands: {displacement:?}",
        );
        let repaired = moved(
            ITEM_AFTER,
            "/// One item in the derived work order",
            "\n#[derive(Debug, Clone, PartialEq, Eq)]\npub struct Item {",
            "/// One numbered item of section A",
        );
        assert!(
            !still_stands(&repaired, &displacement).expect("reads"),
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
            !still_stands(&deleted, &displacement).expect("reads"),
            "⚠⚠ nothing carries the prose that was taken from `Item` any more, so no move stands — \
             `Item` having no doc is a different finding",
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
                opening: "Adds.".to_owned(),
            }],
            "⛔ THE CONTROL FOR BOTH: the same `before`, and an undocumented item put between the \
             doc and `add` — which is the displacement with no doc of its own to glue on",
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
                    key: "struct Held".to_owned(),
                    line: 8,
                },
                Attachment {
                    doc: vec!["Its field.".to_owned()],
                    key: "field inner".to_owned(),
                    line: 10,
                },
            ],
            "⚠⚠ the doc reaches past a five-line attribute, a comment and a blank to the struct",
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
