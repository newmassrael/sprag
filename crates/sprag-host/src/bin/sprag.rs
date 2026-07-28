//! `sprag` — the session-management CLI for a running `sprag-term` daemon.
//!
//! ```text
//! sprag ls                 list every session
//! sprag list-clients [-t SESSION]  list attached clients and the session each views (tmux list-clients)
//! sprag new [name]         create a session with a shell (absent name -> the lowest free), print its name
//! sprag ssh [user@]host [-p PORT] [-L FWD]... [--tmux[=NAME]] [-- cmd...]  create a session running
//!                          ssh to a remote host (a first-classed remote workspace); -L forwards a
//!                          local->remote port; --tmux attaches-or-creates a remote tmux session
//! sprag find NEEDLE [-t SESSION] [--pane N] [--regex]  print each matching line as
//!                          PANE:LINE: text. Literal + ASCII case-insensitive by default;
//!                          --regex reads NEEDLE as a case-SENSITIVE regular expression (use
//!                          (?i) to fold); --pane narrows the sweep to one pane
//! sprag run [NAME] [-t SESSION] [--pane N]  list the commands the pane's project declares
//!                          (its `.sprag.toml`), or, given NAME, TYPE that command at the pane's
//!                          prompt without running it — the Enter is the user's
//! sprag attach NAME        open a sprag-gui window attached to a session (tmux attach-session)
//! sprag kill-session NAME   kill a session (the last one ends the daemon)
//! sprag kill-server [--purge]  kill every session, ending the daemon; --purge also deletes the
//!                              durability snapshot AND every pane's saved scrollback (destroy
//!                              the saved workspace, start fresh)
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
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use serde_json::{Value, json};
use sprag_host::wire::{
    BREAK_PANE_ACTION, CLIENTS_SLOT, JOIN_PANE_ACTION, KILL_SESSION_ACTION, KILL_WINDOW_ACTION,
    NEW_SESSION_ACTION, NEW_WINDOW_ACTION, PANES_SLOT, PASTE_ACTION, RENAME_WINDOW_ACTION,
    SELECT_WINDOW_ACTION, SESSIONS_SLOT, WINDOWS_SLOT, find_slot_for, project_slot_for,
    regex_slot_for,
};
use sprag_host::{PaneFind, SshTarget, mux_action_path, pane_input_path};
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
        Some("find") => find(args.collect()),
        Some("run") => run_project(args.collect()),
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

