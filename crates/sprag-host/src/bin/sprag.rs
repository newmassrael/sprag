//! `sprag` — the session-management CLI for a running `sprag-term` daemon.
//!
//! ```text
//! sprag ls                 list every session
//! sprag new [name]         create a session with a shell (absent name -> the lowest free), print its name
//! sprag attach NAME        open a sprag-gui window attached to a session (tmux attach-session)
//! sprag kill-session NAME   kill a session (the last one ends the daemon)
//! sprag kill-server        kill every session, ending the daemon
//! ```
//!
//! It drives the daemon over the SAME always-on socket the GUI connect-or-spawns
//! (`$XDG_RUNTIME_DIR/sprag-host.sock`, override `SPRAG_HOST_RPC_SOCK`) via the SAME mux
//! control actions the GUI uses ([`SESSIONS_SLOT`], [`NEW_SESSION_ACTION`],
//! [`KILL_SESSION_ACTION`]) — so there is one wire vocabulary, not a CLI-only one. It only
//! CONNECTS (never spawns a daemon): a management command with no server to manage is a clear
//! error, not a silent daemon start. `attach` is the one command that then launches a display
//! process — `sprag-gui` scoped to the session — but its PRE-FLIGHT (does the session exist?)
//! is the same connect-only check, so a typo is a clean error, not a window that flashes and dies.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};
use sprag_host::mux_action_path;
use sprag_host::wire::{KILL_SESSION_ACTION, NEW_SESSION_ACTION, SESSIONS_SLOT};
use sprag_rpc::{HOST_SOCKET, HostConn, socket_path};

/// A management command is talking to an already-running daemon, so the socket either accepts
/// at once or there is nothing to manage — no spawn-race retry to wait out.
const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);

fn main() {
    if let Err(error) = run() {
        eprintln!("sprag: {error}");
        std::process::exit(1);
    }
}

fn run() -> io::Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("ls") => ls(),
        Some("new") => new(args.next()),
        Some("attach") => attach(args.next()),
        Some("kill-session") => kill_session(args.next()),
        Some("kill-server") => kill_server(),
        Some("-h" | "--help" | "help") | None => {
            print_usage();
            Ok(())
        }
        Some(other) => {
            eprintln!("sprag: unknown command {other:?}");
            print_usage();
            std::process::exit(2);
        }
    }
}

fn print_usage() {
    eprintln!("usage: sprag <ls | new [name] | attach NAME | kill-session NAME | kill-server>");
}

/// Env override: the `sprag-gui` binary [`attach`] launches (else the sibling of this exe — they
/// install together — else `sprag-gui` on `PATH`). Mirrors the GUI's own `SPRAG_GUI_HOST_BIN`
/// discovery of `sprag-term`.
const GUI_BIN_ENV: &str = "SPRAG_GUI_BIN";

/// Connect to the running daemon, mapping a refused connection to a clear "no server" message
/// rather than a raw errno — a management command needs the daemon to already exist.
fn connect() -> io::Result<HostConn> {
    let sock = socket_path(HOST_SOCKET);
    HostConn::connect(&sock, CONNECT_TIMEOUT).map_err(|_| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("no server running at {}", sock.display()),
        )
    })
}

/// `ls`: one line per session — its name, its window count, and which one an unscoped request
/// lands in.
fn ls() -> io::Result<()> {
    let mut conn = connect()?;
    let sessions = conn.call(
        "scene/query",
        json!({ "path": mux_action_path(SESSIONS_SLOT) }),
    )?;
    for session in sessions.as_array().into_iter().flatten() {
        let name = session["name"].as_str().unwrap_or("?");
        let windows = session["windows"].as_u64().unwrap_or(0);
        let marker = if session["default"].as_bool().unwrap_or(false) {
            " (default)"
        } else {
            ""
        };
        println!("{name}: {windows} window(s){marker}");
    }
    Ok(())
}

/// `new [name]`: create a session — born with a shell, tmux's `new-session -d` (the registry
/// allocates the lowest free name when none is given) — and print the name it got, the string to
/// scope a client to. The CLI passes no `cmd`/size, so the birth pane runs the default `$SHELL`.
fn new(name: Option<String>) -> io::Result<()> {
    let mut conn = connect()?;
    let args = match &name {
        Some(name) => json!({ "name": name }),
        None => json!({}),
    };
    let answer = conn.call(
        "scene/invoke",
        json!({ "path": mux_action_path(NEW_SESSION_ACTION), "args": args }),
    );
    match answer {
        Ok(answer) => match answer.as_str() {
            Some(created) => {
                println!("{created}");
                Ok(())
            }
            None => Err(io::Error::other("new did not answer with a name")),
        },
        // The host answers a refused create with a JSON-RPC error (`Other`); the only refusal for
        // an explicitly-named create is a duplicate — say so cleanly, mirroring kill-session.
        Err(error) if error.kind() == io::ErrorKind::Other => {
            let named = name.as_deref().unwrap_or_default();
            Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("a session named {named:?} already exists"),
            ))
        }
        Err(error) => Err(error),
    }
}

