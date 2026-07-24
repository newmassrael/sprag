//! `sprag` — the session-management CLI for a running `sprag-term` daemon.
//!
//! ```text
//! sprag ls                 list every session
//! sprag list-clients [-t SESSION]  list attached clients and the session each views (tmux list-clients)
//! sprag new [name]         create a session with a shell (absent name -> the lowest free), print its name
//! sprag attach NAME        open a sprag-gui window attached to a session (tmux attach-session)
//! sprag kill-session NAME   kill a session (the last one ends the daemon)
//! sprag kill-server [--purge]  kill every session, ending the daemon; --purge also deletes the
//!                              durability snapshot (destroy the saved workspace, start fresh)
//!
//! sprag windows -t SESSION                list a session's windows (name, and which is current)
//! sprag new-window -t SESSION [name]      create + select a window, born with a shell; print its name
//! sprag select-window -t SESSION NAME     make NAME the session's current window
//! sprag rename-window -t SESSION [win] NEW rename a window (default: the current one) to NEW
//! sprag kill-window -t SESSION [win]      kill a window (default: the current one); the last ends the session
//! ```
//!
//! The window commands take a `-t SESSION` target because a window lives IN a session and the
//! daemon holds several — the same out-of-band `session` scope the GUI sends. They pre-flight the
//! session's existence (like [`attach`]) so an unknown session is a clean error, then drive the
//! SCOPED mux window actions.
//!
//! It drives the daemon over the SAME always-on socket the GUI connect-or-spawns
//! (`$XDG_RUNTIME_DIR/sprag-host.sock`, override `SPRAG_HOST_RPC_SOCK`) via the SAME mux
//! control actions the GUI uses ([`SESSIONS_SLOT`], [`NEW_SESSION_ACTION`],
//! [`KILL_SESSION_ACTION`]) — so there is one wire vocabulary, not a CLI-only one. It only
//! CONNECTS (never spawns a daemon): a management command with no server to manage is a clear
//! error, not a silent daemon start. `attach` is the one command that then launches a display
//! process — `sprag-gui` scoped to the session — but its PRE-FLIGHT (does the session exist?)
//! is the same connect-only check, so a typo is a clean error, not a window that flashes and dies.

// A binary crate: `cargo doc` builds it with private items, and the crate-root doc above links
// to the bin's own internals (e.g. [`attach`]) as a navigable map. `private_intra_doc_links`
// guards LIBRARY public-API docs, which publish without private items; a bin has no such
// surface, so the lint is a structural false positive here (mirrors `sprag-gui`).
#![allow(rustdoc::private_intra_doc_links)]

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};
use sprag_host::wire::{
    BREAK_PANE_ACTION, CLIENTS_SLOT, JOIN_PANE_ACTION, KILL_SESSION_ACTION, KILL_WINDOW_ACTION,
    NEW_SESSION_ACTION, NEW_WINDOW_ACTION, RENAME_WINDOW_ACTION, SELECT_WINDOW_ACTION,
    SESSIONS_SLOT, WINDOWS_SLOT,
};
use sprag_host::{SshTarget, mux_action_path};
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
        Some("list-clients") => list_clients(args.collect()),
        Some("new") => new(args.next()),
        Some("ssh") => ssh(args.collect()),
        Some("attach") => attach(args.next()),
        Some("kill-session") => kill_session(args.next()),
        Some("kill-server") => kill_server(args.collect()),
        Some("windows") => windows(args.collect()),
        Some("new-window") => new_window(args.collect()),
        Some("select-window") => select_window(args.collect()),
        Some("rename-window") => rename_window(args.collect()),
        Some("kill-window") => kill_window(args.collect()),
        Some("break-pane") => break_pane(args.collect()),
        Some("join-pane") => join_pane(args.collect()),
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
    eprintln!(
        "usage: sprag <ls | list-clients [-t SESSION] | new [name] | attach NAME\n\
         \x20             | ssh [user@]host [-p PORT] [-- command…]\n\
         \x20             | kill-session NAME | kill-server [--purge]>\n\
         \x20      sprag <windows | new-window [name] | select-window NAME\n\
         \x20             | rename-window [window] NAME | kill-window [window]\n\
         \x20             | break-pane PANE [name] | join-pane PANE WINDOW> -t SESSION"
    );
}