/// The project commands a pane declares, listed — or one of them TYPED at that pane's prompt.
///
/// The pane whose project is read defaults to the first of the session's current window, the same
/// choice `sprag ls` makes for the cwd it shows (a session's identity follows its first pane);
/// `--pane` names another. That matters because a project is a function of a pane's working
/// DIRECTORY: two panes of one window can sit in different repositories.
///
/// With no NAME this LISTS, one line per command, `name<TAB>command line` — a shape a script can cut.
/// With a NAME it delivers that command as a pasted line at the pane's prompt and stops there,
/// WITHOUT a newline: a command named by a file in a repository is typed for the user, and the
/// keystroke that runs it stays theirs (see `sprag_host::project` for the whole rationale). This is
/// the same delivery the GUI palette performs, through the same `paste` action, so the two cannot
/// mean different things.
fn run_project(args: Vec<String>) -> io::Result<()> {
    let bad = |message: String| io::Error::new(io::ErrorKind::InvalidInput, message);
    let mut name: Option<String> = None;
    let mut session: Option<String> = None;
    let mut pane: Option<u64> = None;
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-t" | "--target" => {
                session = Some(
                    it.next()
                        .ok_or_else(|| bad("run: -t needs a session name".to_owned()))?,
                );
            }
            "--pane" => {
                let value = it
                    .next()
                    .ok_or_else(|| bad("run: --pane needs a pane id".to_owned()))?;
                pane = Some(
                    value
                        .parse::<u64>()
                        .map_err(|_| bad(format!("run: --pane {value:?} is not a pane id")))?,
                );
            }
            _ if name.is_none() => name = Some(arg),
            other => return Err(bad(format!("run: unexpected argument {other:?}"))),
        }
    }

    let mut conn = connect()?;
    if let Some(session) = &session {
        require_session(&mut conn, session)?;
    }
    let scoped = |path: String| match &session {
        Some(name) => json!({ "session": name, "path": path }),
        None => json!({ "path": path }),
    };

    // Resolve the pane to read the project of.
    let listed: Value = conn.call("scene/query", scoped(mux_action_path(PANES_SLOT)))?;
    let panes: Vec<u64> = listed
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|pane| pane["id"].as_u64())
        .collect();
    let pane = match pane {
        Some(only) if !panes.contains(&only) => {
            let where_ = session.as_deref().unwrap_or("the current session");
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("run: no pane {only} in {where_} (panes: {panes:?})"),
            ));
        }
        Some(only) => only,
        None => *panes
            .first()
            .ok_or_else(|| bad("run: the window holds no pane".to_owned()))?,
    };

    let answer: Value = conn.call(
        "scene/query",
        scoped(mux_action_path(&project_slot_for(pane))),
    )?;
    if answer.is_null() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "run: pane {pane} is in no project (no {} above its working directory)",
                sprag_host::PROJECT_FILE
            ),
        ));
    }
    // A broken config is the project's own error, reported as such rather than as "no commands".
    if let Some(error) = answer["error"].as_str() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("run: {error}"),
        ));
    }
    let project: sprag_host::Project = serde_json::from_value(answer)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("run: {error}")))?;

    let Some(name) = name else {
        // The listing. `name<TAB>command line`, so `cut -f1` yields exactly the names `run` accepts.
        for action in &project.actions {
            println!("{}\t{}", action.name, action.command_line());
        }
        return Ok(());
    };
    let action = project
        .actions
        .iter()
        .find(|action| action.name == name)
        .ok_or_else(|| {
            let known: Vec<&str> = project.actions.iter().map(|a| a.name.as_str()).collect();
            io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "run: {} declares no command named {name:?} (it declares: {known:?})",
                    project.root.display()
                ),
            )
        })?;
    // Delivered as a PASTE — the same action the GUI palette uses, and bracketed so the whole line
    // arrives as one inert unit at the prompt.
    conn.call("scene/invoke", {
        let mut params = scoped(pane_input_path(pane, PASTE_ACTION));
        params["args"] = json!({ "text": action.command_line() });
        params
    })?;
    eprintln!(
        "sprag: typed {:?} at pane {pane}; press Enter there to run it",
        action.command_line()
    );
    Ok(())
}

