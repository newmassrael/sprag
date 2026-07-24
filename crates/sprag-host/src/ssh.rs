//! SSH workspaces: first-classing a remote connection as a session whose birth pane runs `ssh`.
//!
//! A session's first pane is just a process ([`sprag_terminal::command`]), so a *remote* workspace
//! is a session whose birth argv is `ssh -t [user@]host [remote-command…]`. There is no new spawn
//! mechanism: the pane is an ordinary PTY child, so reflow/resize (SIGWINCH forwarded by `ssh -t`),
//! scrollback, introspection, and the durability snapshot all apply unchanged. This module is the
//! single home that turns a parsed [`SshTarget`] into that argv, so the CLI (`sprag ssh`) and any
//! future GUI affordance assemble the same command — the argv then rides the existing
//! `new_session {cmd}` action ([`crate::workspace`]) with no wire or daemon change.
//!
//! Durability note (Slice 1): the pane's argv is recorded like any other, but `ssh` is deliberately
//! OUTSIDE the default exact-command restore allowlist ([`crate::durability`]) because re-running an
//! arbitrary `ssh host '<command>'` on restore is a side-effect risk. So a restored ssh workspace
//! comes back as a plain shell today; restoring the *connection* from structured intent (not opaque
//! argv) is a later slice, not a silent default change.

use std::fmt;

/// A parsed SSH destination and the command to run there.
///
/// Built by the CLI from `sprag ssh [user@]host [-p PORT] [-- remote-command…]` via
/// [`SshTarget::from_args`], or directly from a destination via [`SshTarget::parse`]. [`ssh_argv`]
/// is the SSOT that renders it to the process argv.
///
/// [`ssh_argv`]: SshTarget::ssh_argv
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshTarget {
    /// Remote login user (the `user@` of `user@host`), or `None` to let ssh pick it (the local
    /// user, or whatever the user's ssh config resolves for the host).
    pub user: Option<String>,
    /// Remote host — a name or address. Never empty (enforced by [`SshTarget::parse`]).
    pub host: String,
    /// Remote port (`ssh -p`), or `None` for ssh's own default (22, or the host's ssh-config port).
    pub port: Option<u16>,
    /// The command to run on the remote as an argv, or empty to open the remote login shell. Passed
    /// to ssh verbatim after the destination, so it inherits ssh's remote-quoting rules exactly as a
    /// hand-typed `ssh host <command>` would.
    pub remote_command: Vec<String>,
}

/// Why a `sprag ssh …` invocation could not be turned into an [`SshTarget`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SshTargetError {
    /// No `[user@]host` destination was given at all.
    MissingDestination,
    /// The destination parsed to an empty host (`""` or a bare `@`), so there is nowhere to connect.
    EmptyHost,
    /// A `-p`/`--port` flag was the last argument, with no port number after it.
    MissingPortValue,
    /// A `-p`/`--port` value was not a valid TCP port (`1..=65535`).
    BadPort(String),
    /// An extra positional argument appeared after the destination — the remote command must follow
    /// a `--` separator, so a stray token is a mistake rather than a silently-dropped argument.
    UnexpectedArgument(String),
}

impl fmt::Display for SshTargetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingDestination => f.write_str("ssh needs a [user@]host destination"),
            Self::EmptyHost => f.write_str("ssh destination has no host"),
            Self::MissingPortValue => f.write_str("-p needs a port number"),
            Self::BadPort(value) => write!(f, "invalid port {value:?}: expected 1..=65535"),
            Self::UnexpectedArgument(arg) => {
                write!(
                    f,
                    "unexpected argument {arg:?}; put the remote command after --"
                )
            }
        }
    }
}

impl std::error::Error for SshTargetError {}

