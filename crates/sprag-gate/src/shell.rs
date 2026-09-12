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
//! # ⛔⛔⛔⛔⛔ Which word a command RUNS is answered here too — register item 1085
//!
//! [`crate::shell::simple_commands`] and [`crate::shell::command_word`] exist because two gates
//! had each answered that question for themselves, with a list of what may stand in front of a
//! command, and both lists read a shape they did not name as *not a command*. The grammar belongs
//! to the one module that already splits the words; what a gate DOES with a word it cannot place
//! is the gate's, and both gates now refuse it.
//!
//! # ⚠⚠ What a text scan can and cannot claim
//!
//! This crate takes no dependencies by charter and there is no shell parser in std, so
//! [`crate::shell::commands_named`] is a QUOTE-AWARE WORD SPLITTER and not a shell. It knows single
//! quotes, double quotes, command substitutions, comments that begin a word, and the operators that
//! end a command. It expands nothing, so a word keeps its `${variables}` verbatim; it does not
//! follow a quote across a line break; and it cannot see a command assembled at run time. Those are
//! stated here rather than implied, and every one of them MISSES rather than refusing wrongly.
//!
//! ⚠ A caller for whom a miss is the escape hatch holds the words against
//! [`crate::shell::expansions_of`], a count over the TEXT that shares none of those limits, and
//! refuses whatever the split did not reach.

use crate::sources::workspace_root;
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
    ///
    /// # ⛔⛔⛔⛔⛔ This borrowed `sources::code_lines`, which answers about RUST — register item 1055
    ///
    /// That function's name, its doc and its only other caller all say Rust: it is *what
    /// `Source::code` is*. It was usable here by **coincidence** — it dropped lines starting with
    /// `#`, which is the Rust ATTRIBUTE rule and also, spelled identically, the shell COMMENT rule
    /// — and its `//` clause was dead weight over a shell script.
    ///
    /// ⚠⚠ The coincidence is exactly why nobody noticed, and the bill came due the moment the Rust
    /// side got a real scanner. Putting comment-awareness into `code_lines` lexed these scripts as
    /// Rust — an apostrophe in prose opening a character literal, a `//` in a path opening a
    /// comment — and `the_walk_reaches_scripts_no_declared_selftest_ever_runs` fell from **121
    /// commands to 99** in one edit. The floor caught it; nothing else would have.
    ///
    /// ⇒ So the rule is spelled HERE, where the module's own doc already explains which of the two
    /// shell rules each caller wants. One language, one answer — the rule
    /// `crate::loop_shape::uncommented` was made public for and `crate::rust_source` exists to
    /// keep.
    #[must_use]
    pub fn code(&self) -> Vec<(usize, String)> {
        self.text
            .lines()
            .enumerate()
            .map(|(index, line)| (index + 1, line.trim().to_owned()))
            .filter(|(_, line)| !line.starts_with('#'))
            .collect()
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
            Token::Word(word) => {
                if let Some(argv) = collecting.as_mut() {
                    argv.push(word.text);
                } else if !word.quoted && word.text == program {
                    collecting = Some(vec![word.text]);
                }
            }
        }
    }
    if let Some(argv) = collecting {
        found.push(argv);
    }
    found
}

/// One word of a line of shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word {
    /// What the shell would hand the child: quotes removed, nothing expanded.
    pub text: String,
    /// The word exactly as the line spells it, quotes and all.
    ///
    /// ⚠ Kept beside [`Word::text`] because removing the quotes is already a decision about them:
    /// `"${BX}"` and `'${BX}'` have the same text, and only the first expands the variable.
    pub raw: String,
    /// Whether any part of the word was quoted.
    pub quoted: bool,
}