fn print_usage() {
    eprintln!(
        "usage: sprag <ls | list-clients [-t SESSION] | new [name] | attach NAME\n\
         \x20             | ssh [user@]host [-p PORT] [-L FWD]… [--tmux[=NAME]] [-- command…]\n\
         \x20             | find NEEDLE [-t SESSION] [--pane N] [--regex]\n\
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

/// Delete the durability state for the daemon on this socket — its snapshot AND every pane's saved
/// scrollback — the EXPLICIT "start fresh", reached ONLY by `kill-server --purge`.
///
/// The daemon lifecycle otherwise PRESERVES both: a reboot, a crash, a natural close, and a plain
/// `kill-server` all leave them, so the workspace comes back next launch (the cmux-durable model),
/// and even turning history off (`SPRAG_RESTORE_HISTORY=0`) only stops saving rather than deleting.
/// `--purge` is the one way to destroy saved state, which is why it must take the history with the
/// shape: leaving a pane's recorded output behind after the user asked to start fresh would be the
/// opposite of what they asked for. Best-effort — missing files are fine, and it runs as the daemon
/// is ending (its save loop dies with it), so it does not race a live save.
fn clear_snapshot() {
    let socket = socket_path(HOST_SOCKET);
    let _ = std::fs::remove_file(sprag_host::snapshot_path(&socket));
    sprag_host::purge_histories(&sprag_host::history_dir(&socket));
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

/// `sprag find NEEDLE [-t SESSION] [--pane N]` — search the session's current window and print each
/// matching line as `PANE:LINE: text`, the `grep -n` shape a script or an agent can slice.
///
/// **Session-wide by DEFAULT, not per-pane, on purpose.** The question a terminal user actually has
/// is "which pane has the error", so the sweep is the useful unit; `--pane` narrows it once the
/// answer to that question is known. An agent that already knows its pane uses the `find_in_pane`
/// MCP tool instead. None of the three implements a second search: all read the host's
/// `find.<needle>` family, so there is ONE definition of what matches (`sprag_vt::Screen::find`) and
/// the CLI cannot drift from the GUI's highlight.
///
/// A `--pane` naming a pane the session's current window does not hold is a clean ERROR, not an
/// empty result: the caller asked for a specific pane, and reporting "no matches" for a pane that
/// is not there would answer a question they did not ask. Contrast the needle itself, where finding
/// nothing IS the answer. An invalid `--regex` pattern is an error for the same reason — the search
/// never ran, so exiting 0 with no output would claim it had.
///
/// `--regex` selects a different QUERY, not a mode on the same one. A needle and a pattern are
/// separate languages in which the same string means different things (`a.b`), so the host keeps
/// them at separate addresses and this flag picks which one to send. It also changes the case rule,
/// deliberately: the literal search folds ASCII case, while a pattern is case-sensitive because the
/// language already has `(?i)`.
///
/// Prints the matching LINES (deduped — a line with three matches is one output line), because that
/// is what a grep-shaped output means. A capped answer is reported on stderr rather than silently
/// looking complete. No matches is not an error: it exits 0 having printed nothing, so "the search
/// ran" and "something failed" stay distinguishable (unlike grep's exit 1, which sprag reserves for
/// errors).
fn find(args: Vec<String>) -> io::Result<()> {
    let FindArgs {
        needle,
        session,
        pane: only,
        regex,
    } = find_args(args)?;
    // Which LANGUAGE the needle is in decides which address is queried — the choice is made once,
    // here, and the rest of the sweep is identical.
    let slot = if regex {
        regex_slot_for(&needle)
    } else {
        find_slot_for(&needle)
    };
    let mut conn = connect()?;
    if let Some(session) = &session {
        require_session(&mut conn, session)?;
    }
    let scoped = |path: String| match &session {
        Some(name) => json!({ "session": name, "path": path }),
        None => json!({ "path": path }),
    };
    let listed: Value = conn.call("scene/query", scoped(mux_action_path(PANES_SLOT)))?;
    let mut panes: Vec<u64> = listed
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|pane| pane["id"].as_u64())
        .collect();
    if let Some(only) = only {
        if !panes.contains(&only) {
            let where_ = session.as_deref().unwrap_or("the current session");
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("find: no pane {only} in {where_} (panes: {panes:?})"),
            ));
        }
        panes.retain(|pane| *pane == only);
    }
    let mut truncated = false;
    for pane in panes {
        let answer: Value = conn.call("scene/query", scoped(pane_input_path(pane, &slot)))?;
        let found: PaneFind = serde_json::from_value(answer).unwrap_or_default();
        // A refused pattern is the same refusal for every pane, so report it once and stop rather
        // than repeating it per pane or printing nothing and exiting 0 as if it had searched.
        if let Some(error) = found.error {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("find: invalid pattern: {error}"),
            ));
        }
        truncated |= found.truncated;
        for line in &found.lines {
            println!("{pane}:{}: {}", line.line, line.text);
        }
    }
    if truncated {
        eprintln!("sprag: find: the answer was capped; later matches were not scanned");
    }
    Ok(())
}