impl SshTarget {
    /// Parse a `[user@]host` destination into a target with no port and no remote command.
    ///
    /// Splits on the FIRST `@`: `me@server` → user `me`, host `server`; a bare `server` → no user,
    /// host `server`. An SSH login name cannot contain `@`, so the first `@` is unambiguous. An
    /// empty user (`@server`) collapses to no user; an empty host is [`SshTargetError::EmptyHost`].
    ///
    /// # Errors
    ///
    /// [`SshTargetError::EmptyHost`] if the destination has no host part.
    pub fn parse(destination: &str) -> Result<Self, SshTargetError> {
        let (user, host) = match destination.split_once('@') {
            Some((user, host)) => {
                let user = (!user.is_empty()).then(|| user.to_owned());
                (user, host)
            }
            None => (None, destination),
        };
        if host.is_empty() {
            return Err(SshTargetError::EmptyHost);
        }
        Ok(Self {
            user,
            host: host.to_owned(),
            port: None,
            remote_command: Vec::new(),
        })
    }

    /// Parse a whole `sprag ssh` argument list: `[user@]host [-p PORT] [-- remote-command…]`.
    ///
    /// The FIRST non-flag token is the destination; `-p`/`--port` takes the next token as the port;
    /// `--` ends option parsing so everything after it is the remote command VERBATIM (a `-p` after
    /// `--` is a remote argument, not a local flag). Keeping the whole parse here — not in the CLI
    /// binary — makes every branch unit-testable and keeps the binary a thin call site.
    ///
    /// # Errors
    ///
    /// An [`SshTargetError`] for a missing/empty destination, a missing or malformed port, or a
    /// stray positional argument before `--`.
    pub fn from_args<I>(args: I) -> Result<Self, SshTargetError>
    where
        I: IntoIterator<Item = String>,
    {
        let mut destination: Option<String> = None;
        let mut port: Option<u16> = None;
        let mut remote_command: Vec<String> = Vec::new();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "-p" | "--port" => {
                    let value = args.next().ok_or(SshTargetError::MissingPortValue)?;
                    // A `0` parses as a `u16` but is not a usable TCP port, so reject it with the
                    // same message a non-numeric value gets.
                    let parsed = value.parse::<u16>().ok().filter(|p| *p != 0);
                    port = Some(parsed.ok_or(SshTargetError::BadPort(value))?);
                }
                "--" => {
                    remote_command.extend(args.by_ref());
                    break;
                }
                _ if destination.is_none() => destination = Some(arg),
                _ => return Err(SshTargetError::UnexpectedArgument(arg)),
            }
        }
        let destination = destination.ok_or(SshTargetError::MissingDestination)?;
        let mut target = Self::parse(&destination)?;
        target.port = port;
        target.remote_command = remote_command;
        Ok(target)
    }

    /// The ssh destination argument (`user@host`, or just `host` when no user is set).
    #[must_use]
    pub fn destination(&self) -> String {
        match &self.user {
            Some(user) => format!("{user}@{}", self.host),
            None => self.host.clone(),
        }
    }

    /// Build the process argv: `ssh -t [-p PORT] DEST [remote-command…]`.
    ///
    /// `-t` forces remote pseudo-terminal allocation. The pane already IS a PTY, so a login shell or
    /// a remote program (an editor, `claude`, `tmux attach`) gets a real controlling terminal and
    /// receives SIGWINCH when the pane resizes — the reason a first-classed ssh workspace behaves
    /// like a local one. Without `-t`, ssh runs a remote *command* with no TTY, which breaks every
    /// full-screen program; a single `-t` (not `-tt`) suffices because the pane's stdin is a real
    /// terminal.
    #[must_use]
    pub fn ssh_argv(&self) -> Vec<String> {
        let mut argv = vec!["ssh".to_owned(), "-t".to_owned()];
        if let Some(port) = self.port {
            argv.push("-p".to_owned());
            argv.push(port.to_string());
        }
        argv.push(self.destination());
        argv.extend(self.remote_command.iter().cloned());
        argv
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| (*part).to_owned()).collect()
    }

    #[test]
    fn parse_splits_the_user_from_the_host() {
        let target = SshTarget::parse("me@server").expect("valid destination");
        assert_eq!(target.user.as_deref(), Some("me"));
        assert_eq!(target.host, "server");
    }

    #[test]
    fn parse_a_bare_host_has_no_user() {
        let target = SshTarget::parse("server").expect("valid destination");
        assert_eq!(target.user, None);
        assert_eq!(target.host, "server");
    }

    #[test]
    fn parse_an_empty_leading_user_collapses_to_none() {
        let target = SshTarget::parse("@server").expect("valid destination");
        assert_eq!(target.user, None);
        assert_eq!(target.host, "server");
    }

    #[test]
    fn parse_rejects_a_hostless_destination() {
        // Revert-proof for the EmptyHost guard: both an empty string and a bare `@` must fail.
        assert_eq!(SshTarget::parse(""), Err(SshTargetError::EmptyHost));
        assert_eq!(SshTarget::parse("@"), Err(SshTargetError::EmptyHost));
        assert_eq!(SshTarget::parse("me@"), Err(SshTargetError::EmptyHost));
    }

    #[test]
    fn ssh_argv_for_a_bare_host_is_ssh_dash_t_host() {
        // Revert-proof for the forced-TTY `-t`: dropping it makes this exact-match fail.
        let target = SshTarget::parse("server").unwrap();
        assert_eq!(target.ssh_argv(), strings(&["ssh", "-t", "server"]));
    }

    #[test]
    fn ssh_argv_renders_user_port_and_remote_command() {
        let target = SshTarget {
            user: Some("me".to_owned()),
            host: "server".to_owned(),
            port: Some(2222),
            remote_command: strings(&["tmux", "attach"]),
        };
        assert_eq!(
            target.ssh_argv(),
            strings(&["ssh", "-t", "-p", "2222", "me@server", "tmux", "attach"]),
        );
    }

    #[test]
    fn from_args_takes_the_destination_only() {
        let target = SshTarget::from_args(strings(&["me@server"])).expect("valid");
        assert_eq!(target.user.as_deref(), Some("me"));
        assert_eq!(target.host, "server");
        assert_eq!(target.port, None);
        assert!(target.remote_command.is_empty());
    }

    #[test]
    fn from_args_reads_a_port_and_a_remote_command() {
        let target =
            SshTarget::from_args(strings(&["host", "-p", "2222", "--", "tmux", "attach"])).unwrap();
        assert_eq!(target.port, Some(2222));
        assert_eq!(target.remote_command, strings(&["tmux", "attach"]));
        assert_eq!(target.host, "host");
    }

    #[test]
    fn from_args_accepts_the_long_port_flag() {
        let target = SshTarget::from_args(strings(&["host", "--port", "22"])).unwrap();
        assert_eq!(target.port, Some(22));
    }

    #[test]
    fn from_args_after_the_separator_is_verbatim_not_reparsed() {
        // Revert-proof for the `--` break: a `-p` AFTER `--` is a remote argument, never a local
        // port flag. Removing the `--` arm reparses it and this fails (BadPort / wrong argv).
        let target =
            SshTarget::from_args(strings(&["host", "--", "vim", "-p", "note.txt"])).unwrap();
        assert_eq!(target.port, None);
        assert_eq!(target.remote_command, strings(&["vim", "-p", "note.txt"]));
    }

    #[test]
    fn from_args_needs_a_destination() {
        assert_eq!(
            SshTarget::from_args(strings(&["-p", "22"])),
            Err(SshTargetError::MissingDestination),
        );
    }

    #[test]
    fn from_args_rejects_a_dangling_port_flag() {
        assert_eq!(
            SshTarget::from_args(strings(&["host", "-p"])),
            Err(SshTargetError::MissingPortValue),
        );
    }

    #[test]
    fn from_args_rejects_a_non_numeric_or_zero_port() {
        assert_eq!(
            SshTarget::from_args(strings(&["host", "-p", "nope"])),
            Err(SshTargetError::BadPort("nope".to_owned())),
        );
        assert_eq!(
            SshTarget::from_args(strings(&["host", "-p", "0"])),
            Err(SshTargetError::BadPort("0".to_owned())),
        );
    }

    #[test]
    fn from_args_rejects_a_stray_positional_argument() {
        assert_eq!(
            SshTarget::from_args(strings(&["host", "extra"])),
            Err(SshTargetError::UnexpectedArgument("extra".to_owned())),
        );
    }
}