/// Env override: the `sprag-gui` binary [`attach`] launches (else the sibling of this exe — they
/// install together — else `sprag-gui` on `PATH`). Mirrors the GUI's own `SPRAG_GUI_HOST_BIN`
/// discovery of `sprag-term`.
const GUI_BIN_ENV: &str = "SPRAG_GUI_BIN";

/// Delete the durability snapshot for the daemon on this socket — the EXPLICIT "start fresh",
/// reached ONLY by `kill-server --purge`. The daemon lifecycle otherwise PRESERVES the snapshot: a
/// reboot, a crash, a natural close, and a plain `kill-server` all leave it, so the workspace comes
/// back next launch (the cmux-durable model). `--purge` is the one way to destroy the saved
/// workspace. Best-effort — a missing file is fine, and it runs as the daemon is ending (its save
/// loop dies with it), so it does not race a live save.
fn clear_snapshot() {
    let _ = std::fs::remove_file(sprag_host::snapshot_path(&socket_path(HOST_SOCKET)));
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

/// `ls`: one line per session — its name, its window count, which one an unscoped request lands
/// in, how many clients are attached (viewing) it, and (where known) its current working
/// directory, git branch, and the TCP ports it is listening on. The GUI sidebar shows only the
/// cwd's basename to fit the rail; the FULL path is here, from the same `sessions` slot read.
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
        // cwd + branch are Slice 2's live fields — absent (older daemon) or null (no pane / no
        // repo) just fall away, so the line degrades to the pre-Slice-2 form.
        let cwd = session["cwd"].as_str().unwrap_or("");
        let suffix = match (cwd, session["branch"].as_str()) {
            ("", None) => String::new(),
            ("", Some(branch)) => format!("  [{branch}]"),
            (cwd, None) => format!("  {cwd}"),
            (cwd, Some(branch)) => format!("  {cwd} [{branch}]"),
        };
        // ports is Slice 3's live field — a `:3000 :8080` badge; absent (older daemon) or empty
        // (serving nothing) it falls away, degrading the line to the pre-Slice-3 form.
        let ports = session["ports"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_u64)
            .map(|port| format!(":{port}"))
            .collect::<Vec<_>>()
            .join(" ");
        let ports_suffix = if ports.is_empty() {
            String::new()
        } else {
            format!("  {ports}")
        };
        // attached is Slice's live viewer count (R-PR67): absent (older daemon) or 0 (nobody
        // viewing) it falls away, degrading the line to the pre-attachment form. It is
        // `skip_serializing_if`-elided at 0, so `unwrap_or(0)` restores the honest count.
        let attached = session["attached"].as_u64().unwrap_or(0);
        let attached_suffix = if attached == 0 {
            String::new()
        } else {
            format!("  ({attached} attached)")
        };
        println!("{name}: {windows} window(s){marker}{attached_suffix}{suffix}{ports_suffix}");
    }
    Ok(())
}

/// `list-clients [-t SESSION]`: one line per ATTACHED client — its opaque id and the session it
/// is viewing — tmux `list-clients`. With `-t SESSION`, only clients attached to that session (the
/// session is pre-flighted so a typo is a clean error, like the window commands). The client id is
/// what a `sprag-gui` window mints (`gui-{pid}-{nanos}`); the daemon has no tty/size to report, so
/// the line is `client -> session`, the honest subset tmux's `struct client` row reduces to here.
fn list_clients(args: Vec<String>) -> io::Result<()> {
    let filter = optional_target(args, "list-clients")?;
    let mut conn = connect()?;
    if let Some(session) = &filter {
        require_session(&mut conn, session)?;
    }
    let clients = conn.call(
        "scene/query",
        json!({ "path": mux_action_path(CLIENTS_SLOT) }),
    )?;
    for client in clients.as_array().into_iter().flatten() {
        let id = client["client"].as_str().unwrap_or("?");
        let session = client["session"].as_str().unwrap_or("?");
        if filter.as_deref().is_some_and(|want| want != session) {
            continue;
        }
        println!("{id}: {session}");
    }
    Ok(())
}

