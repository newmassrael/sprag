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
//! sprag wait-for-output --pane N NEEDLE [-t SESSION] [--regex]  BLOCK until that pane's retained
//!                          output matches, then print the matching lines like `find`. The same two
//!                          search languages, in the other tense: `find` asks "does it say this
//!                          now", this asks "tell me when it does". No timeout — wrap it in
//!                          `timeout` if you want one
//! sprag run [NAME] [-t SESSION] [--pane N]  list the commands the pane's project declares
//!                          (its `.sprag.toml`), or, given NAME, TYPE that command at the pane's
//!                          prompt without running it — the Enter is the user's
//! sprag attach NAME [--no-wait | --tui | --remote HOST]  attach a client to a session (tmux
//!                          attach-session). Default: a sprag-gui window, blocking until it
//!                          closes; --no-wait returns once the window has attached (still
//!                          reporting one that fails to). --tui runs sprag-tui in THIS terminal;
//!                          --remote runs it on HOST over ssh, where that session lives
//! sprag kill-session NAME   kill a session (the last one ends the daemon)
//! sprag kill-server [--purge]  kill every session, ending the daemon; --purge also deletes the
//!                              durability snapshot AND every pane's saved scrollback (destroy
//!                              the saved workspace, start fresh)
//!
//! sprag select-pane -t SESSION <PANE | -L|-R|-U|-D>
//!                                         make a pane ACTIVE — by id, or by walking the
//!                                         arrangement left/right/up/down from the pane the
//!                                         session is on (tmux select-pane). Session state: every
//!                                         attached client follows, and a pane verb given no
//!                                         target acts on it
//!
//! sprag windows -t SESSION                list a session's windows (name, and which is current)
//! sprag new-window -t SESSION [name]      create + select a window, born with a shell; print its name
//! sprag select-window -t SESSION NAME     make NAME the session's current window
//! sprag rename-window -t SESSION [win] NEW rename a window (default: the current one) to NEW
//! sprag kill-window -t SESSION [win]      kill a window (default: the current one); the last ends the session
//! sprag resize-window -t SESSION [win] <-x COLS -y ROWS | -a | -A | -L/-R/-U/-D N | -u>
//!                                         PIN a window's size so it stops following the clients
//!                                         attached to it (tmux resize-window). -x/-y an exact
//!                                         size; -a/-A the smallest/largest attached client;
//!                                         -L/-R narrower/wider and -U/-D shorter/taller by N
//!                                         (relative to the size it has now); -u un-pins it.
//!                                         Takes effect while `window-size` is `manual`; the size
//!                                         is stored either way and survives a reboot
//!
//! sprag panes [-t SESSION]                        list the current window's panes (tmux list-panes)
//! sprag layout [-t SESSION]                       print WHERE those panes sit — the arrangement,
//!                                         which pane fills the window, and which are floating
//! sprag split-window [-t SESSION] [-h|-v [-b] PANE] [-- command…]
//!                                         divide PANE right (-h) / below (-v), or append with
//!                                         neither; print the new pane's id (tmux split-window)
//! sprag rename-pane [-t SESSION] PANE <NAME | --clear>  give a pane a NAME, or take it away.
//!                                         A name is an ADDRESS an agent can hold where a pane
//!                                         NUMBER goes stale; unique across the daemon
//! sprag kill-pane [-t SESSION] PANE               close a pane (tmux kill-pane)
//! sprag resize-pane [-t SESSION] PANE -x COLS -y ROWS  resize a pane's PTY + emulator
//! sprag send-keys [-t SESSION] PANE [-l] KEY…     send W3C key names (or, with -l, literal text)
//! sprag capture-pane [-t SESSION] PANE [-p]       print a pane's retained output to stdout
//! sprag agent [-t SESSION] [PANE]                 what the AI agent in each pane is doing
//! sprag report-agent STATE [--pane N] [--source S]  say what the agent in a pane is DOING
//!                          [--name AGENT] [--seq N]  (the pane defaults to $SPRAG_PANE)
//! sprag release-agent [-t SESSION] [--pane N]      hand the pane back to screen inference
//! sprag install-hooks [AGENT…] [--yes] [--dry-run]  wire an agent's OWN config to report-agent,
//! sprag uninstall-hooks [AGENT…] [--yes] [--dry-run]  so it says what it is doing instead of
//! sprag list-hooks                         being guessed at. Writes under $HOME, so it ASKS:
//!                          the prompt shows the edit, --dry-run stops at it, --yes answers it,
//!                          and with no terminal to ask on it refuses rather than assume. Naming
//!                          no AGENT covers every agent actually on this machine. See
//!                          [`install_hooks`] and [`sprag_host::hooks`]
//!                                         (working / blocked / idle), one line per pane an agent
//!                                         manifest claims — a shell prints nothing. Naming a PANE
//!                                         also prints WHICH RULE decided, and how to correct it
//!
//! sprag list-keys                          print the client keymap `config.toml` produces
//! sprag bind-key [-nr] [-T TABLE] KEY ACTION…  give a key a meaning (tmux bind-key)
//! sprag unbind-key [-n] [-T TABLE] KEY     take a key's meaning away (tmux unbind-key)
//!                          -n is -T root: a key that acts with NO prefix. -r is tmux's repeat
//! sprag show-options [-g] [-v] [NAME]      print the options and their values (tmux show-options)
//! sprag set-option [-g] NAME VALUE         set one client option (tmux set-option)
//! sprag set-option [-g] -u NAME            put one back to its default (tmux set-option -u)
//!                          The verbs here that need NO DAEMON: a keybinding and a client option
//!                          are a CLIENT's, not a server's, so they answer while nothing is
//!                          running. Unlike tmux's, the editing verbs WRITE `config.toml` — see
//!                          [`bind_key`]
//! ```
//!
//! ## Which commands take `-t`, and why the two answers differ
//!
//! A WINDOW command's `-t SESSION` is REQUIRED, because a window lives IN a session, the daemon
//! holds several, and there is no useful default for "which session's window list did you mean".
//! They pre-flight the session's existence (like [`attach`]) so an unknown session is a clean
//! error, then drive the SCOPED mux window actions.
//!
//! A PANE command's `-t SESSION` is OPTIONAL, because an unscoped request already HAS a scope —
//! the daemon's default session ([`sprag_host::wire::SESSION_PARAM`]) — so a one-session workspace
//! never has to spell it. That is the rule [`find`] and [`run_project`] already followed; the pane
//! verbs join them rather than inventing a third convention. Both kinds pass the same out-of-band
//! `session` param the GUI sends, so there is one scoping vocabulary, not a CLI-only one.
//!
//! ## The pane verbs and the pane they mean
//!
//! `select-pane`, and with it tmux's BARE `split-window -h` / `-v`, are BUILT — both rest on the
//! daemon's active pane ([`sprag_host::wire::SELECT_PANE_ACTION`]), which is session state every
//! attached client follows rather than a display client's private idea of where its focus ring is.
//! Until that existed this section listed both as impossible, and the reason it gave was right at
//! the time: a direction is meaningless without a pane to be relative to, and there was no "here"
//! for the daemon to resolve.
//!
//! It is a DIFFERENT fact from the pane-input `focus` action, which reports a focus EDGE to the
//! child (DEC private mode 1004) on behalf of a client whose own OS focus moved. That one says
//! "somebody is looking at your window"; this one says "this is the pane the session is on". A
//! `select-pane` built on the former would have sent a program a focus-in report while no client
//! had focused it.
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
use sprag_host::hooks::{self, HookError, Target};
use sprag_host::keymap::{BoundAction, KeySpec, KeyTable};
use sprag_host::shellword::shell_quote;
use sprag_host::wire::{
    AGENT_MANIFESTS_SLOT, BREAK_PANE_ACTION, CLIENTS_SLOT, CLOSE_ACTION, FULL_TEXT_SLOT,
    JOIN_PANE_ACTION, KEY_ACTION, KILL_SESSION_ACTION, KILL_WINDOW_ACTION, LAYOUT_SLOT,
    MOVE_PANE_ACTION, NEEDLE_PARAM, NEW_SESSION_ACTION, NEW_WINDOW_ACTION, PANE_PARAM,
    PANE_WAIT_OUTPUT_METHOD, PANES_SLOT, PASTE_ACTION, PATTERN_PARAM, PaneProcessesWire,
    RELEASE_AGENT_ACTION, RENAME_PANE_ACTION, RENAME_WINDOW_ACTION, REPORT_AGENT_ACTION,
    RESIZE_ACTION, RESIZE_WINDOW_ACTION, SELECT_PANE_ACTION, SELECT_WINDOW_ACTION, SESSIONS_SLOT,
    SPAWN_ACTION, SPLIT_ACTION, SWAP_PANE_ACTION, SelectHow, TEXT_ACTION, WINDOWS_SLOT,
    ZOOM_PANE_ACTION, events_slot_since, find_slot_for, pane_processes_at, project_slot_for,
    regex_slot_for, session_activity_at,
};
use sprag_host::{PaneFind, SshTarget, mux_action_path, pane_input_path};
use sprag_rpc::{
    CallError, EVENTS_CHANGED_METHOD, EVENTS_SUBSCRIBE_METHOD, HOST_SOCKET, HostConn, HostEndpoint,
    INVALID_PARAMS, RpcFault, SINCE_PARAM, socket_path,
};
use sprag_terminal::{LayoutSnapshot, PaneDir, arrangement};

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
        Some("wait-for-output") => wait_for_output(args.collect()),
        Some("run") => run_project(args.collect()),
        Some("attach") => attach(args.collect()),
        Some("kill-session") => kill_session(args.next()),
        Some("kill-server") => kill_server(args.collect()),
        Some("windows") => windows(args.collect()),
        Some("new-window") => new_window(args.collect()),
        Some("select-window") => select_window(args.collect()),
        Some("select-pane") => select_pane(args.collect()),
        Some("rename-window") => rename_window(args.collect()),
        Some("kill-window") => kill_window(args.collect()),
        Some("resize-window") => resize_window(args.collect()),
        Some("break-pane") => break_pane(args.collect()),
        Some("join-pane") => join_pane(args.collect()),
        Some("move-pane") => move_pane(args.collect()),
        Some("swap-pane") => swap_pane(args.collect()),
        Some("zoom-pane") => zoom_pane(args.collect()),
        Some("rename-pane") => rename_pane(args.collect()),
        Some("panes") => panes(args.collect()),
        Some("layout") => layout(args.collect()),
        Some("processes") => processes(args.collect()),
        Some("agent") => agent(args.collect()),
        Some("report-agent") => report_agent(args.collect()),
        Some("release-agent") => release_agent(args.collect()),
        Some("install-hooks") => install_hooks(args.collect()),
        Some("uninstall-hooks") => uninstall_hooks(args.collect()),
        Some("list-hooks") => list_hooks(args.collect()),
        Some("hook") => hook(args.collect()),
        Some("events") => events(args.collect()),
        Some("split-window") => split_window(args.collect()),
        Some("kill-pane") => kill_pane(args.collect()),
        Some("resize-pane") => resize_pane(args.collect()),
        Some("send-keys") => send_keys(args.collect()),
        Some("capture-pane") => capture_pane(args.collect()),
        Some("list-keys") => list_keys(args.collect()),
        Some("bind-key") => bind_key(args.collect()),
        Some("unbind-key") => unbind_key(args.collect()),
        Some("show-options") => show_options(args.collect()),
        Some("set-option") => set_option(args.collect()),
        Some("-V" | "--version" | "version") => {
            print_version();
            Ok(())
        }
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

    let mut conn = connect(None)?;
    if let Some(session) = &session {
        require_session(&mut conn, session)?;
    }
    let scoped = |path: String| scoped_params(session.as_deref(), path);

    // Resolve the pane to read the project of.
    let pane = match pane {
        Some(only) => {
            require_pane(&mut conn, session.as_deref(), only, "run")?;
            only
        }
        None => *pane_ids(&mut conn, session.as_deref())?
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

/// `list-keys`: print the client keymap `config.toml` produces — tmux `list-keys`.
///
/// **This is the one verb here that needs no daemon**, and that is not an accident: a keybinding is
/// what a CLIENT does with a keyboard, so it lives in the user's config file rather than in the
/// server. tmux's `list-keys` has to start a server to answer; sprag's answers on a machine with no
/// session running at all, which is exactly when a user is editing their config.
///
/// The output is tmux's own shape (`bind-key -T prefix KEY COMMAND`) so a tmux user reads it
/// without learning anything, and every line after the first begins with `bind-key` so a script can
/// filter for them. The PREFIX gets the first line on its own, even though it is now also an OPTION
/// (`show-options` prints it; `set-option prefix` changes it, which is tmux's own answer). That is not
/// a second authority: both lines come out of one file, and the keymap's prefix is built FROM the
/// option rather than beside it. It stays because a keymap listing whose prefix a reader has to look
/// up elsewhere is what hid R235's defect — a `send-prefix` binding stranded on an abandoned key,
/// visible only when the two were printed together.
fn list_keys(args: Vec<String>) -> io::Result<()> {
    if let Some(unexpected) = args.first() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("list-keys: unexpected argument {unexpected:?} (it takes none)"),
        ));
    }
    let keymap = sprag_host::config::keymap().map_err(|error| {
        io::Error::new(io::ErrorKind::InvalidData, format!("list-keys: {error}"))
    })?;
    println!("prefix {}", keymap.prefix());
    // Each line carries its own `-T`, exactly as tmux's does, rather than the tables being printed
    // under headings: every line after the first is then a `bind-key` command a user can paste back,
    // which is the property that makes this output worth having in tmux's shape at all.
    //
    // Aligned on the widest key so the actions read as a column. Measured in CHARACTERS rather than
    // bytes: a key spec is a user's string and `%` is not the only thing they may bind.
    let width = |what: fn(&sprag_host::keymap::Bind) -> String| {
        keymap
            .binds()
            .map(|bind| what(bind).chars().count())
            .max()
            .unwrap_or(0)
    };
    let table_width = width(|bind| bind.table().to_string());
    let key_width = width(|bind| bind.key().to_string());
    for bind in keymap.binds() {
        // The repeat column is a fixed two characters wide whether or not this binding repeats, so
        // the actions still line up in a table where only some do.
        let repeat = if bind.repeats() { "-r" } else { "  " };
        let table = format!("{:table_width$}", bind.table());
        let key = format!("{:key_width$}", bind.key().to_string());
        println!("bind-key {repeat} -T {table} {key}  {}", bind.action());
    }
    Ok(())
}

/// The flags `bind-key` and `unbind-key` take before the key — tmux's `[-nr] [-T table]`.
///
/// A record rather than a tuple because `-n` and `-T root` are the SAME flag under two spellings
/// (tmux's manual: *"-n is an alias for -T root"*), and a caller reading `.0` would have to know
/// which of them produced it.
struct KeyFlags {
    /// Which table, from `-n` or `-T`.
    table: KeyTable,
    /// tmux's `-r`, accepted by `bind-key` alone.
    repeat: bool,
}

/// `bind-key [-nr] [-T TABLE] KEY ACTION…`: give a key a meaning — tmux `bind-key`.
///
/// **This EDITS `config.toml`, which tmux's `bind-key` does not**, and the difference is the whole
/// of slice 2's design. tmux's config is an imperative script that a runtime fact cannot be written
/// back into, so its binds are transient and the user has to remember to write them down; sprag's
/// is declarative TOML, so the file simply IS the live table. A client attached right now re-reads
/// it, `list-keys` prints it, and the next attach still has it — one answer rather than three.
///
/// Like `list-keys`, it needs NO DAEMON: a keybinding is a client's, so binding one on a machine
/// with nothing running is exactly as meaningful as binding it with a session up.
///
/// The ACTION is the rest of the line, JOINED — so both tmux's unquoted `bind-key c split-window
/// -h` and a shell-quoted `bind-key c "split-window -h"` arrive as the same string, which is the
/// one `BoundAction` parses.
fn bind_key(args: Vec<String>) -> io::Result<()> {
    let (flags, args) = strip_key_flags("bind-key", args)?;
    let mut rest = args.into_iter();
    let key = rest.next().ok_or_else(|| {
        bad_input("bind-key: needs a key and an action, e.g. `bind-key c \"split-window -h\"`")
    })?;
    let action = rest.collect::<Vec<String>>().join(" ");
    if action.is_empty() {
        // The vocabulary is READ from the type that defines it, never re-spelled here: this
        // message's own copy had been missing `zoom-pane` since R289 shipped it, and nothing could
        // fail because a second list is not checked against anything.
        return Err(bad_input(&format!(
            "bind-key: {key:?} needs an action (there are: {})",
            BoundAction::VOCABULARY.join(", ")
        )));
    }
    // Parsed HERE, so a typo in an argument is reported as one. Rendering it through
    // `ConfigError` would prefix the message with `config.toml` and send the user to fix a file
    // that is fine.
    let key = KeySpec::parse(&key).map_err(|error| bad_input(&format!("bind-key: {error}")))?;
    let action =
        BoundAction::parse(&action).map_err(|error| bad_input(&format!("bind-key: {error}")))?;
    // Refused HERE as well as by `Keymap::bind`, and not instead of it: this is the argument error a
    // CLI user made, and the keymap's is the same contradiction arriving from a file. Naming it in
    // the caller's own terms is the rule every parse in this verb follows.
    if flags.repeat && flags.table == KeyTable::Root {
        return Err(bad_input(&format!(
            "bind-key: {}",
            sprag_host::keymap::KeyError::RepeatInRoot(key.to_string())
        )));
    }
    let path =
        sprag_host::config::bind_key(flags.table, &key, action, flags.repeat).map_err(|error| {
            io::Error::new(io::ErrorKind::InvalidData, format!("bind-key: {error}"))
        })?;
    // Named on stderr because it is the SURPRISING half: a tmux user expects a runtime bind to
    // vanish, and one that has quietly been written to a file they maintain deserves to be told
    // where. stdout stays empty so a script can pipe this without filtering.
    eprintln!("sprag: bound in {}", path.display());
    Ok(())
}

/// `unbind-key [-n] [-T TABLE] KEY`: take a key's meaning away — tmux `unbind-key`.
///
/// Removes the user's own binding, and — only when the DEFAULT keymap binds the key — records an
/// `[[unbind]]` so the default stays suppressed. See [`sprag_host::config::unbind_key`] for why
/// that condition is load-bearing rather than tidiness.
fn unbind_key(args: Vec<String>) -> io::Result<()> {
    let (flags, args) = strip_key_flags("unbind-key", args)?;
    // tmux's `unbind-key` has no `-r` either: repeat is a property of a binding, and this verb
    // removes one. Refused by NAME rather than ignored, the rule this file already follows for a
    // table it does not have.
    if flags.repeat {
        return Err(bad_input(
            "unbind-key: -r is bind-key's; repeat is a property of a binding, not of removing one",
        ));
    }
    let mut rest = args.into_iter();
    let key = rest
        .next()
        .ok_or_else(|| bad_input("unbind-key: needs a key, e.g. `unbind-key o`"))?;
    if let Some(extra) = rest.next() {
        return Err(bad_input(&format!(
            "unbind-key: unexpected argument {extra:?} (it takes one key)"
        )));
    }
    let key = KeySpec::parse(&key).map_err(|error| bad_input(&format!("unbind-key: {error}")))?;
    let path = sprag_host::config::unbind_key(flags.table, &key).map_err(|error| {
        io::Error::new(io::ErrorKind::InvalidData, format!("unbind-key: {error}"))
    })?;
    eprintln!("sprag: unbound in {}", path.display());
    Ok(())
}

/// The only option scope sprag has, and tmux's own flag for it.
///
/// tmux's `-g` selects the GLOBAL table rather than a session's or a window's. Every sprag option is
/// global — one client config file, no per-session or per-window overlay — so the flag carries no
/// information and is accepted as the spelling a tmux user's fingers produce. `-w` / `-p` are refused
/// BY NAME, and `set-window-option` / `show-window-options` are not verbs here at all: a scope with no
/// members would promise an overlay nothing holds. [`strip_key_flags`] applies the same rule one verb
/// over — it now ACCEPTS `-T root`, because slice 4 built the table that flag names, and refuses only
/// the tables sprag still does not have.
const GLOBAL_SCOPE: &str = "-g";

