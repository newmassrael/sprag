//! ShellCheck over every shell script this workspace carries — register item 1086.
//!
//! # What stood here before
//!
//! `.githooks/` carried 28 ShellCheck directives — 9 `disable=` and 19 `source-path=` — and nothing
//! in the tree ran ShellCheck: no hook, no test, no CI step (measured 2026-09-12 by a grep of all
//! three, the directives themselves excluded). The directives read as a checker somebody runs, and
//! the `disable=`s were exemptions from a gate that did not exist.
//!
//! # What is decided here, and what is left to the caller
//!
//! Everything in this module is a function of TEXT: the pinned version, what `shellcheck --version`
//! printed, what a run printed and exited with, and which directives in a script switch the checker
//! off. Spawning stays in `tests/no_shell_script_escapes_the_static_checker.rs`, so the shapes a
//! healthy machine never produces — no tool, the wrong version, an exit status ShellCheck keeps for
//! its own failures — are handed to these functions directly rather than hoped for.
//!
//! # Three ways the gate could go green without a script changing, each measured
//!
//! Measured 2026-09-13 with ShellCheck 0.11.0, each on a script carrying one SC2086:
//!
//! * `SHELLCHECK_OPTS=--exclude=SC2086` in the environment: exit 0.
//!   [`invocation`](crate::shellcheck::invocation) removes the variable.
//! * a `~/.shellcheckrc` holding `disable=SC2086`: exit 0. The invocation names the project's own
//!   [`RC`](crate::shellcheck::RC) with `--rcfile`, which measured as winning over it, and
//!   [`rc_refusals`](crate::shellcheck::rc_refusals) keeps a `disable=` out of that file as well.
//! * a directive in the script: `disable=all` exits 0, and so does a `disable=` that stands before
//!   the file's first command, which ShellCheck applies to the WHOLE file — including when a comment
//!   block separates it from the shebang. [`hatches_in`](crate::shellcheck::hatches_in) refuses
//!   both, and a `disable=` whose line gives no reason.
//!
//! # Why the version is pinned
//!
//! Measured the same day over this tree's 42 scripts with the same options: 0.10.0 reports eleven
//! findings 0.11.0 does not, and 0.11.0 reports one 0.10.0 does not. A gate that took whichever
//! ShellCheck was on PATH would be green on one machine and red on the next about the same bytes, so
//! [`tool_refusal`](crate::shellcheck::tool_refusal) refuses every version but the one
//! [`PIN`](crate::shellcheck::PIN) names — and refuses a machine with none, which is register item
//! 403's answer for a rule whose only tool is missing.

use std::fmt;
use std::path::Path;
use std::process::Command;

/// The file that pins the version and its release digests, relative to the workspace root.
pub const PIN: &str = "crates/sprag-gate/tools/shellcheck.pin";

/// The installer that reads [`PIN`], relative to the workspace root. Every refusal about the tool
/// names it, because it is the one route by which CI and a person get the same checker.
pub const INSTALLER: &str = "crates/sprag-gate/tools/install-shellcheck";

/// The project's ShellCheck configuration, relative to the workspace root.
pub const RC: &str = ".shellcheckrc";

/// The keys [`RC`] may set. [`rc_refusals`] refuses any other, because a `disable=` in that file
/// would exempt every script in the tree with no line beside the code it excuses to say why.
pub const RC_KEYS: [&str; 2] = ["external-sources", "source-path"];

/// The version [`PIN`] declares, from its one `version = "X.Y.Z"` line.
///
/// # Errors
///
/// When the pin has no such line, or more than one: a pin that names two versions pins neither.
pub fn pinned_version(pin: &str) -> Result<String, String> {
    let declared: Vec<&str> = pin
        .lines()
        .filter_map(|line| line.strip_prefix("version = \""))
        .filter_map(|rest| rest.strip_suffix('"'))
        .filter(|version| !version.is_empty())
        .collect();
    match declared.as_slice() {
        [version] => Ok((*version).to_owned()),
        [] => Err(format!("{PIN} declares no `version = \"X.Y.Z\"` line")),
        more => Err(format!(
            "{PIN} declares {} versions ({more:?}), so it pins none of them",
            more.len(),
        )),
    }
}

/// The version `shellcheck --version` reported, from its `version: X.Y.Z` line, or [`None`] where
/// the output carries no such line.
#[must_use]
pub fn version_said(output: &str) -> Option<String> {
    output
        .lines()
        .find_map(|line| line.strip_prefix("version:"))
        .map(|version| version.trim().to_owned())
        .filter(|version| !version.is_empty())
}

