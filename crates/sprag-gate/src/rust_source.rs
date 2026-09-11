//! What a comment is in RUST source, spelled once — register item 1051.
//!
//! # ⛔⛔⛔⛔⛔ Eleven copies of one approximation, and not one of them measured
//!
//! Every gate in this workspace that wants *what the code says, rather than what its prose says
//! about itself* writes `line.trim_start().starts_with("//")` and drops the line. **Eleven code
//! lines in seven files** when this module was written; ten after item 1051 took the first;
//! **five** after item 1055 took `sources::code_lines` — the widest-reach of them, since
//! `Source::code` is what most gates here read — and the copy of it that was sitting in
//! `authored`'s own test module.
//!
//! ⚠⚠ Those numbers are not a claim here: `what_a_comment_is_has_one_spelling_in_this_workspace`
//! walks the tree for them and holds the count as an equality, because the first draft of this
//! paragraph said *six files* off a grep read by eye and no reader could have re-taken it. Register
//! item 1055 carries the five that are left, each with what it calls a comment beside it — and
//! **zero is not the target**, which is the register's sentence and not this module's to overrule.
//!
//! That spelling is an APPROXIMATION of *not in a comment*, and it misses two shapes:
//!
//! - a comment after code on the same line — `let n = 1; // probe_ms=` — which the filter keeps
//!   whole, prose and all;
//! - `/* … */` in any position, which does not begin with `//` at all.
//!
//! [`crate::loop_shape::uncommented`] was made public on the rule that *a second copy is where two
//! readers of one file come to disagree*. It answers about SCXML. This is the same rule reaching
//! the other language this workspace's gates read, and it is the first spelling of it that a case
//! can be run against.
//!
//! # ⚠⚠ It reports the shapes rather than claiming the improvement
//!
//! A [`crate::rust_source::Comment`] says which of the three shapes a span is, so the difference
//! between this and the filter it replaces is a NUMBER a caller can print — `Trailing` plus
//! `Block` is exactly what the whole-line spelling never saw. Register item 1051 is an unguarded
//! **0** of the first kind: a
//! gate over `pty_round_trip.rs` reads its own source for the field names it prints, and one
//! trailing comment spelling one of them would answer it. Nothing counted, so nothing could say the
//! 0 had moved.
//!
//! # ⛔⛔ The two ways a source can run out, and only ONE of them is safe to be quiet about
//!
//! An unterminated `/*` swallows the rest of the file as prose: the needles a gate was looking for
//! go with it, so it goes RED. An unterminated `"` does the opposite — every comment after it is
//! kept as code, and a gate over that text is answered by the sentences discussing it. That is the
//! direction nobody notices, so a [`crate::rust_source::Scan`] reports it and a caller must refuse.
//! A source that shipped either would not compile; a gate that runs on a tree which does not
//! compile is exactly this crate's own subject.
//!
//! # ⛔ What this does NOT tell apart, since the doc that hedges instead of counting is the defect
//!
//! - **One string literal from another.** A field name in an assertion message, a needle or a
//!   fixture is code to this scanner, exactly as a `println!` argument is. A caller wanting *this
//!   is PRINTED* has not got it from here.
//! - **Live code from `#[cfg]`-dead code.** Nothing here evaluates an attribute.
//! - **A macro's expansion.** What a `macro_rules!` body assembles is text this never sees.

/// Which of Rust's three comment shapes a span is.
///
/// ⚠⚠ The distinction is not decoration: [`Shape::WholeLine`] is the only one the
/// `starts_with("//")` filter this module replaces could see, so a caller that counts the other two
/// is measuring what that filter missed rather than asserting it missed something.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Shape {
    /// `// …`, with nothing but whitespace before it on its line. Doc comments are this shape:
    /// `///` and `//!` begin with `//`, and Rust reads them as comments carrying an attribute.
    WholeLine,
    /// `// …` after code on the same line — the shape that makes item 1051 an unguarded zero.
    Trailing,
    /// `/* … */`, in any position, nesting as Rust nests them.
    Block,
}