/// Parse an OPTIONAL `-t SESSION` filter (unlike the window commands' required target). Any
/// non-flag positional is unexpected — `list-clients` takes only the optional target.
fn optional_target(args: Vec<String>, command: &str) -> io::Result<Option<String>> {
    let mut session = None;
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-t" | "--target" => {
                session = Some(it.next().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("{command}: -t needs a session name"),
                    )
                })?);
            }
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "{command}: unexpected argument {other:?} (only -t SESSION is accepted)"
                    ),
                ));
            }
        }
    }
    Ok(session)
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

/// `ssh [user@]host [-p PORT] [-- command…]`: create a session whose first pane runs `ssh` to a
/// remote host — a first-classed remote workspace. The birth pane's argv is `ssh -t …`
/// ([`SshTarget::ssh_argv`]), so the remote login shell (or the given remote command) gets a real
/// TTY and the whole reflow/resize/scrollback machinery applies unchanged; nothing on the wire or
/// in the daemon is ssh-aware — this rides the existing `new_session {cmd}` action. The registry
/// allocates the session name (like `new` with no name), which is printed for scoping a client.
///
/// A malformed destination or port is a clean local error (nothing is sent). The whole argument
/// parse lives in [`SshTarget::from_args`] so every branch is unit-tested there and this stays a
/// thin call site.
fn ssh(args: Vec<String>) -> io::Result<()> {
    let target = SshTarget::from_args(args)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let mut conn = connect()?;
    let answer = conn.call(
        "scene/invoke",
        json!({
            "path": mux_action_path(NEW_SESSION_ACTION),
            "args": { "cmd": target.ssh_argv() },
        }),
    )?;
    match answer.as_str() {
        Some(created) => {
            println!("{created}");
            Ok(())
        }
        None => Err(io::Error::other("ssh did not answer with a name")),
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
        // means the server stopped, which is success, not failure. The snapshot is PRESERVED (the
        // durable default) — use `kill-server --purge` to destroy the saved workspace.
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

/// `kill-server [--purge]`: kill every session, which ends the daemon (the last kill drains its
/// session and exits). Reuses [`KILL_SESSION_ACTION`] over one connection rather than a bespoke
/// action — the last kill is what stops the server, so an EOF partway through is the daemon exiting
/// under us, i.e. done.
///
/// By DEFAULT the durability snapshot is PRESERVED: stopping the daemon does not destroy the saved
/// workspace, so the next launch restores it (the cmux-durable model — your workspace persists).
/// `--purge` additionally DELETES the snapshot: the explicit "start fresh", the one way to destroy
/// the saved workspace.
fn kill_server(args: Vec<String>) -> io::Result<()> {
    let purge = args.iter().any(|a| a == "--purge");
    if let Some(other) = args.iter().find(|a| *a != "--purge") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("kill-server: unexpected argument {other:?} (only --purge is accepted)"),
        ));
    }
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
    if purge {
        clear_snapshot();
        println!("server stopped (workspace purged)");
    } else {
        println!("server stopped");
    }
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

/// Split a window subcommand's args into its required `-t SESSION` target and any trailing
/// positionals. A window lives IN a session, and the daemon holds several — so, like tmux's
/// window/pane commands, these take `-t`.
fn target_and_rest(args: Vec<String>, command: &str) -> io::Result<(String, Vec<String>)> {
    let mut session = None;
    let mut rest = Vec::new();
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-t" | "--target" => {
                session = Some(it.next().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("{command}: -t needs a session name"),
                    )
                })?);
            }
            _ => rest.push(arg),
        }
    }
    let session = session.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{command}: a target session is required (-t SESSION)"),
        )
    })?;
    Ok((session, rest))
}

/// Refuse cleanly if the daemon holds no session named `session` — the window-command pre-flight
/// (like [`attach`]'s), so an unknown session is a clear error rather than a raw scope-refusal, and
/// any later action refusal can be reported as the window-level problem it then must be.
fn require_session(conn: &mut HostConn, session: &str) -> io::Result<()> {
    if session_exists(conn, session)? {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("no session named {session:?}"),
        ))
    }
}

/// `windows -t SESSION`: one line per window — its name, and `(current)` on the active one.
fn windows(args: Vec<String>) -> io::Result<()> {
    let (session, _rest) = target_and_rest(args, "windows")?;
    let mut conn = connect()?;
    require_session(&mut conn, &session)?;
    let windows = conn.call(
        "scene/query",
        json!({ "session": session, "path": mux_action_path(WINDOWS_SLOT) }),
    )?;
    for window in windows.as_array().into_iter().flatten() {
        let name = window["name"].as_str().unwrap_or("?");
        let marker = if window["current"].as_bool().unwrap_or(false) {
            " (current)"
        } else {
            ""
        };
        println!("{name}{marker}");
    }
    Ok(())
}

