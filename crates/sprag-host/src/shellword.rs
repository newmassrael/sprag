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
}
