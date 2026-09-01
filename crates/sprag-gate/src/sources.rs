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

/// WHICH TREE a reader of this crate is about to walk, and whether it is the tree the running
/// invocation is standing in.
///
/// # ⛔⛔⛔⛔⛔ TWO TREES WERE WRITING ONE SENTENCE — register item 809
///
/// [`workspace_root`] answered `env!("CARGO_MANIFEST_DIR")/../..` and nothing else. That macro is
/// expanded when the **rlib is compiled**, so the path it carries is a fact about the build, not
/// about the run — and on a machine where one crate's build output can reach another tree, *the
/// tree this gate is judging* and *the tree somebody is running it in* stopped being the same
/// thing with nothing anywhere saying so.
///
/// ⚠⚠ MEASURED 2026-09-01 on this machine, and it is not a hypothetical: eight arms of
/// `cargo test -p sprag-gate` died at once with
/// `/tmp/sprag-check-2151720-916922480/crates/sprag-gate/../../crates/sprag-plugin/src/ai_loop.scxml:
/// No such file or directory`. `sprag-host`'s own checker cuts a `git worktree` under the temporary
/// directory (`crates/sprag-host/src/checkout.rs`), and a `sprag-gate` compiled THERE had answered
/// for a run HERE. Recompiling this crate from this tree — nothing else changed — turned all 28
/// targets green, which is what made the diagnosis a measurement rather than a story.
///
/// ⚠⚠⚠ THE LOUD FAILURE IS THE LUCKY HALF. A deleted worktree makes the path unreadable and the
/// gates shout. A worktree that still EXISTS makes them read a real tree that is somebody else's,
/// silently, and report green about a workspace nobody asked them to judge.
///
/// ⚠ The running tree is found by walking UP from the process's own directory to the nearest
/// `Cargo.toml` that declares `[workspace]` — a fact about the invocation that no compile can bake
/// in. Under `cargo test` the process starts in its package's root, which is inside the workspace
/// being tested; nothing in this repository moves it (measured: zero `set_current_dir` in any
/// crate).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeUnderTest {
    /// The tree this crate was COMPILED in is the tree this run is standing in. The only state in
    /// which a walk of it is a claim about the workspace the runner meant.
    Agreed(PathBuf),
    /// They are two different trees. Whatever a walk found is about `compiled_in`, and whoever
    /// started this run is in `running_in`.
    Skewed {
        /// Where `env!("CARGO_MANIFEST_DIR")` says this crate's source is.
        compiled_in: PathBuf,
        /// The workspace the running process is actually standing in.
        running_in: PathBuf,
    },
    /// The running tree could not be identified. ⚠ NOT a pass: a root that cannot be checked is
    /// exactly the state item 809 is about, and answering it quietly is the defect.
    Unknown {
        /// Where `env!("CARGO_MANIFEST_DIR")` says this crate's source is.
        compiled_in: PathBuf,
        /// What stopped the walk from naming the running workspace.
        why: String,
    },
}

/// The root this crate was compiled against — `crates/sprag-gate/` is two levels down from it.
///
/// ⚠ Raw and unchecked ON PURPOSE, and not public: it is one of the two halves
/// [`tree_under_test`] compares, and a caller that wanted "the root" and got this would be back to
/// the single silent answer item 809 removed.
fn compiled_in_root() -> PathBuf {
    [env!("CARGO_MANIFEST_DIR"), "..", ".."].iter().collect()
}

/// The workspace the RUNNING process is standing in: the nearest ancestor of its own directory
/// whose `Cargo.toml` declares `[workspace]`.
///
/// ⚠ `[workspace]` rather than the mere presence of a `Cargo.toml`, because every crate directory
/// has one of those and the first hit walking up would be the package, not the workspace. Measured
/// in this repository: exactly one manifest declares it, and no crate manifest does.
fn running_in_root() -> Result<PathBuf, String> {
    let here = std::env::current_dir().map_err(|why| format!("no current directory: {why}"))?;
    let mut at = here.as_path();
    loop {
        let manifest = at.join("Cargo.toml");
        if let Ok(text) = std::fs::read_to_string(&manifest)
            && text.lines().any(|line| line.trim_end() == "[workspace]")
        {
            return Ok(at.to_path_buf());
        }
        at = match at.parent() {
            Some(up) => up,
            None => {
                return Err(format!(
                    "no ancestor of {} declares [workspace]",
                    here.display()
                ));
            }
        };
    }
}

/// Whether the tree this crate judges is the tree it is being run in — register item 809.
#[must_use]
pub fn tree_under_test() -> TreeUnderTest {
    verdict_of(compiled_in_root(), running_in_root())
}