/// `show-options [-g] [-v] [NAME]`: print the options and the values in force — tmux
/// `show-options`, and with a NAME its `show-option` / herdr's singular.
///
/// **Needs no daemon**, like `list-keys`, and for the same reason: every option here is what one
/// CLIENT does with one attachment, so it lives in the user's config file rather than in the server.
///
/// With no NAME, EVERY option is printed, set or not — which is the whole point of having a table. A
/// user who does not already know an option's name cannot find it in a file that does not mention it,
/// and tmux answers the same question the same way. The shape is tmux's (`name value`, sorted) so a
/// script written against one reads the other.
///
/// `-v` prints the VALUE alone, which is what a script actually wants: `$(sprag show-options -v
/// prefix)` needs no `cut`, and a value that never shares a line cannot be mis-split by one.
fn show_options(args: Vec<String>) -> io::Result<()> {
    let (bare, rest) = strip_show_options_flags(args)?;
    let mut rest = rest.into_iter();
    let name = rest.next();
    if let Some(extra) = rest.next() {
        return Err(bad_input(&format!(
            "show-options: unexpected argument {extra:?} (it takes at most one option name)"
        )));
    }
    // Refused rather than defaulted to "all values, one per line": `-v` exists so a caller can read
    // ONE value without parsing, and answering it with a list would hand a script the very ambiguity
    // the flag was asked for to remove.
    if bare && name.is_none() {
        return Err(bad_input(&format!(
            "show-options: -v prints one option's value, so it needs a name (there are: {})",
            sprag_host::options::names()
        )));
    }
    let options = sprag_host::config::options().map_err(|error| {
        io::Error::new(io::ErrorKind::InvalidData, format!("show-options: {error}"))
    })?;
    let Some(name) = name else {
        for (name, value) in options.iter() {
            println!("{name} {}", rendered(name, value));
        }
        return Ok(());
    };
    // Refused with the list, like `set-option`'s: a name-keyed table is only usable if a mistyped
    // name answers with the real ones.
    let value = options.get(&name).ok_or_else(|| {
        bad_input(&format!(
            "show-options: {}",
            sprag_host::options::OptionError::Unknown(name.clone())
        ))
    })?;
    if bare {
        // RAW: a script asked for the value, not a rendering of it.
        println!("{value}");
    } else {
        println!("{name} {}", rendered(&name, value));
    }
    Ok(())
}

/// `value` as the `name value` form prints it — see
/// [`OptionKind::render`](sprag_host::options::OptionKind::render).
///
/// A name with no spec cannot reach here (both callers looked it up), but the fallback is the value
/// itself rather than a panic: a listing is not the place to die over a table inconsistency.
fn rendered(name: &str, value: &str) -> String {
    sprag_host::options::spec(name).map_or_else(|| value.to_owned(), |spec| spec.kind.render(value))
}

/// `set-option [-g] NAME VALUE` / `set-option [-g] -u NAME`: change one client option — tmux
/// `set-option`.
///
/// **This EDITS `config.toml`**, for the reason `bind-key` does: the file IS the live table, so a
/// client attached right now re-reads it, `show-options` prints it, and the next attach still has it.
/// One answer rather than three — and there is no second authority for a setting, which is what makes
/// `show-options` trustworthy. It needs no daemon.
///
/// The NAME and VALUE are validated HERE, into an
/// [`OptionSetting`](sprag_host::options::OptionSetting), so a typo in an argument is reported as one:
/// rendering it through `ConfigError` would prefix the message with `config.toml` and send a user to
/// fix a file that is fine.
fn set_option(args: Vec<String>) -> io::Result<()> {
    let (unset, rest) = strip_set_option_flags(args)?;
    let mut rest = rest.into_iter();
    let name = rest.next().ok_or_else(|| {
        bad_input(&format!(
            "set-option: needs an option and a value, e.g. `set-option prefix C-a` \
             (there are: {})",
            sprag_host::options::names()
        ))
    })?;
    let value = rest.next();
    if let Some(extra) = rest.next() {
        return Err(bad_input(&format!(
            "set-option: unexpected argument {extra:?} (it takes one option and one value)"
        )));
    }
    let path = if unset {
        if let Some(value) = value {
            return Err(bad_input(&format!(
                "set-option: -u removes an option, so it takes no value (got {value:?})"
            )));
        }
        let spec = sprag_host::options::spec(&name).ok_or_else(|| {
            bad_input(&format!(
                "set-option: {}",
                sprag_host::options::OptionError::Unknown(name.clone())
            ))
        })?;
        sprag_host::config::unset_option(spec)
    } else {
        let value = value.ok_or_else(|| {
            bad_input(&format!(
                "set-option: {name:?} needs a value (or -u to put it back to its default)"
            ))
        })?;
        let setting = sprag_host::options::OptionSetting::parse(&name, &value)
            .map_err(|error| bad_input(&format!("set-option: {error}")))?;
        sprag_host::config::set_option(&setting)
    }
    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("set-option: {error}")))?;
    // Named on stderr for `bind-key`'s reason: a tmux user expects a runtime set to vanish, and one
    // written into a file they maintain deserves to say where. stdout stays empty for a script.
    eprintln!(
        "sprag: {} in {}",
        if unset { "unset" } else { "set" },
        path.display()
    );
    Ok(())
}

/// Split `set-option`'s flags off its positional arguments: whether `-u` was given, and the rest.
///
/// Flags are taken until the first non-flag, which is tmux's own shape. A per-window / per-pane scope
/// is refused BY NAME rather than ignored, for [`GLOBAL_SCOPE`]'s reason.
fn strip_set_option_flags(args: Vec<String>) -> io::Result<(bool, Vec<String>)> {
    let mut unset = false;
    let mut rest = Vec::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            GLOBAL_SCOPE => {}
            "-u" => unset = true,
            "-w" | "-p" => {
                return Err(bad_input(&format!(
                    "set-option: {arg} names a per-window / per-pane option table, and sprag has \
                     one client table — only {GLOBAL_SCOPE} (or no flag) applies"
                )));
            }
            _ => {
                rest.push(arg);
                rest.extend(args);
                break;
            }
        }
    }
    Ok((unset, rest))
}

/// Split `show-options`' flags off its positional argument: whether `-v` was given, and the rest.
///
/// The read verb's half of [`strip_set_option_flags`], with the same shape (flags until the first
/// non-flag) and the same refusal of a scope that has no members.
fn strip_show_options_flags(args: Vec<String>) -> io::Result<(bool, Vec<String>)> {
    let mut bare = false;
    let mut rest = Vec::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            GLOBAL_SCOPE => {}
            "-v" => bare = true,
            "-w" | "-p" => {
                return Err(bad_input(&format!(
                    "show-options: {arg} names a per-window / per-pane option table, and sprag has \
                     one client table — only {GLOBAL_SCOPE} (or no flag) applies"
                )));
            }
            _ => {
                rest.push(arg);
                rest.extend(args);
                break;
            }
        }
    }
    Ok((bare, rest))
}

/// Strip the leading `[-nr] [-T TABLE]` flags from `args`, returning them and what is left.
///
/// # Where flag parsing STOPS, and why it has to
///
/// At the first token that is not a recognised flag — not at the first token that does not start
/// with `-`. A key spec may BE `-`, and `bind-key - split-window -h` has to keep working; a parser
/// that treated every leading dash as a flag would take that user's key away.
///
/// `-n`, `-r` and the combined `-nr` are one rule (`-` followed by those letters), because tmux's
/// own synopsis is spelled `bind-key [-nr]` — that is the form a user copies out of the manual.
///
/// `-n` and `-T` are the same flag: tmux documents `-n` as an alias for `-T root`. Given both, they
/// have to AGREE — `-n -T prefix` is two contradictory statements about one binding, and picking
/// either one would be inventing a precedence rule the user cannot see.
fn strip_key_flags(verb: &str, args: Vec<String>) -> io::Result<(KeyFlags, Vec<String>)> {
    let mut flags = KeyFlags {
        table: KeyTable::Prefix,
        repeat: false,
    };
    let mut named_root = false;
    let mut named_table = None;
    let mut rest = args.into_iter().peekable();
    while let Some(token) = rest.peek() {
        let letters = token.strip_prefix('-').filter(|rest| {
            !rest.is_empty() && rest.chars().all(|letter| letter == 'n' || letter == 'r')
        });
        if let Some(letters) = letters {
            named_root |= letters.contains('n');
            flags.repeat |= letters.contains('r');
            rest.next();
            continue;
        }
        if token != "-T" {
            break;
        }
        rest.next();
        let name = rest
            .next()
            .ok_or_else(|| bad_input(&format!("{verb}: -T needs a table name")))?;
        let table =
            KeyTable::parse(&name).map_err(|error| bad_input(&format!("{verb}: {error}")))?;
        named_table = Some(table);
    }
    flags.table = match (named_root, named_table) {
        (true, Some(KeyTable::Prefix)) => {
            return Err(bad_input(&format!(
                "{verb}: -n and -T {:?} contradict; -n IS -T {:?}",
                KeyTable::Prefix.as_str(),
                KeyTable::Root.as_str()
            )));
        }
        (true, _) => KeyTable::Root,
        (false, Some(table)) => table,
        (false, None) => KeyTable::Prefix,
    };
    Ok((flags, rest.collect()))
}

/// An argument the CLI will not take, as the error every verb here reports.
fn bad_input(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.to_owned())
}

fn print_usage() {
    eprintln!(
        "usage: sprag <ls | list-clients [-t SESSION] | new [name]\n\
         \x20             | attach NAME [--no-wait | --tui | --remote HOST]\n\
         \x20             | ssh [user@]host [-p PORT] [-L FWD]… [--tmux[=NAME]] [-- command…]\n\
         \x20             | find NEEDLE [-t SESSION] [--pane N] [--regex]\n\
         \x20             | wait-for-output --pane N NEEDLE [-t SESSION] [--regex]\n\
         \x20             | kill-session NAME | kill-server [--purge]>\n\
         \x20      sprag <windows | new-window [name] | select-window NAME\n\
         \x20             | rename-window [window] NAME | kill-window [window]\n\
         \x20             | resize-window [window]\n\
         \x20                 <-x COLS -y ROWS | -a | -A | -L/-R/-U/-D N | -u>\n\
         \x20             | break-pane PANE [name] | join-pane PANE WINDOW\n\
         \x20             | move-pane PANE -h|-v [-b] TARGET\n\
         \x20             | swap-pane [PANE] <WITH | -L|-R|-U|-D>> -t SESSION\n\
         \x20      sprag <panes | layout | processes [PANE]\n\
         \x20             | select-pane <PANE | -L|-R|-U|-D>\n\
         \x20             | split-window [-h|-v [-b] [PANE]] [-- command…]\n\
         \x20             | kill-pane [PANE]\n\
         \x20             | resize-pane [PANE] -x COLS -y ROWS\n\
         \x20             | zoom-pane [PANE] [-u]\n\
         \x20             | rename-pane PANE <NAME | --clear>\n\
         \x20             | send-keys PANE [-l] KEY… | capture-pane PANE [-p]\n\
         \x20             | agent [PANE]> [-t SESSION]\n\
         \x20      sprag report-agent <working|blocked|idle> [-t SESSION] [--pane N]\n\
         \x20             [--source S] [--name AGENT] [--seq N]\n\
         \x20      sprag release-agent [-t SESSION] [--pane N]\n\
         \x20      sprag <install-hooks | uninstall-hooks> [AGENT…] [--yes] [--dry-run]\n\
         \x20      sprag list-hooks\n\
         \x20      sprag events [-t SESSION] [--since N] [-f]\n\
         \x20      sprag <list-keys | bind-key [-nr] [-T prefix|root] KEY ACTION…\n\
         \x20             | unbind-key [-n] [-T prefix|root] KEY>\n\
         \x20      sprag <show-options [-v] [NAME] | set-option [-u] NAME [VALUE]> [-g]\n\
         \x20      sprag <--version | --help>"
    );
}

/// `--version`: the build, on STDOUT, contacting no daemon.
///
/// Every other command here needs a running server; this one must not, because the first question
/// asked of a tool that is behaving oddly is which build it is — and a version that only answers
/// while the daemon is healthy cannot answer it. It is also the process-start CONTROL every
/// latency comparison subtracts (R278's harness, R281's re-measurement): a command that does the
/// fork/exec and nothing else. Until R281 sprag had none, so that control was measuring the
/// unknown-command path — which prints usage to STDERR and exits 2, i.e. neither the same work nor
/// a success, and the harness recorded the exit code for two rounds before anyone read it.
fn print_version() {
    println!("sprag {}", env!("CARGO_PKG_VERSION"));
}

/// Env override: the `sprag-gui` binary [`attach`] launches (else the sibling of this exe — they
/// install together — else `sprag-gui` on `PATH`). Mirrors the GUI's own `SPRAG_GUI_HOST_BIN`
/// discovery of `sprag-term`. Resolved by [`client_bin`].
const GUI_BIN_ENV: &str = "SPRAG_GUI_BIN";

/// Env override: the `sprag-tui` binary [`attach`] `--tui` execs, resolved the same three ways as
/// [`GUI_BIN_ENV`] by [`client_bin`]. Separate from it because the two clients are separate
/// artifacts a caller may want to point somewhere else independently — a test standing in for one
/// must not silently redirect the other.
const TUI_BIN_ENV: &str = "SPRAG_TUI_BIN";

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
///
/// The message names the endpoint WITH its provenance ([`sprag_rpc::HostEndpoint`]), not just its
/// path: "no server running at
/// /run/user/1000/sprag-host.sock" leaves an operator who overrode the socket — or who meant to
/// and did not — with no way to tell which of those they are looking at. That ambiguity is how a
/// probe pointed at one daemon ended up driving another (R278).
/// It also SHAKES HANDS, for the direction the daemon's own check cannot cover: a daemon older
/// than this CLI answers the unknown protocol param happily, and the mismatch would then surface
/// as a misread slot rather than a sentence. One extra request on a connection this command
/// already holds — herdr, which checks only client-side, spends a whole extra round trip per
/// request to ask the same question.
///
/// # `deadline` is stated BEFORE the connection exists, and that is the point
///
/// This function now performs I/O of its own (the handshake), so a caller cannot bound it
/// afterwards — by the time it holds a `HostConn` the unbounded read has already happened. That is
/// not hypothetical: adding the handshake without this parameter re-opened the exact hole R273
/// closed, and `a_wedged_daemon_cannot_stall_the_agents_hook` caught it. Every caller now answers
/// "how long will you wait" as a condition of getting a connection at all.
///
/// `None` means wait indefinitely, which is right for a MANAGEMENT command: it is a person's
/// command in a person's terminal, and they can interrupt it. `Some` is for the paths that run
/// inside somebody else's process while that process waits.
fn connect(deadline: Option<Duration>) -> io::Result<HostConn> {
    let endpoint = HostEndpoint::for_opts(HOST_SOCKET);
    let mut conn = HostConn::connect(endpoint.path(), CONNECT_TIMEOUT).map_err(|_| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("no server running at {endpoint}"),
        )
    })?;
    // Set BEFORE the handshake, so the very first reply this process waits for is covered.
    if let Some(deadline) = deadline {
        conn.set_read_deadline(Some(deadline))?;
    }
    conn.handshake(&cli_client_id())?;
    Ok(conn)
}

/// This invocation's client id for the handshake — `cli-<pid>`.
///
/// A CLI command is a client that connects, asks, and leaves; it never attaches, so the id only
/// has to be distinct from the display clients' (`gui-…`, [`sprag_rpc::gui_client_prefix`]) and
/// stable for the process. Named rather than empty because the daemon logs it, and "which client
/// was that" is a question an operator asks of a log.
fn cli_client_id() -> String {
    format!("cli-{}", std::process::id())
}