/// `new-window -t SESSION [name]`: create + select a window, born with a shell, and print the
/// name it got (the registry allocates the lowest free one when none is given).
fn new_window(args: Vec<String>) -> io::Result<()> {
    let (session, rest) = target_and_rest(args, "new-window")?;
    let name = rest.into_iter().next();
    let mut conn = connect()?;
    require_session(&mut conn, &session)?;
    let action_args = match &name {
        Some(name) => json!({ "name": name }),
        None => json!({}),
    };
    let answer = conn.call(
        "scene/invoke",
        json!({ "session": session, "path": mux_action_path(NEW_WINDOW_ACTION), "args": action_args }),
    );
    match answer {
        Ok(answer) => match answer.as_str() {
            Some(created) => {
                println!("{created}");
                Ok(())
            }
            None => Err(io::Error::other("new-window did not answer with a name")),
        },
        // The only refusal for an explicitly-named window is a duplicate (the session was
        // pre-flighted), which surfaces as `Other`.
        Err(error) if error.kind() == io::ErrorKind::Other => {
            let named = name.as_deref().unwrap_or_default();
            Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("a window named {named:?} already exists in session {session:?}"),
            ))
        }
        Err(error) => Err(error),
    }
}

/// `select-window -t SESSION NAME`: make NAME the session's current window.
fn select_window(args: Vec<String>) -> io::Result<()> {
    let (session, rest) = target_and_rest(args, "select-window")?;
    let window = rest.into_iter().next().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "select-window needs a window name",
        )
    })?;
    let mut conn = connect()?;
    require_session(&mut conn, &session)?;
    scoped_window_action(
        &mut conn,
        &session,
        SELECT_WINDOW_ACTION,
        json!({ "window": window }),
        &format!("no window named {window:?} in session {session:?}"),
    )?;
    println!("selected {window}");
    Ok(())
}

/// `rename-window -t SESSION [window] NEW`: rename a window (default: the current one) to NEW.
fn rename_window(args: Vec<String>) -> io::Result<()> {
    let (session, mut rest) = target_and_rest(args, "rename-window")?;
    let new = rest.pop().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "rename-window needs a new name",
        )
    })?;
    // An optional leading positional names the window to rename; absent ⇒ the current one.
    let window = rest.pop();
    let mut conn = connect()?;
    require_session(&mut conn, &session)?;
    let action_args = match &window {
        Some(window) => json!({ "window": window, "name": new }),
        None => json!({ "name": new }),
    };
    scoped_window_action(
        &mut conn,
        &session,
        RENAME_WINDOW_ACTION,
        action_args,
        &format!("rename-window: window not found, or {new:?} is already taken"),
    )?;
    println!("renamed to {new}");
    Ok(())
}

/// `kill-window -t SESSION [window]`: kill a window (default: the current one). The session's LAST
/// window ends the SESSION — and the last session ends the daemon, so the reply can be cut short by
/// the exit, which is success (the same `server_gone` handling `kill-session` uses).
fn kill_window(args: Vec<String>) -> io::Result<()> {
    let (session, rest) = target_and_rest(args, "kill-window")?;
    let window = rest.into_iter().next();
    let mut conn = connect()?;
    require_session(&mut conn, &session)?;
    let action_args = match &window {
        Some(window) => json!({ "window": window }),
        None => json!({}),
    };
    let answer = conn.call(
        "scene/invoke",
        json!({ "session": session, "path": mux_action_path(KILL_WINDOW_ACTION), "args": action_args }),
    );
    let target = window.as_deref().unwrap_or("the current window");
    match answer {
        Ok(_) => {
            println!("killed {target}");
            Ok(())
        }
        // Killing the LAST window ends the session, and the last session ends the daemon: the reply
        // can be severed by the exit (EOF / broken pipe / reset), which is success. The snapshot is
        // PRESERVED (the durable default) — use `kill-server --purge` to destroy the saved workspace.
        Err(error) if server_gone(&error) => {
            println!("killed {target} (server ended)");
            Ok(())
        }
        // Otherwise the only refusal (the session was pre-flighted) is an unknown window.
        Err(error) if error.kind() == io::ErrorKind::Other => Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("no window named {target:?} in session {session:?}"),
        )),
        Err(error) => Err(error),
    }
}

