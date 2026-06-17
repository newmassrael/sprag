//! `sprag-term` — a headless terminal multiplexer RPC server.
//!
//! Starts a workspace with one initial pane (a shell, or the command after
//! `--`) on a pseudoterminal and serves pinion's scene-as-data wire over
//! stdin/stdout — one JSON-RPC request per line, no GPU and no window
//! (DESIGN.md §1/§3). An AI peer reads the panes via `scene/snapshot` /
//! `scene/query`, injects input per pane via `scene/invoke`
//! (`/pane_<id>/sprag_input/external/key`), and manages panes via the
//! workspace control surface (`/sprag_mux/external/{spawn,close,resize}`).
//!
//! ```text
//! sprag-term [--size COLSxROWS] [-- <program> [args...]]
//! ```
//!
//! With no command, the initial pane runs `$SHELL` (falling back to
//! `/bin/sh`).

use std::io;
use std::sync::{Arc, Mutex, PoisonError};

use sprag_host::serve;
use sprag_terminal::{CommandBuilder, Workspace};

fn main() -> io::Result<()> {
    let (cols, rows, command, label) = parse_args();
    let workspace = Arc::new(Mutex::new(Workspace::new((cols, rows))));
    workspace
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .spawn(command, label, cols, rows)
        .map_err(io::Error::other)?;
    let stdin = io::stdin();
    let stdout = io::stdout();
    serve(&workspace, stdin.lock(), stdout.lock())
}

/// Parse `[--size COLSxROWS]` then an optional command (after `--`, or the
/// first bare argument). Falls back to `$SHELL` at 80x24.
fn parse_args() -> (u16, u16, CommandBuilder, String) {
    let mut cols: u16 = 80;
    let mut rows: u16 = 24;
    let mut args = std::env::args().skip(1);
    let mut command: Option<(CommandBuilder, String)> = None;

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

    let (command, label) = command.unwrap_or_else(default_shell);
    (cols, rows, command, label)
}

/// Parse a `COLSxROWS` size specifier.
fn parse_size(spec: &str) -> Option<(u16, u16)> {
    let (w, h) = spec.split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

/// Build a command from a program plus the remaining argv (setting a sane
/// `TERM`; the rest of the environment is inherited), plus the program label
/// the workspace records for the pane.
fn build_command(
    program: String,
    rest: &mut impl Iterator<Item = String>,
) -> (CommandBuilder, String) {
    let label = program.clone();
    let mut command = CommandBuilder::new(program);
    for arg in rest {
        command.arg(arg);
    }
    command.env("TERM", "xterm-256color");
    (command, label)
}

/// The default child: `$SHELL`, or `/bin/sh` when it is unset, plus its label.
fn default_shell() -> (CommandBuilder, String) {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let mut command = CommandBuilder::new(&shell);
    command.env("TERM", "xterm-256color");
    (command, shell)
}