/// Why the `shellcheck` a probe reached cannot judge this tree, or [`None`] when it is the pinned
/// one.
///
/// `probe` is what running [`version_probe`] came to: the error kind when it could not be started,
/// or its exit status and everything it printed.
#[must_use]
pub fn tool_refusal(
    probe: Result<(Option<i32>, String), std::io::ErrorKind>,
    pinned: &str,
) -> Option<String> {
    let install = format!(
        "install the pinned one with `bash {INSTALLER} <directory>` and put that directory on PATH"
    );
    match probe {
        Err(std::io::ErrorKind::NotFound) => Some(format!(
            "ShellCheck is not on PATH, and every shell script in this tree is checked by it. A \
             rule whose only tool is missing refuses rather than passes (register item 403): \
             {install}"
        )),
        Err(kind) => Some(format!(
            "`shellcheck --version` could not be started ({kind}): {install}"
        )),
        Ok((status, said)) if status != Some(0) => Some(format!(
            "`shellcheck --version` exited {status:?}, saying: {}",
            said.trim_end(),
        )),
        Ok((_, said)) => match version_said(&said) {
            None => Some(format!(
                "`shellcheck --version` reported no version, saying: {}",
                said.trim_end(),
            )),
            Some(found) if found == pinned => None,
            Some(found) => Some(format!(
                "the ShellCheck on PATH is {found} and this tree is checked against {pinned} \
                 ({PIN}). Findings differ between versions — over this tree 0.10.0 and 0.11.0 \
                 disagree on twelve — so another version's verdict is not this gate's: {install}"
            )),
        },
    }
}

/// One finding, read from a line of ShellCheck's `--format=gcc` output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// The script, spelled as ShellCheck was handed it.
    pub file: String,
    /// One-indexed.
    pub line: usize,
    /// One-indexed.
    pub column: usize,
    /// `error`, `warning` or `note`: the gcc format folds ShellCheck's `info` and `style` into
    /// `note`.
    pub severity: String,
    /// The check's code, `SC` and its number.
    pub code: String,
    /// ShellCheck's own sentence.
    pub message: String,
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}:{}: {}: {} [{}]",
            self.file, self.line, self.column, self.severity, self.message, self.code,
        )
    }
}

/// The finding one line of `--format=gcc` output states, or [`None`] where the line is not one.
///
/// The shape is `file:line:column: severity: message [SCnnnn]`. A path holding a `:` reads as
/// not-a-finding, and [`verdict`] turns that into a refusal: the direction that costs a red rather
/// than a pass.
#[must_use]
pub fn finding_of(line: &str) -> Option<Finding> {
    let mut fields = line.splitn(4, ':');
    let file = fields.next()?.to_owned();
    let line_number = fields.next()?.parse::<usize>().ok()?;
    let column = fields.next()?.parse::<usize>().ok()?;
    let (severity, tail) = fields.next()?.strip_prefix(' ')?.split_once(": ")?;
    if !matches!(severity, "error" | "warning" | "note") {
        return None;
    }
    let (message, code) = tail.strip_suffix(']')?.rsplit_once(" [")?;
    let number = code.strip_prefix("SC")?;
    if file.is_empty() || number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(Finding {
        file,
        line: line_number,
        column,
        severity: severity.to_owned(),
        code: code.to_owned(),
        message: message.to_owned(),
    })
}

/// What one ShellCheck run over a population came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Exit 0 and nothing printed on either stream.
    Clean,
    /// Exit 1, nothing on standard error, and every line printed is a finding.
    Findings(Vec<Finding>),
    /// Anything else. The run cannot be believed either way, and the sentence says why.
    Unread(String),
}

