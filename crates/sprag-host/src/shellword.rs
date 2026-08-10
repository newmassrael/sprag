//! Shell WORD quoting: turning a string into one inert word a POSIX shell cannot re-interpret.
//!
//! Lifted out of the file-drop delivery (`crate::upload`, its first consumer — private, so it is
//! named rather than linked) when per-project commands ([`crate::project`]) became the second. The duplication this avoids is not stylistic:
//! quoting is the boundary between "a path with a space in it" and "a path that runs `$(reboot)`",
//! so two copies would be two chances to get a security-relevant transformation subtly different.
//!
//! Both of those use it for the same reason — a string is about to be PASTED at a shell prompt as a
//! command line, so every word must survive word-splitting and expansion exactly as written. The
//! third consumer (`sprag processes`) uses it the other way round: it RENDERS an argument vector
//! read back from `/proc` as one line, and quoting is what keeps that rendering from claiming
//! something false. An argv joined with bare spaces makes `sleep '4 00'` and `sleep 4 00` print
//! identically, which is exactly the ambiguity the wire refuses to carry — so the one place that
//! has to flatten it must not reintroduce it.

/// Characters that need no quoting: they expand to themselves in every POSIX shell, in any position
/// of a word. Everything else (a space, a quote, `~`, `!`, `*`, `$`, a newline…) sends the whole word
/// through [`shell_quote`].
const UNQUOTED: &str = "_-./=+:,@%";

/// Quote `word` for a shell command line: returned as-is when every character expands to itself
/// in every POSIX shell in any position of a word, otherwise wrapped in single quotes with any embedded `'` closed and
/// re-opened (`'` → `'\''`), the one escape POSIX single quoting allows.
///
/// Quoting only when needed keeps the common `cargo test` / `/home/me/report.pdf` clean on the
/// command line while still making `my report.pdf` — or a word containing `$(reboot)` — a single
/// inert word.
#[must_use]
pub fn shell_quote(word: &str) -> String {
    let safe = !word.is_empty()
        && word
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || UNQUOTED.contains(ch));
    if safe {
        return word.to_owned();
    }
    format!("'{}'", word.replace('\'', r"'\''"))
}

/// Read one quoted word back: the inverse of [`shell_quote`], and what a reader of a stored command
/// line needs to recover the path that was written into it.
///
/// It exists because a WRITER that quotes and a READER that does not are a pair that agrees only
/// about paths needing no quoting — which is every path a developer tries and not the one a user
/// has. [`crate::hooks::Target::program_of`] is the reader in question: it recognises sprag's own
/// entry in an agent's config and reports whether the program it names is still on disk, and both
/// of those are claims about a PATH rather than about the characters it was spelled with.
///
/// POSIX single-word rules, which is all a command line's first word can be: `'…'` is literal to
/// the next `'`, `"…"` is literal except for a backslash escape, and a bare backslash escapes the
/// character after it. Anything unterminated is taken as running to the end, because this reads a
/// word somebody may have hand-edited and refusing would turn a repairable entry into an
/// unrecognised one.
#[must_use]
pub fn shell_unquote(word: &str) -> String {
    let mut out = String::with_capacity(word.len());
    let mut chars = word.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\'' => out.extend(chars.by_ref().take_while(|c| *c != '\'')),
            '"' => {
                while let Some(inner) = chars.next() {
                    match inner {
                        '"' => break,
                        '\\' => out.extend(chars.next()),
                        other => out.push(other),
                    }
                }
            }
            '\\' => out.extend(chars.next()),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_word_is_left_alone_and_anything_else_becomes_one_inert_word() {
        assert_eq!(shell_quote("cargo"), "cargo");
        assert_eq!(shell_quote("/home/me/report.pdf"), "/home/me/report.pdf");
        assert_eq!(shell_quote("my report.pdf"), "'my report.pdf'");
        assert_eq!(
            shell_quote("$(reboot)"),
            "'$(reboot)'",
            "a substitution is quoted into an inert word, never expanded"
        );
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(
            shell_quote(""),
            "''",
            "an empty word must still occupy a position on the command line"
        );
    }

    /// Every word `shell_quote` can produce comes back as the word it was made from — including the
    /// ones a developer's own machine never produces, which is where a writer/reader pair drifts.
    #[test]
    fn unquoting_is_the_inverse_of_quoting() {
        for word in [
            "cargo",
            "/home/me/report.pdf",
            "/home/a dir/sprag",
            "it's",
            "$(reboot)",
            "a\nnewline",
            "",
            "back\\slash",
            "\"double\"",
        ] {
            assert_eq!(
                shell_unquote(&shell_quote(word)),
                word,
                "{word:?} did not survive the round trip",
            );
        }
    }

    /// A word somebody QUOTED BY HAND is read too, because an agent's config is the user's file and
    /// nothing stops them editing it. Double quotes are the form the old reader anticipated, and a
    /// bare backslash is the third way a shell spells the same thing.
    #[test]
    fn a_hand_quoted_word_is_read_the_way_a_shell_would() {
        assert_eq!(shell_unquote("\"/home/a dir/sprag\""), "/home/a dir/sprag");
        assert_eq!(shell_unquote("/home/a\\ dir/sprag"), "/home/a dir/sprag");
        assert_eq!(shell_unquote("\"say \\\"hi\\\"\""), "say \"hi\"");
        assert_eq!(
            shell_unquote("'unterminated"),
            "unterminated",
            "a half-edited entry is still recognisable rather than refused",
        );
    }
}
