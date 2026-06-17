//! `sprag-term` — a headless terminal RPC server.
//!
//! Spawns a shell (or the command given after `--`) on a pseudoterminal and
//! serves pinion's `scene/snapshot` (and the other static-scene read
//! methods) over stdin/stdout — one JSON-RPC request per line. The terminal
//! is exposed as data, with no GPU and no window (DESIGN.md §1/§3).
//!
//! ```text
//! sprag-term [--size COLSxROWS] [-- <program> [args...]]
//! ```
//!
//! With no command, runs `$SHELL` (falling back to `/bin/sh`). Each line on
//! stdin is one JSON-RPC request; each response is one line on stdout.

use std::io;

use sprag_terminal::{serve, CommandBuilder, TerminalSession};

fn main() -> io::Result<()> {
    let (cols, rows, command) = parse_args();
    let mut session = TerminalSession::spawn(command, cols, rows).map_err(io::Error::other)?;
    let stdin = io::stdin();
    let stdout = io::stdout();
    serve(&mut session, stdin.lock(), stdout.lock())
}

/// Parse `[--size COLSxROWS]` then an optional command (after `--`, or the
/// first bare argument). Falls back to `$SHELL` at 80x24.
fn parse_args() -> (u16, u16, CommandBuilder) {
    let mut cols: u16 = 80;
    let mut rows: u16 = 24;
    let mut args = std::env::args().skip(1);
    let mut command: Option<CommandBuilder> = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--size" => {
                if let Some((w, h)) = args.next().as_deref().and_then(parse_size) {
                    cols = w;
                    rows = h;
                }
            }
            "--" => {
                if let Some(program) = args.next() {
                    command = Some(build_command(program, &mut args));
                }
                break;
            }
            _ => {
                command = Some(build_command(arg, &mut args));
                break;
            }
        }
    }

    (cols, rows, command.unwrap_or_else(default_shell))
}

/// Parse a `COLSxROWS` size specifier.
fn parse_size(spec: &str) -> Option<(u16, u16)> {
    let (w, h) = spec.split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

/// Build a command from a program plus the remaining argv, setting a sane
/// `TERM` for the child (the rest of the environment is inherited).
fn build_command(program: String, rest: &mut impl Iterator<Item = String>) -> CommandBuilder {
    let mut command = CommandBuilder::new(program);
    for arg in rest {
        command.arg(arg);
    }
    command.env("TERM", "xterm-256color");
    command
}

/// The default child: `$SHELL`, or `/bin/sh` when it is unset.
fn default_shell() -> CommandBuilder {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let mut command = CommandBuilder::new(shell);
    command.env("TERM", "xterm-256color");
    command
}