/// What the walk was still inside when the source ran out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unclosed {
    /// A `/*` with no partner. The rest of the file was taken as comment, so a needle looked for
    /// in it is not found and the caller's gate goes red on its own.
    Comment,
    /// A `"`, a raw string's `"#` or a `'` with no partner. The rest of the file was taken as
    /// CODE — comments and all — which is the direction a gate cannot notice.
    Literal,
}

/// One comment, as a byte range over the source it was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Comment {
    /// Where it opens: the byte of its first `/`.
    pub at: usize,
    /// Where it ends, exclusive. A `//` comment ends AT its newline and never consumes it, so
    /// removing one leaves the line structure of the file alone; a `/* */` takes whatever newlines
    /// it spans.
    pub end: usize,
    /// Which shape it is.
    pub shape: Shape,
}

/// One reading of a source: its comments, and whether the walk finished on solid ground.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scan {
    /// Every comment, in source order.
    pub comments: Vec<Comment>,
    /// What the walk was inside at the end of the file, if anything. `None` is the only value a
    /// gate should accept — see this module's header for why the two states are not symmetric.
    pub unclosed: Option<Unclosed>,
}

/// Read `source` for its comments.
///
/// ⚠⚠ ONE definition, and [`uncommented`] is written in terms of it — this module's own rule
/// applied to itself, the way `loop_shape::comments` is for the other language.
///
/// # ⛔ The literals are walked, not skipped over, and that is the whole cost of being right
///
/// `"http://…"`, `'"'` and `r#"a " b"#` each defeat a scanner that only looks for `//`, and a
/// lifetime defeats one that reads every `'` as opening a character literal: `&'a str` would
/// swallow the code up to the next quote. The cases in this module's tests are that list.
///
/// ⚠ Every branch here is one a case can move, which was checked by trying to delete one: the `b`
/// prefix of `literal_end` was argued dead and put back by
/// `the_byte_prefix_is_what_lets_a_raw_byte_string_open`.
#[must_use]
pub fn scan(source: &str) -> Scan {
    let bytes = source.as_bytes();
    let mut comments = Vec::new();
    let mut unclosed = None;
    let mut at = 0;
    while at < bytes.len() {
        match (bytes[at], bytes.get(at + 1)) {
            (b'/', Some(b'/')) => {
                let end = bytes[at..]
                    .iter()
                    .position(|byte| *byte == b'\n')
                    .map_or(bytes.len(), |offset| at + offset);
                comments.push(Comment {
                    at,
                    end,
                    shape: line_shape(source, at),
                });
                at = end;
            }
            (b'/', Some(b'*')) => {
                let (end, closed) = block_end(bytes, at);
                comments.push(Comment {
                    at,
                    end,
                    shape: Shape::Block,
                });
                if !closed {
                    unclosed = Some(Unclosed::Comment);
                }
                at = end;
            }
            (b'\'', _) => {
                let (end, closed) = quoted_end(source, at);
                if !closed {
                    unclosed = Some(Unclosed::Literal);
                }
                at = end;
            }
            _ => {
                at = match literal_end(bytes, at) {
                    Some((past, closed)) => {
                        if !closed {
                            unclosed = Some(Unclosed::Literal);
                        }
                        past
                    }
                    None => at + 1,
                }
            }
        }
    }
    Scan { comments, unclosed }
}

/// `source` with every comment removed — what the code SAYS, rather than what its prose says about
/// itself.
///
/// ⚠ The claim a caller may make on this is *"not in a comment"*, which is what the name says and
/// no more. The module header lists the three things it is not, and [`scan`] is what a caller reads
/// to refuse a source this could not finish.
#[must_use]
pub fn uncommented(source: &str) -> String {
    let mut kept = String::with_capacity(source.len());
    let mut at = 0;
    for comment in scan(source).comments {
        kept.push_str(&source[at..comment.at]);
        at = comment.end;
    }
    kept.push_str(&source[at..]);
    kept
}