/// Every SIMPLE COMMAND spelled on one line of shell, as its words in order — register item 1085.
///
/// A command ends where [`commands_named`] ends one, so `[ -x "$w" ] && "$w" -- run` is two. The
/// words are kept whole: the reserved words that introduce a command and the assignments that
/// prefix one are still in the list, and [`command_word`] says which word the shell RUNS.
///
/// # ⛔⛔⛔⛔⛔ Why this is here and not in a gate
///
/// Two gates used to answer *is this word in command position* for themselves, each with a list
/// of what may stand in front of it (`&&`, `;`, `then`, …), and each read a shape its list did not
/// name as NOT A COMMAND. Measured 2026-09-13 by mutating the hooks: `env -u GIT_INDEX_FILE
/// "${BX}" …`, `[ -x "${BX}" ] && "${BX}" …` and `"$BX" …` each handed the wrapper an argv nobody
/// had measured while `a_fleet_ceiling_is_a_measurement_with_a_date` stayed green — the same argv
/// with no prefix was red — and `if cd "$mirror"; then` stood in the mirror while
/// `no_suite_hands_a_child_the_git_environment_it_inherited` stayed green. A list of leads is an
/// allowlist whose default is the escape hatch.
#[must_use]
pub fn simple_commands(line: &str) -> Vec<Vec<Word>> {
    let mut found = Vec::new();
    let mut current = Vec::new();
    for token in tokens(line) {
        match token {
            Token::Break => {
                if !current.is_empty() {
                    found.push(std::mem::take(&mut current));
                }
            }
            Token::Word(word) => current.push(word),
        }
    }
    if !current.is_empty() {
        found.push(current);
    }
    found
}

/// The reserved words after which the shell BEGINS a command rather than running one.
///
/// ⚠ Only those. `fi`, `done`, `esac` and `}` close a construct and are followed by an operator.
/// `for`, `case` and `select` take words that are not a command, so a caller asking behind one of
/// them gets the reserved word back as the command word — a word nobody runs, which is the
/// direction that refuses rather than guesses. `time` is out for the same reason: it takes options
/// of its own, so the word after it is not reliably the one run.
pub const INTRODUCERS: [&str; 9] = [
    "!", "{", "if", "then", "elif", "else", "while", "until", "do",
];

/// Which word of one simple command the shell RUNS: the first past the [`INTRODUCERS`] and then
/// past the `NAME=value` assignments that prefix a command.
///
/// ⚠ A reserved word is reserved only UNQUOTED — `"if"` is a program's name — so a quoted word is
/// never skipped. And an operand of another program is not the command word even when that program
/// runs it (`env`, `command`, `exec`): what such a program does with its operands is its own
/// grammar and not the shell's, so the caller is handed the program and decides for itself.
///
/// `None` when every word is an introducer or an assignment.
#[must_use]
pub fn command_word(words: &[Word]) -> Option<usize> {
    let mut at = 0;
    while words
        .get(at)
        .is_some_and(|word| !word.quoted && INTRODUCERS.contains(&word.text.as_str()))
    {
        at += 1;
    }
    while words.get(at).is_some_and(|word| is_assignment(&word.raw)) {
        at += 1;
    }
    (at < words.len()).then_some(at)
}

/// Whether a word, as spelled, is a `NAME=value` assignment: an unquoted name, then `=`.
fn is_assignment(raw: &str) -> bool {
    raw.split_once('=').is_some_and(|(name, _)| {
        let mut chars = name.chars();
        chars
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
            && chars.all(|rest| rest.is_ascii_alphanumeric() || rest == '_')
    })
}

/// How many times `text` expands the shell variable `name`: `$name`, or `${name` followed by
/// anything that cannot continue the name (`}`, `:-`, …).
///
/// ⚠ A COUNT OVER THE TEXT and not over a parse, on purpose: it is what a caller holds a parse
/// against, so it must not share the parse's blind spots. It counts inside single quotes too, where
/// the shell expands nothing — the direction that over-counts, so a caller comparing the two
/// refuses rather than passes.
#[must_use]
pub fn expansions_of(text: &str, name: &str) -> usize {
    ["$", "${"]
        .iter()
        .map(|lead| {
            let needle = format!("{lead}{name}");
            text.match_indices(needle.as_str())
                .filter(|(at, _)| {
                    !text[at + needle.len()..]
                        .starts_with(|next: char| next.is_ascii_alphanumeric() || next == '_')
                })
                .count()
        })
        .sum()
}