/// `ls`: one line per session — its name, its window count, which one an unscoped request lands
/// in, how many clients are attached (viewing) it, and (where known) its current working
/// directory, git branch, and the TCP ports it is listening on. The GUI sidebar shows only the
/// cwd's basename to fit the rail; the FULL path is here.
///
/// # Two reads, and why this one asks for a FRESH sample
///
/// The structure comes from the `sessions` slot and the last three facts from
/// `session_activity` (R282), joined by NAME — a session's address — rather than by position,
/// since a session created between the two requests would otherwise shift every row after it.
///
/// The activity read declares a tolerance of ZERO, so the daemon samples for this command rather
/// than handing it whatever a GUI's poll last took. A sidebar can be a second behind the world; a
/// one-shot command an operator runs to see which port is taken cannot, because its answer stops
/// updating the instant it is printed and is then read for as long as somebody looks at it. That
/// costs this command one `/proc` walk, which is a cost it asked for.
fn ls() -> io::Result<()> {
    let mut conn = connect(None)?;
    let sessions = conn.call(
        "scene/query",
        json!({ "path": mux_action_path(SESSIONS_SLOT) }),
    )?;
    // Best-effort: a daemon too old to serve the family leaves every line in its structural form
    // rather than failing a listing (`sprag ls` answers "what may I name?" first and foremost). The
    // wire protocol makes that skew a refusal at the door, so this is belt to that suspenders.
    let activity = conn
        .call(
            "scene/query",
            json!({ "path": mux_action_path(&session_activity_at(0)) }),
        )
        .unwrap_or(Value::Null);
    for session in sessions.as_array().into_iter().flatten() {
        let name = session["name"].as_str().unwrap_or("?");
        let windows = session["windows"].as_u64().unwrap_or(0);
        let marker = if session["default"].as_bool().unwrap_or(false) {
            " (default)"
        } else {
            ""
        };
        // This session's row of the sample, by name. `Null` for a session the sample does not carry
        // — an older daemon, or one created since it was taken — and every field below then falls
        // away, degrading the line to its structural form rather than inventing a fact.
        let row = activity["sessions"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|row| row["name"].as_str() == Some(name))
            .unwrap_or(&Value::Null);
        let cwd = row["cwd"].as_str().unwrap_or("");
        let suffix = match (cwd, row["branch"].as_str()) {
            ("", None) => String::new(),
            ("", Some(branch)) => format!("  [{branch}]"),
            (cwd, None) => format!("  {cwd}"),
            (cwd, Some(branch)) => format!("  {cwd} [{branch}]"),
        };
        // A `:3000 :8080` badge; empty (serving nothing, or no sample) falls away.
        let ports = row["ports"]
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
    let mut conn = connect(None)?;
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
        // The area the client reported, in tmux's own `[COLSxROWS]` shape. Omitted rather than
        // faked when it has not reported one: this is what `window-size` arbitrates over, so a
        // client that is not in the arbitration must not read as though it were.
        match (
            client["size"]["cols"].as_u64(),
            client["size"]["rows"].as_u64(),
        ) {
            (Some(cols), Some(rows)) => println!("{id}: {session} [{cols}x{rows}]"),
            _ => println!("{id}: {session}"),
        }
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
    let mut conn = connect(None)?;
    if let Some(session) = &session {
        require_session(&mut conn, session)?;
    }
    let scoped = |path: String| scoped_params(session.as_deref(), path);
    let mut panes = pane_ids(&mut conn, session.as_deref())?;
    if let Some(only) = only {
        require_pane(&mut conn, session.as_deref(), only, "find")?;
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

/// `sprag wait-for-output --pane N NEEDLE [-t SESSION] [--regex]` — BLOCK until that pane's retained
/// output matches, then print the matching lines exactly as `find` does.
///
/// ## The verb `find` could not be
///
/// `find` answers "does it say this NOW"; a caller that wants "tell me WHEN it says this" has, until
/// now, had to run `find` in a loop — which is a poll, and a poll against a terminal is the thing
/// this project keeps removing. The daemon already knows when a pane produced output, so it can
/// answer the question the first time it becomes true rather than the first time somebody asks
/// again.
///
/// The two verbs share their SEARCH and their OUTPUT FORMAT because they are one question asked in
/// two tenses: the same `find.<needle>` / `regex.<pattern>` languages, the same `PaneFind`, the same
/// `PANE:LINE: text` lines. A second spelling of either would be the drift the pairing exists to
/// prevent.
///
/// ## `--pane` is REQUIRED here where `find` sweeps
///
/// A sweep answers "somewhere in this window"; a wait has to name its subject, because the answer is
/// "this pane said it" and a park is on one pane's output. Sweeping would mean parking a wait per
/// pane and racing them, which is a different feature with a different answer shape.
///
/// ## No read deadline, deliberately
///
/// Waiting indefinitely is this verb's contract, exactly as it is `events -f`'s: every other verb
/// here is a request-response against a daemon answering from memory, so a reply that has not
/// arrived in seconds is not coming — but a parked wait that has not answered has simply not
/// happened yet. A caller that wants a bound uses its shell's (`timeout 60 sprag wait-for-output …`),
/// which is exact where a daemon-side clock would not be.
fn wait_for_output(args: Vec<String>) -> io::Result<()> {
    let FindArgs {
        needle,
        session,
        pane,
        regex,
    } = find_args_named(args, "wait-for-output")?;
    let pane = pane.ok_or_else(|| {
        bad_input("wait-for-output: --pane N is required (a wait names the pane it watches)")
    })?;
    let mut conn = connect(None)?;
    if let Some(session) = &session {
        require_session(&mut conn, session)?;
    }
    // Checked before the park for the reason the daemon checks it too: a wait on a pane that is not
    // there cannot be answered and cannot fail, so it would read as "it has not happened yet".
    require_pane(&mut conn, session.as_deref(), pane, "wait-for-output")?;
    // The park's contract is to block; a deadline here would turn "not yet" into an error.
    conn.set_read_deadline(None)?;
    // The scope is spelled the one way this file spells it (`scoped_only`), then the wait's own two
    // keys are added — so a request that names a session here names it the same as every other verb.
    let mut params = scoped_only(session.as_deref());
    params[PANE_PARAM] = Value::from(pane);
    // Which LANGUAGE the needle is in decides which KEY carries it, exactly as it decides which
    // ADDRESS `find` queries. One string cannot mean both.
    params[if regex { PATTERN_PARAM } else { NEEDLE_PARAM }] = Value::from(needle);
    // Through `try_call` so a REFUSAL reaches the operator as the daemon's own sentence rather than
    // behind `host rpc error:`, which is a transport's phrase for a fault nobody could anticipate.
    // The refusals this method produces are all things the caller can act on — a pane in another
    // session, two search languages at once, or a daemon too old to speak this method at all (the
    // skew case, where the answer names every method it DOES serve). Debt item 20's class, fixed at
    // birth on this verb rather than joining the list of verbs that still leak.
    let answer = match conn.try_call(PANE_WAIT_OUTPUT_METHOD, params) {
        Ok(answer) => answer,
        Err(CallError::Fault(fault)) => {
            return Err(bad_input(&format!(
                "wait-for-output: {}",
                fault_sentence(&fault)
            )));
        }
        Err(other) => return Err(other.into()),
    };
    let found: PaneFind = serde_json::from_value(answer["find"].clone()).unwrap_or_default();
    // A refused pattern comes back as a RESULT carrying the engine's message, not as a fault — the
    // taxonomy `regex.<pattern>` uses, because an invalid pattern is a well-formed question whose
    // value the engine rejected. It is still an error to the OPERATOR, so it exits non-zero.
    if let Some(error) = found.error {
        return Err(bad_input(&format!(
            "wait-for-output: invalid pattern: {error}"
        )));
    }
    for line in &found.lines {
        println!("{pane}:{}: {}", line.line, line.text);
    }
    if found.truncated {
        eprintln!("sprag: wait-for-output: the answer was capped; later matches were not scanned");
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
    find_args_named(args, "find")
}

/// [`find_args`], with the VERB in every message — so `wait-for-output` shares one parser with the
/// search it is the blocking tense of, rather than owning a second copy that can drift from it.
///
/// One parser is the point: the two verbs take the same needle in the same two languages, and a
/// caller who learns `--regex` on one has learnt it on the other. What differs is only what each
/// does with `pane` (optional for a sweep, required for a park), which is the CALLER's check
/// because it is the caller's semantics.
fn find_args_named(args: Vec<String>, verb: &str) -> io::Result<FindArgs> {
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
                        .ok_or_else(|| bad(format!("{verb}: -t needs a session name")))?,
                );
            }
            "--pane" => {
                let value = it
                    .next()
                    .ok_or_else(|| bad(format!("{verb}: --pane needs a pane id")))?;
                pane = Some(value.parse::<u64>().map_err(|_| {
                    bad(format!(
                        "{verb}: --pane {value:?} is not a pane id (a number)"
                    ))
                })?);
            }
            "--regex" => regex = true,
            _ if needle.is_none() => needle = Some(arg),
            other => {
                return Err(bad(format!(
                    "{verb}: unexpected argument {other:?} (quote a multi-word needle)"
                )));
            }
        }
    }
    let needle = needle.ok_or_else(|| bad(format!("{verb}: a search needle is required")))?;
    if needle.is_empty() {
        return Err(bad(format!("{verb}: the search needle is empty")));
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
    let mut conn = connect(None)?;
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
    let mut conn = connect(None)?;
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

/// `attach NAME`: attach a client to session NAME — tmux `attach-session -t`.
///
/// Three clients, chosen by flag, and the flag decides far more than which binary runs:
///
/// | | `attach NAME` | `--tui` | `--remote HOST` |
/// |---|---|---|---|
/// | client | a `sprag-gui` window | `sprag-tui`, in this terminal | `sprag-tui`, on HOST |
/// | launched by | spawn + [`own_session`] | [`CommandExt::exec`] | `exec ssh -t` |
/// | pre-flight | this daemon | this daemon | none — the session is HOST's |
///
/// The PRE-FLIGHT is connect-only, like every other command: it verifies NAME exists on the
/// running daemon FIRST, so a typo is a clean "no session" error, not a client that starts and
/// dies on its first (failed) scoped read. Only then is the client launched, handed
/// `SPRAG_GUI_SESSION=NAME` (the attach env `sprag_client`'s `resolve_session` consumes → adopt
/// the session's live panes) and `SPRAG_GUI_HOST_SOCK` pinned to the EXACT socket this CLI
/// reached — so the client joins the daemon we just checked, never a different default it might
/// connect-or-spawn. (Both env vars are named for the GUI and are now read by two clients; the
/// rename is a compatibility change owed to `sprag-client`, `sprag-gui`, `sprag-smoke` and this
/// CLI together, not to `attach` alone.)
///
/// ## The window is SPAWNED, the terminal client EXEC'd — opposite relationships to this terminal
///
/// A window is a second thing on screen and must OUTLIVE the shell that opened it, so it is
/// spawned into a session of its own ([`own_session`]) and a hangup here cannot reach it.
///
/// A terminal client is the exact opposite: it IS this terminal's foreground program, and
/// `own_session` does not merely inconvenience it — it makes the client IMPOSSIBLE. `setsid`
/// leaves the child with no CONTROLLING terminal, and a terminal client acquires its terminal by
/// opening `/dev/tty`, which is precisely the name for "the controlling terminal of this process".
/// MEASURED, both arms inside one real pty: run directly the client takes the terminal (it emits
/// its cursor sequences); run through `setsid` it prints `No such device or address (os error 6)`
/// — ENXIO, from termwiz's `SystemTerminal::new`, on the client's very FIRST line, before it has
/// connected to anything. (A second consequence would follow if that one did not fire first: the
/// kernel delivers SIGWINCH to the foreground process group OF THE TTY, so a client outside this
/// session would never be told the window changed.)
///
/// So the terminal client keeps this session, this process group, and this pid, by REPLACING the
/// process image rather than spawning at all. `exec` is what makes "keeps" exact: job control
/// (`Ctrl-Z`, `Ctrl-C`) addresses the client itself, its exit status IS this command's without
/// anything having to wait for it, and no parent is left sitting on the tty to get any of that
/// subtly wrong.
///
/// That is also why `--no-wait` is REFUSED with `--tui` / `--remote` rather than accepted and
/// ignored: it exists to give a shell back, and a client that holds the terminal until it
/// detaches has no moment at which it could.
///
/// ## `--remote HOST` does not pre-flight, and cannot
///
/// The host socket is `AF_UNIX` — there is no wire off this machine, deliberately (a TCP listener
/// is a separate front with its own threat model). So "attach to a session on HOST" can only mean
/// "run a client on HOST", which is why `--remote` implies `--tui`: `ssh -t HOST sprag attach
/// --tui NAME`. The session named is HOST's, held by HOST's daemon, so checking it here would
/// refuse a perfectly good remote name — or, worse, accept a local session that merely shares it.
/// `SPRAG_GUI_HOST_SOCK` is likewise a path meaningful only on this machine, so it is not exported
/// either; the remote CLI resolves the remote default. `--remote` therefore never opens a local
/// connection at all and works with no local daemon running.
///
/// ## Blocking or returning is the CALLER's choice, because neither answer is right for everyone
///
/// Default: hold the terminal until the window closes, tmux's `attach-session` shape, and the
/// reading under which the window's exit status IS this command's. `--no-wait` returns as soon as
/// the window is up, the shape of launching a GUI from a shell you want back.
///
/// `--no-wait` is NOT "spawn and hope". It returns only once the DAEMON has witnessed the window
/// as an attached client ([`await_window`]), so a window that dies on startup — no display, a
/// broken binary — is still this command's failure rather than a silent exit 0 and a prompt that
/// looks fine. That check costs almost nothing: spawn-to-attached measured 0.13-0.22s, far under
/// what a person reads as a pause. It is also something neither tmux nor cmux can offer, for a
/// structural reason rather than an oversight — it needs a daemon that can SEE its clients as data
/// and a client id the launcher can recognise ([`sprag_rpc::gui_client_prefix`]).
///
/// Not spelled `-d`: tmux's `attach-session -d` means "detach every OTHER client", a different
/// thing entirely, and one this flag would silently shadow for anyone with the muscle memory.
fn attach(args: Vec<String>) -> io::Result<()> {
    let bad = |message: String| io::Error::new(io::ErrorKind::InvalidInput, message);
    let mut name: Option<String> = None;
    let mut wait = true;
    let mut tui = false;
    let mut remote: Option<String> = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--no-wait" => wait = false,
            "--tui" => tui = true,
            // The destination is the NEXT token, as `sprag ssh` spells `-p` and `-L`, rather than
            // a `--remote=HOST` of its own — one convention for "flag takes a value" in this CLI.
            "--remote" => {
                remote = Some(
                    args.next()
                        .ok_or_else(|| bad("attach --remote needs a host".to_owned()))?,
                );
            }
            _ if name.is_none() => name = Some(arg),
            other => return Err(bad(format!("attach: unexpected argument {other:?}"))),
        }
    }
    let name = name.ok_or_else(|| bad("attach needs a session name".to_owned()))?;
    if !wait && (tui || remote.is_some()) {
        return Err(bad(
            "attach --no-wait belongs to the window client; a terminal client IS this terminal, \
             so there is nothing to return to"
                .to_owned(),
        ));
    }
    if let Some(destination) = remote {
        return attach_remote(&destination, &name);
    }
    let sock = socket_path(HOST_SOCKET);
    let mut conn = connect(None)?;
    if !session_exists(&mut conn, &name)? {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("no session named {name:?}"),
        ));
    }
    // Hand the client the session to adopt and the exact socket we reached; do NOT let it fall
    // back to its own default, which could be a different daemon.
    let mut command = Command::new(client_bin(
        if tui { TUI_BIN_ENV } else { GUI_BIN_ENV },
        if tui { "sprag-tui" } else { "sprag-gui" },
    ));
    command
        .env("SPRAG_GUI_SESSION", &name)
        .env("SPRAG_GUI_HOST_SOCK", &sock);
    if tui {
        // The pre-flight's connection belongs to a process that is about to stop existing. Close
        // it HERE rather than leave the daemon's client accounting resting on the descriptor's
        // close-on-exec flag, which is a property of how the socket was opened, not of this code.
        drop(conn);
        return Err(exec_client(&mut command, "sprag-tui"));
    }
    let mut child = own_session(&mut command).spawn().map_err(|error| {
        io::Error::new(error.kind(), format!("could not launch sprag-gui: {error}"))
    })?;
    if !wait {
        return await_window(&mut conn, &mut child);
    }
    let status = child.wait().map_err(|error| {
        io::Error::new(error.kind(), format!("could not launch sprag-gui: {error}"))
    })?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("sprag-gui exited with {status}")))
    }
}

/// How long [`await_window`] gives a window to reach the daemon before calling it a failure.
///
/// Generous against the measured cost — spawn-to-attached runs 0.13-0.22s — because the thing it
/// must not do is fail a window that is merely slow: a first launch on a cold GPU driver pays for
/// shader and pipeline setup that a warm one does not, and a false failure here would send someone
/// hunting a bug that is not there. Overshooting only delays the report of a genuine failure.
const WINDOW_ATTACH_TIMEOUT: Duration = Duration::from_secs(10);

/// Wait until `child`'s window is ATTACHED to the daemon, then return, leaving it running.
///
/// The success condition is the daemon's own client list, not the child still being alive, and the
/// difference is the point: a process that started and then failed to reach the daemon is exactly
/// the failure a caller wants reported, and only the daemon can tell us it never arrived. The
/// window is matched by [`sprag_rpc::gui_client_prefix`] on the pid we just spawned, so a
/// concurrently-launched window of someone else's cannot be mistaken for ours.
///
/// Three outcomes, all of them honest: it attached (`Ok`), it exited first (`Err`, carrying the
/// status, since that is the diagnosis), or it never arrived within [`WINDOW_ATTACH_TIMEOUT`]
/// (`Err`, and the window is deliberately left alone — we spawned it into its own session, and
/// killing something that may simply be slow to paint would be the more destructive guess).
fn await_window(conn: &mut HostConn, child: &mut std::process::Child) -> io::Result<()> {
    let prefix = sprag_rpc::gui_client_prefix(child.id());
    let deadline = std::time::Instant::now() + WINDOW_ATTACH_TIMEOUT;
    loop {
        // The child FIRST: a window that has already exited will never appear in the client list,
        // so asking the daemon again would only spend the whole timeout to reach a worse message.
        if let Some(status) = child.try_wait()? {
            return Err(io::Error::other(format!(
                "sprag-gui exited with {status} before its window attached"
            )));
        }
        let clients: Value = conn.call(
            "scene/query",
            json!({ "path": mux_action_path(CLIENTS_SLOT) }),
        )?;
        if clients
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|client| client["client"].as_str())
            .any(|client| client.starts_with(&prefix))
        {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "the window did not attach within {}s; it is still running, so check it \
                     with `sprag list-clients`",
                    WINDOW_ATTACH_TIMEOUT.as_secs()
                ),
            ));
        }
        std::thread::sleep(WINDOW_ATTACH_POLL);
    }
}

/// The gap between [`await_window`]'s checks — short enough that the common case (a window up in
/// ~0.15s) is not rounded up into a visible pause, long enough not to spin the daemon.
const WINDOW_ATTACH_POLL: Duration = Duration::from_millis(25);

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

/// Whether the running daemon can ADDRESS a session named `name` — asked of the scope resolver,
/// which is the only authority for it, by making the cheapest scoped request there is and reading
/// whether the scope resolved.
///
/// # Why not the `sessions` listing, which this used to scan
///
/// It was wrong twice, and R281 measured both.
///
/// **Wrong as an answer.** `sessions` is the HUMAN listing: it drops a resting empty anchor
/// (`SessionInfo::is_listable`), which is a session the daemon holds, serves, and refuses to let
/// anyone re-create. Validating an ADDRESS against a list filtered for reading made that session
/// unaddressable from its own CLI — on a fresh daemon, `sprag panes -t 0` answered `no session
/// named "0"` in the same breath as `sprag new 0` answered `a session named "0" already exists`.
/// A listing and an address book are different questions, and only one of them may hide a row.
///
/// **Wrong as a cost.** That listing attributes each session's listening ports through a whole
/// `/proc` walk (`ProcScan::scan`), which no caller of this function reads. Measured on the R278
/// harness: the same request costs **0.010 ms** with no live pane anywhere and **4.111 ms** with
/// one — and since every scoped command pre-flights through here, that was **87% of a `sprag`
/// invocation's wall time**, spent to answer a yes/no. (The listing paying it for a reader that
/// does want ports is a separate, still-open item; this one simply stops asking.)
///
/// `windows` is the request because a scope is resolved BEFORE any handler runs (`crate::rpc`'s
/// `handle_parsed`), so any scoped path answers the question; this is the cheapest one, and its
/// own answer is discarded.
fn session_exists(conn: &mut HostConn, name: &str) -> io::Result<bool> {
    match conn.try_call(
        "scene/query",
        json!({ "session": name, "path": mux_action_path(WINDOWS_SLOT) }),
    ) {
        Ok(_) => Ok(true),
        // The daemon heard the request and refused the SCOPE — that IS the answer. Read from the
        // JSON-RPC code rather than from its sentence: a scoped query carries nothing else that
        // can be invalid, and matching wording would make this file depend on how another crate
        // phrases itself.
        Err(CallError::Fault(fault)) if fault.code == INVALID_PARAMS => Ok(false),
        // Anything else — a dead socket, a protocol mismatch, a fault of another code — is this
        // call FAILING, and must never read as "no such session".
        Err(error) => Err(error.into()),
    }
}

/// `attach --remote HOST NAME`: run this client on HOST instead, over ssh.
///
/// The argv is built by [`SshTarget::ssh_argv`] — the SAME author `sprag ssh` uses — so there is
/// one spelling of "how sprag invokes ssh" rather than two that can drift apart, and `--remote
/// me@host` gets the `user@` split for free. `-t` comes from that builder and is load-bearing
/// here: ssh allocates a remote pty automatically only when NO command is given, and the command
/// given here is a full-screen client, which without a terminal cannot even set raw mode.
///
/// `--tui` is spelled out in the remote argv rather than left to the remote's default, because
/// the remote `sprag` is a DIFFERENT build: an argv that names the client it wants cannot be
/// re-read by a version whose default is something else. (Nothing yet negotiates that skew — see
/// the version-handshake debt; naming the client is the cheap half that does not need it.)
fn attach_remote(destination: &str, name: &str) -> io::Result<()> {
    let mut target = SshTarget::parse(destination)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    target.remote_command = vec![
        "sprag".to_owned(),
        "attach".to_owned(),
        "--tui".to_owned(),
        name.to_owned(),
    ];
    let argv = target.ssh_argv();
    let (program, rest) = argv
        .split_first()
        .expect("ssh_argv always names the program");
    let mut command = Command::new(program);
    command.args(rest);
    Err(exec_client(&mut command, program))
}

/// Replace THIS process with `command` — how both terminal clients are launched (see [`attach`]).
///
/// Returns only on failure, and so returns the error itself rather than a `Result` whose `Ok` is
/// unreachable: on success there is no longer a process here to return into.
fn exec_client(command: &mut Command, what: &str) -> io::Error {
    let error = command.exec();
    io::Error::new(error.kind(), format!("could not launch {what}: {error}"))
}

/// The client binary [`attach`] launches: `env_override` if set, else the sibling of this exe,
/// else `bin` on `PATH` — mirroring the GUI's own `host_bin` discovery of `sprag-term`.
///
/// The SIBLING step is what makes a build tree work uninstalled: `target/debug/sprag` finds the
/// `target/debug/sprag-gui` beside it, where `PATH` alone would find nothing — or, worse, an
/// installed client of a different version against this daemon.
fn client_bin(env_override: &str, bin: &str) -> PathBuf {
    if let Some(path) = std::env::var_os(env_override) {
        return PathBuf::from(path);
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(sibling) = exe.parent().map(|dir| dir.join(bin))
        && sibling.exists()
    {
        return sibling;
    }
    PathBuf::from(bin)
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
    let mut conn = connect(None)?;
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
    let mut conn = connect(None)?;
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

/// The request params addressing `path`, carrying the out-of-band `session` scope when the caller
/// named one — the ONE place a scoped request is shaped, so every command spells the scope the same
/// way the GUI does ([`sprag_host::wire::SESSION_PARAM`]).
///
/// `None` sends NO `session` key rather than a name this CLI guessed: absent means "the daemon's
/// default session", which is a decision the daemon owns and can move, and inventing a name here
/// would freeze today's answer into the wire.
fn scoped_params(session: Option<&str>, path: String) -> Value {
    match session {
        Some(name) => json!({ "session": name, "path": path }),
        None => json!({ "path": path }),
    }
}

/// The scope ALONE, for the two methods that read no path — `scene/revision` and `scene/waitFor`.
/// Kept beside [`scoped_params`] so the one way a request names its session is spelled once.
fn scoped_only(session: Option<&str>) -> Value {
    match session {
        Some(name) => json!({ "session": name }),
        None => json!({}),
    }
}

/// [`scoped_params`] plus an action's `args` — the invoke shape, kept beside the query shape so the
/// two cannot drift.
fn scoped_invoke(session: Option<&str>, path: String, args: Value) -> Value {
    let mut params = scoped_params(session, path);
    params["args"] = args;
    params
}

/// Split a PANE-subject subcommand's args into its OPTIONAL `-t SESSION` scope and everything else,
/// which the verb then parses itself.
///
/// The optionality is the whole difference from [`target_and_rest`], and it is principled rather
/// than lenient: see the module docs' "which commands take `-t`". Scanning STOPS at a bare `--`,
/// which is passed through with everything after it — a command run in a new pane may perfectly
/// well contain `-t`, and it belongs to that command, not to this parse.
fn scope_and_rest(args: Vec<String>, command: &str) -> io::Result<(Option<String>, Vec<String>)> {
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
            "--" => {
                rest.push(arg);
                rest.extend(it.by_ref());
            }
            _ => rest.push(arg),
        }
    }
    Ok((session, rest))
}