/// The source's lines with every comment BLANKED rather than removed, one-indexed.
///
/// # ⛔⛔⛔⛔⛔ Why this exists beside [`uncommented`], which already drops comments
///
/// Removing a comment removes its NEWLINES, so every line after a `/* */` is renumbered — and a
/// gate built on that reports a line nobody can open. `loop_shape::uncommented_lines` is the same
/// pair one language over, and the number it measured there was **`ai_loop.scxml:508` for an
/// `<assign>` on line 3601**, which looked entirely plausible.
///
/// ⚠ *Does the needle match?* and *where is it?* are different questions, and only one of them
/// survives having the text edited underneath it. [`uncommented`] is right for the first; this is
/// what the second needs — and what [`crate::sources::Source`] needs, because `product` is `code`
/// with the proving LINE NUMBERS removed.
///
/// ⚠⚠ Blanked CHAR BY CHAR, never by byte: this workspace's commentary is not ASCII, and a space
/// written at a byte offset inside a character is not a string at all.
#[must_use]
pub fn uncommented_lines(source: &str) -> Vec<(usize, String)> {
    let mut blanked = String::with_capacity(source.len());
    let mut at = 0;
    for comment in scan(source).comments {
        blanked.push_str(&source[at..comment.at]);
        for character in source[comment.at..comment.end].chars() {
            blanked.push(if character == '\n' { '\n' } else { ' ' });
        }
        at = comment.end;
    }
    blanked.push_str(&source[at..]);
    blanked
        .lines()
        .enumerate()
        .map(|(index, line)| (index + 1, line.to_owned()))
        .collect()
}

/// Whether a `//` at `at` has code before it on its line.
fn line_shape(source: &str, at: usize) -> Shape {
    let start = source[..at].rfind('\n').map_or(0, |newline| newline + 1);
    match source[start..at].trim().is_empty() {
        true => Shape::WholeLine,
        false => Shape::Trailing,
    }
}

/// Where the `/*` at `at` closes, exclusive, and whether it closed at all.
///
/// ⛔ NESTED, because Rust nests them: `/* /* */ */` is one comment and a scanner that stopped at
/// the first `*/` would hand the caller ` */` as code.
fn block_end(bytes: &[u8], at: usize) -> (usize, bool) {
    let mut depth = 0usize;
    let mut cursor = at;
    while cursor < bytes.len() {
        match (bytes[cursor], bytes.get(cursor + 1)) {
            (b'/', Some(b'*')) => {
                depth += 1;
                cursor += 2;
            }
            (b'*', Some(b'/')) => {
                depth -= 1;
                cursor += 2;
                if depth == 0 {
                    return (cursor, true);
                }
            }
            _ => cursor += 1,
        }
    }
    (bytes.len(), false)
}

/// Where the string literal opening at `at` ends, exclusive, and whether it closed — or `None` if
/// no string opens there.
///
/// ⚠ The prefixes are `b`, `r` and `br`, and a prefix that is the tail of an identifier is not one:
/// `r#type` is a raw IDENTIFIER, which this refuses by requiring the quote.
///
/// ⛔⛔ **THE `b` ARM WAS WRITTEN OUT OF THIS AS DEAD WEIGHT AND A CASE PUT IT BACK** — a byte
/// string is indeed found from its quote, so the arm looked like it could change no answer. It
/// changes `br#"…"#`: without it the walk arrives at the `r` with `b` behind it, the guard below
/// reads that as the tail of an identifier, and the raw string is never opened at all. The
/// reasoning was sound and the case was not, which is the only reason to run one.
fn literal_end(bytes: &[u8], at: usize) -> Option<(usize, bool)> {
    let mut cursor = at;
    if bytes[cursor] == b'b' {
        cursor += 1;
    }
    let raw = bytes.get(cursor) == Some(&b'r');
    if raw {
        cursor += 1;
    }
    let mut hashes = 0;
    while raw && bytes.get(cursor) == Some(&b'#') {
        hashes += 1;
        cursor += 1;
    }
    if bytes.get(cursor) != Some(&b'"') {
        return None;
    }
    if cursor > at && at > 0 && is_ident(bytes[at - 1]) {
        return None;
    }
    cursor += 1;
    Some(match raw {
        true => raw_end(bytes, cursor, hashes),
        false => escaped_end(bytes, cursor),
    })
}

