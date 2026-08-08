//! The single home for "how a pane process is spawned".
//!
//! Building a pane's [`CommandBuilder`] is the same everywhere: set
//! `TERM=xterm-256color` (the rest of the environment is inherited by the
//! child), append the arguments, and record the program string as the pane's
//! introspection label. Every frontend resolves its own *spec* from a
//! different source -- the windowed GUI reads `SPRAG_GUI_CMD`, the headless
//! server parses `--`/argv, the mux spawn control reads a JSON argv array --
//! but the actual command assembly must not be re-encoded per frontend. These
//! two functions are that one assembly site; a frontend keeps only its policy
//! (where the spec comes from) and calls one of these.

use crate::pane_pty::CommandBuilder;

/// Build a pane command from a program and its arguments: a [`CommandBuilder`]
/// with `TERM=xterm-256color` set (the rest of the environment inherited by the
/// child), with the program string returned as the pane's introspection label.
#[must_use]
pub fn command_from_parts<S, I>(program: S, args: I) -> (CommandBuilder, String)
where
    S: AsRef<str>,
    I: IntoIterator,
    I::Item: AsRef<str>,
{
    let program = program.as_ref();
    let mut command = CommandBuilder::new(program);
    for arg in args {
        command.arg(arg.as_ref());
    }
    command.env("TERM", "xterm-256color");
    (command, program.to_owned())
}

/// A pane command from a SHELL COMMAND LINE: `$SHELL -c <line>`, the form tmux's
/// `default-command` takes.
///
/// Handed to the shell whole rather than split into an argv here, and that is the
/// point: a user's `sh -c 'exec top'`, a pipeline or a quoted argument are the
/// SHELL's grammar, and a splitter of our own would be a second, poorer one.
/// Whether the command exists is the shell's answer too — it lands in the pane,
/// where the user can read it.
///
/// The LABEL is the command's own first word rather than the shell's, because a
/// pane running `htop` that introspects as `bash` is a pane a user cannot find. A
/// leading `exec ` is stripped and a path is reduced to its basename, both for the
/// same reason and both what tmux's own window naming does: `exec /usr/bin/htop`
/// is a pane called `htop`.
#[must_use]
pub fn shell_command_line(line: &str) -> (CommandBuilder, String) {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned());
    let (command, _shell_label) = command_from_parts(shell, ["-c", line]);
    (command, command_line_label(line))
}

/// The introspection label for a shell command LINE — see [`shell_command_line`].
///
/// Every step can decline (a line of separators has no basename; `exec` alone
/// leaves no word), so each falls back to what it was given and the last resort is
/// the trimmed line itself. A pane is never labelled with NOTHING unless the line
/// was nothing, which the caller's own vocabulary rules out: an empty
/// `default-command` means "no command", not "a command with no name".
fn command_line_label(line: &str) -> String {
    let line = line.trim();
    let first = line
        .strip_prefix("exec ")
        .map(str::trim_start)
        .filter(|rest| !rest.is_empty())
        .unwrap_or(line)
        .split_whitespace()
        .next()
        .unwrap_or(line);
    std::path::Path::new(first)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(first)
        .to_owned()
}