/// One piece of a shell line: a word, or the end of a command.
enum Token {
    Word(Word),
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
    // Where the word being built BEGAN, so its spelling can be handed back beside its text. Every
    // arm that starts a word does so on the character this iteration opened with, so marking it
    // once at the top of the loop is marking it for all of them.
    let mut begin = 0;
    // The innermost context. A `$(` inside a double-quoted string pushes a plain one back on, so
    // the command inside is tokenised as a command.
    let mut inside_double: Vec<bool> = vec![false];
    let chars: Vec<char> = line.chars().collect();
    let mut at = 0;
    let flush = |out: &mut Vec<Token>,
                 word: &mut String,
                 quoted: &mut bool,
                 started: &mut bool,
                 from: usize,
                 to: usize| {
        if *started {
            out.push(Token::Word(Word {
                text: std::mem::take(word),
                raw: chars[from..to].iter().collect(),
                quoted: *quoted,
            }));
            *quoted = false;
            *started = false;
        }
    };
    while at < chars.len() {
        if !started {
            begin = at;
        }
        let one = chars[at];
        let in_double = *inside_double.last().expect("the stack keeps its floor");
        // ── INSIDE A DOUBLE-QUOTED STRING: only three things are not literal text.
        if in_double {
            if one == '"' {
                inside_double.pop();
                at += 1;
            } else if one == '$' && chars.get(at + 1) == Some(&'(') {
                flush(&mut out, &mut word, &mut quoted, &mut started, begin, at);
                out.push(Token::Break);
                inside_double.push(false);
                at += 2;
            } else if one == '`' {
                flush(&mut out, &mut word, &mut quoted, &mut started, begin, at);
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
                // ⚠ Past the closing quote, and never past the line: an unterminated one would
                // otherwise leave `at` one beyond the end, and the spelling below is a slice.
                at = (at + 1).min(chars.len());
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
                flush(&mut out, &mut word, &mut quoted, &mut started, begin, at);
                out.push(Token::Break);
                inside_double.pop();
                at += 1;
            }
            '|' | '&' | ';' | '(' | ')' | '<' | '>' | '`' => {
                flush(&mut out, &mut word, &mut quoted, &mut started, begin, at);
                out.push(Token::Break);
                at += 1;
            }
            one if one.is_whitespace() => {
                flush(&mut out, &mut word, &mut quoted, &mut started, begin, at);
                at += 1;
            }
            one => {
                word.push(one);
                started = true;
                at += 1;
            }
        }
    }
    flush(&mut out, &mut word, &mut quoted, &mut started, begin, at);
    out
}

#[cfg(test)]
mod tests {
    use super::{command_word, commands_named, expansions_of, is_shell_script, simple_commands};

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

    /// ⛔⛔⛔ A word keeps its SPELLING beside its text — register item 1085. Removing the quotes is
    /// a decision about them, and `'$w'` does not expand what `"$w"` does.
    #[test]
    fn a_word_keeps_its_spelling_beside_its_text() {
        let commands = simple_commands(r#"[ -x "$w" ] && '$w' -- bash -c "$x""#);
        let spelled: Vec<Vec<&str>> = commands
            .iter()
            .map(|words| words.iter().map(|word| word.raw.as_str()).collect())
            .collect();
        assert_eq!(
            spelled,
            vec![
                vec!["[", "-x", "\"$w\"", "]"],
                vec!["'$w'", "--", "bash", "-c", "\"$x\""],
            ],
        );
        assert_eq!(commands[1][0].text, "$w");
        assert_eq!(
            simple_commands("'unterminated")[0][0].raw,
            "'unterminated",
            "an unterminated quote is the rest of the line, and its spelling must not run past it",
        );
    }

    /// ⛔⛔⛔ The word a command RUNS is past the grammar that introduces it and the assignments that
    /// prefix it — and a quoted reserved word, or an operand of another program, is not grammar.
    #[test]
    fn the_word_a_command_runs_is_past_its_grammar_and_its_assignments() {
        let first = |line: &str| {
            let commands = simple_commands(line);
            command_word(&commands[0]).map(|at| commands[0][at].raw.clone())
        };
        assert_eq!(
            first(r#"if ! LANE=x OTHER="a b" "$w" -- run"#).as_deref(),
            Some("\"$w\""),
        );
        assert_eq!(
            first(r#"env -u GIT_INDEX_FILE "$w""#).as_deref(),
            Some("env")
        );
        assert_eq!(first(r#""if" x"#).as_deref(), Some("\"if\""));
        assert_eq!(first("LANE=x"), None);
    }

    /// ⚠ The count a parse is held against, both ways: every spelling of this variable is counted,
    /// and a name that merely begins with it is another variable.
    #[test]
    fn an_expansion_is_counted_under_every_spelling_and_no_longer_name() {
        assert_eq!(
            expansions_of(r#"$BX ${BX} "${BX:-}" '$BX' $BXY ${BXY} BX"#, "BX"),
            4,
        );
    }
}