/// Where a string that honours `\` escapes closes, exclusive, and whether it closed.
fn escaped_end(bytes: &[u8], mut at: usize) -> (usize, bool) {
    while at < bytes.len() {
        match bytes[at] {
            b'\\' => at += 2,
            b'"' => return (at + 1, true),
            _ => at += 1,
        }
    }
    (bytes.len(), false)
}

/// Where a raw string closes, exclusive, and whether it closed: a quote carrying `hashes` hashes.
fn raw_end(bytes: &[u8], mut at: usize, hashes: usize) -> (usize, bool) {
    while at < bytes.len() {
        let closes = bytes[at] == b'"'
            && bytes[at + 1..]
                .iter()
                .take(hashes)
                .filter(|byte| **byte == b'#')
                .count()
                == hashes;
        if closes {
            return (at + 1 + hashes, true);
        }
        at += 1;
    }
    (bytes.len(), false)
}

/// Where the `'` at `at` stops mattering, and whether a literal it opened was closed.
///
/// ⛔⛔ A LIFETIME IS NOT A CHARACTER LITERAL, and reading `&'a str` as one opens a literal that
/// runs to the next quote in the file — which is how a scanner comes to read code as a string and
/// a comment inside it as code. The three shapes are told apart by what follows the quote: an
/// escape is always a literal, a single character CLOSED by a quote is one, and anything else is a
/// lifetime or a loop label, which opens nothing and so can never be unclosed.
fn quoted_end(source: &str, at: usize) -> (usize, bool) {
    let bytes = source.as_bytes();
    match bytes.get(at + 1) {
        Some(b'\\') => {
            // Past the backslash and the character it escapes — `'\''` closes on the quote AFTER
            // the one it escaped — then on to the close.
            let mut cursor = at + 3;
            while cursor < bytes.len() && bytes[cursor] != b'\'' {
                cursor += 1;
            }
            match cursor < bytes.len() {
                true => (cursor + 1, true),
                false => (bytes.len(), false),
            }
        }
        Some(_) => {
            let rest = &source[at + 1..];
            let first = rest.chars().next().expect("a byte here means a character");
            match rest.as_bytes().get(first.len_utf8()) {
                Some(b'\'') => (at + 1 + first.len_utf8() + 1, true),
                _ => (at + 1, true),
            }
        }
        None => (at + 1, true),
    }
}

/// Whether a byte can be part of a Rust identifier, for telling a literal's prefix from a suffix.
fn is_ident(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || !byte.is_ascii()
}

#[cfg(test)]
mod tests {
    use super::{Shape, Unclosed, scan, uncommented, uncommented_lines};