/// Connect, and pre-flight an explicitly named session so a typo is a clean error rather than a
/// raw scope refusal — the pane verbs' shared opening, mirroring the window verbs'
/// [`require_session`]. An ABSENT scope is not checked: the default session is whatever the daemon
/// says it is, and a client that names nothing cannot have named it wrong.
fn connect_scoped(session: Option<&str>) -> io::Result<HostConn> {
    let mut conn = connect(None)?;
    if let Some(session) = session {
        require_session(&mut conn, session)?;
    }
    Ok(conn)
}

/// Name the scope in an error message: the session the caller asked for, or the honest stand-in for
/// the one they did not name.
fn scope_name(session: Option<&str>) -> &str {
    session.unwrap_or("the default session")
}

/// The ids of the panes the scoped session's CURRENT window holds — the one read behind every
/// pane-id check and the `panes` listing, so a client and the daemon cannot disagree on which panes
/// are addressable.
fn pane_ids(conn: &mut HostConn, session: Option<&str>) -> io::Result<Vec<u64>> {
    let listed: Value = conn.call(
        "scene/query",
        scoped_params(session, mux_action_path(PANES_SLOT)),
    )?;
    Ok(listed
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|pane| pane["id"].as_u64())
        .collect())
}

/// Refuse cleanly if the scoped session's current window holds no pane `pane`, naming what IS
/// there — the pane-command pre-flight, and the pane-level peer of [`require_session`].
///
/// An absent pane is an ERROR rather than an empty result for the same reason `find --pane`'s is:
/// the caller named a specific pane, and answering as if it were merely quiet would be an answer to
/// a question they did not ask. It matters most for the verbs that address a pane's OWN external by
/// path ([`send_keys`], [`capture_pane`]) — there a wrong id is an unknown ADDRESS, whose raw
/// refusal says nothing about panes.
///
/// The sentence that used to end here — "unlike the mux actions' pane-level `Rejected`" — was
/// **false, and R283 measured it false**: a mux action's `Rejected` carries no payload at all, so
/// `sprag report-agent --pane 999` reached the operator as `scene/invoke
/// /sprag_mux/external/report_agent: host rpc error: InvokeRejected`. It is not pane-level; it is
/// not any level. See [`agent_refusal`] for what those two verbs say now, and
/// `claudedocs/PINION-PR82-*` for why they still cannot say only one thing.
fn require_pane(
    conn: &mut HostConn,
    session: Option<&str>,
    pane: u64,
    command: &str,
) -> io::Result<()> {
    let panes = pane_ids(conn, session)?;
    if panes.contains(&pane) {
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!(
            "{command}: no pane {pane} in {} (panes: {panes:?})",
            scope_name(session)
        ),
    ))
}

/// Read a scene SLOT, translating "this daemon has no such address" into a sentence about the
/// daemon rather than a Rust variant name.
///
/// # The failure this exists for, measured rather than imagined
///
/// A slot is ADDITIVE: a daemon older than this binary simply does not serve one added since it was
/// built, and pinion answers `UnknownIntrospectPath` — "not in its schema". That is the RIGHT
/// failure (a loud refusal, never a wrong answer that parses, which is why adding a slot needs no
/// `WIRE_PROTOCOL` bump), and R290 verified it with a control: the same `sprag` against a daemon
/// built at the parent commit read `panes` cleanly and refused `pane_processes.0`.
///
/// What it printed, though, was `host rpc error: UnknownIntrospectPath` — a Rust enum variant, at an
/// operator. That is exactly the class R283 measured and fixed for `report-agent` and
/// `release-agent`, which had been leaking `InvokeRejected` the same way; those were INVOKE paths
/// and this is the QUERY one. The remedy is the one the protocol-mismatch message already names, so
/// the wording matches it deliberately: a person who hits either should not have to learn that they
/// are two different mechanisms to know they want the same thing.
///
/// Every OTHER fault passes through untouched. This translates one refusal whose cause it can state;
/// dressing up faults it cannot explain would be the guess-as-fact [`agent_refusal`] refuses to make.
fn query_slot(conn: &mut HostConn, path: &str, command: &str) -> io::Result<Value> {
    match conn.try_call("scene/query", json!({ "path": path })) {
        Ok(value) => Ok(value),
        Err(CallError::Fault(fault)) => {
            Err(unknown_slot(command, path, &fault)
                .unwrap_or_else(|| CallError::Fault(fault).into()))
        }
        Err(other) => Err(other.into()),
    }
}

/// The refusal above as a pure function of the fault — `None` for anything else, which is what keeps
/// [`query_slot`] from dressing up a fault it cannot explain.
///
/// Matched on the fault's structured `data`, never on its rendered line: `Display` prefers `data`
/// and so the two agree today, but a substring test against a rendering is a test against a
/// presentation decision, and it would also fire on a daemon that merely mentioned the word.
/// Captured from a live daemon rather than invented — the reply is
/// `{"code":-32602,"message":"Invalid params","data":"UnknownIntrospectPath"}`.
fn unknown_slot(command: &str, path: &str, fault: &RpcFault) -> Option<io::Error> {
    if fault.data.as_ref().and_then(Value::as_str)? != "UnknownIntrospectPath" {
        return None;
    }
    Some(io::Error::new(
        io::ErrorKind::Unsupported,
        format!(
            "{command}: this daemon does not serve {path} — it is older than this `sprag`. \
             Restart it to bring it to this build — `sprag kill-server` (sessions are restored \
             from the durability snapshot)",
        ),
    ))
}

/// Turn a refused agent invoke into a sentence about PANES — what [`report_agent`] and
/// [`release_agent`] say instead of the raw refusal (R283).
///
/// # What the daemon actually refused, and why this is a disjunction
///
/// Both handlers answer `InvokeError::Rejected` for exactly two causes, read off their source:
/// this host installs no agent detector, or it holds no pane with that id (daemon-WIDE — the agent
/// memory is keyed by [`PaneId`](sprag_terminal::PaneId) alone, so a hook may report a pane in
/// another session).
/// `InvokeError` has no payload — not the trait's three-variant enum, not the RPC's — so **which of
/// the two it was cannot cross the wire**, and no amount of care on this side recovers it. Filed as
/// `claudedocs/PINION-PR82-*`; when it lands, this function's body is one line reading the reason
/// the daemon attached.
///
/// So the sentence names both, in the order they are worth checking, and says which of them the
/// daemon could not tell us. Naming one alone would be a guess dressed as a fact — and a guess that
/// is right today only because `sprag-term` happens to install a detector, which is a claim about
/// another file's wiring, not about this refusal.
///
/// Nothing is asked of the daemon to build it: this runs only when a call already failed, and a
/// pane-list read here would still not settle the question (the `panes` slot is scoped to one
/// window; the daemon's check is not).
fn agent_refusal(command: &str, pane: u64, fault: &RpcFault) -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        format!(
            "{command}: the daemon refused pane {pane} — either no pane {pane} exists on it \
             (check `sprag panes`), or this host runs no agent detector. All it could say was \
             {:?}",
            // The fault's own rendering, which prefers the `data` the peer attached over the
            // JSON-RPC category in `message`. That is what makes the gap VISIBLE rather than
            // described: the operator reads the exact token the wire carried, and it is a Rust
            // variant name with no pane in it.
            fault.to_string(),
        ),
    )
}

/// `report-agent STATE [-t SESSION] [--pane N] [--source S] [--name AGENT] [--seq N]`: say what the
/// agent in a pane is doing, from INSIDE that pane.
///
/// The pane defaults to `$SPRAG_PANE` — the variable the daemon publishes into every pane it births —
/// so the useful form is the short one and it takes no argument a hook would have to discover:
///
/// ```text
/// sprag report-agent working
/// sprag report-agent idle --source myhook
/// ```
///
/// That default is the whole reason a shell hook needs no JSON-RPC of its own. Outside a pane there is
/// no such variable and no default: the error says so rather than reporting about somebody else's
/// pane.
///
/// `--source` defaults to `cli` because a report must always name an authority (the daemon refuses an
/// unnamed one), and the honest name for a report typed at a command line is the command line. A hook
/// SHOULD pass its own, since a source is also the unit a replay is refused against.
///
/// Prints what the daemon did with it — `accepted`/`refused`, whether the published verdict `changed`,
/// and the published `seq` — because a report that arrived out of order is refused silently otherwise,
/// which is exactly the case a reporter needs to see.
fn report_agent(args: Vec<String>) -> io::Result<()> {
    let (session, rest) = scope_and_rest(args, "report-agent")?;
    let mut state: Option<String> = None;
    let mut pane: Option<u64> = None;
    let mut source: Option<String> = None;
    let mut name: Option<String> = None;
    let mut seq: Option<u64> = None;
    let mut it = rest.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--pane" => pane = Some(named_pane(&mut it, "report-agent")?),
            "--source" => {
                source = Some(
                    it.next()
                        .ok_or_else(|| bad_input("report-agent: --source needs a name"))?,
                );
            }
            "--name" => {
                name = Some(
                    it.next()
                        .ok_or_else(|| bad_input("report-agent: --name needs an agent name"))?,
                );
            }
            "--seq" => {
                let value = it
                    .next()
                    .ok_or_else(|| bad_input("report-agent: --seq needs a number"))?;
                seq = Some(value.parse::<u64>().map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("report-agent: --seq {value:?} is not a number"),
                    )
                })?);
            }
            other if state.is_none() && !other.starts_with('-') => state = Some(other.to_owned()),
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "report-agent: unexpected argument {other:?} (report-agent STATE \
                         [-t SESSION] [--pane N] [--source S] [--name AGENT] [--seq N])"
                    ),
                ));
            }
        }
    }
    // Checked HERE rather than left to the daemon's refusal: `working|blocked|idle` is a vocabulary a
    // person mistypes, and `unknown` is the mistake worth naming — it reads like a state and means
    // "scrape me again", which is `release-agent`.
    let state =
        state.ok_or_else(|| bad_input("report-agent: needs a state (working | blocked | idle)"))?;
    if sprag_detect::AgentState::from_wire(&state).is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            match state.as_str() {
                "unknown" => "report-agent: `unknown` is not reportable — use `release-agent` to \
                              hand the pane back to the screen"
                    .to_owned(),
                other => {
                    format!("report-agent: {other:?} is not a state (working | blocked | idle)")
                }
            },
        ));
    }
    let pane = pane.map_or_else(|| own_pane("report-agent"), Ok)?;
    let mut conn = connect_scoped(session.as_deref())?;
    let mut params = serde_json::json!({
        "id": pane,
        "state": state,
        "source": source.unwrap_or_else(|| "cli".to_owned()),
    });
    if let Some(name) = name {
        params["name"] = Value::String(name);
    }
    if let Some(seq) = seq {
        params["seq"] = Value::from(seq);
    }
    // `try_call`, so a refusal stays a REFUSAL rather than becoming a rendered sentence this side
    // would then have to match on ([`agent_refusal`]). A transport failure is passed through
    // untouched: it is not about panes, and dressing it as if it were would be the same class of
    // wrong answer this replaces.
    let answer: Value = conn
        .try_call(
            "scene/invoke",
            scoped_invoke(
                session.as_deref(),
                mux_action_path(REPORT_AGENT_ACTION),
                params,
            ),
        )
        .map_err(|error| match error {
            CallError::Fault(fault) => agent_refusal("report-agent", pane, &fault),
            CallError::Transport(error) => error,
        })?;
    let accepted = answer["accepted"].as_bool().unwrap_or(false);
    println!(
        "pane {pane}: {} {} seq={}",
        if accepted { "accepted" } else { "REFUSED" },
        if answer["changed"].as_bool().unwrap_or(false) {
            "(state changed)"
        } else {
            "(no change)"
        },
        answer["seq"].as_u64().unwrap_or(0),
    );
    if !accepted {
        eprintln!(
            "sprag: report-agent: refused as stale — a --seq at or below the last one from this \
             source is a replay"
        );
    }
    Ok(())
}

/// `release-agent [-t SESSION] [--pane N]`: hand a pane back to the screen, dropping whatever report
/// was in force.
///
/// The pane defaults to `$SPRAG_PANE`, like [`report_agent`], so an agent's exit hook is
/// `sprag release-agent` with no arguments. Answers whether a report was actually dropped, so a
/// caller can tell that from "there was nobody reporting".
///
/// A daemon also does this on its own for a pane whose CHILD has exited (`sprag_host::sweep_once`) —
/// the case a hook cannot cover, because a killed process runs no hook.
fn release_agent(args: Vec<String>) -> io::Result<()> {
    let (session, rest) = scope_and_rest(args, "release-agent")?;
    let mut pane: Option<u64> = None;
    let mut it = rest.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--pane" => pane = Some(named_pane(&mut it, "release-agent")?),
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "release-agent: unexpected argument {other:?} (release-agent \
                         [-t SESSION] [--pane N])"
                    ),
                ));
            }
        }
    }
    let pane = pane.map_or_else(|| own_pane("release-agent"), Ok)?;
    let mut conn = connect_scoped(session.as_deref())?;
    let answer: Value = conn
        .try_call(
            "scene/invoke",
            scoped_invoke(
                session.as_deref(),
                mux_action_path(RELEASE_AGENT_ACTION),
                serde_json::json!({ "id": pane }),
            ),
        )
        .map_err(|error| match error {
            CallError::Fault(fault) => agent_refusal("release-agent", pane, &fault),
            CallError::Transport(error) => error,
        })?;
    if answer["released"].as_bool().unwrap_or(false) {
        println!("pane {pane}: released — its state comes from the screen again");
    } else {
        println!("pane {pane}: nothing to release (no report was in force)");
    }
    Ok(())
}

/// `install-hooks [AGENT]…` / `uninstall-hooks [AGENT]…`: wire an agent's own configuration to
/// [`report_agent`], or take that wiring back out.
///
/// This is the one thing sprag does that writes under the user's HOME, into a file it did not
/// create, so it ASKS. The prompt shows the edit that will be applied, derived from the plan that
/// will be written rather than described beside it — a summary composed separately would be a
/// second account of what happens.
///
/// `--dry-run` prints the plan and stops. `--yes` answers the prompt. With NEITHER a terminal to
/// ask on nor `--yes`, it refuses: assuming yes would break the promise to ask, and assuming no
/// would exit 0 having silently done nothing, which is the failure mode of an installer that
/// cannot tell you it did not install.
///
/// Naming no AGENT covers every target whose agent is actually on this machine — installing into
/// an agent that is not here would create its config directory on its behalf. The ones skipped are
/// PRINTED, because a silent cap reads as "covered everything".
fn install_hooks(args: Vec<String>) -> io::Result<()> {
    edit_hooks(args, true)
}

/// `uninstall-hooks` — see [`install_hooks`]. Removes only entries sprag owns, and only from
/// targets it is asked about.
fn uninstall_hooks(args: Vec<String>) -> io::Result<()> {
    edit_hooks(args, false)
}

/// Both halves of the installer: they differ in the plan they derive and in nothing else, so the
/// asking, the refusal and the reporting have one definition.
fn edit_hooks(args: Vec<String>, install: bool) -> io::Result<()> {
    let verb = if install {
        "install-hooks"
    } else {
        "uninstall-hooks"
    };
    let mut named: Vec<&'static Target> = Vec::new();
    let mut assume_yes = false;
    let mut dry_run = false;
    for arg in args {
        match arg.as_str() {
            "-y" | "--yes" => assume_yes = true,
            "--dry-run" => dry_run = true,
            other if other.starts_with('-') => {
                return Err(bad_input(&format!(
                    "{verb}: unexpected argument {other:?} ({verb} [AGENT…] [--yes] [--dry-run])"
                )));
            }
            other => named.push(
                hooks::target(other).ok_or_else(|| HookError::UnknownTarget(other.to_owned()))?,
            ),
        }
    }

    let targets = if named.is_empty() {
        let mut present = Vec::new();
        for target in hooks::TARGETS {
            if hooks::status(target)?.present {
                present.push(target);
            } else {
                println!("{}: not on this machine, skipped", target.label);
            }
        }
        present
    } else {
        named
    };

    let mut plans = Vec::new();
    for target in targets {
        let plan = if install {
            hooks::plan_install(target, &std::env::current_exe()?)?
        } else {
            hooks::plan_uninstall(target)?
        };
        plans.push(plan);
    }
    let plans: Vec<hooks::Plan> = plans.into_iter().filter(|plan| !plan.is_empty()).collect();
    if plans.is_empty() {
        println!("nothing to do — every named agent is already as you asked");
        return Ok(());
    }

    println!(
        "sprag will change {}:",
        if plans.len() == 1 {
            "one file".to_owned()
        } else {
            format!("{} files", plans.len())
        }
    );
    for plan in &plans {
        println!("\n  {}", plan.path.display());
        for change in &plan.changes {
            println!("    {change}");
        }
    }
    if dry_run {
        println!("\n--dry-run: nothing written");
        return Ok(());
    }
    if !assume_yes && !confirm()? {
        println!("nothing written");
        return Ok(());
    }

    for plan in &plans {
        let backup = plan.apply()?;
        match backup {
            Some(backup) => println!(
                "{}: written (previous contents kept at {})",
                plan.path.display(),
                backup.display()
            ),
            None => println!("{}: created", plan.path.display()),
        }
        // An agent may hold what its config now names until its user has seen it, so writing the
        // file is not always the whole install. Printed only for a write that installs, because
        // there is nothing left to do about an entry that has just been taken back out.
        if install && let Some(follow_up) = hooks::target(plan.target).and_then(|t| t.follow_up) {
            println!("    {follow_up}");
        }
    }
    Ok(())
}

/// Ask, on the terminal, and read the answer there.
///
/// Refuses rather than deciding when there is no terminal — see [`install_hooks`] for why neither
/// default is acceptable. Anything but an explicit yes is a no.
fn confirm() -> io::Result<bool> {
    use std::io::{BufRead as _, IsTerminal as _, Write as _};
    if !io::stdin().is_terminal() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "nothing to ask on (stdin is not a terminal) — pass --yes to answer in advance, or \
             --dry-run to see the edit",
        ));
    }
    print!("\nproceed? [y/N] ");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().lock().read_line(&mut answer)?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes" | "Yes"))
}

/// `list-hooks`: one line per target — whether its agent is here, and how much of the integration
/// is in place.
fn list_hooks(args: Vec<String>) -> io::Result<()> {
    if let Some(other) = args.first() {
        return Err(bad_input(&format!(
            "list-hooks: unexpected argument {other:?} (list-hooks)"
        )));
    }
    for target in hooks::TARGETS {
        let status = hooks::status(target)?;
        // What is wired but cannot FIRE is reported before the count, because it outranks it:
        // every event can be installed and every one of them still fail. Both causes read as one
        // word here for the same reason they are one question on `Status` — to the user an install
        // that cannot run is one state, whatever put it there.
        let state = if status.inert() {
            "BROKEN".to_owned()
        } else if status.complete() {
            "installed".to_owned()
        } else if status.installed > 0 {
            format!("partly installed ({}/{})", status.installed, status.total)
        } else if status.present {
            "available".to_owned()
        } else {
            "not found".to_owned()
        };
        println!("{:<12} {state:<24} {}", target.name, status.path.display());
        if let Some(missing) = &status.missing_program {
            println!(
                "    its hooks run {} , which is not there any more — \
                 `sprag install-hooks {}` points them at this binary",
                missing.display(),
                target.name,
            );
        }
        if let Some(switch) = status.disabled_by {
            println!(
                "    `{switch}` is false in that file, so {} runs no hooks at all — \
                 turn it back on with the agent's own command, not with sprag",
                target.label,
            );
        }
        // Said for an install that is otherwise fine, because "installed" is exactly the answer
        // that would let a user stop looking while the agent is still holding the hook.
        if let Some(follow_up) = target.follow_up
            && status.installed > 0
            && !status.inert()
        {
            println!("    {follow_up}");
        }
    }
    Ok(())
}