/// [`tree_under_test`]'s policy with both answers injected — register item 809.
///
/// ⚠⚠ PUBLIC BECAUSE THE SKEW IS NOT A STATE A MACHINE PRODUCES ON DEMAND. A gate can only drive
/// it by being handed the two answers, which is `sprag_scratch`'s `root_from` split and item 802's
/// `xdg_home_from` split applied to the question of which TREE is being judged. A policy that can
/// only be exercised by breaking a build is a policy nobody exercises.
///
/// ⚠ Both paths are canonicalised before they are compared: one is built by joining `".."` twice
/// and the other by walking up, so two spellings of one directory would otherwise read as two
/// trees. Item 804 paid for the opposite mistake in the same family — a comparison that folded a
/// symlink's two paths together — so the direction is chosen rather than inherited.
#[must_use]
pub fn verdict_of(compiled_in: PathBuf, running_in: Result<PathBuf, String>) -> TreeUnderTest {
    let running_in = match running_in {
        Ok(root) => root,
        Err(why) => return TreeUnderTest::Unknown { compiled_in, why },
    };
    let (Ok(built), Ok(run)) = (compiled_in.canonicalize(), running_in.canonicalize()) else {
        return TreeUnderTest::Unknown {
            compiled_in,
            why: format!(
                "{} cannot be resolved on this filesystem",
                running_in.display()
            ),
        };
    };
    if built == run {
        TreeUnderTest::Agreed(built)
    } else {
        TreeUnderTest::Skewed {
            compiled_in: built,
            running_in: run,
        }
    }
}

/// The sentence a reader is owed when the two trees are not one — register item 809.
///
/// ⚠ It names BOTH, because *your gate read the wrong tree* without saying which two leaves the
/// reader with nothing to act on; and it names the repair, because the repair is not obvious
/// (nothing in the source changed, so the ordinary instinct is to look for a source bug).
#[must_use]
pub fn tree_skew_sentence(verdict: &TreeUnderTest) -> String {
    match verdict {
        TreeUnderTest::Agreed(root) => {
            format!("the tree under test is {}", root.display())
        }
        TreeUnderTest::Skewed {
            compiled_in,
            running_in,
        } => format!(
            "⛔ REGISTER ITEM 809: this gate would judge {}, but the run is standing in {}. \
             `sprag-gate` was compiled against another tree, so every walk it does is about that \
             one. Nothing in the source is wrong. Recompile this crate from here -- \
             `find crates/sprag-gate/src -name '*.rs' -exec touch {{}} +` then re-run -- and if it \
             comes back, the build output of two trees is reaching one place.",
            compiled_in.display(),
            running_in.display(),
        ),
        TreeUnderTest::Unknown { compiled_in, why } => format!(
            "⛔ REGISTER ITEM 809: this gate would judge {}, and which tree the run is in cannot \
             be established: {why}. An unidentified root is refused rather than assumed -- a walk \
             nobody can attribute is not a claim about this workspace.",
            compiled_in.display(),
        ),
    }
}

/// The workspace root — `crates/sprag-gate/` is two levels down from it.
///
/// # Panics
///
/// When the tree this crate was compiled against is not the tree the run is standing in, or when
/// the running tree cannot be named at all — register item 809, and [`tree_under_test`] carries
/// the whole argument. A gate that answered anyway would be judging somebody else's workspace.
#[must_use]
pub fn workspace_root() -> PathBuf {
    let verdict = tree_under_test();
    match verdict {
        TreeUnderTest::Agreed(root) => root,
        _ => panic!("{}", tree_skew_sentence(&verdict)),
    }
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

    let (mut sources, declared): (Vec<Source>, Vec<Vec<String>>) = paths
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
            // ⚠⚠⚠ READ OFF THE RAW TEXT AND NOT OFF `code`, AND THE FIRST DRAFT GOT THIS WRONG:
            // `code` drops every line starting with `#`, so the attribute this looks for is exactly
            // what it had already thrown away. The gate went green measuring nothing.
            let modules = test_only_modules(&file, &text);
            (
                Source {
                    file,
                    code,
                    product,
                },
                modules,
            )
        })
        .unzip();

    // ⚠⚠⚠⚠⚠ **AND A WHOLE FILE CAN BE TEST-ONLY, WHICH THE PER-FILE SPLIT ABOVE CANNOT SEE.**
    // `proving_lines` reads one file at a time, so a `#[cfg(test)]` that governs a module lives in
    // a DIFFERENT file from the code it excludes — and every line of that module then counts as
    // shipping. Measured 2026-08-26: `sprag-host/src/live_agent.rs` is declared
    // `#[cfg(test)] mod live_agent;` in `lib.rs`, and **fourteen of the twenty-four sites item
    // 470's driver ratchet was pinning were assertions inside it**. The ratchet was counting its
    // own gates.
    //
    // ⚠⚠ THE FAILURE MODE THAT MAKES THIS WORTH FIXING RATHER THAN NOTING is not the overcount. It
    // is that a per-state pin satisfied by a mixture of driver arms and test assertions can be held
    // FLAT while a real arm appears, because a gate deleted in the same commit pays for it. An
    // overstated ratchet is not a strict one.
    let proving: std::collections::BTreeSet<String> = declared.into_iter().flatten().collect();
    for source in &mut sources {
        if proving.contains(&source.file) {
            source.product.clear();
        }
    }
    sources
}