/// The verdict a run's exit status and output come to.
///
/// ShellCheck documents its statuses: 0 every file was checked and nothing found, 1 every file was
/// checked and something found, 2 a file could not be processed, 3 bad syntax on its command line,
/// 4 bad options. Only the first two are verdicts about scripts. Every other shape — a 0 that
/// printed something, a 1 that printed nothing readable, anything on standard error — is
/// [`Verdict::Unread`], because a shape nobody classified is a red here and never a pass.
#[must_use]
pub fn verdict(status: Option<i32>, stdout: &str, stderr: &str) -> Verdict {
    let printed: Vec<&str> = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let complaint = stderr.trim();
    let everything = [stdout.trim(), complaint]
        .into_iter()
        .filter(|said| !said.is_empty())
        .collect::<Vec<_>>()
        .join(" | ");
    match status {
        Some(0 | 1) if !complaint.is_empty() => Verdict::Unread(format!(
            "exited {status:?} and wrote to standard error, which a run that checked every file \
             does not: {complaint}"
        )),
        Some(0) if printed.is_empty() => Verdict::Clean,
        Some(0) => Verdict::Unread(format!(
            "exited 0, ShellCheck's word for nothing found, and printed {} line(s): {}",
            printed.len(),
            printed.join(" | "),
        )),
        Some(1) if printed.is_empty() => Verdict::Unread(
            "exited 1, ShellCheck's word for something found, and printed no finding".to_owned(),
        ),
        Some(1) => {
            let mut findings = Vec::with_capacity(printed.len());
            for line in printed {
                match finding_of(line) {
                    Some(finding) => findings.push(finding),
                    None => {
                        return Verdict::Unread(format!(
                            "exited 1 and printed a line that is not a finding: {line}"
                        ));
                    }
                }
            }
            Verdict::Findings(findings)
        }
        Some(2) => Verdict::Unread(format!(
            "exited 2: a file could not be processed, so the others' silence covers nothing about \
             it: {everything}"
        )),
        Some(3) => Verdict::Unread(format!(
            "exited 3: its command line has bad syntax, so it checked nothing: {everything}"
        )),
        Some(4) => Verdict::Unread(format!(
            "exited 4: it was given bad options, so it checked nothing: {everything}"
        )),
        Some(other) => Verdict::Unread(format!(
            "exited {other}, a status ShellCheck does not document: {everything}"
        )),
        None => Verdict::Unread(format!(
            "was ended by a signal before it could say anything a verdict rests on: {everything}"
        )),
    }
}

/// One way a script's own text switches the checker off, found by [`hatches_in`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hatch {
    /// `disable=all`: every check, for whatever follows.
    DisablesAll {
        /// The script.
        file: String,
        /// One-indexed.
        line: usize,
    },
    /// A `disable=` before the file's first command, which ShellCheck applies to the whole file.
    CoversTheWholeFile {
        /// The script.
        file: String,
        /// One-indexed.
        line: usize,
    },
    /// A `disable=` whose line says nothing about why.
    GivesNoReason {
        /// The script.
        file: String,
        /// One-indexed.
        line: usize,
    },
}

impl fmt::Display for Hatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DisablesAll { file, line } => write!(
                f,
                "{file}:{line}: `disable=all` switches every check off for what follows — name the \
                 checks being excused instead",
            ),
            Self::CoversTheWholeFile { file, line } => write!(
                f,
                "{file}:{line}: this `disable=` stands before the file's first command, where \
                 ShellCheck applies it to the WHOLE file — put it on the line above the command it \
                 excuses",
            ),
            Self::GivesNoReason { file, line } => write!(
                f,
                "{file}:{line}: this `disable=` gives no reason — say why on the same line, after a \
                 second `#`",
            ),
        }
    }
}

/// A ShellCheck directive: its `key=value` words, and whether a `#` reason follows them.
struct Directive {
    settings: Vec<(String, String)>,
    reason: bool,
}

/// The directive a line of shell carries, or [`None`] where the line is not one.
///
/// ShellCheck reads `#shellcheck` with no space as a directive too (measured 2026-09-13: a
/// `#shellcheck disable=SC2086` silenced the finding), so this does.
fn directive_of(line: &str) -> Option<Directive> {
    let body = line.trim_start().strip_prefix('#')?.trim_start();
    let rest = body.strip_prefix("shellcheck")?;
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let (settings, reason) = match rest.split_once('#') {
        Some((settings, reason)) => (settings, !reason.trim().is_empty()),
        None => (rest, false),
    };
    Some(Directive {
        settings: settings
            .split_whitespace()
            .filter_map(|word| word.split_once('='))
            .map(|(key, value)| (key.to_owned(), value.to_owned()))
            .collect(),
        reason,
    })
}

/// The one-indexed lines of `text` that carry a `disable=` directive.
#[must_use]
pub fn disables(text: &str) -> Vec<usize> {
    text.lines()
        .enumerate()
        .filter_map(|(index, line)| {
            directive_of(line)
                .filter(|directive| directive.settings.iter().any(|(key, _)| key == "disable"))
                .map(|_| index + 1)
        })
        .collect()
}

