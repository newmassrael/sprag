//! `sprag` — the session-management CLI for a running `sprag-term` daemon.
//!
//! ```text
//! sprag ls                 list every session
//! sprag new [name]         create a session (absent name -> the lowest free), print its name
//! sprag kill-session NAME   kill a session (the last one ends the daemon)
//! sprag kill-server        kill every session, ending the daemon
//! ```
//!
//! It drives the daemon over the SAME always-on socket the GUI connect-or-spawns
//! (`$XDG_RUNTIME_DIR/sprag-host.sock`, override `SPRAG_HOST_RPC_SOCK`) via the SAME mux
//! control actions the GUI uses ([`SESSIONS_SLOT`], [`NEW_SESSION_ACTION`],
//! [`KILL_SESSION_ACTION`]) — so there is one wire vocabulary, not a CLI-only one. It only
//! CONNECTS (never spawns): a management command with no server to manage is a clear error,
//! not a silent daemon start. Launching a GUI attached to a session (`attach`) is display
//! work a later addition; this is the headless half.

use std::io;
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
    eprintln!("usage: sprag <ls | new [name] | kill-session NAME | kill-server>");
}

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

/// `new [name]`: create a session (the registry allocates the lowest free name when none is
/// given) and print the name it got — the string to scope a client to.
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