/// `hook AGENT`: what an installed entry runs. NOT a command for a person.
///
/// The agent hands it a payload on stdin; [`sprag_host::hooks::report_for`] decides what that means
/// and this delivers it. Every failure is swallowed and the exit status is always 0, because this
/// runs inside EVERY session of that agent, including ones started in a terminal that has nothing
/// to do with sprag: a multiplexer that makes somebody's agent print errors because its own daemon
/// is down, or because they are not in a pane, is not shippable. [`report_agent`] keeps the loud
/// behaviour for the person invoking it directly.
///
/// The `seq` orders two reports from separate processes, so one that overtakes an earlier one is
/// refused rather than applied out of order. It comes from [`sprag_host::hooks::report_seq`], whose
/// own docs say why it is a boot-relative count and not the wall clock: a clock that can be stepped
/// backwards would make the second of two events refusable and park a stale state on the pane.
fn hook(args: Vec<String>) -> io::Result<()> {
    let _ = deliver_hook(args);
    Ok(())
}

/// How long [`deliver_hook`] waits for the daemon's answer before abandoning the report.
///
/// THIS RUNS IN THE AGENT'S CRITICAL PATH — an agent waits for its hooks — so a daemon that accepts
/// the connection and then wedges would stall somebody's editing session. `CONNECT_TIMEOUT` bounds
/// only the accept; without a READ deadline the call itself waits forever. Tighter than the
/// [`sprag_host::hooks::AGENT_TIMEOUT_SECS`] written into the agent's own config, so ours trips
/// first and in silence; that one is the backstop for what a client-side deadline cannot cover.
/// Generous for what it covers: a report is one local round trip on a unix socket, measured in tens
/// of microseconds.
const HOOK_DEADLINE: Duration = Duration::from_secs(2);

/// [`hook`]'s body. `None` at every step that means "nothing to say", which the caller cannot
/// distinguish from success and must not.
fn deliver_hook(args: Vec<String>) -> Option<()> {
    use std::io::Read as _;
    let target = hooks::target(args.first()?)?;
    let mut payload = String::new();
    io::stdin().read_to_string(&mut payload).ok()?;
    let outcome = hooks::report_for(target, &serde_json::from_str(&payload).ok()?)?;
    let pane = std::env::var(sprag_host::PANE_ENV_VAR)
        .ok()?
        .parse::<u64>()
        .ok()?;
    // The bound is stated to `connect`, not set after it: the handshake it performs is itself a
    // wait, and this one runs while an agent holds still for it.
    let mut conn = connect(Some(HOOK_DEADLINE)).ok()?;
    let (action, params) = match outcome {
        hooks::Outcome::Report(state) => (
            REPORT_AGENT_ACTION,
            json!({
                "id": pane,
                "state": state.wire_str()?,
                "source": format!("hook:{}", target.name),
                "name": target.agent,
                "seq": hooks::report_seq()?,
                // This report is made ON BEHALF OF the agent that spawned this process, not by it,
                // so it must not outlive that agent. `SessionEnd` covers the graceful exit; this
                // covers the two it cannot — an agent that is killed or crashes runs no hook, and
                // this hook swallows its own failures, so even a clean release can be lost.
                // `report-agent` deliberately does NOT set it: a person's report is theirs to
                // withdraw, and binding it would retire it as soon as their command returned.
                "bind": true,
            }),
        ),
        hooks::Outcome::Release => (RELEASE_AGENT_ACTION, json!({ "id": pane })),
    };
    let _: Value = conn
        .call(
            "scene/invoke",
            scoped_invoke(None, mux_action_path(action), params),
        )
        .ok()?;
    Some(())
}

/// The pane THIS process is running in, from `SPRAG_PANE` — what the daemon told the pane at birth.
///
/// The error names the variable rather than saying "pass --pane", because a caller outside a pane
/// cannot fix this by guessing an id: either they are in a sprag pane and the daemon published it, or
/// they are addressing somebody else's pane and should say which.
fn own_pane(command: &str) -> io::Result<u64> {
    let raw = std::env::var(sprag_host::PANE_ENV_VAR).map_err(|_| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "{command}: no ${} in the environment — run it inside a sprag pane, or name one \
                 with --pane",
                sprag_host::PANE_ENV_VAR
            ),
        )
    })?;
    raw.parse::<u64>().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "{command}: ${}={raw:?} is not a pane id",
                sprag_host::PANE_ENV_VAR
            ),
        )
    })
}

/// The `--pane N` value for a verb that takes one.
fn named_pane(it: &mut impl Iterator<Item = String>, command: &str) -> io::Result<u64> {
    let value = it
        .next()
        .ok_or_else(|| bad_input(&format!("{command}: --pane needs a pane id")))?;
    value.parse::<u64>().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{command}: --pane {value:?} is not a pane id"),
        )
    })
}

/// `panes [-t SESSION]`: one line per pane of the scoped session's CURRENT window — tmux
/// `list-panes`. `ID: COLSxROWS  COMMAND`, plus `name=NAME` when somebody named it, the child's own
/// window title in brackets when it has set one, and `opened by pane N` when some other pane's
/// occupant asked for this one.
///
/// The pane ID leads the line because it is what every other pane verb takes, so `sprag panes`
/// is the discovery step that makes the rest usable from a shell — `cut -d: -f1` yields exactly the
/// ids they accept. tmux prints a per-window INDEX and marks the active pane; sprag's id is
/// registry-unique, so it needs no window prefix, and the active pane IS marked — R276 gave the
/// daemon one. (The sentence that used to end here said there was none to mark, five lines above
/// the code that marks it: an inherited claim, false at HEAD.)
///
/// It answers WHO is in the window, and nothing about WHERE they sit — that is [`layout`], which
/// reads a different slot. Neither verb joins the other's, on purpose: see [`layout`].
fn panes(args: Vec<String>) -> io::Result<()> {
    let (session, rest) = scope_and_rest(args, "panes")?;
    if let Some(other) = rest.first() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("panes: unexpected argument {other:?} (only -t SESSION is accepted)"),
        ));
    }
    let mut conn = connect_scoped(session.as_deref())?;
    let listed: Value = conn.call(
        "scene/query",
        scoped_params(session.as_deref(), mux_action_path(PANES_SLOT)),
    )?;
    // The whole entry, not just the id — this is the one command whose subject is the LIST, so it
    // reads the slot directly rather than through `pane_ids`.
    for pane in listed.as_array().into_iter().flatten() {
        let id = pane["id"].as_u64().unwrap_or_default();
        let cols = pane["cols"].as_u64().unwrap_or(0);
        let rows = pane["rows"].as_u64().unwrap_or(0);
        let command = pane["command"].as_str().unwrap_or("?");
        // The name a PERSON gave the pane, absent for one nobody named. It sits ahead of the title
        // and is quoted differently on purpose: the two are the opposite kind of fact. This one is
        // IDENTITY — unique across the daemon, and what an agent addresses the pane by — where the
        // title is a display name the child rewrites on every prompt. A reader who cannot tell them
        // apart at a glance would eventually type one where the other was meant.
        // QUOTED, and that is not decoration. A name may contain spaces, so `name=my build` cannot
        // be read back by anything — and this listing IS a machine-readable contract (its leading
        // field feeds every other verb). `{:?}` is total here because a name can hold no control
        // character, so the only escapes it can produce are `\"` and `\\`.
        let name = match pane["name"].as_str() {
            Some(name) => format!("  name={name:?}"),
            None => String::new(),
        };
        // The child's live OSC 0/2 title, absent until it sets one — a DISPLAY name, never
        // identity, so it trails the command rather than replacing it.
        let title = match pane["title"].as_str() {
            Some(title) if !title.is_empty() => format!("  [{title}]"),
            _ => String::new(),
        };
        // The ACTIVE pane, marked the way tmux's own `list-panes` marks it — TRAILING, because
        // this listing's leading field is a contract (`cut -d: -f1` feeds these ids to every other
        // verb) and a marker in front of the id would break it. Exactly one row can carry it:
        // these rows are one window's panes.
        let active = if pane["active"] == json!(true) {
            "  (active)"
        } else {
            ""
        };
        // WHO ASKED for this pane, absent for one nobody claims — which is every pane a person made.
        // It is what tells an operator that a pane appeared because an agent asked for it, and which
        // agent's pane to go and read; without it an agent-opened pane is indistinguishable from one
        // the operator made and forgot.
        //
        // The opener is named by ID and carries no liveness note, deliberately: this listing is ONE
        // window's panes, so an opener sitting in another window (or another session — ids are
        // registry-unique) is absent here while being perfectly alive, and a "(gone)" derived from
        // this list would be a confident lie. Saying whether it still exists needs a second read at a
        // different scope, which is the two-instant join `layout` is a separate verb to avoid.
        let opened_by = match pane["opened_by"].as_u64() {
            Some(opener) => format!("  opened by pane {opener}"),
            None => String::new(),
        };
        println!("{id}: {cols}x{rows}  {command}{name}{title}{opened_by}{active}");
    }
    Ok(())
}

/// `layout [-t SESSION]`: WHERE the scoped session's current window puts its panes — the
/// arrangement, which pane fills the window, and which panes are out of the tiling.
///
/// [`panes`] answers WHO and this answers WHERE, which is the `panes`/`layout` slot split the
/// daemon already publishes. Until this existed the CLI exposed only the first half, so
/// `move-pane`, `swap-pane` and `zoom-pane` — the three verbs written for callers that draw
/// nothing — produced no observable change in any CLI reading: a swap leaves the pane listing
/// byte-identical, and a zoom leaves it untouched entirely.
///
/// # Why this is a second verb rather than a column on [`panes`]
///
/// Because marking the zoom on the pane listing would make one command join TWO slot reads at two
/// instants, which is exactly the torn read
/// [`LayoutSnapshot::zoomed`](sprag_terminal::LayoutSnapshot::zoomed) is a `PaneId` rather than a
/// flag to remove: the pane a window is filled by and the set of panes it holds are published in
/// separate slots, so a reader that combined them could print a pane as filling a window it had
/// already left. One verb, one slot, one revision — and the two verbs together still say more than
/// a joined listing could, because `panes` carries each pane's real `COLSxROWS` (the daemon's
/// arbitrated PTY size, already reflowed for a zoom) while this carries the shape.
///
/// Scoped like [`panes`] rather than like the window verbs: same subject, same optional `-t`.
fn layout(args: Vec<String>) -> io::Result<()> {
    let (session, rest) = scope_and_rest(args, "layout")?;
    if let Some(other) = rest.first() {
        return Err(bad_input(&format!(
            "layout: unexpected argument {other:?} (only -t SESSION is accepted)"
        )));
    }
    let mut conn = connect_scoped(session.as_deref())?;
    let answer: Value = conn.call(
        "scene/query",
        scoped_params(session.as_deref(), mux_action_path(LAYOUT_SLOT)),
    )?;
    // Through the SSOT type, never by walking the arena JSON by hand: the served shape is a flat
    // arena whose nodes name their children by index (R264), and a second reader of that encoding
    // is a second thing that can come to disagree with the daemon about it.
    let snapshot: LayoutSnapshot = serde_json::from_value(answer).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("layout: the daemon's arrangement did not parse: {error}"),
        )
    })?;
    // The drawing itself is the library's, so the agent-facing surface and this one show an
    // operator the SAME picture of one arrangement. What is spelled here is what is this verb's
    // own: the revision heading, and the naming — a pane by the host id the caller passes back to
    // `select-pane`, which is a different integer from the one an MCP tool takes.
    print!(
        "revision {}\n{}",
        snapshot.revision,
        arrangement::render(&snapshot, &|pane| format!("pane {pane}")),
    );
    Ok(())
}

/// `processes [PANE]`: WHAT EACH PANE IS RUNNING — its terminal device, the child the daemon
/// spawned, and the job that currently owns its terminal, with every process in that job.
///
/// The third of the pane verbs and the one the other two cannot answer: [`panes`] says WHO (and its
/// `command` is the label a pane was SPAWNED with, frozen at birth — a pane opened as `bash` and now
/// three hours into a `cargo build` still lists as `bash`), [`layout`] says WHERE, and this says what
/// is actually running. A shell hands its terminal to the job it starts and takes it back when that
/// job ends, so the foreground group is the OS's own answer to "what did the user set going", and
/// until this verb existed nothing outside the daemon could ask for it.
///
/// # Why it takes no `-t`
///
/// Because the daemon's answer is registry-wide by construction and pretending otherwise would cost
/// something: `/proc` carries no index by process group, so enumerating ONE pane's job is the same
/// full pass that answers every other pane. Narrowing here would mean either a second slot read to
/// learn which ids are in scope, or two scopes each paying the same walk. A row names its pane by
/// the id every other verb takes, so `sprag processes 7` narrows client-side without either cost —
/// which is also strictly more than a rival that can ask about one pane at a time.
///
/// # The rendering, and the one thing it must not do
///
/// A pane's block is `ID: DEVICE  child PID` and then one line per process of its job. A process
/// line is `PID NAME  ARGV`, and **the argv is quoted per argument** ([`shell_quote`]): the wire
/// deliberately carries the argument VECTOR and never a pre-joined string, because joining with
/// spaces makes an argument containing a space indistinguishable from two arguments — so the one
/// place that has to render it flat must not undo that. `sleep '4 00'` and `sleep 4 00` are
/// different commands and this prints them differently.
///
/// The reading's AGE heads the output, because a sampled fact read without it is one whose freshness
/// the reader has to guess at: a job list that predates the build somebody just started looks
/// identical to one that does not. Tolerance zero, so a one-shot human command waits for its own
/// fresh walk rather than printing something held for a display poll.
fn processes(args: Vec<String>) -> io::Result<()> {
    let mut wanted: Option<u64> = None;
    for arg in args {
        if wanted.is_some() {
            return Err(bad_input(&format!(
                "processes: unexpected argument {arg:?} (processes [PANE])"
            )));
        }
        wanted = Some(
            arg.parse::<u64>()
                .map_err(|_| bad_input(&format!("processes: {arg:?} is not a pane id")))?,
        );
    }
    let mut conn = connect(None)?;
    let reading = query_slot(
        &mut conn,
        &mux_action_path(&pane_processes_at(0)),
        "processes",
    )?;
    let wire: PaneProcessesWire = serde_json::from_value(reading).map_err(|error| {
        bad_input(&format!(
            "processes: the host's answer did not parse: {error}"
        ))
    })?;
    let rows: Vec<_> = wire
        .panes
        .iter()
        .filter(|row| wanted.is_none_or(|id| row.id == id))
        .collect();
    if let Some(id) = wanted
        && rows.is_empty()
    {
        // The caller ASKED about that pane, so silence would be the wrong answer — the same rule
        // `require_pane` follows for every verb that takes a target.
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "processes: no pane {id} (panes: {:?})",
                wire.panes.iter().map(|row| row.id).collect::<Vec<_>>()
            ),
        ));
    }
    println!("sampled {} ms ago", wire.sampled_ms_ago);
    for row in rows {
        let device = row.tty.as_deref().unwrap_or("(no device)");
        // A pane whose child has been reaped keeps its place and its final screen, so it is listed
        // rather than dropped — and it says it has no child instead of printing a pid it lost.
        let child = row
            .shell_pid
            .map_or_else(|| "no child".to_owned(), |pid| format!("child {pid}"));
        println!("{}: {device}  {child}", row.id);
        let Some(job) = &row.foreground else {
            // Distinct from "no child": a live child whose terminal nobody owns is a real state
            // (nothing has called `tcsetpgrp`), and calling it the same thing would hide it.
            println!("     (no job owns the terminal)");
            continue;
        };
        for process in &job.processes {
            let argv = process
                .argv
                .iter()
                .map(|arg| shell_quote(arg))
                .collect::<Vec<_>>()
                .join(" ");
            // An empty argv is a FACT the wire keeps (a zombie, a kernel thread), so it is said
            // rather than rendered as a blank column.
            let argv = if argv.is_empty() {
                "(no command line)".to_owned()
            } else {
                argv
            };
            println!("     {} {}  {argv}", process.pid, process.name);
        }
    }
    Ok(())
}

/// `agent [-t SESSION] [PANE]`: what the AI agent in each pane is doing — H3's verdict, from the
/// same pane list every other surface reads.
///
/// `ID: STATE  NAME  rule=RULE  seq=N`, one line per pane that an agent manifest CLAIMS. A pane
/// running a shell prints nothing, which is the pane list's own additive rule at this surface: the
/// `agent` key is absent for a pane with no known state, so a workspace of shells prints nothing at
/// all rather than a column of "none". `sprag panes` is the verb that lists every pane.
///
/// Naming a PANE turns the same reading into a DIAGNOSIS, and that is why one verb does both. The
/// state alone answers "is it waiting for me"; when the answer is WRONG, the only useful next fact is
/// which rule produced it (H3's D7 — a gate that cannot say what it saw cannot be debugged), and what
/// to do about it. So a named pane also prints the rule's remedy: the id is what an `[[agent]]` block
/// in `config.toml` redefines or disables, which is what makes a mis-detected pane fixable without a
/// release. A named pane with no agent says so explicitly rather than printing nothing — the caller
/// asked about that pane, which is [`require_pane`]'s rule, and "no manifest claims this" is a real
/// answer that is NOT the same as "idle" (D3: those are opposite instructions to a person).
///
/// The rule is never re-derived here. It is the value the daemon's detector produced, carried on the
/// wire, so this verb cannot come to disagree with the state it explains.
///
/// # Why it reports a broken `config.toml` before it reports anything else
///
/// Because the remedy above is otherwise a trap. This verb tells a user with a wrong verdict to edit
/// an `[[agent]]` block, and a daemon whose manifest file will not parse keeps the last list that
/// WORKED and says so nowhere — so the user edits, sees nothing change, and the surface that sent
/// them there stays silent. Worse for a pane the file was supposed to claim: `no agent  (no manifest
/// claims this pane)` is what an unparsed claim looks like from here, which reads as a detection
/// problem rather than a syntax one.
///
/// So the report is asked for on every `agent` call, not only a diagnosing one, and printed on
/// stderr because it qualifies the answer rather than being part of it.
fn agent(args: Vec<String>) -> io::Result<()> {
    let (session, rest) = scope_and_rest(args, "agent")?;
    let mut wanted: Option<u64> = None;
    for arg in rest {
        if wanted.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("agent: unexpected argument {arg:?} (agent [-t SESSION] [PANE])"),
            ));
        }
        wanted = Some(arg.parse::<u64>().map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("agent: {arg:?} is neither -t nor a pane id"),
            )
        })?);
    }
    let mut conn = connect_scoped(session.as_deref())?;
    if let Some(pane) = wanted {
        require_pane(&mut conn, session.as_deref(), pane, "agent")?;
    }
    // FIRST, and on stderr. Every line below was produced by a ruleset that is not the user's, so
    // the caveat has to arrive before the readings it qualifies — and a script slicing `ID: STATE`
    // out of stdout must not have to skip a sentence to find them.
    let manifests: Value = conn.call(
        "scene/query",
        scoped_params(session.as_deref(), mux_action_path(AGENT_MANIFESTS_SLOT)),
    )?;
    if let Some(error) = manifests["error"].as_str() {
        eprintln!("sprag: agent: {error}");
        eprintln!(
            "sprag: agent: the states below came from the manifests that last worked, not from \
             that file"
        );
    }
    let listed: Value = conn.call(
        "scene/query",
        scoped_params(session.as_deref(), mux_action_path(PANES_SLOT)),
    )?;
    for entry in listed.as_array().into_iter().flatten() {
        let id = entry["id"].as_u64().unwrap_or_default();
        if wanted.is_some_and(|pane| pane != id) {
            continue;
        }
        let agent = &entry["agent"];
        // ADDITIVE: no key means no manifest claims the pane. Silent in the LIST (a shell is not an
        // answer anybody asked for) and stated for a pane the caller named.
        let Some(state) = agent["state"].as_str() else {
            if wanted.is_some() {
                println!("{id}: no agent  (no manifest claims this pane — not the same as idle)");
            }
            continue;
        };
        let name = agent["name"].as_str().unwrap_or("(unidentified)");
        let seq = agent["seq"].as_u64().unwrap_or(0);
        // The two kinds of evidence are ADDITIVE on the wire and mutually exclusive in fact: a
        // reported verdict carries a `source` and no `rule`, a scraped one the reverse. Printing
        // whichever is there is what lets a reader tell an authority from an inference — the
        // distinction R271 put on the wire and nothing surfaced.
        let source = agent["source"].as_str();
        let rule = agent["rule"].as_str();
        let origin = source
            .map(|source| format!("source={source}"))
            .unwrap_or_else(|| format!("rule={}", rule.unwrap_or("(none)")));
        println!("{id}: {state}  {name}  {origin}  seq={seq}");
        if wanted.is_some() {
            // The advice has to follow the evidence. Telling somebody to redefine a manifest rule
            // for a verdict a HOOK reported names a rule that never fired, and the edit would do
            // nothing at all.
            match source {
                Some(source) => println!(
                    "    `{source}` reported this, and a report outranks the screen. \
                     `sprag release-agent --pane {id}` hands the pane back to screen inference."
                ),
                None => println!(
                    "    `{}` is the rule that fired. If this verdict is wrong, redefine or \
                     disable that id in an [[agent]] block in config.toml — the daemon picks the \
                     edit up on its own.",
                    rule.unwrap_or("(none)"),
                ),
            }
        }
    }
    Ok(())
}