/// `find`'s parsed arguments — the needle, which session to search, and which pane to narrow to.
struct FindArgs {
    needle: String,
    session: Option<String>,
    /// The one pane to search, or `None` to sweep the whole window.
    pane: Option<u64>,
    /// Read the needle as a REGULAR EXPRESSION rather than literal text — which sends a different
    /// QUERY, not the same one with a flag: the two are separate languages and the host keeps them
    /// at separate addresses (`sprag_host::wire::REGEX_FIELD`).
    regex: bool,
}

/// Parse `find`'s arguments: the required NEEDLE positional plus optional `-t SESSION` and
/// `--pane N`. A second positional is a mistake (a multi-word needle must be one quoted argument),
/// not a silent join, and a non-numeric `--pane` is rejected here rather than sent as a path that
/// could not match anything.
fn find_args(args: Vec<String>) -> io::Result<FindArgs> {
    let bad = |message: String| io::Error::new(io::ErrorKind::InvalidInput, message);
    let mut needle: Option<String> = None;
    let mut session = None;
    let mut pane = None;
    let mut regex = false;
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-t" | "--target" => {
                session = Some(
                    it.next()
                        .ok_or_else(|| bad("find: -t needs a session name".to_owned()))?,
                );
            }
            "--pane" => {
                let value = it
                    .next()
                    .ok_or_else(|| bad("find: --pane needs a pane id".to_owned()))?;
                pane = Some(value.parse::<u64>().map_err(|_| {
                    bad(format!(
                        "find: --pane {value:?} is not a pane id (a number)"
                    ))
                })?);
            }
            "--regex" => regex = true,
            _ if needle.is_none() => needle = Some(arg),
            other => {
                return Err(bad(format!(
                    "find: unexpected argument {other:?} (quote a multi-word needle)"
                )));
            }
        }
    }
    let needle = needle.ok_or_else(|| bad("find: a search needle is required".to_owned()))?;
    if needle.is_empty() {
        return Err(bad("find: the search needle is empty".to_owned()));
    }
    Ok(FindArgs {
        needle,
        session,
        pane,
        regex,
    })
}

/// Parse an OPTIONAL `-t SESSION` filter (unlike the window commands' required target). Any/// Parse an OPTIONAL `-t SESSION` filter (unlike the window commands' required target). Any
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
/// `-L FWD` requests a local→remote port forward (repeatable). Because the ssh process itself holds
/// the local listener, the forwarded port also surfaces in the session's sidebar ports badge for
/// free — the existing per-pane `/proc` port scan attributes it like any other listening server.
///
/// `--tmux[=NAME]` runs the remote-tmux preset (`tmux new-session -A -s NAME`, attach-or-create), so
/// the remote session survives the ssh link dropping. It and a `--` remote command are mutually
/// exclusive.
///
/// A malformed destination, port, or forward is a clean local error (nothing is sent). The whole
/// argument parse lives in [`SshTarget::from_args`] so every branch is unit-tested there and this
/// stays a thin call site.
fn ssh(args: Vec<String>) -> io::Result<()> {
    let target = SshTarget::from_args(args)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    // The structured endpoint marks the birth pane a sanctioned remote workspace (reconnect on
    // restore + dropped-file scp), alongside the argv the pane actually runs.
    let remote = serde_json::to_value(target.remote()).expect("SshRemote serialises");
    let mut conn = connect()?;
    let answer = conn.call(
        "scene/invoke",
        json!({
            "path": mux_action_path(NEW_SESSION_ACTION),
            "args": { "cmd": target.ssh_argv(), "remote": remote },
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
/// Foreground (tmux's attach holds the terminal until the client leaves), but the window runs in
/// a session of its OWN ([`own_session`]) — closing that terminal must not close the window.
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
    let mut command = Command::new(gui_bin());
    command
        .env("SPRAG_GUI_SESSION", &name)
        .env("SPRAG_GUI_HOST_SOCK", &sock);
    let status = own_session(&mut command).status().map_err(|error| {
        io::Error::new(error.kind(), format!("could not launch sprag-gui: {error}"))
    })?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("sprag-gui exited with {status}")))
    }
}