/// Every [`Hatch`] in one script's text.
///
/// A text scan and not a shell parser: a directive-shaped line inside a heredoc or a multi-line
/// string reads as a directive, and a line inside one reads as a command. The first errs toward a
/// refusal; the second can only make a head-of-file `disable=` look scoped when a string opened
/// above it, which no script in this tree does before its first command.
#[must_use]
pub fn hatches_in(file: &str, text: &str) -> Vec<Hatch> {
    let mut found = Vec::new();
    let mut seen_command = false;
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !trimmed.starts_with('#') {
            seen_command = true;
            continue;
        }
        let Some(directive) = directive_of(trimmed) else {
            continue;
        };
        let disabled: Vec<&str> = directive
            .settings
            .iter()
            .filter(|(key, _)| key == "disable")
            .flat_map(|(_, codes)| codes.split(','))
            .map(str::trim)
            .collect();
        if disabled.is_empty() {
            continue;
        }
        let line = index + 1;
        if disabled.contains(&"all") {
            found.push(Hatch::DisablesAll {
                file: file.to_owned(),
                line,
            });
        }
        if !seen_command {
            found.push(Hatch::CoversTheWholeFile {
                file: file.to_owned(),
                line,
            });
        }
        if !directive.reason {
            found.push(Hatch::GivesNoReason {
                file: file.to_owned(),
                line,
            });
        }
    }
    found
}

/// Why [`RC`]'s text holds more than the options this tree agreed on, one sentence per line that
/// does.
#[must_use]
pub fn rc_refusals(text: &str) -> Vec<String> {
    text.lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let setting = line.trim();
            if setting.is_empty() || setting.starts_with('#') {
                return None;
            }
            let at = index + 1;
            match setting.split_once('=') {
                Some((key, _)) if RC_KEYS.contains(&key.trim()) => None,
                Some((key, _)) => Some(format!(
                    "{RC}:{at}: `{}` is not one of {RC_KEYS:?}. This file applies to every script \
                     in the tree at once, so an exemption here has no line beside the code it \
                     excuses — put a `disable=` above that command, with its reason",
                    key.trim(),
                )),
                None => Some(format!(
                    "{RC}:{at}: `{setting}` is not a `key=value` setting, so what it does to the \
                     verdict is unread"
                )),
            }
        })
        .collect()
}

/// The ShellCheck run the gate makes over `files`, standing in `root`.
///
/// Two lines here are the gate's own defence and a unit test holds each: `SHELLCHECK_OPTS` is
/// removed, and [`RC`] is named with `--rcfile`. Without either, an environment variable or a
/// configuration file nearer the script changes the verdict (measured — see this module's doc).
#[must_use]
pub fn invocation(root: &Path, files: &[String]) -> Command {
    let mut run = Command::new("shellcheck");
    run.current_dir(root)
        .env_remove("SHELLCHECK_OPTS")
        .arg(format!("--rcfile={}", root.join(RC).display()))
        .arg("--format=gcc")
        .arg("--severity=style")
        .args(files);
    run
}

/// `shellcheck --version`, under [`invocation`]'s rule about the environment.
#[must_use]
pub fn version_probe() -> Command {
    let mut probe = Command::new("shellcheck");
    probe.env_remove("SHELLCHECK_OPTS").arg("--version");
    probe
}

#[cfg(test)]
mod tests {
    use super::{
        Hatch, INSTALLER, RC, Verdict, disables, finding_of, hatches_in, invocation,
        pinned_version, rc_refusals, tool_refusal, verdict, version_said,
    };
    use std::ffi::OsStr;
    use std::path::Path;

    const REAL_FINDING: &str = ".githooks/pre-push:340:9: note: Double quote to prevent globbing \
                                and word splitting. [SC2086]";

    #[test]
    fn a_pin_names_exactly_one_version() {
        assert_eq!(
            pinned_version("# comment\nversion = \"0.11.0\"\nlinux_x86_64 = \"ab\"\n"),
            Ok("0.11.0".to_owned()),
        );
        assert!(
            pinned_version("linux_x86_64 = \"ab\"\n")
                .expect_err("a pin with no version pins nothing")
                .contains("no `version"),
        );
        assert!(
            pinned_version("version = \"0.10.0\"\nversion = \"0.11.0\"\n")
                .expect_err("a pin naming two versions pins neither")
                .contains("2 versions"),
        );
    }