/// `events [-t SESSION] [--since N] [-f]`: what has CHANGED in the scoped session, one line each.
///
/// The half of a wake that a bare revision number could never carry. A client woken by
/// `scene/waitFor` learns only that something moved and re-reads everything to find out what; this
/// asks the daemon, which already knew.
///
/// ## Why `-f` is the verb and the rest is scaffolding
///
/// Without it this is a poll, and a poll is what the whole feature exists to remove. With it the
/// verb BLOCKS until something happens and then says what — which is the primitive an agent
/// orchestrating other agents actually needs, and the one thing sprag's tooling could not express
/// at all. `sprag events -f | while read -r line; do …` is the shape it buys.
///
/// It is built from two calls that already existed and one that did not: park on `scene/waitFor`,
/// read `events.<since>`, repeat. No new blocking method — the cursor IS the revision the wait
/// answers with, so the pair composes (see `sprag_host::events`).
///
/// ## The long-poll connection carries NO read deadline, deliberately
///
/// Every other verb here is a request-response against a local daemon answering from memory, so a
/// reply that has not arrived in seconds is not slow, it is not coming. A parked `waitFor` is the
/// opposite: waiting indefinitely is its contract. The GUI's poll thread makes the same distinction
/// on its own dedicated connection, and this follows it rather than inventing a second policy.
///
/// ## `lost` is REPORTED, never swallowed
///
/// A reader that falls behind the daemon's ring is told, on stderr so a script slicing stdout is
/// unaffected. The honest response is to re-read the world (`sprag panes`, `sprag windows`), and
/// saying so is the difference between a gap the caller can act on and one it cannot see.
fn events(args: Vec<String>) -> io::Result<()> {
    let bad = |message: String| io::Error::new(io::ErrorKind::InvalidInput, message);
    let (session, rest) = scope_and_rest(args, "events")?;
    let mut since: Option<u64> = None;
    let mut follow = false;
    let mut pane: Option<u64> = None;
    let mut kinds: Vec<String> = Vec::new();
    let usage = "events [-t SESSION] [--since N] [-f [--pane ID] [--kind KIND]…]";
    let mut it = rest.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-f" | "--follow" => follow = true,
            "--since" => {
                let value = it
                    .next()
                    .ok_or_else(|| bad("events: --since needs a revision".to_owned()))?;
                since =
                    Some(value.parse::<u64>().map_err(|_| {
                        bad(format!("events: --since {value:?} is not a revision"))
                    })?);
            }
            "--pane" => {
                let value = it
                    .next()
                    .ok_or_else(|| bad("events: --pane needs a pane id".to_owned()))?;
                pane = Some(
                    value
                        .parse::<u64>()
                        .map_err(|_| bad(format!("events: --pane {value:?} is not a pane id")))?,
                );
            }
            "--kind" => {
                kinds.push(
                    it.next()
                        .ok_or_else(|| bad("events: --kind needs a change name".to_owned()))?,
                );
            }
            other => {
                return Err(bad(format!(
                    "events: unexpected argument {other:?} ({usage})"
                )));
            }
        }
    }
    // A filter is a property of a WAIT, and refusing it without one is the honest consequence of
    // having exactly ONE matcher: the daemon's. Filtering a non-blocking read would mean a second
    // implementation of "does this event match", client-side, free to disagree with the first — the
    // shape this round removed from the host's own serializer. `-f --since 0 --pane N` still prints
    // that pane's backlog, because a wait whose cursor already has a match answers at once.
    if !follow && (pane.is_some() || !kinds.is_empty()) {
        return Err(bad(format!(
            "events: --pane / --kind narrow what to WAIT for, so they need -f ({usage})"
        )));
    }
    let filter = sprag_host::events::EventFilter::narrowing_wire(pane, &kinds);

    let mut conn = connect_scoped(session.as_deref())?;
    // A cursor the caller did not give is NOW, not zero: `events -f` means "tell me what happens",
    // and replaying a daemon's whole history first would bury that under a backlog nobody asked
    // for. `--since 0` is how a caller asks for the backlog, and it is the only way to get it.
    let mut cursor = match since {
        Some(cursor) => cursor,
        None => {
            let answer: Value = conn.call("scene/revision", scoped_only(session.as_deref()))?;
            answer["revision"].as_u64().unwrap_or(0)
        }
    };

    // The backlog, read WITHOUT blocking — the only thing the slot can do that the wait cannot, and
    // the reason the slot stays. Skipped when a filter is in force: the wait below answers a cursor
    // that already has a match immediately, so the backlog arrives through the one matcher instead of
    // through a second one here.
    if filter.is_none() {
        let batch: Value = conn.call(
            "scene/query",
            scoped_params(
                session.as_deref(),
                mux_action_path(&events_slot_since(cursor)),
            ),
        )?;
        cursor = print_events(&batch, cursor);
    }
    if !follow {
        return Ok(());
    }

    // SUBSCRIBE, then read. One request for the whole follow, where this loop paid a round trip per
    // change until R298 — see [`EVENTS_SUBSCRIBE_METHOD`]. It parks on the JOURNAL for the same
    // reason the wait did: `scene/waitFor` is released by pane OUTPUT, which records nothing, so a
    // follow built on it spun at socket speed against any pane running a build (measured: 22 431
    // returns a second, every one empty).
    //
    // No deadline: waiting is this call's contract, not a hazard (see the doc above).
    conn.set_read_deadline(None)?;
    let mut params = scoped_only(session.as_deref());
    params[SINCE_PARAM] = json!(cursor);
    if let Some(filter) = &filter {
        params[sprag_host::events::EventFilter::WIRE_KEY] = filter.clone();
    }
    match conn.try_call(EVENTS_SUBSCRIBE_METHOD, params) {
        Ok(_) => {}
        // EVERY fault, not only `Invalid params`, and reading the skew run's output is what widened
        // this arm: a daemon too old to speak this method answers method-not-found, and the narrow
        // form rendered that as `host rpc error: sprag-term host: 'events/subscribe' is
        // unsupported; use one of: …` — the daemon's own perfectly good sentence behind a
        // transport's phrase for a fault nobody could anticipate. Every refusal this call can produce
        // is one the caller can act on (a filter it mistyped, a daemon it should restart), so every
        // one is rendered as the daemon wrote it. Debt item 20's class, and R296's fix shape on the
        // verb beside this one.
        Err(CallError::Fault(fault)) => {
            return Err(bad(format!("events: {}", fault_sentence(&fault))));
        }
        Err(other) => return Err(other.into()),
    }
    // The subscription's own `next` is deliberately NOT adopted here: it echoes the `since` just
    // sent, and this loop already holds that. Reading it back would be a second author of a number
    // it has, which is how the FIRST version of this loop acquired a real bug — it adopted
    // `scene/waitFor`'s revision as the cursor and so skipped whatever was recorded AT that
    // revision, unrecoverably, since the read that follows is `> cursor`. Every cursor here now
    // comes from a BATCH, and a batch's `next` is the last record it actually carried.
    loop {
        let batch = conn.next_notification(EVENTS_CHANGED_METHOD)?;
        cursor = print_events(&batch, cursor);
    }
}

/// The operator-facing half of a fault: its `data` sentence when it has one, else its `message`.
///
/// The same precedence [`RpcFault`]'s own `Display` uses, read off the STRUCTURE rather than by
/// re-parsing a rendered line — [`unknown_slot`]'s rule, for the same reason.
fn fault_sentence(fault: &RpcFault) -> String {
    fault
        .data
        .as_ref()
        .and_then(Value::as_str)
        .unwrap_or(&fault.message)
        .to_owned()
}

/// Print one change batch as `TYPE<TAB>SUBJECT` lines and answer the cursor to resume from.
///
/// Shared by the backlog read and the wait, because they answer the same shape
/// ([`sprag_host::events::Batch::to_wire`]) — printing it twice would be two formats for one fact.
///
/// **`lost` is REPORTED, never swallowed**, on stderr so a script slicing stdout is unaffected. The
/// honest response is to re-read the world, and saying so is the difference between a gap the caller
/// can act on and one it cannot see.
fn print_events(batch: &Value, cursor: u64) -> u64 {
    if batch["lost"].as_bool().unwrap_or(false) {
        eprintln!(
            "sprag: events: fell behind the daemon's log — some changes were dropped before this \
             read. Re-read the world (`sprag panes`, `sprag windows`); what follows is only what \
             survived."
        );
    }
    for event in batch["events"].as_array().into_iter().flatten() {
        let kind = event["type"].as_str().unwrap_or("?");
        // The subject key is named for WHAT it is, so a reader that has matched the type already
        // knows which slot to re-read. Printed as `TYPE<TAB>SUBJECT` — the shape `sprag run`'s
        // listing uses, which a script can cut.
        let subject = ["pane", "window", "session"]
            .iter()
            .find_map(|key| match &event[*key] {
                Value::String(name) => Some(name.clone()),
                Value::Number(id) => Some(id.to_string()),
                _ => None,
            })
            .unwrap_or_default();
        println!("{kind}\t{subject}");
    }
    batch["next"].as_u64().unwrap_or(cursor)
}

/// `split-window [-t SESSION] [-h|-v [-b] PANE] [-- command…]`: add a pane to the scoped session's
/// current window and print its id — tmux `split-window`.
///
/// `--` introduces the argv the pane runs; absent, it is born with `$SHELL`, exactly as tmux's
/// bare `split-window`. The id is printed on stdout because it is the argument every other pane
/// verb takes, so a script can capture it (`pane=$(sprag split-window -v 3)`).
///
/// `-h` / `-v` take the pane POSITIONALLY — the convention `kill-pane [PANE]` and
/// `resize-pane [PANE]` set — and OMITTING it is tmux's bare form: divide the pane the session is
/// on. That form was refused until the daemon held an active pane to mean "here"
/// ([`sprag_host::wire::SELECT_PANE_ACTION`]); this doc used to record the refusal as forced.
/// Naming a pane with no direction is still refused with the reason: an axis is what turns a
/// target into a placement, and the direction-less append is what naming NEITHER already asks for.
///
/// `-b` puts the new pane BEFORE its target (left of, or above) — tmux `-b`.
fn split_window(args: Vec<String>) -> io::Result<()> {
    let bad = |message: String| io::Error::new(io::ErrorKind::InvalidInput, message);
    let (session, rest) = scope_and_rest(args, "split-window")?;
    let mut command: Option<Vec<String>> = None;
    let mut dir: Option<&'static str> = None;
    let mut before = false;
    let mut pane: Option<u64> = None;
    let mut it = rest.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--" => command = Some(it.by_ref().collect()),
            "-h" | "-v" => {
                if dir.is_some() {
                    return Err(bad(
                        "split-window: -h and -v name one axis; give only one".to_owned()
                    ));
                }
                dir = Some(if arg == "-h" {
                    "horizontal"
                } else {
                    "vertical"
                });
            }
            "-b" => before = true,
            other => {
                // Anything left is the pane to divide. Parsed here rather than by position so the
                // flags may come in any order, and refused as a NUMBER error when it is not one —
                // which is what a mistyped flag looks like from here.
                if pane.is_some() {
                    return Err(bad(format!(
                        "split-window: unexpected argument {other:?} (a command goes after `--`)"
                    )));
                }
                pane = Some(other.parse::<u64>().map_err(|_| {
                    bad(format!(
                        "split-window: {other:?} is neither a flag nor a pane id"
                    ))
                })?);
            }
        }
    }
    // A pane with no direction has nothing to ask for. A DIRECTION with no pane is tmux's bare
    // `-h` / `-v` — "divide where I am" — which was refused with "sprag has no current pane" until
    // the daemon gained an active pane to mean it (`SELECT_PANE_ACTION`), and is now the same
    // request with the target left to the daemon.
    let placement = match (dir, pane) {
        (Some(dir), pane) => Some((dir, pane)),
        (None, None) => None,
        (None, Some(pane)) => {
            return Err(bad(format!(
                "split-window: pane {pane} needs an axis to be divided on — -h (right) or -v \
                 (below); omit both to append instead"
            )));
        }
    };
    if before && placement.is_none() {
        return Err(bad(
            "split-window: -b names which side of a target, so it needs -h or -v".to_owned(),
        ));
    }
    let mut action_args = match &command {
        Some(command) if command.is_empty() => {
            return Err(bad("split-window: `--` needs a command".to_owned()));
        }
        Some(command) => json!({ "cmd": command }),
        None => json!({}),
    };
    // A directional split is a DIFFERENT action from an append, not the same one with a flag: the
    // daemon divides a pane the caller names, and refuses when it cannot reach it.
    let action = match placement {
        Some((dir, pane)) => {
            let map = action_args.as_object_mut().expect("json! built an object");
            // Absent `pane` is the action's own "the active pane" default, so the bare form sends
            // no target rather than the CLI resolving one — the daemon holds the fact, and a
            // client that read it back to send it would be racing whoever moved it.
            if let Some(pane) = pane {
                map.insert("pane".to_owned(), json!(pane));
            }
            map.insert("dir".to_owned(), json!(dir));
            if before {
                map.insert("before".to_owned(), json!(true));
            }
            SPLIT_ACTION
        }
        None => SPAWN_ACTION,
    };
    let mut conn = connect_scoped(session.as_deref())?;
    let answer = conn.call(
        "scene/invoke",
        scoped_invoke(session.as_deref(), mux_action_path(action), action_args),
    );
    match answer {
        Ok(answer) => match answer.as_u64() {
            Some(id) => {
                println!("{id}");
                Ok(())
            }
            None => Err(io::Error::other(
                "split-window did not answer with a pane id",
            )),
        },
        // A well-formed request meets one refusal per action: a spawn's is the OS declining the
        // fork/exec, and a split's is additionally an unreachable target — which is the likelier
        // of the two to be the caller's own mistake, so it is named first.
        Err(error) if error.kind() == io::ErrorKind::Other => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            match (placement, &command) {
                (Some((_, Some(pane))), _) => format!(
                    "split-window: pane {pane} is not in the window's tiling (it exited, it is \
                     floating, or it belongs to another window), or the pane's command could not \
                     be run"
                ),
                // The bare form named no pane, so the refusal is about the window rather than
                // about a target the caller chose.
                (Some((_, None)), _) => "split-window: this session's current window holds no \
                     pane to divide, or the pane's command could not be run"
                    .to_owned(),
                (None, Some(command)) => {
                    format!("split-window: the pane's command could not be run: {command:?}")
                }
                (None, None) => {
                    "split-window: the pane's shell could not be run (check $SHELL)".to_owned()
                }
            },
        )),
        Err(error) => Err(error),
    }
}

/// `kill-pane [-t SESSION] PANE`: close the pane with id PANE — tmux `kill-pane`.
///
/// Closing the LAST live pane drains the daemon, so the reply can be cut short by its exit; that is
/// success, the same `server_gone` reading `kill-session` and `kill-window` make.
fn kill_pane(args: Vec<String>) -> io::Result<()> {
    let (session, rest) = scope_and_rest(args, "kill-pane")?;
    let mut rest = rest.into_iter();
    // Absent ⇒ the active pane, which the DAEMON resolves (`CLOSE_ACTION`): a CLI that read it
    // back to send it would be racing whoever moved it between the two calls.
    let pane = rest
        .next()
        .map(|arg| parse_pane_id(Some(arg), "kill-pane"))
        .transpose()?;
    if let Some(other) = rest.next() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("kill-pane: unexpected argument {other:?}"),
        ));
    }
    let mut conn = connect_scoped(session.as_deref())?;
    let answer = conn.call(
        "scene/invoke",
        scoped_invoke(
            session.as_deref(),
            mux_action_path(CLOSE_ACTION),
            pane.map_or_else(|| json!({}), |pane| json!({ "id": pane })),
        ),
    );
    let named = pane.map_or_else(
        || "the active pane".to_owned(),
        |pane| format!("pane {pane}"),
    );
    match answer {
        Ok(_) => {
            println!("killed {named}");
            Ok(())
        }
        Err(error) if server_gone(&error) => {
            println!("killed {named} (server ended)");
            Ok(())
        }
        // The session was pre-flighted, so the only refusal left is an unknown pane.
        Err(error) if error.kind() == io::ErrorKind::Other => Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "kill-pane: no such pane ({named}) in {}",
                scope_name(session.as_deref())
            ),
        )),
        Err(error) => Err(error),
    }
}

/// `resize-pane [-t SESSION] PANE -x COLS -y ROWS`: resize a pane's PTY and emulator — tmux
/// `resize-pane -x -y`.
///
/// BOTH dimensions are required, because the wire action takes both and a terminal has no notion of
/// "the other one, unchanged" that this CLI could honestly supply: reading the pane's current size
/// and sending it back would race any client resizing the same pane. tmux's relative forms
/// (`-U`/`-D`/`-L`/`-R`) are absent for the same reason `split-window -h` is — they move a DIVIDER,
/// which is layout the daemon does not model as an op.
///
/// No `cell_width`/`cell_height` is sent: those carry a display's font metric so the PTY's pixel
/// winsize and XTWINOPS reports are truthful, and a shell has none. Omitting them leaves the pane's
/// last-known cell geometry untouched, which is the honest answer rather than a zeroed guess.
fn resize_pane(args: Vec<String>) -> io::Result<()> {
    let bad = |message: String| io::Error::new(io::ErrorKind::InvalidInput, message);
    let (session, rest) = scope_and_rest(args, "resize-pane")?;
    let mut pane: Option<u64> = None;
    let mut cols: Option<u64> = None;
    let mut rows: Option<u64> = None;
    let mut it = rest.into_iter();
    while let Some(arg) = it.next() {
        let mut dimension = |name: &str, flag: &str| -> io::Result<u64> {
            let value = it
                .next()
                .ok_or_else(|| bad(format!("resize-pane: {flag} needs a {name} count")))?;
            match value.parse::<u64>() {
                Ok(0) | Err(_) => Err(bad(format!(
                    "resize-pane: {flag} {value:?} is not a positive {name} count"
                ))),
                Ok(count) => Ok(count),
            }
        };
        match arg.as_str() {
            "-x" | "--width" => cols = Some(dimension("column", "-x")?),
            "-y" | "--height" => rows = Some(dimension("row", "-y")?),
            _ if pane.is_none() => pane = Some(parse_pane_id(Some(arg), "resize-pane")?),
            other => return Err(bad(format!("resize-pane: unexpected argument {other:?}"))),
        }
    }
    // No pane id ⇒ the active one, resolved by the daemon exactly as `kill-pane`'s is.
    let (Some(cols), Some(rows)) = (cols, rows) else {
        return Err(bad(
            "resize-pane needs both dimensions (-x COLS -y ROWS)".to_owned()
        ));
    };
    let mut conn = connect_scoped(session.as_deref())?;
    conn.call(
        "scene/invoke",
        scoped_invoke(
            session.as_deref(),
            mux_action_path(RESIZE_ACTION),
            pane.map_or_else(
                || json!({ "cols": cols, "rows": rows }),
                |pane| json!({ "id": pane, "cols": cols, "rows": rows }),
            ),
        ),
    )
    .map(|_: Value| ())
    // The session was pre-flighted, so a refusal is an unknown pane or a winsize the kernel
    // declined — reported together because the wire does not distinguish them.
    .map_err(|error| {
        if error.kind() == io::ErrorKind::Other {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "resize-pane: no such pane ({}) in {}, or {cols}x{rows} was refused",
                    pane.map_or_else(
                        || "the active pane".to_owned(),
                        |pane| format!("pane {pane}")
                    ),
                    scope_name(session.as_deref())
                ),
            )
        } else {
            error
        }
    })?;
    println!(
        "resized {} to {cols}x{rows}",
        pane.map_or_else(
            || "the active pane".to_owned(),
            |pane| format!("pane {pane}")
        ),
    );
    Ok(())
}