/// `attach NAME`: open a `sprag-gui` window attached to session NAME — tmux `attach-session -t`.
///
/// The PRE-FLIGHT is connect-only, like every other command: it verifies NAME exists on the
/// running daemon FIRST, so a typo is a clean "no session" error, not a GUI window that flashes
/// open and dies on its first (failed) scoped read. Only then does it launch `sprag-gui`, handing
/// it `SPRAG_GUI_SESSION=NAME` (the attach env its `resolve_session` consumes → adopt the session's
/// live panes) and `SPRAG_GUI_HOST_SOCK` pinned to the EXACT socket this CLI reached — so the
/// window joins the daemon we just checked, never a different default it might connect-or-spawn.
/// Foreground (tmux's attach holds the terminal until the client leaves); background it with `&`.
fn attach(name: Option<String>) -> io::Result<()> {
    let name = name.ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "attach needs a session name")
    })?;
    let sock = socket_path(HOST_SOCKET);
    let mut conn = connect()?;
    if !session_exists(&mut conn, &name)? {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("no session named {name:?}"),
        ));
    }
    // Hand the window the session to adopt and the exact socket we reached; do NOT let it fall
    // back to its own default, which could be a different daemon.
    let status = std::process::Command::new(gui_bin())
        .env("SPRAG_GUI_SESSION", &name)
        .env("SPRAG_GUI_HOST_SOCK", &sock)
        .status()
        .map_err(|error| {
            io::Error::new(error.kind(), format!("could not launch sprag-gui: {error}"))
        })?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("sprag-gui exited with {status}")))
    }
}

/// Whether the running daemon holds a session named `name` — the [`attach`] pre-flight, over the
/// same `sessions` slot [`ls`] reads.
fn session_exists(conn: &mut HostConn, name: &str) -> io::Result<bool> {
    let sessions = conn.call(
        "scene/query",
        json!({ "path": mux_action_path(SESSIONS_SLOT) }),
    )?;
    Ok(sessions
        .as_array()
        .into_iter()
        .flatten()
        .any(|session| session["name"].as_str() == Some(name)))
}

/// The `sprag-gui` binary [`attach`] launches: [`GUI_BIN_ENV`] if set, else the sibling of this
/// exe (installed together), else `sprag-gui` on `PATH` — mirroring the GUI's own `host_bin`.
fn gui_bin() -> PathBuf {
    if let Some(path) = std::env::var_os(GUI_BIN_ENV) {
        return PathBuf::from(path);
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(sibling) = exe.parent().map(|dir| dir.join("sprag-gui"))
        && sibling.exists()
    {
        return sibling;
    }
    PathBuf::from("sprag-gui")
}

/// `kill-session NAME`: kill one session. Killing the LAST one ends the daemon, so its reply may
/// be cut short by the exit — an EOF there is success, not failure.
fn kill_session(name: Option<String>) -> io::Result<()> {
    let name = name.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "kill-session needs a session name",
        )
    })?;
    let mut conn = connect()?;
    match kill_one(&mut conn, &name) {
        Ok(()) => {
            println!("killed {name}");
            Ok(())
        }
        // Killing the LAST session ends the daemon; its reply can be cut off by the exit at any
        // point — an EOF on the read, or a broken pipe / reset on the next write. Any of those
        // means the server stopped, which is success, not failure.
        Err(error) if server_gone(&error) => {
            println!("killed {name} (server ended)");
            Ok(())
        }
        // The host answers a refused kill with a JSON-RPC error, which `HostConn` surfaces as
        // `Other`; for `kill_session` the only refusal is an unknown name — say so cleanly
        // rather than echo the raw wire error.
        Err(error) if error.kind() == io::ErrorKind::Other => Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("no session named {name:?}"),
        )),
        Err(error) => Err(error),
    }
}

/// `kill-server`: kill every session, which ends the daemon (the last kill drains its session
/// and exits). Reuses [`KILL_SESSION_ACTION`] over one connection rather than a bespoke action —
/// the last kill is what stops the server, so an EOF partway through is the daemon exiting under
/// us, i.e. done.
fn kill_server() -> io::Result<()> {
    let mut conn = connect()?;
    let sessions = conn.call(
        "scene/query",
        json!({ "path": mux_action_path(SESSIONS_SLOT) }),
    )?;
    let names: Vec<String> = sessions
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|session| session["name"].as_str().map(str::to_owned))
        .collect();
    for name in &names {
        match kill_one(&mut conn, name) {
            Ok(()) => {}
            // The last kill ended the daemon; the connection is gone (an EOF, or a broken pipe /
            // reset if the exit raced our next write), and so is the server — done, not an error.
            Err(error) if server_gone(&error) => break,
            Err(error) => return Err(error),
        }
    }
    println!("server stopped");
    Ok(())
}

/// Whether an error means the DAEMON is gone (not a request-level refusal) — the same
/// dead-connection classification the GUI's poll thread (`detach_reason`) makes. Killing the
/// last session ends the daemon, and its reply can be severed at any point: an EOF on the read,
/// or a broken pipe / reset if the exit races the next write.
fn server_gone(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::UnexpectedEof | io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset
    )
}

/// Issue one `kill_session {name}` — the shared call behind both kill commands.
fn kill_one(conn: &mut HostConn, name: &str) -> io::Result<()> {
    conn.call(
        "scene/invoke",
        json!({ "path": mux_action_path(KILL_SESSION_ACTION), "args": { "name": name } }),
    )
    .map(|_: Value| ())
}