    /// Every lexical shape this scanner must tell apart, as a source it could really be handed,
    /// what must survive it, and which comment shape it must report.
    ///
    /// # ⛔⛔⛔⛔⛔ BOTH DIRECTIONS ON EVERY ROW, because a scanner that removes everything is as
    /// wrong as one that removes nothing — and only one of those announces itself
    ///
    /// A row asserting only that the prose is gone is passed by `|_| String::new()`. A row
    /// asserting only that the code survives is passed by the identity. Each row therefore names
    /// code that must REMAIN and carries the needle `probe_ms=` inside a comment, which must be
    /// GONE — and every row is written so that dropping the branch it is about breaks one of the
    /// two.
    ///
    /// ⛔⛔ **THE NEEDLE IS SPELLED `probe_ms=` IN EVERY ROW BECAUSE THAT IS THE HAZARD** — register
    /// items 1041, 1042 and 1051. The gate those items built asks whether a field name the
    /// instrument declares appears in the suite's code, and every row puts that name where a
    /// scanner might wrongly call it code.
    const CASES: &[(&str, &str, &str, Shape)] = &[
        (
            "a whole-line comment, the one shape the filter this replaces could see",
            "// probe_ms= is discussed here\nlet n = probe();\n",
            "let n = probe();",
            Shape::WholeLine,
        ),
        (
            "a trailing comment, which is register item 1051 itself",
            "let n = probe(); // probe_ms= is discussed here\n",
            "let n = probe();",
            Shape::Trailing,
        ),
        (
            "a doc comment, which is a whole-line comment carrying an attribute",
            "/// probe_ms= is discussed here\nlet n = probe();\n",
            "let n = probe();",
            Shape::WholeLine,
        ),
        (
            "a block comment, which never begins with two slashes",
            "let n = /* probe_ms= is discussed here */ probe();\n",
            "probe();",
            Shape::Block,
        ),
        (
            "a NESTED block comment, whose first close belongs to the inner one",
            "/* outer /* inner */ probe_ms= still the comment */ let n = probe();\n",
            "let n = probe();",
            Shape::Block,
        ),
        (
            "two slashes INSIDE a string, which no parser reads as a comment",
            "let url = \"https://host\"; let n = probe(); // probe_ms= is discussed here\n",
            "let n = probe();",
            Shape::Trailing,
        ),
        (
            "a raw string holding an odd quote, which escape rules would pair with the wrong one",
            "let r = r#\"a \" b\"#; let n = probe(); // probe_ms= is discussed here\n",
            "let n = probe();",
            Shape::Trailing,
        ),
        (
            "a quote as a CHARACTER, which a scanner without character literals opens a string on",
            "let q = '\"'; let n = probe(); // probe_ms= is discussed here\n",
            "let n = probe();",
            Shape::Trailing,
        ),
        (
            "an escaped quote inside a character literal, whose close is the SECOND quote after it",
            "let q = '\\''; let n = probe(); // probe_ms= is discussed here\n",
            "let n = probe();",
            Shape::Trailing,
        ),
        (
            "a LIFETIME, which is not a character literal however much it looks like one starting",
            "fn f<'a>(s: &'a str) -> &'a str { s } // probe_ms= is discussed here\nlet n = probe();\n",
            "fn f<'a>(s: &'a str) -> &'a str { s }",
            Shape::Trailing,
        ),
        (
            "a raw IDENTIFIER, which wears a raw string's prefix and opens nothing",
            "let r#type = 1; let n = probe(); // probe_ms= is discussed here\n",
            "let r#type = 1;",
            Shape::Trailing,
        ),
    ];

    #[test]
    fn every_lexical_shape_is_told_apart_in_both_directions() {
        for (what, source, survives, shape) in CASES {
            let code = uncommented(source);
            assert!(
                code.contains(survives),
                "⛔ ITEM 1051: {what} — the code {survives:?} did not survive the scan, so a gate \
                 reading this would go red about something the file really says.\nsource: \
                 {source:?}\nleft: {code:?}",
            );
            assert!(
                !code.contains("probe_ms="),
                "⛔ ITEM 1051: {what} — the prose `probe_ms=` survived the scan, so a gate asking \
                 whether this file PRINTS that field is answered by the sentence discussing \
                 it.\nsource: {source:?}\nleft: {code:?}",
            );
            let read = scan(source);
            assert_eq!(
                read.unclosed, None,
                "⛔ ITEM 1051: {what} — the walk ran off the end of a source that closes \
                 everything it opens.\nsource: {source:?}",
            );
            assert!(
                read.comments.iter().any(|comment| comment.shape == *shape),
                "⛔ ITEM 1051: {what} — no {shape:?} comment was found, so the census that says \
                 how much more this sees than a whole-line filter cannot count it.\nsource: \
                 {source:?}\nfound: {:?}",
                read.comments,
            );
        }
    }