/// `send-keys [-t SESSION] PANE [-l] [--] KEY…`: deliver keystrokes to a pane — tmux `send-keys`.
///
/// Two languages, chosen by `-l`, exactly as tmux does. By default each argument is a KEY: a W3C
/// `KeyboardEvent.key` name (`Enter`, `Escape`, `Tab`, `ArrowUp`, `F5`) or a single character, and
/// tmux's `C-` / `M-` / `S-` prefixes apply modifiers ([`parse_key_token`]) — the host encodes it
/// against the pane's LIVE input modes (DECCKM, the Kitty protocol, newline mode), which is why the
/// encoding cannot live here. With `-l` each argument is LITERAL UTF-8, written as typed with no
/// key lookup and no Enter appended.
///
/// A key name the encoder does not recognise is a clean error naming the vocabulary, because the
/// host rejects it rather than injecting nothing: sending a keystroke that silently vanished is the
/// one outcome a script cannot detect.
///
/// Literal text is sent as `text`, not `paste`: it stands for typing, and only a paste is wrapped
/// in bracketed-paste markers. `sprag run` remains the command that PASTES, and it says so.
fn send_keys(args: Vec<String>) -> io::Result<()> {
    let bad = |message: String| io::Error::new(io::ErrorKind::InvalidInput, message);
    let (session, rest) = scope_and_rest(args, "send-keys")?;
    let mut pane: Option<u64> = None;
    let mut literal = false;
    let mut tokens: Vec<String> = Vec::new();
    let mut it = rest.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-l" | "--literal" => literal = true,
            // Everything after `--` is payload, so a literal `-l` or a key named `-t` can be sent.
            "--" => tokens.extend(it.by_ref()),
            _ if pane.is_none() => pane = Some(parse_pane_id(Some(arg), "send-keys")?),
            _ => tokens.push(arg),
        }
    }
    let pane = pane.ok_or_else(|| bad("send-keys needs a pane id".to_owned()))?;
    if tokens.is_empty() {
        return Err(bad(format!(
            "send-keys needs at least one {}",
            if literal { "string" } else { "key name" }
        )));
    }
    let mut conn = connect_scoped(session.as_deref())?;
    // This verb addresses the PANE's own external, so a wrong id is an unknown ADDRESS rather than
    // a pane-level refusal — pre-flight it so the error is about panes.
    require_pane(&mut conn, session.as_deref(), pane, "send-keys")?;
    for token in &tokens {
        let (path, action_args) = if literal {
            (TEXT_ACTION, json!({ "text": token }))
        } else {
            let (key, mods) = parse_key_token(token)?;
            let (ctrl, alt, shift) = mods;
            (
                KEY_ACTION,
                json!({ "key": key, "ctrl": ctrl, "alt": alt, "shift": shift }),
            )
        };
        conn.call(
            "scene/invoke",
            scoped_invoke(session.as_deref(), pane_input_path(pane, path), action_args),
        )
        .map(|_: Value| ())
        .map_err(|error| {
            if error.kind() == io::ErrorKind::Other {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    if literal {
                        // The pane was pre-flighted and literal text needs no encoding, so the
                        // only refusal left is the PTY itself declining the write (a child that
                        // died between the check and this send).
                        format!("send-keys: pane {pane}'s PTY refused the text {token:?}")
                    } else {
                        format!(
                            "send-keys: pane {pane} refused {token:?} — a key is a W3C key name \
                             (Enter, Escape, Tab, ArrowUp, F5) or a single character, optionally \
                             prefixed C- / M- / S-"
                        )
                    },
                )
            } else {
                error
            }
        })?;
    }
    println!(
        "sent {} {} to pane {pane}",
        tokens.len(),
        if literal { "string(s)" } else { "key(s)" }
    );
    Ok(())
}

/// Split a `send-keys` token into its W3C key name and `(ctrl, alt, shift)` modifiers, reading
/// tmux's `C-` / `M-` / `S-` prefixes.
///
/// The prefixes are unambiguous rather than a heuristic: no W3C key name contains a hyphen, so a
/// leading `C-` can only be the modifier. They stack (`C-M-x`), and the remainder is passed through
/// UNTRANSLATED — this maps tmux's modifier spelling onto the wire's, and deliberately does not
/// invent a tmux→W3C name table (`Up` → `ArrowUp` and its ~40 siblings), because a half-right
/// table would turn a clean "unknown key" refusal into a key the caller did not ask for.
///
/// A token that is nothing but prefixes (`C-`) names no key, and is refused here rather than sent
/// as an empty key the host would reject with less to say.
fn parse_key_token(token: &str) -> io::Result<(String, (bool, bool, bool))> {
    let (mut ctrl, mut alt, mut shift) = (false, false, false);
    let mut rest = token;
    loop {
        let flag = match rest.get(..2) {
            Some("C-") => &mut ctrl,
            Some("M-") => &mut alt,
            Some("S-") => &mut shift,
            _ => break,
        };
        *flag = true;
        rest = &rest[2..];
    }
    if rest.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("send-keys: {token:?} names no key (only modifier prefixes)"),
        ));
    }
    Ok((rest.to_owned(), (ctrl, alt, shift)))
}

/// `capture-pane [-t SESSION] PANE [-p]`: print a pane's retained output — its scrollback and its
/// visible screen — to stdout. tmux `capture-pane -p`.
///
/// **stdout is the only destination, and `-p` is accepted as saying so.** tmux writes to a paste
/// BUFFER unless `-p` is given, and sprag has no buffers (tmux's `set-buffer` / `paste-buffer` /
/// `list-buffers` family is unbuilt), so there is nowhere else the output could go. Accepting the
/// flag costs a tmux user nothing and claims nothing false — what would be false is accepting the
/// buffer-naming `-b`, which is therefore not accepted.
///
/// The text is the host's [`FULL_TEXT_SLOT`], the same read the `read_pane` MCP tool makes, so an
/// agent and a shell see one definition of what a pane's output IS rather than two.
fn capture_pane(args: Vec<String>) -> io::Result<()> {
    let bad = |message: String| io::Error::new(io::ErrorKind::InvalidInput, message);
    let (session, rest) = scope_and_rest(args, "capture-pane")?;
    let mut pane: Option<u64> = None;
    for arg in rest {
        match arg.as_str() {
            // tmux's "print to stdout", which is the only thing this can do; see the doc above.
            "-p" | "--print" => {}
            _ if pane.is_none() => pane = Some(parse_pane_id(Some(arg), "capture-pane")?),
            other => return Err(bad(format!("capture-pane: unexpected argument {other:?}"))),
        }
    }
    let pane = pane.ok_or_else(|| bad("capture-pane needs a pane id".to_owned()))?;
    let mut conn = connect_scoped(session.as_deref())?;
    // Pre-flighted like `send-keys` and for the same reason: this addresses the pane's OWN
    // external, so a wrong id would surface as an unknown address rather than as "no such pane".
    // Printing nothing and exiting 0 would be worse still — it would claim the pane exists and had
    // said nothing.
    require_pane(&mut conn, session.as_deref(), pane, "capture-pane")?;
    let answer: Value = conn.call(
        "scene/query",
        scoped_params(session.as_deref(), pane_input_path(pane, FULL_TEXT_SLOT)),
    )?;
    let text = answer.as_str().unwrap_or_default();
    print!("{text}");
    if !text.is_empty() && !text.ends_with('\n') {
        println!();
    }
    Ok(())
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
    let mut conn = connect(None)?;
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
    let mut conn = connect(None)?;
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
    let mut conn = connect(None)?;
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

/// `select-pane [-t SESSION] [PANE | -L|-R|-U|-D]`: make a pane active — tmux `select-pane`.
///
/// A pane id and a direction name the same thing two ways, so exactly one is given. A direction
/// with no neighbour is not an error: it prints where the caller still is, because walking into the
/// edge of a layout is what a keybinding does at the edge, not a mistake it should fail on.
///
/// The four outcomes get four sentences, from the daemon's own
/// [`outcome`](sprag_host::wire::SelectHow) word. Before it there were two, and one of them was
/// wrong for the commonest case a script meets: `-L` at the left edge printed `already on 0`, an
/// answer to a question the caller had not asked.
///
/// The flag parse is [`keymap::direction_of`](sprag_host::keymap::direction_of), not a table of its
/// own: this verb used to map `-L` straight to the wire word `"left"`, which was a third spelling of
/// one vocabulary and skipped [`PaneDir`] on the way.
fn select_pane(args: Vec<String>) -> io::Result<()> {
    let bad = |message: String| io::Error::new(io::ErrorKind::InvalidInput, message);
    let (session, rest) = scope_and_rest(args, "select-pane")?;
    let mut dir: Option<PaneDir> = None;
    let mut pane: Option<u64> = None;
    for arg in rest {
        let flag = sprag_host::keymap::direction_of(&arg);
        match flag {
            Some(named) => {
                if dir.is_some() {
                    return Err(bad(
                        "select-pane: -L/-R/-U/-D name one direction; give only one".to_owned(),
                    ));
                }
                dir = Some(named);
            }
            None => {
                if pane.is_some() {
                    return Err(bad(format!(
                        "select-pane: unexpected argument {arg:?} (one pane id, or one direction)"
                    )));
                }
                pane = Some(arg.parse::<u64>().map_err(|_| {
                    bad(format!(
                        "select-pane: {arg:?} is neither a direction flag nor a pane id"
                    ))
                })?);
            }
        }
    }
    let action_args = match (pane, dir) {
        (Some(pane), None) => json!({ "pane": pane }),
        (None, Some(dir)) => json!({ "dir": dir.wire_str() }),
        (None, None) => {
            return Err(bad(
                "select-pane needs a pane id or a direction: sprag select-pane PANE | -L|-R|-U|-D"
                    .to_owned(),
            ));
        }
        (Some(_), Some(_)) => {
            return Err(bad(
                "select-pane: a pane id and a direction name the same target two ways; give one"
                    .to_owned(),
            ));
        }
    };
    let mut conn = connect_scoped(session.as_deref())?;
    let answer = conn
        .call(
            "scene/invoke",
            scoped_invoke(
                session.as_deref(),
                mux_action_path(SELECT_PANE_ACTION),
                action_args,
            ),
        )
        .map_err(|error| {
            if error.kind() == io::ErrorKind::Other {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    match pane {
                        Some(pane) => format!("no pane {pane} in the current window"),
                        None => "this session's current window holds no panes".to_owned(),
                    },
                )
            } else {
                error
            }
        })?;
    let selected = answer["pane"].as_u64().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "select-pane: the daemon answered without a pane",
        )
    })?;
    println!(
        "{}",
        select_sentence(SelectHow::read(&answer, dir), dir, selected)
    );
    Ok(())
}

/// What `select-pane` prints, as a pure function of the daemon's answer — so every one of the four
/// outcomes is pinned by a unit test rather than only by whichever of them a live daemon can be
/// driven into.
///
/// `dir` is the direction asked for, if one was. It is an argument rather than a field of the outcome
/// because the outcome is the DAEMON's fact and the phrasing is this surface's: only the caller knows
/// it said `-L`, and only that makes "nothing to the left of 0" sayable.
///
/// **No arm can panic**, deliberately. An outcome word can only reach here from a daemon, so an
/// `at_edge` answered to a request that named a PANE is a wrong answer that parses — and this
/// degrades to the true half of it ("nothing moved") instead of turning a rendering into a crash,
/// which is the failure mode `sprag list-keys`' own flag table had until this round.
fn select_sentence(how: SelectHow, dir: Option<PaneDir>, pane: u64) -> String {
    match (how, dir) {
        (SelectHow::Moved, _) => format!("selected {pane}"),
        // Named for what the caller ASKED and could not have: an edge press is not "already on".
        (SelectHow::AtEdge, Some(dir)) => format!("nothing {} {pane}", dir.beyond()),
        // No remedy is offered for the float itself, and that is not an omission: NO CLI verb docks
        // a pane (`SET_FLOATING_ACTION` appears nowhere in this binary), so "dock it" would name an
        // action this surface cannot perform. What it can do is take a pane id, and it says so.
        (SelectHow::Untiled, _) => format!(
            "{pane} is floating, so nothing is beside it in any direction; name a pane to move to"
        ),
        (SelectHow::AlreadyActive | SelectHow::AtEdge, _) => format!("already on {pane}"),
    }
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
    let mut conn = connect(None)?;
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
    let mut conn = connect(None)?;
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

/// `resize-window -t SESSION [window] {-x COLS -y ROWS | -a | -A | -L/-R/-U/-D N | -u}`: PIN a
/// window's size, or un-pin it — tmux `resize-window`. Default window: the current one.
///
/// Five ways to say it, exactly one per call:
///
/// * `-x COLS -y ROWS` — that rectangle. BOTH or neither: a window is a shape somebody chose, and
///   completing half of one from whatever happens to be pinned would produce one nobody decided on.
///   A zero is refused here, where it was typed.
/// * `-a` / `-A` — the SMALLEST / LARGEST attached client, folded per dimension. What a user means by
///   "pin what I have right now", without reading numbers off `list-clients` and typing them back.
/// * `-L N` / `-R N` / `-U N` / `-D N` — relative. Each names an EDGE and how far to push it, so
///   `-L`/`-U` SHRINK the window (the left edge moves right; the top edge moves down) and `-R`/`-D`
///   grow it — `resize-pane`'s own convention, under tmux's flag names. Two axes may be named
///   together (`-R 4 -U 2`); two directions on ONE axis is a contradiction and is refused.
/// * `-u` — no rectangle at all. Spelled as `set-option -u` spells the same idea.
///
/// A call naming NOTHING is refused rather than treated as `-u`: an argument-less resize has named no
/// intent, and reading it as "un-pin" would throw a decision away on an empty command line.
///
/// # Why the CLI computes none of them
///
/// The last three are DESCRIPTIONS, and they become a rectangle only against the window's current
/// size and its clients' reported areas — facts the daemon holds. Reading those back to do the
/// arithmetic here would put a SECOND geometry model in a client, which is the defect this front has
/// spent three rounds removing. So the description crosses the wire and the ACTION resolves it
/// (`sprag_host::window::SizeRequest`), which is also why this prints the rectangle the DAEMON
/// answered rather than one computed locally — for `-a` and a relative form there is nothing to
/// compute locally, and for `-x`/`-y` printing back the request would be a guess that happened to be
/// right.
///
/// The adjustment is REQUIRED with its flag (`-U 2`, never a bare `-U`). tmux's bare form defaults to
/// 1 and takes its count as a trailing positional, which sprag cannot copy: the window target here is
/// a leading positional and window names are integers by default, so `resize-window -U 5` would be
/// genuinely ambiguous between "5 shorter" and "window 5".
///
/// # It pins; it does not switch to pinning
///
/// The size is stored whatever `window-size` currently says, and the note names the gap when the
/// policy in force is not `manual`. Pinning first and choosing to use it second is a legal order —
/// refusing it would make the natural sequence fail for no mechanical reason — but a value that
/// silently does nothing is what this front keeps finding, so it is reported rather than assumed
/// understood. The verb does NOT write the option: that would be a daemon-side command editing the
/// user's `config.toml`, which is [`set_option`]'s job and nobody else's.
fn resize_window(args: Vec<String>) -> io::Result<()> {
    let bad = |message: String| io::Error::new(io::ErrorKind::InvalidInput, message);
    let (session, rest) = target_and_rest(args, "resize-window")?;
    let mut window: Option<String> = None;
    let mut cols: Option<u64> = None;
    let mut rows: Option<u64> = None;
    // One slot per DIRECTION rather than a running total, so `-L 5 -R 3` is caught as the
    // contradiction it is instead of quietly becoming "2 narrower".
    let mut edges: [Option<u64>; 4] = [None; 4];
    const LEFT: usize = 0;
    const RIGHT: usize = 1;
    const UP: usize = 2;
    const DOWN: usize = 3;
    let mut from: Option<&'static str> = None;
    let mut unpin = false;
    let mut it = rest.into_iter();
    while let Some(arg) = it.next() {
        let mut count = |what: &str, flag: &str| -> io::Result<u64> {
            let value = it
                .next()
                .ok_or_else(|| bad(format!("resize-window: {flag} needs a {what}")))?;
            match value.parse::<u64>() {
                Ok(0) | Err(_) => Err(bad(format!(
                    "resize-window: {flag} {value:?} is not a positive {what}"
                ))),
                Ok(count) => Ok(count),
            }
        };
        match arg.as_str() {
            "-x" | "--width" => cols = Some(count("column count", "-x")?),
            "-y" | "--height" => rows = Some(count("row count", "-y")?),
            "-L" => edges[LEFT] = Some(count("column count", "-L")?),
            "-R" => edges[RIGHT] = Some(count("column count", "-R")?),
            "-U" => edges[UP] = Some(count("row count", "-U")?),
            "-D" => edges[DOWN] = Some(count("row count", "-D")?),
            // Two folds are a contradiction, not a last-writer-wins: caught HERE rather than by the
            // mode count below, because both spellings land in one slot and a second would otherwise
            // overwrite the first in silence. A test found exactly that.
            "-a" | "-A" if from.is_some() => {
                return Err(bad(
                    "resize-window: -a and -A name opposite folds — use one".to_owned(),
                ));
            }
            "-a" => from = Some("smallest"),
            "-A" => from = Some("largest"),
            "-u" | "--unset" => unpin = true,
            _ if window.is_none() => window = Some(arg),
            other => return Err(bad(format!("resize-window: unexpected argument {other:?}"))),
        }
    }
    for (one, other, axis) in [(LEFT, RIGHT, "-L and -R"), (UP, DOWN, "-U and -D")] {
        if edges[one].is_some() && edges[other].is_some() {
            return Err(bad(format!(
                "resize-window: {axis} move the same edge opposite ways — name one"
            )));
        }
    }
    let delta = |less: usize, more: usize| -> Option<i64> {
        match (edges[less], edges[more]) {
            (None, None) => None,
            // A `u64` count reaches the wire as an `i32` delta; a count past that range is a typo,
            // not a resize, and the clamp at the far end is the resolver's business, not this cast's.
            (less, more) => Some(
                i64::from(i32::try_from(more.unwrap_or(0)).unwrap_or(i32::MAX))
                    - i64::from(i32::try_from(less.unwrap_or(0)).unwrap_or(i32::MAX)),
            ),
        }
    };
    let (adjust_cols, adjust_rows) = (delta(LEFT, RIGHT), delta(UP, DOWN));

    // Exactly one spelling. Counted rather than matched, because five modes make a tuple match a wall
    // of arms that all mean the same thing, and the message a user needs is the same one either way.
    let named = [
        (cols.is_some() || rows.is_some()) as u8,
        (adjust_cols.is_some() || adjust_rows.is_some()) as u8,
        from.is_some() as u8,
        unpin as u8,
    ];
    match named.iter().sum::<u8>() {
        0 => {
            return Err(bad(
                "resize-window needs a size: -x COLS -y ROWS, -a, -A, -L/-R/-U/-D N, or -u to \
                 un-pin"
                    .to_owned(),
            ));
        }
        1 => {}
        _ => {
            return Err(bad(
                "resize-window: -x/-y, -a, -A, -L/-R/-U/-D and -u are five ways to name one size \
                 — use one"
                    .to_owned(),
            ));
        }
    }
    // Half a rectangle, checked after the mode so the message is about the dimensions rather than
    // about mixing spellings.
    if from.is_none() && adjust_cols.is_none() && adjust_rows.is_none() && !unpin {
        let (Some(_), Some(_)) = (cols, rows) else {
            return Err(bad(
                "resize-window needs both dimensions (-x COLS -y ROWS), or -u to un-pin".to_owned(),
            ));
        };
    }

    let mut conn = connect(None)?;
    require_session(&mut conn, &session)?;
    let mut action_args = json!({});
    if let Some(window) = &window {
        action_args["window"] = json!(window);
    }
    if let (Some(cols), Some(rows)) = (cols, rows) {
        action_args["cols"] = json!(cols);
        action_args["rows"] = json!(rows);
    }
    if let Some(delta) = adjust_cols {
        action_args["adjust_cols"] = json!(delta);
    }
    if let Some(delta) = adjust_rows {
        action_args["adjust_rows"] = json!(delta);
    }
    if let Some(policy) = from {
        action_args["from"] = json!(policy);
    }
    let target = window.as_deref().unwrap_or("the current window");
    let answer = scoped_window_action(
        &mut conn,
        &session,
        RESIZE_WINDOW_ACTION,
        action_args,
        // Two causes in one message, because the wire does not distinguish them — the same honesty
        // `resize_pane` already practises. Named in the order a user can act on: the size is what
        // they just typed, the window name is what they typed before it.
        &format!(
            "resize-window: could not resize {target} of session {session:?} — that size could not \
             be worked out (-a/-A need an attached client that has reported an area; -L/-R/-U/-D \
             need a window that already has one), or no window is named {target:?}"
        ),
    )?;
    // What the DAEMON pinned, not what was asked for: the two differ for every spelling but -x/-y,
    // and printing the request would be this CLI quietly claiming to have done the arithmetic.
    match (answer["cols"].as_u64(), answer["rows"].as_u64()) {
        (Some(cols), Some(rows)) => println!("pinned {target} to {cols}x{rows}"),
        _ => println!("un-pinned {target}"),
    }
    // The gap between storing a size and USING one, named the moment it exists rather than left for
    // the user to discover as "I resized and nothing moved". Read from the user's file here, the way
    // every option verb reads it — the daemon was never asked what it thinks the policy is.
    if !unpin {
        let policy = sprag_host::config::window_size();
        if policy != sprag_host::WindowSize::Manual {
            eprintln!(
                "sprag: note: window-size is {}, so the panes still follow the attached clients \
                 — `sprag set-option window-size manual` to lay them out over this size",
                policy.name()
            );
        }
    }
    Ok(())
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
) -> io::Result<Value> {
    conn.call(
        "scene/invoke",
        json!({ "session": session, "path": mux_action_path(action), "args": action_args }),
    )
    .map_err(|error| {
        if error.kind() == io::ErrorKind::Other {
            io::Error::new(io::ErrorKind::NotFound, message.to_owned())
        } else {
            error
        }
    })
}

/// `rename-pane [-t SESSION] PANE <NAME | --clear>`: give the pane with id PANE the name NAME, or
/// take its name away.
///
/// A pane's name is an ADDRESS, not a decoration — unique across the whole daemon, and the handle
/// an agent holds where a pane NUMBER goes stale the moment an earlier pane closes. See
/// [`RENAME_PANE_ACTION`] for the whole argument and every refusal.
///
/// PANE is an id, as it is for every pane verb here. The listing this reads back
/// ([`panes`]) leads with that id for exactly that reason, and the CLI's id — unlike the agent
/// surface's positional number — does not move when another pane closes, so there is nothing here
/// for a name to repair.
///
/// `--clear` rather than an empty NAME: `sprag rename-pane 3 ""` and a shell that expanded an unset
/// variable into nothing are the same command line, and only one of them meant it.
fn rename_pane(args: Vec<String>) -> io::Result<()> {
    let (session, rest) = scope_and_rest(args, "rename-pane")?;
    let mut rest = rest.into_iter();
    let pane = parse_pane_id(rest.next(), "rename-pane")?;
    let new = rest.next().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "rename-pane needs a NAME, or --clear to take the pane's name away",
        )
    })?;
    if let Some(other) = rest.next() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("rename-pane: unexpected argument {other:?}"),
        ));
    }
    let clearing = new == "--clear";
    let action_args = if clearing {
        json!({ "pane": pane })
    } else {
        json!({ "pane": pane, "name": new })
    };
    let mut conn = connect_scoped(session.as_deref())?;
    let answer: Value = conn
        .call(
            "scene/invoke",
            scoped_invoke(
                session.as_deref(),
                mux_action_path(RENAME_PANE_ACTION),
                action_args,
            ),
        )
        // The daemon knows WHICH of these it refused and cannot say so — `InvokeError::Rejected`
        // carries no payload (upstream PINION-PR82) — so the sentence lists the causes rather than
        // guessing one. It closes for every verb at once when that lands.
        //
        // CLEARING lists only one, because it can only fail one way: there is no name to be taken,
        // too long or malformed. A disjunction that named causes the request cannot have is worse
        // than no disjunction at all — it sends the reader looking for a mistake they did not make.
        .map_err(|error| {
            if error.kind() == io::ErrorKind::Other {
                let why = if clearing {
                    format!("rename-pane: no pane {pane}")
                } else {
                    format!(
                        "rename-pane: no pane {pane}, or {new:?} is already taken, blank, over 80 \
                         bytes, all digits, or contains a control character"
                    )
                };
                io::Error::new(io::ErrorKind::NotFound, why)
            } else {
                error
            }
        })?;
    // What the DAEMON recorded, never the argument that was sent: a name is trimmed on the way in,
    // so `rename-pane 0 " build "` lands as `build`, and echoing the argument would report a name
    // the pane does not have. Quoted for the listing's reason one function down.
    match answer["name"].as_str() {
        Some(recorded) => println!("pane {pane} is now {recorded:?}"),
        None => println!("pane {pane} has no name"),
    }
    Ok(())
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
    let mut conn = connect(None)?;
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
    let mut conn = connect(None)?;
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

