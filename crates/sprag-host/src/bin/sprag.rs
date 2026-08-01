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
//! sprag split-window [-t SESSION] [-h|-v [-b] PANE] [-- command…]
//!                                         divide PANE right (-h) / below (-v), or append with
//!                                         neither; print the new pane's id (tmux split-window)
//! sprag kill-pane [-t SESSION] PANE               close a pane (tmux kill-pane)
//! sprag resize-pane [-t SESSION] PANE -x COLS -y ROWS  resize a pane's PTY + emulator
//! sprag send-keys [-t SESSION] PANE [-l] KEY…     send W3C key names (or, with -l, literal text)
//! sprag capture-pane [-t SESSION] PANE [-p]       print a pane's retained output to stdout
//! sprag agent [-t SESSION] [PANE]                 what the AI agent in each pane is doing
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
//! ## What the pane verbs deliberately do NOT offer
//!
//! * **tmux's BARE `split-window -h` / `-v`** (the flag with no pane). The flags themselves are
//!   built — they drive [`sprag_host::wire::SPLIT_ACTION`], which divides a pane the caller names
//!   — but tmux's bare form means "split the CURRENT pane", and the daemon has no current pane to
//!   mean (the same fact that leaves `select-pane` below unbuilt). So the pane is named
//!   positionally and asking for a direction without one is refused with the reason.
//! * **`select-pane`.** There is no active-pane concept in the daemon to select: the pane-input
//!   `focus` action reports a focus EDGE to the child (DEC private mode 1004) on behalf of a client
//!   whose own focus moved — it does not make a pane current, and nothing reads such a fact. A
//!   `select-pane` built on it would send a program a focus-in report while no client focused it.
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
use sprag_host::keymap::{BoundAction, KeySpec, KeyTable};
use sprag_host::wire::{
    AGENT_MANIFESTS_SLOT, BREAK_PANE_ACTION, CLIENTS_SLOT, CLOSE_ACTION, FULL_TEXT_SLOT,
    JOIN_PANE_ACTION, KEY_ACTION, KILL_SESSION_ACTION, KILL_WINDOW_ACTION, NEW_SESSION_ACTION,
    NEW_WINDOW_ACTION, PANES_SLOT, PASTE_ACTION, RENAME_WINDOW_ACTION, RESIZE_ACTION,
    RESIZE_WINDOW_ACTION, SELECT_WINDOW_ACTION, SESSIONS_SLOT, SPAWN_ACTION, SPLIT_ACTION,
    TEXT_ACTION, WINDOWS_SLOT, events_slot_since, find_slot_for, project_slot_for, regex_slot_for,
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
        Some("attach") => attach(args.collect()),
        Some("kill-session") => kill_session(args.next()),
        Some("kill-server") => kill_server(args.collect()),
        Some("windows") => windows(args.collect()),
        Some("new-window") => new_window(args.collect()),
        Some("select-window") => select_window(args.collect()),
        Some("rename-window") => rename_window(args.collect()),
        Some("kill-window") => kill_window(args.collect()),
        Some("resize-window") => resize_window(args.collect()),
        Some("break-pane") => break_pane(args.collect()),
        Some("join-pane") => join_pane(args.collect()),
        Some("panes") => panes(args.collect()),
        Some("agent") => agent(args.collect()),
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
        return Err(bad_input(&format!(
            "bind-key: {key:?} needs an action (there are: detach-client, send-prefix, \
             split-window -h|-v [-b], select-pane -t :.+)"
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
         \x20             | kill-session NAME | kill-server [--purge]>\n\
         \x20      sprag <windows | new-window [name] | select-window NAME\n\
         \x20             | rename-window [window] NAME | kill-window [window]\n\
         \x20             | resize-window [window]\n\
         \x20                 <-x COLS -y ROWS | -a | -A | -L/-R/-U/-D N | -u>\n\
         \x20             | break-pane PANE [name] | join-pane PANE WINDOW> -t SESSION\n\
         \x20      sprag <panes | split-window [-h|-v [-b] PANE] [-- command…]\n\
         \x20             | kill-pane PANE\n\
         \x20             | resize-pane PANE -x COLS -y ROWS\n\
         \x20             | send-keys PANE [-l] KEY… | capture-pane PANE [-p]\n\
         \x20             | agent [PANE]> [-t SESSION]\n\
         \x20      sprag events [-t SESSION] [--since N] [-f]\n\
         \x20      sprag <list-keys | bind-key [-nr] [-T prefix|root] KEY ACTION…\n\
         \x20             | unbind-key [-n] [-T prefix|root] KEY>\n\
         \x20      sprag <show-options [-v] [NAME] | set-option [-u] NAME [VALUE]> [-g]"
    );
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
    let mut conn = connect()?;
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
    let mut conn = connect()?;
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
    let mut conn = connect()?;
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
/// refusal says nothing about panes, unlike the mux actions' pane-level `Rejected`.
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

/// `panes [-t SESSION]`: one line per pane of the scoped session's CURRENT window — tmux
/// `list-panes`. `ID: COLSxROWS  COMMAND`, plus the child's own window title in brackets when it
/// has set one.
///
/// The pane ID leads the line because it is what every other pane verb takes, so `sprag panes`
/// is the discovery step that makes the rest usable from a shell — `cut -d: -f1` yields exactly the
/// ids they accept. tmux prints a per-window INDEX and marks the active pane; sprag's id is
/// registry-unique (so it needs no window prefix) and there is no active pane to mark.
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
        // The child's live OSC 0/2 title, absent until it sets one — a DISPLAY name, never
        // identity, so it trails the command rather than replacing it.
        let title = match pane["title"].as_str() {
            Some(title) if !title.is_empty() => format!("  [{title}]"),
            _ => String::new(),
        };
        println!("{id}: {cols}x{rows}  {command}{title}");
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
        let rule = agent["rule"].as_str().unwrap_or("(none)");
        let seq = agent["seq"].as_u64().unwrap_or(0);
        println!("{id}: {state}  {name}  rule={rule}  seq={seq}");
        if wanted.is_some() {
            println!(
                "    `{rule}` is the rule that fired. If this verdict is wrong, redefine or \
                 disable that id in an [[agent]] block in config.toml — the daemon picks the edit \
                 up on its own."
            );
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
            other => {
                return Err(bad(format!(
                    "events: unexpected argument {other:?} (events [-t SESSION] [--since N] [-f])"
                )));
            }
        }
    }

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

    loop {
        let batch: Value = conn.call(
            "scene/query",
            scoped_params(
                session.as_deref(),
                mux_action_path(&events_slot_since(cursor)),
            ),
        )?;
        if batch["lost"].as_bool().unwrap_or(false) {
            eprintln!(
                "sprag: events: fell behind the daemon's log — some changes were dropped before \
                 this read. Re-read the world (`sprag panes`, `sprag windows`); what follows is \
                 only what survived."
            );
        }
        for event in batch["events"].as_array().into_iter().flatten() {
            let kind = event["type"].as_str().unwrap_or("?");
            // The subject key is named for WHAT it is, so a reader that has matched the type
            // already knows which slot to re-read. Printed as `TYPE<TAB>SUBJECT` — the shape
            // `sprag run`'s listing uses, which a script can cut.
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
        cursor = batch["next"].as_u64().unwrap_or(cursor);
        if !follow {
            return Ok(());
        }
        // Park. No deadline: waiting is this call's contract, not a hazard (see the doc above).
        //
        // THE WAIT'S ANSWER IS A SIGNAL, NOT A CURSOR, and the first version of this loop took it
        // for one. `waitFor` answers the revision it advanced TO, so adopting it as the cursor
        // skips whatever was recorded AT that revision — and the read that follows is `> cursor`,
        // so the skipped record is never offered again. It survived a manual drive because a spawn
        // happens to bump twice (the record lands above the wait's answer), and it would have lost
        // exactly the event this niche is about: `ChannelRegistry::announce` bumps ONCE and records
        // at that very revision, so every agent transition would have vanished. The cursor stays
        // where the last READ left it; the wait only says it is worth reading again.
        conn.set_read_deadline(None)?;
        let mut params = scoped_only(session.as_deref());
        params["since"] = json!(cursor);
        let _: Value = conn.call("scene/waitFor", params)?;
    }
}

/// `split-window [-t SESSION] [-h|-v [-b] PANE] [-- command…]`: add a pane to the scoped session's
/// current window and print its id — tmux `split-window`.
///
/// `--` introduces the argv the pane runs; absent, it is born with `$SHELL`, exactly as tmux's
/// bare `split-window`. The id is printed on stdout because it is the argument every other pane
/// verb takes, so a script can capture it (`pane=$(sprag split-window -v 3)`).
///
/// The direction and the pane it divides are INSEPARABLE here, which is the one place this
/// diverges from tmux and the divergence is forced: tmux's bare `-h` splits the CURRENT pane, and
/// sprag's daemon has no current-pane concept to mean (the same fact that leaves `select-pane`
/// unbuilt). So `-h` / `-v` take the pane POSITIONALLY — the convention `kill-pane PANE` and
/// `resize-pane PANE` already set — and naming neither is the direction-less append tmux's bare
/// form gives. Asking for one without the other is refused with the reason rather than guessed at.
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
    // The two halves of a directional split arrive together or not at all: a direction with no
    // pane has nothing to be relative to, and a pane with no direction has nothing to ask for.
    let placement = match (dir, pane) {
        (Some(dir), Some(pane)) => Some((dir, pane)),
        (None, None) => None,
        (Some(dir), None) => {
            return Err(bad(format!(
                "split-window: {dir_flag} needs the pane to divide (sprag has no current pane): \
                 sprag split-window {dir_flag} PANE",
                dir_flag = if dir == "horizontal" { "-h" } else { "-v" },
            )));
        }
        (None, Some(pane)) => {
            return Err(bad(format!(
                "split-window: pane {pane} needs an axis to be divided on — -h (right) or -v \
                 (below); omit both to append instead"
            )));
        }
    };
    if before && placement.is_none() {
        return Err(bad(
            "split-window: -b names which side of a target, so it needs -h or -v with a pane"
                .to_owned(),
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
            map.insert("pane".to_owned(), json!(pane));
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
                (Some((_, pane)), _) => format!(
                    "split-window: pane {pane} is not in the window's tiling (it exited, it is \
                     floating, or it belongs to another window), or the pane's command could not \
                     be run"
                ),
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
    let pane = parse_pane_id(rest.next(), "kill-pane")?;
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
            json!({ "id": pane }),
        ),
    );
    match answer {
        Ok(_) => {
            println!("killed pane {pane}");
            Ok(())
        }
        Err(error) if server_gone(&error) => {
            println!("killed pane {pane} (server ended)");
            Ok(())
        }
        // The session was pre-flighted, so the only refusal left is an unknown pane.
        Err(error) if error.kind() == io::ErrorKind::Other => Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "kill-pane: no pane {pane} in {}",
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
    let pane = pane.ok_or_else(|| bad("resize-pane needs a pane id".to_owned()))?;
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
            json!({ "id": pane, "cols": cols, "rows": rows }),
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
                    "resize-pane: no pane {pane} in {}, or {cols}x{rows} was refused",
                    scope_name(session.as_deref())
                ),
            )
        } else {
            error
        }
    })?;
    println!("resized pane {pane} to {cols}x{rows}");
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

    let mut conn = connect()?;
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