    /// ⚠⚠ **The rows must not all be the same row.** Eleven cases over three shapes is a population
    /// a mutation can shrink without any row failing — delete the nested block row and every
    /// remaining row still passes — so the shapes present are counted, and the count is what a
    /// deletion cannot leave alone.
    #[test]
    fn the_cases_reach_every_shape_and_the_population_is_a_floor() {
        let mut shapes: Vec<Shape> = CASES.iter().map(|(_, _, _, shape)| *shape).collect();
        shapes.sort_unstable();
        shapes.dedup();
        assert_eq!(
            shapes,
            vec![Shape::WholeLine, Shape::Trailing, Shape::Block],
            "⛔ ITEM 1051: the cases no longer reach all three comment shapes, so one of them is \
             held by nothing",
        );
        assert!(
            CASES.len() >= 11,
            "⛔ ITEM 1051: {} case(s) remain of the eleven lexical shapes this scanner was written \
             against. Cases went missing, which makes the scan's claim vacuous for whatever they \
             held — put them back, or write the new number here and say in the register which \
             shape stopped mattering.",
            CASES.len(),
        );
    }

    /// ⛔⛔ **THE TWO ENDINGS ARE NOT SYMMETRIC, AND THE QUIET ONE IS THE HAZARD** — an unterminated
    /// `/*` takes the needles with it and the caller's gate goes red by itself, while an
    /// unterminated `"` hands every comment after it back as code. Only the second needs reporting,
    /// and reporting BOTH is what makes `unclosed == None` a sentence a caller can hold.
    #[test]
    fn a_source_that_runs_out_inside_something_says_which_something() {
        let eaten = "/* probe_ms= and no close";
        assert_eq!(scan(eaten).unclosed, Some(Unclosed::Comment));
        assert!(
            !uncommented(eaten).contains("probe_ms="),
            "⛔ ITEM 1051: an unterminated comment let its own text back out as code",
        );

        for leaked in [
            "let s = \"no close\n// probe_ms= is discussed here\n",
            "let s = r#\"no close\n// probe_ms= is discussed here\n",
            "let s = '\\n\n// probe_ms= is discussed here\n",
        ] {
            assert_eq!(
                scan(leaked).unclosed,
                Some(Unclosed::Literal),
                "⛔ ITEM 1051: a source that ran out inside a literal reported nothing, so the \
                 caller cannot refuse it — and the comment after it is now code: {:?}",
                uncommented(leaked),
            );
        }
    }

    /// ⛔⛔⛔⛔⛔ **THIS CASE WAS WRITTEN TO PROVE A BRANCH DEAD AND PROVED IT LOAD-BEARING** — the
    /// argument was that a byte string is found from its quote and a raw byte string from its `r`,
    /// so [`super::literal_end`]'s `b` arm could go. It cannot: with the arm gone the walk reaches
    /// `br#"…"#` at the `r`, the identifier guard reads the `b` behind it as the tail of a name,
    /// and the raw string never opens — every comment to the end of the file then comes back as
    /// code. Deleting the arm is the mutation this row is red under.
    #[test]
    fn the_byte_prefix_is_what_lets_a_raw_byte_string_open() {
        for source in [
            "let b = b\"//\"; let n = probe(); // probe_ms= is discussed here\n",
            "let b = br#\"a \" b\"#; let n = probe(); // probe_ms= is discussed here\n",
        ] {
            let code = uncommented(source);
            assert!(
                code.contains("let n = probe();"),
                "⛔ ITEM 1051: a byte string cost the code after it: {code:?}",
            );
            assert!(
                !code.contains("probe_ms="),
                "⛔ ITEM 1051: a byte string cost the comment after it: {code:?}",
            );
            assert_eq!(scan(source).unclosed, None);
        }
    }

    /// ⚠ A `//` comment ends AT its newline, so removing one renumbers nothing after it — the
    /// defect [`crate::loop_shape::uncommented_lines`] exists for, one language over.
    #[test]
    fn removing_a_line_comment_leaves_the_lines_where_they_were() {
        let source = "let a = 1; // one\nlet b = 2;\n// two\nlet c = 3;\n";
        assert_eq!(
            uncommented(source).lines().count(),
            source.lines().count(),
            "⛔ ITEM 1051: removing a line comment moved the lines under it",
        );
    }

