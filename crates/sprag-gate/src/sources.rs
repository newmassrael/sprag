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
            let code = text
                .lines()
                .enumerate()
                .map(|(index, line)| (index + 1, line.trim().to_owned()))
                .filter(|(_, line)| !line.starts_with("//") && !line.starts_with('#'))
                .collect();
            Source { file, code }
        })
        .collect()
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
}
