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
    fn default_shell_command_labels_the_program_that_runs() {
        let (_command, label) = default_shell_command();
        // $SHELL when set, else /bin/sh -- either way the label is the program.
        assert!(!label.is_empty());
    }
}