/// Issue a scoped window `scene/invoke`, mapping a request-level refusal (`Other`) to `message` —
/// the shared call behind `select-window` / `rename-window`, whose only refusal (the session
/// pre-flighted) is a window-level one.
fn scoped_window_action(
    conn: &mut HostConn,
    session: &str,
    action: &str,
    action_args: Value,
    message: &str,
) -> io::Result<()> {
    conn.call(
        "scene/invoke",
        json!({ "session": session, "path": mux_action_path(action), "args": action_args }),
    )
    .map(|_: Value| ())
    .map_err(|error| {
        if error.kind() == io::ErrorKind::Other {
            io::Error::new(io::ErrorKind::NotFound, message.to_owned())
        } else {
            error
        }
    })
}

/// A required positional PANE id — a non-negative integer, how sprag addresses a pane on the wire
/// (unique across the whole daemon). tmux names a pane `window.index`; sprag's global id is enough.
fn parse_pane_id(arg: Option<String>, command: &str) -> io::Result<u64> {
    let raw = arg.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{command} needs a pane id"),
        )
    })?;
    raw.parse::<u64>().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{command}: pane id {raw:?} must be a number"),
        )
    })
}

/// `break-pane -t SESSION PANE [name]`: break the pane with id PANE out of its window into a NEW
/// window (born current), printing the new window's name. tmux `break-pane` — the pane's source
/// window is DERIVED from its (registry-unique) id, so only the pane id is named.
fn break_pane(args: Vec<String>) -> io::Result<()> {
    let (session, rest) = target_and_rest(args, "break-pane")?;
    let mut rest = rest.into_iter();
    let pane = parse_pane_id(rest.next(), "break-pane")?;
    let name = rest.next();
    let mut conn = connect()?;
    require_session(&mut conn, &session)?;
    let mut action_args = json!({ "pane": pane });
    if let Some(name) = &name {
        action_args["name"] = json!(name);
    }
    let answer = conn.call(
        "scene/invoke",
        json!({ "session": session, "path": mux_action_path(BREAK_PANE_ACTION), "args": action_args }),
    );
    match answer {
        Ok(answer) => match answer.as_str() {
            Some(created) => {
                println!("{created}");
                Ok(())
            }
            None => Err(io::Error::other("break-pane did not answer with a name")),
        },
        // The refusals (the pane is its window's only one, an explicit name is taken, or no window
        // holds the pane) surface as `Other`.
        Err(error) if error.kind() == io::ErrorKind::Other => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "break-pane refused: pane {pane} is its window's only pane, no window holds it, or the name is taken"
            ),
        )),
        Err(error) => Err(error),
    }
}

/// `join-pane -t SESSION PANE WINDOW`: move the pane with id PANE into the window named WINDOW,
/// appending it there. A move that empties the pane's old window closes it. tmux `join-pane`.
fn join_pane(args: Vec<String>) -> io::Result<()> {
    let (session, rest) = target_and_rest(args, "join-pane")?;
    let mut rest = rest.into_iter();
    let pane = parse_pane_id(rest.next(), "join-pane")?;
    let window = rest.next().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "join-pane needs a destination window",
        )
    })?;
    let mut conn = connect()?;
    require_session(&mut conn, &session)?;
    let answer = conn.call(
        "scene/invoke",
        json!({ "session": session, "path": mux_action_path(JOIN_PANE_ACTION), "args": { "pane": pane, "window": window } }),
    );
    match answer {
        Ok(answer) => {
            if answer["closed_source"].as_bool().unwrap_or(false) {
                println!("joined pane {pane} into {window} (source window closed)");
            } else {
                println!("joined pane {pane} into {window}");
            }
            Ok(())
        }
        // The refusals (no such destination window, no window holds the pane, or the pane already
        // lives in the destination) surface as `Other`.
        Err(error) if error.kind() == io::ErrorKind::Other => Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "join-pane refused: no window named {window:?} in session {session:?}, no pane {pane}, or it already lives there"
            ),
        )),
        Err(error) => Err(error),
    }
}