/// The default pane command: `$SHELL` (or `/bin/sh` when it is unset), no
/// arguments. The fallback every frontend shares when no explicit command spec
/// is given.
#[must_use]
pub fn default_shell_command() -> (CommandBuilder, String) {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned());
    command_from_parts(shell, std::iter::empty::<&str>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_is_the_program_name() {
        let (_command, label) = command_from_parts("bash", ["-i", "-l"]);
        assert_eq!(label, "bash");
    }

    #[test]
    fn accepts_no_arguments() {
        let (_command, label) = command_from_parts("/bin/sh", std::iter::empty::<&str>());
        assert_eq!(label, "/bin/sh");
    }

    #[test]
    fn accepts_owned_string_args() {
        // The headless server passes an owned-`String` argv iterator; the mux
        // passes borrowed `&str`. Both must satisfy the one signature.
        let args = vec!["-c".to_owned(), "echo hi".to_owned()];
        let (_command, label) = command_from_parts("sh", args);
        assert_eq!(label, "sh");
    }

    #[test]
    fn a_shell_command_line_is_labelled_by_what_it_runs() {
        // The label is what a user looks for in a sidebar, so it must be the COMMAND rather than the
        // shell that carries it.
        assert_eq!(command_line_label("htop"), "htop");
        assert_eq!(command_line_label("  htop -d 5 "), "htop");
        assert_eq!(command_line_label("exec htop"), "htop", "exec is stripped");
        assert_eq!(
            command_line_label("exec /usr/bin/htop"),
            "htop",
            "and a path is reduced to its basename"
        );
        assert_eq!(
            command_line_label("sh -c 'exec top'"),
            "sh",
            "the outer program is the honest label when the user spelled one"
        );
        // Never NOTHING: every step declines to what it was given, so a line with no basename and a
        // line whose only word is `exec` both still name the pane. Written from what the code answers
        // rather than from what the doc claims — the first spelling of this test asserted `""` for
        // `///` and would have let an EMPTY label through for `exec `.
        assert_eq!(command_line_label("///"), "///");
        assert_eq!(command_line_label("exec "), "exec");
        assert_eq!(command_line_label("exec  htop"), "htop");
    }

    #[test]
    fn a_shell_command_line_runs_through_the_shell() {
        let (_command, label) = shell_command_line("htop");
        assert_eq!(label, "htop");
    }

    #[test]
    fn default_shell_command_labels_the_program_that_runs() {
        let (_command, label) = default_shell_command();
        // $SHELL when set, else /bin/sh -- either way the label is the program.
        assert!(!label.is_empty());
    }

    /// **Every TABULATION capability this `TERM` promises is one the emulator honours.**
    ///
    /// The two halves of a promise, in one gate. `TERM=xterm-256color` is not a label: it is a
    /// contract with ncurses, `tput` and every curses program, which read that terminfo entry and
    /// emit exactly the byte strings below. Until R333 sprag honoured only `ht` — a child that took
    /// the terminal at its word and sent `cbt`, `hts` or `tbc` got no refusal and no effect, so its
    /// cursor ended up somewhere the child believed it was not. An unimplemented sequence is a
    /// missing feature; an unimplemented sequence the terminal ADVERTISES is a wrong answer.
    ///
    /// The capability strings are pinned as literals rather than read from the local terminfo
    /// database, which is a machine fact a test must not depend on. What ties them to reality is the
    /// TERM assertion: change the advertised entry and this reddens, which is the moment to re-read
    /// the new entry's capabilities rather than the moment to discover the mismatch in a user's
    /// pane.
    #[test]
    fn every_tabulation_capability_this_term_advertises_is_one_the_emulator_honours() {
        use sprag_vt::VtPort as _;

        let (command, _label) = command_from_parts("sh", std::iter::empty::<&str>());
        assert_eq!(
            command.get_env("TERM").and_then(|term| term.to_str()),
            Some("xterm-256color"),
            "the advertised entry is what fixes the capability strings below",
        );

        // `infocmp xterm-256color`: ht=^I, hts=\EH, cbt=\E[Z, tbc=\E[3g.
        let mut em = sprag_vt::Emulator::new(40, 4);
        em.advance(b"\t");
        assert_eq!(em.screen().cursor().col, 8, "ht=^I");

        // hts=\EH sets a stop, and the ht above must then find it rather than the fixed grid.
        em.advance(b"\r\x1b[1;4H\x1bH\r\t");
        assert_eq!(em.screen().cursor().col, 3, "hts=\\EH");

        // cbt=\E[Z walks back to the previous stop — here the one hts just set is behind us.
        em.advance(b"\x1b[1;10H\x1b[Z");
        assert_eq!(em.screen().cursor().col, 8, "cbt=\\E[Z");

        // tbc=\E[3g clears every stop, so the next ht runs to the last column.
        em.advance(b"\x1b[3g\r\t");
        assert_eq!(em.screen().cursor().col, 39, "tbc=\\E[3g");
    }
}