    /// ⛔⛔⛔⛔⛔ **AND A BLOCK COMMENT DOES MOVE THEM, WHICH IS THE WHOLE REASON FOR
    /// [`super::uncommented_lines`]** — register item 1055.
    ///
    /// [`super::uncommented`] takes a `/* */`'s newlines with it, so every line under one is
    /// renumbered. `crate::sources::Source` cannot survive that: `product` is `code` with the
    /// PROVING LINE NUMBERS removed, so a renumbered `code` ships the wrong lines. The two
    /// assertions are the pair — removal moves them, blanking does not — because a test that only
    /// checked the blanked side would pass against a function that had quietly become the other
    /// one.
    #[test]
    fn a_block_comment_renumbers_what_is_removed_and_not_what_is_blanked() {
        let source = "let a = 1;\n/* probe_ms=\n   still the comment */\nlet d = 4;\n";
        let removed = uncommented(source);
        let moved = removed
            .lines()
            .position(|line| line.trim() == "let d = 4;")
            .map(|index| index + 1);
        assert_eq!(
            moved,
            Some(3),
            "⛔ ITEM 1055: removal is supposed to take the comment's newlines with it, putting the \
             file's line 4 at 3 — if it has stopped doing that, the reason this function exists \
             beside it has gone: {removed:?}",
        );

        let lines = uncommented_lines(source);
        assert_eq!(
            lines.len(),
            source.lines().count(),
            "⛔ ITEM 1055: blanking moved the lines, so a line number taken from it names \
             something else: {lines:?}",
        );
        assert_eq!(
            lines.last().map(|(at, line)| (*at, line.trim())),
            Some((4, "let d = 4;")),
            "⛔ ITEM 1055: the line under a block comment is not where the file has it: {lines:?}",
        );
        assert!(
            !lines.iter().any(|(_, line)| line.contains("probe_ms=")),
            "⛔ ITEM 1055: the prose survived blanking: {lines:?}",
        );
    }

    /// ⚠⚠ **A trailing comment leaves its code ON ITS OWN LINE, with the columns before it
    /// intact** — what `crate::sources::code_lines` needs, since it trims what this hands back and
    /// a line that lost its leading text would be trimmed into a different statement.
    #[test]
    fn a_trailing_comment_leaves_the_code_before_it_where_it_was() {
        let source = "    let n = probe(); // probe_ms= here\n";
        let lines = uncommented_lines(source);
        let (at, only) = lines.first().expect("one line").clone();
        // ⚠ The three clauses are PROPERTIES rather than a hand-counted string: the first draft of
        // this case spelled the expected spaces out and was wrong by one, which is a case that
        // fails for a reason having nothing to do with what it is about.
        assert_eq!(at, 1);
        assert_eq!(
            only.chars().count(),
            source.trim_end_matches('\n').chars().count(),
            "⛔ ITEM 1055: blanking changed the line's width, so a column taken from it points \
             somewhere else: {only:?}",
        );
        assert!(
            only.starts_with("    let n = probe();") && only.trim_end() == "    let n = probe();",
            "⛔ ITEM 1055: the code before the comment did not survive with its columns: {only:?}",
        );
        assert!(
            !only.contains("probe_ms="),
            "⛔ ITEM 1055: the prose survived blanking: {only:?}",
        );
    }

    /// ⛔ **The commentary this workspace writes is not ASCII**, and a blank written at a byte
    /// offset inside a character is not a string at all — the refusal
    /// `crate::loop_shape::uncommented_lines` carries, met here in the language whose comments this
    /// repository actually writes Korean in.
    #[test]
    fn a_comment_that_is_not_ascii_is_blanked_without_splitting_a_character() {
        let lines =
            uncommented_lines("let n = probe(); // 주석이 여기 있다 probe_ms=\nlet d = 4;\n");
        assert_eq!(lines.len(), 2);
        assert!(
            lines[0].1.starts_with("let n = probe();"),
            "⛔ ITEM 1055: {lines:?}",
        );
        assert!(
            !lines[0].1.contains("probe_ms=") && !lines[0].1.contains('주'),
            "⛔ ITEM 1055: a non-ASCII comment survived blanking: {lines:?}",
        );
    }
}