    #[test]
    fn the_version_is_read_off_its_own_line() {
        assert_eq!(
            version_said(
                "ShellCheck - shell script analysis tool\nversion: 0.11.0\nlicense: GPL\n"
            ),
            Some("0.11.0".to_owned()),
        );
        assert_eq!(
            version_said("ShellCheck - shell script analysis tool\n"),
            None
        );
        assert_eq!(version_said("version: \n"), None);
    }

    /// Every shape a probe can come to, because a healthy machine produces exactly one of them.
    #[test]
    fn the_tool_is_refused_unless_it_is_the_pinned_one() {
        let pinned = "0.11.0";
        let said = |version: &str| format!("ShellCheck\nversion: {version}\n");
        assert_eq!(tool_refusal(Ok((Some(0), said(pinned))), pinned), None);

        let missing = tool_refusal(Err(std::io::ErrorKind::NotFound), pinned)
            .expect("a machine with no ShellCheck must be refused, never passed (item 403)");
        assert!(
            missing.contains("item 403") && missing.contains(INSTALLER),
            "the refusal must name the rule and the installer: {missing}",
        );
        let other = tool_refusal(Ok((Some(0), said("0.10.0"))), pinned)
            .expect("another version's findings are not this gate's verdict");
        assert!(
            other.contains("0.10.0") && other.contains(pinned) && other.contains(INSTALLER),
            "the refusal must name both versions and the installer: {other}",
        );
        assert!(
            tool_refusal(Ok((Some(2), said(pinned))), pinned)
                .expect("a probe that failed cannot vouch for the tool")
                .contains("exited Some(2)"),
        );
        assert!(
            tool_refusal(Ok((Some(0), "ShellCheck\n".to_owned())), pinned)
                .expect("a probe that named no version cannot vouch for it")
                .contains("reported no version"),
        );
        assert!(
            tool_refusal(Err(std::io::ErrorKind::PermissionDenied), pinned)
                .expect("a tool that cannot be started judges nothing")
                .contains("could not be started"),
        );
    }

    #[test]
    fn a_gcc_line_is_a_finding_and_nothing_else_is() {
        let finding = finding_of(REAL_FINDING).expect("the measured line is a finding");
        assert_eq!(finding.file, ".githooks/pre-push");
        assert_eq!((finding.line, finding.column), (340, 9));
        assert_eq!(finding.severity, "note");
        assert_eq!(finding.code, "SC2086");
        assert_eq!(finding.to_string(), REAL_FINDING);
        for not_one in [
            "",
            "In .githooks/pre-push line 340:",
            ".githooks/pre-push:340: note: no column [SC2086]",
            ".githooks/pre-push:340:9: info: a severity gcc never prints [SC2086]",
            ".githooks/pre-push:340:9: note: no code",
            ".githooks/pre-push:340:9: note: a code that is not one [SCx]",
        ] {
            assert_eq!(finding_of(not_one), None, "{not_one:?} is not a finding");
        }
    }

    /// Every documented status and the undocumented shapes around them. The population only ever
    /// produces the first two, which is why they are driven here.
    #[test]
    fn only_a_quiet_zero_is_clean_and_only_a_readable_one_is_findings() {
        assert_eq!(verdict(Some(0), "", ""), Verdict::Clean);
        assert_eq!(
            verdict(Some(1), &format!("{REAL_FINDING}\n"), ""),
            Verdict::Findings(vec![finding_of(REAL_FINDING).expect("a finding")]),
        );
        let unread = |status: Option<i32>, stdout: &str, stderr: &str, words: &str| match verdict(
            status, stdout, stderr,
        ) {
            Verdict::Unread(why) => assert!(
                why.contains(words),
                "{status:?}/{stdout:?}/{stderr:?} must say {words:?}: {why}",
            ),
            other => panic!(
                "{status:?}/{stdout:?}/{stderr:?} is not a verdict about scripts, got {other:?}"
            ),
        };
        unread(Some(0), REAL_FINDING, "", "printed 1 line(s)");
        unread(Some(0), "", "a complaint", "standard error");
        unread(Some(1), "", "", "printed no finding");
        unread(Some(1), REAL_FINDING, "a complaint", "standard error");
        unread(
            Some(1),
            &format!("{REAL_FINDING}\nsomething else\n"),
            "",
            "not a finding: something else",
        );
        unread(
            Some(2),
            "",
            "x.sh: does not exist",
            "could not be processed",
        );
        unread(Some(3), "", "", "bad syntax");
        unread(Some(4), "", "", "bad options");
        unread(Some(7), "", "", "does not document");
        unread(None, "", "", "signal");
    }