/// Every file the source at `file` declares as a module that exists ONLY under `#[cfg(test)]`,
/// workspace-relative, in both spellings Rust allows (`name.rs` and `name/mod.rs`).
///
/// ⚠ Both are answered without asking the filesystem, because a name that resolves to neither is
/// simply in nobody's set: the caller matches against the files the walk actually found.
///
/// ⚠⚠ `text` is the RAW file. It cannot be [`Source::code`]: that drops every line starting with
/// `#`, which is the attribute this is looking for.
fn test_only_modules(file: &str, text: &str) -> Vec<String> {
    let Some(dir) = file.rsplit_once('/').map(|(head, _)| head) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    let mut governed = false;
    for raw in text.lines() {
        let line = raw.trim();
        if attribute_is_cfg_test(line).is_some_and(str::is_empty) {
            governed = true;
            continue;
        }
        if !governed {
            continue;
        }
        // Between the attribute and its item there may be more attributes, doc comments or nothing
        // at all — none of those is the item, so none of them ends the wait.
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            continue;
        }
        governed = false;
        let named = line
            .strip_prefix("pub ")
            .unwrap_or(line)
            .strip_prefix("mod ")
            .and_then(|rest| rest.strip_suffix(';'))
            .map(str::trim);
        if let Some(named) = named.filter(|it| !it.is_empty()) {
            found.push(format!("{dir}/{named}.rs"));
            found.push(format!("{dir}/{named}/mod.rs"));
        }
    }
    found
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
    ///
    /// # ⚠⚠⚠⚠⚠ Why the second arm is a BOUNDARY and no longer a RATIO
    ///
    /// It read `product > code / 4` — *and it must not have swallowed the file*. That is a proxy,
    /// and it was wrong in both directions.
    ///
    /// It decayed as this workspace was TESTED. [`Source::product`]'s own doc holds the finding:
    /// a ratchet that counts test code punishes testing, which is backwards (register item 470,
    /// measured). This arm was one. Measured 2026-08-29: `outer.rs` stood at 25.16 % of its own
    /// lines — about twenty of margin over the floor — and the round that added register item 746's
    /// two outage gates spent every one of them. A gate that goes red on the round that adds a gate
    /// is not guarding the split; it is charging rent on it.
    ///
    /// ⚠⚠ And it was BLIND in the direction that actually costs something. A reader whose items
    /// close too early leaves the test module in the shipping half — every ratchet built on the
    /// split then counts its own gates, which is item 470's finding all over again — and a ratio
    /// FLOOR gets greener the worse that gets. The boundary below is red for it: the last shipped
    /// line has to be the last code line before the test module, so a split that stops early
    /// (swallowing what ships) or late (keeping what proves) has moved it either way.
    ///
    /// ⚠ What this arm does NOT claim: that the reader is right about every shape. That is the
    /// table above, and [`an_unclosed_test_item_is_refused_rather_than_swallowing_the_file`] is the
    /// arm for a reader that loses its place outright. This one asks only whether the shapes those
    /// two cover are the shapes the real file turned out to have.
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

        // ⚠ THE MODULE'S OWN LINE AND NOT ITS ATTRIBUTE, because `code` drops every line starting
        // with `#` — the same trap `test_only_modules` is commented for one screen up.
        let (module, _) = driver
            .code
            .iter()
            .rev()
            .find(|(_, text)| text.as_str() == "mod tests {")
            .expect("the driver's test module is what makes it the file this gate is measured on");
        let (last_shipped, _) = driver
            .code
            .iter()
            .rev()
            .find(|(line, _)| line < module)
            .expect("the driver ships code before its own test module");
        assert_eq!(
            driver.product.last().map(|(line, _)| *line),
            Some(*last_shipped),
            "the shipping half must run to the test module and stop there. It ends at {:?} and \
             the last code line before `mod tests {{` (line {module}) is {last_shipped} — so the \
             split moved the boundary: earlier means it swallowed code that SHIPS, later means it \
             kept code that PROVES and every ratchet built on this is now counting its own gates",
            driver.product.last().map(|(line, _)| *line),
        );
    }
}
