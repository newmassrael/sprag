//! Every SHELL script this workspace carries, and the argv of the commands they spell.
//!
//! # Why this is beside [`crate::sources`] rather than inside it
//!
//! [`crate::sources`] answers about RUST. Two gates now ask about shell — *does every declared
//! selftest run somewhere this suite looks* (register item 799) and *does any script spell a `sed`
//! the other platform reads differently* (item 1006) — and they need the same file list, the same
//! refusal to walk `target/` and `.git/`, and the same decision about what counts as a shell
//! script. A walk copied into the second is the duplication item 213 is about.
//!
//! # ⚠⚠⚠⚠⚠ What is SHARED is the walk. The COMMENT RULE is not, and that is measured
//!
//! There are two honest ways to drop a comment from shell text and this workspace needs both:
//!
//! * cut every line at its first `#`. Item 799's gate does this, because it is looking for the word
//!   `--selftest` and must not find it in the prose that explains it. Its own doc says the scan
//!   errs toward seeing LESS code, which for that question costs a red rather than a pass.
//! * drop only the lines that BEGIN with `#`, which is what [`crate::shell::ShellSource::code`]
//!   returns.
//!
//! **The first rule cannot be used for item 1006 and the measurement says so plainly**: the script
//! that carried the label defect is `s/probe#[0-9]*/RUN/g; …`, and a cut at the first `#` deletes
//! the offence four characters in. A gate built on that rule would have been green on the very line
//! it exists to catch. So the walk is shared and each caller states its own rule.
//!
//! # ⚠⚠ What a text scan can and cannot claim
//!
//! This crate takes no dependencies by charter and there is no shell parser in std, so
//! [`crate::shell::commands_named`] is a QUOTE-AWARE WORD SPLITTER and not a shell. It knows single
//! quotes, double quotes, command substitutions, comments that begin a word, and the operators that
//! end a command. It expands nothing, so a word keeps its `${variables}` verbatim; it does not
//! follow a quote across a line break; and it cannot see a command assembled at run time. Those are
//! stated here rather than implied, and every one of them MISSES rather than refusing wrongly.

use crate::sources::{code_lines, workspace_root};
use std::path::PathBuf;

/// One shell script this workspace carries.
#[derive(Debug, Clone)]
pub struct ShellSource {
    /// Relative to the workspace root, so a gate's message is a path a person can open.
    pub file: String,
    /// The file exactly as it is on disk — each caller applies its own comment rule to it.
    pub text: String,
}

impl ShellSource {
    /// The lines that are not WHOLE-LINE comments, as `(one-indexed line number, trimmed text)`.
    ///
    /// ⚠ Whole-line and not a cut at the first `#`: see this module's own doc for the measurement
    /// that chose it. A `#` inside a script is part of the script.
    #[must_use]
    pub fn code(&self) -> Vec<(usize, String)> {
        code_lines(&self.text)
    }