    #[test]
    fn a_disable_is_refused_where_it_reaches_too_far_or_says_nothing() {
        let file = "x.sh";
        assert_eq!(
            hatches_in(
                file,
                "#!/bin/sh\nset -eu\n# shellcheck disable=SC2086  # a word-split list\nf $a\n"
            ),
            Vec::<Hatch>::new(),
            "a scoped disable with its reason is the shape this tree keeps",
        );
        assert_eq!(
            hatches_in(
                file,
                "#!/bin/sh\nset -eu\n# shellcheck disable=all  # quiet\nf $a\n"
            ),
            vec![Hatch::DisablesAll {
                file: file.to_owned(),
                line: 3
            }],
        );
        assert_eq!(
            hatches_in(
                file,
                "#!/bin/sh\n# shellcheck disable=SC2086,all  # quiet\nset -eu\n"
            ),
            vec![
                Hatch::DisablesAll {
                    file: file.to_owned(),
                    line: 2
                },
                Hatch::CoversTheWholeFile {
                    file: file.to_owned(),
                    line: 2
                },
            ],
        );
        assert_eq!(
            hatches_in(
                file,
                "#!/bin/sh\n# A header.\n\n# shellcheck disable=SC2086  # why\nf $a\n"
            ),
            vec![Hatch::CoversTheWholeFile {
                file: file.to_owned(),
                line: 4
            }],
            "measured: a comment block between the shebang and the directive does not scope it",
        );
        assert_eq!(
            hatches_in(file, "set -eu\n#shellcheck disable=SC2086\nf $a\n"),
            vec![Hatch::GivesNoReason {
                file: file.to_owned(),
                line: 2
            }],
            "measured: `#shellcheck` with no space is a directive to ShellCheck",
        );
        assert_eq!(
            hatches_in(file, "set -eu\n# shellcheck disable=SC2086 #\nf $a\n"),
            vec![Hatch::GivesNoReason {
                file: file.to_owned(),
                line: 2
            }],
            "an empty `#` is not a reason",
        );
        assert_eq!(
            hatches_in(
                file,
                "# shellcheck shell=sh\n# shellcheck source=/dev/null\n. ./tail.sh\n"
            ),
            Vec::<Hatch>::new(),
            "only `disable=` switches a check off; `shell=` and `source=` at the head are not \
             exemptions",
        );
        assert_eq!(
            disables("set -eu\n# shellcheck disable=SC2086  # a\nf\n#shellcheck disable=SC2034\n"),
            vec![2, 4],
        );
        assert_eq!(
            disables("#!/bin/sh\n# shellcheck is discussed here\n"),
            Vec::<usize>::new()
        );
    }

    #[test]
    fn the_project_configuration_holds_only_the_agreed_options() {
        assert_eq!(
            rc_refusals("# why\nexternal-sources=true\n\nsource-path=SCRIPTDIR\n"),
            Vec::<String>::new(),
        );
        let refused = rc_refusals("external-sources=true\ndisable=SC2086\nnonsense\n");
        assert_eq!(refused.len(), 2, "{refused:?}");
        assert!(
            refused[0].starts_with(&format!("{RC}:2: `disable`")),
            "{refused:?}"
        );
        assert!(refused[1].contains("not a `key=value`"), "{refused:?}");
    }

    /// The two defences [`invocation`] exists for, read off the command it builds.
    #[test]
    fn the_invocation_is_deaf_to_the_environment_and_names_the_project_configuration() {
        let root = Path::new("/the/root");
        let run = invocation(root, &["a.sh".to_owned()]);
        assert!(
            run.get_envs()
                .any(|(key, value)| key == OsStr::new("SHELLCHECK_OPTS") && value.is_none()),
            "measured: SHELLCHECK_OPTS=--exclude=SC2086 turns a finding into exit 0, so the \
             gate's run must remove it",
        );
        let args: Vec<&OsStr> = run.get_args().collect();
        assert!(
            args.contains(&OsStr::new("--rcfile=/the/root/.shellcheckrc")),
            "the project's configuration must be named, or the nearest one to each script is \
             used instead: {args:?}",
        );
        assert_eq!(args.last(), Some(&OsStr::new("a.sh")));
    }
}