/// Give `cmd`'s child a session of its own (`setsid` between `fork` and `exec`), so a hangup on
/// the terminal that ran this CLI cannot reach it.
///
/// A tty hangup SIGHUPs the foreground process group of that tty's session, and a plain spawn
/// leaves the child sitting in it — so closing the launching terminal killed a window the user
/// never asked to close, while the session it viewed lived on in the daemon. Two windows, and
/// shutting one destroyed the other: tmux is spared this only because its client IS the terminal,
/// so there is just the one.
///
/// MEASURED against a real PTY hangup: before, the window died 5/5 within 0.1s of the hangup;
/// after, it survived 4/4 across a 20s watch AND was still an ATTACHED CLIENT of the daemon
/// (`sprag list-clients`) — alive as a client, not merely undead as a process. Changing only this
/// call in an otherwise identical harness flipped the outcome, so it is the whole cause.
///
/// The window is not detached in any OTHER way, on purpose. It keeps the inherited stdio, so a
/// window that fails to come up still says so where the user is looking, and the CLI still blocks
/// on it, so a window that dies is still reported as this command's failure. What it gives up is
/// the launching terminal's job control — Ctrl-C there no longer reaches the window, because that
/// too is addressed to the tty's foreground group.
///
/// The third spawn site to want this and the only one that lacked it: `sprag-term`'s `daemonize`
/// claims a session as its first act, and a pane's child gets one from `portable-pty` before
/// `exec` (`sprag_terminal::pane_pty`, which relies on it to address the pane's group).
fn own_session(cmd: &mut Command) -> &mut Command {
    // SAFETY: the closure runs in the forked child between `fork` and `exec`, where only
    // async-signal-safe work is permitted. `setsid` is async-signal-safe and takes no pointers,
    // and `last_os_error` only wraps `errno` — no allocation, no lock to inherit held. The one
    // documented failure (the caller already leads a process group) is unreachable here: the child
    // is freshly forked, so its pid cannot be the group id it inherited, and reporting the Err is
    // an honest floor rather than a path relied on.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        })
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
/// `--purge` additionally DELETES the snapshot and every pane's saved scrollback: the explicit
/// "start fresh", the one way to destroy
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The property the whole of [`own_session`] exists for, asserted where it can be seen without
    /// a display: the spawned child LEADS a session of its own, so the hangup that goes to the
    /// launching terminal's foreground group has no path to it. Revert-proof by construction —
    /// drop the `pre_exec` and the child inherits this process's session, failing both asserts.
    ///
    /// A `sleep` stands in for the window: `own_session` configures a spawn and knows nothing of
    /// what is spawned, so the stand-in only has to outlive the read.
    #[test]
    fn a_launched_window_leads_a_session_of_its_own() {
        let mut command = Command::new("/bin/sleep");
        command.arg("30");
        let mut child = own_session(&mut command)
            .spawn()
            .expect("spawn the stand-in for the window");
        let pid = i32::try_from(child.id()).expect("a pid fits in pid_t");
        // SAFETY: `getsid` takes no pointers and reads a plain id. The child has not been waited
        // on yet, so its pid is still its own rather than free to be recycled onto a stranger.
        let (child_sid, own_sid) = unsafe { (libc::getsid(pid), libc::getsid(0)) };
        let _ = child.kill();
        let _ = child.wait();

        assert_ne!(child_sid, -1, "read the child's session id");
        assert_ne!(
            child_sid, own_sid,
            "the window does not share the launching terminal's session",
        );
        assert_eq!(
            child_sid, pid,
            "it LEADS its own session, which is what makes the hangup unreachable",
        );
    }
}