    /// The file with every line cut at its FIRST `#`, comments and quoted hashes alike.
    ///
    /// ⚠ The OTHER rule, and the module's doc says why both are here. This one is for a question
    /// whose wrong answer must be a red: item 799 looks for the word `--selftest` and the prose
    /// explaining that flag says it too, so a scan that saw the comment would send this suite to
    /// run a hook with an argument it does not understand. It errs toward seeing LESS code.
    ///
    /// ⛔ It is the wrong rule for anything about a script's TEXT — `s/probe#…/` loses its subject
    /// four characters in.
    #[must_use]
    pub fn code_cut_at_hash(&self) -> String {
        self.text
            .lines()
            .map(|line| match line.find('#') {
                Some(at) => &line[..at],
                None => line,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Whether a file this walk reached is a SHELL SCRIPT.
///
/// ⚠ By NAME or by SHEBANG, and the second is not decoration: `.githooks/` names its hooks
/// `pre-push`, not `pre-push.sh`, and `tests/doubles/` names its stand-ins after the programs they
/// stand in for. A name-only filter would walk past both.
#[must_use]
pub fn is_shell_script(name: &str, text: &str) -> bool {
    name.ends_with(".sh")
        || text
            .lines()
            .next()
            .is_some_and(|first| first.starts_with("#!") && first.contains("sh"))
}

/// Every shell script IN THE TREE, as `(repo-relative path, contents)`.
///
/// ⚠ `.git/` and `target/` are not source and are not walked. Nothing else is skipped, which is the
/// property this function exists for: a walk that can be told where not to look is a walk whose
/// exemption list is the answer.
///
/// # Panics
///
/// When the tree cannot be read, or when the walk found so few files that it is plainly pointed
/// somewhere else. A probe pointed at nothing must never read as clean.
#[must_use]
pub fn shell_sources() -> Vec<ShellSource> {
    let root = workspace_root();
    let mut stack = vec![root.clone()];
    let mut found: Vec<ShellSource> = Vec::new();
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir)
            .unwrap_or_else(|why| panic!("{} must be readable: {why}", dir.display()));
        for entry in entries {
            let path: PathBuf = entry.expect("a directory entry").path();
            let name = path
                .file_name()
                .expect("a directory entry has a name")
                .to_string_lossy()
                .into_owned();
            if path.is_dir() {
                if name != ".git" && name != "target" {
                    stack.push(path);
                }
                continue;
            }
            // A file with some OTHER extension is not a shell script, and skipping it here is what
            // keeps this from reading every `.rs` in the workspace to learn that.
            if path.extension().is_some_and(|ext| ext != "sh") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if is_shell_script(&name, &text) {
                found.push(ShellSource {
                    file: path
                        .strip_prefix(&root)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .into_owned(),
                    text,
                });
            }
        }
    }
    assert!(
        found.len() > 20,
        "a walk that found only {} shell script(s) is pointed at the wrong tree — this workspace's \
         hooks and test doubles alone are more than that, and a probe pointed at nothing must \
         never read as clean",
        found.len(),
    );
    found.sort_by(|one, two| one.file.cmp(&two.file));
    found
}

/// Every argv, `program` included, of a command named `program` spelled on one line of shell.
///
/// The words are what the shell would hand the child: quotes removed, nothing expanded. A command
/// ends at the first unquoted operator, so `printf x | sed -n 's/a/b/' && echo` yields one argv of
/// three words.
///
/// ⚠ See this module's doc for what this cannot see. In particular an unterminated quote keeps the
/// rest of the line as ONE word, which ends the command's argv rather than inventing one.
#[must_use]
pub fn commands_named(line: &str, program: &str) -> Vec<Vec<String>> {
    let mut found = Vec::new();
    let mut collecting: Option<Vec<String>> = None;
    for token in tokens(line) {
        match token {
            Token::Break => {
                if let Some(argv) = collecting.take() {
                    found.push(argv);
                }
            }
            Token::Word(word, quoted) => {
                if let Some(argv) = collecting.as_mut() {
                    argv.push(word);
                } else if !quoted && word == program {
                    collecting = Some(vec![word]);
                }
            }
        }
    }
    if let Some(argv) = collecting {
        found.push(argv);
    }
    found
}

/// One piece of a shell line: a word (with whether any of it was quoted), or the end of a command.
enum Token {
    Word(String, bool),
    Break,
}

/// Split one line of shell into words and command boundaries.
///
/// ⚠ The operators that BREAK a command are the ones this repository's hooks actually spell —
/// `| & ; ( ) < >` and a backtick. `=` does not break: `x=$(sed …)` is an assignment whose value is
/// a command, and the `$(` is what opens it.
///
/// ⛔⛔⛔⛔⛔ **A COMMAND SUBSTITUTION INSIDE DOUBLE QUOTES IS A COMMAND, and the first draft of
/// this could not see one.** `x="$(printf '%s' "$1" | sed 's/^ //')"` is how most of this
/// repository's shell is written; a scanner that treated the outer `"` as opening one flat string
/// read the whole line as a word and found no `sed` at all. Measured 2026-09-10: it judged 24
/// commands, and **eight more appeared** the moment the nesting was modelled — `scratch-guard.sh`'s
/// only one among them. A miss is the safe direction for a limit that is STATED; this one was not
/// stated, it was not known, and it was a quarter of the population.
fn tokens(line: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    let mut started = false;
    // The innermost context. A `$(` inside a double-quoted string pushes a plain one back on, so
    // the command inside is tokenised as a command.
    let mut inside_double: Vec<bool> = vec![false];
    let chars: Vec<char> = line.chars().collect();
    let mut at = 0;
    let flush = |out: &mut Vec<Token>, word: &mut String, quoted: &mut bool, started: &mut bool| {
        if *started {
            out.push(Token::Word(std::mem::take(word), *quoted));
            *quoted = false;
            *started = false;
        }
    };
    while at < chars.len() {
        let one = chars[at];
        let in_double = *inside_double.last().expect("the stack keeps its floor");
        // ── INSIDE A DOUBLE-QUOTED STRING: only three things are not literal text.
        if in_double {
            if one == '"' {
                inside_double.pop();
                at += 1;
            } else if one == '$' && chars.get(at + 1) == Some(&'(') {
                flush(&mut out, &mut word, &mut quoted, &mut started);
                out.push(Token::Break);
                inside_double.push(false);
                at += 2;
            } else if one == '`' {
                flush(&mut out, &mut word, &mut quoted, &mut started);
                out.push(Token::Break);
                inside_double.push(false);
                at += 1;
            } else if one == '\\' && at + 1 < chars.len() {
                word.push(chars[at]);
                word.push(chars[at + 1]);
                started = true;
                at += 2;
            } else {
                word.push(one);
                started = true;
                at += 1;
            }
            continue;
        }
        match one {
            '\'' => {
                quoted = true;
                started = true;
                at += 1;
                // Inside single quotes a backslash is an ordinary character, which is why this is
                // not the same branch as the double-quoted one above.
                while at < chars.len() && chars[at] != '\'' {
                    word.push(chars[at]);
                    at += 1;
                }
                at += 1;
            }
            '"' => {
                quoted = true;
                started = true;
                inside_double.push(true);
                at += 1;
            }
            '\\' if at + 1 < chars.len() => {
                word.push(chars[at + 1]);
                started = true;
                quoted = true;
                at += 2;
            }
            '#' if !started => {
                // A `#` that BEGINS a word starts a comment; one inside a word does not, which is
                // exactly the difference `probe#7` turns on.
                break;
            }
            ')' if inside_double.len() > 1 => {
                flush(&mut out, &mut word, &mut quoted, &mut started);
                out.push(Token::Break);
                inside_double.pop();
                at += 1;
            }
            '|' | '&' | ';' | '(' | ')' | '<' | '>' | '`' => {
                flush(&mut out, &mut word, &mut quoted, &mut started);
                out.push(Token::Break);
                at += 1;
            }
            one if one.is_whitespace() => {
                flush(&mut out, &mut word, &mut quoted, &mut started);
                at += 1;
            }
            one => {
                word.push(one);
                started = true;
                at += 1;
            }
        }
    }
    flush(&mut out, &mut word, &mut quoted, &mut started);
    out
}

#[cfg(test)]
mod tests {
    use super::{commands_named, is_shell_script};

    #[test]
    fn a_name_or_a_shebang_decides_what_is_a_shell_script() {
        assert!(is_shell_script("tidy.sh", "echo hi\n"));
        assert!(is_shell_script("pre-push", "#!/usr/bin/env bash\nexit 0\n"));
        assert!(!is_shell_script(
            "NOTES",
            "# --selftest is discussed here\n"
        ));
    }

    #[test]
    fn the_words_are_what_the_shell_would_hand_the_child() {
        assert_eq!(
            commands_named("printf x | command sed -n 's/a/b/' \"$file\"", "sed"),
            vec![vec![
                "sed".to_owned(),
                "-n".to_owned(),
                "s/a/b/".to_owned(),
                "$file".to_owned(),
            ]],
        );
    }

    /// ⚠⚠ The `#` rule both ways, because the whole gate rests on it: a `#` that begins a word ends
    /// the line, and one inside a word is part of the script.
    #[test]
    fn a_hash_inside_a_word_is_not_a_comment() {
        assert_eq!(
            commands_named("sed 's/probe#[0-9]*/RUN/g' # erase them", "sed"),
            vec![vec!["sed".to_owned(), "s/probe#[0-9]*/RUN/g".to_owned()]],
        );
    }

    /// ⛔ A MENTION IS NOT A CALL. The program's name inside quotes is text the script prints.
    #[test]
    fn a_quoted_mention_is_not_a_command() {
        assert!(commands_named("echo \"run sed -n 's/a/b/'\"", "sed").is_empty());
    }

    /// ⛔ Two commands on one line are two argvs, not one long one.
    #[test]
    fn an_operator_ends_the_argv() {
        assert_eq!(
            commands_named("sed -n 'p' | sed 'd'", "sed"),
            vec![
                vec!["sed".to_owned(), "-n".to_owned(), "p".to_owned()],
                vec!["sed".to_owned(), "d".to_owned()],
            ],
        );
    }

    /// ⚠ A substitution is where most of this repository's `sed` calls live.
    #[test]
    fn a_command_substitution_opens_a_command() {
        assert_eq!(
            commands_named("kb=$(sed -n 's/^k = \\(.*\\)/\\1/p' \"$decl\")", "sed"),
            vec![vec![
                "sed".to_owned(),
                "-n".to_owned(),
                "s/^k = \\(.*\\)/\\1/p".to_owned(),
                "$decl".to_owned(),
            ]],
        );
    }

    /// ⛔⛔⛔⛔⛔ **AND ONE INSIDE DOUBLE QUOTES, with its own double-quoted words.** This is the
    /// shape `scratch-guard.sh` and `hosted-read.sh` are written in, and the first draft of the
    /// splitter read the whole line as a single word and found nothing. Six of this tree's `sed`
    /// commands live behind exactly this.
    #[test]
    fn a_substitution_inside_double_quotes_is_still_a_command() {
        assert_eq!(
            commands_named(
                "carried=\"$(printf '%s' \"${1:-}\" | sed 's/^ //; s/ $//')\"",
                "sed",
            ),
            vec![vec!["sed".to_owned(), "s/^ //; s/ $//".to_owned()]],
        );
    }

    /// ⚠ And the nesting must CLOSE: what follows the substitution is text again, not argv.
    #[test]
    fn the_substitution_ends_where_its_paren_does() {
        assert_eq!(
            commands_named("x=\"$(sed -n p)\" && sed -n d", "sed"),
            vec![
                vec!["sed".to_owned(), "-n".to_owned(), "p".to_owned()],
                vec!["sed".to_owned(), "-n".to_owned(), "d".to_owned()],
            ],
        );
    }
}