/// `move-pane -t SESSION PANE -h|-v [-b] TARGET`: put PANE beside TARGET — tmux `move-pane`.
///
/// NEITHER window is named: both are derived from the two pane ids, so the same command re-places a
/// pane inside its own window and moves it into another. `-h` puts it right of TARGET, `-v` below,
/// `-b` on the other side — [`split_window`]'s flags, because it is [`split_window`]'s placement.
fn move_pane(args: Vec<String>) -> io::Result<()> {
    let bad = |message: String| io::Error::new(io::ErrorKind::InvalidInput, message);
    let (session, rest) = target_and_rest(args, "move-pane")?;
    let mut dir: Option<&'static str> = None;
    let mut before = false;
    let mut panes: Vec<u64> = Vec::new();
    for arg in rest {
        match arg.as_str() {
            "-h" | "-v" => {
                if dir.is_some() {
                    return Err(bad(
                        "move-pane: -h and -v name one axis; give only one".to_owned()
                    ));
                }
                dir = Some(if arg == "-h" {
                    "horizontal"
                } else {
                    "vertical"
                });
            }
            "-b" => before = true,
            other => {
                if panes.len() == 2 {
                    return Err(bad(format!("move-pane: unexpected argument {other:?}")));
                }
                panes.push(other.parse::<u64>().map_err(|_| {
                    bad(format!(
                        "move-pane: {other:?} is neither a flag nor a pane id"
                    ))
                })?);
            }
        }
    }
    let (&pane, &target) = match panes.as_slice() {
        [pane, target] => (pane, target),
        _ => {
            return Err(bad(
                "move-pane needs the pane to move and the pane to put it beside".to_owned(),
            ));
        }
    };
    // An axis is REQUIRED, unlike `split-window`'s bare form: a split with no axis has an append to
    // fall back on ("put a shell in this window"), while a move with no axis has not said anything
    // at all — the pane is already in a window, so `join-pane` is the verb for "somewhere in there".
    let dir = dir.ok_or_else(|| {
        bad(format!(
            "move-pane: pane {pane} needs an axis to land on beside pane {target} — -h (right) or \
             -v (below); use join-pane to append into a window instead"
        ))
    })?;
    let mut conn = connect(None)?;
    require_session(&mut conn, &session)?;
    let answer = conn.call(
        "scene/invoke",
        json!({
            "session": session,
            "path": mux_action_path(MOVE_PANE_ACTION),
            "args": { "pane": pane, "target": target, "dir": dir, "before": before },
        }),
    );
    match answer {
        Ok(answer) => {
            if answer["closed_source"].as_bool().unwrap_or(false) {
                println!("moved pane {pane} beside {target} (source window closed)");
            } else {
                println!("moved pane {pane} beside {target}");
            }
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::Other => Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "move-pane refused: session {session:?} has no pane {pane}, or pane {target} is not tiled there, or they are the same pane"
            ),
        )),
        Err(error) => Err(error),
    }
}

/// `swap-pane -t SESSION [PANE] <WITH | -L|-R|-U|-D>`: exchange two panes' positions — tmux
/// `swap-pane`.
///
/// PANE omitted means the session's ACTIVE pane. The partner is either a pane id or a direction,
/// exactly one of them; a direction at the edge of the layout prints "nothing to trade with" and
/// succeeds, which is what a key bound to this deserves.
fn swap_pane(args: Vec<String>) -> io::Result<()> {
    let bad = |message: String| io::Error::new(io::ErrorKind::InvalidInput, message);
    let (session, rest) = target_and_rest(args, "swap-pane")?;
    let mut dir: Option<&'static str> = None;
    let mut panes: Vec<u64> = Vec::new();
    for arg in rest {
        match arg.as_str() {
            "-L" | "-R" | "-U" | "-D" => {
                if dir.is_some() {
                    return Err(bad("swap-pane: give only one direction".to_owned()));
                }
                dir = Some(match arg.as_str() {
                    "-L" => "left",
                    "-R" => "right",
                    "-U" => "up",
                    _ => "down",
                });
            }
            other => {
                if panes.len() == 2 {
                    return Err(bad(format!("swap-pane: unexpected argument {other:?}")));
                }
                panes.push(other.parse::<u64>().map_err(|_| {
                    bad(format!(
                        "swap-pane: {other:?} is neither a flag nor a pane id"
                    ))
                })?);
            }
        }
    }
    let mut args = json!({});
    // The two shapes, and exactly one of them — the wire refuses "both" and "neither" as malformed,
    // so the CLI names the mistake here rather than letting it read as a daemon refusal.
    match (panes.as_slice(), dir) {
        ([pane, with], None) => {
            args["pane"] = json!(pane);
            args["with"] = json!(with);
        }
        ([with], None) => args["with"] = json!(with),
        ([pane], Some(dir)) => {
            args["pane"] = json!(pane);
            args["dir"] = json!(dir);
        }
        ([], Some(dir)) => args["dir"] = json!(dir),
        _ => {
            return Err(bad(
                "swap-pane takes a pane to swap with OR a direction (-L/-R/-U/-D), not both"
                    .to_owned(),
            ));
        }
    }
    let mut conn = connect(None)?;
    require_session(&mut conn, &session)?;
    let answer = conn.call(
        "scene/invoke",
        json!({ "session": session, "path": mux_action_path(SWAP_PANE_ACTION), "args": args }),
    );
    match answer {
        Ok(answer) => {
            let a = answer["a"].as_u64();
            match (
                answer["changed"].as_bool().unwrap_or(false),
                a,
                answer["b"].as_u64(),
            ) {
                (true, Some(a), Some(b)) => println!("swapped pane {a} with {b}"),
                (false, Some(a), Some(b)) => println!("pane {a} is already pane {b}"),
                (_, Some(a), None) => println!("pane {a} has nothing to trade with that way"),
                _ => println!("nothing to swap"),
            }
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::Other => Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("swap-pane refused: session {session:?} has no such pane, or it is not tiled"),
        )),
        Err(error) => Err(error),
    }
}

/// `zoom-pane -t SESSION [PANE] [-u|-Z]`: fill the window with one pane, or end that — tmux
/// `resize-pane -Z`.
///
/// PANE omitted means the session's ACTIVE pane. Neither flag TOGGLES, which is what a key bound to
/// this wants; `-Z` and `-u` are the explicit on and off, spelled as tmux spells them.
///
/// The window is never named: the daemon derives it from the pane, so `zoom-pane 7` zooms pane 7's
/// own window even when the session is looking at another one.
fn zoom_pane(args: Vec<String>) -> io::Result<()> {
    let bad = |message: String| io::Error::new(io::ErrorKind::InvalidInput, message);
    let (session, rest) = target_and_rest(args, "zoom-pane")?;
    let mut on: Option<bool> = None;
    let mut pane: Option<u64> = None;
    for arg in rest {
        match arg.as_str() {
            flag @ ("-u" | "-Z") => {
                if on.is_some() {
                    return Err(bad(
                        "zoom-pane: -Z and -u name one state; give only one".to_owned()
                    ));
                }
                on = Some(flag == "-Z");
            }
            other if pane.is_none() => {
                pane = Some(other.parse::<u64>().map_err(|_| {
                    bad(format!(
                        "zoom-pane: {other:?} is neither a flag nor a pane id"
                    ))
                })?);
            }
            other => return Err(bad(format!("zoom-pane: unexpected argument {other:?}"))),
        }
    }
    let mut args = json!({});
    if let Some(pane) = pane {
        args["pane"] = json!(pane);
    }
    if let Some(on) = on {
        args["on"] = json!(on);
    }
    let mut conn = connect(None)?;
    require_session(&mut conn, &session)?;
    let answer = conn.call(
        "scene/invoke",
        json!({ "session": session, "path": mux_action_path(ZOOM_PANE_ACTION), "args": args }),
    );
    match answer {
        Ok(answer) => {
            let pane = answer["pane"].as_u64().unwrap_or_default();
            // Four answers, four sentences, and no arm consults what this process ASKED for: the
            // daemon REFUSES a target it cannot zoom rather than answering one of these about it,
            // so each pair means exactly one thing. A verb whose success is ambiguous forces its
            // caller to print the causes the answer is consistent with, which is the shape R283
            // measured across fifteen failure paths and filed upstream.
            match (
                answer["zoomed"].as_bool().unwrap_or(false),
                answer["changed"].as_bool().unwrap_or(false),
            ) {
                (true, true) => println!("pane {pane} fills its window"),
                (true, false) => println!("pane {pane} already fills its window"),
                (false, true) => println!("pane {pane}'s window shows its arrangement again"),
                (false, false) => {
                    println!("pane {pane}'s window already showed its arrangement");
                }
            }
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::Other => Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "zoom-pane refused: session {session:?} has no pane {}, or it is floating",
                pane.map_or_else(|| "to be active on".to_owned(), |p| p.to_string())
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

    /// All four `select-pane` sentences, including the two a live daemon is hard to drive into —
    /// which is why the rendering is a pure function rather than four `println!`s inside the verb.
    ///
    /// The `-L`-at-the-edge line is the one this round CHANGED: it printed `already on 0`, an answer
    /// to a question the caller had not asked. Its integration test asserted that string, so the
    /// suite agreed with the wrong sentence for as long as it existed.
    #[test]
    fn select_pane_says_which_of_the_four_things_happened() {
        assert_eq!(
            select_sentence(SelectHow::Moved, None, 3),
            "selected 3",
            "the shape a script greps, unchanged",
        );
        assert_eq!(
            select_sentence(SelectHow::Moved, Some(PaneDir::Left), 3),
            "selected 3",
        );
        assert_eq!(
            select_sentence(SelectHow::AlreadyActive, None, 0),
            "already on 0",
        );
        // The four LITERALS, not `format!("nothing {} 0", dir.beyond())` — which is what this
        // asserted on its first draft and is not a test: it formats the string it then compares, so
        // `beyond()` could return anything and it would still pass (proved by changing one phrase and
        // watching it stay green). The register carries the same defect for `list-keys`' `-r` column.
        assert_eq!(
            PaneDir::ALL.map(|dir| select_sentence(SelectHow::AtEdge, Some(dir), 0)),
            [
                "nothing to the left of 0",
                "nothing to the right of 0",
                "nothing above 0",
                "nothing below 0",
            ]
            .map(str::to_owned),
            "the direction the caller ASKED for is what makes this sayable, and each of the four \
             reads as English",
        );
        assert_eq!(
            select_sentence(SelectHow::Untiled, Some(PaneDir::Up), 2),
            "2 is floating, so nothing is beside it in any direction; name a pane to move to",
            "and it advises only what THIS surface can do — no CLI verb docks a pane",
        );
        // A daemon answering a word its request could not produce degrades to the true half of it
        // rather than panicking in a rendering — the failure mode `list-keys`' flag table had.
        assert_eq!(select_sentence(SelectHow::AtEdge, None, 5), "already on 5",);
    }

    /// A slot the daemon does not serve reaches an operator as a SENTENCE with a remedy, not as a
    /// Rust enum variant — R283's finding, on the query path this time.
    ///
    /// The fault is the one a live daemon actually sends, captured rather than invented: R290 asked
    /// a running `sprag-term` for a path it has no slot for and read
    /// `{"code":-32602,"message":"Invalid params","data":"UnknownIntrospectPath"}` off the socket.
    /// The live half — that a daemon built at the PARENT commit refuses exactly this for
    /// `pane_processes.0` while answering `panes` cleanly — was proven with a worktree build and a
    /// control, and cannot be a standing test: this suite spawns the CURRENT daemon, which serves
    /// every slot this binary knows.
    #[test]
    fn an_unserved_slot_is_reported_as_an_old_daemon_and_nothing_else_is() {
        let fault = |data: Value| RpcFault {
            code: -32602,
            message: "Invalid params".to_owned(),
            data: Some(data),
        };
        let error = unknown_slot(
            "processes",
            "/x/y.0",
            &fault(json!("UnknownIntrospectPath")),
        )
        .expect("an unknown path is explained");
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert_eq!(
            error.to_string(),
            "processes: this daemon does not serve /x/y.0 — it is older than this `sprag`. \
             Restart it to bring it to this build — `sprag kill-server` (sessions are restored \
             from the durability snapshot)",
        );

        // THE CONTROL, and the reason this is a `data` match rather than a substring one: a
        // different refusal is left alone, and so is a fault whose rendered line merely mentions it.
        assert!(
            unknown_slot(
                "processes",
                "/x/y.0",
                &fault(json!("no session named \"x\""))
            )
            .is_none(),
            "another refusal keeps its own words",
        );
        assert!(
            unknown_slot(
                "processes",
                "/x/y.0",
                &fault(json!("a pane named UnknownIntrospectPath")),
            )
            .is_none(),
            "and a mention is not the refusal",
        );
        assert!(
            unknown_slot(
                "processes",
                "/x/y.0",
                &RpcFault {
                    code: -32602,
                    message: "Invalid params".to_owned(),
                    data: None,
                },
            )
            .is_none(),
            "a fault with no detail says nothing about the daemon's age",
        );
    }

    /// tmux's modifier spelling maps onto the wire's flags, and the KEY NAME passes through
    /// untouched — the two halves of [`parse_key_token`]'s contract.
    ///
    /// The pass-through is the half worth pinning: `Up` stays `Up` rather than becoming `ArrowUp`,
    /// because a tmux→W3C name table would have to be right about ~40 names to be worth having, and
    /// a half-right one turns a clean "unknown key" refusal into a key nobody asked for.
    #[test]
    fn a_key_token_reads_tmux_modifier_prefixes_and_keeps_the_name() {
        assert_eq!(
            parse_key_token("Enter").unwrap(),
            ("Enter".to_owned(), (false, false, false)),
            "an unprefixed token is the key name, verbatim",
        );
        assert_eq!(
            parse_key_token("C-c").unwrap(),
            ("c".to_owned(), (true, false, false)),
            "C- is ctrl",
        );
        assert_eq!(
            parse_key_token("M-x").unwrap(),
            ("x".to_owned(), (false, true, false)),
            "M- is alt",
        );
        assert_eq!(
            parse_key_token("C-M-S-Tab").unwrap(),
            ("Tab".to_owned(), (true, true, true)),
            "the prefixes stack, and the remainder is still a whole key name",
        );
        assert_eq!(
            parse_key_token("Up").unwrap().0,
            "Up",
            "no tmux->W3C name translation happens here",
        );
    }

    /// A token that is nothing but prefixes names no key, and says so here rather than travelling
    /// as an empty key the host would refuse with less to say.
    #[test]
    fn a_key_token_of_only_modifiers_is_refused_locally() {
        let error = parse_key_token("C-M-").expect_err("modifiers alone name no key");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(
            error.to_string().contains("names no key"),
            "and it says why: {error}",
        );
    }

    /// `-t` is OPTIONAL for a pane command (the module docs' rule), and everything after a bare
    /// `--` is payload — so a command run in a new pane may contain `-t` without this parse
    /// claiming it.
    #[test]
    fn a_pane_commands_scope_is_optional_and_stops_at_a_double_dash() {
        let split = |args: &[&str]| {
            scope_and_rest(args.iter().map(|a| (*a).to_owned()).collect(), "test").unwrap()
        };

        assert_eq!(
            split(&["7"]),
            (None, vec!["7".to_owned()]),
            "no -t is the default scope, not an error",
        );
        assert_eq!(
            split(&["-t", "work", "7"]),
            (Some("work".to_owned()), vec!["7".to_owned()]),
            "a named scope is taken out of the positionals",
        );
        assert_eq!(
            split(&["-t", "work", "--", "ssh", "-t", "host"]),
            (
                Some("work".to_owned()),
                vec![
                    "--".to_owned(),
                    "ssh".to_owned(),
                    "-t".to_owned(),
                    "host".to_owned()
                ]
            ),
            "the -t AFTER `--` belongs to the command being run, not to this parse",
        );
    }
}
